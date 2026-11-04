//! PubConv conversion engine, shared by the Windows app and the command line.
//!
//! The bundled page engine (a trimmed LibreOffice, in `runtime/` next to the
//! exe) lays out each page; PubConv draws what it gets wrong (WordArt,
//! linked text boxes) on top. Nothing leaves the computer.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use pubcore::flow::{FlowParams, Flower};
use pubcore::fonts::FontLibrary;
use pubcore::render::{self, Plan};

pub const APP_NAME: &str = "PubConv";
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Files per page-engine run: small enough for smooth progress, large enough
/// that start-up time doesn't dominate.
const BATCH: usize = 8;

#[derive(Debug, Clone, Default)]
pub struct Options {
    /// Save all PDFs here; `None` saves each next to its .pub.
    pub out_dir: Option<PathBuf>,
    /// Page engine (soffice) to use instead of the bundled one.
    pub soffice: Option<PathBuf>,
    pub keep_temp: bool,
}

#[derive(Debug, Clone)]
pub struct FileDone {
    pub index: usize,
    pub src: PathBuf,
    /// Output PDF on success, a plain-English reason on failure.
    pub result: Result<PathBuf, String>,
    /// What PubConv fixed, e.g. "2 WordArt, 1 linked story".
    pub note: String,
}

#[derive(Debug, Clone)]
pub enum Event {
    Started { total: usize },
    File(FileDone),
}

#[derive(Debug, Clone, Default)]
pub struct Summary {
    pub ok: usize,
    pub failed: Vec<(PathBuf, String)>,
    pub out_dirs: BTreeSet<PathBuf>,
    pub cancelled: bool,
}

pub fn is_pub(p: &Path) -> bool {
    p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pub"))
}

/// .pub files in `inputs` (files or folders), in a stable order, without duplicates.
pub fn collect_pub_files(inputs: &[PathBuf], recurse: bool) -> Vec<PathBuf> {
    fn walk(path: &Path, recurse: bool, out: &mut Vec<PathBuf>) {
        if path.is_dir() {
            let Ok(rd) = std::fs::read_dir(path) else { return };
            let mut entries: Vec<PathBuf> = rd.filter_map(|e| e.ok().map(|e| e.path())).collect();
            entries.sort();
            for p in entries {
                if p.is_dir() {
                    if recurse {
                        walk(&p, recurse, out);
                    }
                } else if is_pub(&p) {
                    out.push(p);
                }
            }
        } else if is_pub(path) {
            out.push(path.to_path_buf());
        }
    }
    let mut out = Vec::new();
    for i in inputs {
        walk(i, recurse, &mut out);
    }
    let mut seen = BTreeSet::new();
    out.retain(|p| seen.insert(p.canonicalize().unwrap_or_else(|_| p.clone())));
    out
}

/// The page engine: explicit path, $PUBCONV_SOFFICE, the copy bundled with
/// PubConv, or an installed LibreOffice (developer setups).
pub fn find_soffice(explicit: Option<&Path>) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(Path::to_path_buf));
    let mut candidates: Vec<PathBuf> = explicit.map(Path::to_path_buf).into_iter().collect();
    if let Some(v) = std::env::var_os("PUBCONV_SOFFICE") {
        candidates.push(v.into());
    }
    if let Some(d) = &exe_dir {
        candidates.push(d.join("runtime/program/soffice.com"));
        candidates.push(d.join("libreoffice/program/soffice.com"));
    }
    for base in ["ProgramFiles", "ProgramFiles(x86)"] {
        if let Some(pf) = std::env::var_os(base) {
            candidates.push(PathBuf::from(pf).join("LibreOffice/program/soffice.com"));
        }
    }
    candidates.into_iter().find(|p| p.is_file())
}

fn file_url(p: &Path) -> String {
    let s = p.to_string_lossy().replace('\\', "/");
    if s.starts_with('/') { format!("file://{s}") } else { format!("file:///{s}") }
}

struct Job {
    index: usize,
    src: PathBuf,
    work: PathBuf,
    out: PathBuf,
    plan: Option<Plan>,
    note: String,
}

fn describe(plan: &Plan) -> String {
    let mut parts = Vec::new();
    if !plan.wordart.is_empty() {
        parts.push(format!("{} WordArt", plan.wordart.len()));
    }
    if !plan.chains.is_empty() {
        parts.push(format!("{} linked stor{}", plan.chains.len(), if plan.chains.len() == 1 { "y" } else { "ies" }));
    }
    if parts.is_empty() { "Converted".into() } else { format!("Fixed {}", parts.join(", ")) }
}

