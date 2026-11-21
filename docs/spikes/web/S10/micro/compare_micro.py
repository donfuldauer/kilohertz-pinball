"""Compare micro-test outputs of engines with the native reference (ref.bin).

usage: python3 -I compare_micro.py <micro_dir> <label=out.bin> [<label=out.bin> ...]

Per op and engine:
  exact     records whose output bytes equal the reference
  nanbits   records where reference and engine are both NaN in every differing lane
            but the bits differ (allowed by the Wasm spec for arithmetic ops)
  MISMATCH  any other difference (a real semantic difference)
  SPEC      NaN results violating the spec's NaN rules: an arithmetic op returned a
            NaN without the quiet bit, or a non-canonical NaN although every NaN
            input was canonical; or a bitwise op (abs/neg/copysign/moves/pmin/pmax/
            lane moves/bitselect) changed any bit.
Then a cross-engine table: per op, is the full output (incl. NaN bits) identical to
the first engine given (the baseline)?
Exit 0 iff no MISMATCH and no SPEC on any engine.
"""
import hashlib
import struct
import sys

d = sys.argv[1]
ops = [l.split() for l in open(f'{d}/ops.txt') if l.strip()]
inp = open(f'{d}/inputs.bin', 'rb').read()
N = struct.unpack_from('<I', inp, 8)[0]
mem0 = 0x10000
blob = inp[12:]
A32, B32, A64, B64 = 0x10000, 0x20000, 0x30000, 0x50000
ref = open(f'{d}/ref.bin', 'rb').read()
engines = [a.split('=', 1) for a in sys.argv[2:]]
data = {lab: open(p, 'rb').read() for lab, p in engines}

BITWISE = {'abs', 'neg', 'copysign', 'load_store', 'reinterpret_roundtrip', 'pmin', 'pmax',
           'splat_extract_replace', 'bitselect_f32'}
CMP = {'eq', 'ne', 'lt', 'gt', 'le', 'ge'}


def u32(off):
    return struct.unpack_from('<I', blob, off - mem0)[0]


def u64(off):
    return struct.unpack_from('<Q', blob, off - mem0)[0]


def isnan32(u):
    return (u & 0x7f800000) == 0x7f800000 and (u & 0x007fffff) != 0


def isnan64(u):
    return (u & 0x7ff0000000000000) == 0x7ff0000000000000 and (u & 0x000fffffffffffff) != 0


UNARY = {'abs', 'neg', 'sqrt', 'ceil', 'floor', 'trunc', 'nearest'}


def lanes_of(ty, op, kind, w):
    """(lane width in bytes, lanes hold floats?)"""
    if kind != 'v128':
        return w, kind in ('f32', 'f64')
    if ty == 'f64x2':
        return 8, op not in CMP
    if ty == 'i32x4':
        return 4, False
    if ty == 'f32x4':
        return 4, op not in CMP
    return 4, True


def nan_inputs(ty, op, i, lane):
    """[(bits, width)] of the float operands that feed this output lane."""
    if ty == 'f32':
        if op == 'demote_f64':
            return [(u64(A64 + 8 * i), 8)]
        return [(u32(A32 + 4 * i), 4)] + ([] if op in UNARY else [(u32(B32 + 4 * i), 4)])
    if ty == 'f64':
        if op == 'promote_f32':
            return [(u32(A32 + 4 * i), 4)]
        return [(u64(A64 + 8 * i), 8)] + ([] if op in UNARY else [(u64(B64 + 8 * i), 8)])
    if ty == 'f32x4':
        if op == 'demote_f64x2_zero':
            return [(u64(A64 + 8 * (i + lane)), 8)]
        return [(u32(A32 + 4 * (i + lane)), 4)] + ([] if op in UNARY else [(u32(B32 + 4 * (i + lane)), 4)])
    if ty == 'f64x2':
        if op == 'promote_low_f32x4':
            return [(u32(A32 + 4 * (i + lane)), 4)]
        return [(u64(A64 + 8 * (i + lane)), 8)] + ([] if op in UNARY else [(u64(B64 + 8 * (i + lane)), 8)])
    return []


def input_nans_canonical(ty, op, i, lane):
    for v, w in nan_inputs(ty, op, i, lane):
        if w == 4 and isnan32(v) and (v & 0x7fffffff) != 0x7fc00000:
            return False
        if w == 8 and isnan64(v) and (v & 0x7fffffffffffffff) != 0x7ff8000000000000:
            return False
    return True


