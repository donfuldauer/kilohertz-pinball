"""S10 wasm float micro-test generator.

Writes (into the directory given as argv[1]):
  micro.wat     one exported function per operation, op_<name>(n) loops over n input
                records and stores each result into a 16-byte output slot
  inputs.bin    the operand arrays (little endian), identical for every engine
  ops.txt       op name, kind, result width (bytes per element)
The op list covers every float/conversion operator found in the shipped modules
(S10/micro/inventory/union_ops.txt) plus the closely related ones (min/max,
nearest/trunc/ceil/floor in both widths and SIMD, int<->float lane conversions,
NaN-transparent moves). No relaxed-SIMD op (none is in any module).

Memory layout (bytes): A32 @0x10000, B32 @0x20000, A64 @0x30000, B64 @0x50000,
I32 @0x70000, I64 @0x80000, OUT @0x100000 (16 bytes per record); N <= 8192 records,
arrays padded by 3 (f32) / 1 (f64) entries so v128 loads at the last record stay
inside the array (lanes = consecutive records).
"""
import struct
import sys

OUTDIR = sys.argv[1]
N = 6144

A32, B32, A64, B64, I32, I64, OUT = 0x10000, 0x20000, 0x30000, 0x50000, 0x70000, 0x80000, 0x100000


def f2u(x):
    return struct.unpack('<I', struct.pack('<f', x))[0]


def d2u(x):
    return struct.unpack('<Q', struct.pack('<d', x))[0]


F32_SPECIAL = [
    0x00000000, 0x80000000, 0x3f800000, 0xbf800000, 0x3f000000, 0xbf000000,
    f2u(1.5), f2u(2.5), f2u(-1.5), f2u(-2.5), f2u(0.49999997), f2u(0.50000006),
    f2u(8388607.5), f2u(8388608.0), f2u(-8388607.5), f2u(16777216.0), f2u(16777218.0),
    0x7f7fffff, 0xff7fffff, 0x00800000, 0x80800000, 0x00000001, 0x80000001, 0x007fffff, 0x807fffff,
    0x7f800000, 0xff800000,
    0x7fc00000, 0xffc00000, 0x7fc00001, 0xffc00001, 0x7fe00000, 0x7f800001, 0xff800001, 0x7fbfffff,
    0x7fffffff, 0xffffffff, 0x7fa00000,
    f2u(2147483648.0), f2u(-2147483648.0), f2u(2147483520.0), f2u(-2147483904.0),
    f2u(4294967296.0), f2u(4294967040.0), f2u(9.223372036854776e18), f2u(-9.223372036854776e18),
    f2u(1.8446744073709552e19), f2u(-0.99999994), f2u(0.99999994), f2u(3.1415927), f2u(-1e-30),
    f2u(1e30), f2u(1e-38), f2u(6.0), f2u(1.0e-7),
]
F64_SPECIAL = [
    0, 0x8000000000000000, d2u(1.0), d2u(-1.0), d2u(0.5), d2u(-0.5), d2u(1.5), d2u(2.5), d2u(-1.5),
    d2u(-2.5), d2u(0.49999999999999994), d2u(4503599627370495.5), d2u(4503599627370496.0),
    d2u(-4503599627370495.5), d2u(9007199254740993.0),
    0x7fefffffffffffff, 0xffefffffffffffff, 0x0010000000000000, 0x8010000000000000, 1,
    0x8000000000000001, 0x000fffffffffffff,
    0x7ff0000000000000, 0xfff0000000000000,
    0x7ff8000000000000, 0xfff8000000000000, 0x7ff8000000000001, 0xfff8000000000001,
    0x7ffc000000000000, 0x7ff0000000000001, 0xfff0000000000001, 0x7ff7ffffffffffff, 0x7fffffffffffffff,
    0xffffffffffffffff, 0x7ff4000000000000,
    d2u(2147483647.0), d2u(2147483647.9999998), d2u(2147483648.0), d2u(-2147483648.0),
    d2u(-2147483648.9999998), d2u(-2147483649.0), d2u(4294967295.0), d2u(4294967295.5),
    d2u(4294967296.0), d2u(9.223372036854775e18), d2u(9.223372036854776e18), d2u(-9.223372036854776e18),
    d2u(1.8446744073709550e19), d2u(1.8446744073709552e19), d2u(-0.9999999999999999),
    d2u(3.4028234663852886e38), d2u(3.4028235677973366e38), d2u(1.401298464324817e-45),
    d2u(7.006492321624085e-46), d2u(1.1754942807573643e-38), d2u(1e300), d2u(1e-300), d2u(0.1),
]
I32_SPECIAL = [0, 1, -1, 2, -2, 0x7fffffff, -0x80000000, 16777216, 16777217, 16777219, -16777217, 0x7fffff80,
               0x7fffffc0, 0x7fffffbf, 0x40000001, 3, 100, -100]
