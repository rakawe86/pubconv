//! Gives pubconv.exe its icon in File Explorer and the taskbar.
fn main() {
    println!("cargo:rerun-if-changed=assets/pubconv.ico");
    winresource::WindowsResource::new().set_icon("assets/pubconv.ico").compile().unwrap();
}
