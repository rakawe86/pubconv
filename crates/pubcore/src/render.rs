//! Turn WordArt and flowed linked text into vector paths on output pages.
//!
//! Text is drawn as glyph outlines (shaped by rustybuzz), so output looks
//! the same everywhere without embedding fonts. Coordinates are page points
//! with y pointing down; the PDF writer flips them.

use std::collections::BTreeMap;
use std::f64::consts::PI;
use std::sync::Arc;

use unicode_bidi::{bidi_class, BidiClass, BidiInfo};

use crate::contents::{self, Pages};
use crate::escher::{self, Anchor, EMU_PER_PT};
use crate::flow::{Flower, Styled};
use crate::fonts::{Font, FontLibrary};
use crate::quill;
use crate::textbox::{self, Chain};
use crate::wordart::{self, WordArt};
use crate::{PubFile, Result};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Seg {
    Move(f64, f64),
    Line(f64, f64),
    Cubic(f64, f64, f64, f64, f64, f64),
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Paint {
    pub fill: Option<[f64; 3]>,
    pub stroke: Option<([f64; 3], f64)>,
}

/// A picture in a form a PDF can hold directly.
#[derive(Debug, Clone, PartialEq)]
pub enum Codec {
    Rgb,
    Jpeg { components: u8 },
    /// PNG's own compressed rows (PDF reads them unchanged).
    Png { colors: u8 },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub codec: Codec,
    pub data: Vec<u8>,
}

/// A picture painted inside an item's path, instead of a flat colour.
#[derive(Debug, Clone)]
pub struct ImageFill {
    pub image: Arc<Image>,
    /// The area it covers on the page: left, top, width, height.
    pub area: [f64; 4],
    /// Degrees (clockwise) around the area's centre.
    pub rotation: f64,
    /// Repeat at this size (points) instead of stretching across the area.
    pub tile: Option<(f64, f64)>,
}

#[derive(Debug, Clone)]
pub struct Item {
    pub path: Vec<Seg>,
    pub paint: Paint,
    pub image: Option<ImageFill>,
    /// Invisible text laid over the outlines so the PDF stays searchable and copyable.
    pub text: Vec<HiddenText>,
}

/// One run of invisible (render mode 3) text; y is the baseline, y down.
#[derive(Debug, Clone, PartialEq)]
pub struct HiddenText {
    pub text: String,
    pub x: f64,
    pub y: f64,
    pub size: f64,
    /// Horizontal scale in percent, so the text spans the same width as the drawing.
    pub scale: f64,
    /// Degrees (clockwise) around (x, y).
    pub angle: f64,
}

fn is_rtl(c: char) -> bool {
    matches!(bidi_class(c), BidiClass::R | BidiClass::AL)
}

/// The parts of a line left to right on the page, and whether each reads right to left.
fn visual_runs(text: &str) -> Vec<(std::ops::Range<usize>, bool)> {
    if !text.chars().any(is_rtl) {
        return vec![(0..text.len(), false)];
    }
    let info = BidiInfo::new(text, None);
    let Some(para) = info.paragraphs.first() else { return Vec::new() };
    let (levels, runs) = info.visual_runs(para, para.range.clone());
    runs.into_iter().map(|r| { let rtl = levels[r.start].is_rtl(); (r, rtl) }).collect()
}

/// Text in the order it sits on the page, which is how PDF readers expect to
/// find right-to-left words (they turn them back when searching and copying).
fn visual_text(text: &str) -> String {
    visual_runs(text).into_iter()
        .map(|(r, rtl)| if rtl { text[r].chars().rev().collect() } else { text[r].to_string() })
        .collect()
}

/// Hidden text set in Helvetica (metrically the same as Arial), stretched to `width`.
fn hidden(text: &str, x: f64, y: f64, size: f64, width: f64) -> Option<HiddenText> {
    let text = text.trim();
    if text.is_empty() || size <= 0.0 {
        return None;
    }
    // every glyph of the hidden font is HIDDEN_ADVANCE em wide
    let natural = text.chars().count() as f64 * size * crate::pdf::HIDDEN_ADVANCE;
    let scale = if natural > 0.0 { (100.0 * width / natural).clamp(1.0, 1000.0) } else { 100.0 };
    Some(HiddenText { text: visual_text(text), x, y, size, scale, angle: 0.0 })
}

/// What we draw, by output page index (0-based), plus what LibreOffice must skip.
#[derive(Debug, Default)]
pub struct Plan {
    pub page_size: (f64, f64),
    pub drawings: BTreeMap<usize, Vec<Item>>,
    /// WordArt shapes we draw (LibreOffice's empty outline gets neutralised).
    pub wordart: Vec<WordArt>,
    /// Linked chains we flow (LibreOffice's duplicated text gets blanked).
    pub chains: Vec<Chain>,
    /// Things we left to LibreOffice, with the reason.
    pub skipped: Vec<String>,
}

const BLACK: [f64; 3] = [0.0, 0.0, 0.0];

/// Build the drawing plan for a file. Anything we can't place confidently is
/// left to LibreOffice untouched.
pub fn plan(file: &PubFile, fonts: &FontLibrary, flower: &Flower) -> Result<Plan> {
    let page_size = contents::page_size(file).unwrap_or((612.0, 792.0));
    let pages = contents::pages(file);
    let mut plan = Plan { page_size, ..Plan::default() };

    for w in wordart::wordart_shapes(file) {
        let (Some(anchor), Some(seq)) = (w.anchor, w.page_seq()) else {
            plan.skipped.push(format!("WordArt \"{}\": no page position (grouped?)", w.text));
            continue;
        };
        let on = pages.pages_of(seq);
        if on.is_empty() {
            plan.skipped.push(format!("WordArt \"{}\": page unknown", w.text));
            continue;
        }
        match wordart_paths(file, &w, anchor, page_size, fonts) {
            Some(items) => {
                for &p in &on {
                    plan.drawings.entry(p).or_default().extend(items.iter().cloned());
                }
                plan.wordart.push(w);
            }
            None => plan.skipped.push(format!("WordArt \"{}\": font not available", w.text)),
        }
    }

    for chain in textbox::chains(file) {
        match chain_items(file, &chain, &pages, page_size, fonts, flower) {
            Ok(Some(per_page)) => {
                for (p, items) in per_page {
                    plan.drawings.entry(p).or_default().extend(items);
                }
                plan.chains.push(chain);
            }
            Ok(None) => plan.skipped.push(format!("linked story {}: a box's page is unknown", chain.story_id)),
            Err(e) => plan.skipped.push(format!("linked story {}: {e}", chain.story_id)),
        }
    }

    // single text boxes with page-number fields (typically master-page footers):
    // LibreOffice prints the field as '#', so draw each page's number ourselves
    let fields = quill::page_number_fields(file);
    let flowed: std::collections::BTreeSet<u32> = plan.chains.iter().map(|c| c.story_id).collect();
    if !fields.is_empty() {
        let stories = quill::stories(file).unwrap_or_default();
        for b in textbox::text_boxes(file) {
            let (Some(at), false, false) = (fields.get(&b.story_id()), flowed.contains(&b.story_id()), b.is_table) else { continue };
            let Some(story) = stories.iter().find(|s| s.id == b.story_id()) else { continue };
            let chain = Chain { story_id: b.story_id(), boxes: vec![b.clone()] };
            let mut drawn = BTreeMap::new();
            for p in pages.pages_of(b.seq) {
                let text: String = story.text.chars().enumerate()
                    .map(|(i, c)| if at.contains(&i) { (p + 1).to_string() } else { c.to_string() })
                    .collect();
                match chain_items_text(file, &chain, &pages, page_size, fonts, flower, Some(&text)) {
                    Ok(Some(mut per_page)) => {
                        drawn.insert(p, per_page.remove(&p).unwrap_or_default());
                    }
                    _ => {
                        drawn.clear();
                        break;
                    }
                }
            }
            if drawn.is_empty() {
                plan.skipped.push(format!("page number in story {}: couldn't lay it out", b.story_id()));
                continue;
            }
            for (p, items) in drawn {
                plan.drawings.entry(p).or_default().extend(items);
            }
            plan.chains.push(chain);
        }
    }

    // text boxes in a turned group: LibreOffice draws them unturned
    let taken: std::collections::BTreeSet<u32> = plan.chains.iter().map(|c| c.story_id).collect();
    for b in textbox::text_boxes(file) {
        if b.rotation == 0.0 || taken.contains(&b.story_id()) || b.is_table {
            continue;
        }
        let chain = Chain { story_id: b.story_id(), boxes: vec![b] };
        if let Ok(Some(per_page)) = chain_items(file, &chain, &pages, page_size, fonts, flower) {
            for (p, items) in per_page {
                plan.drawings.entry(p).or_default().extend(items);
            }
            plan.chains.push(chain);
        }
    }

    // a single text box whose text doesn't fit: LibreOffice prints the hidden
    // overflow below the box, Publisher hides it. Draw what fits ourselves, for
    // stories that clearly overflow (2+ lines, so a borderline measurement
    // never cuts text Publisher shows)
    let taken: std::collections::BTreeSet<u32> = plan.chains.iter().map(|c| c.story_id).collect();
    {
        for b in textbox::text_boxes(file) {
            if taken.contains(&b.story_id()) || b.is_table {
                continue;
            }
            let chain = Chain { story_id: b.story_id(), boxes: vec![b.clone()] };
            let overflows = flower.flow_chain(file, &chain, fonts).is_ok_and(|f| f.overset.len() >= 2 && f.boxes.iter().any(|x| !x.lines.is_empty()));
            if !overflows {
                continue;
            }
            if let Ok(Some(per_page)) = chain_items(file, &chain, &pages, page_size, fonts, flower) {
                for (p, items) in per_page {
                    plan.drawings.entry(p).or_default().extend(items);
                }
                plan.chains.push(chain);
            }
        }
    }
    Ok(plan)
}

// ---------- glyph outlines ----------

/// Outlines of one line of text, in font units, y down, baseline at 0.
struct Shaped {
    contours: Vec<Seg>,
    advance: f64,
    /// Each glyph: where its outline starts in `contours`, its pen position and advance.
    glyphs: Vec<(usize, f64, f64)>,
}

struct Collector {
    out: Vec<Seg>,
    dx: f64,
    dy: f64,
    scale: f64,
}

impl Collector {
    fn p(&self, x: f32, y: f32) -> (f64, f64) {
        (self.dx + x as f64 * self.scale, self.dy - y as f64 * self.scale)
    }
}

impl ttf_parser::OutlineBuilder for Collector {
    fn move_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.p(x, y);
        self.out.push(Seg::Move(x, y));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        let (x, y) = self.p(x, y);
        self.out.push(Seg::Line(x, y));
    }
    fn quad_to(&mut self, x1: f32, y1: f32, x: f32, y: f32) {
        // elevate quadratic to cubic using the previous point
        let (px, py) = last_point(&self.out);
        let (qx, qy) = self.p(x1, y1);
        let (ex, ey) = self.p(x, y);
        self.out.push(Seg::Cubic(
            px + 2.0 / 3.0 * (qx - px), py + 2.0 / 3.0 * (qy - py),
            ex + 2.0 / 3.0 * (qx - ex), ey + 2.0 / 3.0 * (qy - ey),
            ex, ey,
        ));
    }
    fn curve_to(&mut self, x1: f32, y1: f32, x2: f32, y2: f32, x: f32, y: f32) {
        let (a, b) = self.p(x1, y1);
        let (c, d) = self.p(x2, y2);
        let (e, f) = self.p(x, y);
        self.out.push(Seg::Cubic(a, b, c, d, e, f));
    }
    fn close(&mut self) {
        self.out.push(Seg::Close);
    }
}

