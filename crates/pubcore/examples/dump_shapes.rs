//! Print every shape chunk's blocks, for finding where Publisher stores a setting:
//! make two files that differ in one setting and diff this tool's output.
//!
//!   cargo run --release -p pubcore --example dump_shapes -- file.pub

use pubcore::contents::{chunk_refs, length_prefixed, Block};
use pubcore::PubFile;

fn show(d: &[u8], b: &Block, depth: usize) {
    let pad = "  ".repeat(depth);
    match b.value {
        Some(v) => println!("{pad}{:#04x}/{:#04x} = {v} ({v:#x})", b.id, b.btype),
        None if b.data_len > 4 && depth < 4 => {
            println!("{pad}{:#04x}/{:#04x} [{} bytes]", b.id, b.btype, b.data_len);
            for k in b.children(d) {
                show(d, &k, depth + 1);
            }
        }
        None => println!("{pad}{:#04x}/{:#04x} {:02x?}", b.id, b.btype, &d[b.data_offset..b.next.min(b.data_offset + 24)]),
    }
}

fn main() {
    let path = std::env::args().nth(1).expect("usage: dump_shapes file.pub");
    let file = PubFile::open(&path).expect("open");
    let d = file.stream(pubcore::ole::CONTENTS).expect("Contents stream");
    for c in chunk_refs(d) {
        println!("chunk seq {} type {:#x} at {:#x}", c.seq, c.ctype, c.offset);
        for b in length_prefixed(d, c.offset) {
            show(d, &b, 1);
        }
    }
}
