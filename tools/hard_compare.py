"""Score PubConv and plain LibreOffice against Publisher's own PDFs on the hard corpus.

  python tools/hard_compare.py                 # convert everything, score, print table
  python tools/hard_compare.py --no-run        # score existing outputs only
  python tools/hard_compare.py nl_parish_8p    # only some files

Inputs : testdata/hard/*.pub, testdata/reference/hard/<name>.ref.pdf, hard_meta.json
Outputs: .build/hard/pubconv/*.pdf, .build/hard/lo/*.pdf, .build/hard/results.json,
         .build/hard/png/<name>_p<N>.png (reference | PubConv | LibreOffice, low DPI)

Scores (each against Publisher's PDF):
  pages   page count, output vs reference
  recall  share of the reference's words present in the output (counted, not just seen)
  excess  extra word occurrences not in the reference, as % of reference words
  dup     share of the reference's 6-word phrases that occur MORE often in the output
          than in the reference: LibreOffice's linked-text bug prints a story in full in
          every box of its chain, which shows up here. Phrases are read both in page order
          and in story order (box by box along each chain), so a sentence that one tool
          breaks across two boxes and the other doesn't is not counted as duplicated
  flow    per linked box: words found inside that box's rectangle vs the reference (avg)
  WA txt  WordArt headlines found as text on their page (search/copy works)
  WA vis  visual similarity inside each WordArt's area (1 = identical)
  visual  whole-page visual similarity, averaged over pages (1 = identical)
"""
import json
import re
import subprocess
import sys
import time
from collections import Counter
from math import cos, radians, sin
from pathlib import Path

import fitz
import numpy as np

ROOT = Path(__file__).resolve().parent.parent
SRC = ROOT / "testdata" / "hard"
REF = ROOT / "testdata" / "reference" / "hard"
WORK = ROOT / ".build" / "hard"
CLI = ROOT / "target" / "release" / "pubconv-cli.exe"
ENGINE = ROOT / ".build" / "stage" / "runtime" / "program" / "soffice.com"
LO = Path("C:/Program Files/LibreOffice/program/soffice.com")
TOOLS = {"pubconv": WORK / "pubconv", "lo": WORK / "lo"}
DPI = 40
FLAGS = fitz.TEXTFLAGS_WORDS | fitz.TEXT_DEHYPHENATE
fitz.TOOLS.mupdf_display_errors(False)


# ------------------------------------------------------------------ running
def run(cmd, timeout):
    t = time.time()
    try:
        p = subprocess.run(cmd, capture_output=True, text=True, encoding="utf-8", errors="replace", timeout=timeout)
        return p.returncode, (p.stdout + p.stderr).strip(), time.time() - t
    except subprocess.TimeoutExpired:
        return None, "TIMEOUT", time.time() - t


def convert(names):
    if not CLI.exists():
        print("building pubconv-cli (cargo build --release)...", flush=True)
        subprocess.run(["cargo", "build", "--release"], cwd=ROOT, check=True)
    log = {}
    for d in TOOLS.values():
        d.mkdir(parents=True, exist_ok=True)
    profile = (WORK / "lo_profile").as_uri()
    for n in names:
        src = SRC / f"{n}.pub"
        for tool, out in TOOLS.items():
            pdf = out / f"{n}.pdf"
            pdf.unlink(missing_ok=True)
            if tool == "pubconv":
                cmd = [str(CLI), str(src), "-o", str(out), "--soffice", str(ENGINE)]
            else:
                cmd = [str(LO), f"-env:UserInstallation={profile}", "--headless", "--convert-to", "pdf",
                       "--outdir", str(out), str(src)]
            code, text, secs = run(cmd, 900)
            log.setdefault(n, {})[tool] = dict(exit=code, secs=round(secs, 1), out=text[-600:], pdf=pdf.exists())
            print(f"  {tool:8} {n:24} exit={code} {secs:5.1f}s {'pdf' if pdf.exists() else 'NO PDF'}"
                  f"{'  ' + text.splitlines()[-1][:110] if text and tool == 'pubconv' else ''}", flush=True)
    return log