fn last_point(segs: &[Seg]) -> (f64, f64) {
    for s in segs.iter().rev() {
        match *s {
            Seg::Move(x, y) | Seg::Line(x, y) | Seg::Cubic(_, _, _, _, x, y) => return (x, y),
            Seg::Close => {}
        }
    }
    (0.0, 0.0)
}

/// Shape `text` with `font` at `size` points; origin at the baseline start.
fn shape(font: &Font, text: &str, size: f64) -> Option<Shaped> {
    let face = rustybuzz::Face::from_slice(font.data(), font.index())?;
    let scale = size / face.units_per_em() as f64;
    // as Publisher: no pair kerning below 14pt (the layout measured it that way)
    let no_kern = [rustybuzz::Feature::new(rustybuzz::ttf_parser::Tag::from_bytes(b"kern"), 0, ..)];
    let features: &[rustybuzz::Feature] = if crate::fonts::kerns_at(size) { &[] } else { &no_kern };
    let mut c = Collector { out: Vec::new(), dx: 0.0, dy: 0.0, scale };
    let mut pen = 0.0;
    let mut placed = Vec::new();
    // each direction is shaped on its own, so numbers and Latin inside Arabic stay readable
    for (range, rtl) in visual_runs(text) {
        let mut buf = rustybuzz::UnicodeBuffer::new();
        buf.push_str(&text[range]);
        if rtl {
            buf.set_direction(rustybuzz::Direction::RightToLeft);
        }
        buf.guess_segment_properties();
        let glyphs = rustybuzz::shape(&face, features, buf);
        for (info, pos) in glyphs.glyph_infos().iter().zip(glyphs.glyph_positions()) {
            c.dx = pen + pos.x_offset as f64 * scale;
            c.dy = -(pos.y_offset as f64) * scale;
            placed.push((c.out.len(), pen, pos.x_advance as f64 * scale));
            face.outline_glyph(ttf_parser::GlyphId(info.glyph_id as u16), &mut c);
            pen += pos.x_advance as f64 * scale;
        }
    }
    Some(Shaped { contours: c.out, advance: pen, glyphs: placed })
}

/// One line of plain text as a filled outline, baseline starting at (x, y).
pub fn text_item(font: &Font, text: &str, size: f64, x: f64, y: f64, color: [f64; 3]) -> Option<Item> {
    let s = shape(font, text, size)?;
    Some(Item { path: translate(&s.contours, x, y), paint: Paint { fill: Some(color), stroke: None }, image: None, text: Vec::new() })
}

