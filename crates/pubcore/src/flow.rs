//! Publisher-style text flow through a chain of linked text boxes.
//!
//! Publisher doesn't store where text breaks between linked boxes; it
//! re-flows on open. Box sizes, margins, font, size and paragraph spacing
//! come from the file; the rules below were measured from Publisher's own
//! PDFs and reproduce its line breaks exactly on the test files.

use hyphenation::{Hyphenator, Language, Load, Standard};

use crate::fonts::{Font, FontLibrary, Measurer};
use crate::quill::{self, LineSpacing};
use crate::textbox::{Chain, Obstacle, DEFAULT_INSET};
use crate::{Error, PubFile, Result};

#[derive(Debug, Clone, Copy)]
pub struct FlowParams {
    /// Height of one "single" line as a multiple of the font size, for fonts
    /// whose metrics can't be read (normally `Font::line_em` gives it per font;
    /// this is Arial's, from Publisher's PDFs: 1.19 lines x 1.0882 = 1.295 em pitch).
    pub single_line_em: f64,
    /// Only hyphenate when the gap at the line end would exceed this. Publisher says
    /// 0.25in (18pt); its line breaks on the hard corpus match best at 19.5pt.
    pub hyphen_zone: f64,
    pub hyphenate: bool,
    /// Line slots (of the story's line pitch) that "(Continued from page N)" takes
    /// at the top of a box and "(Continued on page N)" at the bottom. Measured with
    /// Publisher on boxes 15-80pt tall in 0.5pt steps, in Arial, Georgia, Times
    /// and Verdana: with the "on" notice, n lines need exactly (n + 1) pitches.
    pub continued_from_slots: f64,
    pub continued_on_slots: f64,
}

impl Default for FlowParams {
    fn default() -> Self {
        Self { single_line_em: 1.0882, hyphen_zone: 19.5, hyphenate: true, continued_from_slots: 2.0, continued_on_slots: 1.0 }
    }
}

/// Spacing for one story, in points.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spacing {
    pub line_pitch: f64,
    pub space_after: f64,
    /// Height of one single-spaced line: all a box's last line needs below its
    /// top, unless the box ends in a "continued on" notice.
    pub line_height: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoxGeom {
    pub left: f64,
    pub top: f64,
    pub width: f64,
    pub height: f64,
    /// Inner margins: left, top, right, bottom.
    pub inset: [f64; 4],
    /// Which box of the chain this is (a box with columns becomes one geometry per column).
    pub chain_box: usize,
    /// Areas the text flows around, in the same coordinates as `left`/`top`.
    pub obstacles: Vec<Obstacle>,
    /// The box ends in a "continued on" notice: then every line, the last one
    /// too, needs a full line pitch.
    pub whole_slots: bool,
}

impl BoxGeom {
    pub fn new(left: f64, top: f64, width: f64, height: f64) -> Self {
        Self { left, top, width, height, inset: [DEFAULT_INSET; 4], chain_box: 0, obstacles: Vec::new(), whole_slots: false }
    }

    fn text_width(&self) -> f64 {
        self.width - self.inset[0] - self.inset[2]
    }

