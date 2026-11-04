//! Draw a render plan onto LibreOffice's PDF.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use lopdf::{Dictionary, Document, Object, Stream};

use crate::render::{Item, Seg};
use crate::{Error, Result};

fn pdf_err(e: lopdf::Error) -> Error {
    Error::Pdf(e.to_string())
}

/// Page height in points (MediaBox, inherited if needed).
fn page_height(doc: &Document, mut id: lopdf::ObjectId) -> Option<f64> {
    for _ in 0..16 {
        let dict = doc.get_dictionary(id).ok()?;
        if let Ok(mb) = dict.get(b"MediaBox").and_then(Object::as_array) {
            let n = |o: &Object| o.as_float().map(|v| v as f64).or_else(|_| o.as_i64().map(|v| v as f64)).ok();
            return Some(n(&mb[3])? - n(&mb[1])?);
        }
        id = dict.get(b"Parent").and_then(Object::as_reference).ok()?;
    }
    None
}

/// Resource name of the font used for the hidden text layer.
const HIDDEN_FONT: &str = "PubConvText";
/// Advance of every hidden-font glyph, in em (its /DW is 1000 x this).
pub const HIDDEN_ADVANCE: f64 = 0.5;

/// `s` as a hex string of 2-byte codes for the hidden Identity-H font: the
/// code is the Unicode code point, so every BMP character searches and copies
/// correctly; anything else becomes U+FFFD.
fn hidden_codes(s: &str) -> String {
    let mut out = String::from("<");
    for c in s.chars() {
        let u = c as u32;
        let u = if u <= 0xFFFF && !(0xD800..=0xDFFF).contains(&u) { u } else { 0xFFFD };
        let _ = write!(out, "{u:04X}");
    }
    out.push('>');
    out
}