fn translate(segs: &[Seg], dx: f64, dy: f64) -> Vec<Seg> {
    segs.iter().map(|s| match *s {
        Seg::Move(x, y) => Seg::Move(x + dx, y + dy),
        Seg::Line(x, y) => Seg::Line(x + dx, y + dy),
        Seg::Cubic(a, b, c, d, e, f) => Seg::Cubic(a + dx, b + dy, c + dx, d + dy, e + dx, f + dy),
        Seg::Close => Seg::Close,
    }).collect()
}

// ---------- flowed linked text ----------


fn chain_items(
    file: &PubFile,
    chain: &Chain,
    pages: &Pages,
    page_size: (f64, f64),
    fonts: &FontLibrary,
    flower: &Flower,
) -> Result<Option<BTreeMap<usize, Vec<Item>>>> {
    chain_items_text(file, chain, pages, page_size, fonts, flower, None)
}

/// As `chain_items`, drawing `text` instead of the story's own text.
fn chain_items_text(
    file: &PubFile,
    chain: &Chain,
    pages: &Pages,
    page_size: (f64, f64),
    fonts: &FontLibrary,
    flower: &Flower,
    text_override: Option<&str>,
) -> Result<Option<BTreeMap<usize, Vec<Item>>>> {
    let (flowed, styled) = flower.flow_chain_styled(file, chain, fonts, text_override)?;
    let mut out: BTreeMap<usize, Vec<Item>> = BTreeMap::new();
    let ascent = flowed.measurer_ascent;
    for bl in &flowed.boxes {
        let Some(b) = chain.boxes.get(bl.geom.chain_box) else { return Ok(None) };
        let on = pages.pages_of(b.seq);
        if on.is_empty() {
            return Ok(None);
        }
        // the flowed box: this column of the text box, with room kept for continued notices
        let g = &bl.geom;
        let (x0, y0) = (g.left + page_size.0 / 2.0, g.top + page_size.1 / 2.0);
        let base = y0 + g.inset[1];
        // one drawing per colour, in the order the colours first appear
        let mut by_color: Vec<([f64; 3], Vec<Seg>, Vec<HiddenText>)> = Vec::new();
        let width = g.width - g.inset[0] - g.inset[2];
        for line in &bl.lines {
            let shown = line.text.trim_end_matches(' ');
            let (lx, lw) = if line.width > 0.0 { (line.x, line.width) } else { (0.0, width) };
            let y = base + line.y + line.ascent;
            for (color, segs, run) in place_line(&styled, line.start, shown, x0 + g.inset[0] + lx, y, lw, line.align, line.paragraph_end) {
                let at = by_color.iter().position(|c| c.0 == color).unwrap_or_else(|| {
                    by_color.push((color, Vec::new(), Vec::new()));
                    by_color.len() - 1
                });
                by_color[at].1.extend(segs);
                by_color[at].2.extend(run);
            }
        }
        for (color, path, text) in by_color {
            let mut item = Item { path, paint: Paint { fill: Some(color), stroke: None }, image: None, text };
            turn_with_box(&mut item, b);
            for &p in &on {
                out.entry(p).or_default().push(item.clone());
            }
        }
    }
    // "(Continued on/from page N)", as Publisher draws them: 8pt italic Calibri,
    // "from" at the top left of the box, "on" at the bottom right
    let notice_font = fonts.face("Calibri", false, true)?;
    let page_no = |i: usize| chain.boxes.get(i).and_then(|b| pages.pages_of(b.seq).first().map(|p| p + 1));
    for (i, b) in chain.boxes.iter().enumerate() {
        let Some(&page) = pages.pages_of(b.seq).first() else { continue };
        let (x0, y0) = (b.anchor.xs + page_size.0 / 2.0, b.anchor.ys + page_size.1 / 2.0);
        let mut notices = Vec::new();
        let (from, on) = flowed.notices.get(i).copied().unwrap_or_default();
        // the notices sit on line slots: "from" on the first, "on" on the one
        // after the box's last line (Publisher probe and hard corpus)
        let first_base = y0 + b.inset[1] + ascent;
        if let (true, Some(n)) = (from, i.checked_sub(1).and_then(page_no)) {
            notices.push((format!("(Continued from page {n})"), false, first_base));
        }
        if let (true, Some(n)) = (on, page_no(i + 1)) {
            let area = b.anchor.height() - b.inset[1] - b.inset[3];
            let slot = crate::flow::continued_on_slot(area, flowed.spacing.line_pitch, flower.params.continued_on_slots);
            notices.push((format!("(Continued on page {n})"), true, y0 + b.inset[1] + slot + ascent));
        }
        for (words, right, y) in notices {
            let Some(s) = shape(&notice_font, &words, NOTICE_SIZE) else { continue };
            let x = if right { x0 + b.anchor.width() - b.inset[2] - s.advance } else { x0 + b.inset[0] };
            let text = hidden(&words, x, y, NOTICE_SIZE, s.advance).into_iter().collect();
            let mut item = Item { path: translate(&s.contours, x, y), paint: Paint { fill: Some(BLACK), stroke: None }, image: None, text };
            turn_with_box(&mut item, b);
            out.entry(page).or_default().push(item);
        }
    }
    Ok(Some(out))
}

/// Turn a text box's drawing the way its group turns the box, in page coordinates
/// (the anchor is centred on the page's centre, so that is added by the caller's page size).
fn turn_with_box(item: &mut Item, b: &textbox::TextBox) {
    if b.rotation == 0.0 {
        return;
    }
    let (sin, cos) = b.rotation.to_radians().sin_cos();
    let (cx, cy) = (b.page_center.0, b.page_center.1);
    let t = |x: f64, y: f64| (cx + (x - cx) * cos - (y - cy) * sin, cy + (x - cx) * sin + (y - cy) * cos);
    for s in &mut item.path {
        *s = match *s {
            Seg::Move(x, y) => { let p = t(x, y); Seg::Move(p.0, p.1) }
            Seg::Line(x, y) => { let p = t(x, y); Seg::Line(p.0, p.1) }
            Seg::Cubic(a, b, c, d, e, f) => {
                let (p, q, r) = (t(a, b), t(c, d), t(e, f));
                Seg::Cubic(p.0, p.1, q.0, q.1, r.0, r.1)
            }
            Seg::Close => Seg::Close,
        };
    }
    for h in &mut item.text {
        (h.x, h.y) = t(h.x, h.y);
        h.angle = b.rotation;
    }
}

/// Paragraph alignment as Quill stores it (checked against Publisher's PDFs).
mod align {
    pub const RIGHT: u32 = 1;
    pub const CENTER: u32 = 2;
    pub const JUSTIFY: u32 = 3;
    /// Also drawn justified by Publisher, last line left as usual.
    pub const JUSTIFY_ALT: u32 = 4;
}

