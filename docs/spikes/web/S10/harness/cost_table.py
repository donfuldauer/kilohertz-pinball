"""S10 req 4: cost tables from runs/cost (medians over reps), relative to V8 (Node).

usage: python3 -I cost_table.py <runs/cost>
"""
import glob
import json
import os
import re
import statistics
import sys

d = sys.argv[1]
ORDER = ['v8_default', 'jsc_default', 'jsc_omg', 'jsc_bbq', 'jsc_ipint', 'jsc_nojit', 'sm160_default', 'sm160_ion',
         'sm160_baseline', 'sm153_default']


def med(xs):
    xs = [x for x in xs if x is not None]
    return statistics.median(xs) if xs else None


def load_json_tail(path):
    s = open(path, encoding='utf-8', errors='replace').read()
    m = re.search(r'\{.*\}', s, re.S)
    if not m:
        return None
    try:
        return json.loads(m.group(0))
    except json.JSONDecodeError:
        # lua play-timed: 'TIMING {...}'
        m = re.search(r'TIMING (\{.*\})', s)
        return json.loads(m.group(1)) if m else None


def rss(path):
    try:
        return int(open(path).read().split()[1])
    except (OSError, IndexError, ValueError):
        return None


data = {}
for p in glob.glob(f'{d}/*__*__*__r*.txt'):
    b = os.path.basename(p)[:-4]
    kind, eng, scn, rep = b.split('__')
    data.setdefault((kind, eng, scn), []).append(p)

engines = [e for e in ORDER if any(k[1] == e for k in data)]


def fmt(x, nd=1):
    return '-' if x is None else f'{x:.{nd}f}'


print('## Sim per tick (S1 `bench`, steady state after 5000 ticks; µs; median of reps)')
print('| scenario | metric | ' + ' | '.join(engines) + ' |')
print('|---|---|' + '---|' * len(engines))
for scn in ['apex_circuit_bot60', 'feature_lab_bot60', 'sky_deck_mb8']:
    rows = {'mean': {}, 'p50': {}, 'p99': {}, 'batch16_p99': {}, 'peak_rss_MB': {}}
    for e in engines:
        ps = data.get(('sim', e, scn), [])
        js = [load_json_tail(p) for p in ps]
        js = [j for j in js if j and 'per_tick_steady' in j]
        rows['mean'][e] = med([j['per_tick_steady']['mean_us'] for j in js])
        rows['p50'][e] = med([j['per_tick_steady']['p50_us'] for j in js])
        rows['p99'][e] = med([j['per_tick_steady']['p99_us'] for j in js])
        rows['batch16_p99'][e] = med([j['batch_steady']['p99_us'] for j in js])
        rows['peak_rss_MB'][e] = med([rss(p[:-4] + '.rss') / 1024 if rss(p[:-4] + '.rss') else None for p in ps])
    for k, v in rows.items():
        print(f'| {scn} | {k} | ' + ' | '.join(fmt(v.get(e)) for e in engines) + ' |')
    base = rows['mean'].get('v8_default')
    print(f'| {scn} | mean / V8 | ' + ' | '.join(fmt(rows['mean'][e] / base, 2) if base and rows['mean'].get(e) else '-' for e in engines) + ' |')
print()
print('## Lua callbacks (S2 `play-timed`, real rules.lua in the v2 module; µs; median of reps)')
print('| scenario | metric | ' + ' | '.join(engines) + ' |')
print('|---|---|' + '---|' * len(engines))
for scn in ['apex_circuit_bot60', 'sky_deck_mb8']:
    rows = {'on_tick mean': {}, 'on_tick p99': {}, 'on_switch mean': {}, 'on_switch p99': {}, 'on_init': {}}
    for e in engines:
        js = [load_json_tail(p) for p in data.get(('lua', e, scn), [])]
        js = [j for j in js if j and 'on_tick' in j]
        rows['on_tick mean'][e] = med([j['on_tick']['mean_us'] for j in js])
        rows['on_tick p99'][e] = med([j['on_tick']['p99_us'] for j in js])
        rows['on_switch mean'][e] = med([j['on_switch']['mean_us'] for j in js])
        rows['on_switch p99'][e] = med([j['on_switch']['p99_us'] for j in js])
        rows['on_init'][e] = med([j['on_init']['mean_us'] for j in js])
    for k, v in rows.items():
        print(f'| {scn} | {k} | ' + ' | '.join(fmt(v.get(e)) for e in engines) + ' |')
    base = rows['on_tick mean'].get('v8_default')
    print(f'| {scn} | on_tick mean / V8 | ' + ' | '.join(fmt(rows['on_tick mean'][e] / base, 2) if base and rows['on_tick mean'].get(e) else '-' for e in engines) + ' |')