summary = {}
bad = 0
for lab, _ in engines:
    e = data[lab]
    rows = []
    for k, (name, kind, w) in enumerate(ops):
        ty, op = name.split('.', 1)
        w = int(w)
        lw, isfloat = lanes_of(ty, op, kind, w)
        exact = nanbits = mism = spec = 0
        first_bad = None
        base = k * N * 16
        for i in range(N):
            r = ref[base + 16 * i: base + 16 * i + w]
            g = e[base + 16 * i: base + 16 * i + w]
            if r == g:
                exact += 1
                continue
            allnan = True
            specbad = False
            for lane in range(w // lw):
                rl = int.from_bytes(r[lane * lw:(lane + 1) * lw], 'little')
                gl = int.from_bytes(g[lane * lw:(lane + 1) * lw], 'little')
                if rl == gl:
                    continue
                isn = isnan32 if lw == 4 else isnan64
                quiet = 0x00400000 if lw == 4 else 0x0008000000000000
                canon = 0x7fc00000 if lw == 4 else 0x7ff8000000000000
                absmask = 0x7fffffff if lw == 4 else 0x7fffffffffffffff
                if isfloat and isn(rl) and isn(gl) and op not in BITWISE:
                    if not (gl & quiet):
                        specbad = True
                    if input_nans_canonical(ty, op, i, lane) and (gl & absmask) != canon:
                        specbad = True
                    continue
                allnan = False
            if allnan:
                nanbits += 1
                if specbad:
                    spec += 1
                    first_bad = first_bad or (i, r.hex(), g.hex(), 'SPEC')
            else:
                mism += 1
                first_bad = first_bad or (i, r.hex(), g.hex(), 'MISMATCH')
        rows.append((name, exact, nanbits, mism, spec, first_bad))
        if mism or spec:
            bad += 1
    summary[lab] = rows

print(f'# micro-test vs native reference: {len(ops)} ops x {N} records (lanes per record per op)')
print('| engine | ops | records exact | records NaN-bits-only | MISMATCH records | SPEC records | ops with any NaN-bit difference |')
print('|---|---|---|---|---|---|---|')
for lab, rows in summary.items():
    ex = sum(r[1] for r in rows); nb = sum(r[2] for r in rows); mm = sum(r[3] for r in rows); sp = sum(r[4] for r in rows)
    nbops = ' '.join(f'{r[0]}({r[2]})' for r in rows if r[2])
    print(f'| {lab} | {len(rows)} | {ex} | {nb} | {mm} | {sp} | {nbops or "-"} |')
for lab, rows in summary.items():
    for r in rows:
        if r[3] or r[4]:
            print(f'BAD {lab} {r[0]} mismatch={r[3]} spec={r[4]} first: record {r[5][0]} ref={r[5][1]} got={r[5][2]} ({r[5][3]})')
# cross-engine exact equality (incl. NaN bits)
labs = [l for l, _ in engines]
if len(labs) > 1:
    b0 = labs[0]
    print(f'\n# cross-engine: full output bytes (incl. NaN bits) vs {b0}')
    print('| op | ' + ' | '.join(labs[1:]) + ' |')
    print('|---|' + '---|' * (len(labs) - 1))
    diffops = 0
    for k, (name, kind, w) in enumerate(ops):
        sl = slice(k * N * 16, (k + 1) * N * 16)
        cells = []
        anyd = False
        for lab in labs[1:]:
            if data[lab][sl] == data[b0][sl]:
                cells.append('=')
            else:
                nd = sum(1 for i in range(N) if data[lab][sl][16 * i:16 * i + 16] != data[b0][sl][16 * i:16 * i + 16])
                cells.append(f'NaN bits differ in {nd} records')
                anyd = True
        if anyd:
            diffops += 1
            print(f'| {name} | ' + ' | '.join(cells) + ' |')
    print(f'ops identical across all engines (incl. NaN bits): {len(ops) - diffops}/{len(ops)}')
    for lab in labs:
        print(f'digest {lab} {hashlib.sha256(data[lab]).hexdigest()[:16]}')
print(f'== micro: {bad} (engine, op) pairs with MISMATCH or SPEC violations')
sys.exit(1 if bad else 0)