/// Outlines and hidden text for one line in a column `width` wide starting at
/// `x`, one entry per style on the line: left, right, centred, or justified by
/// widening the word gaps. `start` is the line's first char in the story.
fn place_line(
    styled: &Styled,
    start: usize,
    text: &str,
    x: f64,
    y: f64,
    width: f64,
    alignment: Option<u32>,
    last_in_paragraph: bool,
) -> Vec<([f64; 3], Vec<Seg>, Option<HiddenText>)> {
    let pieces: Vec<_> = styled.pieces(start, text).into_iter()
        .filter_map(|(piece, run)| Some((piece, run, shape(&run.font, piece, run.size)?)))
        .collect();
    let total: f64 = pieces.iter().map(|p| p.2.advance).sum();
    let slack = (width - total).max(0.0);
    let gaps = text.matches(' ').count();
    let stretch = matches!(alignment, Some(align::JUSTIFY | align::JUSTIFY_ALT)) && !last_in_paragraph && gaps > 0 && slack > 0.0;
    let extra = if stretch { slack / gaps as f64 } else { 0.0 };
    let dx = match alignment {
        Some(align::RIGHT) => slack,
        Some(align::CENTER) => slack / 2.0,
        _ => 0.0,
    };
    let mut pen = x + dx;
    let mut out = Vec::new();
    for (piece, run, whole) in pieces {
        let spaces = piece.matches(' ').count();
        let hidden_run = hidden(piece, pen, y, run.size, whole.advance + spaces as f64 * extra);
        let segs = if !stretch {
            translate(&whole.contours, pen, y)
        } else {
            let mut segs = Vec::new();
            let mut at = 0;
            for (k, word) in piece.split(' ').enumerate() {
                if !word.is_empty() {
                    let before = if at == 0 { 0.0 } else { shape(&run.font, &piece[..at], run.size).map_or(0.0, |s| s.advance) };
                    if let Some(w) = shape(&run.font, word, run.size) {
                        segs.extend(translate(&w.contours, pen + before + k as f64 * extra, y));
                    }
                }
                at += word.len() + 1;
            }
            segs
        };
        out.push((run.color, segs, hidden_run));
        pen += whole.advance + spaces as f64 * extra;
    }
    out
}

/// Continued notices, measured from Publisher's PDFs: size, and baseline
/// below the top margin ("from") or above the bottom margin ("on").
const NOTICE_SIZE: f64 = 8.0;

// ---------- WordArt ----------

fn escher_rgb(v: Option<u32>) -> Option<[f64; 3]> {
    let v = v?;
    if v & 0xFF00_0000 != 0 {
        return None; // scheme/system colour index: not resolved yet
    }
    Some([(v & 0xFF) as f64 / 255.0, ((v >> 8) & 0xFF) as f64 / 255.0, ((v >> 16) & 0xFF) as f64 / 255.0])
}

const DARK_GREY: [f64; 3] = [0.25, 0.25, 0.25];

fn luminance(c: [f64; 3]) -> f64 {
    0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2]
}

/// One colour for a WordArt fill. Solid and pattern fills use their colour.
/// Gradients (fill types 4-7) run between two colours: use the one that stands
/// out more on a white page, so a colour-to-white gradient doesn't vanish.
/// Textures and pictures (2, 3) have no single colour: dark grey stays readable.
fn wordart_fill(w: &WordArt) -> [f64; 3] {
    let front = escher_rgb(w.fill_color);
    match w.fill_type.unwrap_or(0) {
        // no fill colour stored: Office's default, white
        0 if w.fill_color.is_none() => [1.0, 1.0, 1.0],
        0 => front.unwrap_or(BLACK),
        // shortcut: a pattern is drawn in its background colour, which covers most of it; tile the real bitmap if a file needs it.
        1 => escher_rgb(w.fill_back_color).or(front).unwrap_or(BLACK),
        2 | 3 => DARK_GREY,
        _ => {
            let both = [front, escher_rgb(w.fill_back_color)];
            let darkest = both.into_iter().flatten().min_by(|a, b| luminance(*a).total_cmp(&luminance(*b)));
            match darkest {
                Some(c) if luminance(c) < 0.8 => c,
                _ => escher_rgb(w.line_color).filter(|c| luminance(*c) < 0.8).unwrap_or(DARK_GREY),
            }
        }
    }
}

/// Colour stops along a gradient, from where the fill colour starts (s = 0) to its far end.
fn gradient_stops(w: &WordArt) -> Option<Vec<(f64, [f64; 3])>> {
    let fore = escher_rgb(w.fill_color)?;
    let back = escher_rgb(w.fill_back_color).unwrap_or([1.0, 1.0, 1.0]);
    // fillShadeColors: count, allocated, entry size, then (colour, position 16.16)
    const FILL_SHADE_COLORS: u16 = 0x0197;
    if let Some(b) = w.props.complex.get(&FILL_SHADE_COLORS) {
        let rd = |i: usize| b.get(i..i + 4).map(|v| u32::from_le_bytes([v[0], v[1], v[2], v[3]]));
        let n = b.get(0..2).map_or(0, |v| u16::from_le_bytes([v[0], v[1]]) as usize);
        let mut stops: Vec<(f64, [f64; 3])> = (0..n)
            .filter_map(|i| Some((rd(6 + 8 * i + 4)? as f64 / 65536.0, escher_rgb(rd(6 + 8 * i))?)))
            .collect();
        stops.sort_by(|a, b| a.0.total_cmp(&b.0));
        if stops.len() >= 2 {
            return Some(stops);
        }
    }
    // the far colour sits at the focus: 100 = far end, 0 = near end, 50 = middle; negative swaps them
    let focus = w.props.get_i32(wordart::pid::FILL_FOCUS).unwrap_or(0);
    let (near, far) = if focus < 0 { (back, fore) } else { (fore, back) };
    let p = (focus.abs() as f64 / 100.0).clamp(0.0, 1.0);
    Some(vec![(0.0, near), (p, far), (1.0, near)])
}

fn stop_color(stops: &[(f64, [f64; 3])], s: f64) -> [f64; 3] {
    for pair in stops.windows(2) {
        let ((a, ca), (b, cb)) = (pair[0], pair[1]);
        if s <= b {
            if b <= a {
                return cb;
            }
            let t = ((s - a) / (b - a)).clamp(0.0, 1.0);
            // Publisher blends in linear light (its PDFs: red to blue is a120a0 halfway, not 781e78)
            return [0, 1, 2].map(|i| (ca[i].powf(2.2) + (cb[i].powf(2.2) - ca[i].powf(2.2)) * t).powf(1.0 / 2.2));
        }
    }
    stops.last().map_or(BLACK, |x| x.1)
}