I64_SPECIAL = [0, 1, -1, 0x7fffffffffffffff, -0x8000000000000000, 9007199254740993, -9007199254740993,
               0x20000020000001, 0x7ffffffffffffdff, 0x7ffffffffffffe00, 16777217, 0x100000000, -0x100000000,
               0x8000008000000001 - (1 << 64), 0x4000000000000400, 0x400000000000040 + 1, 12345678912345]


class Rng:  # splitmix64, inputs only
    def __init__(self, s):
        self.s = s & ((1 << 64) - 1)

    def next(self):
        self.s = (self.s + 0x9E3779B97F4A7C15) & ((1 << 64) - 1)
        z = self.s
        z = ((z ^ (z >> 30)) * 0xBF58476D1CE4E5B9) & ((1 << 64) - 1)
        z = ((z ^ (z >> 27)) * 0x94D049BB133111EB) & ((1 << 64) - 1)
        return z ^ (z >> 31)


def build_inputs():
    r = Rng(0x4B485A10)
    ks, kd = len(F32_SPECIAL), len(F64_SPECIAL)
    a32, b32, a64, b64, i32, i64 = [], [], [], [], [], []
    for i in range(N):
        # first block: all special x special pairs; then random patterns of three kinds
        if i < ks * ks:
            a32.append(F32_SPECIAL[i // ks]); b32.append(F32_SPECIAL[i % ks])
        else:
            k = i % 3
            if k == 0:   # raw bit patterns (incl. NaNs/subnormals)
                a32.append(r.next() & 0xffffffff); b32.append(r.next() & 0xffffffff)
            elif k == 1:  # "physics" magnitudes
                a32.append(f2u(((r.next() >> 11) / 2.0 ** 53 - 0.5) * 2.0 ** ((r.next() % 40) - 20)))
                b32.append(f2u(((r.next() >> 11) / 2.0 ** 53 - 0.5) * 2.0 ** ((r.next() % 40) - 20)))
            else:        # halfway / integer-ish values
                v = (r.next() % 2000001) / 2.0 - 500000.0
                a32.append(f2u(v)); b32.append(f2u(float(r.next() % 7) - 3.0))
        if i < kd * kd:
            a64.append(F64_SPECIAL[i // kd]); b64.append(F64_SPECIAL[i % kd])
        else:
            k = i % 3
            if k == 0:
                a64.append(r.next()); b64.append(r.next())
            elif k == 1:
                a64.append(d2u(((r.next() >> 11) / 2.0 ** 53 - 0.5) * 2.0 ** ((r.next() % 80) - 40)))
                b64.append(d2u(((r.next() >> 11) / 2.0 ** 53 - 0.5) * 2.0 ** ((r.next() % 80) - 40)))
            else:
                v = (r.next() % 20000001) / 2.0 - 5000000.0
                a64.append(d2u(v)); b64.append(d2u(float(r.next() % 7) - 3.0))
        v = r.next() & 0xffffffff
        i32.append(I32_SPECIAL[i] if i < len(I32_SPECIAL) else (v - (1 << 32) if v >= (1 << 31) else v))
        i64.append(I64_SPECIAL[i] if i < len(I64_SPECIAL) else r.next())
    a32 += a32[:3]; b32 += b32[:3]; a64 += a64[:1]; b64 += b64[:1]
    return a32, b32, a64, b64, i32, i64


def wat_ops():
    """(name, kind, out_bytes, body) where body computes the stored value at $o from
    addresses $a/$b ($i scaled). kinds: f32 f64 i32 i64 v128."""
    ops = []
    L32a, L32b = '(f32.load (local.get $a4))', '(f32.load (local.get $b4))'
    L64a, L64b = '(f64.load (local.get $a8))', '(f64.load (local.get $b8))'
    Li32, Li64 = '(i32.load (local.get $i4))', '(i64.load (local.get $i8))'
    Va32, Vb32 = '(v128.load (local.get $a4))', '(v128.load (local.get $b4))'
    Va64, Vb64 = '(v128.load (local.get $a8))', '(v128.load (local.get $b8))'
    for op in ['add', 'sub', 'mul', 'div', 'min', 'max', 'copysign']:
        ops.append((f'f32.{op}', 'f32', 4, f'(f32.{op} {L32a} {L32b})'))
        ops.append((f'f64.{op}', 'f64', 8, f'(f64.{op} {L64a} {L64b})'))
    for op in ['abs', 'neg', 'sqrt', 'ceil', 'floor', 'trunc', 'nearest']:
        ops.append((f'f32.{op}', 'f32', 4, f'(f32.{op} {L32a})'))
        ops.append((f'f64.{op}', 'f64', 8, f'(f64.{op} {L64a})'))
    for op in ['eq', 'ne', 'lt', 'gt', 'le', 'ge']:
        ops.append((f'f32.{op}', 'i32', 4, f'(f32.{op} {L32a} {L32b})'))
        ops.append((f'f64.{op}', 'i32', 4, f'(f64.{op} {L64a} {L64b})'))
    ops.append(('f32.load_store', 'f32', 4, L32a))
    ops.append(('f64.load_store', 'f64', 8, L64a))
    ops.append(('f32.reinterpret_roundtrip', 'i32', 4, '(i32.reinterpret_f32 (f32.reinterpret_i32 (i32.load (local.get $a4))))'))
    ops.append(('f64.reinterpret_roundtrip', 'i64', 8, '(i64.reinterpret_f64 (f64.reinterpret_i64 (i64.load (local.get $a8))))'))
    ops.append(('f32.demote_f64', 'f32', 4, f'(f32.demote_f64 {L64a})'))
    ops.append(('f64.promote_f32', 'f64', 8, f'(f64.promote_f32 {L32a})'))
    for s in ['s', 'u']:
        ops.append((f'f32.convert_i32_{s}', 'f32', 4, f'(f32.convert_i32_{s} {Li32})'))
        ops.append((f'f32.convert_i64_{s}', 'f32', 4, f'(f32.convert_i64_{s} {Li64})'))
        ops.append((f'f64.convert_i32_{s}', 'f64', 8, f'(f64.convert_i32_{s} {Li32})'))
        ops.append((f'f64.convert_i64_{s}', 'f64', 8, f'(f64.convert_i64_{s} {Li64})'))
        ops.append((f'i32.trunc_sat_f32_{s}', 'i32', 4, f'(i32.trunc_sat_f32_{s} {L32a})'))
        ops.append((f'i32.trunc_sat_f64_{s}', 'i32', 4, f'(i32.trunc_sat_f64_{s} {L64a})'))
        ops.append((f'i64.trunc_sat_f32_{s}', 'i64', 8, f'(i64.trunc_sat_f32_{s} {L32a})'))
        ops.append((f'i64.trunc_sat_f64_{s}', 'i64', 8, f'(i64.trunc_sat_f64_{s} {L64a})'))
    # trapping truncations, guarded like mlua's Value::eq (in range -> convert, else 0x5a5a...)
    ops.append(('i64.trunc_f64_s_guarded', 'i64', 8,
                f'(if (result i64) (i32.and (f64.ge {L64a} (f64.const -0x1p63)) (f64.lt {L64a} (f64.const 0x1p63))) '
                f'(then (i64.trunc_f64_s {L64a})) (else (i64.const 0x5a5a5a5a5a5a5a5a)))'))
    ops.append(('i32.trunc_f32_s_guarded', 'i32', 4,
                f'(if (result i32) (i32.and (f32.ge {L32a} (f32.const -0x1p31)) (f32.lt {L32a} (f32.const 0x1p31))) '
                f'(then (i32.trunc_f32_s {L32a})) (else (i32.const 0x5a5a5a5a)))'))
    # SIMD (lanes = 4 consecutive f32 records / 2 consecutive f64 records)
    for op in ['add', 'sub', 'mul', 'div', 'min', 'max', 'pmin', 'pmax', 'eq', 'ne', 'lt', 'gt', 'le', 'ge']:
        ops.append((f'f32x4.{op}', 'v128', 16, f'(f32x4.{op} {Va32} {Vb32})'))
        ops.append((f'f64x2.{op}', 'v128', 16, f'(f64x2.{op} {Va64} {Vb64})'))
    for op in ['abs', 'neg', 'sqrt', 'ceil', 'floor', 'trunc', 'nearest']:
        ops.append((f'f32x4.{op}', 'v128', 16, f'(f32x4.{op} {Va32})'))
        ops.append((f'f64x2.{op}', 'v128', 16, f'(f64x2.{op} {Va64})'))
    ops.append(('f64x2.promote_low_f32x4', 'v128', 16, f'(f64x2.promote_low_f32x4 {Va32})'))
    ops.append(('f32x4.demote_f64x2_zero', 'v128', 16, f'(f32x4.demote_f64x2_zero {Va64})'))
    ops.append(('f32x4.convert_i32x4_s', 'v128', 16, '(f32x4.convert_i32x4_s (v128.load (local.get $a4)))'))
    ops.append(('f32x4.convert_i32x4_u', 'v128', 16, '(f32x4.convert_i32x4_u (v128.load (local.get $a4)))'))
    ops.append(('i32x4.trunc_sat_f32x4_s', 'v128', 16, f'(i32x4.trunc_sat_f32x4_s {Va32})'))
    ops.append(('i32x4.trunc_sat_f32x4_u', 'v128', 16, f'(i32x4.trunc_sat_f32x4_u {Va32})'))
    ops.append(('f32x4.splat_extract_replace', 'v128', 16,
                f'(f32x4.replace_lane 2 (f32x4.splat (f32x4.extract_lane 1 {Va32})) {L32b})'))
    ops.append(('f64x2.splat_extract_replace', 'v128', 16,
                f'(f64x2.replace_lane 0 (f64x2.splat (f64x2.extract_lane 1 {Va64})) {L64b})'))
    ops.append(('v128.bitselect_f32', 'v128', 16, f'(v128.bitselect {Va32} {Vb32} (f32x4.lt {Va32} {Vb32}))'))
    return ops


def wat(ops):
    out = ['(module', '  (memory (export "memory") 32 32)']
    for name, kind, w, body in ops:
        fn = 'op_' + name.replace('.', '_')
        store = {'f32': 'f32.store', 'f64': 'f64.store', 'i32': 'i32.store', 'i64': 'i64.store',
                 'v128': 'v128.store'}[kind]
        out.append(f'''  (func (export "{fn}") (param $n i32) (local $i i32) (local $a4 i32) (local $b4 i32)
    (local $a8 i32) (local $b8 i32) (local $i4 i32) (local $i8 i32) (local $o i32)
    (block $done (loop $l
      (br_if $done (i32.ge_u (local.get $i) (local.get $n)))
      (local.set $a4 (i32.add (i32.const {A32}) (i32.shl (local.get $i) (i32.const 2))))
      (local.set $b4 (i32.add (i32.const {B32}) (i32.shl (local.get $i) (i32.const 2))))
      (local.set $a8 (i32.add (i32.const {A64}) (i32.shl (local.get $i) (i32.const 3))))
      (local.set $b8 (i32.add (i32.const {B64}) (i32.shl (local.get $i) (i32.const 3))))
      (local.set $i4 (i32.add (i32.const {I32}) (i32.shl (local.get $i) (i32.const 2))))
      (local.set $i8 (i32.add (i32.const {I64}) (i32.shl (local.get $i) (i32.const 3))))
      (local.set $o (i32.add (i32.const {OUT}) (i32.shl (local.get $i) (i32.const 4))))
      ({store} (local.get $o) {body})
      (local.set $i (i32.add (local.get $i) (i32.const 1)))
      (br $l))))''')
    out.append(')')
    return '\n'.join(out) + '\n'


def main():
    ops = wat_ops()
    a32, b32, a64, b64, i32, i64 = build_inputs()
    with open(f'{OUTDIR}/micro.wat', 'w') as f:
        f.write(wat(ops))
    with open(f'{OUTDIR}/ops.txt', 'w') as f:
        for name, kind, w, _ in ops:
            f.write(f'{name} {kind} {w}\n')
    # inputs.bin: header "KHZMICRO" N, then the six arrays at their memory offsets
    # relative to 0x10000 (one blob copied to memory[0x10000..0x100000)).
    blob = bytearray(OUT - A32)
    struct.pack_into(f'<{len(a32)}I', blob, A32 - A32, *a32)
    struct.pack_into(f'<{len(b32)}I', blob, B32 - A32, *b32)
    struct.pack_into(f'<{len(a64)}Q', blob, A64 - A32, *a64)
    struct.pack_into(f'<{len(b64)}Q', blob, B64 - A32, *b64)
    struct.pack_into(f'<{len(i32)}i', blob, I32 - A32, *i32)
    struct.pack_into(f'<{len(i64)}Q', blob, I64 - A32, *[x & ((1 << 64) - 1) for x in i64])
    with open(f'{OUTDIR}/inputs.bin', 'wb') as f:
        f.write(struct.pack('<8sI', b'KHZMICRO', N))
        f.write(blob)
    print(f'{len(ops)} ops, N={N}')


main()
