"""Compare PubConv's text flow with Publisher's, box by box, on the hard corpus.

  python tools/tune_flow.py                    # current defaults
  python tools/tune_flow.py 29.04 14 [files]   # try other continued-notice spaces

For every linked story, Publisher's own start of each box (testdata/reference/hard/hard_meta.json)
is compared with where PubConv's flow starts it. Prints the share of boxes that start
on exactly the same character and the mean error in characters.
"""
import json
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TOOL = ROOT / "target" / "release" / "examples" / "flow_starts.exe"
META = json.loads((ROOT / "testdata/reference/hard/hard_meta.json").read_text(encoding="utf-8"))


def ours(pub, params):
    out = subprocess.run([str(TOOL), str(pub), *params], capture_output=True, text=True, encoding="utf-8").stdout
    return [json.loads(l) for l in out.splitlines() if l.startswith("{")]


def score(names, params, verbose=False):
    exact = total = 0
    err = 0.0
    for name in names:
        mine = ours(ROOT / "testdata/hard" / f"{name}.pub", params)
        for st in META[name]["stories"]:
            if len(st["boxes"]) < 2:
                continue
            ref = [b["start"] for b in st["boxes"]]
            match = [m for m in mine if abs(m["len"] - st["text_len"]) <= 1 and len(m["starts"]) == len(ref)]
            if not match:
                if verbose:
                    print(f"  {name}/{st['name']}: no matching story")
                continue
            got = sorted(s if s is not None else st["text_len"] for s in match[0]["starts"])
            for r, g in zip(ref[1:], got[1:]):  # box 1 always starts at 0
                total += 1
                exact += r == g
                err += abs(r - g)
            if verbose:
                bad = [(i + 1, r, g) for i, (r, g) in enumerate(zip(ref, got)) if r != g]
                print(f"  {name}/{st['name']}: {len(ref)} boxes, {len(bad)} differ {bad[:4]}")
    return exact, total, err / max(total, 1)


def main():
    args = sys.argv[1:]
    params = args[:2] if len(args) >= 2 and args[0].replace(".", "").isdigit() else []
    names = [a for a in args if not a.replace(".", "").isdigit()] or sorted(META)
    exact, total, err = score(names, params, verbose=True)
    print(f"\n{exact}/{total} boxes start exactly where Publisher starts them; mean error {err:.1f} chars")


if __name__ == "__main__":
    main()