/// How far (0..1) a point of the shape's box is along its gradient. Fill types: 4 and 7
/// run in a straight line, 5 and 6 spread as a rectangle out of their focus area.
/// Measured from Publisher's PDFs: the angle turns in the unit box (so 45 degrees runs
/// corner to corner on any box), clockwise from the fill colour at the top, and a
/// negative angle starts from the opposite end.
fn gradient_position(w: &WordArt, u: f64, v: f64) -> f64 {
    let p = |pid| w.props.get_i32(pid);
    if matches!(w.fill_type, Some(5 | 6)) {
        let rect = [wordart::pid::FILL_TO_LEFT, wordart::pid::FILL_TO_TOP, wordart::pid::FILL_TO_RIGHT, wordart::pid::FILL_TO_BOTTOM]
            .map(|q| p(q).map_or(0.0, |x| x as f64 / 65536.0));
        let along = |x: f64, a: f64, b: f64| {
            if x < a { (a - x) / a.max(1e-6) } else if x > b { (x - b) / (1.0 - b).max(1e-6) } else { 0.0 }
        };
        return along(u, rect[0], rect[2]).max(along(v, rect[1], rect[3]));
    }
    let angle = p(wordart::pid::FILL_ANGLE).map_or(0.0, |x| x as f64 / 65536.0);
    let (sin, cos) = angle.abs().to_radians().sin_cos();
    let sign = if angle < 0.0 { -1.0 } else { 1.0 };
    let (ex, ey) = (sin * sign, -cos * sign);
    let span = (ex.abs() + ey.abs()).max(1e-6);
    (0.5 - ((u - 0.5) * ex + (v - 0.5) * ey) / span).clamp(0.0, 1.0)
}

fn gradient_image(w: &WordArt) -> Option<Image> {
    const SIDE: usize = 256;
    let stops = gradient_stops(w)?;
    let mut data = Vec::with_capacity(SIDE * SIDE * 3);
    for y in 0..SIDE {
        for x in 0..SIDE {
            let (u, v) = ((x as f64 + 0.5) / SIDE as f64, (y as f64 + 0.5) / SIDE as f64);
            let c = stop_color(&stops, gradient_position(w, u, v));
            data.extend(c.map(|k| (k * 255.0).round() as u8));
        }
    }
    Some(Image { width: SIDE as u32, height: SIDE as u32, codec: Codec::Rgb, data })
}

/// A stored picture, when a PDF can hold its bytes directly.
// shortcut: JPEG and plain 8-bit PNG only; other pictures (PNG with transparency, BMP, EMF) get the dark grey fill, decode them if a file needs it.
fn picture(file: &PubFile, index: u32) -> Option<Image> {
    let (rtype, inst, raw) = escher::blip(file, index)?;
    // record body: one or two 16-byte ids and a tag byte, then the file itself
    let two_ids = |one: u16| inst != one;
    match rtype {
        0xF01D | 0xF02A => {
            let skip = if two_ids(if rtype == 0xF01D { 0x46A } else { 0x6E2 }) { 33 } else { 17 };
            let jpeg = raw.get(skip..)?;
            let (width, height, components) = jpeg_size(jpeg)?;
            Some(Image { width, height, codec: Codec::Jpeg { components }, data: jpeg.to_vec() })
        }
        0xF01E => {
            let png = raw.get(if two_ids(0x6E0) { 33 } else { 17 }..)?;
            png_image(png)
        }
        _ => None,
    }
}

fn jpeg_size(d: &[u8]) -> Option<(u32, u32, u8)> {
    let mut i = 2;
    while i + 9 < d.len() {
        if d[i] != 0xFF {
            i += 1;
            continue;
        }
        let m = d[i + 1];
        if matches!(m, 0xC0..=0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF) {
            return Some((u16::from_be_bytes([d[i + 7], d[i + 8]]) as u32, u16::from_be_bytes([d[i + 5], d[i + 6]]) as u32, d[i + 9]));
        }
        if m == 0xFF {
            i += 1;
            continue;
        }
        i += 2 + u16::from_be_bytes([*d.get(i + 2)?, *d.get(i + 3)?]) as usize;
    }
    None
}

fn png_image(d: &[u8]) -> Option<Image> {
    let mut i = 8;
    let (mut width, mut height, mut colors) = (0, 0, 0);
    let mut idat = Vec::new();
    while i + 8 <= d.len() {
        let len = u32::from_be_bytes(d.get(i..i + 4)?.try_into().ok()?) as usize;
        let kind = d.get(i + 4..i + 8)?;
        let body = d.get(i + 8..i + 8 + len)?;
        match kind {
            b"IHDR" => {
                let (depth, color, interlace) = (*body.get(8)?, *body.get(9)?, *body.get(12)?);
                colors = match (depth, color, interlace) {
                    (8, 2, 0) => 3,
                    (8, 0, 0) => 1,
                    _ => return None,
                };
                width = u32::from_be_bytes(body.get(0..4)?.try_into().ok()?);
                height = u32::from_be_bytes(body.get(4..8)?.try_into().ok()?);
            }
            b"IDAT" => idat.extend_from_slice(body),
            b"IEND" => break,
            _ => {}
        }
        i += 12 + len;
    }
    (colors != 0 && !idat.is_empty()).then_some(Image { width, height, codec: Codec::Png { colors }, data: idat })
}

/// How a WordArt shape's depth is drawn, from its 3-D properties ([MS-ODRAW] 2.3.20-21).
struct Depth {
    back: f64,
    rx: f64,
    ry: f64,
    /// Parallel projection: depth slides along this (x, y per point of depth).
    skew: Option<(f64, f64)>,
    /// Perspective: where the eye is, relative to the box centre.
    eye: (f64, f64, f64),
    center: (f64, f64),
}

impl Depth {
    fn read(w: &WordArt, bw: f64, bh: f64, center: (f64, f64)) -> Option<Depth> {
        use wordart::pid::*;
        let p = |q| w.props.get_i32(q);
        if p(THREED_OBJECT_FLAGS).unwrap_or(0) & 0x8 == 0 {
            return None;
        }
        let back = p(THREED_EXTRUDE_BACK).map_or(457200.0, f64::from) / EMU_PER_PT;
        let deg = |q| p(q).map_or(0.0, |v| v as f64 / 65536.0).to_radians();
        let style = p(THREED_STYLE_FLAGS).unwrap_or(0x0004_0004);
        let parallel = style & 0x4 != 0;
        let skew_deg = p(THREED_SKEW_ANGLE).map_or(225.0, |v| v as f64 / 65536.0).to_radians();
        // Publisher draws the stored skew at half: 36pt of depth at 50% slides about 9pt
        let skew_amount = p(THREED_SKEW_AMOUNT).map_or(50.0, f64::from) / 200.0;
        let eye = (
            p(THREED_VIEW_X).map_or(1_250_000.0, f64::from) / EMU_PER_PT + p(THREED_ORIGIN_X).map_or(0.5, |v| v as f64 / 65536.0) * bw,
            p(THREED_VIEW_Y).map_or(-1_250_000.0, f64::from) / EMU_PER_PT + p(THREED_ORIGIN_Y).map_or(-0.5, |v| v as f64 / 65536.0) * bh,
            p(THREED_VIEW_Z).map_or(9_000_000.0, f64::from) / EMU_PER_PT,
        );
        Some(Depth {
            back,
            rx: deg(THREED_ROT_X),
            ry: deg(THREED_ROT_Y),
            skew: parallel.then_some((skew_amount * skew_deg.cos(), -skew_amount * skew_deg.sin())),
            eye,
            center,
        })
    }

