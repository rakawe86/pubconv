"""Side-by-side sheet: Publisher's own PDF | a PubConv output PDF, first page each.

  python tools/sheet.py OUT_DIR SHEET.png name[:page] ...
"""
import os
import sys

import fitz
from PIL import Image, ImageDraw

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
REFS = [os.path.join(ROOT, "testdata", "reference", d) for d in ("corpus", "wordart", "linked")]


def page(path, n=0, dpi=50):
    doc = fitz.open(path)
    pg = doc[min(n, len(doc) - 1)]
    pix = pg.get_pixmap(dpi=dpi)
    return Image.frombytes("RGB", (pix.width, pix.height), pix.samples)


def main(out_dir, names, sheet_path):
    rows = []
    for spec in names:
        name, _, pno = spec.partition(":")
        n = int(pno or 0)
        ref = next(os.path.join(d, name + ".ref.pdf") for d in REFS if os.path.exists(os.path.join(d, name + ".ref.pdf")))
        rows.append((f"{name} p{n + 1}", page(ref, n), page(os.path.join(out_dir, name + ".pdf"), n)))
    w = max(max(a.width, b.width) for _, a, b in rows)
    h = max(max(a.height, b.height) for _, a, b in rows)
    per_row = 2
    sheet_rows = (len(rows) + per_row - 1) // per_row
    sheet = Image.new("RGB", (per_row * (2 * (w + 6) + 20), sheet_rows * (h + 24)), "white")
    d = ImageDraw.Draw(sheet)
    for i, (label, a, b) in enumerate(rows):
        x = (i % per_row) * (2 * (w + 6) + 20)
        y = (i // per_row) * (h + 24)
        d.text((x + 2, y + 4), f"{label}: Publisher | PubConv", fill="black")
        sheet.paste(a, (x, y + 20))
        sheet.paste(b, (x + w + 6, y + 20))
        d.rectangle([x + w + 6, y + 20, x + w + 6 + b.width - 1, y + 20 + b.height - 1], outline=(255, 0, 0))
    sheet.save(sheet_path)
    print("saved", sheet_path)


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[3:], sys.argv[2])
