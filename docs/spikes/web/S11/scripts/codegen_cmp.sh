#!/bin/sh
# S11 req 2: is wasi-sdk 32's clang codegen for wasm32 host-independent?
# The arm64-macos package's libc++/libc++abi/compiler-rt archives were compiled ON the macOS arm64
# runner by that package's own clang (their DWARF holds /Users/runner/work/... paths); the
# x86_64-linux package's were compiled on Linux x86_64 (/src/... paths). This compares every
# archive member's CODE (llvm-objdump -d -r, relocated immediates and reloc addends masked, debug
# sections ignored) between the two packages. Identical code => same instruction selection on
# both hosts for these inputs; remaining byte differences are path strings only.
# usage: scripts/codegen_cmp.sh <linux-wasi-sdk-dir> <mac-wasi-sdk-dir> <out-dir>
set -eu
L=$1; M=$2; O=$3
OD=${OBJDUMP:-llvm-objdump-18}; AR=${LLVMAR:-llvm-ar-18}
mkdir -p "$O"
norm() { # disassemble one object; mask immediates that carry a relocation, and reloc addends
  "$OD" -d -r --no-show-raw-insn "$1" | tail -n +3 | awk '
    /R_WASM_/ { sub(/\+[0-9]+$/, "+A"); sub(/^ *[0-9a-f]+:/, ""); if (prev != "") { sub(/[-0-9]+$/, "R", prev) }; if (prev != "") print prev; prev=""; print "  reloc " $0; next }
    { if (prev != "") print prev; prev=$0 }
    END { if (prev != "") print prev }' | sed -E 's/^ *[0-9a-f]+://'
}
total=0; same_bytes=0; same_code=0; diff_code=0
: > "$O/summary.txt"
for a in lib/clang/22/lib/wasm32-unknown-wasip1/libclang_rt.builtins.a \
         share/wasi-sysroot/lib/wasm32-wasip1/libc++abi.a share/wasi-sysroot/lib/wasm32-wasip1/libc++.a \
         share/wasi-sysroot/lib/wasm32-wasip1/libc.a share/wasi-sysroot/lib/wasm32-wasip1/libsetjmp.a; do
  n=$(echo "$a" | tr '/' '_')
  rm -rf "$O/$n.l" "$O/$n.m"; mkdir -p "$O/$n.l" "$O/$n.m"
  (cd "$O/$n.l" && "$AR" x "$L/$a"); (cd "$O/$n.m" && "$AR" x "$M/$a")
  if ! "$AR" t "$L/$a" > "$O/$n.l.members" || ! "$AR" t "$M/$a" > "$O/$n.m.members" || ! cmp -s "$O/$n.l.members" "$O/$n.m.members"; then
    echo "MEMBER-LIST-DIFFERS $a" | tee -a "$O/summary.txt"
  fi
  t=0; sb=0; sc=0; dc=0
  for f in $(ls "$O/$n.l"); do
    t=$((t + 1))
    if cmp -s "$O/$n.l/$f" "$O/$n.m/$f"; then sb=$((sb + 1)); continue; fi
    if norm "$O/$n.l/$f" > "$O/$n.l/$f.dis" && norm "$O/$n.m/$f" > "$O/$n.m/$f.dis" && cmp -s "$O/$n.l/$f.dis" "$O/$n.m/$f.dis"; then
      sc=$((sc + 1))
    else
      dc=$((dc + 1)); echo "CODE-DIFFERS $a $f" >> "$O/summary.txt"
    fi
  done
  echo "$a: members $t, byte-identical $sb, code-identical (paths/offsets only) $sc, code differs $dc" | tee -a "$O/summary.txt"
  total=$((total + t)); same_bytes=$((same_bytes + sb)); same_code=$((same_code + sc)); diff_code=$((diff_code + dc))
  rm -rf "$O/$n.l" "$O/$n.m"
done
echo "TOTAL members $total, byte-identical $same_bytes, code-identical $same_code, code differs $diff_code" | tee -a "$O/summary.txt"
