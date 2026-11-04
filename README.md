# PubConv

Converts Microsoft Publisher (`.pub`) files to PDF, offline, and gets right what other converters get wrong: **WordArt** and **newsletters whose text flows between linked text boxes**.

- **Windows app** (`pubconv.exe`): a modern full-window interface (HTML on the system WebView2, light and dark). Add files or folders or drag them in, choose where PDFs go, convert. Shows a result per file.
- **Command line** (`pubconv-cli.exe`), for bulk and scripted jobs.
- **Self-contained:** a trimmed LibreOffice page engine ships inside, so users install nothing else.
- **Free and open source** (MPL-2.0). Nothing is uploaded; see [PRIVACY.md](PRIVACY.md).

## Download
- **Microsoft Store** (coming soon: signed by Microsoft, installs without warnings, updates itself).
- **Installer:** `PubConv-Setup` from [Releases](https://github.com/rakawe86/pubconv/releases). Needs no admin rights; remove it any time from Windows Settings → Apps. Until code signing is in place, Windows may show "Windows protected your PC": click **More info → Run anyway**.
- **Portable zip:** also from Releases, for USB sticks and locked-down PCs. Unzip and run `pubconv.exe`.

## Support PubConv
PubConv is free. If it saved you time, you can say thanks with a donation: **[Donate with PayPal](https://www.paypal.com/ncp/payment/X45F9F8S58KL8)**. Donations are voluntary and unlock nothing; every feature is free for everyone.

Found a file that converts badly? [Open an issue](https://github.com/rakawe86/pubconv/issues) and attach it if you can.

## How it works
```
.pub ─► pubcore reads WordArt, text stories, styles, linked chains, page layout
     └► patched copy (WordArt outlines + duplicated linked text removed) ─► page engine ─► base PDF
pubcore: shapes WordArt (warped glyph outlines) + flows linked text Publisher's way ─► drawn onto the base PDF
```
Anything PubConv can't place confidently is left to the page engine, so output is never worse than LibreOffice alone. Text PubConv draws itself also gets an invisible text layer, so PDFs stay searchable and copyable.

Robustness:
- A damaged file falls back to plain conversion instead of stopping the batch.
- A hung page engine is stopped after a time limit.
- Same-named files saved to one folder get "(2)", "(3)".
- Read-only originals still get the full fixes.

## Layout
| Path | What |
|---|---|
| `crates/pubcore` | .pub reader, Publisher-style text flow, WordArt renderer, PDF overlay |
| `crates/pubconv` | conversion engine (`lib.rs`) and command line (`bin/pubconv-cli.rs`) |
| `crates/pubconv-app` | Windows app: window and WebView2 host (`src/main.rs`), interface (`ui/index.html`, open it in a browser for a design preview: `#list`, `#running`, `#done`) |
| `crates/pubconv-setup` | installer and uninstaller, styled like the app (`ui/index.html`, open it in a browser for a preview) |
| `packaging/` | release script: app folders, installer, portable zip, Microsoft Store (MSIX) package |
| `testdata/` | 81 test .pub files, Publisher's own PDFs (`reference/`), expected results (`fixtures/`) |
| `tools/` | stress test, comparison sheets, Publisher comparison scripts |
| `store/` | Microsoft Store listing texts and screenshots |

## Build and test
Needs Rust with the MSVC toolchain (and Visual Studio's C++ build tools), Python 3 with `pillow pymupdf psutil`, and LibreOffice (on the build PC only).
```bash
cargo test --release
```
```bash
cargo build --release
```
```bash
python tools/stress_test.py
```
```bash
python packaging/build_release.py
```
Build output goes to `target/`, `.build/` and `dist/`, which Git ignores.

## Test coverage
- **Unit and integration (17 tests):**
  - every WordArt property of 40 shapes;
  - box positions to 0.01 pt;
  - linked-text breaks identical to Publisher (17/17), and every line identical (38/38, 33/33);
  - render planning, patching and page mapping.
- **Fuzzing:** 4,920 damaged files per run by default; 24,600 with `FUZZ_PER_FILE=300`. Zero crashes allowed.
- **End to end (`tools/stress_test.py`):**
  - accuracy against Publisher's PDFs;
  - searchable output;
  - 1,040 files in deep folders with awkward names;
  - same-named files saved into one folder;
  - damaged and fake files;
  - PDFs locked by another program;
  - read-only originals.
- **Hard documents (`tools/hard_compare.py`):** 25 complex Publisher documents in `testdata/hard` (8–60 page newsletters with jumping stories, all 40 WordArt shapes, 11 languages, columns, booklets, tri-folds), scored against Publisher's own PDFs and against plain LibreOffice. Latest: flow 98.4%, text recall 99.4%, duplicated text 0.1% (LibreOffice: 64%, 87%, 37%); 162 of 308 linked boxes start on exactly Publisher's character. Remaining differences are mostly hyphenation: Publisher uses Microsoft's own dictionary, which no public pattern set reproduces. `tools/tune_flow.py` compares each box's starting character with Publisher's.

## Known limits
These are the rare cases that still differ from Publisher:
- **WordArt fills:** a fill picture that is a transparent PNG, BMP or EMF is drawn in dark grey. Pattern fills use their main colour. 3D lighting is approximate, so some metallic presets look brighter than in Publisher. Text around a full circle is approximate.
- **Text wrap:** tight wrap follows common shapes, freeforms and edited wrap outlines. Less common preset shapes (hearts, clouds, callouts) wrap around their box.
- **Linked text:** fonts, sizes, bold, italic, colours, alignment and spacing are kept. Underlines, indents, bullets and tabs in linked text are not drawn, and theme colours print black.
- **Groups:** a flipped group isn't mirrored, and a filled text box or picture in a turned group is left to the page engine, which doesn't turn it.

## Licence
MPL-2.0. Bundled LibreOffice components are MPL-2.0; other dependencies are MIT/Apache. See `packaging/THIRD-PARTY-NOTICES.txt`.
