//! Text boxes: a Contents shape chunk (which story) joined by sequence
//! number to its drawing shape (where, and its inner margins).

use std::collections::BTreeMap;

use crate::contents::{self, TextLink};
use crate::escher::{self, Anchor, EMU_PER_PT};
use crate::PubFile;

/// Inner-margin property ids ([MS-ODRAW] dxTextLeft..dyTextBottom), in EMU.
const INSET_PIDS: [u16; 4] = [0x0081, 0x0082, 0x0083, 0x0084];
/// Text-box columns: count, and the gap between them in EMU (tertiary properties).
const COLUMNS_PID: u16 = 0x008C;
const COLUMN_GAP_PID: u16 = 0x008D;
/// Space kept between wrapped text and a shape ([MS-ODRAW] dxWrapDistLeft..dyWrapDistBottom), EMU.
const WRAP_DIST_PIDS: [u16; 4] = [0x0384, 0x0385, 0x0386, 0x0387];
/// Publisher's default wrap distance, 0.04in (measured from its PDFs).
const DEFAULT_WRAP_DIST: f64 = 2.88;
/// Publisher's default text box margin, 0.04in.
pub const DEFAULT_INSET: f64 = 2.88;

/// Something text flows around, in points from the page centre.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Obstacle {
    /// Left, top, right, bottom, wrap distance included.
    pub rect: [f64; 4],
    /// The shape's edge, when text follows it rather than the box (tight wrap).
    pub outline: Vec<(f64, f64)>,
    /// Wrap distance: left, top, right, bottom.
    pub distance: [f64; 4],
    /// Nothing may sit beside it (top-and-bottom wrap).
    pub whole_row: bool,
}

impl Obstacle {
    /// How far across it blocks the row `top..bottom`, if at all.
    pub fn in_row(&self, top: f64, bottom: f64) -> Option<(f64, f64)> {
        if self.rect[1] >= bottom || self.rect[3] <= top {
            return None;
        }
        if self.whole_row {
            return Some((f64::MIN, f64::MAX));
        }
        if self.outline.is_empty() {
            return Some((self.rect[0], self.rect[2]));
        }
        // the outline's widest reach inside the row (hollows beside a star's arms stay blocked, as in Publisher)
        let (band_top, band_bottom) = (top - self.distance[1], bottom + self.distance[3]);
        let (mut lo, mut hi) = (f64::MAX, f64::MIN);
        for i in 0..self.outline.len() {
            let (a, b) = (self.outline[i], self.outline[(i + 1) % self.outline.len()]);
            let (t0, t1) = if a.1 == b.1 {
                if a.1 < band_top || a.1 > band_bottom { continue } else { (0.0, 1.0) }
            } else {
                let (p, q) = ((band_top - a.1) / (b.1 - a.1), (band_bottom - a.1) / (b.1 - a.1));
                let (p, q) = (p.min(q).max(0.0), p.max(q).min(1.0));
                if p > q { continue }
                (p, q)
            };
            for t in [t0, t1] {
                let x = a.0 + (b.0 - a.0) * t;
                lo = lo.min(x);
                hi = hi.max(x);
            }
        }
        (lo <= hi).then_some((lo - self.distance[0], hi + self.distance[2]))
    }
}

