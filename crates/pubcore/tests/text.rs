//! Stories, boxes, styles and linked-text flow, all read from the .pub
//! itself, checked against Publisher's own results.

mod common;

use common::{fixture, norm, testdata};
use pubcore::contents::{linked_chains, page_size, text_links};
use pubcore::flow::{FlowParams, Flower};
use pubcore::fonts::FontLibrary;
use pubcore::quill::{stories, styled_stories, LineSpacing};
use pubcore::textbox::{chains, text_boxes};
use pubcore::PubFile;

fn open(rel: &str) -> PubFile {
    PubFile::open(testdata(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

#[test]
fn linked_boxes_share_one_story() {
    let one = open("linked/link3_mid.pub");
    let s = stories(&one).unwrap();
    assert_eq!(s.len(), 1);
    assert_eq!(s[0].text.chars().count(), 2600); // 520 words x 5 chars
    assert!(s[0].text.starts_with("w001 w002"));
    assert_eq!(linked_chains(&one)[&s[0].id].len(), 3, "three boxes in the chain");

    let two = open("linked/unlinked.pub");
    assert_eq!(stories(&two).unwrap().len(), 2);
    let ids: Vec<u32> = text_links(&two).iter().map(|l| l.story_id).collect();
    assert_eq!(ids, vec![2, 3]);
    assert!(linked_chains(&two).is_empty());
}

#[test]
fn only_real_linked_chains_are_found_across_the_corpus() {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(testdata("corpus")).unwrap() {
        let path = entry.unwrap().path();
        let file = PubFile::open(&path).unwrap();
        for c in chains(&file) {
            found.push(format!("{} ({} boxes)", path.file_stem().unwrap().to_string_lossy(), c.boxes.len()));
        }
    }
    found.sort();
    assert_eq!(found, vec!["f13_linked_text (2 boxes)", "r02_newsletter (2 boxes)"]);
}

#[test]
fn box_positions_and_margins_come_from_the_file() {
    let truth = fixture("linked_truth.json");
    for (name, t) in truth.as_object().unwrap() {
        let file = open(&format!("linked/{name}.pub"));
        let (pw, ph) = page_size(&file).expect("page size");
        assert_eq!((pw, ph), (612.0, 792.0), "{name}: letter page");
        let boxes = text_boxes(&file);
        let want = t["boxes"].as_array().unwrap();
        assert_eq!(boxes.len(), want.len(), "{name}: text boxes found");
        for (k, (b, w)) in boxes.iter().zip(want).enumerate() {
            let ctx = format!("{name} box{}", k + 1);
            assert!((b.anchor.xs + pw / 2.0 - w["left"].as_f64().unwrap()).abs() < 0.01, "{ctx} left");
            assert!((b.anchor.ys + ph / 2.0 - w["top"].as_f64().unwrap()).abs() < 0.01, "{ctx} top");
            assert!((b.anchor.width() - w["width"].as_f64().unwrap()).abs() < 0.01, "{ctx} width");
            assert!((b.anchor.height() - w["height"].as_f64().unwrap()).abs() < 0.01, "{ctx} height");
            assert_eq!(b.inset, [2.88; 4], "{ctx} inner margins");
        }
    }
}

#[test]
fn styles_come_from_the_file() {
    let (stories, defaults) = styled_stories(&open("linked/link2_big.pub")).unwrap();
    let first = &stories[0].char_runs[0].1;
    assert_eq!(first.font.as_deref(), Some("Arial"));
    assert_eq!(first.size, Some(18.0));
    // Publisher's "Normal" paragraph style: 6pt after, 1.19 lines
    assert_eq!(defaults.para.space_after, Some(6.0));
    match defaults.para.line_spacing {
        Some(LineSpacing::Lines(x)) => assert!((x - 1.19).abs() < 1e-6, "line spacing {x}"),
        other => panic!("line spacing {other:?}"),
    }
    let (n, _) = styled_stories(&open("corpus/r02_newsletter.pub")).unwrap();
    let news = n.iter().find(|s| s.story.text.contains("Garden project")).unwrap();
    assert_eq!(news.char_runs[0].1.size, Some(11.0));
}

#[test]
fn box_break_points_match_publisher() {
    let truth = fixture("linked_truth.json");
    let fonts = FontLibrary::system();
    let flower = Flower::new(FlowParams::default());
    let mut report = Vec::new();
    let mut checked = 0;
    for (name, t) in truth.as_object().unwrap() {
        if name == "unlinked" {
            continue;
        }
        let file = open(&format!("linked/{name}.pub"));
        let chain = chains(&file).into_iter().next().unwrap_or_else(|| panic!("{name}: no chain"));
        let flowed = flower.flow_chain(&file, &chain, &fonts).unwrap();
        for (k, (ours, theirs)) in flowed.boxes.iter().zip(t["boxes"].as_array().unwrap()).enumerate() {
            let expected = if k > 0 && theirs["length"].as_u64().unwrap() <= 1 {
                None // Publisher left this box empty
            } else {
                Some(theirs["start"].as_u64().unwrap() as usize)
            };
            checked += 1;
            if ours.start() != expected {
                report.push(format!("{name} box{}: publisher {expected:?}, ours {:?}", k + 1, ours.start()));
            }
        }
    }
    assert_eq!(checked, 17);
    assert!(report.is_empty(), "mismatches:\n{}", report.join("\n"));
}

#[test]
fn prose_lines_match_publisher_line_for_line() {
    let docs = fixture("prose.json");
    let fonts = FontLibrary::system();
    let flower = Flower::new(FlowParams::default());
    let mut report = Vec::new();
    for (name, d) in docs.as_object().unwrap() {
        let file = open(&format!("corpus/{name}.pub"));
        let needle = d["story_contains"].as_str().unwrap();
        let story_id = stories(&file).unwrap().into_iter().find(|s| s.text.contains(needle)).unwrap().id;
        let chain = chains(&file).into_iter().find(|c| c.story_id == story_id).expect("linked chain");
        let flowed = flower.flow_chain(&file, &chain, &fonts).unwrap();
        let (mut same, mut total) = (0, 0);
        for (k, (ours, theirs)) in flowed.boxes.iter().zip(d["publisher_lines"].as_array().unwrap()).enumerate() {
            let theirs = theirs.as_array().unwrap();
            for i in 0..ours.lines.len().max(theirs.len()) {
                let a = ours.lines.get(i).map_or("<none>".into(), |l| norm(&l.text));
                let b = theirs.get(i).and_then(|v| v.as_str()).map_or("<none>".into(), norm);
                total += 1;
                if a == b {
                    same += 1;
                } else {
                    report.push(format!("{name} box{} line{}:\n  pub:  {b}\n  ours: {a}", k + 1, i + 1));
                }
            }
        }
        println!("{name}: {same}/{total} lines identical");
    }
    assert!(report.is_empty(), "{} mismatching lines:\n{}", report.len(), report.join("\n"));
}
