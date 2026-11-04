//! PubConv for Windows: a full-window app whose interface (ui/index.html) runs
//! in the system WebView2 and talks to this process through small JSON
//! messages. Conversion runs on a worker thread; results are pushed back to
//! the page as they arrive, so the window never freezes.

// release builds have no console window; debug builds keep one for diagnostics
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::borrow::Cow;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde_json::{json, Value};
use tao::dpi::LogicalSize;
use tao::event::{Event as WinEvent, WindowEvent};
use tao::event_loop::{ControlFlow, EventLoopBuilder, EventLoopProxy};
use tao::platform::windows::WindowExtWindows;
use tao::window::{Window, WindowBuilder};
use wry::http::{header::CONTENT_TYPE, Response};
use wry::{DragDropEvent, WebContext, WebView, WebViewBuilder};

use pubconv::{collect_pub_files, convert, Event, Options, Summary, APP_NAME, VERSION};

/// The project page: help, and the donate link (kept there so it can change without a new release).
const HOME: &str = "https://github.com/rakawe86/pubconv";
const DONATE: &str = "https://www.paypal.com/ncp/payment/X45F9F8S58KL8";
const UI: &str = include_str!("../ui/index.html");
const ICON: &[u8] = include_bytes!("../assets/pubconv.ico");
const LOGO: &[u8] = include_bytes!("../assets/pubconv.png");
/// The page is served from this origin, so it gets normal storage (remembered settings).
const ORIGIN: &str = "http://pubconv.localhost";

enum UserEvent {
    /// A JSON message from the page.
    Ipc(String),
    /// Files dropped onto the window.
    Drop(Vec<PathBuf>),
    /// Something is being dragged over the window (true) or left it (false).
    Drag(bool),
    /// A JSON event for the page, from the worker thread.
    ToPage(Value),
    /// Show the window if the page hasn't asked for it yet.
    Show,
}

#[derive(Default)]
struct State {
    files: Vec<(u64, PathBuf)>,
    next_id: u64,
    recurse: bool,
    cancel: Option<Arc<AtomicBool>>,
    shown: bool,
}

struct App {
    window: Window,
    webview: WebView,
    proxy: EventLoopProxy<UserEvent>,
    state: State,
}

impl App {
    fn emit(&self, e: Value) {
        let _ = self.webview.evaluate_script(&format!("window.pc.on({e})"));
    }

    fn show(&mut self) {
        if !self.state.shown {
            self.state.shown = true;
            self.window.set_visible(true);
            self.window.set_focus();
        }
    }

    fn add_paths(&mut self, paths: Vec<PathBuf>) {
        if self.state.cancel.is_some() || paths.is_empty() {
            return;
        }
        let found = collect_pub_files(&paths, self.state.recurse);
        let mut added = Vec::new();
        for f in &found {
            if self.state.files.iter().any(|(_, p)| p == f) {
                continue;
            }
            self.state.next_id += 1;
            let id = self.state.next_id;
            self.state.files.push((id, f.clone()));
            added.push(json!({
                "id": id,
                "name": f.file_name().unwrap_or_default().to_string_lossy(),
                "dir": f.parent().map(|p| p.display().to_string()).unwrap_or_default(),
            }));
        }
        self.emit(json!({ "type": "added", "files": added, "found": found.len() }));
    }

    fn pick_files(&mut self) {
        let picked = rfd::FileDialog::new()
            .set_title("Choose Publisher files")
            .add_filter("Publisher files", &["pub"])
            .add_filter("All files", &["*"])
            .set_parent(&self.window)
            .pick_files();
        self.add_paths(picked.unwrap_or_default());
    }

    fn pick_folder(&self, title: &str) -> Option<PathBuf> {
        rfd::FileDialog::new().set_title(title).set_parent(&self.window).pick_folder()
    }