print()
print('## Wasm rules in wasmi-in-wasm (S4 `bench`, design wasmi-instr-depth; µs per callback; median of reps)')
print('| scenario | metric | ' + ' | '.join(engines) + ' |')
print('|---|---|' + '---|' * len(engines))
for scn in ['apex_circuit_bot60', 'sky_deck_bot60']:
    rows = {'tick mean': {}, 'tick p99': {}, 'switch mean': {}, 'rules load_ms': {}}
    for e in engines:
        js = [load_json_tail(p) for p in data.get(('rules', e, scn), [])]
        js = [j for j in js if j and 'tick' in j]
        rows['tick mean'][e] = med([j['tick']['mean_us'] for j in js])
        rows['tick p99'][e] = med([j['tick']['p99_us'] for j in js])
        rows['switch mean'][e] = med([j['switch']['mean_us'] for j in js])
        rows['rules load_ms'][e] = med([j['load_ms'] for j in js])
    for k, v in rows.items():
        print(f'| {scn} | {k} | ' + ' | '.join(fmt(v.get(e)) for e in engines) + ' |')
    base = rows['tick mean'].get('v8_default')
    print(f'| {scn} | tick mean / V8 | ' + ' | '.join(fmt(rows['tick mean'][e] / base, 2) if base and rows['tick mean'].get(e) else '-' for e in engines) + ' |')
print()
print('## Worst-case rules callback: hostile body run to the 100 000-fuel budget (S4 `worst`, wasmi-instr-depth; ms; median over reps of the per-case median)')
print('| case | ' + ' | '.join(engines) + ' |')
print('|---|' + '---|' * len(engines))
cases = {}
for e in engines:
    for p in glob.glob(f'{d}/worst__{e}__all__r*.txt'):
        for line in open(p):
            m = re.match(r'\S+ (\S+) min_us=[0-9.]+ median_us=([0-9.]+)', line)
            if m:
                cases.setdefault(m.group(1), {}).setdefault(e, []).append(float(m.group(2)) / 1000)
worst_row = {}
for c, per in cases.items():
    vals = {e: med(per.get(e, [])) for e in engines}
    for e, v in vals.items():
        if v is not None:
            worst_row[e] = max(worst_row.get(e, 0), v)
    print(f'| {c} | ' + ' | '.join(fmt(vals[e], 2) for e in engines) + ' |')
print('| **max over cases** | ' + ' | '.join(fmt(worst_row.get(e), 2) for e in engines) + ' |')
print()
print('## Module load: compile + instantiate (ms; median of reps)')
print('| module | ' + ' | '.join(engines) + ' |')
print('|---|' + '---|' * len(engines))
for mod in ['s1', 's8', 's4']:
    cells = []
    for e in engines:
        vals = []
        for p in glob.glob(f'{d}/load__{e}__{mod}__r*.txt'):
            m = re.search(r'load_ms=([0-9.]+)', open(p).read())
            if m:
                vals.append(float(m.group(1)))
        cells.append(fmt(med(vals)))
    name = {'s1': 'sim (S1 +simd128, 3.36 MB)', 's8': 'sim + Lua (S8 v2, 4.08 MB)', 's4': 'sim + wasmi (S4, 8.47 MB)'}[mod]
    print(f'| {name} | ' + ' | '.join(cells) + ' |')