    /// A point of the box (x, y) at `z` points behind the front, as drawn.
    fn project(&self, x: f64, y: f64, z: f64) -> (f64, f64) {
        let (x0, y0) = (x - self.center.0, y - self.center.1);
        let (sy, cy) = self.ry.sin_cos();
        let (x1, z1) = (x0 * cy + z * sy, -x0 * sy + z * cy);
        let (sx, cx) = self.rx.sin_cos();
        let (y2, z2) = (y0 * cx + z1 * sx, -y0 * sx + z1 * cx);
        let (px, py) = match self.skew {
            Some((dx, dy)) => (x1 + z2 * dx, y2 + z2 * dy),
            None => {
                let k = self.eye.2 / (self.eye.2 + z2).max(1.0);
                (self.eye.0 + (x1 - self.eye.0) * k, self.eye.1 + (y2 - self.eye.1) * k)
            }
        };
        (px + self.center.0, py + self.center.1)
    }
}

const SIDE_SHADES: usize = 6;

/// The sides of an extruded shape: one quad per outline edge, lit from the top left
/// and gathered per shade so the page holds a handful of paths, not thousands.
fn extrusion(polys: &[Vec<(f64, f64)>], d: &Depth, base: [f64; 3], turn: &dyn Fn(f64, f64) -> (f64, f64)) -> Vec<Item> {
    let mut shades: Vec<Vec<Seg>> = vec![Vec::new(); SIDE_SHADES];
    for poly in polys {
        for e in poly.windows(2) {
            let (a, b) = (e[0], e[1]);
            let (nx, ny) = (b.1 - a.1, a.0 - b.0);
            let len = (nx * nx + ny * ny).sqrt().max(1e-9);
            let light = (0.5 - 0.5 * (nx + ny) / len * std::f64::consts::FRAC_1_SQRT_2).clamp(0.0, 0.999);
            let mut quad = [d.project(a.0, a.1, 0.0), d.project(b.0, b.1, 0.0), d.project(b.0, b.1, d.back), d.project(a.0, a.1, d.back)];
            // all quads one way round, so overlaps add up instead of punching holes
            let area: f64 = (0..4).map(|i| { let (p, q) = (quad[i], quad[(i + 1) % 4]); p.0 * q.1 - q.0 * p.1 }).sum();
            if area < 0.0 {
                quad.reverse();
            }
            let path = &mut shades[(light * SIDE_SHADES as f64) as usize];
            for (i, p) in quad.iter().enumerate() {
                let (x, y) = turn(p.0, p.1);
                path.push(if i == 0 { Seg::Move(x, y) } else { Seg::Line(x, y) });
            }
            path.push(Seg::Close);
        }
    }
    shades.into_iter().enumerate().filter(|(_, p)| !p.is_empty()).map(|(i, path)| {
        let k = 0.3 + 0.5 * i as f64 / (SIDE_SHADES - 1) as f64;
        Item { path, paint: Paint { fill: Some(base.map(|c| c * k)), stroke: None }, image: None, text: Vec::new() }
    }).collect()
}

/// Flatten curves and split long lines so a warp can bend them smoothly.
fn polylines(segs: &[Seg], max_len: f64) -> Vec<Vec<(f64, f64)>> {
    let mut out: Vec<Vec<(f64, f64)>> = Vec::new();
    let mut cur: Vec<(f64, f64)> = Vec::new();
    let push_line = |cur: &mut Vec<(f64, f64)>, to: (f64, f64)| {
        let from = *cur.last().unwrap_or(&to);
        let n = ((((to.0 - from.0).powi(2) + (to.1 - from.1).powi(2)).sqrt() / max_len).ceil() as usize).max(1);
        for i in 1..=n {
            let t = i as f64 / n as f64;
            cur.push((from.0 + (to.0 - from.0) * t, from.1 + (to.1 - from.1) * t));
        }
    };
    for s in segs {
        match *s {
            Seg::Move(x, y) => {
                if cur.len() > 1 {
                    out.push(std::mem::take(&mut cur));
                }
                cur = vec![(x, y)];
            }
            Seg::Line(x, y) => push_line(&mut cur, (x, y)),
            Seg::Cubic(a, b, c, d, e, f) => {
                let (px, py) = *cur.last().unwrap_or(&(e, f));
                for i in 1..=8 {
                    let t = i as f64 / 8.0;
                    let mt = 1.0 - t;
                    let x = mt.powi(3) * px + 3.0 * mt * mt * t * a + 3.0 * mt * t * t * c + t.powi(3) * e;
                    let y = mt.powi(3) * py + 3.0 * mt * mt * t * b + 3.0 * mt * t * t * d + t.powi(3) * f;
                    push_line(&mut cur, (x, y));
                }
            }
            Seg::Close => {
                if let Some(&first) = cur.first() {
                    push_line(&mut cur, first);
                }
                if cur.len() > 1 {
                    out.push(std::mem::take(&mut cur));
                }
            }
        }
    }
    if cur.len() > 1 {
        out.push(cur);
    }
    out
}

/// Default bend for each warp when the file stores none (from the prototype).
fn default_adjust(st: u16) -> f64 {
    match st {
        138 | 139 | 152 | 153 => 0.45,
        140 | 141 | 154 | 155 | 160..=165 => 0.35,
        156 | 157 => 0.25,
        158 | 159 => 0.2,
        166 | 167 | 174 | 175 => 0.3,
        168 | 169 => 0.33,
        172 | 173 => 0.5,
        _ => 0.3,
    }
}

