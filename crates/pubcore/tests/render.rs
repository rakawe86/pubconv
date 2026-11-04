//! The render plan puts each fix on the right output page.

mod common;

use common::testdata;
use pubcore::flow::{FlowParams, Flower};
use pubcore::fonts::FontLibrary;
use pubcore::render::plan;
use pubcore::PubFile;

fn plan_for(rel: &str) -> pubcore::render::Plan {
    let fonts = FontLibrary::system();
    let flower = Flower::new(FlowParams::default());
    plan(&PubFile::open(testdata(rel)).unwrap(), &fonts, &flower).unwrap()
}

#[test]
fn wordart_is_drawn_on_its_page() {
    let p = plan_for("corpus/r04_certificate.pub");
    assert_eq!(p.wordart.len(), 1);
    assert!(p.skipped.is_empty(), "{:?}", p.skipped);
    assert_eq!(p.drawings.keys().copied().collect::<Vec<_>>(), vec![0]);
    assert!(p.drawings[&0][0].path.len() > 100, "real glyph outlines");
}

#[test]
fn linked_boxes_on_two_pages_are_drawn_on_both() {
    let p = plan_for("linked/link2_pages.pub");
    assert_eq!(p.chains.len(), 1);
    assert_eq!(p.drawings.keys().copied().collect::<Vec<_>>(), vec![0, 1]);
}

#[test]
fn files_without_our_fixes_get_no_drawings() {
    let p = plan_for("corpus/f08_table.pub");
    assert!(p.wordart.is_empty() && p.chains.is_empty() && p.drawings.is_empty());
}

#[test]
fn every_sample_file_plans_without_error() {
    for dir in ["corpus", "wordart", "linked"] {
        for e in std::fs::read_dir(testdata(dir)).unwrap() {
            let path = e.unwrap().path();
            let p = plan_for(&format!("{dir}/{}", path.file_name().unwrap().to_string_lossy()));
            assert!(p.skipped.is_empty(), "{}: {:?}", path.display(), p.skipped);
        }
    }
}
