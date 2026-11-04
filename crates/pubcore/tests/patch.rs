//! Patched copies keep every stream's size and change only what we target.

mod common;

use common::testdata;
use pubcore::contents::{linked_chains, text_links};
use pubcore::patch::{prepare_for_libreoffice, MISSING_STORY};
use pubcore::wordart::wordart_shapes;
use pubcore::PubFile;

fn tmp(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join("pubcore-tests");
    std::fs::create_dir_all(&dir).unwrap();
    dir.join(name)
}

#[test]
fn wordart_geometry_is_neutralised_but_its_data_kept() {
    let src = testdata("corpus/r04_certificate.pub");
    let dst = tmp("cert.pub");
    prepare_for_libreoffice(&src, &dst).unwrap();
    let before = PubFile::open(&src).unwrap();
    let after = PubFile::open(&dst).unwrap();
    let (b, a) = (wordart_shapes(&before), wordart_shapes(&after));
    assert_eq!(b.len(), 1);
    assert_ne!(b[0].shape_type, 0);
    assert_eq!(a[0].shape_type, 0, "shape type cleared");
    assert_eq!(a[0].text, b[0].text, "WordArt text untouched");
    assert_eq!(a[0].anchor, b[0].anchor);
    for (name, data) in &before.streams {
        assert_eq!(data.len(), after.streams[name].len(), "{name} keeps its size");
    }
}

#[test]
fn shared_stories_are_blanked_and_others_untouched() {
    let src = testdata("linked/link3_mid.pub");
    let dst = tmp("link3.pub");
    prepare_for_libreoffice(&src, &dst).unwrap();
    let after = PubFile::open(&dst).unwrap();
    assert!(linked_chains(&after).is_empty());
    let ids: Vec<u32> = text_links(&after).iter().map(|l| l.story_id).collect();
    assert_eq!(ids, vec![MISSING_STORY, MISSING_STORY + 1, MISSING_STORY + 2]);

    // a file without linked boxes comes through byte-identical
    let plain = testdata("linked/unlinked.pub");
    let copy = tmp("unlinked.pub");
    prepare_for_libreoffice(&plain, &copy).unwrap();
    assert_eq!(
        PubFile::open(&plain).unwrap().streams,
        PubFile::open(&copy).unwrap().streams
    );
}
