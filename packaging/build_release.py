"""Build every PubConv deliverable:

  dist/PubConv-1.0.0-windows.zip         the app, portable (GitHub release)
  dist/PubConv-1.0.0.msix                Microsoft Store package (Microsoft signs it)

Each app folder holds pubconv.exe (window), pubconv-cli.exe, a trimmed page
engine in runtime/, and the licence files. Settings: packaging/release.json.
Run:  python packaging/build_release.py
"""
import json
import os
import shutil
import subprocess
import sys
from pathlib import Path

from PIL import Image

ROOT = Path(__file__).resolve().parent.parent
PKG = ROOT / "packaging"
DIST = ROOT / "dist"
TARGET = ROOT / "target"
STAGE = ROOT / ".build" / "stage"
CARGO = Path.home() / ".cargo" / "bin" / "cargo.exe"
def find_sdk_bin() -> Path | None:
    """MakeAppx/MakePri: the nuget Build Tools unpacked into .build/sdk,
    else an installed Windows SDK (as on GitHub's Windows runners), newest first."""
    local = sorted((ROOT / ".build" / "sdk" / "bin").glob("*/x64"), reverse=True)
    kits = Path(os.environ.get("ProgramFiles(x86)", r"C:\Program Files (x86)")) / "Windows Kits" / "10" / "bin"
    installed = sorted((p.parent for p in kits.glob("10.*/x64/makeappx.exe")), reverse=True)
    for d in local + installed:
        if (d / "makeappx.exe").exists() and (d / "makepri.exe").exists():
            return d
    return None


SDK_BIN = find_sdk_bin()

# page-engine parts not needed for silent conversion (verified: 82/82 sample
# files give identical text and page counts with and without them)
ENGINE_SKIP_DIRS = [
    "help", "share/extensions", "share/gallery", "share/template", "share/wizards", "share/basic",
    "share/xpdfimport", "program/classes", "program/wizards", "program/logs",
]


def run(cmd, **kw):
    print(">", " ".join(str(c) for c in cmd))
    subprocess.run([str(c) for c in cmd], check=True, **kw)


def cargo_build() -> Path:
    env = dict(os.environ, CARGO_TARGET_DIR=str(TARGET))
    # the app's Windows libraries need MinGW's dlltool and as on PATH (GitHub's Windows machines have them)
    run([CARGO, "build", "--release", "-p", "pubconv", "-p", "pubconv-app"], cwd=ROOT, env=env)
    out = STAGE / "bin-full"
    out.mkdir(parents=True, exist_ok=True)
    for exe in ("pubconv.exe", "pubconv-cli.exe"):
        shutil.copy2(TARGET / "release" / exe, out / exe)
    return out


def stage_engine(lo_dir: Path) -> Path:
    dst = STAGE / "runtime"
    if (dst / "program" / "soffice.com").exists():
        return dst
    skip = {(lo_dir / d).resolve() for d in ENGINE_SKIP_DIRS}

    def ignore(folder, names):
        here = Path(folder).resolve()
        out = [n for n in names if (here / n).resolve() in skip]
        out += [n for n in names if n.startswith("python-core") or (here.name == "config" and n.startswith("images_"))]
        out += [n for n in names if n == "update-settings.ini"]  # the bundled copy never updates itself
        return out

    shutil.copytree(lo_dir, dst, ignore=ignore)
    return dst


def assemble(bins: Path, engine: Path, name: str) -> Path:
    app = STAGE / name
    if app.exists():
        shutil.rmtree(app)
    shutil.copytree(engine, app / "runtime")
    for f in bins.iterdir():
        shutil.copy2(f, app / f.name)
    for f in ("LICENSE-MPL-2.0.txt", "THIRD-PARTY-NOTICES.txt"):
        shutil.copy2(PKG / f, app / f)
    shutil.copy2(PKG / "README.txt", app / "README.txt")
    return app


def zip_dir(src: Path, zip_name: str):
    DIST.mkdir(exist_ok=True)
    base = DIST / zip_name
    out = DIST / f"{zip_name}.zip"  # not with_suffix: "1.0.0-windows" would look like an extension
    if out.exists():
        out.unlink()
    # zip the folder itself so users unzip into one "PubConv" folder
    shutil.make_archive(str(base), "zip", root_dir=src.parent, base_dir=src.name)
    print("  wrote", out, f"{out.stat().st_size / 1e6:.0f} MB")


def store_assets(dst: Path):
    icon = Image.open(ROOT / "crates/pubconv-app/assets/pubconv.png").convert("RGBA")
    dst.mkdir(parents=True, exist_ok=True)

    def square(name, size):
        icon.resize((size, size), Image.LANCZOS).save(dst / name)

    for scale in (100, 125, 150, 200, 400):
        square(f"Square44x44Logo.scale-{scale}.png", 44 * scale // 100)
        square(f"Square150x150Logo.scale-{scale}.png", 150 * scale // 100)
        square(f"StoreLogo.scale-{scale}.png", 50 * scale // 100)
        w, h = 310 * scale // 100, 150 * scale // 100
        wide = Image.new("RGBA", (w, h), (0, 0, 0, 0))
        s = int(h * 0.8)
        wide.paste(icon.resize((s, s), Image.LANCZOS), ((w - s) // 2, (h - s) // 2))
        wide.save(dst / f"Wide310x150Logo.scale-{scale}.png")
    for t in (16, 20, 24, 30, 32, 36, 40, 48, 60, 64, 72, 80, 96, 256):
        square(f"Square44x44Logo.targetsize-{t}.png", t)
        square(f"Square44x44Logo.targetsize-{t}_altform-unplated.png", t)


def build_msix(app: Path, cfg: dict):
    if SDK_BIN is None:
        print("! Windows SDK Build Tools not found; skipping MSIX")
        return
    layout = STAGE / "msix"
    if layout.exists():
        shutil.rmtree(layout)
    shutil.copytree(app, layout)
    store_assets(layout / "Assets")
    manifest = (PKG / "AppxManifest.template.xml").read_text(encoding="utf-8")
    for k, v in cfg.items():
        manifest = manifest.replace("{{" + k + "}}", str(v))
    (layout / "AppxManifest.xml").write_text(manifest, encoding="utf-8")
    # resources.pri lets Windows pick the right icon size for each scale
    pri_cfg = STAGE / "priconfig.xml"
    run([SDK_BIN / "makepri.exe", "createconfig", "/cf", pri_cfg, "/dq", "en-US", "/o"])
    run([SDK_BIN / "makepri.exe", "new", "/pr", layout, "/cf", pri_cfg, "/of", layout / "resources.pri", "/o"])
    DIST.mkdir(exist_ok=True)
    out = DIST / f"PubConv-{cfg['version'].rsplit('.', 1)[0]}.msix"
    run([SDK_BIN / "makeappx.exe", "pack", "/d", layout, "/p", out, "/o"])
    print("  wrote", out, f"{out.stat().st_size / 1e6:.0f} MB")


def main():
    cfg = json.loads((PKG / "release.json").read_text(encoding="utf-8"))
    ver = cfg["version"].rsplit(".", 1)[0]
    engine = stage_engine(Path(cfg["libreoffice_dir"]))
    full = assemble(cargo_build(), engine, "PubConv")
    if "--no-zip" not in sys.argv:
        zip_dir(full, f"PubConv-{ver}-windows")
    build_msix(full, cfg)


if __name__ == "__main__":
    main()
