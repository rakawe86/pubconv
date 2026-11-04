//! Turn WordArt and flowed linked text into vector paths on output pages.
//!
//! Text is drawn as glyph outlines (shaped by rustybuzz), so output looks
//! the same everywhere without embedding fonts. Coordinates are page points
//! with y pointing down; the PDF writer flips them.

use std::collections::BTreeMap;
use std::f64::consts::PI;

use crate::contents::{self, Pages};
use crate::escher::Anchor;
use crate::flow::{Flowed, Flower};
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

#[derive(Debug, Clone)]
pub struct Item {
    pub path: Vec<Seg>,
    pub paint: Paint,
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
    Some(HiddenText { text: text.to_string(), x, y, size, scale })
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
        let (Some(anchor), Some(seq)) = (w.anchor, w.seq) else {
            plan.skipped.push(format!("WordArt \"{}\": no page position (grouped?)", w.text));
            continue;
        };
        let on = pages.pages_of(seq);
        if on.is_empty() {
            plan.skipped.push(format!("WordArt \"{}\": page unknown", w.text));
            continue;
        }
        match wordart_paths(&w, anchor, page_size, fonts) {
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

    // a single text box whose text doesn't fit: LibreOffice prints the hidden
    // overflow below the box, Publisher hides it. Draw what fits ourselves, for
    // one-style stories that clearly overflow (2+ lines, so a borderline
    // measurement never cuts text Publisher shows)
    let taken: std::collections::BTreeSet<u32> = plan.chains.iter().map(|c| c.story_id).collect();
    if let Ok((styled, _)) = quill::styled_stories(file) {
        for b in textbox::text_boxes(file) {
            if taken.contains(&b.story_id()) || b.is_table {
                continue;
            }
            let Some(s) = styled.iter().find(|s| s.story.id == b.story_id()) else { continue };
            // a last run holding only the closing paragraph mark doesn't count
            let total = s.story.text.chars().count();
            let mut runs: Vec<&quill::CharStyle> = Vec::new();
            for (i, (end, c)) in s.char_runs.iter().enumerate() {
                let start = if i == 0 { 0 } else { s.char_runs[i - 1].0 };
                if end.saturating_sub(start) > 1 || *end < total {
                    runs.push(c);
                }
            }
            let uniform = runs.windows(2).all(|w| w[0] == w[1]) && s.para_runs.windows(2).all(|w| w[0].1 == w[1].1);
            let chain = Chain { story_id: b.story_id(), boxes: vec![b.clone()] };
            let overflows = uniform
                && flower.flow_chain(file, &chain, fonts).is_ok_and(|f| f.overset.len() >= 2 && f.boxes.iter().any(|x| !x.lines.is_empty()));
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
    let mut buf = rustybuzz::UnicodeBuffer::new();
    buf.push_str(text);
    buf.guess_segment_properties();
    // as Publisher: no pair kerning below 14pt (the layout measured it that way)
    let no_kern = [rustybuzz::Feature::new(rustybuzz::ttf_parser::Tag::from_bytes(b"kern"), 0, ..)];
    let features: &[rustybuzz::Feature] = if crate::fonts::kerns_at(size) { &[] } else { &no_kern };
    let glyphs = rustybuzz::shape(&face, features, buf);
    let mut c = Collector { out: Vec::new(), dx: 0.0, dy: 0.0, scale };
    let mut pen = 0.0;
    for (info, pos) in glyphs.glyph_infos().iter().zip(glyphs.glyph_positions()) {
        c.dx = pen + pos.x_offset as f64 * scale;
        c.dy = -(pos.y_offset as f64) * scale;
        face.outline_glyph(ttf_parser::GlyphId(info.glyph_id as u16), &mut c);
        pen += pos.x_advance as f64 * scale;
    }
    Some(Shaped { contours: c.out, advance: pen })
}

/// One line of plain text as a filled outline, baseline starting at (x, y).
pub fn text_item(font: &Font, text: &str, size: f64, x: f64, y: f64, color: [f64; 3]) -> Option<Item> {
    let s = shape(font, text, size)?;
    Some(Item { path: translate(&s.contours, x, y), paint: Paint { fill: Some(color), stroke: None }, text: Vec::new() })
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
    let (flowed, font): (Flowed, Font) = flower.flow_chain_text(file, chain, fonts, text_override)?;
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
        let base = y0 + g.inset[1] + ascent;
        let mut path = Vec::new();
        let mut text = Vec::new();
        let width = g.width - g.inset[0] - g.inset[2];
        for line in &bl.lines {
            let shown = line.text.trim_end_matches(' ');
            let (lx, lw) = if line.width > 0.0 { (line.x, line.width) } else { (0.0, width) };
            let y = base + line.y;
            if let Some((segs, run)) = place_line(&font, shown, flowed.size, x0 + g.inset[0] + lx, y, lw, flowed.align, line.paragraph_end) {
                path.extend(segs);
                text.extend(run);
            }
        }
        let item = Item { path, paint: Paint { fill: Some(BLACK), stroke: None }, text };
        for p in on {
            out.entry(p).or_default().push(item.clone());
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
            let item = Item { path: translate(&s.contours, x, y), paint: Paint { fill: Some(BLACK), stroke: None }, text };
            out.entry(page).or_default().push(item);
        }
    }
    Ok(Some(out))
}

/// Paragraph alignment as Quill stores it (checked against Publisher's PDFs).
mod align {
    pub const RIGHT: u32 = 1;
    pub const CENTER: u32 = 2;
    pub const JUSTIFY: u32 = 3;
    /// Also drawn justified by Publisher, last line left as usual.
    pub const JUSTIFY_ALT: u32 = 4;
}

/// Outline and hidden text for one line in a column `width` wide starting at
/// `x`: left, right, centred, or justified by widening the word gaps.
fn place_line(
    font: &Font,
    text: &str,
    size: f64,
    x: f64,
    y: f64,
    width: f64,
    alignment: Option<u32>,
    last_in_paragraph: bool,
) -> Option<(Vec<Seg>, Option<HiddenText>)> {
    let whole = shape(font, text, size)?;
    let slack = (width - whole.advance).max(0.0);
    let gaps = text.matches(' ').count();
    let stretch = match alignment {
        Some(align::JUSTIFY | align::JUSTIFY_ALT) => !last_in_paragraph,
        _ => false,
    };
    if stretch && gaps > 0 && slack > 0.0 {
        let extra = slack / gaps as f64;
        let mut segs = Vec::new();
        let mut start = 0;
        for (k, word) in text.split(' ').enumerate() {
            if !word.is_empty() {
                let before = if start == 0 { 0.0 } else { shape(font, &text[..start], size).map_or(0.0, |s| s.advance) };
                if let Some(w) = shape(font, word, size) {
                    segs.extend(translate(&w.contours, x + before + k as f64 * extra, y));
                }
            }
            start += word.len() + 1;
        }
        return Some((segs, hidden(text, x, y, size, whole.advance + slack)));
    }
    let dx = match alignment {
        Some(align::RIGHT) => slack,
        Some(align::CENTER) => slack / 2.0,
        _ => 0.0,
    };
    Some((translate(&whole.contours, x + dx, y), hidden(text, x + dx, y, size, whole.advance)))
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
        0 | 1 => front.unwrap_or(BLACK),
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
fn warp(st: u16, a: f64, u: f64, v: f64, aspect: f64) -> (f64, f64) {
    if matches!(st, 144 | 145 | 148 | 149) {
        return circular_arch(st, u, v, aspect);
    }
    let s = (PI * u).sin();
    let env = |top: f64, bot: f64| (u, top + v * (bot - top));
    match st {
        138 => env(a * (2.0 * u - 1.0).abs() * 2.0, 1.0),
        139 => env(0.0, 1.0 - a * (2.0 * u - 1.0).abs() * 2.0),
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

/// Arch up/down: text along a circular arc whose chord spans the box width,
/// letters `thick` of the box height tall (Publisher's look; not a half oval).
fn circular_arch(st: u16, u: f64, v: f64, aspect: f64) -> (f64, f64) {
    // work in units of box width; box height = aspect
    let h = aspect;
    let t = h * if matches!(st, 148 | 149) { 0.55 } else { 0.45 };
    let s = (h - t).max(1e-3); // sagitta of the outer edge
    let r_out = (s * s + 0.25) / (2.0 * s);
    let sweep = 2.0 * (0.5 / r_out).min(1.0).asin();
    let th = (u - 0.5) * sweep;
    let (x, y) = if matches!(st, 144 | 148) {
        let r = r_out - v * t; // letter tops on the outer edge
        (0.5 + r * th.sin(), r_out - r * th.cos())
    } else {
        let r = (r_out - t) + v * t; // letter tops toward the centre above
        (0.5 + r * th.sin(), (h - r_out) + r * th.cos())
    };
    (x, y / h)
}

fn wordart_paths(w: &WordArt, a: Anchor, page: (f64, f64), fonts: &FontLibrary) -> Option<Vec<Item>> {
    let font = fonts.face(&w.font, w.bold, w.italic).ok()?;
    // lay out lines (centred), in arbitrary units; WordArt stretches to its box.
    // Vertical WordArt stacks one letter per line in the box turned on its side.
    let text_lines: Vec<String> = if w.vertical {
        w.text.chars().filter(|c| *c != '\r').map(String::from).collect()
    } else {
        w.text.split('\r').map(String::from).collect()
    };
    let lines: Vec<Shaped> = text_lines.iter().filter_map(|l| shape(&font, l, 100.0)).collect();
    let max_w = lines.iter().map(|l| l.advance).fold(0.0, f64::max);
    let mut segs = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        segs.extend(translate(&l.contours, (max_w - l.advance) / 2.0, i as f64 * 120.0));
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
    let st = if w.shape_type == 0 { 136 } else { w.shape_type };
    let adj = w.adjust.map_or(default_adjust(st), |v| (v as f64 / 21600.0).clamp(0.05, 0.6));
    let (bw, bh) = (a.width(), a.height());
    let (cx, cy) = (a.xs + page.0 / 2.0 + bw / 2.0, a.ys + page.1 / 2.0 + bh / 2.0);
    // vertical WordArt: the stored frame is already the tall one Publisher
    // shows, and its 90 degree rotation is how Publisher marks the stacking
    let rotation = if w.vertical { 0.0 } else { w.rotation };
    let (ox, oy) = (cx - bw / 2.0, cy - bh / 2.0);
    let (sin, cos) = (rotation.to_radians().sin(), rotation.to_radians().cos());
    let mut path = Vec::new();
    for poly in &polys {
        for (i, &(x, y)) in poly.iter().enumerate() {
            let (u, v) = ((x - x0) / (x1 - x0), (y - y0) / (y1 - y0));
            let (wx, wy) = warp(st, adj, u, v, bh / bw);
            let (px, py) = (ox + wx * bw, oy + wy * bh);
            // rotate about the box centre (clockwise, y down)
            let (rx, ry) = (cx + (px - cx) * cos - (py - cy) * sin, cy + (px - cx) * sin + (py - cy) * cos);
            path.push(if i == 0 { Seg::Move(rx, ry) } else { Seg::Line(rx, ry) });
        }
        path.push(Seg::Close);
    }
    let fill = wordart_fill(w);
    let line = escher_rgb(w.line_color);
    let paint = if w.filled {
        Paint { fill: Some(fill), stroke: line.map(|c| (c, 0.75)) }
    } else {
        Paint { fill: None, stroke: Some((line.unwrap_or(BLACK), 1.0)) }
    };
    // searchable copy of the headline across the WordArt's box
    let words = w.text.replace('\r', " ");
    let size = (bh * 0.6).min(bw * 0.5).max(1.0);
    let text = hidden(&words, ox, oy + bh * 0.75, size, bw).into_iter().collect();
    Some(vec![Item { path, paint, text }])
}
