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

#[derive(Debug, Clone, PartialEq)]
pub struct TextBox {
    pub seq: u32,
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
    /// Areas the text flows around (left, top, right, bottom in points from
    /// the page centre, wrap distance included).
    pub obstacles: Vec<[f64; 4]>,
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
    let shapes: BTreeMap<u32, escher::Shape> =
        escher::shapes(file).into_iter().filter_map(|s| s.seq.map(|q| (q, s))).collect();
    contents::shape_chunks(file)
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
                Some([a.xs - d[0], a.ys - d[1], a.xe + d[2], a.ye + d[3]])
            }).collect();
            Some(TextBox {
                seq: c.seq,
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