    fn start(&mut self, ids: &[u64], out_dir: Option<PathBuf>) {
        if self.state.cancel.is_some() {
            return;
        }
        let jobs: Vec<(u64, PathBuf)> = self.state.files.iter().filter(|(id, _)| ids.contains(id)).cloned().collect();
        if jobs.is_empty() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.state.cancel = Some(cancel.clone());
        self.emit(json!({ "type": "started", "ids": jobs.iter().map(|(id, _)| *id).collect::<Vec<_>>() }));
        let proxy = self.proxy.clone();
        std::thread::spawn(move || {
            let (ids, files): (Vec<u64>, Vec<PathBuf>) = jobs.into_iter().unzip();
            let opts = Options { out_dir, ..Options::default() };
            let send = |e: Value| {
                let _ = proxy.send_event(UserEvent::ToPage(e));
            };
            let result = convert(&files, &opts, &cancel, |e| {
                if let Event::File(f) = e {
                    send(match f.result {
                        Ok(out) => json!({ "type": "file", "id": ids[f.index], "ok": true, "note": f.note, "out": out }),
                        Err(err) => json!({ "type": "file", "id": ids[f.index], "ok": false, "error": err }),
                    });
                }
            });
            send(finished_event(result));
        });
    }

    fn on_ipc(&mut self, body: &str) {
        let Ok(m) = serde_json::from_str::<Value>(body) else { return };
        let path = |k: &str| m.get(k).and_then(Value::as_str).map(PathBuf::from);
        if let Some(r) = m.get("recurse").and_then(Value::as_bool) {
            self.state.recurse = r;
        }
        match m["cmd"].as_str().unwrap_or("") {
            "ready" => {
                self.emit(json!({ "type": "init", "version": VERSION }));
                self.show();
                // files passed on the command line ("Open with", or dropped on the shortcut)
                let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
                self.add_paths(args);
            }
            "addFiles" => self.pick_files(),
            "addFolder" => {
                if let Some(d) = self.pick_folder("Choose a folder of Publisher files") {
                    self.add_paths(vec![d]);
                }
            }
            "chooseOutDir" => {
                if let Some(d) = self.pick_folder("Choose where to save the PDFs") {
                    self.emit(json!({ "type": "outDir", "path": d }));
                }
            }
            "remove" => {
                let ids: Vec<u64> = m["ids"].as_array().map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
                self.state.files.retain(|(id, _)| !ids.contains(id));
            }
            "clear" => self.state.files.clear(),
            "start" => {
                let ids: Vec<u64> = m["ids"].as_array().map(|a| a.iter().filter_map(Value::as_u64).collect()).unwrap_or_default();
                self.start(&ids, path("outDir"));
            }
            "cancel" => {
                if let Some(c) = &self.state.cancel {
                    c.store(true, Ordering::Relaxed);
                }
            }
            "open" | "openDir" => {
                if let Some(p) = path("path") {
                    shell_open("open", &p.to_string_lossy(), None);
                }
            }
            "reveal" => {
                if let Some(p) = path("path") {
                    shell_open("open", "explorer.exe", Some(&format!("/select,\"{}\"", p.display())));
                }
            }
            "url" => {
                let url = if m["which"] == "donate" { DONATE.to_string() } else { format!("{HOME}#readme") };
                shell_open("open", &url, None);
            }
            _ => {}
        }
    }
}

fn finished_event(result: Result<Summary, String>) -> Value {
    match result {
        Err(e) => json!({ "type": "finished", "error": e, "ok": 0, "failed": 0 }),
        Ok(s) => json!({
            "type": "finished",
            "ok": s.ok,
            "failed": s.failed.len(),
            "cancelled": s.cancelled,
            "outDir": s.out_dirs.iter().next(),
        }),
    }
}

fn wide(s: &str) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;
    std::ffi::OsStr::new(s).encode_wide().chain(Some(0)).collect()
}

fn shell_open(verb: &str, target: &str, params: Option<&str>) {
    let (verb, target) = (wide(verb), wide(target));
    let params = params.map(wide);
    unsafe {
        winapi::um::shellapi::ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            target.as_ptr(),
            params.as_ref().map_or(std::ptr::null(), |p| p.as_ptr()),
            std::ptr::null(),
            winapi::um::winuser::SW_SHOWNORMAL,
        );
    }
}

fn message_box(text: &str) {
    let (text, title) = (wide(text), wide(APP_NAME));
    unsafe {
        winapi::um::winuser::MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), winapi::um::winuser::MB_ICONERROR);
    }
}

