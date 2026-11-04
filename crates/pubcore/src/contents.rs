//! Contents stream: Publisher's block-structured document data.
//!
//! A trailer directory lists every chunk; a chunk's position in that list is
//! its sequence number, which drawing shapes reference from their client
//! data. Shape chunks say which story a text box shows (block 0x27); linked
//! boxes share one story id. Ports libmspub's parseContents, parseBlock,
//! parseContentChunkReference and parseShape.

use std::collections::BTreeMap;

use crate::{u32_at, PubFile};

pub mod block_id {
    pub const CHUNK_TYPE: u8 = 0x02;
    pub const CHUNK_OFFSET: u8 = 0x04;
    pub const SHAPE_TEXT_ID: u8 = 0x27;
    pub const SHAPE_WIDTH: u8 = 0xAA;
    pub const SHAPE_HEIGHT: u8 = 0xAB;
    /// Flags (empty blocks): the box shows "(Continued on page N)" / "(Continued from page N)".
    pub const SHAPE_CONTINUED_ON: u8 = 0x31;
    pub const SHAPE_CONTINUED_FROM: u8 = 0x32;
    /// Shapes this text box's text wraps around: a list of sequence numbers.
    pub const SHAPE_WRAP_AROUND: u8 = 0x47;
    pub const DOCUMENT_SIZE: u8 = 0x12;
    pub const DOCUMENT_WIDTH: u8 = 0x01;
    pub const DOCUMENT_HEIGHT: u8 = 0x02;
}

pub mod chunk_type {
    pub const SHAPE: u32 = 0x01;
    pub const TABLE: u32 = 0x10;
    pub const ALTSHAPE: u32 = 0x20;
    pub const GROUP: u32 = 0x30;
    pub const LOGO: u32 = 0x31;
    pub const PAGE: u32 = 0x43;
    pub const DOCUMENT: u32 = 0x44;
}

const TRAILER_DIRECTORY: u8 = 0x90;
const GENERAL_CONTAINER: u8 = 0x88;
const EMU_PER_PT: f64 = 12700.0;

/// One block: id, type, and either an integer value or a nested data range.
#[derive(Debug, Clone, Copy)]
pub struct Block {
    pub id: u8,
    pub btype: u8,
    /// Integer value for 1/2/4-byte blocks.
    pub value: Option<u32>,
    /// Offset of the block's data (after id and type).
    pub data_offset: usize,
    pub data_len: usize,
    pub next: usize,
}

impl Block {
    /// Children of a variable-length block (they start after its length field).
    pub fn children(&self, d: &[u8]) -> Vec<Block> {
        blocks(d, self.data_offset + 4, self.data_offset + self.data_len)
    }
}

/// Fixed data length by block type, or `None` for variable-length blocks
/// whose first u32 is their length (libmspub getBlockDataLength).
fn fixed_len(btype: u8) -> Option<usize> {
    match btype {
        0x78 | 0x05 | 0x08 | 0x0A => Some(0),
        0x10 | 0x12 | 0x18 | 0x1A | 0x07 => Some(2),
        0x20 | 0x22 | 0x58 | 0x68 | 0x70 | 0xB8 => Some(4),
        0x28 => Some(8),
        0x38 => Some(16),
        0x48 => Some(24),
        0xC0 | 0x80 | 0x82 | 0x88 | 0x8A | 0x90 | 0x98 | 0xA0 => None,
        _ => Some(0),
    }
}

pub fn block_at(d: &[u8], pos: usize) -> Option<Block> {
    let (&id, &btype) = (d.get(pos)?, d.get(pos + 1)?);
    let data_offset = pos + 2;
    match fixed_len(btype) {
        None => {
            let len = u32_at(d, data_offset)? as usize;
            Some(Block { id, btype, value: None, data_offset, data_len: len, next: data_offset.checked_add(len)? })
        }
        Some(len) => {
            let raw = d.get(data_offset..data_offset + len)?;
            let value = match len {
                1 => Some(raw[0] as u32),
                2 => Some(u16::from_le_bytes([raw[0], raw[1]]) as u32),
                4 => Some(u32::from_le_bytes([raw[0], raw[1], raw[2], raw[3]])),
                _ => None,
            };
            Some(Block { id, btype, value, data_offset, data_len: len, next: data_offset + len })
        }
    }
}

