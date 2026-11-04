//! What used to be PubConv's known limits, on files Publisher made
//! (tools/make_hard_corpus.py, the lim_* documents).

mod common;

use common::testdata;
use pubcore::escher;
use pubcore::flow::{FlowParams, Flower};
use pubcore::fonts::FontLibrary;
use pubcore::quill::styled_stories;
use pubcore::render::{plan, Plan};
use pubcore::textbox::{chains, text_boxes};
use pubcore::wordart::wordart_shapes;
use pubcore::PubFile;

fn open(name: &str) -> PubFile {
    PubFile::open(testdata(&format!("hard/{name}.pub"))).unwrap()
}

fn plan_of(name: &str) -> Plan {
    plan(&open(name), &FontLibrary::system(), &Flower::new(FlowParams::default())).unwrap()
}

#[test]
fn grouped_wordart_is_found_and_placed_like_publisher() {
    let file = open("lim_groups");
    let all = wordart_shapes(&file);
    assert_eq!(all.len(), 5);
    assert!(all.iter().all(|w| w.anchor.is_some() && w.page_seq().is_some()), "{all:?}");
    let p = plan_of("lim_groups");
    assert!(p.skipped.is_empty(), "{:?}", p.skipped);
    assert_eq!(p.wordart.len(), 5);
    // a plain group: where Publisher reported the shape (x 80, y 40), from the page centre
    let w = all.iter().find(|w| w.text == "Grouped").unwrap();
    let a = w.anchor.unwrap();
    assert!((a.xs + 306.0 - 80.0).abs() < 1.0 && (a.ys + 396.0 - 40.0).abs() < 1.0, "{a:?}");
    // the turned group's angle reaches its members
    assert_eq!(all.iter().find(|w| w.text == "Rotated").unwrap().rotation, 30.0);
    // nesting and the master page
    assert!(all.iter().any(|w| w.text == "Nested"));
    assert!(p.drawings[&0].len() > 3 && p.drawings[&1].len() > 3);
}

#[test]
fn linked_boxes_in_a_group_flow_as_a_chain() {
    let c = chains(&open("lim_groups"));
    assert_eq!(c.len(), 1);
    assert_eq!(c[0].boxes.len(), 2);
    assert!(escher::shapes(&open("lim_groups")).iter().filter(|s| s.root_seq.is_some()).count() >= 10);
}

#[test]
fn text_box_in_a_turned_group_is_drawn_turned() {
    let b = text_boxes(&open("lim_groups")).into_iter().find(|b| b.rotation != 0.0).expect("a turned box");
    assert_eq!(b.rotation, 30.0);
    let p = plan_of("lim_groups");
    assert!(p.chains.iter().any(|c| c.story_id == b.story_id()), "we draw it, not the page engine");
}

#[test]
fn gradients_and_pictures_paint_through_the_letters() {
    let p = plan_of("lim_wordart_fills");
    let painted = |page| p.drawings[&page].iter().filter(|i| i.image.is_some()).count();
    assert_eq!(painted(0), 7);
    assert_eq!(painted(1), 4, "fade, rainbow, texture, picture");
    let pictured: Vec<_> = p.drawings[&1].iter().filter_map(|i| i.image.as_ref()).collect();
    assert!(pictured.iter().any(|f| f.tile.is_some()), "textures repeat");
    assert_eq!(pictured.iter().filter(|f| matches!(f.image.codec, pubcore::render::Codec::Jpeg { .. })).count(), 2);
}

#[test]
fn depth_and_shadow_add_drawings_behind_the_front() {
    let p = plan_of("lim_wordart_3d");
    let items = &p.drawings[&0];
    assert!(items.len() > 7 * 2, "sides and a shadow as well as 7 fronts: {}", items.len());
}

#[test]
fn story_styles_and_colours_come_through() {
    let (stories, _) = styled_stories(&open("lim_mixed_story")).unwrap();
    let s = &stories[0];
    let red = [200.0 / 255.0, 30.0 / 255.0, 30.0 / 255.0];
    assert!(s.char_runs.iter().any(|(_, c)| c.color == Some(red)), "{:?}", s.char_runs);
    assert!(s.char_runs.iter().any(|(_, c)| c.font.as_deref() == Some("Verdana") && c.italic));
    let p = plan_of("lim_mixed_story");
    let colours: std::collections::BTreeSet<String> = p.drawings[&0].iter().map(|i| format!("{:?}", i.paint.fill)).collect();
    assert!(colours.len() >= 4, "black, red, green, blue: {colours:?}");
}

