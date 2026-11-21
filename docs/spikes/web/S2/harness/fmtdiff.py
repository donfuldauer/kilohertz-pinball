# Per-conversion mismatch counts between two format dumps (floats.rs FORMAT_SCRIPT).
import sys
from collections import Counter
a = open(sys.argv[1]).read().split('\n'); b = open(sys.argv[2]).read().split('\n')
fmts = ['tostring','%.14g','%.17g','%a','%.3a','%.0a','%A','%e','%.3e','%.0e','%f','%.3f','%.0f','%g','%.3g','%.0g','%#g','%10.4f','%-24.10e','%+.5g','% .2e','%.20g','%.40e','%.60f']
c = Counter(); ex = {}; lines = 0; fields = 0
if len(a) != len(b): print('LINE COUNT DIFFERS', len(a), len(b))
for x, y in zip(a, b):
    xa = x.split(' '); ya = y.split(' ')
    fields += len(xa)
    if x == y: continue
    lines += 1
    if len(xa) != len(ya):
        c['field-count'] += 1; ex.setdefault('field-count', (x[:200], y[:200])); continue
    n = len(xa)
    for i, (p, q) in enumerate(zip(xa, ya)):
        if p != q:
            name = fmts[i] if i < len(fmts) else ('roundtrip_tostring' if i == n - 2 else 'roundtrip_17g' if i == n - 1 else '%q')
            c[name] += 1; ex.setdefault(name, (p, q))
print(f'values={len(a)} fields={fields} differing_values={lines}')
for k, v in sorted(c.items()): print(f'  {k}: {v} differ, e.g. native={ex[k][0]!r} wasm={ex[k][1]!r}')
