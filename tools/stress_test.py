"""End-to-end stress test of the real converter (pubconv-cli + the shipped,
trimmed page engine).

  python tools/stress_test.py

A  accuracy: all sample files vs Publisher's own PDFs (page counts, text)
B  volume: 1,000+ files in deep folders with awkward names, one output folder
C  damaged and fake files: never hangs or crashes; each file gets a result
D  locked output: a PDF held open by another program gives a clear message
E  read-only originals still get the full PubConv fixes
"""
import ctypes
import os
import random
import re
import shutil
import subprocess
import sys
import threading
import time
from pathlib import Path

import fitz
import psutil

ROOT = Path(__file__).resolve().parent.parent
CLI = ROOT / "target" / "release" / "pubconv-cli.exe"
ENGINE = ROOT / ".build" / "stage" / "runtime" / "program" / "soffice.com"
WORK = ROOT / ".build" / "stress"
TD = ROOT / "testdata"
results = []


def check(name, ok, detail=""):
    results.append((name, ok, detail))
    print(f"[{'PASS' if ok else 'FAIL'}] {name}" + (f": {detail}" if detail else ""), flush=True)


def run_cli(inputs, out=None, recurse=True, timeout=3600):
    """Run pubconv-cli; returns (exit code, stdout lines, seconds, peak MB of the whole process tree)."""
    cmd = [str(CLI), *map(str, inputs), "--soffice", str(ENGINE)]
    if recurse:
        cmd.append("-r")
    if out:
        cmd += ["-o", str(out)]
    start = time.time()
    p = subprocess.Popen(cmd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True, encoding="utf-8", errors="replace")
    peak = [0.0]

    def watch():
        try:
            proc = psutil.Process(p.pid)
            while p.poll() is None:
                try:
                    rss = proc.memory_info().rss + sum(c.memory_info().rss for c in proc.children(recursive=True))
                    peak[0] = max(peak[0], rss / 1e6)
                except psutil.Error:
                    pass
                time.sleep(0.2)
        except psutil.Error:
            pass

    t = threading.Thread(target=watch, daemon=True)
    t.start()
    try:
        stdout, _ = p.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        p.kill()
        stdout, _ = p.communicate()
        return None, stdout.splitlines(), time.time() - start, peak[0]
    t.join(1)
    return p.returncode, stdout.splitlines(), time.time() - start, peak[0]


def words(doc):
    return re.findall(r"\w+", " ".join(pg.get_text() for pg in doc).lower())


# ---------------------------------------------------------------- A accuracy
def test_accuracy():
    out = WORK / "A"
    srcs = [TD / d for d in ("corpus", "wordart", "linked")]
    total = sum(len(list(s.glob("*.pub"))) for s in srcs)
    code, lines, secs, peak = run_cli(srcs, out)
    oks = [l for l in lines if l.startswith("OK")]
    check(f"A1 all {total} sample files convert", code == 0 and len(oks) == total, f"{len(oks)} OK, exit {code}, {secs:.0f}s, peak {peak:.0f} MB")
    bad_pages, low_text = [], []
    for sub in ("corpus", "wordart", "linked"):
        for ref in (TD / "reference" / sub).glob("*.ref.pdf"):
            name = ref.name[: -len(".ref.pdf")]
            pdf = out / f"{name}.pdf"
            if not pdf.exists():
                bad_pages.append(f"{name}: missing")
                continue
            r, o = fitz.open(ref), fitz.open(pdf)
            # Publisher prints business cards 10-up on a sheet; everything else must match page for page
            if len(r) != len(o) and name != "f19_business_card":
                bad_pages.append(f"{name}: {len(r)} vs {len(o)} pages")
            rw, ow = words(r), set(words(o))
            if len(rw) >= 20:
                recall = sum(w in ow for w in rw) / len(rw)
                if recall < 0.9:
                    low_text.append(f"{name} {recall:.0%}")
    check("A2 page counts match Publisher", not bad_pages, "; ".join(bad_pages[:5]))
    check("A3 text matches Publisher (>=90% of words, every text file)", not low_text, "; ".join(low_text[:5]))
    # what PubConv draws itself (WordArt, flowed newsletter text) must stay searchable
    probes = [("r04_certificate", "Certificate of Achievement"), ("r01_flyer", "SUMMER FAIR"),
              ("r08_poster_dense", "LIVE MUSIC"), ("r02_newsletter", "Garden project grows"),
              ("f13_linked_text", "friendly place"), ("link3_mid", "w200")]
    missing = [f"{n}: '{t}'" for n, t in probes if not any(pg.search_for(t) for pg in fitz.open(out / f"{n}.pdf"))]
    check("A4 WordArt and newsletter text is searchable", not missing, "; ".join(missing))