/// Convert `files`, reporting each result through `on_event`. Checks
/// `cancel` between batches.
pub fn convert(
    files: &[PathBuf],
    opts: &Options,
    cancel: &AtomicBool,
    mut on_event: impl FnMut(Event),
) -> Result<Summary, String> {
    if files.is_empty() {
        return Err("No Publisher (.pub) files were found.".into());
    }
    let soffice = find_soffice(opts.soffice.as_deref())
        .ok_or("The page engine that ships with PubConv is missing. Please reinstall PubConv.")?;
    let temp = std::env::temp_dir().join(format!("pubconv-{}", std::process::id()));
    std::fs::create_dir_all(&temp).map_err(|e| format!("Couldn't create a working folder ({e})."))?;
    let profile = file_url(&temp.join("profile"));
    let fonts = FontLibrary::system();
    let flower = Flower::new(FlowParams::default());
    let mut summary = Summary::default();
    let mut used_outputs = BTreeSet::new();
    on_event(Event::Started { total: files.len() });

    for (b, batch) in files.chunks(BATCH).enumerate() {
        if cancel.load(Ordering::Relaxed) {
            summary.cancelled = true;
            break;
        }
        let mut jobs = Vec::new();
        for (k, src) in batch.iter().enumerate() {
            let index = b * BATCH + k;
            let out_dir = opts.out_dir.clone().unwrap_or_else(|| src.parent().unwrap_or(Path::new(".")).to_path_buf());
            let out = unique_output(&out_dir, src, &mut used_outputs);
            let work = temp.join(format!("f{index:06}.pub"));
            let (plan, note) = plan_file(src, &work, &fonts, &flower);
            if plan.is_none() {
                let _ = std::fs::copy(src, &work);
            }
            jobs.push(Job { index, src: src.clone(), work, out, plan, note });
        }

        // one damaged file can stall a whole batch: then give each file the
        // batch didn't finish its own run, so only the stuck one is skipped
        let mut timed_out = BTreeSet::new();
        if run_engine(&soffice, &profile, &temp, &jobs)? {
            for j in &jobs {
                if j.work.with_extension("pdf").is_file() || cancel.load(Ordering::Relaxed) {
                    continue;
                }
                if run_engine(&soffice, &profile, &temp, std::slice::from_ref(j))? {
                    timed_out.insert(j.index);
                }
            }
        }

        for j in jobs {
            let result = finish(&j).map_err(|e| {
                if timed_out.contains(&j.index) { "This file took too long to convert and was skipped. It may be damaged.".into() } else { e }
            });
            match &result {
                Ok(out) => {
                    summary.ok += 1;
                    if let Some(d) = out.parent() {
                        summary.out_dirs.insert(d.to_path_buf());
                    }
                }
                Err(e) => summary.failed.push((j.src.clone(), e.clone())),
            }
            let note = if result.is_ok() { j.note.clone() } else { String::new() };
            on_event(Event::File(FileDone { index: j.index, src: j.src, result, note }));
        }
    }
    if !opts.keep_temp {
        let _ = std::fs::remove_dir_all(&temp);
    }
    Ok(summary)
}

/// Plan our fixes and write the patched copy. A damaged file can make the
/// reader fail or even panic; either way the file still gets a plain
/// conversion instead of stopping the whole batch.
fn plan_file(src: &Path, work: &Path, fonts: &FontLibrary, flower: &Flower) -> (Option<Plan>, String) {
    let attempt = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        let plan = render::plan(&pubcore::PubFile::open(src)?, fonts, flower)?;
        pubcore::patch::prepare_for_plan(src, work, &plan)?;
        Ok::<_, pubcore::Error>(plan)
    }));
    match attempt {
        Ok(Ok(plan)) => {
            let note = describe(&plan);
            (Some(plan), note)
        }
        _ => {
            let _ = std::fs::remove_file(work); // a half-written copy must not be converted
            (None, "Converted (standard layout)".into())
        }
    }
}

/// `name.pdf` in `dir`, or `name (2).pdf`, `name (3).pdf`… if another file in
/// this run already uses that name (same-named files from different folders).
fn unique_output(dir: &Path, src: &Path, used: &mut BTreeSet<PathBuf>) -> PathBuf {
    let stem = src.file_stem().unwrap_or_default().to_string_lossy().into_owned();
    let key = |p: &Path| PathBuf::from(p.to_string_lossy().to_lowercase());
    let mut n = 1;
    loop {
        let name = if n == 1 { format!("{stem}.pdf") } else { format!("{stem} ({n}).pdf") };
        let candidate = dir.join(name);
        if used.insert(key(&candidate)) {
            return candidate;
        }
        n += 1;
    }
}

/// Run the page engine on one batch, with a time limit so a damaged file
/// can't hang the conversion. Returns whether the limit was hit.
fn run_engine(soffice: &Path, profile: &str, outdir: &Path, jobs: &[Job]) -> Result<bool, String> {
    use std::process::Stdio;
    use std::time::{Duration, Instant};
    let mut cmd = Command::new(soffice);
    cmd.arg(format!("-env:UserInstallation={profile}"))
        .args(["--headless", "--norestore", "--nologo", "--convert-to", "pdf", "--outdir"])
        .arg(outdir)
        .args(jobs.iter().map(|j| &j.work))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW: no console flashes from the app
    }
    let mut child = cmd
        .spawn()
        .map_err(|e| format!("The page engine couldn't start ({e}). Please reinstall PubConv."))?;
    let limit = Duration::from_secs(60 + 20 * jobs.len() as u64);
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return Ok(false),
            Ok(None) if start.elapsed() > limit => {
                let _ = child.kill();
                let _ = child.wait();
                return Ok(true);
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(format!("The page engine stopped unexpectedly ({e}).")),
        }
    }
}

fn finish(j: &Job) -> Result<PathBuf, String> {
    let base = j.work.with_extension("pdf");
    if !base.is_file() {
        return Err("This file couldn't be read. It may be damaged or from a very old Publisher version.".into());
    }
    if let Some(parent) = j.out.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("Couldn't create the output folder ({e})."))?;
    }
    let (drawings, page_h) = match &j.plan {
        Some(plan) => (plan.drawings.clone(), plan.page_size.1),
        None => (BTreeMap::new(), 792.0),
    };
    let written = if drawings.is_empty() {
        std::fs::copy(&base, &j.out).map(|_| ()).map_err(|e| e.to_string())
    } else {
        pubcore::pdf::overlay(&base, &j.out, &drawings, page_h).map_err(|e| e.to_string())
    };
    written.map_err(|e| {
        if e.contains("being used by another process") || e.contains("os error 32") {
            "The PDF is open in another program. Close it and try again.".to_string()
        } else {
            format!("Couldn't save the PDF ({e}).")
        }
    })?;
    Ok(j.out.clone())
}