#[test]
fn mixed_lines_take_the_tallest_font_and_paragraph_alignment() {
    let file = open("lim_mixed_story");
    let chain = chains(&file).remove(0);
    let (f, _) = Flower::new(FlowParams::default()).flow_chain_styled(&file, &chain, &FontLibrary::system(), None).unwrap();
    let lines: Vec<_> = f.boxes.iter().flat_map(|b| b.lines.iter()).collect();
    let aligns: std::collections::BTreeSet<Option<u32>> = lines.iter().map(|l| l.align).collect();
    assert!(aligns.len() >= 3, "{aligns:?}");
    // the 20pt heading's lines are further apart than the 9pt paragraph's
    let pitch = |a: &pubcore::flow::Line, b: &pubcore::flow::Line| b.y - a.y;
    let first = pitch(lines[0], lines[1]);
    assert!((first - 25.44).abs() < 0.1, "Publisher's pitch for 20pt Arial Black: {first}");
}

#[test]
fn tight_wrap_follows_the_shape_not_its_box() {
    let boxes = text_boxes(&open("lim_tight_wrap"));
    let o: Vec<_> = boxes.iter().flat_map(|b| b.obstacles.iter()).collect();
    let oval = o.iter().find(|o| o.outline.len() > 20).expect("an oval's outline");
    let [l, t, r, _] = oval.rect;
    // at the very top an oval's edge is near its middle, far from the box's corners
    let (lo, hi) = oval.in_row(t + 3.0, t + 8.0).unwrap();
    assert!(lo > l + 40.0 && hi < r - 40.0, "{lo} {hi} in {l}..{r}");
    // around its middle it reaches (almost) the box's sides
    let (lo, hi) = oval.in_row((t + oval.rect[3]) / 2.0, (t + oval.rect[3]) / 2.0 + 10.0).unwrap();
    assert!(lo < l + 5.0 && hi > r - 5.0);
    assert!(oval.in_row(t - 50.0, t - 40.0).is_none());
}

#[test]
fn right_to_left_wordart_keeps_numbers_and_latin_readable() {
    // the hidden text is stored in page order: right-to-left letters turned, numbers not
    let p = plan_of("lim_rtl");
    let hidden: Vec<&str> = p.drawings[&0].iter().flat_map(|i| i.text.iter()).map(|t| t.text.as_str()).collect();
    assert!(hidden.iter().any(|t| t.contains("2026")), "{hidden:?}");
    assert!(hidden.iter().any(|t| t.contains("PubConv")), "{hidden:?}");
    assert!(hidden.iter().any(|t| t.contains("123")), "{hidden:?}");
    assert!(hidden.iter().any(|t| t.starts_with("مكب")), "turned: {hidden:?}");
    assert_eq!(hidden.iter().filter(|t| t.contains("السطر") || t.contains("رطسلا")).count(), 2, "one per line");
}

/// The drawn extent of the WordArt whose hidden text contains `word`.
fn drawn_size(p: &Plan, page: usize, word: &str) -> (f64, f64) {
    use pubcore::render::Seg;
    let item = p.drawings[&page].iter().find(|i| i.text.iter().any(|t| t.text.contains(word))).unwrap();
    let (mut x0, mut y0, mut x1, mut y1) = (f64::MAX, f64::MAX, f64::MIN, f64::MIN);
    for s in &item.path {
        if let Seg::Move(x, y) | Seg::Line(x, y) = *s {
            (x0, y0, x1, y1) = (x0.min(x), y0.min(y), x1.max(x), y1.max(y));
        }
    }
    (x1 - x0, y1 - y0)
}

#[test]
fn turned_wordart_is_laid_out_before_it_turns() {
    // turned a quarter: Office stores the box on its side, so the words must run down the page
    let p = plan_of("wa_rotated_styles");
    let (w, h) = drawn_size(&p, 0, "Gala");
    assert!(h > w * 1.5, "{w} x {h}");
}

#[test]
fn arches_keep_letters_readable_in_a_tall_box() {
    // Publisher's arch overflows its box by about half a letter on each side
    let file = open("wa_shapes_gallery");
    let box_w = wordart_shapes(&file).iter().find(|w| w.text == "Fun Run").unwrap().anchor.unwrap().width();
    let (w, _) = drawn_size(&plan_of("wa_shapes_gallery"), 0, "Fun");
    assert!(w > box_w * 0.95, "{w} vs box {box_w}");
}

#[test]
fn flipped_wordart_is_read_from_the_file() {
    let all = wordart_shapes(&open("wa_rotated_styles"));
    assert!(all.iter().any(|w| w.text == "Size 48" && (w.flip_h || w.flip_v)), "mirrored in Publisher");
}
