//! Embeds the app files (PUBCONV_PAYLOAD, set by packaging/build_release.py) and the
//! exe's icon and manifest. Built without a payload, this crate is the uninstaller.
use std::path::PathBuf;

fn main() {
    println!("cargo:rerun-if-env-changed=PUBCONV_PAYLOAD");
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("payload.bin");
    match std::env::var("PUBCONV_PAYLOAD") {
        Ok(p) => {
            println!("cargo:rerun-if-changed={p}");
            std::fs::copy(&p, &out).unwrap();
        }
        Err(_) => std::fs::write(&out, b"").unwrap(),
    }
    // without asInvoker, Windows forces an admin prompt on any exe named "setup" or "install"
    winresource::WindowsResource::new()
        .set_icon("../pubconv-app/assets/pubconv.ico")
        .set_manifest(
            r#"<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3"><security><requestedPrivileges>
    <requestedExecutionLevel level="asInvoker" uiAccess="false"/>
  </requestedPrivileges></security></trustInfo>
  <application xmlns="urn:schemas-microsoft-com:asm.v3"><windowsSettings>
    <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
  </windowsSettings></application>
</assembly>"#,
        )
        .compile()
        .unwrap();
}