# ---------------------------------------------------------------- B volume
AWKWARD = [
    "Newsletter Spring 2019", "نشرة الأخبار الشهرية", "Café menu – été", "Tom's & Jerry (final) [v2]",
    "100% Fun! #1", "über straße", "x" * 150, "Ünïcödé ñame", "  leading spaces", "dots.in.name",
]


def test_volume():
    src = WORK / "B_in"
    out = WORK / "B_out"
    samples = sorted((TD / "corpus").glob("*.pub")) + sorted((TD / "linked").glob("*.pub"))
    rng = random.Random(7)
    n = 0
    for i in range(130):
        depth = 1 + i % 5
        parts = [f"Year {2000 + (i + k) % 25}" if k % 2 == 0 else AWKWARD[(i + k) % len(AWKWARD)][:40].strip() for k in range(depth)]
        folder = src.joinpath(f"Batch {i:03}", *parts)  # unique per i, so every one of the 1,040 files survives
        folder.mkdir(parents=True, exist_ok=True)
        for j in range(8):
            s = rng.choice(samples)
            # awkward names repeat across folders (tests numbering when saved to one folder);
            # within a folder every file is unique
            stem = f"{AWKWARD[(i * 8 + j) % len(AWKWARD)].strip()} {j}" if j % 3 == 0 else f"{s.stem} {j}"
            shutil.copy(s, folder / f"{stem}.pub")
            n += 1
    total = len(list(src.rglob("*.pub")))
    code, lines, secs, peak = run_cli([src], out)
    oks = [l for l in lines if l.startswith("OK")]
    pdfs = list(out.glob("*.pdf"))
    check("B1 1,000+ files in deep, oddly named folders all convert", code == 0 and len(oks) == total,
          f"{len(oks)}/{total} OK in {secs:.0f}s ({total / secs:.1f} files/s), peak memory {peak:.0f} MB")
    check("B2 same-named files never overwrite each other", len(pdfs) == total, f"{len(pdfs)} PDFs for {total} files")
    numbered = [p.name for p in pdfs if re.search(r" \(\d+\)\.pdf$", p.name)]
    check("B3 duplicates get numbered names", len(numbered) > 0, f"e.g. {numbered[:2]}")
    broken = [p.name for p in rng.sample(pdfs, min(60, len(pdfs))) if len(fitz.open(p)) == 0]
    check("B4 random sample of 60 output PDFs open and have pages", not broken, ", ".join(broken[:3]))
    check("B5 memory stays reasonable", peak < 1500, f"peak {peak:.0f} MB")


