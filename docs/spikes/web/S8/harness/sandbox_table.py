# Builds the native-vs-wasm PASS/FAIL table for the sandbox cases.
# usage: python3 -I sandbox_table.py <variant> <describe.txt> <native.txt> <node.txt> [chromium.txt]
# A case PASSes when every field except the trailing "mem=" (Lua heap bytes,
# pointer-size dependent by construction) is identical on every route.
import sys
v, desc, nat, node = sys.argv[1:5]
chro = sys.argv[5] if len(sys.argv) > 5 else None
known = {
    'mem_gradual_keep': 'known: memory-cap fault tick depends on pointer size (32-bit wasm objects are smaller)',
    'collectgarbage_count': "known in base: collectgarbage('count') reports the platform's heap bytes (removed by web-parity)",
    'math_transcendental': 'known in base: glibc vs musl libm (fixed by KHZ_LUA_MATH)',
}
def load(p):
    d = {}
    for line in open(p, encoding='utf-8'):
        line = line.rstrip('\n')
        if not line or ' | ' not in line: continue
        name = line.split(' | ')[0]
        body, _, mem = line.rpartition(' | mem=')
        d[name] = (body, mem)
    return d
D = dict(l.rstrip('\n').split('\t', 1) for l in open(desc, encoding='utf-8') if '\t' in l)
N, W = load(nat), load(node)
C = load(chro) if chro else {}
fails = 0
print(f'| variant | case | checks | native outcome | Node | Chromium | verdict |')
print('|---|---|---|---|---|---|---|')
for name, what in D.items():
    nb, nm = N.get(name, ('MISSING', ''))
    wb, wm = W.get(name, ('MISSING', ''))
    cb, cm = C.get(name, ('n/a', '')) if chro else ('n/a', '')
    out = nb.split(' | ')[1] if ' | ' in nb else nb
    out = out.replace('events=', '')[:60]
    okw = nb == wb
    okc = (cb == nb) if chro else True
    verdict = 'PASS' if okw and okc else 'FAIL'
    if verdict == 'FAIL':
        fails += 1
        if name in known: verdict += ' (' + known[name] + ')'
    print(f"| {v} | {name} | {what} | {out} | {'=' if okw else 'differs'} | {('=' if okc else 'differs') if chro else 'n/a'} | {verdict} |")
print(f'\n{v}: {len(D)} cases, {fails} FAIL')
