"""S3: prints the markdown tables of RESULTS.md from the JSON results.

Usage: python3 -I tables.py <S dir> <section>
Sections: matrix <file>, compare <jsonl>, sizes <log>, bench <file>, limits
"""
import json
import re
import sys


def matrix(path):
    d = json.load(open(path))
    if not d.get("ok"):
        print(f"FAILED before any pipeline: `{d.get('error')}`")
        return
    r = d["report"]
    print(f"Adapter: {r['meta']['adapter'] or '(WebGPU: no name)'} ({r['meta']['backend']})\n")
    print("| Item | Result | Errors (scope / log), first line |")
    print("|---|---|---|")
    for i in r["items"]:
        errs = list(i.get("scope_errors", []))
        lg = i.get("logs", {})
        errs += [f"log: {x}" for x in lg.get("log", [])]
        errs += [f"uncaptured: {x}" for x in lg.get("uncaptured", [])]
        errs += [f"deferred: {x}" for x in lg.get("deferred_scope_errors", [])]
        cm = [m for m in (i.get("compilation_messages") or []) if m.startswith("Error")]
        errs += [f"compile: {m}" for m in cm if not errs]
        first = ""
        # keep the informative part of wgpu's multi-line messages
        if errs:
            flat = " ".join(errs[0].split())
            m = re.search(r"(In [^.]*?label = '[^']*'.*?)(Check|$)", flat)
            first = (m.group(1) if m else flat)[:260]
        print(f"| {i['item']} | {'OK' if i['ok'] else 'ERROR'} | {first.replace('|', '/')} |")


def compare(path):
    print("| Table | max abs | mean abs | px > 2 | px > 8 | px > 16 | px > 32 | % px > 8 | playfield-only max / mean / px > 8 |")
    print("|---|---|---|---|---|---|---|---|---|")
    for line in open(path):
        d = json.loads(line)
        a, p = d["all"], d["playfield_only"]
        name = d["b"].split("/")[-1]
        print(f"| {name} | {a['max_abs']} | {a['mean_abs']} | {a['px_gt_2']} | {a['px_gt_8']} | {a['px_gt_16']} | {a['px_gt_32']} | {a['pct_gt_8']} | {p['max_abs']} / {p['mean_abs']} / {p['px_gt_8']} |")


def bench(path):
    d = json.load(open(path))
    if not d.get("ok"):
        print(f"FAILED: {d.get('error')}")
        return
    r = d["report"]
    rows = r["rows"]
    sizes = []
    for x in rows:
        if x["size"] not in sizes:
            sizes.append(x["size"])
    variants = []
    for x in rows:
        if x["variant"] not in variants:
            variants.append(x["variant"])
    print(f"Backend {r['meta']['backend']}, {r['frames']} frames per cell, ms per frame (wall clock to on_submitted_work_done)\n")
    print("| Variant | " + " | ".join(sizes) + " |")
    print("|---|" + "---|" * len(sizes))
    for v in variants:
        cells = []
        for s in sizes:
            ms = next(x["ms_per_frame"] for x in rows if x["size"] == s and x["variant"] == v)
            full = next(x["ms_per_frame"] for x in rows if x["size"] == s and x["variant"].startswith("full"))
            cells.append(f"{ms:.0f} ({100*ms/full:.0f} %)")
        print(f"| {v} | " + " | ".join(cells) + " |")


def sizes(path):
    cur = None
    out = []
    for line in open(path):
        line = line.strip()
        m = re.match(r"== (\w+) opt-level=(\w+) \(cargo build (\S+?)s\)", line)
        if m:
            cur = {"variant": m.group(1), "opt": m.group(2), "secs": m.group(3), "files": []}
            out.append(cur)
        elif line.startswith("{") and cur is not None:
            cur["files"].append(json.loads(line))
        elif line.startswith("naga source-path") and cur is not None:
            cur["naga"] = line.split(":")[-1].strip()
    mib = lambda b: f"{b/1048576:.2f}"
    print("| Variant | opt | cargo raw | after wasm-bindgen | after wasm-opt -Oz | -Oz gzip -9 | -Oz brotli -11 | JS glue (raw / br) | naga paths in wasm |")
    print("|---|---|---|---|---|---|---|---|---|")
    for c in out:
        f = c["files"]
        if len(f) < 4:
            continue
        raw, bg, opt, js = f
        print(f"| {c['variant']} | {c['opt']} | {mib(raw['raw'])} | {mib(bg['raw'])} | {mib(opt['raw'])} | {mib(opt['gzip9'])} | {mib(opt['brotli11'])} | {js['raw']//1024} KiB / {js['brotli11']//1024} KiB | {c.get('naga','?')} |")
    print("\n(MiB = 2^20 bytes)")


if __name__ == "__main__":
    sec = sys.argv[2]
    if sec == "matrix":
        matrix(sys.argv[3])
    elif sec == "compare":
        compare(sys.argv[3])
    elif sec == "bench":
        bench(sys.argv[3])
    elif sec == "sizes":
        sizes(sys.argv[3])