/// Warp a point of the text's ink box (u, v in 0..1, v down) into the shape
/// box (unit square, y down). `aspect` is box height / width.
fn warp(st: u16, a: f64, u: f64, v: f64) -> (f64, f64) {
    if matches!(st, 148 | 149) {
        // arch "pour": letters fill the half ring between the box's ellipse and half its size
        let (r, th) = if st == 148 { (1.0 - 0.5 * v, PI + PI * u) } else { (0.5 + 0.5 * v, PI - PI * u) };
        return (0.5 + 0.5 * r * th.cos(), 0.5 + 0.5 * r * th.sin());
    }
    let s = (PI * u).sin();
    let env = |top: f64, bot: f64| (u, top + v * (bot - top));
    match st {
        138 => env(a * (2.0 * u - 1.0).abs(), 1.0),
        139 => env(0.0, 1.0 - a * (2.0 * u - 1.0).abs()),
        140 => env(a * (2.0 * u - 1.0).abs(), 1.0 - a * (1.0 - (2.0 * u - 1.0).abs())),
        141 => env(a * (1.0 - (2.0 * u - 1.0).abs()), 1.0 - a * (2.0 * u - 1.0).abs()),
        152 => env(a * (1.0 - (PI * u / 2.0).sin()), 1.0 - a * (PI * u / 2.0).sin() * 0.5),
        153 => env(a * (PI * u / 2.0).sin() * 0.5, 1.0 - a * (1.0 - (PI * u / 2.0).sin())),
        154 => env(a * 1.6 * (1.0 - u), 1.0 - a * 0.4 * u),
        155 => env(a * 1.6 * u, 1.0 - a * 0.4 * (1.0 - u)),
        156 => env(a * (0.5 + 0.5 * (2.0 * PI * u).sin()), 1.0 - a * (0.5 - 0.5 * (2.0 * PI * u).sin())),
        157 => env(a * (0.5 - 0.5 * (2.0 * PI * u).sin()), 1.0 - a * (0.5 + 0.5 * (2.0 * PI * u).sin())),
        158 => env(a * (0.5 + 0.5 * (4.0 * PI * u).sin()), 1.0 - a * (0.5 - 0.5 * (4.0 * PI * u).sin())),
        159 => env(a * (0.5 - 0.5 * (4.0 * PI * u).sin()), 1.0 - a * (0.5 + 0.5 * (4.0 * PI * u).sin())),
        160 => env(a * (1.0 - s), 1.0 - a * (1.0 - s)),
        161 => env(a * s, 1.0 - a * s),
        162 => env(0.0, 1.0 - a * (1.0 - s)),
        163 => env(0.0, 1.0 - a * s),
        164 => env(a * (1.0 - s), 1.0),
        165 => env(a * s, 1.0),
        166 => env(a * s, 1.0 - a * (1.0 - s)),
        167 => env(a * s * 0.7, 1.0 - a * s * 0.7),
        168 => env(a * u, 1.0 - a * u),
        169 => env(a * (1.0 - u), 1.0 - a * (1.0 - u)),
        170 | 171 => {
            // perspective: one horizontal edge narrower
            let k = if st == 170 { v } else { 1.0 - v };
            let (l, r) = (a * (1.0 - k), 1.0 - a * (1.0 - k));
            (l + u * (r - l), v)
        }
        172 => env(a * (1.0 - u), 1.0 - a * u),
        173 => env(a * u, 1.0 - a * (1.0 - u)),
        174 => env(a * (1.0 - s), 1.0 - a * s),
        175 => env(a * s, 1.0 - a * (1.0 - s)),
        142 | 143 | 146 | 147 | 150 | 151 => {
            let thick = if matches!(st, 142 | 143 | 150 | 151) { 0.55 } else { 0.45 };
            let ang = u * 2.0 * PI - PI;
            let r = 1.0 - v * thick;
            (0.5 + 0.5 * r * ang.cos(), 0.5 + 0.5 * r * ang.sin())
        }
        _ => env(0.0, 1.0),
    }
}

/// Arch up (144) and down (145) as measured from Publisher's PDFs. The text runs
/// along half of the box's inscribed ellipse at the size that exactly fills it, with
/// the ellipse 0.30 em above the baseline, so letters stand out past the box. Each
/// letter is moved and turned whole, not bent, so thin script letters stay readable.
struct ArchCurve {
    up: bool,
    /// Half width and half height of the ellipse, in points.
    a: f64,
    b: f64,
    /// Ellipse length from its start, at equal steps of angle.
    lengths: Vec<f64>,
    /// Points per font unit.
    scale: f64,
}

impl ArchCurve {
    const STEPS: usize = 128;

    /// Font units (the layout's em is 100) from the baseline up to the ellipse.
    const ABOVE_BASELINE: f64 = 30.0;

    fn new(up: bool, a: f64, b: f64, text_width: f64) -> Self {
        let point = |t: f64| (a * t.cos(), b * t.sin());
        let mut lengths = vec![0.0];
        for i in 0..Self::STEPS {
            let (p, q) = (point(PI * i as f64 / Self::STEPS as f64), point(PI * (i + 1) as f64 / Self::STEPS as f64));
            lengths.push(lengths[i] + ((q.0 - p.0).powi(2) + (q.1 - p.1).powi(2)).sqrt());
        }
        let total = *lengths.last().unwrap_or(&1.0);
        ArchCurve { up, a, b, lengths, scale: total / text_width.max(1e-6) }
    }

    /// Offset from the box's centre for a point of a letter whose middle is `u` (0..1)
    /// along the text: `dx` font units from that middle, `y` font units below the baseline.
    fn place(&self, u: f64, dx: f64, y: f64) -> (f64, f64) {
        let total = *self.lengths.last().unwrap_or(&1.0);
        let along = u.clamp(0.0, 1.0) * total;
        let i = self.lengths.partition_point(|&l| l < along).clamp(1, Self::STEPS);
        let span = (self.lengths[i] - self.lengths[i - 1]).max(1e-9);
        let t = (i as f64 - 1.0 + (along - self.lengths[i - 1]) / span) / Self::STEPS as f64;
        // left to right: over the top when arching up, under the bottom when arching down
        let th = if self.up { PI + PI * t } else { PI - PI * t };
        let (x, y0) = (self.a * th.cos(), self.b * th.sin());
        let (nx, ny) = (th.cos() / self.a.max(1e-6), th.sin() / self.b.max(1e-6));
        let norm = (nx * nx + ny * ny).sqrt().max(1e-9);
        let (nx, ny) = (nx / norm, ny / norm);
        let above = (y + Self::ABOVE_BASELINE) * self.scale;
        let out = if self.up { -above } else { above };
        // along the arc, in the direction the text reads
        let (tx, ty) = if self.up { (-ny, nx) } else { (ny, -nx) };
        let side = dx * self.scale;
        (x + nx * out + tx * side, y0 + ny * out + ty * side)
    }
}

