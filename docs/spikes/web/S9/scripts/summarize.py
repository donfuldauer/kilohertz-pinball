"""S9: prints the RESULTS.md tables from results/*.json (run with python3 -I).
usage: summarize.py <S9 root>"""
import json
import os
import statistics
import sys

R = os.path.join(sys.argv[1], "results")


def load(n):
    p = os.path.join(R, n)
    return json.load(open(p)) if os.path.exists(p) else None


def replay(n):
    d = load(n)
    if not d:
        return
    print(f"\n### {n} (Chromium {d['chromium']}, flags {' '.join(d['flags'])}, adapter {d['gpu']['backend']} {d['gpu']['adapter'] or '(WebGPU, SwiftShader)'}, {d['gpu']['size']})\n")
    print("| scenario | rules | ticks | compared | state/ext/cmd bad | Lua stream steps / mismatches | final state_hash | frames | ticks/frame | fps | render ms (median) | GPU errors | non-bg / colours |")
    print("|---|---|---|---|---|---|---|---|---|---|---|---|---|")
    for r in d["results"]:
        st = r["status"]
        rp = st["replay"]
        ls = st.get("lua_stream")
        lss = f"{ls['steps']} / {ls['tick_mismatch'] + ls['draw_mismatch'] + ls['digest_mismatch'] + ls['queue_mismatch']}" if ls else "–"
        ge = len(r["gpuErrors"]["uncaptured"]) + len(r["gpuErrors"]["deferred_scope_errors"])
        print(f"| {r['scenario']} | {r['rules']} | {st['tick']} | {rp['compared']} | {rp['state_bad']}/{rp['ext_bad']}/{rp['cmd_bad']} | {lss} | {st['state_hash']} | {r['frames']} | {r['tpfMin']}–{r['tpfMax']} | {r['frames'] / r['wallMs'] * 1000:.1f} | {r['renderMsMedian']:.1f} | {ge} | {r['pixels']['non_background_fraction']} / {r['pixels']['distinct_colours']} |")
    print(f"\nconsole errors {len(d['consoleErrors'])}, page errors {len(d['pageErrors'])}, crashed {d['crashed']}, fails {d['fails']}")


def loadtime(n):
    d = load(n)
    if not d:
        return
    print(f"\n### {n} (median of {d['runs']}; load average during runs {min(d['loadavg']):.1f}–{max(d['loadavg']):.1f} on 4 cores)\n")
    keys = list(d["median"]["cold"].keys())
    print("| mark (ms since navigation start) | cold | warm |\n|---|---|---|")
    for k in keys:
        print(f"| {k} | {d['median']['cold'][k]} | {d['median']['warm'][k]} |")


replay("replay_webgpu.json")
replay("replay_webgl2.json")
loadtime("loadtime_webgpu.json")
loadtime("loadtime_webgl2.json")
b = load("backends.json")
if b:
    print("\n### backends.json\n")
    print("| frame | tick | state_hash | distinct colours | non-background | mean luma | render ms |\n|---|---|---|---|---|---|---|")
    for k, v in b["shots"].items():
        if "tick" in v:
            s = v["stats"]
            print(f"| {k} | {v['tick']} | {v['state_hash']} | {s['distinct_colours']} | {s['non_background_fraction']} | {s['mean_luma']} | {v['render_ms_med']:.1f} |")
        else:
            s = v["stats"]
            print(f"| {k} | – | – | {s['distinct_colours']} | {s['non_background_fraction']} | {s['mean_luma']} | – |")
    print("\n| table | WebGPU vs WebGL2: mean abs | max abs | % px > 8 | % px > 32 | 8x8-block max |\n|---|---|---|---|---|---|")
    for t, c in b["compare"].items():
        print(f"| {t} | {c['mean_abs']} | {c['max_abs']} | {c['pct_px_gt8']} | {c['pct_px_gt32']} | {c['block8_max_abs']} |")
    for k in ("explicit_webgpu", "explicit_webgl2"):
        a = b["auto"][k]
        print(f"\n{k}: gpuInfo backend={a['gpuInfo']['backend']} adapter='{a['gpuInfo']['adapter']}' format={a['gpuInfo']['format']} surface_formats={a['gpuInfo']['surface_formats']}; navigator.gpu adapter.info={a['adapter'].get('adapter')}")
    for k in ("auto_with_webgpu", "auto_without_navigator_gpu", "auto_without_adapter"):
        a = b["auto"][k]
        print(f"{k}: backend={a['gpu']['backend']} adapter='{a['gpu']['adapter']}'")
p = load("player_webgpu.json")
if p:
    print("\n### player_webgpu.json loads\n")
    print("| # | name | via | ok | error / rules | read ms | load ms | linear memory before → after (B) |\n|---|---|---|---|---|---|---|---|")
    for i, l in enumerate(p["loads"]):
        print(f"| {i} | {l['name']} | {l.get('via') or 'builtin'} | {l['ok']} | {l.get('error') or ''} | {l.get('readMs') and round(l['readMs'], 1)} | {l.get('loadMs') and round(l['loadMs'], 1)} | {l.get('memBefore')} → {l.get('memAfter')} |")
    print(f"\nfinal: {p['memory_final']}")
f = load("failures_webgpu.json")
if f:
    print("\n### failures_webgpu.json stack\n")
    print(json.dumps(f["stack"]["highwater"]), f["stack"]["caseTop"], f["stack"]["layout"])
