#![allow(dead_code)]

use std::path::PathBuf;

pub fn testdata(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../testdata").join(rel)
}

pub fn fixture(name: &str) -> serde_json::Value {
    let text = std::fs::read_to_string(testdata(&format!("fixtures/{name}"))).expect("fixture");
    serde_json::from_str(&text).expect("valid json")
}

pub fn hex(v: &serde_json::Value) -> Option<u32> {
    v.as_str().map(|s| u32::from_str_radix(s.trim_start_matches("0x"), 16).expect("hex"))
}

/// Whitespace-normalised line text, for comparing with Publisher's PDF lines.
pub fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}
