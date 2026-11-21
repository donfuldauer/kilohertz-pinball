"""Median over repeated bench runs (A27).

usage: python3 -I bench_median.py <bench_r1> <bench_r2> ... -- prints markdown tables:
steady per-tick mean / p50 / p99 / p99.9 / max (median of runs; max = max of runs),
16-tick batch p99 / max, whole-run mean, and wasm/native ratios.
"""
import json
import os
import statistics as st
import sys

dirs = sys.argv[1:]
data = {}
for d in dirs:
    for f in os.listdir(d):
        if f.endswith(".json"):
            route, scn = f[:-5].split("__")
            try:
                data.setdefault((route, scn), []).append(json.load(open(os.path.join(d, f))))
            except Exception:  # noqa: BLE001
                pass
routes = ["native", "native_scalar", "native_lua", "wasip1_simd", "bindgen_o3", "bindgen_o3_lto",
          "bindgen_o3_lto_wopt_O3", "bindgen_os_lto_wopt_Oz", "bindgen_nosimd"]
scns = ["apex_circuit_bot60", "sky_deck_bot60", "feature_lab_bot60", "apex_circuit_mb8", "sky_deck_mb8", "feature_lab_mb8"]


def med(vals):
    return st.median(vals) if vals else float("nan")


print(f"Median of {len(dirs)} runs; steady = ticks after the first 5000.")
print()
print("| route | scenario | balls mean/max | mean us | p50 us | p99 us | p99.9 us | max us (of runs) | 16-tick batch p99 / max us | whole-run mean us |")
print("|---|---|---|---|---|---|---|---|---|---|")
means = {}
for r in routes:
    for s in scns:
        js = data.get((r, s), [])
        if not js:
            continue
        if "lua_per_tick_steady" in js[0]:
            t = [j["lua_per_tick_steady"] for j in js]
            m = med([x["mean_us"] for x in t])
            means[(r, s)] = m
            print(f"| {r} | {s} | | {m:.2f} | {med([x['p50_us'] for x in t]):.2f} | {med([x['p99_us'] for x in t]):.2f} | "
                  f"{med([x['p999_us'] for x in t]):.2f} | {max(x['max_us'] for x in t):.0f} | | |")
            continue
        t = [j["per_tick_steady"] for j in js]
        b = [j["batch_steady"] for j in js]
        m = med([x["mean_us"] for x in t])
        means[(r, s)] = m
        print(f"| {r} | {s} | {js[0]['balls_mean']:.2f}/{js[0]['balls_max']} | {m:.2f} | {med([x['p50_us'] for x in t]):.2f} | "
              f"{med([x['p99_us'] for x in t]):.2f} | {med([x['p999_us'] for x in t]):.2f} | {max(x['max_us'] for x in t):.0f} | "
              f"{med([x['p99_us'] for x in b]):.0f} / {max(x['max_us'] for x in b):.0f} | {med([j['mean_tick_us_total'] for j in js]):.2f} |")
print()
print("Ratio of steady mean per tick to native (playback backend on both):")
print()
print("| route | " + " | ".join(scns) + " |")
print("|---|" + "---|" * len(scns))
for r in routes:
    if r == "native":
        continue
    cells = []
    for s in scns:
        a, n = means.get((r, s)), means.get(("native", s))
        cells.append(f"{a / n:.2f}" if a and n else "")
    print(f"| {r} | " + " | ".join(cells) + " |")
