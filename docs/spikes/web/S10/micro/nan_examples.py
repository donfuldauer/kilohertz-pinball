"""Print NaN-producing examples of selected ops across engines (input bits -> output bits).
usage: python3 -I nan_examples.py <micro_dir> <op>[,<op>...] <label=bin> ..."""
import struct, sys
d = sys.argv[1]; want = sys.argv[2].split(',')
ops = [l.split() for l in open(f'{d}/ops.txt') if l.strip()]
inp = open(f'{d}/inputs.bin', 'rb').read(); N = struct.unpack_from('<I', inp, 8)[0]; blob = inp[12:]
engines = [a.split('=', 1) for a in sys.argv[3:]]
data = {l: open(p, 'rb').read() for l, p in engines}
ref = open(f'{d}/ref.bin', 'rb').read()
for k, (name, kind, w) in enumerate(ops):
    if name not in want: continue
    w = int(w); base = k * N * 16
    shown = 0
    seen = set()
    for i in range(N):
        outs = [data[l][base+16*i: base+16*i+w].hex() for l, _ in engines]
        if len(set(outs)) == 1: continue
        if name.startswith('f64'):
            a = struct.unpack_from('<Q', blob, 0x20000 + 8*i)[0]; b = struct.unpack_from('<Q', blob, 0x40000 + 8*i)[0]; ins = f'a={a:016x} b={b:016x}'
            fo = lambda h: f'{int.from_bytes(bytes.fromhex(h[:16]), "little"):016x}'
        else:
            a = struct.unpack_from('<I', blob, 0x0 + 4*i)[0]; b = struct.unpack_from('<I', blob, 0x10000 + 4*i)[0]; ins = f'a={a:08x} b={b:08x}'
            fo = lambda h: f'{int.from_bytes(bytes.fromhex(h[:8]), "little"):08x}'
        key = tuple(fo(o) for o in outs)
        if key in seen: continue
        seen.add(key)
        print(f'{name} rec {i} {ins} -> ' + ' '.join(f'{l}:{fo(o)}' for (l, _), o in zip(engines, outs)))
        shown += 1
        if shown >= 6: break