# ------------------------------------------------------------------ text scoring
def norm_words(s):
    return re.findall(r"\w+", s.lower())


def page_words(page, clip=None):
    ws = sorted(page.get_text("words", flags=FLAGS, clip=clip), key=lambda w: (w[5], w[6], w[7]))  # block, line, word
    out, carry = [], ""
    for i, w in enumerate(ws):
        text = carry + w[4]
        carry = ""
        # a word hyphenated at a line end continues on the next line (MuPDF only joins
        # these within one text block; Publisher often writes each line as its own)
        last_on_line = i + 1 == len(ws) or ws[i + 1][5:7] != w[5:7]
        if last_on_line and text.endswith("-") and len(text) > 1 and i + 1 < len(ws):
            carry = text[:-1]
            continue
        out += norm_words(text)
    return out + norm_words(carry)


def doc_words(doc):
    return [w for pg in doc for w in page_words(pg)]


def shingles(ws, k=6):
    return Counter(tuple(ws[i:i + k]) for i in range(len(ws) - k + 1))


def wordart_words(meta, npages):
    """Words of the WordArt headlines (master-page WordArt counts once per page).
    Publisher's PDF draws WordArt as outlines, so these never count as excess."""
    c = Counter()
    for w in meta.get("wordart", []):
        for x in norm_words(w["text"]):
            c[x] += npages if w["page"] == "M" else 1
    return c


def chain_words(doc, meta):
    """Words of the linked stories read box by box in chain order, then everything else
    on the pages, so phrases that run on from one box into the next stay together."""
    words, rects = [], {}
    for st in meta.get("stories", []):
        for b in st["boxes"]:
            p = b["page"] - 1
            if p >= len(doc):
                continue
            r = fitz.Rect(b["left"], b["top"], b["left"] + b["width"], b["top"] + b["height"])
            words += page_words(doc[p], r)
            rects.setdefault(p, []).append(r)
    for i, pg in enumerate(doc):
        ws = pg.get_text("words", flags=FLAGS)
        for w in sorted(ws, key=lambda w: (w[5], w[6], w[7])):
            if not any(fitz.Rect(w[:4]).intersects(r) for r in rects.get(i, [])):
                words += norm_words(w[4])
    return words


def text_scores(ref, out, meta):
    rw, ow = doc_words(ref), doc_words(out)
    rc, oc = Counter(rw), Counter(ow)
    total = sum(rc.values())
    if total < 20:  # WordArt-only documents: nothing to recall
        return None, None, None
    allowed = rc + wordart_words(meta, len(ref))
    recall = sum(min(c, oc[w]) for w, c in rc.items()) / total
    excess = sum(max(0, c - allowed[w]) for w, c in oc.items()) / total
    rs, os_ = shingles(rw), shingles(ow)
    # a phrase is duplicated only if it's over-represented in both reading orders
    rs2, os2 = shingles(chain_words(ref, meta)), shingles(chain_words(out, meta))
    dup = sum(1 for s, c in rs.items() if os_[s] > max(c, rs2[s]) and os2[s] > max(c, rs2[s])) / max(1, len(rs))
    return recall, excess, dup


def flow_score(ref, out, meta):
    """Average per-box word recall inside each linked box's rectangle."""
    vals, excess = [], []
    for st in meta.get("stories", []):
        if len(st["boxes"]) < 2:
            continue
        for b in st["boxes"]:
            p = b["page"] - 1
            if p >= len(ref):
                continue
            r = fitz.Rect(b["left"], b["top"], b["left"] + b["width"], b["top"] + b["height"])
            rc = Counter(page_words(ref[p], r))
            if sum(rc.values()) < 5:
                continue
            oc = Counter(page_words(out[p], r)) if p < len(out) else Counter()
            vals.append(sum(min(c, oc[w]) for w, c in rc.items()) / sum(rc.values()))
            excess.append(sum(max(0, c - rc[w]) for w, c in oc.items()) / sum(rc.values()))
    if not vals:
        return None, None
    return sum(vals) / len(vals), sum(excess) / len(excess)