/// Set the window's big and small icons from the embedded .ico.
fn set_window_icon(window: &Window) {
    use winapi::um::winuser::{
        CreateIconFromResourceEx, GetSystemMetrics, LookupIconIdFromDirectoryEx, SendMessageW, ICON_BIG, ICON_SMALL,
        LR_DEFAULTCOLOR, SM_CXICON, SM_CXSMICON, WM_SETICON,
    };
    let hwnd = window.hwnd() as winapi::shared::windef::HWND;
    for (which, metric) in [(ICON_BIG, SM_CXICON), (ICON_SMALL, SM_CXSMICON)] {
        unsafe {
            let size = GetSystemMetrics(metric);
            let offset = LookupIconIdFromDirectoryEx(ICON.as_ptr() as *mut u8, 1, size, size, LR_DEFAULTCOLOR);
            if offset <= 0 || offset as usize >= ICON.len() {
                continue;
            }
            let data = &ICON[offset as usize..];
            let icon = CreateIconFromResourceEx(data.as_ptr() as *mut u8, data.len() as u32, 1, 0x0003_0000, size, size, LR_DEFAULTCOLOR);
            if !icon.is_null() {
                SendMessageW(hwnd, WM_SETICON, which as usize, icon as isize);
            }
        }
    }
}

/// WebView2 keeps its data (cache, remembered settings) here; the install
/// folder may be read-only (Microsoft Store installs always are).
fn data_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join(APP_NAME).join("WebView2"))
}

fn main() {
    let event_loop = EventLoopBuilder::<UserEvent>::with_user_event().build();
    let proxy = event_loop.create_proxy();
    let title = APP_NAME.to_string();
    let window = match WindowBuilder::new()
        .with_title(title)
        .with_inner_size(LogicalSize::new(1280.0, 820.0))
        .with_min_inner_size(LogicalSize::new(880.0, 600.0))
        .with_maximized(true)
        .with_visible(false) // shown once the page has drawn, so there's no white flash
        .build(&event_loop)
    {
        Ok(w) => w,
        Err(e) => {
            message_box(&format!("Couldn't open the window: {e}"));
            return;
        }
    };
    set_window_icon(&window);

    let mut context = WebContext::new(data_dir());
    let (p_ipc, p_drop) = (proxy.clone(), proxy.clone());
    let webview = WebViewBuilder::new_with_web_context(&mut context)
        .with_custom_protocol("pubconv".into(), |_, req| {
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
        .with_drag_drop_handler(move |e| {
            let ev = match e {
                DragDropEvent::Enter { .. } => UserEvent::Drag(true),
                DragDropEvent::Leave => UserEvent::Drag(false),
                DragDropEvent::Drop { paths, .. } => UserEvent::Drop(paths),
                _ => return true,
            };
            let _ = p_drop.send_event(ev);
            true
        })
        .build(&window);
    let webview = match webview {
        Ok(w) => w,
        Err(e) => {
            message_box(&format!(
                "PubConv needs Microsoft Edge WebView2, which is part of Windows 10 and 11 but seems to be missing or damaged on this PC.\n\n\
                 Install it free from https://go.microsoft.com/fwlink/p/?LinkId=2124703 and start PubConv again.\n\n({e})"
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

    let mut app = App { window, webview, proxy, state: State { recurse: true, ..State::default() } };
    event_loop.run(move |event, _, flow| {
        *flow = ControlFlow::Wait;
        let _ = &context; // the WebView2 environment lives as long as the window
        match event {
            WinEvent::UserEvent(UserEvent::Ipc(body)) => app.on_ipc(&body),
            WinEvent::UserEvent(UserEvent::Drop(paths)) => {
                app.emit(json!({ "type": "drag", "on": false }));
                app.add_paths(paths);
            }
            WinEvent::UserEvent(UserEvent::Drag(on)) => app.emit(json!({ "type": "drag", "on": on })),
            WinEvent::UserEvent(UserEvent::ToPage(e)) => {
                if e["type"] == "finished" {
                    app.state.cancel = None;
                }
                app.emit(e);
            }
            WinEvent::UserEvent(UserEvent::Show) => app.show(),
            WinEvent::WindowEvent { event: WindowEvent::CloseRequested, .. } => {
                if let Some(c) = &app.state.cancel {
                    c.store(true, Ordering::Relaxed);
                }
                *flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}