# ---------------------------------------------------------------- C damaged
def test_damaged():
    src = WORK / "C_in"
    src.mkdir(parents=True, exist_ok=True)
    rng = random.Random(11)
    samples = sorted((TD / "corpus").glob("*.pub"))
    for i in range(120):
        d = bytearray(rng.choice(samples).read_bytes())
        kind = i % 4
        if kind == 0:
            for _ in range(rng.randint(1, 30)):
                d[rng.randrange(len(d))] ^= rng.randint(1, 255)
        elif kind == 1:
            s = rng.randrange(len(d))
            d[s:s + 2048] = os.urandom(min(2048, len(d) - s))
        elif kind == 2:
            del d[rng.randrange(1, len(d)):]
        else:
            i4 = rng.randrange(len(d) - 4)
            d[i4:i4 + 4] = rng.choice([b"\xff\xff\xff\xff", b"\x00\x00\x00\x00", b"\xff\xff\xff\x7f"])
        (src / f"damaged_{i:03}.pub").write_bytes(bytes(d))
    (src / "empty.pub").write_bytes(b"")
    (src / "random_noise.pub").write_bytes(os.urandom(200_000))
    (src / "actually_text.pub").write_text("This is not a Publisher file.\n" * 100)
    shutil.copy(TD / "reference" / "corpus" / "r01_flyer.ref.pdf", src / "actually_a_pdf.pub")
    total = len(list(src.glob("*.pub")))
    code, lines, secs, peak = run_cli([src], WORK / "C_out", timeout=3600)
    reported = [l for l in lines if l.startswith(("OK", "FAIL"))]
    check("C1 damaged/fake files: run finishes (no hang)", code is not None, f"{secs:.0f}s")
    check("C2 no crash", code in (0, 1) and not any("panicked" in l for l in lines), f"exit {code}")
    check("C3 every file gets a clear result", len(reported) == total,
          f"{len(reported)}/{total} reported; {sum(l.startswith('OK') for l in reported)} still converted")
    fails = [l for l in lines if l.startswith("FAIL")]
    plain = all("couldn't be read" in l or "took too long" in l for l in fails)
    check("C4 failure messages are plain English", plain, fails[0][:120] if fails else "no failures")


# ---------------------------------------------------------------- D locked output
def test_locked():
    out = WORK / "D_out"
    out.mkdir(parents=True, exist_ok=True)
    target = out / "r04_certificate.pdf"
    target.write_bytes(b"%PDF-1.4 placeholder")
    k32 = ctypes.windll.kernel32
    k32.CreateFileW.restype = ctypes.c_void_p
    handle = k32.CreateFileW(str(target), 0xC0000000, 0, None, 3, 0x80, None)  # no sharing: like an open PDF viewer
    try:
        code, lines, secs, _ = run_cli([TD / "corpus" / "r04_certificate.pub", TD / "corpus" / "r01_flyer.pub"], out, recurse=False)
    finally:
        k32.CloseHandle(ctypes.c_void_p(handle))
    locked = [l for l in lines if "r04_certificate" in l]
    other = [l for l in lines if "r01_flyer" in l]
    check("D1 locked PDF gives a clear message", bool(locked) and locked[0].startswith("FAIL") and "open in another program" in locked[0], locked[0][:140] if locked else "no line")
    check("D2 other files still convert", bool(other) and other[0].startswith("OK"))


# ---------------------------------------------------------------- E read-only originals
def test_readonly():
    src = WORK / "E_in"
    src.mkdir(parents=True, exist_ok=True)
    for name in ("r04_certificate", "r02_newsletter"):
        p = src / f"{name}.pub"
        shutil.copy(TD / "corpus" / f"{name}.pub", p)
        os.chmod(p, 0o444)
    code, lines, _, _ = run_cli([src], WORK / "E_out")
    fixed = [l for l in lines if l.startswith("OK") and "Fixed" in l]
    check("E1 read-only originals still get WordArt/newsletter fixes", len(fixed) == 2, " | ".join(l[-40:] for l in lines if l.startswith(("OK", "FAIL"))))
    for p in src.glob("*.pub"):
        os.chmod(p, 0o666)


def main():
    if not CLI.exists() or not ENGINE.exists():
        sys.exit("build first: cargo build --release, and stage the engine (packaging/build_release.py)")
    if WORK.exists():
        for p in WORK.rglob("*"):
            if p.is_file():
                os.chmod(p, 0o666)
        shutil.rmtree(WORK)
    WORK.mkdir(parents=True)
    for t in (test_accuracy, test_volume, test_damaged, test_locked, test_readonly):
        t()
    failed = [r for r in results if not r[1]]
    print(f"\n{len(results) - len(failed)}/{len(results)} checks passed")
    sys.exit(1 if failed else 0)


if __name__ == "__main__":
    main()