/// ToUnicode CMap mapping each 2-byte code to the same Unicode code point.
fn identity_to_unicode() -> Vec<u8> {
    let mut s = String::from(
        "/CIDInit /ProcSet findresource begin
12 dict begin
begincmap
         /CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def
         /CMapName /Adobe-Identity-UCS def
/CMapType 2 def
         1 begincodespacerange
<0000> <FFFF>
endcodespacerange
",
    );
    // ranges may only vary in their last byte; surrogates have no characters
    let rows: Vec<u32> = (0u32..=0xFF).filter(|hi| !(0xD8..=0xDF).contains(hi)).collect();
    for chunk in rows.chunks(100) {
        let _ = writeln!(s, "{} beginbfrange", chunk.len());
        for hi in chunk {
            let _ = writeln!(s, "<{hi:02X}00> <{hi:02X}FF> <{hi:02X}00>");
        }
        s.push_str("endbfrange
");
    }
    s.push_str("endcmap
CMapName currentdict /CMap defineresource pop
end
end
");
    s.into_bytes()
}

/// The hidden text font: Type0, Identity-H, glyphs never drawn (render mode 3).
fn add_hidden_font_object(doc: &mut Document) -> lopdf::ObjectId {
    let name = || Object::Name(HIDDEN_FONT.as_bytes().to_vec());
    let mut descriptor = Dictionary::new();
    descriptor.set("Type", Object::Name(b"FontDescriptor".to_vec()));
    descriptor.set("FontName", name());
    descriptor.set("Flags", 32);
    descriptor.set("FontBBox", vec![0.into(), (-200).into(), 1000.into(), 900.into()]);
    descriptor.set("ItalicAngle", 0);
    descriptor.set("Ascent", 900);
    descriptor.set("Descent", -200);
    descriptor.set("CapHeight", 700);
    descriptor.set("StemV", 80);
    let descriptor = doc.add_object(descriptor);
    let mut info = Dictionary::new();
    info.set("Registry", Object::string_literal("Adobe"));
    info.set("Ordering", Object::string_literal("Identity"));
    info.set("Supplement", 0);
    let mut cid = Dictionary::new();
    cid.set("Type", Object::Name(b"Font".to_vec()));
    cid.set("Subtype", Object::Name(b"CIDFontType2".to_vec()));
    cid.set("BaseFont", name());
    cid.set("CIDSystemInfo", info);
    cid.set("FontDescriptor", Object::Reference(descriptor));
    cid.set("DW", (HIDDEN_ADVANCE * 1000.0) as i64);
    cid.set("CIDToGIDMap", Object::Name(b"Identity".to_vec()));
    let cid = doc.add_object(cid);
    let mut cmap = Stream::new(Dictionary::new(), identity_to_unicode());
    let _ = cmap.compress();
    let cmap = doc.add_object(cmap);
    let mut f = Dictionary::new();
    f.set("Type", Object::Name(b"Font".to_vec()));
    f.set("Subtype", Object::Name(b"Type0".to_vec()));
    f.set("BaseFont", name());
    f.set("Encoding", Object::Name(b"Identity-H".to_vec()));
    f.set("DescendantFonts", vec![Object::Reference(cid)]);
    f.set("ToUnicode", Object::Reference(cmap));
    doc.add_object(f)
}

fn content(items: &[Item], h: f64) -> Vec<u8> {
    let mut s = String::from("q\n");
    let y = |v: f64| h - v;
    for it in items {
        // invisible text (render mode 3): searchable and copyable, never painted
        for t in &it.text {
            let _ = writeln!(
                s,
                "BT 3 Tr /{HIDDEN_FONT} {:.3} Tf {:.2} Tz 1 0 0 1 {:.3} {:.3} Tm {} Tj ET",
                t.size, t.scale, t.x, y(t.y), hidden_codes(&t.text)
            );
        }
        if it.path.is_empty() {
            continue;
        }
        if let Some([r, g, b]) = it.paint.fill {
            let _ = writeln!(s, "{r:.3} {g:.3} {b:.3} rg");
        }
        if let Some(([r, g, b], w)) = it.paint.stroke {
            let _ = writeln!(s, "{r:.3} {g:.3} {b:.3} RG {w:.2} w 1 j");
        }
        for seg in &it.path {
            let _ = match *seg {
                Seg::Move(px, py) => writeln!(s, "{px:.3} {:.3} m", y(py)),
                Seg::Line(px, py) => writeln!(s, "{px:.3} {:.3} l", y(py)),
                Seg::Cubic(a, b, c, d, e, f) => {
                    writeln!(s, "{a:.3} {:.3} {c:.3} {:.3} {e:.3} {:.3} c", y(b), y(d), y(f))
                }
                Seg::Close => writeln!(s, "h"),
            };
        }
        s.push_str(match (it.paint.fill.is_some(), it.paint.stroke.is_some()) {
            (true, true) => "B\n",
            (true, false) => "f\n",
            (false, true) => "S\n",
            (false, false) => "n\n",
        });
    }
    s.push_str("Q\n");
    s.into_bytes()
}

/// Height in points of each page of a PDF, in order.
pub fn page_heights(path: &Path, fallback_h: f64) -> Result<Vec<f64>> {
    let doc = Document::load(path).map_err(pdf_err)?;
    Ok(doc.get_pages().values().map(|&id| page_height(&doc, id).unwrap_or(fallback_h)).collect())
}

/// The page's own Resources dictionary, made page-local if it was inherited
/// from the page tree, so adding to it can't affect other pages' resources.
fn page_resources_mut(doc: &mut Document, pid: lopdf::ObjectId) -> Result<&mut Dictionary> {
    // find what currently applies: on the page, or inherited from a parent
    let mut found: Option<Object> = None;
    let mut cur = pid;
    for _ in 0..16 {
        let dict = doc.get_dictionary(cur).map_err(pdf_err)?;
        if let Ok(r) = dict.get(b"Resources") {
            found = Some(r.clone());
            break;
        }
        match dict.get(b"Parent").and_then(Object::as_reference) {
            Ok(p) => cur = p,
            Err(_) => break,
        }
    }
    let inherited = cur != pid;
    let id = match found {
        Some(Object::Reference(rid)) if !inherited => rid,
        Some(Object::Reference(rid)) => {
            let copy = doc.get_dictionary(rid).map_err(pdf_err)?.clone();
            doc.add_object(copy)
        }
        Some(Object::Dictionary(d)) => doc.add_object(d),
        _ => doc.add_object(Dictionary::new()),
    };
    doc.get_object_mut(pid).and_then(Object::as_dict_mut).map_err(pdf_err)?.set("Resources", Object::Reference(id));
    doc.get_object_mut(id).and_then(Object::as_dict_mut).map_err(pdf_err)
}

fn add_hidden_font(doc: &mut Document, pid: lopdf::ObjectId, font: lopdf::ObjectId) -> Result<()> {
    let fonts_entry = page_resources_mut(doc, pid)?.get(b"Font").ok().cloned();
    match fonts_entry {
        Some(Object::Reference(fid)) => {
            doc.get_object_mut(fid).and_then(Object::as_dict_mut).map_err(pdf_err)?.set(HIDDEN_FONT, Object::Reference(font));
        }
        Some(Object::Dictionary(mut d)) => {
            d.set(HIDDEN_FONT, Object::Reference(font));
            page_resources_mut(doc, pid)?.set("Font", d);
        }
        _ => {
            let mut d = Dictionary::new();
            d.set(HIDDEN_FONT, Object::Reference(font));
            page_resources_mut(doc, pid)?.set("Font", d);
        }
    }
    Ok(())
}

/// Write `output`: the PDF at `input` with each page's items drawn on top.
pub fn overlay(input: &Path, output: &Path, drawings: &BTreeMap<usize, Vec<Item>>, fallback_h: f64) -> Result<()> {
    let mut doc = Document::load(input).map_err(pdf_err)?;
    let pages = doc.get_pages();
    let mut hidden_font = None;
    for (&idx, items) in drawings {
        let Some(&pid) = pages.get(&(idx as u32 + 1)) else { continue };
        if items.iter().any(|i| !i.text.is_empty()) {
            let font = *hidden_font.get_or_insert_with(|| add_hidden_font_object(&mut doc));
            add_hidden_font(&mut doc, pid, font)?;
        }
        let h = page_height(&doc, pid).unwrap_or(fallback_h);
        // isolate LibreOffice's content in q/Q so its last transform can't shift ours
        let existing: Vec<Object> = match doc.get_dictionary(pid).and_then(|d| d.get(b"Contents")) {
            Ok(Object::Reference(r)) => vec![Object::Reference(*r)],
            Ok(Object::Array(a)) => a.clone(),
            _ => Vec::new(),
        };
        let open = doc.add_object(Stream::new(Dictionary::new(), b"q\n".to_vec()));
        let close = doc.add_object(Stream::new(Dictionary::new(), b"\nQ\n".to_vec()));
        let mut ours = Stream::new(Dictionary::new(), content(items, h));
        let _ = ours.compress();
        let ours = doc.add_object(ours);
        let mut list = vec![Object::Reference(open)];
        list.extend(existing);
        list.push(Object::Reference(close));
        list.push(Object::Reference(ours));
        doc.get_object_mut(pid).and_then(Object::as_dict_mut).map_err(pdf_err)?.set("Contents", list);
    }
    doc.save(output)?;
    Ok(())
}
