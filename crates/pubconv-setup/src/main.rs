//! PubConv's installer: a small window in the app's own style (ui/index.html in
//! WebView2) that unpacks the embedded app for the current user, with no admin
//! prompt. Built without a payload, the same program is uninstall.exe.

// release builds have no console window; debug builds keep one for diagnostics
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::borrow::Cow;
use std::fs::{self, File};
use std::io::{self, BufWriter};
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

use serde_json::{json, Value};
use tao::dpi::LogicalSize;
use tao::event::{Event, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder};
use tao::platform::windows::{IconExtWindows, WindowBuilderExtWindows};
use tao::window::{Icon, WindowBuilder};
use winapi::um::shlobj::{SHGetFolderPathW, CSIDL_DESKTOPDIRECTORY, CSIDL_LOCAL_APPDATA, CSIDL_PROGRAMS};
use wry::http::{header::CONTENT_TYPE, Response};
use wry::WebViewBuilder;

const PAYLOAD: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/payload.bin"));
const UI: &str = include_str!("../ui/index.html");
const LOGO: &[u8] = include_bytes!("../../pubconv-app/assets/pubconv.png");
const VERSION: &str = env!("CARGO_PKG_VERSION");
const ORIGIN: &str = "http://setup.localhost";
const UNINSTALL_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\PubConv";
const NO_WINDOW: u32 = 0x0800_0000; // CREATE_NO_WINDOW

/// One file in the payload (format written by packaging/build_release.py).
struct Entry<'a> {
    path: PathBuf,
    size: u64,
    deflated: &'a [u8],
}

/// Repeated [u32 path length][path][u64 size][u64 compressed size][zlib data], little-endian.
fn entries(mut p: &[u8]) -> Result<Vec<Entry<'_>>, String> {
    fn take<'a>(p: &mut &'a [u8], n: usize) -> Result<&'a [u8], String> {
        if n > p.len() {
            return Err("The installer file is damaged. Please download it again.".into());
        }
        let (head, rest) = p.split_at(n);
        *p = rest;
        Ok(head)
    }
    let mut out = Vec::new();
    while !p.is_empty() {
        let n = u32::from_le_bytes(take(&mut p, 4)?.try_into().unwrap()) as usize;
        let path = PathBuf::from(String::from_utf8_lossy(take(&mut p, n)?).as_ref());
        // never write outside the install folder
        if !path.components().all(|c| matches!(c, Component::Normal(_))) {
            return Err("The installer file is damaged. Please download it again.".into());
        }
        let size = u64::from_le_bytes(take(&mut p, 8)?.try_into().unwrap());
        let packed = u64::from_le_bytes(take(&mut p, 8)?.try_into().unwrap()) as usize;
        out.push(Entry { path, size, deflated: take(&mut p, packed)? });
    }
    Ok(out)
}

/// Unpack the payload into `dest`, reporting percent done.
fn extract(payload: &[u8], dest: &Path, mut progress: impl FnMut(u64)) -> Result<(), String> {
    let files = entries(payload)?;
    let total: u64 = files.iter().map(|f| f.size).sum::<u64>().max(1);
    let (mut done, mut shown) = (0, 0);
    for f in files {
        let out = dest.join(&f.path);
        let write = || -> io::Result<u64> {
            fs::create_dir_all(out.parent().unwrap_or(dest))?;
            let mut w = BufWriter::new(File::create(&out)?);
            io::copy(&mut flate2::read::ZlibDecoder::new(f.deflated), &mut w)
        };
        match write() {
            Ok(n) if n == f.size => done += n,
            Ok(_) => return Err("The installer file is damaged. Please download it again.".into()),
            Err(e) => return Err(format!("Couldn't write {} ({e}).", out.display())),
        }
        let percent = done * 100 / total;
        if percent != shown {
            shown = percent;
            progress(percent);
        }
    }
    Ok(())
}

fn known_folder(csidl: i32) -> Result<PathBuf, String> {
    let mut buf = [0u16; 260];
    let hr = unsafe { SHGetFolderPathW(std::ptr::null_mut(), csidl, std::ptr::null_mut(), 0, buf.as_mut_ptr()) };
    if hr < 0 {
        return Err(format!("Windows didn't report a system folder (error {hr:#x})."));
    }
    let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    Ok(PathBuf::from(String::from_utf16_lossy(&buf[..n])))
}

