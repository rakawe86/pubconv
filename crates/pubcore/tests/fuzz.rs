//! Robustness: thousands of damaged copies of every sample file go through
//! every reader and the render planner. Nothing may panic; errors are fine.
//!
//! Runs in release builds only (it's slow unoptimised):
//!   cargo test --release -p pubcore --test fuzz

mod common;

use std::panic::{catch_unwind, AssertUnwindSafe};

use common::testdata;
use pubcore::flow::{FlowParams, Flower};
use pubcore::fonts::FontLibrary;
use pubcore::PubFile;

/// Small deterministic generator so failures are reproducible from the seed.
struct XorShift(u64);
impl XorShift {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

fn mutate(data: &[u8], rng: &mut XorShift) -> Vec<u8> {
    let mut d = data.to_vec();
    match rng.below(4) {
        0 => {
            // flip a few random bytes anywhere
            for _ in 0..1 + rng.below(20) {
                let i = rng.below(d.len());
                d[i] ^= 1 + rng.below(255) as u8;
            }
        }
        1 => {
            // overwrite a run with random bytes (damages one structure badly)
            let start = rng.below(d.len());
            let len = (1 + rng.below(512)).min(d.len() - start);
            for b in &mut d[start..start + len] {
                *b = rng.next() as u8;
            }
        }
        2 => {
            // set a random u32 to an extreme value (lengths, offsets, counts)
            let i = rng.below(d.len().saturating_sub(4));
            let v: u32 = [0, 1, 0x7FFF_FFFF, 0xFFFF_FFFF, 0x0001_0000][rng.below(5)];
            d[i..i + 4].copy_from_slice(&v.to_le_bytes());
        }
        _ => {
            // truncate
            d.truncate(rng.below(d.len()));
        }
    }
    d
}

#[test]
#[cfg_attr(debug_assertions, ignore = "slow unoptimised; run with --release")]
fn damaged_files_never_panic() {
    let fonts = FontLibrary::system();
    let flower = Flower::new(FlowParams::default());
    let dir = std::env::temp_dir().join("pubcore-fuzz");
    std::fs::create_dir_all(&dir).unwrap();
    let mut sources = Vec::new();
    for sub in ["corpus", "wordart", "linked"] {
        for e in std::fs::read_dir(testdata(sub)).unwrap() {
            sources.push(e.unwrap().path());
        }
    }
    let per_file = std::env::var("FUZZ_PER_FILE").ok().and_then(|v| v.parse().ok()).unwrap_or(60);
    let (mut opened, mut total, mut panics) = (0, 0, Vec::new());
    for (fi, src) in sources.iter().enumerate() {
        let original = std::fs::read(src).unwrap();
        for k in 0..per_file {
            let seed = 0x9E37_79B9_7F4A_7C15u64 ^ ((fi as u64) << 32 | k as u64);
            let mut rng = XorShift(seed | 1);
            let damaged = mutate(&original, &mut rng);
            let path = dir.join("case.pub");
            std::fs::write(&path, &damaged).unwrap();
            total += 1;
            let r = catch_unwind(AssertUnwindSafe(|| {
                let Ok(file) = PubFile::open(&path) else { return false };
                let _ = pubcore::wordart::wordart_shapes(&file);
                let _ = pubcore::quill::styled_stories(&file);
                let _ = pubcore::contents::pages(&file);
                let _ = pubcore::textbox::chains(&file);
                if let Ok(plan) = pubcore::render::plan(&file, &fonts, &flower) {
                    let _ = pubcore::patch::prepare_for_plan(&path, dir.join("out.pub"), &plan);
                }
                true
            }));
            match r {
                Ok(true) => opened += 1,
                Ok(false) => {}
                Err(_) => panics.push(format!("{} seed {seed:#x}", src.display())),
            }
        }
    }
    println!("{total} damaged files, {opened} still opened as .pub, {} panics", panics.len());
    assert!(panics.is_empty(), "panics:\n{}", panics.join("\n"));
}
