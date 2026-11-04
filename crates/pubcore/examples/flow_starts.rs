//! For tuning flow rules against Publisher: prints, per linked story, its
//! length and where each box's text starts (one JSON line per story).
//!
//!   cargo run --release -p pubcore --example flow_starts -- file.pub [continued_from_slots continued_on_slots]

use pubcore::flow::{FlowParams, Flower};
use pubcore::fonts::FontLibrary;
use pubcore::{textbox, PubFile};

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let file = PubFile::open(&args[0]).expect("open");
    let mut params = FlowParams::default();
    if let (Some(a), Some(b)) = (args.get(1).filter(|a| !a.starts_with("--")), args.get(2)) {
        params.continued_from_slots = a.parse().unwrap();
        params.continued_on_slots = b.parse().unwrap();
    }
    // for tuning: PUBCONV_ZONE overrides the hyphenation zone
    if let Some(z) = std::env::var("PUBCONV_ZONE").ok().and_then(|z| z.parse().ok()) {
        params.hyphen_zone = z;
    }
    let flower = Flower::new(params);
    let fonts = FontLibrary::system();
    // every story, including ones in a single box
    let mut by_story: std::collections::BTreeMap<u32, Vec<textbox::TextBox>> = Default::default();
    for b in textbox::text_boxes(&file) {
        by_story.entry(b.story_id()).or_default().push(b);
    }
    let all: Vec<textbox::Chain> = by_story.into_iter().map(|(story_id, boxes)| textbox::Chain { story_id, boxes }).collect();
    let chains = if args.iter().any(|a| a == "--all") { all } else { textbox::chains(&file) };
    for chain in chains {
        if args.iter().any(|a| a == "--obstacles") {
            for b in &chain.boxes {
                eprintln!("box {} at ({:.0},{:.0}) {:.0}x{:.0}: obstacles {:?}", b.seq, b.anchor.xs, b.anchor.ys, b.anchor.width(), b.anchor.height(), b.obstacles);
            }
        }
        let Ok(f) = flower.flow_chain(&file, &chain, &fonts) else { continue };
        // start of each chain box = start of its first column that has text
        let starts: Vec<String> = (0..chain.boxes.len())
            .map(|i| f.boxes.iter().filter(|b| b.geom.chain_box == i).find_map(|b| b.start()).map_or("null".into(), |s| s.to_string()))
            .collect();
        let flags: Vec<String> = chain.boxes.iter().map(|b| format!("\"{}{}\"", if b.continued_from { "F" } else { "" }, if b.continued_on { "O" } else { "" })).collect();
        let len = pubcore::quill::styled_stories(&file).ok()
            .and_then(|(s, _)| s.into_iter().find(|s| s.story.id == chain.story_id).map(|s| s.story.text.chars().count()))
            .unwrap_or(0);
        let placed = f.overset.first().map_or(len, |l| l.start);
        // --lines N: also print every line of chain box N (1-based)
        if let Some(n) = args.iter().position(|a| a == "--lines").and_then(|i| args.get(i + 1)).and_then(|n| n.parse::<usize>().ok()) {
            for b in f.boxes.iter().filter(|b| b.geom.chain_box + 1 == n) {
                eprintln!("box {n}: inset {:?} height {:.2} whole_slots {}", b.geom.inset, b.geom.height, b.geom.whole_slots);
                for l in &b.lines {
                    eprintln!("  y {:7.2} {}{}", l.y, l.text, if l.paragraph_end { " <P>" } else { "" });
                }
            }
        }
        println!("{{\"story\":{},\"len\":{len},\"placed\":{placed},\"starts\":[{}],\"flags\":[{}],\"align\":{:?}}}", chain.story_id, starts.join(","), flags.join(","), f.boxes.iter().flat_map(|b| b.lines.first()).next().and_then(|l| l.align).map_or(-1, |a| a as i64));
    }
}
