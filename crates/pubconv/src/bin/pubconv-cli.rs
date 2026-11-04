//! PubConv command line, for IT staff and bulk jobs.
//!
//!   pubconv-cli <file.pub | folder>... [-o OUTDIR] [-r] [--soffice PATH] [--keep-temp]

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::AtomicBool;

use pubconv::{collect_pub_files, convert, Event, Options, APP_NAME, VERSION};

const USAGE: &str = "usage: pubconv-cli <file.pub | folder>... [-o OUTDIR] [-r] [--soffice PATH] [--keep-temp]

  -o, --out DIR     write PDFs here (default: next to each .pub)
  -r, --recurse     include .pub files in subfolders
  --soffice PATH    use this page engine (soffice.com) instead of the bundled one
  --keep-temp       keep the working folder (for troubleshooting)";

fn main() -> ExitCode {
    let mut inputs = Vec::new();
    let mut opts = Options::default();
    let mut recurse = false;
    let mut it = std::env::args_os().skip(1);
    while let Some(arg) = it.next() {
        match arg.to_string_lossy().as_ref() {
            "-h" | "--help" => {
                println!("{APP_NAME} {VERSION}\n\n{USAGE}");
                return ExitCode::SUCCESS;
            }
            "-o" | "--out" => match it.next() {
                Some(d) => opts.out_dir = Some(d.into()),
                None => return usage("-o needs a folder"),
            },
            "-r" | "--recurse" => recurse = true,
            "--soffice" => match it.next() {
                Some(p) => opts.soffice = Some(p.into()),
                None => return usage("--soffice needs a path"),
            },
            "--keep-temp" => opts.keep_temp = true,
            s if s.starts_with('-') => return usage(&format!("unknown option {s}")),
            _ => inputs.push(PathBuf::from(arg)),
        }
    }
    if inputs.is_empty() {
        return usage("give at least one .pub file or folder");
    }
    let files = collect_pub_files(&inputs, recurse);
    let cancel = AtomicBool::new(false);
    let result = convert(&files, &opts, &cancel, |e| match e {
        Event::Started { total } => println!("Converting {total} file(s)..."),
        Event::File(f) => match &f.result {
            Ok(out) => println!("OK    {}  ->  {}  [{}]", f.src.display(), out.display(), f.note),
            Err(e) => println!("FAIL  {}  ({e})", f.src.display()),
        },
    });
    match result {
        Ok(s) => {
            println!("\n{} converted, {} failed", s.ok, s.failed.len());
            if s.failed.is_empty() { ExitCode::SUCCESS } else { ExitCode::from(1) }
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

fn usage(msg: &str) -> ExitCode {
    eprintln!("{msg}\n\n{USAGE}");
    ExitCode::from(2)
}