# ------------------------------------------------------------------ visual scoring
def render(page, dpi=DPI, clip=None):
    pix = page.get_pixmap(dpi=dpi, colorspace=fitz.csGRAY, clip=clip, alpha=False)
    return np.frombuffer(pix.samples, dtype=np.uint8).reshape(pix.height, pix.width).astype(np.float32)


def blur(a):
    k = np.array([1, 2, 1], np.float32) / 4
    a = np.apply_along_axis(lambda r: np.convolve(r, k, mode="same"), 1, a)
    return np.apply_along_axis(lambda c: np.convolve(c, k, mode="same"), 0, a)


def similarity(a, b):
    h, w = min(a.shape[0], b.shape[0]), min(a.shape[1], b.shape[1])
    a, b = blur(a[:h, :w]), blur(b[:h, :w])
    ink = (255 - a).sum() + (255 - b).sum()
    if ink < 1:
        return 1.0
    return float(max(0.0, 1 - np.abs(a - b).sum() / ink))


def visual_score(ref, out):
    vals = []
    for i in range(len(ref)):
        vals.append(similarity(render(ref[i]), render(out[i])) if i < len(out) else 0.0)
    return vals


def wa_rect(w, page):
    cx, cy = w["left"] + w["width"] / 2, w["top"] + w["height"] / 2
    a = radians(w.get("rotation", 0) or 0)
    pts = [(dx * cos(a) - dy * sin(a), dx * sin(a) + dy * cos(a))
           for dx in (-w["width"] / 2, w["width"] / 2) for dy in (-w["height"] / 2, w["height"] / 2)]
    r = fitz.Rect(cx + min(p[0] for p in pts), cy + min(p[1] for p in pts),
                  cx + max(p[0] for p in pts), cy + max(p[1] for p in pts))
    return (r + (-4, -4, 4, 4)) & page.rect


def wordart_scores(ref, out, meta):
    found, vis, per = 0, [], []
    was = meta.get("wordart", [])
    for w in was:
        pages = [0] if w["page"] == "M" else [w["page"] - 1]
        p = pages[0]
        want = norm_words(w["text"])
        if p < len(out):
            have = set(page_words(out[p]))
            ok = all(x in have for x in want)
        else:
            ok = False
        found += ok
        if p < len(ref) and p < len(out):
            r = wa_rect(w, ref[p])
            if r.width > 2 and r.height > 2:
                v = similarity(render(ref[p], 72, r), render(out[p], 72, r))
                vis.append(v)
                per.append((w["text"], w["page"], round(v, 2), ok))
    return found, len(was), (sum(vis) / len(vis) if vis else None), per


# ------------------------------------------------------------------ pictures
def save_pngs(name, ref, outs, vis_by_tool, limit=6):
    from PIL import Image
    d = WORK / "png"
    d.mkdir(parents=True, exist_ok=True)
    n = len(ref)
    worst = sorted(range(n), key=lambda i: min(v[i] if i < len(v) else 0 for v in vis_by_tool.values()))
    pages = sorted(range(n) if n <= 12 else worst[:limit])
    for i in pages:
        ims = []
        for doc in [ref] + outs:
            if doc is not None and i < len(doc):
                pix = doc[i].get_pixmap(dpi=50, alpha=False)
                ims.append(Image.frombytes("RGB", (pix.width, pix.height), pix.samples))
            else:
                ims.append(Image.new("RGB", ims[0].size, (255, 200, 200)))
        W = sum(im.width for im in ims) + 10 * (len(ims) - 1)
        canvas = Image.new("RGB", (W, max(im.height for im in ims)), (128, 128, 128))
        x = 0
        for im in ims:
            canvas.paste(im, (x, 0))
            x += im.width + 10
        canvas.save(d / f"{name}_p{i + 1}.png")