fn wordart_paths(file: &PubFile, w: &WordArt, a: Anchor, page: (f64, f64), fonts: &FontLibrary) -> Option<Vec<Item>> {
    let font = fonts.face(&w.font, w.bold, w.italic).ok()?;
    // lay out lines (centred), in arbitrary units; WordArt stretches to its box.
    // Vertical WordArt stacks one letter per line in the box turned on its side.
    // the vertical flag only means stacked letters on a shape turned a quarter (it's also set on upside-down text)
    let stacked = w.vertical && w.is_vertical();
    let text_lines: Vec<String> = if stacked {
        w.text.chars().filter(|c| *c != '\r').map(String::from).collect()
    } else {
        w.text.split('\r').map(String::from).collect()
    };
    let lines: Vec<Shaped> = text_lines.iter().filter_map(|l| shape(&font, l, 100.0)).collect();
    let max_w = lines.iter().map(|l| l.advance).fold(0.0, f64::max);
    let mut segs = Vec::new();
    // arches place letters whole: each one's outlines, with the text position of its middle
    let mut letters: Vec<(Vec<Vec<(f64, f64)>>, f64)> = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        let (dx, dy) = ((max_w - l.advance) / 2.0, i as f64 * 120.0);
        segs.extend(translate(&l.contours, dx, dy));
        for (k, &(start, pen, advance)) in l.glyphs.iter().enumerate() {
            let end = l.glyphs.get(k + 1).map_or(l.contours.len(), |g| g.0);
            let outline = polylines(&translate(&l.contours[start..end], dx, dy), max_w.max(1.0) / 80.0);
            if !outline.is_empty() {
                letters.push((outline, dx + pen + advance / 2.0));
            }
        }
    }
    let polys = polylines(&segs, max_w.max(1.0) / 80.0);
    let pts = polys.iter().flatten();
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for &(x, y) in pts {
        x0 = x0.min(x);
        y0 = y0.min(y);
        x1 = x1.max(x);
        y1 = y1.max(y);
    }
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    // stacked letters stay readable: no warp across the whole column
    let st = if w.shape_type == 0 || stacked { 136 } else { w.shape_type };
    let adj = w.adjust.map_or(default_adjust(st), |v| (v as f64 / 21600.0).clamp(0.05, 0.6));
    let (cx, cy) = (a.xs + page.0 / 2.0 + a.width() / 2.0, a.ys + page.1 / 2.0 + a.height() / 2.0);
    // Office stores a shape turned 45-135 degrees (either way) with its box already on
    // its side, keeping the centre; the letters are laid out in the box before turning
    let sideways = !stacked && matches!(w.rotation.rem_euclid(360.0), r if (45.0..135.0).contains(&r) || (225.0..315.0).contains(&r));
    let (bw, bh) = if sideways { (a.height(), a.width()) } else { (a.width(), a.height()) };
    // vertical WordArt: the stored frame is already the tall one Publisher
    // shows, and its 90 degree rotation is how Publisher marks the stacking
    let rotation = if stacked { 0.0 } else { w.rotation };
    let (ox, oy) = (cx - bw / 2.0, cy - bh / 2.0);
    let (sin, cos) = (rotation.to_radians().sin(), rotation.to_radians().cos());
    // rotate about the box centre (clockwise, y down)
    let turn = |px: f64, py: f64| (cx + (px - cx) * cos - (py - cy) * sin, cy + (px - cx) * sin + (py - cy) * cos);
    let arch = matches!(st, 144 | 145).then(|| ArchCurve::new(st == 144, bw / 2.0, bh / 2.0, max_w));
    let warped: Vec<Vec<(f64, f64)>> = match &arch {
        Some(c) => letters.iter().flat_map(|(outline, middle)| {
            let u = middle / max_w.max(1e-6);
            outline.iter().map(move |poly| poly.iter().map(|&(x, y)| {
                let (dx, dy) = c.place(u, x - middle, y);
                (cx + dx, cy + dy)
            }).collect::<Vec<_>>())
        }).collect(),
        None => polys.iter().map(|poly| poly.iter().map(|&(x, y)| {
            let (u, v) = ((x - x0) / (x1 - x0), (y - y0) / (y1 - y0));
            let (wx, wy) = warp(st, adj, u, v);
            (ox + wx * bw, oy + wy * bh)
        }).collect()).collect(),
    };
    let warped: Vec<Vec<(f64, f64)>> = warped.into_iter().map(|poly| poly.into_iter().map(|(x, y)| {
        (if w.flip_h { 2.0 * cx - x } else { x }, if w.flip_v { 2.0 * cy - y } else { y })
    }).collect()).collect();
    let depth = Depth::read(w, bw, bh, (cx, cy));
    let front = |x: f64, y: f64| match &depth {
        Some(d) => d.project(x, y, 0.0),
        None => (x, y),
    };
    let mut path = Vec::new();
    for poly in &warped {
        for (i, &(x, y)) in poly.iter().enumerate() {
            let (fx, fy) = front(x, y);
            let (rx, ry) = turn(fx, fy);
            path.push(if i == 0 { Seg::Move(rx, ry) } else { Seg::Line(rx, ry) });
        }
        path.push(Seg::Close);
    }
    let fill = wordart_fill(w);
    // Publisher lights a 3-D front: it comes out a little darker than the flat colour
    let fill = if depth.is_some() { fill.map(|c| c * 0.85) } else { fill };
    // gradients and pictures are painted through the letters, across the shape's box
    let area = [ox, oy, bw, bh];
    let image = if !w.filled {
        None
    } else {
        match w.fill_type.unwrap_or(0) {
            4..=7 => gradient_image(w).map(|i| ImageFill { image: Arc::new(i), area, rotation, tile: None }),
            2 => picture(file, w.props.get(wordart::pid::FILL_BLIP).unwrap_or(0)).map(|i| {
                // textures repeat at their natural size (Publisher's 96 dpi)
                let tile = Some((i.width as f64 * 0.75, i.height as f64 * 0.75));
                ImageFill { image: Arc::new(i), area, rotation, tile }
            }),
            3 => picture(file, w.props.get(wordart::pid::FILL_BLIP).unwrap_or(0))
                .map(|i| ImageFill { image: Arc::new(i), area, rotation, tile: None }),
            _ => None,
        }
    };
    // fLine (bit 3) counts only with its "use" bit (bit 19) set; the default line is thin black
    let line_flags = w.props.get(wordart::pid::LINE_FLAGS).unwrap_or(0);
    let line_off = line_flags & 0x8_0000 != 0 && line_flags & 0x8 == 0;
    let line_on = line_flags & 0x8_0000 != 0 && line_flags & 0x8 != 0;
    let line_width = w.props.get(wordart::pid::LINE_WIDTH).map_or(0.75, |v| v as f64 / EMU_PER_PT);
    let line = if line_off { None } else { escher_rgb(w.line_color).or(line_on.then_some(BLACK)) };
    let paint = if w.filled {
        Paint { fill: if image.is_some() { None } else { Some(fill) }, stroke: line.map(|c| (c, line_width)) }
    } else {
        Paint { fill: None, stroke: Some((line.unwrap_or(BLACK), 1.0)) }
    };
    // searchable copy of the headline across the WordArt's box; right-to-left
    // lines each get their own, since their words can't be read across a line break
    let size = (bh * 0.6).min(bw * 0.5).max(1.0);
    let text: Vec<HiddenText> = if w.text.chars().any(is_rtl) && w.text.contains('\r') {
        let lines: Vec<&str> = w.text.split('\r').collect();
        let step = bh / lines.len() as f64;
        lines.iter().enumerate()
            .filter_map(|(i, l)| hidden(l, ox, oy + step * (i as f64 + 0.75), (step * 0.6).min(size), bw))
            .collect()
    } else {
        hidden(&w.text.replace('\r', " "), ox, oy + bh * 0.75, size, bw).into_iter().collect()
    };
    let mut text = text;
    for t in &mut text {
        (t.x, t.y) = turn(t.x, t.y);
        t.angle = rotation;
    }
    let mut items = Vec::new();
    // fShadow is bit 1 of the shadow flags
    if w.props.get(wordart::pid::SHADOW_FLAGS).unwrap_or(0) & 0x2 != 0 {
        let shadow = escher_rgb(w.props.get(wordart::pid::SHADOW_COLOR)).unwrap_or([0.5, 0.5, 0.5]);
        let off = |pid| w.props.get_i32(pid).map_or(2.0, |v| v as f64 / EMU_PER_PT);
        let moved = translate(&path, off(wordart::pid::SHADOW_X), off(wordart::pid::SHADOW_Y));
        items.push(Item { path: moved, paint: Paint { fill: Some(shadow), stroke: None }, image: None, text: Vec::new() });
    }
    if let Some(d) = &depth {
        // the sides take the extrusion colour when it's switched on, else the shape's own
        let flags = w.props.get(wordart::pid::THREED_OBJECT_FLAGS).unwrap_or(0);
        let own = flags & 0x2_0000 != 0 && flags & 0x2 != 0;
        let base = own.then(|| escher_rgb(w.props.get(wordart::pid::THREED_EXTRUSION_COLOR))).flatten().unwrap_or(fill);
        items.extend(extrusion(&warped, d, base, &turn));
    }
    items.push(Item { path, paint, image, text });
    Some(items)
}
