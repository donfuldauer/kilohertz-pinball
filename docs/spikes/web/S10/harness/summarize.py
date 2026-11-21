"""S10: PASS/FAIL tables from runs/res/*.res (one line per job:
STATUS|section|engine|tier|item|wall_s|maxrss_kb|detail).

A FAIL that matches an entry of harness/known_diffs.txt ("section :: engine regex :: item regex :: detail regex :: cause")
is reported as ROOTED with its cause; any other FAIL is unexplained.
usage: python3 -I summarize.py <res_dir>     exit 0 iff no unexplained FAIL
"""
import collections
import glob
import os
import re
import sys

KNOWN = []
kf = os.path.join(os.path.dirname(os.path.abspath(__file__)), 'known_diffs.txt')
if os.path.exists(kf):
    for line in open(kf, encoding='utf-8'):
        if line.strip() and not line.startswith('#'):
            KNOWN.append(line.rstrip('\n').split(' :: ', 4))


def rooted(r):
    for sec, eng, item, det, cause in KNOWN:
        if r[1] == sec and re.search(eng, r[2]) and re.search(item, r[4]) and re.search(det, r[7]):
            return cause
    return None


rows = []
for p in sorted(glob.glob(os.path.join(sys.argv[1], '*.res'))):
    for line in open(p, encoding='utf-8', errors='replace'):
        f = line.rstrip('\n').split('|', 7)
        if len(f) == 8:
            if f[0] == 'FAIL' and rooted(f):
                f[0] = 'ROOTED'
            rows.append(f)

ORDER = ['v8/default', 'jsc/default', 'jsc/ipint', 'jsc/bbq', 'jsc/omg', 'jsc/nojit', 'sm160/default',
         'sm160/baseline', 'sm160/ion', 'sm153/default', 'sm153/baseline', 'sm153/ion', 'sm160a/default',
         'sm160a/baseline', 'sm160a/ion', 'sm153a/default']
cols = [c for c in ORDER if any(f'{r[2]}/{r[3]}' == c for r in rows)]
sections = sorted({r[1] for r in rows})
CELL = {'PASS': 'P', 'FAIL': '**F**', 'INFO': 'i', 'ROOTED': 'R'}

print('# S10 run_engines summary')
print()
print('Legend: P = PASS (identical to the native x86_64 reference), **F** = FAIL (unexplained), R = differs with a '
      'rooted cause (harness/known_diffs.txt, listed below), i = info only, - = not run. Engines: v8 = Node 22 '
      '(V8 12.4, control), jsc = JavaScriptCore 327409@main, sm160 = SpiderMonkey 160.0.1, sm153 = SpiderMonkey '
      '153.7.0esr, ...a = linux-aarch64 build under qemu-aarch64.')
print()
print('## Totals per section and engine/tier (PASS/total)')
print('| section | ' + ' | '.join(cols) + ' |')
print('|---|' + '---|' * len(cols))
tot = collections.Counter()
for sec in sections:
    cells = []
    for c in cols:
        rs = [r for r in rows if r[1] == sec and f'{r[2]}/{r[3]}' == c]
        if not rs:
            cells.append('-')
            continue
        n = collections.Counter(r[0] for r in rs)
        for k, v in n.items():
            tot[k] += v
        txt = f"{n['PASS']}/{n['PASS'] + n['FAIL'] + n['ROOTED']}"
        if n['ROOTED']:
            txt += f" ({n['ROOTED']} rooted)"
        if n['INFO']:
            txt += f" (+{n['INFO']} info)"
        if n['FAIL']:
            txt += ' **FAIL**'
        cells.append(txt)
    print(f'| {sec} | ' + ' | '.join(cells) + ' |')
print()
for sec in sections:
    items = []
    for r in rows:
        if r[1] == sec and r[4] not in items:
            items.append(r[4])
    print(f'## {sec}')
    print('| item | ' + ' | '.join(cols) + ' |')
    print('|---|' + '---|' * len(cols))
    for it in items:
        cells = []
        for c in cols:
            m = [r for r in rows if r[1] == sec and r[4] == it and f'{r[2]}/{r[3]}' == c]
            cells.append(CELL.get(m[0][0], m[0][0]) if m else '-')
        print(f'| {it} | ' + ' | '.join(cells) + ' |')
    print()
print('## Wall time per job (s): sum per section and engine/tier (indicative; machine shared, PAR jobs in parallel)')
print('| section | ' + ' | '.join(cols) + ' |')
print('|---|' + '---|' * len(cols))
for sec in sections:
    cells = []
    for c in cols:
        rs = [r for r in rows if r[1] == sec and f'{r[2]}/{r[3]}' == c]
        cells.append(f'{sum(float(r[5] or 0) for r in rs):.0f}' if rs else '-')
    print(f'| {sec} | ' + ' | '.join(cells) + ' |')
print()
print('## Details of every FAIL, ROOTED and INFO line')
for r in rows:
    if r[0] != 'PASS':
        cause = rooted(['FAIL'] + r[1:]) if r[0] == 'ROOTED' else None
        print(f'- {r[0]} {r[1]} {r[2]}/{r[3]} {r[4]}: {r[7][:900]}' + (f' -- ROOT CAUSE: {cause}' if cause else ''))
print()
print(f"== total: {tot['PASS']} PASS, {tot['FAIL']} unexplained FAIL, {tot['ROOTED']} rooted differences, {tot['INFO']} INFO")
sys.exit(1 if tot['FAIL'] else 0)
