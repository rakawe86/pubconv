//! System fonts and text measurement (advance widths, kerned as Publisher kerns).

use std::sync::Arc;

use fontdb::{Database, Family, Query, Stretch, Style, Weight};

use crate::{Error, Result};

pub struct FontLibrary {
    db: Database,
}

impl FontLibrary {
    pub fn system() -> Self {
        let mut db = Database::new();
        db.load_system_fonts();
        Self { db }
    }

    /// Load a face by family name, falling back to Arial if it's missing.
    pub fn face(&self, family: &str, bold: bool, italic: bool) -> Result<Font> {
        self.find(family, bold, italic)
            .or_else(|| self.find("Arial", bold, italic))
            .ok_or_else(|| Error::FontNotFound(family.to_string()))
    }

    fn find(&self, family: &str, bold: bool, italic: bool) -> Option<Font> {
        let weight = if bold { Weight::BOLD } else { Weight::NORMAL };
        self.find_exact(family, weight, italic).or_else(|| {
            // Publisher names some faces by their old-style family, e.g. "Arial Black",
            // which the font itself files as family "Arial", weight Black
            const WEIGHTS: [(&str, u16); 10] = [
                (" Extra Bold", 800), (" Ultra Bold", 800), (" Semibold", 600), (" Demibold", 600), (" Black", 900),
                (" Heavy", 900), (" Bold", 700), (" Medium", 500), (" Light", 300), (" Thin", 100),
            ];
            let (base, w) = WEIGHTS.iter().find_map(|(suffix, w)| family.strip_suffix(suffix).map(|b| (b, *w)))?;
            self.find_exact(base, Weight(w), italic).filter(|f| f.weight() >= w.saturating_sub(100))
        })
    }

    fn find_exact(&self, family: &str, weight: Weight, italic: bool) -> Option<Font> {
        let id = self.db.query(&Query {
            families: &[Family::Name(family)],
            weight,
            style: if italic { Style::Italic } else { Style::Normal },
            stretch: Stretch::Normal,
        })?;
        self.db.with_face_data(id, |data, index| Font { data: Arc::new(data.to_vec()), index })?
            .validate()
    }
}

/// A loaded font face. Cheap to clone.
#[derive(Clone)]
pub struct Font {
    data: Arc<Vec<u8>>,
    index: u32,
}

impl Font {
    fn validate(self) -> Option<Self> {
        let ok = self.face().is_some();
        ok.then_some(self)
    }

    fn face(&self) -> Option<ttf_parser::Face<'_>> {
        ttf_parser::Face::parse(&self.data, self.index).ok()
    }

    /// OS/2 weight class (400 regular, 700 bold, 900 black).
    pub fn weight(&self) -> u16 {
        self.face().map_or(400, |f| f.weight().to_number())
    }

    pub fn data(&self) -> &[u8] {
        &self.data
    }

    pub fn index(&self) -> u32 {
        self.index
    }

    /// Height of one "single" line as a multiple of the font size, the way
    /// Publisher sets it: the OS/2 typographic line (ascender - descender +
    /// line gap) for fonts with an OS/2 table of version 2 or later, else the
    /// hhea line. Matches its PDFs within 0.4% for Arial, Times, Georgia,
    /// Verdana, Trebuchet, Garamond, Century, Century Gothic and Bookman.
    pub fn line_em(&self) -> Option<f64> {
        let face = self.face()?;
        let upm = face.units_per_em() as f64;
        let os2 = face.tables().os2;
        let units = match os2 {
            Some(t) if t.version >= 2 => {
                t.typographic_ascender() as f64 - t.typographic_descender() as f64 + t.typographic_line_gap() as f64
            }
            _ => {
                let h = face.tables().hhea;
                h.ascender as f64 - h.descender as f64 + h.line_gap as f64
            }
        };
        (units > 0.0).then(|| units / upm)
    }

    pub fn sized(&self, size: f64) -> Measurer {
        Measurer { font: self.clone(), size, kerning: kerns_at(size) }
    }
}

/// Publisher's default pair kerning applies only to text of 14pt and up
/// (its line breaks on the hard corpus match best without kerning below that).
pub fn kerns_at(size: f64) -> bool {
    size >= KERN_FROM
}

const KERN_FROM: f64 = 14.0;

impl Font {
}

/// Measures text in points at one font size.
pub struct Measurer {
    font: Font,
    pub size: f64,
    pub kerning: bool,
}

impl Measurer {
    pub fn width(&self, s: &str) -> f64 {
        if self.kerning {
            // kerned text is measured exactly as it's drawn (GPOS and `kern`)
            if let Some(face) = rustybuzz::Face::from_slice(self.font.data(), self.font.index()) {
                let mut buf = rustybuzz::UnicodeBuffer::new();
                buf.push_str(s);
                buf.guess_segment_properties();
                let glyphs = rustybuzz::shape(&face, &[], buf);
                let units: i64 = glyphs.glyph_positions().iter().map(|p| p.x_advance as i64).sum();
                return units as f64 * self.size / face.units_per_em() as f64;
            }
        }
        let Some(face) = self.font.face() else { return 0.0 };
        let units: i64 = s.chars()
            .map(|c| face.glyph_hor_advance(face.glyph_index(c).unwrap_or(ttf_parser::GlyphId(0))).unwrap_or(0) as i64)
            .sum();
        units as f64 * self.size / face.units_per_em() as f64
    }

    /// Height of the glyphs themselves (ascender + descender), in points.
    pub fn glyph_height(&self) -> f64 {
        self.font.face().map_or(self.size, |f| {
            (f.ascender() as f64 - f.descender() as f64) * self.size / f.units_per_em() as f64
        })
    }

    pub fn ascent(&self) -> f64 {
        self.font.face().map_or(self.size * 0.9, |f| f.ascender() as f64 * self.size / f.units_per_em() as f64)
    }
}