/// A preset shape's edge, on its box as a unit square.
// shortcut: common shapes only; others wrap around their box, add them as files need them.
fn preset_outline(shape_type: u16) -> Vec<(f64, f64)> {
    let star = |points: usize, inner: f64, turn: f64| -> Vec<(f64, f64)> {
        let ring: Vec<(f64, f64)> = (0..points * 2).map(|i| {
            let r = if i % 2 == 0 { 0.5 } else { 0.5 * inner };
            let a = turn + std::f64::consts::PI * i as f64 / points as f64;
            (r * a.sin(), -r * a.cos())
        }).collect();
        // Office stretches a star to touch all four sides of its box
        let (x0, x1) = ring.iter().fold((f64::MAX, f64::MIN), |m, p| (m.0.min(p.0), m.1.max(p.0)));
        let (y0, y1) = ring.iter().fold((f64::MAX, f64::MIN), |m, p| (m.0.min(p.1), m.1.max(p.1)));
        ring.into_iter().map(|(x, y)| ((x - x0) / (x1 - x0), (y - y0) / (y1 - y0))).collect()
    };
    match shape_type {
        3 => star(36, 1.0, 0.0), // ellipse
        4 => vec![(0.5, 0.0), (1.0, 0.5), (0.5, 1.0), (0.0, 0.5)],
        5 => vec![(0.5, 0.0), (1.0, 1.0), (0.0, 1.0)],
        6 => vec![(0.0, 0.0), (1.0, 1.0), (0.0, 1.0)],
        7 => vec![(0.25, 0.0), (1.0, 0.0), (0.75, 1.0), (0.0, 1.0)],
        8 => vec![(0.0, 1.0), (0.25, 0.0), (0.75, 0.0), (1.0, 1.0)],
        9 => vec![(0.25, 0.0), (0.75, 0.0), (1.0, 0.5), (0.75, 1.0), (0.25, 1.0), (0.0, 0.5)],
        10 => vec![(0.29, 0.0), (0.71, 0.0), (1.0, 0.29), (1.0, 0.71), (0.71, 1.0), (0.29, 1.0), (0.0, 0.71), (0.0, 0.29)],
        12 => star(5, 0.382, 0.0),
        56 => star(5, 0.81, 0.0),
        58 => star(8, 0.75, 0.0),
        59 => star(16, 0.75, 0.0),
        92 => star(24, 0.75, 0.0),
        60 => star(32, 0.75, 0.0),
        187 => star(4, 0.25, 0.0),
        _ => Vec::new(),
    }
}

/// An array property: count, allocated, entry size, then (x, y) pairs.
fn point_array(b: &[u8]) -> Vec<(f64, f64)> {
    let u16_at = |i: usize| b.get(i..i + 2).map(|v| u16::from_le_bytes([v[0], v[1]]));
    let (Some(n), Some(size)) = (u16_at(0), u16_at(4)) else { return Vec::new() };
    // size 0xFFF0 is Office's short form, 4 bytes per point
    let per = if size == 0xFFF0 || size == 4 { 4 } else { 8 };
    (0..n as usize).filter_map(|i| {
        let at = 6 + i * per;
        if per == 4 {
            Some((u16_at(at)? as i16 as f64, u16_at(at + 2)? as i16 as f64))
        } else {
            let v = |k: usize| b.get(k..k + 4).map(|v| i32::from_le_bytes([v[0], v[1], v[2], v[3]]) as f64);
            Some((v(at)?, v(at + 4)?))
        }
    }).collect()
}