fn install_dir() -> Result<PathBuf, String> {
    Ok(known_folder(CSIDL_LOCAL_APPDATA)?.join("Programs").join("PubConv"))
}

fn start_menu_link() -> Result<PathBuf, String> {
    Ok(known_folder(CSIDL_PROGRAMS)?.join("PubConv.lnk"))
}

fn desktop_link() -> Result<PathBuf, String> {
    Ok(known_folder(CSIDL_DESKTOPDIRECTORY)?.join("PubConv.lnk"))
}

fn wide(s: &std::ffi::OsStr) -> Vec<u16> {
    s.encode_wide().chain(Some(0)).collect()
}

fn create_shortcut(lnk: &Path, target: &Path) -> Result<(), String> {
    use winapi::shared::winerror::SUCCEEDED;
    use winapi::um::combaseapi::{CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER};
    use winapi::um::objbase::COINIT_APARTMENTTHREADED;
    use winapi::um::objidl::IPersistFile;
    use winapi::um::shobjidl_core::{CLSID_ShellLink, IShellLinkW};
    use winapi::Interface;
    let failed = || format!("Couldn't create the shortcut {}.", lnk.display());
    unsafe {
        CoInitializeEx(std::ptr::null_mut(), COINIT_APARTMENTTHREADED);
        let mut link: *mut IShellLinkW = std::ptr::null_mut();
        let hr = CoCreateInstance(&CLSID_ShellLink, std::ptr::null_mut(), CLSCTX_INPROC_SERVER, &IShellLinkW::uuidof(), &mut link as *mut _ as *mut _);
        if !SUCCEEDED(hr) {
            return Err(failed());
        }
        (*link).SetPath(wide(target.as_os_str()).as_ptr());
        (*link).SetWorkingDirectory(wide(target.parent().unwrap_or(target).as_os_str()).as_ptr());
        let mut file: *mut IPersistFile = std::ptr::null_mut();
        let mut hr = (*link).QueryInterface(&IPersistFile::uuidof(), &mut file as *mut _ as *mut _);
        if SUCCEEDED(hr) {
            hr = (*file).Save(wide(lnk.as_os_str()).as_ptr(), 1);
            (*file).Release();
        }
        (*link).Release();
        if SUCCEEDED(hr) { Ok(()) } else { Err(failed()) }
    }
}

fn reg(args: &[&str]) -> Result<(), String> {
    let status = Command::new("reg").args(args).creation_flags(NO_WINDOW).output();
    match status {
        Ok(o) if o.status.success() => Ok(()),
        _ => Err("Couldn't update the Windows list of installed apps.".into()),
    }
}

fn install(desktop: bool, progress: impl FnMut(u64)) -> Result<(), String> {
    let dest = install_dir()?;
    // an earlier version: replace it, so no old engine files stay behind
    if dest.join("pubconv.exe").exists() && fs::remove_dir_all(&dest).is_err() {
        return Err("PubConv seems to be open. Close it and try again.".into());
    }
    extract(PAYLOAD, &dest, progress)?;
    let exe = dest.join("pubconv.exe");
    create_shortcut(&start_menu_link()?, &exe)?;
    if desktop {
        create_shortcut(&desktop_link()?, &exe)?;
    }
    let uninstall = format!("\"{}\"", dest.join("uninstall.exe").display());
    let kilobytes = (entries(PAYLOAD)?.iter().map(|f| f.size).sum::<u64>() / 1024).to_string();
    let exe = exe.display().to_string();
    let dir = dest.display().to_string();
    for (name, kind, value) in [
        ("DisplayName", "REG_SZ", "PubConv"),
        ("DisplayVersion", "REG_SZ", VERSION),
        ("Publisher", "REG_SZ", "PubConv"),
        ("DisplayIcon", "REG_SZ", exe.as_str()),
        ("InstallLocation", "REG_SZ", dir.as_str()),
        ("UninstallString", "REG_SZ", uninstall.as_str()),
        ("URLInfoAbout", "REG_SZ", "https://github.com/rakawe86/pubconv"),
        ("NoModify", "REG_DWORD", "1"),
        ("NoRepair", "REG_DWORD", "1"),
        ("EstimatedSize", "REG_DWORD", kilobytes.as_str()),
    ] {
        reg(&["add", UNINSTALL_KEY, "/v", name, "/t", kind, "/d", value, "/f"])?;
    }
    Ok(())
}

