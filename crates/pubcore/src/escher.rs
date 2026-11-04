//! OfficeArt ("Escher") drawing records, as specified in [MS-ODRAW].
//!
//! Publisher keeps shape containers at offsets referenced from the Contents
//! stream, with gaps between them, so containers are found by scanning.

use std::collections::BTreeMap;

use crate::{i32_at, u16_at, u32_at};

pub const SP_CONTAINER: u16 = 0xF004;
pub const FSP: u16 = 0xF00A;
pub const FOPT: u16 = 0xF00B;
/// Tertiary property table: Publisher keeps text-box columns here.
pub const TERTIARY_FOPT: u16 = 0xF122;
pub const CLIENT_ANCHOR: u16 = 0xF010;
pub const CLIENT_DATA: u16 = 0xF011;
pub const EMU_PER_PT: f64 = 12700.0;
/// Client-data field holding the shape's Contents sequence number.
pub const FIELD_SHAPE_SEQ: u16 = 0x6801;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub ver: u8,
    pub inst: u16,
    pub rtype: u16,
    pub len: u32,
}

/// A record found while walking: header plus the byte range of its body.
#[derive(Debug, Clone, Copy)]
pub struct Record {
    pub header: Header,
    /// Offset of the 8-byte header in the stream.
    pub at: usize,
    pub body: usize,
}

impl Record {
    pub fn body<'a>(&self, data: &'a [u8]) -> &'a [u8] {
        let end = (self.body + self.header.len as usize).min(data.len());
        &data[self.body.min(end)..end]
    }
}

pub fn header_at(data: &[u8], pos: usize) -> Option<Header> {
    let vi = u16_at(data, pos)?;
    Some(Header {
        ver: (vi & 0xF) as u8,
        inst: vi >> 4,
        rtype: u16_at(data, pos + 2)?,
        len: u32_at(data, pos + 4)?,
    })
}

/// Every record in `data[start..end]`, descending into containers (ver 0xF).
pub fn walk(data: &[u8], start: usize, end: usize) -> Vec<Record> {
    let mut out = Vec::new();
    walk_into(data, start, end.min(data.len()), &mut out, 0);
    out
}

fn walk_into(data: &[u8], start: usize, end: usize, out: &mut Vec<Record>, depth: u32) {
    let mut pos = start;
    while pos + 8 <= end && depth < 32 {
        let Some(h) = header_at(data, pos) else { break };
        let body = pos + 8;
        let next = body.saturating_add(h.len as usize);
        if next > end {
            break; // not a record (gap between Publisher's containers)
        }
        out.push(Record { header: h, at: pos, body });
        if h.ver == 0xF {
            walk_into(data, body, next, out, depth + 1);
        }
        pos = next;
    }
}

/// Offsets of plausible shape containers (ver 0xF, type 0xF004) in a stream.
pub fn shape_containers(data: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut pos = 0;
    while pos + 8 <= data.len() {
        if u16_at(data, pos + 2) == Some(SP_CONTAINER) && data[pos] & 0xF == 0xF {
            if let Some(len) = u32_at(data, pos + 4) {
                if len >= 8 && (len as usize) <= data.len() - pos - 8 {
                    out.push(pos);
                }
            }
        }
        pos += 1;
    }
    out
}

/// A shape's property table: simple values plus complex (variable-size) data.
#[derive(Debug, Clone, Default)]
pub struct Fopt {
    pub values: BTreeMap<u16, u32>,
    pub complex: BTreeMap<u16, Vec<u8>>,
}

impl Fopt {
    pub fn parse(body: &[u8], count: u16) -> Self {
        let mut f = Fopt::default();
        let mut complex_ids = Vec::new();
        for i in 0..count as usize {
            let (Some(opid), Some(op)) = (u16_at(body, 6 * i), u32_at(body, 6 * i + 2)) else { break };
            let pid = opid & 0x3FFF;
            f.values.insert(pid, op);
            if opid & 0x8000 != 0 {
                complex_ids.push((pid, op as usize));
            }
        }
        // complex data follows the table, in the same order
        let mut pos = 6 * count as usize;
        for (pid, size) in complex_ids {
            let end = (pos + size).min(body.len());
            f.complex.insert(pid, body[pos.min(end)..end].to_vec());
            pos = end;
        }
        f
    }

    pub fn get(&self, pid: u16) -> Option<u32> {
        self.values.get(&pid).copied()
    }

    pub fn get_i32(&self, pid: u16) -> Option<i32> {
        self.get(pid).map(|v| v as i32)
    }
}

/// Publisher's client records (anchor, data): a length, then (u16 id, i32) pairs.
pub fn client_fields(body: &[u8]) -> BTreeMap<u16, i32> {
    let mut f = BTreeMap::new();
    let mut pos = 4;
    while pos + 6 <= body.len() {
        if let (Some(id), Some(v)) = (u16_at(body, pos), i32_at(body, pos + 2)) {
            f.insert(id, v);
        }
        pos += 6;
    }
    f
}

/// One drawing shape: its type, Contents sequence number, position and properties.
#[derive(Debug, Clone)]
pub struct Shape {
    pub offset: usize,
    pub shape_type: u16,
    pub seq: Option<u32>,
    pub anchor: Option<Anchor>,
    pub fopt: Fopt,
    pub tertiary: Fopt,
}

/// Every shape container in the file's Escher streams.
pub fn shapes(file: &crate::PubFile) -> Vec<Shape> {
    let mut out = Vec::new();
    for (_, data) in file.escher_streams() {
        for start in shape_containers(data) {
            let Some(h) = header_at(data, start) else { continue };
            let mut s = Shape { offset: start, shape_type: 0, seq: None, anchor: None, fopt: Fopt::default(), tertiary: Fopt::default() };
            for rec in walk(data, start + 8, start + 8 + h.len as usize) {
                match rec.header.rtype {
                    FSP => s.shape_type = rec.header.inst,
                    FOPT => s.fopt = Fopt::parse(rec.body(data), rec.header.inst),
                    TERTIARY_FOPT => s.tertiary = Fopt::parse(rec.body(data), rec.header.inst),
                    CLIENT_ANCHOR => s.anchor = Anchor::parse(rec.body(data)),
                    CLIENT_DATA => s.seq = client_fields(rec.body(data)).get(&FIELD_SHAPE_SEQ).map(|&v| v as u32),
                    _ => {}
                }
            }
            out.push(s);
        }
    }
    out
}

/// Shape position in points, measured from the page centre.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Anchor {
    pub xs: f64,
    pub ys: f64,
    pub xe: f64,
    pub ye: f64,
}

impl Anchor {
    /// Publisher's client anchor: a length, then (u16 id, i32 EMU) pairs for
    /// XS/YS/XE/YE (ids 0x2001..0x2004), relative to the page centre.
    pub fn parse(body: &[u8]) -> Option<Self> {
        let f = client_fields(body);
        let pt = |id| f.get(&id).map(|&v| v as f64 / EMU_PER_PT);
        Some(Anchor { xs: pt(0x2001)?, ys: pt(0x2002)?, xe: pt(0x2003)?, ye: pt(0x2004)? })
    }

    pub fn width(&self) -> f64 {
        self.xe - self.xs
    }

    pub fn height(&self) -> f64 {
        self.ye - self.ys
    }
}
