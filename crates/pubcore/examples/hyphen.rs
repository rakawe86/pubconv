//! Print the hyphenation points PubConv's dictionary gives each word.
//!   cargo run --release -p pubcore --example hyphen -- Castellanos estimate O'Sullivan
use hyphenation::{Hyphenator, Language, Load, Standard};

fn main() {
    let mut d = Standard::from_embedded(Language::EnglishUS).expect("embedded dictionary");
    d.minima = (2, 2); // as the engine uses it
    for w in std::env::args().skip(1) {
        println!("{w}: {}", d.hyphenate(&w).into_iter().segments().collect::<Vec<_>>().join("-"));
    }
}
