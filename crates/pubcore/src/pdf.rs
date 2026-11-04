//! Draw a render plan onto LibreOffice's PDF.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use lopdf::{Dictionary, Document, Object, Stream};

use crate::render::{Codec, Image, Item, Seg};
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

/// One picture as a PDF image object.
fn add_image_object(doc: &mut Document, i: &Image) -> lopdf::ObjectId {
    let mut d = Dictionary::new();
    d.set("Type", Object::Name(b"XObject".to_vec()));
    d.set("Subtype", Object::Name(b"Image".to_vec()));
    d.set("Width", i.width as i64);
    d.set("Height", i.height as i64);
    d.set("BitsPerComponent", 8);
    d.set("Interpolate", true);
    let space = |n: u8| Object::Name(match n { 1 => b"DeviceGray".to_vec(), 4 => b"DeviceCMYK".to_vec(), _ => b"DeviceRGB".to_vec() });
    let compress = matches!(i.codec, Codec::Rgb);
    match i.codec {
        Codec::Rgb => d.set("ColorSpace", space(3)),
        Codec::Jpeg { components } => {
            d.set("ColorSpace", space(components));
            d.set("Filter", Object::Name(b"DCTDecode".to_vec()));
            if components == 4 {
                // Adobe's CMYK JPEGs store their values inverted
                d.set("Decode", vec![1.into(), 0.into(), 1.into(), 0.into(), 1.into(), 0.into(), 1.into(), 0.into()]);
            }
        }
        Codec::Png { colors } => {
            d.set("ColorSpace", space(colors));
            d.set("Filter", Object::Name(b"FlateDecode".to_vec()));
            let mut parms = Dictionary::new();
            parms.set("Predictor", 15);
            parms.set("Colors", colors as i64);
            parms.set("BitsPerComponent", 8);
            parms.set("Columns", i.width as i64);
            d.set("DecodeParms", parms);
        }
    }
    let mut s = Stream::new(d, i.data.clone());
    if compress {
        let _ = s.compress();
    } else {
        s = s.with_compression(false);
    }
    doc.add_object(s)
}

/// Paint a picture through the current clip (y down coordinates, flipped here).
fn paint_image(s: &mut String, fill: &crate::render::ImageFill, name: &str, h: f64) {
    let [x, top, w, ht] = fill.area;
    let (cx, cy) = (x + w / 2.0, h - (top + ht / 2.0));
    let (sin, cos) = fill.rotation.to_radians().sin_cos();
    // clockwise on the page, around the area's centre
    let _ = writeln!(s, "{cos:.5} {:.5} {sin:.5} {cos:.5} {:.3} {:.3} cm", -sin, cx - cx * cos - cy * sin, cy + cx * sin - cy * cos);
    let bottom = h - (top + ht);
    match fill.tile {
        // more tiles than this means tiny tiles: one stretched copy reads better than a huge file
        Some((tw, th)) if tw > 0.0 && th > 0.0 && (w / tw).ceil() * (ht / th).ceil() <= 2500.0 => {
            let mut y = top + ht;
            while y > top {
                let mut px = x;
                while px < x + w {
                    let _ = writeln!(s, "q {tw:.3} 0 0 {th:.3} {px:.3} {:.3} cm /{name} Do Q", h - y);
                    px += tw;
                }
                y -= th;
            }
        }
        _ => {
            let _ = writeln!(s, "q {w:.3} 0 0 {ht:.3} {x:.3} {bottom:.3} cm /{name} Do Q");
        }
    }
}

fn image_name(fill: &crate::render::ImageFill) -> String {
    format!("PubConvImg{:x}", std::sync::Arc::as_ptr(&fill.image) as usize)
}

fn content(items: &[Item], h: f64) -> Vec<u8> {
    let mut s = String::from("q\n");
    let y = |v: f64| h - v;
    for it in items {
        // invisible text (render mode 3): searchable and copyable, never painted
        for t in &it.text {
            let _ = writeln!(
                s,
                "BT 3 Tr /{HIDDEN_FONT} {:.3} Tf {:.2} Tz {:.5} {:.5} {:.5} {:.5} {:.3} {:.3} Tm {} Tj ET",
                t.size, t.scale, t.angle.to_radians().cos(), -t.angle.to_radians().sin(), t.angle.to_radians().sin(), t.angle.to_radians().cos(), t.x, y(t.y), hidden_codes(&t.text)
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
        let write_path = |s: &mut String| {
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
        };
        if let Some(fill) = &it.image {
            s.push_str("q\n");
            write_path(&mut s);
            s.push_str("W n\n");
            paint_image(&mut s, fill, &image_name(fill), h);
            s.push_str("Q\n");
        }
        write_path(&mut s);
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

/// Add a named object to a page's resources under `kind` (Font, XObject).
fn add_resource(doc: &mut Document, pid: lopdf::ObjectId, kind: &str, name: &str, object: lopdf::ObjectId) -> Result<()> {
    let entry = page_resources_mut(doc, pid)?.get(kind.as_bytes()).ok().cloned();
    match entry {
        Some(Object::Reference(fid)) => {
            doc.get_object_mut(fid).and_then(Object::as_dict_mut).map_err(pdf_err)?.set(name, Object::Reference(object));
        }
        Some(Object::Dictionary(mut d)) => {
            d.set(name, Object::Reference(object));
            page_resources_mut(doc, pid)?.set(kind, d);
        }
        _ => {
            let mut d = Dictionary::new();
            d.set(name, Object::Reference(object));
            page_resources_mut(doc, pid)?.set(kind, d);
        }
    }
    Ok(())
}

/// Write `output`: the PDF at `input` with each page's items drawn on top.
pub fn overlay(input: &Path, output: &Path, drawings: &BTreeMap<usize, Vec<Item>>, fallback_h: f64) -> Result<()> {
    let mut doc = Document::load(input).map_err(pdf_err)?;
    let pages = doc.get_pages();
    let mut hidden_font = None;
    // a WordArt on a master page repeats on every page: its picture is stored once
    let mut images: BTreeMap<String, lopdf::ObjectId> = BTreeMap::new();
    for (&idx, items) in drawings {
        let Some(&pid) = pages.get(&(idx as u32 + 1)) else { continue };
        if items.iter().any(|i| !i.text.is_empty()) {
            let font = *hidden_font.get_or_insert_with(|| add_hidden_font_object(&mut doc));
            add_resource(&mut doc, pid, "Font", HIDDEN_FONT, font)?;
        }
        for fill in items.iter().filter_map(|i| i.image.as_ref()) {
            let name = image_name(fill);
            let object = match images.get(&name) {
                Some(&o) => o,
                None => {
                    let o = add_image_object(&mut doc, &fill.image);
                    images.insert(name.clone(), o);
                    o
                }
            };
            add_resource(&mut doc, pid, "XObject", &name, object)?;
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
