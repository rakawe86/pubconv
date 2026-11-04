//! Print every drawing shape's type, sequence number and properties, for
//! finding where Publisher stores a setting (diff two files' output).
//!
//!   cargo run --release -p pubcore --example dump_escher -- file.pub

fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_escher file.pub");
    let file = pubcore::PubFile::open(&path).expect("open");
    for s in pubcore::escher::shapes(&file) {
        println!("shape type {} seq {:?}", s.shape_type, s.seq);
        for (pid, v) in &s.fopt.values {
            println!("  {pid:#06x} = {v} ({v:#x})");
        }
        for (pid, v) in &s.fopt.complex {
            println!("  {pid:#06x} complex {} bytes", v.len());
        }
        for (pid, v) in &s.tertiary.values {
            println!("  tertiary {pid:#06x} = {v} ({v:#x})");
        }
    }
}