    fn text_height(&self) -> f64 {
        self.height - self.inset[1] - self.inset[3]
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Line {
    /// Index (in chars) of the line's first character within the story.
    pub start: usize,
    /// Text as displayed, including a trailing '-' when hyphenated.
    pub text: String,
    pub paragraph_end: bool,
    /// Where the line sits across the box's text area when text wraps around
    /// a shape: offset from the left and width (0 = the whole width).
    pub x: f64,
    pub width: f64,
    /// Top of the line's row, from the top of the box's text area.
    pub y: f64,
    /// Baseline below the row's top: the tallest font on the line.
    pub ascent: f64,
    /// Paragraph alignment as stored (Quill value; see `render::align`).
    pub align: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BoxLines {
    pub geom: BoxGeom,
    pub lines: Vec<Line>,
}

impl BoxLines {
    pub fn start(&self) -> Option<usize> {
        self.lines.first().map(|l| l.start)
    }
}

#[derive(Debug, Clone)]
pub struct Flowed {
    pub boxes: Vec<BoxLines>,
    /// Lines that didn't fit in any box (Publisher hides them as overset text).
    pub overset: Vec<Line>,
    /// The story's first style: spacing, size and ascent (points). The continued notices sit on its slots.
    pub spacing: Spacing,
    pub size: f64,
    pub measurer_ascent: f64,
    /// Per chain box: whether Publisher shows its "(Continued from …)" and
    /// "(Continued on …)" notices (it drops them in boxes too short to hold them).
    pub notices: Vec<(bool, bool)>,
}

/// One stretch of text in one font: `end` is its exclusive char index in the story.
pub struct StyleRun {
    pub end: usize,
    pub font: Font,
    pub size: f64,
    pub color: [f64; 3],
    pub measure: Measurer,
    /// Height of one single-spaced line.
    pub line_height: f64,
}

/// One paragraph's settings; `end` as in `StyleRun`.
#[derive(Debug, Clone, Copy)]
pub struct ParaRun {
    pub end: usize,
    pub align: Option<u32>,
    pub line_spacing: Option<LineSpacing>,
    pub space_before: f64,
    pub space_after: f64,
}

/// A story's styles, ready to measure with.
pub struct Styled {
    pub runs: Vec<StyleRun>,
    pub paras: Vec<ParaRun>,
}

impl Styled {
    /// The run holding char `pos` (the last run past the story's end).
    pub fn run_at(&self, pos: usize) -> &StyleRun {
        let i = self.runs.partition_point(|r| r.end <= pos);
        &self.runs[i.min(self.runs.len() - 1)]
    }

    fn para_at(&self, pos: usize) -> &ParaRun {
        let i = self.paras.partition_point(|r| r.end <= pos);
        &self.paras[i.min(self.paras.len() - 1)]
    }

    /// Width of `s` when its first char is char `pos` of the story.
    pub fn width(&self, pos: usize, s: &str) -> f64 {
        self.pieces(pos, s).into_iter().map(|(piece, r)| r.measure.width(piece)).sum()
    }

    /// `s` split where the style changes, each part with its run.
    pub fn pieces<'a>(&'a self, pos: usize, s: &'a str) -> Vec<(&'a str, &'a StyleRun)> {
        if self.runs.len() == 1 {
            return vec![(s, &self.runs[0])];
        }
        let mut out = Vec::new();
        let (mut start, mut at, mut current) = (0, pos, self.run_at(pos));
        for (i, _) in s.char_indices() {
            let r = self.run_at(at);
            if !std::ptr::eq(r, current) && i > start {
                out.push((&s[start..i], current));
                start = i;
            }
            current = r;
            at += 1;
        }
        if start < s.len() {
            out.push((&s[start..], current));
        }
        out
    }

    /// Row pitch for a line: the tallest run on it, spaced as its paragraph says.
    fn pitch(&self, pos: usize, s: &str) -> (f64, f64) {
        let tallest = self.pieces(pos, s).into_iter().map(|(_, r)| r.line_height).fold(0.0, f64::max);
        let tallest = if tallest > 0.0 { tallest } else { self.run_at(pos).line_height };
        (self.spaced(pos, tallest), tallest)
    }

    fn spaced(&self, pos: usize, line_height: f64) -> f64 {
        match self.para_at(pos).line_spacing {
            Some(LineSpacing::Lines(x)) => x * line_height,
            Some(LineSpacing::Points(p)) => p,
            None => line_height,
        }
    }
}

pub struct Flower {
    pub params: FlowParams,
    dict: Option<Standard>,
}

impl Flower {
    pub fn new(params: FlowParams) -> Self {
        let dict = params.hyphenate.then(|| {
            let mut d = Standard::from_embedded(Language::EnglishUS).expect("embedded en-US patterns");
            d.minima = (2, 2); // Publisher hyphenates e.g. "friend-ly"
            d
        });
        Self { params, dict }
    }

    /// Flow a linked chain using only what's stored in the file.
    pub fn flow_chain(&self, file: &PubFile, chain: &Chain, fonts: &FontLibrary) -> Result<Flowed> {
        self.flow_chain_with_font(file, chain, fonts).map(|(f, _)| f)
    }

    /// As `flow_chain`, also returning the font used (for drawing).
    pub fn flow_chain_with_font(&self, file: &PubFile, chain: &Chain, fonts: &FontLibrary) -> Result<(Flowed, Font)> {
        self.flow_chain_text(file, chain, fonts, None)
    }

    /// As `flow_chain_with_font`, flowing `text` (e.g. with page numbers filled
    /// in) instead of the story's own text, in the story's style.
    pub fn flow_chain_text(&self, file: &PubFile, chain: &Chain, fonts: &FontLibrary, text: Option<&str>) -> Result<(Flowed, Font)> {
        self.flow_chain_styled(file, chain, fonts, text).map(|(f, s)| {
            let font = s.runs[0].font.clone();
            (f, font)
        })
    }

    /// As `flow_chain_text`, also returning every style the story uses (for drawing).
    // shortcut: with `text` (page numbers filled in) styles stay on the story's char positions, so a style change after a multi-digit number lands a char or two late.
    pub fn flow_chain_styled(&self, file: &PubFile, chain: &Chain, fonts: &FontLibrary, text: Option<&str>) -> Result<(Flowed, Styled)> {
        let (stories, defaults) = quill::styled_stories(file)?;
        let s = stories.iter().find(|s| s.story.id == chain.story_id)
            .ok_or(Error::Malformed("chain refers to a missing story"))?;
        let story_len = text.map_or(s.story.text.chars().count(), |t| t.chars().count());
        let styled = self.styles(s, &defaults, fonts, story_len)?;
        let first = &styled.runs[0];
        let m = &first.measure;
        let line_pitch = styled.spaced(0, first.line_height);
        let spacing = Spacing { line_pitch, space_after: styled.paras[0].space_after, line_height: first.line_height };
        let mut boxes = Vec::new();
        let mut notices = Vec::new();
        for (i, b) in chain.boxes.iter().enumerate() {
            // Publisher keeps a notice only if a whole line slot of text fits with
            // it, deciding "on" first: "on" needs 2 slots, "from" 3, both 4
            let area = b.anchor.height() - b.inset[1] - b.inset[3] + 1e-6;
            let (from_slots, on_slots) = (self.params.continued_from_slots, self.params.continued_on_slots);
            let on = b.continued_on && area >= (on_slots + 1.0) * line_pitch;
            let from = b.continued_from && area >= (from_slots + 1.0 + if on { on_slots } else { 0.0 }) * line_pitch;
            notices.push((from, on));
            let top = b.inset[1] + if from { from_slots * line_pitch } else { 0.0 };
            // "on" sits in the last whole line slot of the box (slots counted from
            // the top margin); text ends above it
            let bottom = if on {
                b.anchor.height() - b.inset[1] - continued_on_slot(area, line_pitch, on_slots)
            } else {
                b.inset[3]
            };
            // columns split the text area evenly, with the gap between them
            let n = b.columns.max(1) as f64;
            let text_w = b.anchor.width() - b.inset[0] - b.inset[2];
            let col_w = ((text_w - (n - 1.0) * b.column_gap) / n).max(1.0);
            for c in 0..b.columns.max(1) {
                boxes.push(BoxGeom {
                    left: b.anchor.xs + b.inset[0] + c as f64 * (col_w + b.column_gap),
                    top: b.anchor.ys,
                    width: col_w,
                    height: b.anchor.height(),
                    inset: [0.0, top, 0.0, bottom],
                    chain_box: i,
                    obstacles: b.obstacles.clone(),
                    whole_slots: on,
                });
            }
        }
        let (size, ascent) = (m.size, m.ascent());
        let mut flowed = self.flow(text.unwrap_or(&s.story.text), &boxes, &styled, spacing, size, ascent);
        flowed.notices = notices;
        Ok((flowed, styled))
    }

    /// A story's runs with their fonts loaded. Neighbouring runs that look the
    /// same become one, so text kerns across them, as it does in one run.
    fn styles(&self, s: &quill::StyledStory, defaults: &quill::Defaults, fonts: &FontLibrary, len: usize) -> Result<Styled> {
        let mut runs: Vec<StyleRun> = Vec::new();
        let mut last: Option<quill::CharStyle> = None;
        let mut loaded: Vec<(String, bool, bool, Font)> = Vec::new();
        let char_runs: Vec<(usize, quill::CharStyle)> = if s.char_runs.is_empty() {
            vec![(len, quill::CharStyle::default())]
        } else {
            s.char_runs.clone()
        };
        for (end, c) in &char_runs {
            let cs = c.over(&defaults.char);
            if last.as_ref() == Some(&cs) {
                if let Some(r) = runs.last_mut() {
                    r.end = *end;
                }
                continue;
            }
            let name = cs.font.clone().unwrap_or_else(|| "Arial".into());
            let font = match loaded.iter().find(|f| f.0 == name && f.1 == cs.bold && f.2 == cs.italic) {
                Some(f) => f.3.clone(),
                None => {
                    let f = fonts.face(&name, cs.bold, cs.italic)?;
                    loaded.push((name, cs.bold, cs.italic, f.clone()));
                    f
                }
            };
            let size = cs.size.unwrap_or(10.0);
            let line_height = font.line_em().unwrap_or(self.params.single_line_em) * size;
            runs.push(StyleRun { end: *end, measure: font.sized(size), font, size, color: cs.color.unwrap_or([0.0, 0.0, 0.0]), line_height });
            last = Some(cs);
        }
        // the last run holding only the closing paragraph mark would otherwise set the story's end
        if let Some(r) = runs.last_mut() {
            r.end = r.end.max(len);
        }
        let mut paras: Vec<ParaRun> = s.para_runs.iter().map(|(end, p)| {
            let p = p.over(&defaults.para);
            ParaRun { end: *end, align: p.align, line_spacing: p.line_spacing, space_before: p.space_before.unwrap_or(0.0), space_after: p.space_after.unwrap_or(0.0) }
        }).collect();
        if paras.is_empty() {
            let p = &defaults.para;
            paras.push(ParaRun { end: len, align: p.align, line_spacing: p.line_spacing, space_before: p.space_before.unwrap_or(0.0), space_after: p.space_after.unwrap_or(0.0) });
        }
        Ok(Styled { runs, paras })
    }

    /// Flow `text` through `boxes` in chain order. Each box breaks lines at
    /// its own width, so text that jumps to a wider or narrower box re-wraps
    /// there, as Publisher does.
    pub fn flow(&self, text: &str, boxes: &[BoxGeom], m: &Styled, spacing: Spacing, size: f64, ascent: f64) -> Flowed {
        let chars: Vec<char> = text.chars().collect();
        let mut pos = 0;
        let mut out = Vec::new();
        for geom in boxes {
            let (width, avail) = (geom.text_width(), geom.text_height());
            let (left, top) = (geom.left + geom.inset[0], geom.top + geom.inset[1]);
            let wraps = !geom.obstacles.is_empty();
            let mut lines = Vec::new();
            let mut y = 0.0;
            while pos < chars.len() {
                // a paragraph's space before, except at the top of a box
                let starts_para = pos == 0 || chars[pos - 1] == '\r';
                let before = if starts_para && y > 0.0 { m.para_at(pos).space_before } else { 0.0 };
                let row_top = y + before;
                // the last line needs only a single line's height, or a whole pitch
                // when a "continued on" notice follows it (measured with Publisher)
                let fits = |height: f64, pitch: f64| row_top + if geom.whole_slots { pitch } else { height } <= avail + 1e-6;
                let start = m.run_at(pos);
                if !fits(start.line_height, m.spaced(pos, start.line_height)) {
                    break;
                }
                // the row's free parts, left to right (all of it when nothing wraps)
                let row = (top + row_top, top + row_top + start.measure.glyph_height());
                let segments = if wraps { free_segments(left, left + width, row, &geom.obstacles) } else { vec![(left, left + width)] };
                let mut para_end = false;
                let mut pitch: f64 = 0.0;
                let mut row_lines = Vec::new();
                let mut after = pos;
                for (a, b) in segments {
                    // beside a shape a part only takes text that fits; elsewhere a too-long word still goes in
                    let Some((mut line, next)) = self.line_in(&chars, after, m, b - a, wraps) else { continue };
                    if wraps {
                        (line.x, line.width) = (a - left, b - a);
                    }
                    pitch = pitch.max(m.pitch(line.start, &line.text).0);
                    para_end = line.paragraph_end;
                    row_lines.push(line);
                    after = next;
                    if para_end || after >= chars.len() {
                        break; // a new paragraph starts on the next row
                    }
                }
                let row_height = row_lines.iter().map(|l| m.pitch(l.start, &l.text).1).fold(0.0, f64::max);
                // a tall run later in the line can push it out of the box
                if !row_lines.is_empty() && !lines.is_empty() && !fits(row_height, pitch) {
                    break;
                }
                let line_ascent = row_lines.iter()
                    .flat_map(|l| m.pieces(l.start, &l.text))
                    .map(|(_, r)| r.measure.ascent())
                    .fold(0.0, f64::max);
                for mut line in row_lines {
                    line.y = row_top;
                    line.ascent = line_ascent;
                    line.align = m.para_at(line.start).align;
                    lines.push(line);
                }
                pos = after;
                // nothing fit this row (it's all taken by a shape): step down one line
                let step = if pitch > 0.0 { pitch } else { m.spaced(pos.min(chars.len().saturating_sub(1)), start.line_height) };
                let last = pos.saturating_sub(1).min(chars.len().saturating_sub(1));
                y = row_top + step + if para_end { m.para_at(last).space_after } else { 0.0 };
            }
            out.push(BoxLines { geom: geom.clone(), lines });
        }
        let last_width = boxes.last().map_or(0.0, BoxGeom::text_width);
        let mut overset = Vec::new();
        while let Some((line, next)) = self.next_line(&chars, pos, m, last_width) {
            overset.push(line);
            pos = next;
        }
        Flowed { boxes: out, overset, spacing, size, measurer_ascent: ascent, notices: Vec::new() }
    }

    /// Greedy line breaking at one width; trailing spaces hang past the right edge.
    pub fn break_lines(&self, text: &str, m: &Styled, width: f64) -> Vec<Line> {
        let chars: Vec<char> = text.chars().collect();
        let mut lines = Vec::new();
        let mut pos = 0;
        while let Some((line, next)) = self.next_line(&chars, pos, m, width) {
            lines.push(line);
            pos = next;
        }
        lines
    }

    /// As `next_line`; when `strict`, nothing rather than a first word that
    /// doesn't fit (then only a hyphenated part of it may go in).
    fn line_in(&self, chars: &[char], pos: usize, m: &Styled, width: f64, strict: bool) -> Option<(Line, usize)> {
        let (line, next) = self.next_line(chars, pos, m, width)?;
        if !strict || m.width(pos, line.text.trim_end_matches(' ')) <= width + 1e-6 {
            return Some((line, next));
        }
        // the line is one word too wide for this part: try the start of it, hyphenated
        let word: String = chars[pos..].iter().take_while(|&&c| c != ' ' && c != '\r').collect();
        let (head, shown) = self.breaks(&word).into_iter().rev().map(|(b, add)| (&word[..b], hyphenated(&word[..b], add)))
            .find(|(_, shown)| m.width(pos, shown) <= width)?;
        let n = head.chars().count();
        Some((plain_line(pos, shown, false), pos + n))
    }

    /// The line starting at char `pos`, and where the next one starts.
    fn next_line(&self, chars: &[char], pos: usize, m: &Styled, width: f64) -> Option<(Line, usize)> {
        if pos >= chars.len() {
            return None;
        }
        let para_end = chars[pos..].iter().position(|&c| c == '\r').map_or(chars.len(), |i| pos + i);
        let mut line = String::new();
        for word in tokens(&chars[pos..para_end]) {
            if !line.is_empty() && m.width(pos, (line.clone() + &word).trim_end_matches(' ')) > width {
                let len = line.chars().count();
                return Some(match self.hyphen_split(pos, &line, &word, m, width) {
                    Some((head, shown)) => {
                        let next = pos + len + head.chars().count();
                        (plain_line(pos, format!("{line}{shown}"), false), next)
                    }
                    None => (plain_line(pos, line, false), pos + len),
                });
            }
            line.push_str(&word);
        }
        Some((plain_line(pos, line, true), para_end + 1)) // past the paragraph mark
    }

    /// Longest break of `word` that fits after `line`: the part that goes on
    /// this line and how it's shown (with its hyphen).
    fn hyphen_split(&self, pos: usize, line: &str, word: &str, m: &Styled, width: f64) -> Option<(String, String)> {
        let core = word.trim_end_matches(' ');
        let breaks = self.breaks(core);
        // a hard hyphen is an ordinary break; the zone only limits added ones
        let hard = breaks.iter().any(|&(_, add)| !add);
        if !hard && width - m.width(pos, line.trim_end_matches(' ')) <= self.params.hyphen_zone {
            return None;
        }
        breaks.into_iter().rev().map(|(b, add)| (&core[..b], hyphenated(&core[..b], add)))
            .find(|(_, shown)| m.width(pos, &format!("{line}{shown}")) <= width)
            .map(|(head, shown)| (head.to_string(), shown))
    }

    /// Where `word` may break, ascending, as (byte index, whether a hyphen is
    /// added). As in Publisher: a word with hard hyphens breaks only after them,
    /// and never right after an apostrophe.
    fn breaks(&self, word: &str) -> Vec<(usize, bool)> {
        let hard: Vec<(usize, bool)> = word.char_indices()
            .filter(|&(i, c)| c == '-' && i > 0 && i + 1 < word.len())
            .map(|(i, _)| (i + 1, false)).collect();
        if !hard.is_empty() {
            return hard;
        }
        let Some(dict) = self.dict.as_ref() else { return hard };
        dict.hyphenate(word).breaks.into_iter()
            .filter(|&b| !word[..b].ends_with(['\'', '\u{2019}']))
            .map(|b| (b, true)).collect()
    }
}

fn plain_line(start: usize, text: String, paragraph_end: bool) -> Line {
    Line { start, text, paragraph_end, x: 0.0, width: 0.0, y: 0.0, ascent: 0.0, align: None }
}

/// Where a box's "(Continued on page N)" line slot starts, from the top of its
/// text area `area` tall: the last whole slot of `pitch` (it takes `slots` of them).
pub fn continued_on_slot(area: f64, pitch: f64, slots: f64) -> f64 {
    ((area / pitch + 1e-6).floor() - slots).max(0.0) * pitch
}

/// The start of a broken word as shown at the line end.
fn hyphenated(head: &str, add: bool) -> String {
    if add { format!("{head}-") } else { head.to_string() }
}

/// Parts of `left..right` that no obstacle covers in the row `top..bottom`,
/// left to right; slivers under a point wide are dropped.
fn free_segments(left: f64, right: f64, (top, bottom): (f64, f64), obstacles: &[Obstacle]) -> Vec<(f64, f64)> {
    let mut free = vec![(left, right)];
    for (lo, hi) in obstacles.iter().filter_map(|o| o.in_row(top, bottom)) {
        free = free.into_iter().flat_map(|(a, b)| {
            let mut parts = Vec::new();
            if lo > a {
                parts.push((a, lo.min(b)));
            }
            if hi < b {
                parts.push((hi.max(a), b));
            }
            parts
        }).collect();
    }
    free.retain(|(a, b)| b - a >= 1.0);
    free
}

/// Words with their trailing spaces attached.
fn tokens(para: &[char]) -> Vec<String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < para.len() {
        let mut j = i;
        while j < para.len() && para[j] != ' ' {
            j += 1;
        }
        while j < para.len() && para[j] == ' ' {
            j += 1;
        }
        out.push(para[i..j].iter().collect());
        i = j;
    }
    out
}
