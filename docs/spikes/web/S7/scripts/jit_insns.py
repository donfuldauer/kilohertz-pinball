"""Disassembles the guest code blocks of a `qemu-aarch64 -d in_asm` log (raw OBJD-T hex, qemu
built without capstone) that lie in anonymous executable mappings (V8's JIT code space), and
counts mnemonics. usage: python3 -I jit_insns.py <in_asm.log> <maps.txt> <out_dir>
Prints totals of FP/SIMD arithmetic mnemonics found in JIT code vs in the node binary."""
import collections
import os
import re
import subprocess
import sys

log, maps, out = sys.argv[1:4]
os.makedirs(out, exist_ok=True)
jit = []
for line in open(maps):
    p = line.split()
    if len(p) >= 5 and "x" in p[1] and p[4] == "0" and len(p) == 5:
        a, b = (int(x, 16) for x in p[0].split("-"))
        jit.append((a, b))
blocks = []
cur = None
for line in open(log, errors="replace"):
    m = re.match(r"^0x([0-9a-f]+):", line)
    if m:
        cur = [int(m.group(1), 16), ""]
        blocks.append(cur)
    elif line.startswith("OBJD-T: ") and cur is not None:
        cur[1] += line[8:].strip()


def in_jit(a):
    return any(lo <= a < hi for lo, hi in jit)


sel = [(a, bytes.fromhex(h)) for a, h in blocks if h and in_jit(a)]
print(f"jit regions={len(jit)} blocks total={len(blocks)} jit blocks={len(sel)} jit bytes={sum(len(b) for _, b in sel)}")
binf = os.path.join(out, "blk.bin")
counts = collections.Counter()
lines = []
for a, b in sel:
    open(binf, "wb").write(b)
    d = subprocess.run(["aarch64-linux-gnu-objdump", "-D", "-b", "binary", "-m", "aarch64",
                        f"--adjust-vma={a:#x}", binf], capture_output=True, text=True).stdout
    for l in d.splitlines():
        m = re.match(r"^\s*([0-9a-f]+):\s+[0-9a-f]{8}\s+(\S+)\s*(.*)$", l)
        if m:
            counts[m.group(2)] += 1
            lines.append(f"{m.group(1)}\t{m.group(2)}\t{m.group(3)}")
open(os.path.join(out, "jit_disasm.txt"), "w").write("\n".join(lines) + "\n")
fp = {k: v for k, v in counts.items() if re.match(r"^(f|v?fm|faddp|fmla|fmls|fnm)", k)}
print("FP/SIMD-FP mnemonics in JIT code:", dict(sorted(fp.items(), key=lambda kv: -kv[1])))
fused = {k: v for k, v in counts.items() if k in ("fmla", "fmls", "fmadd", "fmsub", "fnmadd", "fnmsub")}
print("fused multiply-add mnemonics in JIT code:", fused if fused else "none")
