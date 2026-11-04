//! Same-size patches applied to a temporary copy before LibreOffice converts
//! it, so LibreOffice leaves alone what we draw ourselves.

use std::collections::BTreeMap;
use std::path::Path;

use crate::escher::{self, FSP};
use crate::{contents, ole, wordart, PubFile, Result};

/// First story id that no file uses, for boxes whose text we flow ourselves.
/// Each blanked box gets its own id from here up, so blanked boxes don't
/// look like a new linked chain.
pub const MISSING_STORY: u32 = 0x7FFF_0000;

/// Streams of `file` with every WordArt shape's type set to 0 (no preset
/// geometry), so LibreOffice stops drawing the empty WordArt outline.
pub fn neutralize_wordart(file: &PubFile) -> BTreeMap<String, Vec<u8>> {
    neutralize(file, &wordart::wordart_shapes(file))
}

/// As `neutralize_wordart`, for the given shapes only.
pub fn neutralize(file: &PubFile, shapes: &[wordart::WordArt]) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for w in shapes {
        let name = format!("/{}", w.stream);
        let data = out
            .entry(name.clone())
            .or_insert_with(|| file.stream(&name).unwrap_or_default().to_vec());
        let Some(h) = escher::header_at(data, w.offset) else { continue };
        let end = w.offset + 8 + h.len as usize;
        if let Some(fsp) = escher::walk(data, w.offset + 8, end).into_iter().find(|r| r.header.rtype == FSP) {
            data[fsp.at] &= 0x0F; // keep ver, clear the low instance bits
            data[fsp.at + 1] = 0;
        }
    }
    out
}

/// Contents stream with every shared story pointed at a story that doesn't
/// exist, so LibreOffice leaves linked boxes empty.
pub fn blank_shared_stories(file: &PubFile) -> BTreeMap<String, Vec<u8>> {
    let links: Vec<contents::TextLink> = contents::linked_chains(file).into_values().flatten().collect();
    blank(file, &links)
}

/// Point the given text links at stories that don't exist.
pub fn blank(file: &PubFile, links: &[contents::TextLink]) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    if links.is_empty() {
        return out;
    }
    let mut data = file.stream(ole::CONTENTS).unwrap_or_default().to_vec();
    for (i, link) in links.iter().enumerate() {
        let id = MISSING_STORY + i as u32;
        data[link.value_offset..link.value_offset + 4].copy_from_slice(&id.to_le_bytes());
    }
    out.insert(ole::CONTENTS.to_string(), data);
    out
}

/// Write a copy of `src` where LibreOffice skips exactly what `plan` draws.
pub fn prepare_for_plan(src: impl AsRef<Path>, dst: impl AsRef<Path>, plan: &crate::render::Plan) -> Result<()> {
    let file = PubFile::open(src.as_ref())?;
    let mut patches = neutralize(&file, &plan.wordart);
    let links: Vec<contents::TextLink> = plan.chains.iter().flat_map(|c| c.boxes.iter().map(|b| b.link)).collect();
    patches.extend(blank(&file, &links));
    ole::write_patched_copy(src, dst, &patches)
}

/// Write a copy of `src` with both patches applied to everything.
pub fn prepare_for_libreoffice(src: impl AsRef<Path>, dst: impl AsRef<Path>) -> Result<()> {
    let file = PubFile::open(src.as_ref())?;
    let mut patches = neutralize_wordart(&file);
    patches.extend(blank_shared_stories(&file));
    ole::write_patched_copy(src, dst, &patches)
}
