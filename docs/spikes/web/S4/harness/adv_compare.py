"""Compares adversarial runner outputs against the native wasmtime baseline.

usage: python3 -I adv_compare.py <baseline.txt> <design.txt>...
For every case line: EXACT (identical after the design name), CLASS (same
load outcome and per-call outcome classes, but fuel / queue / RNG / memory /
scores differ: a different kill point), or DIFF (classification differs).
Boundary lines (fuel boundaries) compare the last OK loop count.
"""
import re
import sys


def parse(path):
    out = {}
    for line in open(path, encoding="utf-8"):
        line = line.rstrip("\n")
        if not line or line.startswith("ERROR") or "crossOriginIsolated" in line:
            continue
        parts = line.split(" ", 2)
        if len(parts) < 3:
            continue
        design, case, rest = parts
        key = case + (" [boundary]" if rest.startswith("last_ok=") or rest.startswith("boundary") or rest.startswith("load-failed") else "")
        out[key] = rest
    return out


def classes(rest):
    segs = [s.strip() for s in rest.split(" | ") if s.strip()]
    res = []
    for s in segs:
        m = re.match(r"(\w+(?:\([^)]*\))?)=(\S+)", s)
        res.append(m.group(0) if m else s)
    return res


def diffsum(b, g):
    """The first differing field per call segment, native -> design."""
    bs, gs = b.split(" | "), g.split(" | ")
    out = []
    for i, (x, y) in enumerate(zip(bs, gs)):
        if x != y:
            fx, fy = x.split(" "), y.split(" ")
            d = [f"{a}->{c.split('=',1)[-1]}" for a, c in zip(fx, fy) if a != c]
            out.append(f"seg{i}:" + ",".join(d)[:160])
        if len(out) >= 2:
            break
    if len(bs) != len(gs):
        out.append(f"segments {len(bs)}->{len(gs)}")
    return " ; ".join(out)


base = parse(sys.argv[1])
rows = []
for path in sys.argv[2:]:
    got = parse(path)
    design = open(path, encoding="utf-8").readline().split(" ", 1)[0]
    counts = {"EXACT": 0, "CLASS": 0, "DIFF": 0, "MISSING": 0}
    for case, b in base.items():
        g = got.get(case)
        if g is None:
            v = "MISSING"
        elif g == b:
            v = "EXACT"
        elif case.endswith("[boundary]"):
            v = "DIFF"
        elif classes(g) == classes(b):
            v = "CLASS"
        else:
            v = "DIFF"
        counts[v] += 1
        if v != "EXACT":
            rows.append(f"{path.split('/')[-1]:34s} {case:42s} {v:6s} {diffsum(b, g or '')}")
    print(f"{path.split('/')[-1]:40s} EXACT={counts['EXACT']} CLASS={counts['CLASS']} DIFF={counts['DIFF']} MISSING={counts['MISSING']} of {len(base)}")
print()
print("\n".join(rows))
