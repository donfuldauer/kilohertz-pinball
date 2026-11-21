# Median-of-reps table of per-callback cost. usage: bench_table.py <bench_dir> [chromium.json]
import json, os, sys, statistics as st
d = sys.argv[1]
rows = {}
for f in sorted(os.listdir(d)):
    if not f.endswith('.txt') or f.count('.') != 3: continue
    route, scn, rep, _ = f.split('.')
    txt = open(os.path.join(d, f)).read().strip()
    if not txt.startswith('TIMING '): continue
    j = json.loads(txt[7:])
    rows.setdefault((scn, route), []).append(j)
if len(sys.argv) > 2 and os.path.exists(sys.argv[2]):
    c = json.load(open(sys.argv[2]))
    for scn, j in c.get('timing', {}).items():
        rows.setdefault((scn, 'chromium'), []).append(j)
# Single-run extra routes: route (a) clang-18 and route (c) emscripten (Node), if present.
for f in sorted(os.listdir(d)):
    for pre, route in (('route_a_clang18_', 'route_a_clang18'), ('route_c_emscripten_', 'route_c_emscripten')):
        if f.startswith(pre) and f.endswith('.txt'):
            txt = open(os.path.join(d, f)).read().strip()
            if txt.startswith('TIMING '):
                rows.setdefault((f[len(pre):-4], route), []).append(json.loads(txt[7:]))
def med(js, cb, k):
    v = [j[cb][k] for j in js if j[cb]['n']]
    return st.median(v) if v else float('nan')
order = ['native', 'wasm_node', 'remote_node', 'chromium', 'route_a_clang18', 'route_c_emscripten']
print('| scenario | route | reps | on_tick n | on_tick mean us | on_tick p99 us | on_tick max us | on_switch n | on_switch mean us | on_switch p99 us | on_init us | run wall ms |')
print('|---|---|---|---|---|---|---|---|---|---|---|---|')
for scn in sorted({k[0] for k in rows}):
    for route in order:
        js = rows.get((scn, route))
        if not js: continue
        print(f"| {scn} | {route} | {len(js)} | {js[0]['on_tick']['n']} | {med(js,'on_tick','mean_us'):.2f} | {med(js,'on_tick','p99_us'):.1f} | {med(js,'on_tick','max_us'):.0f} | {js[0]['on_switch']['n']} | {med(js,'on_switch','mean_us'):.2f} | {med(js,'on_switch','p99_us'):.1f} | {med(js,'on_init','mean_us'):.0f} | {st.median([j['wall_ms'] for j in js]):.0f} |")
