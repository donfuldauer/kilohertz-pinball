"""Writes a copy of an S1 scenario (.scn) whose tick count is min(N, original).

usage: python3 -I truncate_scn.py <in.scn> <N> <out.scn>
Layout (s1_harness io.rs): magic[8] version u32 seed u64 table_hash u64 ticks u64 (offset 28)
nops u64 ops... ; trailer = FNV-1a 64 (offset 0xcbf29ce484222325, prime 0x100000001b3) of all
bytes before it. Ops after the new last tick are kept (the driver stops at `ticks`).
"""
import struct
import sys

src, n, dst = sys.argv[1], int(sys.argv[2]), sys.argv[3]
b = bytearray(open(src, "rb").read())
assert b[:8] == b"S1SCEN\0\0"


def fnv(data):
    h = 0xCBF29CE484222325
    for x in data:
        h ^= x
        h = (h * 0x100000001B3) & 0xFFFFFFFFFFFFFFFF
    return h


body = b[:-8]
assert fnv(body) == struct.unpack_from("<Q", b, len(b) - 8)[0], "checksum"
old = struct.unpack_from("<Q", body, 28)[0]
struct.pack_into("<Q", body, 28, min(n, old))
open(dst, "wb").write(bytes(body) + struct.pack("<Q", fnv(body)))
print(f"{src}: ticks {old} -> {min(n, old)}")
