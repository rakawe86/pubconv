//! WordArt reader vs the Python prototype's dump of all 40 WordArt files.

mod common;

use common::{fixture, hex, testdata};
use pubcore::{wordart::wordart_shapes, PubFile};

#[test]
fn every_wordart_property_matches_the_prototype() {
    let expected = fixture("wordart.json");
    let mut checked = 0;
    for (rel, shapes) in expected.as_object().unwrap() {
        let file = PubFile::open(testdata(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"));
        let got = wordart_shapes(&file);
        let want = shapes.as_array().unwrap();
        assert_eq!(got.len(), want.len(), "{rel}: number of WordArt shapes");
        for (g, w) in got.iter().zip(want) {
            let ctx = format!("{rel} @{}", g.offset);
            assert_eq!(g.offset as u64, w["offset"].as_u64().unwrap(), "{ctx} offset");
            assert_eq!(g.shape_type as u64, w["shape_type"].as_u64().unwrap(), "{ctx} warp type");
            assert_eq!(g.text, w["gtextUNICODE"].as_str().unwrap(), "{ctx} text");
            assert_eq!(g.font, w["gtextFont"].as_str().unwrap(), "{ctx} font");
            let flags = hex(&w["gtextFlags"]).unwrap_or(0);
            assert_eq!(g.bold, flags & 0x20 != 0, "{ctx} bold");
            assert_eq!(g.italic, flags & 0x10 != 0, "{ctx} italic");
            assert_eq!(g.size, w["gtextSize"].as_f64(), "{ctx} size");
            assert_eq!(g.adjust.map(i64::from), w["adjust1"].as_i64(), "{ctx} bend");
            assert_eq!(g.fill_color, hex(&w["fillColor"]), "{ctx} fill colour");
            assert_eq!(g.line_color, hex(&w["lineColor"]), "{ctx} line colour");
            assert_eq!(g.fill_back_color, hex(&w["fillBackColor"]), "{ctx} gradient colour");
            let rot = w["rotation"].as_i64().map_or(0.0, |r| r as f64 / 65536.0);
            assert_eq!(g.rotation, rot, "{ctx} rotation");
            let a = g.anchor.expect("anchor");
            for (k, v) in [("xs", a.xs), ("ys", a.ys), ("xe", a.xe), ("ye", a.ye)] {
                assert!((v - w["anchor"][k].as_f64().unwrap()).abs() < 1e-6, "{ctx} anchor {k}");
            }
            checked += 1;
        }
    }
    assert!(checked >= 40, "checked {checked} WordArt shapes");
}

#[test]
fn anchor_matches_publishers_own_position() {
    // preset01 was placed by Publisher at x=72, y=100, 252 x 113.6 pt on a 612 x 792 page
    let file = PubFile::open(testdata("wordart/preset01.pub")).unwrap();
    let a = wordart_shapes(&file)[0].anchor.unwrap();
    assert!((a.xs + 306.0 - 72.0).abs() < 0.01);
    assert!((a.ys + 396.0 - 100.0).abs() < 0.01);
    assert!((a.width() - 252.0).abs() < 0.01);
    assert!((a.height() - 113.625).abs() < 0.01);
}

#[test]
fn reads_arabic_and_multiline_wordart() {
    let ar = wordart_shapes(&PubFile::open(testdata("wordart/text_arabic.pub")).unwrap());
    assert_eq!(ar[0].text, "مرحبا بكم");
    let ml = wordart_shapes(&PubFile::open(testdata("wordart/text_multiline.pub")).unwrap());
    assert_eq!(ml[0].text, "Line One\rLine Two");
}

#[test]
fn plain_text_boxes_are_not_wordart() {
    let file = PubFile::open(testdata("corpus/f01_plain_text.pub")).unwrap();
    assert!(wordart_shapes(&file).is_empty());
}
