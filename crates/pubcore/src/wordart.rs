//! WordArt: the geometry-text properties of [MS-ODRAW] 2.3.22, which
//! libmspub reads past. Everything needed to draw it lives in the shape's
//! property table plus Publisher's client anchor.

use crate::escher::{self, Anchor, Fopt, CLIENT_ANCHOR, FOPT, FSP};
use crate::{utf16le, PubFile};

pub mod pid {
    pub const ROTATION: u16 = 0x0004;
    pub const GTEXT_UNICODE: u16 = 0x00C0;
    pub const GTEXT_ALIGN: u16 = 0x00C2;
    pub const GTEXT_SIZE: u16 = 0x00C3;
    pub const GTEXT_SPACING: u16 = 0x00C4;
    pub const GTEXT_FONT: u16 = 0x00C5;
    pub const GTEXT_FLAGS: u16 = 0x00FF;
    pub const ADJUST_1: u16 = 0x0147;
    pub const FILL_TYPE: u16 = 0x0180;
    pub const FILL_COLOR: u16 = 0x0181;
    pub const FILL_BACK_COLOR: u16 = 0x0183;
    pub const FILL_BLIP: u16 = 0x0186;
    pub const FILL_ANGLE: u16 = 0x018B;
    pub const FILL_FOCUS: u16 = 0x018C;
    pub const FILL_TO_LEFT: u16 = 0x018D;
    pub const FILL_TO_TOP: u16 = 0x018E;
    pub const FILL_TO_RIGHT: u16 = 0x018F;
    pub const FILL_TO_BOTTOM: u16 = 0x0190;
    pub const FILL_FLAGS: u16 = 0x01BF;
    pub const LINE_COLOR: u16 = 0x01C0;
    pub const LINE_WIDTH: u16 = 0x01CB;
    pub const LINE_FLAGS: u16 = 0x01FF;
    pub const SHADOW_COLOR: u16 = 0x0201;
    pub const SHADOW_X: u16 = 0x0205;
    pub const SHADOW_Y: u16 = 0x0206;
    pub const SHADOW_FLAGS: u16 = 0x023F;
    pub const THREED_EXTRUDE_BACK: u16 = 0x0285;
    pub const THREED_EXTRUSION_COLOR: u16 = 0x0287;
    pub const THREED_OBJECT_FLAGS: u16 = 0x02BF;
    pub const THREED_ROT_Y: u16 = 0x02C0;
    pub const THREED_ROT_X: u16 = 0x02C1;
    pub const THREED_VIEW_X: u16 = 0x02CB;
    pub const THREED_VIEW_Y: u16 = 0x02CC;
    pub const THREED_VIEW_Z: u16 = 0x02CD;
    pub const THREED_ORIGIN_X: u16 = 0x02CE;
    pub const THREED_ORIGIN_Y: u16 = 0x02CF;
    pub const THREED_SKEW_ANGLE: u16 = 0x02D0;
    pub const THREED_SKEW_AMOUNT: u16 = 0x02D1;
    pub const THREED_STYLE_FLAGS: u16 = 0x02FF;
}

const GTEXT_PIDS: [u16; 8] = [0x00C0, 0x00C1, 0x00C2, 0x00C3, 0x00C4, 0x00C5, 0x00C6, 0x00FF];
const FLAG_ITALIC: u32 = 0x10;
const FLAG_BOLD: u32 = 0x20;
/// Letters stacked top to bottom (Publisher's vertical WordArt presets).
const FLAG_VERTICAL: u32 = 0x2000;
/// Fill-style booleans: fFilled (bit 4) is only meaningful when fUsefFilled (bit 20) is set.
const FILL_FILLED: u32 = 0x10;
const FILL_USE_FILLED: u32 = 0x0010_0000;

#[derive(Debug, Clone, PartialEq)]
pub struct WordArt {
    /// Escher stream it was found in and its shape container's offset.
    pub stream: String,
    pub offset: usize,
    /// Warp style: shape type 136 (plain) ..= 175 (can down).
    pub shape_type: u16,
    pub text: String,
    pub font: String,
    /// Points; `None` means Publisher's default (36 pt).
    pub size: Option<f64>,
    pub bold: bool,
    pub italic: bool,
    pub vertical: bool,
    pub spacing: Option<u32>,
    pub adjust: Option<i32>,
    /// Degrees.
    pub rotation: f64,
    /// Colours as stored (0x00BBGGRR, high byte set = scheme/system index).
    pub fill_color: Option<u32>,
    pub fill_back_color: Option<u32>,
    pub line_color: Option<u32>,
    pub fill_type: Option<u32>,
    pub filled: bool,
    pub anchor: Option<Anchor>,
    /// Contents sequence number (for finding its page).
    pub seq: Option<u32>,
    /// Inside a group: the outermost group's sequence number, which is what pages list.
    pub root_seq: Option<u32>,
    /// All its drawing properties (gradient, 3-D, shadow...).
    pub props: Fopt,
    /// Mirrored left-right / top-bottom.
    pub flip_h: bool,
    pub flip_v: bool,
}