# ------------------------------------------------------------------ main
def score(names, log):
    meta_all = json.loads((REF / "hard_meta.json").read_text(encoding="utf-8"))
    results = {}
    for n in names:
        ref = fitz.open(REF / f"{n}.ref.pdf")
        meta = meta_all.get(n, {})
        res = dict(ref_pages=len(ref))
        outs, vis_all = [], {}
        for tool, d in TOOLS.items():
            pdf = d / f"{n}.pdf"
            if not pdf.exists():
                res[tool] = dict(ok=False, run=log.get(n, {}).get(tool))
                outs.append(None)
                continue
            out = fitz.open(pdf)
            outs.append(out)
            rec, exc, dup = text_scores(ref, out, meta)
            flow, flow_exc = flow_score(ref, out, meta)
            wf, wn, wv, wper = wordart_scores(ref, out, meta)
            vis = visual_score(ref, out)
            vis_all[tool] = vis
            res[tool] = dict(ok=True, pages=len(out), recall=rec, excess=exc, dup=dup, flow=flow, flow_excess=flow_exc,
                             wa_found=wf, wa_total=wn, wa_visual=wv, wa_detail=wper, visual=sum(vis) / len(vis),
                             visual_pages=[round(v, 3) for v in vis], run=log.get(n, {}).get(tool))
        save_pngs(n, ref, outs, vis_all)
        results[n] = res
    return results


def fmt(r, ref_pages):
    if not r.get("ok"):
        return "NO OUTPUT".ljust(52)
    pg = f"{r['pages']}/{ref_pages}" + ("" if r["pages"] == ref_pages else "!")
    fl = "  -  " if r["flow"] is None else f"{r['flow']:.0%}".rjust(5)
    wa = "  -  " if not r["wa_total"] else f"{r['wa_found']}/{r['wa_total']}".rjust(5)
    wv = "  - " if r["wa_visual"] is None else f"{r['wa_visual']:.2f}"
    if r["recall"] is None:
        tx = "   -     -     - "
    else:
        tx = f"{r['recall']:5.0%} {r['excess']:5.0%} {r['dup']:5.0%}"
    return f"{pg:>6} {tx} {fl} {wa} {wv:>4} {r['visual']:5.2f}"


def table(results):
    head = f"{'pages':>6} {'recal':>5} {'exces':>5} {'dup':>5} {'flow':>5} {'WAtxt':>5} {'WAvi':>4} {'visua':>5}"
    print(f"\n{'file':24} | {'PubConv':^52} | {'LibreOffice':^52}")
    print(f"{'':24} | {head:52} | {head:52}")
    print("-" * 136)
    for n, r in results.items():
        print(f"{n:24} | {fmt(r['pubconv'], r['ref_pages']):52} | {fmt(r['lo'], r['ref_pages']):52}")
    # totals
    def avg(tool, k):
        v = [r[tool][k] for r in results.values() if r[tool].get("ok") and r[tool].get(k) is not None]
        return sum(v) / len(v) if v else float("nan")
    print("-" * 136)
    for tool in TOOLS:
        pm = sum(1 for r in results.values() if r[tool].get("ok") and r[tool]["pages"] == r["ref_pages"])
        print(f"{tool:11} page match {pm}/{len(results)}  recall {avg(tool, 'recall'):.1%}  excess {avg(tool, 'excess'):.1%}"
              f"  dup {avg(tool, 'dup'):.1%}  flow {avg(tool, 'flow'):.1%}  WA vis {avg(tool, 'wa_visual'):.2f}"
              f"  visual {avg(tool, 'visual'):.3f}")


def main():
    args = sys.argv[1:]
    norun = "--no-run" in args
    names = [a for a in args if not a.startswith("--")] or sorted(p.stem for p in SRC.glob("*.pub"))
    logf = WORK / "run_log.json"
    log = json.loads(logf.read_text(encoding="utf-8")) if logf.exists() else {}
    if not norun:
        print(f"converting {len(names)} files with PubConv and LibreOffice...", flush=True)
        log.update(convert(names))
        logf.write_text(json.dumps(log, indent=1), encoding="utf-8")
    results = score(names, log)
    old = WORK / "results.json"
    allres = json.loads(old.read_text(encoding="utf-8")) if old.exists() else {}
    allres.update(results)
    old.write_text(json.dumps(allres, indent=1, ensure_ascii=False), encoding="utf-8")
    table(results)


if __name__ == "__main__":
    main()
