//! Quill: the text engine stream (`Quill/QuillSub/CONTENTS`). Text lives in
//! one TEXT chunk as UTF-16; STRS gives each story's length and SYID its id.
//! Chunk layout follows libmspub's parseQuill / parseQuillChunkReference.

use crate::{u16_at, u32_at, utf16le, Error, PubFile, Result};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub name: String,
    pub id: u16,
    pub name2: String,
    pub offset: usize,
    pub length: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Story {
    /// Story id as referenced from shapes (Contents block 0x27).
    pub id: u32,
    /// Text with paragraph marks as '\r'.
    pub text: String,
}

pub fn chunks(data: &[u8]) -> Vec<Chunk> {
    let mut out = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut off = 0x18usize;
    while off != 0xFFFF_FFFF && seen.insert(off) {
        let (Some(num), Some(next)) = (u16_at(data, off + 2), u32_at(data, off + 4)) else { break };
        let mut pos = off + 8;
        for _ in 0..num {
            let (Some(name), Some(id), Some(name2), Some(o), Some(l)) = (
                data.get(pos + 2..pos + 6),
                u16_at(data, pos + 6),
                data.get(pos + 12..pos + 16),
                u32_at(data, pos + 16),
                u32_at(data, pos + 20),
            ) else {
                return out;
            };
            out.push(Chunk {
                name: String::from_utf8_lossy(name).into_owned(),
                id,
                name2: String::from_utf8_lossy(name2).into_owned(),
                offset: o as usize,
                length: l as usize,
            });
            pos += 24;
        }
        off = next as usize;
    }
    out
}

fn strs(data: &[u8], c: &Chunk) -> Option<Vec<usize>> {
    let n = u32_at(data, c.offset)? as usize;
    let start = c.offset + 4 + u32_at(data, c.offset + 4)? as usize;
    (0..n).map(|i| u32_at(data, start + 4 * i).map(|v| v as usize)).collect()
}

fn syid(data: &[u8], c: &Chunk) -> Option<Vec<u32>> {
    let n = u32_at(data, c.offset + 4)? as usize;
    (0..n).map(|i| u32_at(data, c.offset + 8 + 4 * i)).collect()
}

// ---------- styles (libmspub getCharacterStyle / getParagraphStyle) ----------

const EMU_PER_PT: f64 = 12700.0;

