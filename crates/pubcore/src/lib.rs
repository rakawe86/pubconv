//! Core reader for Microsoft Publisher (.pub) files.
//!
//! Fills the two gaps that make existing converters look wrong:
//! WordArt (geometry-text properties libmspub ignores) and linked text
//! boxes (Publisher stores one story per chain and re-flows it on open).

pub mod contents;
pub mod escher;
pub mod flow;
pub mod fonts;
pub mod ole;
pub mod patch;
pub mod pdf;
pub mod quill;
pub mod render;
pub mod textbox;
pub mod wordart;

pub use ole::PubFile;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a Publisher file: missing stream {0}")]
    MissingStream(&'static str),
    #[error("malformed {0}")]
    Malformed(&'static str),
    #[error("font not found: {0}")]
    FontNotFound(String),
    #[error("PDF error: {0}")]
    Pdf(String),
}

pub type Result<T> = std::result::Result<T, Error>;

/// Little-endian readers that return `None` instead of panicking on short data.
pub(crate) fn u16_at(d: &[u8], pos: usize) -> Option<u16> {
    d.get(pos..pos + 2).map(|b| u16::from_le_bytes([b[0], b[1]]))
}

pub(crate) fn u32_at(d: &[u8], pos: usize) -> Option<u32> {
    d.get(pos..pos + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

pub(crate) fn i32_at(d: &[u8], pos: usize) -> Option<i32> {
    u32_at(d, pos).map(|v| v as i32)
}

pub(crate) fn utf16le(d: &[u8]) -> String {
    let units: Vec<u16> = d.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
}