/// The folder this uninstaller may delete: only a real PubConv install.
fn own_install_dir() -> Result<PathBuf, String> {
    let dir = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf));
    match dir {
        Some(d) if d.file_name().is_some_and(|n| n == "PubConv") && d.join("uninstall.exe").exists() => Ok(d),
        _ => Err("This uninstaller isn't inside a PubConv installation, so it removed nothing.".into()),
    }
}

fn uninstall() -> Result<(), String> {
    let dir = own_install_dir()?;
    let exe = dir.join("pubconv.exe");
    if exe.exists() && fs::remove_file(&exe).is_err() {
        return Err("PubConv seems to be open. Close it and try again.".into());
    }
    for lnk in [start_menu_link()?, desktop_link()?] {
        let _ = fs::remove_file(lnk);
    }
    let _ = reg(&["delete", UNINSTALL_KEY, "/f"]);
    // only the app's cache and remembered settings; PDFs and .pub files are never in here
    let _ = fs::remove_dir_all(known_folder(CSIDL_LOCAL_APPDATA)?.join("PubConv"));
    Ok(())
}

/// A running exe can't delete itself: a hidden command does it just after we exit.
fn delete_install_folder_after_exit() {
    if let Ok(dir) = own_install_dir() {
        let _ = Command::new("cmd")
            .raw_arg(format!("/c ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"", dir.display()))
            .current_dir(std::env::temp_dir())
            .creation_flags(NO_WINDOW)
            .spawn();
    }
}

fn silent() -> i32 {
    let result = if PAYLOAD.is_empty() { uninstall() } else { install(true, |_| {}) };
    if PAYLOAD.is_empty() && result.is_ok() {
        delete_install_folder_after_exit();
    }
    i32::from(result.is_err())
}

fn message_box(text: &str) {
    let (text, title) = (wide(text.as_ref()), wide("PubConv".as_ref()));
    unsafe {
        winapi::um::winuser::MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), winapi::um::winuser::MB_ICONERROR);
    }
}

enum UserEvent {
    Ipc(String),
    ToPage(Value),
    Show,
}

