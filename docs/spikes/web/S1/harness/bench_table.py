"""Summarizes bench/*.json into markdown tables (A27).

usage: python3 -I bench_table.py <bench_dir> [chromium json ...]
"""
import json
import os
import sys

d = sys.argv[1]
rows = {}
for f in sorted(os.listdir(d)):
    if not f.endswith(".json"):
        continue
    route, scn = f[:-5].split("__")
    try:
        rows[(route, scn)] = json.load(open(os.path.join(d, f)))
    except Exception as e:  # noqa: BLE001
        rows[(route, scn)] = {"error": str(e)}
for extra in sys.argv[2:]:
    j = json.load(open(extra))
    tag = os.path.basename(extra)[:-5]
    for scn, r in j.get("results", {}).items():
        if "bench" in r:
            rows[(tag, scn)] = r["bench"]

routes = sorted({r for r, _ in rows})
scns = sorted({s for _, s in rows})
print("| route | scenario | ticks | balls mean/max | mean us | p50 us | p99 us | p99.9 us | max us | 16-tick batch p50 / p99 / max us |")
print("|---|---|---|---|---|---|---|---|---|---|")
for r in routes:
    for s in scns:
        j = rows.get((r, s))
        if j is None:
            continue
        if "lua_per_tick_steady" in j:
            t = j["lua_per_tick_steady"]
            print(f"| {r} | {s} | {t['n']} (steady) | | {t['mean_us']:.2f} | {t['p50_us']:.2f} | {t['p99_us']:.2f} | {t['p999_us']:.2f} | {t['max_us']:.1f} | |")
            continue
        if "per_tick_steady" not in j:
            print(f"| {r} | {s} | error | | | | | | | |")
            continue
        t, b = j["per_tick_steady"], j["batch_steady"]
        print(
            f"| {r} | {s} | {j['ticks']} | {j['balls_mean']:.2f}/{j['balls_max']} | {t['mean_us']:.2f} | {t['p50_us']:.2f} | "
            f"{t['p99_us']:.2f} | {t['p999_us']:.2f} | {t['max_us']:.1f} | {b['p50_us']:.1f} / {b['p99_us']:.1f} / {b['max_us']:.1f} |"
        )
print()
print("Whole-run mean (one clock read per 16 ticks, all ticks incl. warm-up), us/tick:")
print()
print("| route | " + " | ".join(scns) + " |")
print("|---|" + "---|" * len(scns))
for r in routes:
    cells = []
    for s in scns:
        j = rows.get((r, s))
        cells.append(f"{j['mean_tick_us_total']:.2f}" if j and "mean_tick_us_total" in j else "")
    print(f"| {r} | " + " | ".join(cells) + " |")
