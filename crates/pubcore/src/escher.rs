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
pub const SPGR_CONTAINER: u16 = 0xF003;
pub const FSPGR: u16 = 0xF009;
pub const CHILD_ANCHOR: u16 = 0xF00F;
const ROTATION_PID: u16 = 0x0004;
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
#[derive(Debug, Clone, Default, PartialEq)]
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
    /// Inside a group: the outermost group's sequence number (what the page lists).
    pub root_seq: Option<u32>,
    /// Degrees the enclosing groups turn this shape.
    pub group_rotation: f64,
}

/// Every shape container in the file's Escher streams.
pub fn shapes(file: &crate::PubFile) -> Vec<Shape> {
    let mut out = Vec::new();
    for (_, data) in file.escher_streams() {
        let placed = grouped(data);
        for start in shape_containers(data) {
            let Some(h) = header_at(data, start) else { continue };
            let mut s = Shape { offset: start, shape_type: 0, seq: None, anchor: None, fopt: Fopt::default(), tertiary: Fopt::default(), root_seq: None, group_rotation: 0.0 };
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
            if let Some(p) = placed.get(&start) {
                (s.anchor, s.root_seq, s.group_rotation) = (Some(p.anchor), p.root_seq, p.rotation);
            }
            out.push(s);
        }
    }
    out
}

/// The picture at 1-based `index` of the file's picture store: its record type
/// and the record's body. Publisher keeps the bytes in the delay stream.
pub fn blip(file: &crate::PubFile, index: u32) -> Option<(u16, u16, Vec<u8>)> {
    const BSTORE: u16 = 0xF001;
    const BSE: u16 = 0xF007;
    let main = file.stream(crate::ole::ESCHER)?;
    let store = walk(main, 0, main.len()).into_iter().find(|r| r.header.rtype == BSTORE)?;
    let entries = walk(main, store.body, store.body + store.header.len as usize);
    let bse = entries.iter().filter(|r| r.header.rtype == BSE).nth(index.checked_sub(1)? as usize)?;
    let body = bse.body(main);
    let (data, at) = if body.len() > 36 {
        (body, 36) // stored right after the entry
    } else {
        (file.stream(crate::ole::ESCHER_DELAY)?, u32_at(body, 28)? as usize)
    };
    let h = header_at(data, at)?;
    let raw = data.get(at + 8..at + 8 + h.len as usize)?;
    Some((h.rtype, h.inst, raw.to_vec()))
}

/// Where a shape inside a group really is on the page.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placed {
    pub anchor: Anchor,
    pub rotation: f64,
    pub root_seq: Option<u32>,
}

/// A group's own coordinates and where they land on the page.
#[derive(Clone, Copy)]
struct Frame {
    /// Group coordinates: left, top, right, bottom.
    rect: [f64; 4],
    /// The group's box on the page before turning, in points.
    area: Anchor,
    rotation: f64,
    root_seq: Option<u32>,
}

impl Frame {
    /// Map a child's rectangle (in this group's coordinates) to the page.
    fn place(&self, r: [f64; 4]) -> Anchor {
        let (rw, rh) = (self.rect[2] - self.rect[0], self.rect[3] - self.rect[1]);
        let (sx, sy) = (if rw != 0.0 { self.area.width() / rw } else { 0.0 }, if rh != 0.0 { self.area.height() / rh } else { 0.0 });
        let a = Anchor {
            xs: self.area.xs + (r[0] - self.rect[0]) * sx,
            ys: self.area.ys + (r[1] - self.rect[1]) * sy,
            xe: self.area.xs + (r[2] - self.rect[0]) * sx,
            ye: self.area.ys + (r[3] - self.rect[1]) * sy,
        };
        // a turned group swings its children around its centre
        let (gx, gy) = ((self.area.xs + self.area.xe) / 2.0, (self.area.ys + self.area.ye) / 2.0);
        let (cx, cy) = ((a.xs + a.xe) / 2.0, (a.ys + a.ye) / 2.0);
        let (sin, cos) = self.rotation.to_radians().sin_cos();
        let (nx, ny) = (gx + (cx - gx) * cos - (cy - gy) * sin, gy + (cx - gx) * sin + (cy - gy) * cos);
        Anchor { xs: a.xs + nx - cx, ys: a.ys + ny - cy, xe: a.xe + nx - cx, ye: a.ye + ny - cy }
    }
}

/// Shapes nested in groups, by shape container offset. Their position is
/// relative to the group, so it has to be worked out from the outside in.
// shortcut: flipped groups aren't mirrored, add when a file needs it (FSP flags 0x40/0x80).
pub fn grouped(data: &[u8]) -> BTreeMap<usize, Placed> {
    let mut out = BTreeMap::new();
    let mut pos = 0;
    while pos + 8 <= data.len() {
        match header_at(data, pos) {
            Some(h) if h.rtype == SPGR_CONTAINER && h.ver == 0xF && pos + 8 + h.len as usize <= data.len() => {
                visit_group(data, pos + 8, pos + 8 + h.len as usize, None, &mut out, 0);
                pos += 8 + h.len as usize;
            }
            _ => pos += 1,
        }
    }
    out
}

fn visit_group(data: &[u8], start: usize, end: usize, parent: Option<Frame>, out: &mut BTreeMap<usize, Placed>, depth: u32) {
    if depth > 16 {
        return;
    }
    let mut frame: Option<Frame> = None;
    let mut pos = start;
    let mut first = true;
    while pos + 8 <= end {
        let Some(h) = header_at(data, pos) else { break };
        let (body, next) = (pos + 8, (pos + 8).saturating_add(h.len as usize));
        if next > end {
            break;
        }
        if h.rtype == SPGR_CONTAINER {
            visit_group(data, body, next, frame.or(parent), out, depth + 1);
        } else if h.rtype == SP_CONTAINER {
            let mut rect = None;
            let mut child = None;
            let mut client = None;
            let mut seq = None;
            let mut rotation = 0.0;
            for r in walk(data, body, next) {
                let d = r.body(data);
                let box_of = |d: &[u8]| Some([i32_at(d, 0)? as f64, i32_at(d, 4)? as f64, i32_at(d, 8)? as f64, i32_at(d, 12)? as f64]);
                match r.header.rtype {
                    FSPGR => rect = box_of(d),
                    CHILD_ANCHOR => child = box_of(d),
                    CLIENT_ANCHOR => client = Anchor::parse(d),
                    CLIENT_DATA => seq = client_fields(d).get(&FIELD_SHAPE_SEQ).map(|&v| v as u32),
                    FOPT => rotation = Fopt::parse(d, r.header.inst).get_i32(ROTATION_PID).map_or(0.0, |v| v as f64 / 65536.0),
                    _ => {}
                }
            }
            // where this shape lands: client anchors are page positions, child anchors are the parent's coordinates
            let context = frame.or(parent);
            let area = match (child, context) {
                (Some(c), Some(f)) => Some(f.place(c)),
                _ => client,
            };
            let turned = context.map_or(0.0, |f| f.rotation);
            let root_seq = context.and_then(|f| f.root_seq).or(seq);
            if first && rect.is_some() {
                // the group's own header shape; the top-level drawing's has no box at all
                if let (Some(rect), Some(area)) = (rect, area) {
                    frame = Some(Frame { rect, area, rotation: turned + rotation, root_seq });
                }
            } else if let (Some(f), Some(area)) = (context, area.filter(|_| child.is_some())) {
                out.insert(pos, Placed { anchor: area, rotation: f.rotation, root_seq: f.root_seq.or(seq) });
            }
        }
        first = false;
        pos = next;
    }
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