/// Blocks laid end to end in `d[start..end]`.
pub fn blocks(d: &[u8], start: usize, end: usize) -> Vec<Block> {
    let mut out = Vec::new();
    let mut pos = start;
    let end = end.min(d.len());
    while pos + 2 <= end {
        let Some(b) = block_at(d, pos) else { break };
        if b.next <= pos || b.next > end {
            break;
        }
        pos = b.next;
        out.push(b);
    }
    out
}

/// A block whose data starts with its own u32 length, then child blocks.
pub fn length_prefixed(d: &[u8], pos: usize) -> Vec<Block> {
    match u32_at(d, pos) {
        Some(len) => blocks(d, pos + 4, pos + len as usize),
        None => Vec::new(),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkRef {
    pub seq: u32,
    pub ctype: u32,
    pub offset: usize,
}

/// The trailer directory: every chunk with a type and offset, by sequence number.
pub fn chunk_refs(d: &[u8]) -> Vec<ChunkRef> {
    let Some(trailer) = u32_at(d, 0x1A).map(|v| v as usize) else { return Vec::new() };
    let mut out = Vec::new();
    let mut pos = trailer + 4;
    for _ in 0..3 {
        let Some(part) = block_at(d, pos) else { break };
        if part.btype == TRAILER_DIRECTORY {
            for (seq, b) in part.children(d).into_iter().enumerate() {
                if b.btype != GENERAL_CONTAINER {
                    continue;
                }
                let kids = b.children(d);
                let get = |id| kids.iter().find(|k| k.id == id).and_then(|k| k.value);
                if let (Some(ctype), Some(offset)) = (get(block_id::CHUNK_TYPE), get(block_id::CHUNK_OFFSET)) {
                    out.push(ChunkRef { seq: seq as u32, ctype, offset: offset as usize });
                }
            }
        }
        pos = part.next;
    }
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextLink {
    /// Offset of the u32 story id within the Contents stream.
    pub value_offset: usize,
    pub story_id: u32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ShapeChunk {
    pub seq: u32,
    pub ctype: u32,
    pub text: Option<TextLink>,
    /// Size in points, when stored.
    pub width: Option<f64>,
    pub height: Option<f64>,
    pub continued_on: bool,
    pub continued_from: bool,
    /// Sequence numbers of shapes the text wraps around.
    pub wrap_around: Vec<u32>,
}

/// Every shape chunk (shapes, groups, tables, logos), in sequence order.
pub fn shape_chunks(file: &PubFile) -> Vec<ShapeChunk> {
    use chunk_type::*;
    let Some(d) = file.stream(crate::ole::CONTENTS) else { return Vec::new() };
    let mut out = Vec::new();
    for c in chunk_refs(d) {
        if ![SHAPE, ALTSHAPE, GROUP, LOGO, TABLE].contains(&c.ctype) {
            continue;
        }
        let mut s = ShapeChunk { seq: c.seq, ctype: c.ctype, text: None, width: None, height: None, continued_on: false, continued_from: false, wrap_around: Vec::new() };
        for b in length_prefixed(d, c.offset) {
            match (b.id, b.value) {
                (block_id::SHAPE_TEXT_ID, Some(v)) if b.data_len == 4 => {
                    s.text = Some(TextLink { value_offset: b.data_offset, story_id: v })
                }
                (block_id::SHAPE_WIDTH, Some(v)) => s.width = Some(v as f64 / EMU_PER_PT),
                (block_id::SHAPE_HEIGHT, Some(v)) => s.height = Some(v as f64 / EMU_PER_PT),
                (block_id::SHAPE_CONTINUED_ON, _) => s.continued_on = true,
                (block_id::SHAPE_CONTINUED_FROM, _) => s.continued_from = true,
                (block_id::SHAPE_WRAP_AROUND, None) => {
                    s.wrap_around = b.children(d).into_iter().filter(|k| k.btype == 0x68).filter_map(|k| k.value).collect();
                }
                _ => {}
            }
        }
        out.push(s);
    }
    out
}

/// Page size in points (from the document chunk).
pub fn page_size(file: &PubFile) -> Option<(f64, f64)> {
    let d = file.stream(crate::ole::CONTENTS)?;
    let doc = chunk_refs(d).into_iter().find(|c| c.ctype == chunk_type::DOCUMENT)?;
    let size = length_prefixed(d, doc.offset).into_iter().find(|b| b.id == block_id::DOCUMENT_SIZE)?;
    let kids = size.children(d);
    let get = |id| kids.iter().find(|k| k.id == id).and_then(|k| k.value).map(|v| v as f64 / EMU_PER_PT);
    Some((get(block_id::DOCUMENT_WIDTH)?, get(block_id::DOCUMENT_HEIGHT)?))
}

/// A page and the sequence numbers of the shapes placed on it.
#[derive(Debug, Clone, PartialEq)]
pub struct Page {
    pub seq: u32,
    pub is_master: bool,
    pub shapes: Vec<u32>,
}

/// Where shapes appear in the output: normal pages in document order (the
/// order LibreOffice exports them), plus master pages, whose shapes show on
/// every page. Ports libmspub's parseDocumentChunk / parsePageChunk.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Pages {
    pub normal: Vec<Page>,
    pub masters: Vec<Page>,
}

impl Pages {
    /// Output page indexes (0-based) on which shape `seq` is drawn.
    pub fn pages_of(&self, seq: u32) -> Vec<usize> {
        if let Some(i) = self.normal.iter().position(|p| p.shapes.contains(&seq)) {
            return vec![i];
        }
        if self.masters.iter().any(|p| p.shapes.contains(&seq)) {
            return (0..self.normal.len()).collect();
        }
        Vec::new()
    }
}

const PAGE_SHAPES: u8 = 0x02;
const THIS_MASTER_NAME: u8 = 0x0E;
const DOCUMENT_PAGE_LIST: u8 = 0x02;
const SHAPE_SEQNUM_TYPE: u8 = 0x70;
/// Publisher's placeholder pages (libmspub getPageTypeBySeqNum).
const DUMMY_PAGES: [u32; 4] = [0x10D, 0x110, 0x113, 0x117];

pub fn pages(file: &PubFile) -> Pages {
    let Some(d) = file.stream(crate::ole::CONTENTS) else { return Pages::default() };
    let refs = chunk_refs(d);
    let mut by_seq = BTreeMap::new();
    for c in refs.iter().filter(|c| c.ctype == chunk_type::PAGE && !DUMMY_PAGES.contains(&c.seq)) {
        let mut page = Page { seq: c.seq, is_master: false, shapes: Vec::new() };
        for b in length_prefixed(d, c.offset) {
            match b.id {
                PAGE_SHAPES if b.value.is_none() => {
                    page.shapes = b.children(d).into_iter()
                        .filter(|k| k.btype == SHAPE_SEQNUM_TYPE)
                        .filter_map(|k| k.value)
                        .collect();
                }
                THIS_MASTER_NAME if b.btype == 0xC0 => {
                    let name = d.get(b.data_offset + 4..b.next).unwrap_or_default();
                    page.is_master = name.iter().any(|&x| x != 0);
                }
                _ => {}
            }
        }
        by_seq.insert(c.seq, page);
    }
    // document order of normal pages
    let order: Vec<u32> = refs.iter().find(|c| c.ctype == chunk_type::DOCUMENT)
        .map(|doc| {
            length_prefixed(d, doc.offset).into_iter()
                .filter(|b| b.id == DOCUMENT_PAGE_LIST && b.value.is_none())
                .flat_map(|b| b.children(d))
                .filter(|k| k.id == 0)
                .filter_map(|k| k.value)
                .collect()
        })
        .unwrap_or_default();
    let mut out = Pages::default();
    for seq in order {
        if let Some(p) = by_seq.remove(&seq) {
            if p.is_master { out.masters.push(p) } else { out.normal.push(p) }
        }
    }
    // pages not in the list (e.g. masters) keep their role
    for p in by_seq.into_values() {
        if p.is_master || !p.shapes.is_empty() && out.normal.is_empty() {
            if p.is_master { out.masters.push(p) } else { out.normal.push(p) }
        } else if !p.is_master {
            out.masters.push(Page { is_master: true, ..p }); // unlisted page: treat like a master background
        }
    }
    out
}

/// Every text box's link to its story, in sequence order.
pub fn text_links(file: &PubFile) -> Vec<TextLink> {
    shape_chunks(file).into_iter().filter_map(|s| s.text).collect()
}

/// Stories shown in more than one box, with their boxes in chain order.
pub fn linked_chains(file: &PubFile) -> BTreeMap<u32, Vec<TextLink>> {
    let mut by_story: BTreeMap<u32, Vec<TextLink>> = BTreeMap::new();
    for link in text_links(file) {
        by_story.entry(link.story_id).or_default().push(link);
    }
    by_story.retain(|_, links| links.len() > 1);
    by_story
}