/// A shape's edge in points from the page centre: its stored wrap outline (a
/// picture's edited wrap points), its freeform points, or its preset's shape.
fn outline(s: &escher::Shape, a: Anchor) -> Vec<(f64, f64)> {
    const WRAP_POLYGON: u16 = 0x0383;
    const VERTICES: u16 = 0x0145;
    const GEO: [u16; 4] = [0x0140, 0x0141, 0x0142, 0x0143];
    const ROTATION: u16 = 0x0004;
    let unit: Vec<(f64, f64)> = if let Some(b) = s.fopt.complex.get(&WRAP_POLYGON) {
        point_array(b).into_iter().map(|(x, y)| (x / 21600.0, y / 21600.0)).collect()
    } else if let Some(b) = s.fopt.complex.get(&VERTICES) {
        let g = GEO.map(|p| s.fopt.get_i32(p).map(|v| v as f64));
        let (l, t) = (g[0].unwrap_or(0.0), g[1].unwrap_or(0.0));
        let (r, bm) = (g[2].unwrap_or(21600.0), g[3].unwrap_or(21600.0));
        point_array(b).into_iter().map(|(x, y)| ((x - l) / (r - l).max(1.0), (y - t) / (bm - t).max(1.0))).collect()
    } else {
        preset_outline(s.shape_type)
    };
    let (cx, cy) = ((a.xs + a.xe) / 2.0, (a.ys + a.ye) / 2.0);
    let turn = s.group_rotation + s.fopt.get_i32(ROTATION).map_or(0.0, |v| v as f64 / 65536.0);
    let (sin, cos) = turn.to_radians().sin_cos();
    unit.into_iter().map(|(u, v)| {
        let (x, y) = (a.xs + u * a.width() - cx, a.ys + v * a.height() - cy);
        (cx + x * cos - y * sin, cy + x * sin + y * cos)
    }).collect()
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextBox {
    /// Sequence number the page lists: its own, or its outermost group's.
    pub seq: u32,
    /// Degrees its groups turn it (0 unless it's inside a turned group).
    pub rotation: f64,
    /// Centre of the box on the page, with y down from the page's top left.
    pub page_center: (f64, f64),
    pub link: TextLink,
    /// Position in points from the page centre.
    pub anchor: Anchor,
    /// Inner margins in points: left, top, right, bottom.
    pub inset: [f64; 4],
    /// Publisher adds "(Continued on page N)" at the bottom / "(Continued from page N)" at the top.
    pub continued_on: bool,
    pub continued_from: bool,
    /// Columns inside the box (1 = none) and the gap between them, in points.
    pub columns: u32,
    pub column_gap: f64,
    /// Areas the text flows around.
    pub obstacles: Vec<Obstacle>,
    /// A table's text (all its cells in one story), not a text box.
    pub is_table: bool,
}

impl TextBox {
    pub fn story_id(&self) -> u32 {
        self.link.story_id
    }
}

/// Every text box whose position is known, in sequence order.
pub fn text_boxes(file: &PubFile) -> Vec<TextBox> {
    let page = contents::page_size(file).unwrap_or((612.0, 792.0));
    let shapes: BTreeMap<u32, escher::Shape> =
        escher::shapes(file).into_iter().filter_map(|s| s.seq.map(|q| (q, s))).collect();
    let chunks = contents::shape_chunks(file);
    let modes: BTreeMap<u32, Option<u32>> = chunks.iter().map(|c| (c.seq, c.wrap_mode)).collect();
    chunks
        .into_iter()
        .filter_map(|c| {
            let link = c.text?;
            let shape = shapes.get(&c.seq)?;
            let inset = INSET_PIDS.map(|p| shape.fopt.get(p).map_or(DEFAULT_INSET, |v| v as f64 / EMU_PER_PT));
            let columns = shape.tertiary.get(COLUMNS_PID).unwrap_or(1).clamp(1, 63);
            let column_gap = shape.tertiary.get(COLUMN_GAP_PID).map_or(72000.0, |v| v as f64) / EMU_PER_PT;
            let obstacles = c.wrap_around.iter().filter_map(|q| {
                let s = shapes.get(q)?;
                let a = s.anchor?;
                let d = WRAP_DIST_PIDS.map(|p| s.fopt.get(p).map_or(DEFAULT_WRAP_DIST, |v| v as f64 / EMU_PER_PT));
                let mode = modes.get(q).copied().flatten();
                let follow = matches!(mode, Some(contents::wrap::TIGHT | contents::wrap::THROUGH));
                let outline = if follow { outline(s, a) } else { Vec::new() };
                Some(Obstacle {
                    rect: [a.xs - d[0], a.ys - d[1], a.xe + d[2], a.ye + d[3]],
                    outline,
                    distance: d,
                    whole_row: mode == Some(contents::wrap::TOP_BOTTOM),
                })
            }).collect();
            Some(TextBox {
                seq: shape.root_seq.unwrap_or(c.seq),
                rotation: shape.group_rotation,
                page_center: {
                    let a = shape.anchor?;
                    (page.0 / 2.0 + (a.xs + a.xe) / 2.0, page.1 / 2.0 + (a.ys + a.ye) / 2.0)
                },
                link,
                anchor: shape.anchor?,
                inset,
                continued_on: c.continued_on,
                continued_from: c.continued_from,
                columns,
                column_gap,
                obstacles,
                is_table: c.ctype == contents::chunk_type::TABLE,
            })
        })
        .collect()
}

/// A story and the boxes it flows through, in chain order.
#[derive(Debug, Clone, PartialEq)]
pub struct Chain {
    pub story_id: u32,
    pub boxes: Vec<TextBox>,
}

/// Stories that flow through more than one box.
pub fn chains(file: &PubFile) -> Vec<Chain> {
    let mut by_story: BTreeMap<u32, Vec<TextBox>> = BTreeMap::new();
    for b in text_boxes(file) {
        by_story.entry(b.story_id()).or_default().push(b);
    }
    by_story
        .into_iter()
        .filter(|(_, boxes)| boxes.len() > 1)
        .map(|(story_id, boxes)| Chain { story_id, boxes })
        .collect()
}
