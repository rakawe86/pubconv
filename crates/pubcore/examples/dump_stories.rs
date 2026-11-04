//! Print every story's id, length and text (control characters as <U+XXXX>).
//!
//!   cargo run --release -p pubcore --example dump_stories -- file.pub [max_chars]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let file = pubcore::PubFile::open(&args[0]).expect("open");
    let max: usize = args.get(1).and_then(|a| a.parse().ok()).unwrap_or(80);
    if let Some(q) = file.stream(pubcore::ole::QUILL) {
        for c in pubcore::quill::chunks(q) {
            println!("chunk {} {} {} at {:#x} len {}", c.name, c.id, c.name2, c.offset, c.length);
        }
    }
    let (stories, _) = pubcore::quill::styled_stories(&file).expect("stories");
    for s in stories {
        let text: String = s.story.text.chars().take(max).map(|c| {
            if (c as u32) < 0x20 || (0xE000..0xF900).contains(&(c as u32)) { format!("<U+{:04X}>", c as u32) } else { c.to_string() }
        }).collect();
        println!("story {} len {}: {text}", s.story.id, s.story.text.chars().count());
        if args.iter().any(|a| a == "--styles") {
            println!("  char runs {:?}", s.char_runs);
            println!("  para runs {:?}", s.para_runs);
        }
    }
}