impl WordArt {
    fn from_fopt(stream: &str, offset: usize, shape_type: u16, f: &Fopt) -> Self {
        let text = |p| f.complex.get(&p).map(|b| utf16le(b).trim_end_matches('\0').to_string());
        let flags = f.get(pid::GTEXT_FLAGS).unwrap_or(0);
        let fill_flags = f.get(pid::FILL_FLAGS).unwrap_or(0);
        WordArt {
            stream: stream.to_string(),
            offset,
            shape_type,
            text: text(pid::GTEXT_UNICODE).unwrap_or_default(),
            font: text(pid::GTEXT_FONT).unwrap_or_else(|| "Arial".into()),
            size: f.get(pid::GTEXT_SIZE).map(|v| v as f64 / 65536.0),
            bold: flags & FLAG_BOLD != 0,
            italic: flags & FLAG_ITALIC != 0,
            vertical: flags & FLAG_VERTICAL != 0,
            spacing: f.get(pid::GTEXT_SPACING),
            adjust: f.get_i32(pid::ADJUST_1),
            rotation: f.get_i32(pid::ROTATION).map_or(0.0, |v| v as f64 / 65536.0),
            fill_color: f.get(pid::FILL_COLOR),
            fill_back_color: f.get(pid::FILL_BACK_COLOR),
            line_color: f.get(pid::LINE_COLOR),
            fill_type: f.get(pid::FILL_TYPE),
            filled: !(fill_flags & FILL_USE_FILLED != 0 && fill_flags & FILL_FILLED == 0),
            anchor: None,
            seq: None,
            root_seq: None,
            props: f.clone(),
            flip_h: false,
            flip_v: false,
        }
    }

    pub fn is_vertical(&self) -> bool {
        (self.rotation - 90.0).abs() < 1.0
    }

    /// The sequence number the page lists for this shape.
    pub fn page_seq(&self) -> Option<u32> {
        self.root_seq.or(self.seq)
    }
}

/// All WordArt shapes in the file, in stream order.
pub fn wordart_shapes(file: &PubFile) -> Vec<WordArt> {
    let mut out = Vec::new();
    for (name, data) in file.escher_streams() {
        let short = name.trim_start_matches('/');
        let placed = escher::grouped(data);
        for start in escher::shape_containers(data) {
            let Some(h) = escher::header_at(data, start) else { continue };
            let mut shape_type = 0;
            let mut flags = 0;
            let mut current: Option<WordArt> = None;
            for rec in escher::walk(data, start + 8, start + 8 + h.len as usize) {
                match rec.header.rtype {
                    FSP => {
                        shape_type = rec.header.inst;
                        flags = crate::u32_at(rec.body(data), 4).unwrap_or(0);
                    }
                    FOPT => {
                        let f = Fopt::parse(rec.body(data), rec.header.inst);
                        if f.values.keys().any(|k| GTEXT_PIDS.contains(k)) {
                            current = Some(WordArt::from_fopt(short, start, shape_type, &f));
                        }
                    }
                    CLIENT_ANCHOR => {
                        if let Some(w) = current.as_mut() {
                            w.anchor = Anchor::parse(rec.body(data));
                        }
                    }
                    escher::CLIENT_DATA => {
                        if let Some(w) = current.as_mut() {
                            w.seq = escher::client_fields(rec.body(data)).get(&escher::FIELD_SHAPE_SEQ).map(|&v| v as u32);
                        }
                    }
                    _ => {}
                }
            }
            if let Some(w) = current.as_mut() {
                (w.flip_h, w.flip_v) = (flags & 0x40 != 0, flags & 0x80 != 0);
            }
            if let (Some(w), Some(p)) = (current.as_mut(), placed.get(&start)) {
                w.anchor = Some(p.anchor);
                w.rotation += p.rotation;
                w.root_seq = p.root_seq;
            }
            out.extend(current);
        }
    }
    out
}