mod style_id {
    pub const BOLD: u8 = 0x02;
    pub const ITALIC: u8 = 0x03;
    pub const TEXT_SIZE: u8 = 0x0C;
    pub const FONT_INDEX_CONTAINER: u8 = 0x24;
    /// Text colour: a list holding an index into the "PL  " chunk.
    pub const COLOR_INDEX_CONTAINER: u8 = 0x44;
    pub const PARA_ALIGNMENT: u8 = 0x04;
    pub const PARA_SPACE_BEFORE: u8 = 0x12;
    pub const PARA_SPACE_AFTER: u8 = 0x13;
    pub const PARA_LINE_SPACING: u8 = 0x34;
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CharStyle {
    pub font: Option<String>,
    /// Points.
    pub size: Option<f64>,
    pub bold: bool,
    pub italic: bool,
    pub color: Option<[f64; 3]>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LineSpacing {
    /// Multiple of single spacing (Publisher shows "sp").
    Lines(f64),
    Points(f64),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParaStyle {
    pub align: Option<u32>,
    /// Points.
    pub space_before: Option<f64>,
    pub space_after: Option<f64>,
    pub line_spacing: Option<LineSpacing>,
}

impl ParaStyle {
    /// Values set here win; the rest come from `base`.
    pub fn over(&self, base: &ParaStyle) -> ParaStyle {
        ParaStyle {
            align: self.align.or(base.align),
            space_before: self.space_before.or(base.space_before),
            space_after: self.space_after.or(base.space_after),
            line_spacing: self.line_spacing.or(base.line_spacing),
        }
    }
}

impl CharStyle {
    pub fn over(&self, base: &CharStyle) -> CharStyle {
        CharStyle {
            font: self.font.clone().or_else(|| base.font.clone()),
            size: self.size.or(base.size),
            bold: self.bold || base.bold,
            italic: self.italic || base.italic,
            color: self.color.or(base.color),
        }
    }
}

fn char_style(d: &[u8], pos: usize, fonts: &[String], palette: &[[f64; 3]]) -> CharStyle {
    use crate::contents::length_prefixed;
    let mut s = CharStyle::default();
    for b in length_prefixed(d, pos) {
        match b.id {
            style_id::BOLD => s.bold = true,
            style_id::ITALIC => s.italic = true,
            style_id::TEXT_SIZE => s.size = b.value.map(|v| v as f64 / EMU_PER_PT),
            style_id::FONT_INDEX_CONTAINER => {
                // first general container inside, then its first block's value
                let idx = b.children(d).into_iter().find(|c| c.btype == 0x88)
                    .and_then(|c| c.children(d).first().and_then(|x| x.value));
                s.font = idx.and_then(|i| fonts.get(i as usize).cloned());
            }
            style_id::COLOR_INDEX_CONTAINER => {
                let idx = b.children(d).into_iter().find(|k| k.id == 0).and_then(|k| k.value);
                s.color = idx.and_then(|i| palette.get(i as usize).copied());
            }
            _ => {}
        }
    }
    s
}

fn para_style(d: &[u8], pos: usize) -> ParaStyle {
    use crate::contents::length_prefixed;
    let mut s = ParaStyle::default();
    for b in length_prefixed(d, pos) {
        match (b.id, b.value) {
            (style_id::PARA_ALIGNMENT, v) => s.align = v,
            (style_id::PARA_SPACE_BEFORE, Some(v)) => s.space_before = Some(v as f64 / EMU_PER_PT),
            (style_id::PARA_SPACE_AFTER, Some(v)) => s.space_after = Some(v as f64 / EMU_PER_PT),
            (style_id::PARA_LINE_SPACING, Some(v)) => {
                // bit 0: points, stored in eighths of an EMU; bit 1: lines, stored
                // as EMU at a notional 96pt font (libmspub's reading)
                s.line_spacing = if v & 1 != 0 {
                    Some(LineSpacing::Points((v - 1) as f64 / 8.0 / EMU_PER_PT))
                } else if v & 2 != 0 {
                    Some(LineSpacing::Lines((v - 2) as f64 / EMU_PER_PT / 96.0))
                } else {
                    None
                }
            }
            _ => {}
        }
    }
    s
}

/// The text colour list ("PL  " chunk): length-prefixed entries end to end,
/// each with its RGB as block 1; index by a run's colour.
fn palette(d: &[u8], c: &Chunk) -> Vec<[f64; 3]> {
    use crate::contents::length_prefixed;
    let mut out = Vec::new();
    let mut pos = c.offset + 12; // past a count and an offset
    while pos + 4 <= c.offset + c.length {
        let Some(len) = u32_at(d, pos).map(|v| v as usize).filter(|&l| l >= 4) else { break };
        let v = length_prefixed(d, pos).into_iter().find(|b| b.id == 1).and_then(|b| b.value).unwrap_or(0);
        // high byte set: a scheme colour, not an RGB (entry 0 is "automatic"); black is what those are in practice
        // shortcut: scheme colours print black; resolve them through the document's colour scheme if a file needs it.
        let v = if v >> 24 != 0 { 0 } else { v };
        out.push([(v & 0xFF) as f64 / 255.0, ((v >> 8) & 0xFF) as f64 / 255.0, ((v >> 16) & 0xFF) as f64 / 255.0]);
        pos += len;
    }
    out
}

fn font_names(d: &[u8], c: &Chunk) -> Vec<String> {
    let Some(n) = u32_at(d, c.offset + 4).map(|v| v as usize) else { return Vec::new() };
    let mut pos = c.offset + 8 + 12 + 4 * n;
    let mut out = Vec::with_capacity(n.min(4096)); // n comes from the file: never trust it for allocation
    for _ in 0..n.min(4096) {
        let Some(len) = u16_at(d, pos).map(|v| v as usize) else { break };
        out.push(d.get(pos + 2..pos + 2 + 2 * len).map(utf16le).unwrap_or_default());
        pos += 2 + 2 * len + 4;
    }
    out
}

/// FDPC/FDPP: (end stream offset, style data offset) per run.
fn fdp_runs(d: &[u8], c: &Chunk) -> Vec<(usize, usize)> {
    let Some(n) = u16_at(d, c.offset).map(|v| v as usize) else { return Vec::new() };
    let ends = c.offset + 8;
    let offs = ends + 4 * n;
    (0..n)
        .filter_map(|i| Some((u32_at(d, ends + 4 * i)? as usize, c.offset + u16_at(d, offs + 2 * i)? as usize)))
        .collect()
}

/// Document default styles (second STSH chunk): character and paragraph
/// styles alternate; index 0/1 is the "Normal" style.
fn defaults(d: &[u8], cs: &[Chunk], fonts: &[String], palette: &[[f64; 3]]) -> (Vec<CharStyle>, Vec<ParaStyle>) {
    let (mut chars, mut paras) = (Vec::new(), Vec::new());
    let Some(c) = cs.iter().filter(|c| c.name == "STSH").nth(1) else { return (chars, paras) };
    let Some(n) = u32_at(d, c.offset + 4).map(|v| v as usize) else { return (chars, paras) };
    for i in 0..n.min(4096) {
        let Some(off) = u32_at(d, c.offset + 20 + 4 * i) else { break };
        let pos = c.offset + 20 + off as usize + 2;
        if i % 2 == 0 {
            chars.push(char_style(d, pos, fonts, palette));
        } else {
            paras.push(para_style(d, pos));
        }
    }
    (chars, paras)
}

/// A story with its formatting runs. Run ends are exclusive char indexes
/// within the story.
#[derive(Debug, Clone, PartialEq)]
pub struct StyledStory {
    pub story: Story,
    pub char_runs: Vec<(usize, CharStyle)>,
    pub para_runs: Vec<(usize, ParaStyle)>,
}

/// The document's "Normal" style.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Defaults {
    pub char: CharStyle,
    pub para: ParaStyle,
}

/// All stories with their formatting, plus the document defaults.
pub fn styled_stories(file: &PubFile) -> Result<(Vec<StyledStory>, Defaults)> {
    let data = file.stream(crate::ole::QUILL).ok_or(Error::MissingStream("Quill"))?;
    let cs = chunks(data);
    let text = cs.iter().find(|c| c.name == "TEXT").ok_or(Error::Malformed("Quill: no TEXT chunk"))?;
    let fonts = cs.iter().find(|c| c.name == "FONT").map(|c| font_names(data, c)).unwrap_or_default();
    let palette = cs.iter().find(|c| c.name == "PL  ").map(|c| palette(data, c)).unwrap_or_default();
    let (dc, dp) = defaults(data, &cs, &fonts, &palette);
    let defaults = Defaults { char: dc.into_iter().next().unwrap_or_default(), para: dp.into_iter().next().unwrap_or_default() };
    let to_char = |end: usize| end.saturating_sub(text.offset) / 2;
    let char_runs: Vec<(usize, CharStyle)> = cs.iter().filter(|c| c.name == "FDPC")
        .flat_map(|c| fdp_runs(data, c))
        .map(|(e, s)| (to_char(e), char_style(data, s, &fonts, &palette)))
        .collect();
    let para_runs: Vec<(usize, ParaStyle)> = cs.iter().filter(|c| c.name == "FDPP")
        .flat_map(|c| fdp_runs(data, c))
        .map(|(e, s)| (to_char(e), para_style(data, s)))
        .collect();
    let mut out = Vec::new();
    let mut start = 0;
    for story in stories(file)? {
        let end = start + story.text.chars().count();
        let char_runs = runs_within(&char_runs, start, end);
        let para_runs = runs_within(&para_runs, start, end);
        out.push(StyledStory { story, char_runs, para_runs });
        start = end;
    }
    Ok((out, defaults))
}

/// Runs ending inside (start, end], re-based to the story's first char.
fn runs_within<T: Clone>(runs: &[(usize, T)], start: usize, end: usize) -> Vec<(usize, T)> {
    runs.iter().filter(|(e, _)| *e > start && *e <= end).map(|(e, s)| (e - start, s.clone())).collect()
}

/// All text stories in the file.
/// Page-number fields: story id -> character positions. Each TOKN chunk lists
/// the fields of one story (chunk id = the story's index in SYID): a count,
/// then the fields' start positions; Publisher stores the field itself as '#'.
pub fn page_number_fields(file: &PubFile) -> std::collections::BTreeMap<u32, Vec<usize>> {
    let mut out = std::collections::BTreeMap::new();
    let Some(data) = file.stream(crate::ole::QUILL) else { return out };
    let Ok(stories) = stories(file) else { return out };
    for c in chunks(data).iter().filter(|c| c.name == "TOKN") {
        let Some(story) = stories.get(c.id as usize) else { continue };
        let Some(n) = u32_at(data, c.offset).map(|v| v as usize) else { continue };
        let text: Vec<char> = story.text.chars().collect();
        let fields: Vec<usize> = (0..n.min(256))
            .filter_map(|i| u32_at(data, c.offset + 12 + 4 * i).map(|v| v as usize))
            .filter(|&p| text.get(p) == Some(&'#'))
            .collect();
        if !fields.is_empty() {
            out.insert(story.id, fields);
        }
    }
    out
}

pub fn stories(file: &PubFile) -> Result<Vec<Story>> {
    let data = file.stream(crate::ole::QUILL).ok_or(Error::MissingStream("Quill"))?;
    let cs = chunks(data);
    let find = |n: &str| cs.iter().find(|c| c.name == n);
    let text = find("TEXT").ok_or(Error::Malformed("Quill: no TEXT chunk"))?;
    let lengths = find("STRS").and_then(|c| strs(data, c)).ok_or(Error::Malformed("Quill STRS"))?;
    let ids = find("SYID").and_then(|c| syid(data, c)).ok_or(Error::Malformed("Quill SYID"))?;
    let mut pos = text.offset;
    let mut out = Vec::new();
    for (id, len) in ids.into_iter().zip(lengths) {
        let end = (pos + 2 * len).min(data.len());
        out.push(Story { id, text: utf16le(&data[pos.min(end)..end]) });
        pos = end;
    }
    Ok(out)
}