fn main() {
    if std::env::args().any(|a| a == "--silent") {
        std::process::exit(silent());
    }
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let window = match WindowBuilder::new()
        .with_title(if PAYLOAD.is_empty() { "Uninstall PubConv" } else { "Install PubConv" })
        .with_inner_size(LogicalSize::new(640.0, 480.0))
        .with_resizable(false)
        .with_maximizable(false)
        .with_visible(false) // shown once the page has drawn, so there's no white flash
        .with_window_icon(Icon::from_resource(1, None).ok())
        .with_skip_taskbar(false)
        .build(&event_loop)
    {
        Ok(w) => w,
        Err(e) => {
            message_box(&format!("Couldn't open the window: {e}"));
            return;
        }
    };
    let p_ipc = proxy.clone();
    let mut context = wry::WebContext::new(Some(std::env::temp_dir().join("PubConvSetup")));
    let webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_custom_protocol("setup".into(), |_, req| {
            let (kind, body): (&str, &[u8]) = if req.uri().path() == "/logo.png" { ("image/png", LOGO) } else { ("text/html; charset=utf-8", UI.as_bytes()) };
            Response::builder().header(CONTENT_TYPE, kind).body(Cow::Borrowed(body)).unwrap()
        })
        .with_url(format!("{ORIGIN}/"))
        .with_navigation_handler(|url| url.starts_with(ORIGIN))
        .with_new_window_req_handler(|_, _| wry::NewWindowResponse::Deny)
        .with_devtools(cfg!(debug_assertions))
        .with_ipc_handler(move |req| {
            let _ = p_ipc.send_event(UserEvent::Ipc(req.into_body()));
        })
        .build(&window);
    let webview = match webview {
        Ok(w) => w,
        Err(e) => {
            message_box(&format!(
                "PubConv needs Microsoft Edge WebView2, which is part of Windows 10 and 11 but seems to be missing or damaged on this PC.\n\n\
                 Install it free from https://go.microsoft.com/fwlink/p/?LinkId=2124703 and run this again.\n\n({e})"
            ));
            return;
        }
    };

    // safety net: show the window even if the page never reports in
    let p_show = proxy.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(4));
        let _ = p_show.send_event(UserEvent::Show);
    });

    let (mut busy, mut uninstalled) = (false, false);
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        let _ = &context; // the WebView2 environment lives as long as the window
        let mut close = false;
        match event {
            Event::UserEvent(UserEvent::Show) => window.set_visible(true),
            Event::UserEvent(UserEvent::ToPage(e)) => {
                if e["type"] != "progress" {
                    busy = false;
                }
                uninstalled |= e["type"] == "done" && PAYLOAD.is_empty();
                let _ = webview.evaluate_script(&format!("window.setup.on({e})"));
            }
            Event::UserEvent(UserEvent::Ipc(body)) => {
                let m: Value = serde_json::from_str(&body).unwrap_or_default();
                match m["cmd"].as_str().unwrap_or("") {
                    "ready" => {
                        let dir = install_dir().map(|d| d.display().to_string()).unwrap_or_default();
                        let mode = if PAYLOAD.is_empty() { "uninstall" } else { "install" };
                        let init = json!({ "type": "init", "mode": mode, "version": VERSION, "dir": dir });
                        let _ = webview.evaluate_script(&format!("window.setup.on({init})"));
                        window.set_visible(true);
                    }
                    "run" if !busy => {
                        busy = true;
                        let (p, desktop) = (proxy.clone(), m["desktop"].as_bool().unwrap_or(true));
                        std::thread::spawn(move || {
                            let tick = p.clone();
                            let result = if PAYLOAD.is_empty() {
                                uninstall()
                            } else {
                                install(desktop, move |percent| {
                                    let _ = tick.send_event(UserEvent::ToPage(json!({ "type": "progress", "percent": percent })));
                                })
                            };
                            let e = match result {
                                Ok(()) => json!({ "type": "done" }),
                                Err(e) => json!({ "type": "error", "message": e }),
                            };
                            let _ = p.send_event(UserEvent::ToPage(e));
                        });
                    }
                    "open" => {
                        if let Ok(d) = install_dir() {
                            let _ = Command::new(d.join("pubconv.exe")).current_dir(&d).spawn();
                        }
                        close = true;
                    }
                    "close" => close = true,
                    _ => {}
                }
            }
            // closing mid-install would leave half a program behind
            Event::WindowEvent { event: WindowEvent::CloseRequested, .. } => close = !busy,
            _ => {}
        }
        if close {
            if uninstalled {
                delete_install_folder_after_exit();
            }
            *flow = ControlFlow::Exit;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};

    fn entry(path: &str, data: &[u8]) -> Vec<u8> {
        let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
        z.write_all(data).unwrap();
        let z = z.finish().unwrap();
        let mut out = (path.len() as u32).to_le_bytes().to_vec();
        out.extend(path.as_bytes());
        out.extend((data.len() as u64).to_le_bytes());
        out.extend((z.len() as u64).to_le_bytes());
        out.extend(z);
        out
    }

    #[test]
    fn unpacks_files_and_rejects_bad_payloads() {
        let dest = std::env::temp_dir().join(format!("pubconv-setup-test-{}", std::process::id()));
        let mut payload = entry("runtime/a.txt", b"hello");
        payload.extend(entry("b.txt", b"world"));
        let mut last = 0;
        extract(&payload, &dest, |p| last = p).unwrap();
        let mut text = String::new();
        File::open(dest.join("runtime/a.txt")).unwrap().read_to_string(&mut text).unwrap();
        assert_eq!((text.as_str(), last), ("hello", 100));
        fs::remove_dir_all(&dest).unwrap();

        assert!(extract(&entry("../evil.txt", b"x"), &dest, |_| {}).is_err());
        assert!(extract(&entry(r"C:\evil.txt", b"x"), &dest, |_| {}).is_err());
        assert!(extract(&payload[..payload.len() - 3], &dest, |_| {}).is_err());
        assert!(!dest.exists());
    }
}
