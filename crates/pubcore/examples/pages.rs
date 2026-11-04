//! Print which output page each text box and WordArt lands on.
fn main() {
    for f in std::env::args().skip(1) {
        let file = pubcore::PubFile::open(&f).expect("open");
        let p = pubcore::contents::pages(&file);
        println!("{f}");
        println!("  normal  {:?}", p.normal.iter().map(|x| (x.seq, x.shapes.clone())).collect::<Vec<_>>());
        println!("  masters {:?}", p.masters.iter().map(|x| (x.seq, x.shapes.clone())).collect::<Vec<_>>());
        let tb: Vec<_> = pubcore::textbox::text_boxes(&file).iter().map(|b| (b.seq, p.pages_of(b.seq))).collect();
        println!("  text boxes (seq, pages) {tb:?}");
    }
}
