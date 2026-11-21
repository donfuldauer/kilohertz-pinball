#!/bin/sh
# S11 req 2 (b)/(c): are wasm-bindgen and wasm-opt deterministic with respect to input path,
# output path, working directory, repetition and thread count; which host paths remain in the
# final module? Input: results/repro/a_plain/pin_web.cargo.wasm (from repro_paths.sh).
# usage: scripts/tool_determinism.sh
set -eu
S11=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
SPK=$(dirname "$S11")
WB=$SPK/S1/tools/bin/wasm-bindgen
WT=$SPK/S1/tools/bin/wasm-tools
WO=$SPK/S9/tools/binaryen-version_132/bin/wasm-opt
IN=$S11/results/repro/a_plain/pin_web.cargo.wasm
O=$S11/results/tooldet
rm -rf "$O"; mkdir -p "$O/in a/x" "$O/inb"
cp "$IN" "$O/in a/x/some name.wasm"; cp "$IN" "$O/inb/pin_web.wasm"
sh256() { sha256sum "$1" | cut -c1-64; }
echo "== wasm-bindgen 0.2.130: two input paths/names, two output dirs, two working directories"
(cd "$O/in a" && "$WB" --target web --out-name pin_web --out-dir "$O/bg1" "x/some name.wasm")
(cd / && "$WB" --target web --out-name pin_web --out-dir "$O/bg2 long dir" "$O/inb/pin_web.wasm")
for f in pin_web_bg.wasm pin_web.js pin_web.d.ts pin_web_bg.wasm.d.ts; do
  a=$(sh256 "$O/bg1/$f"); b=$(sh256 "$O/bg2 long dir/$f")
  echo "$f run1 $a run2 $b $( [ "$a" = "$b" ] && echo SAME || echo DIFF)"
done
echo "== wasm-bindgen output custom sections (names kept unless --remove-name-section; DWARF dropped unless --keep-debug)"
"$WT" objdump "$O/bg1/pin_web_bg.wasm" 2>/dev/null | grep -E 'custom|name|producers|debug' | head -12 || true
echo "== wasm-opt version_132 -Oz: default threads twice, BINARYEN_CORES=1, BINARYEN_CORES=4"
F="--enable-simd --enable-exception-handling --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals --enable-reference-types --enable-multivalue --strip-debug --strip-producers"
# shellcheck disable=SC2086
"$WO" -Oz $F "$O/bg1/pin_web_bg.wasm" -o "$O/opt_default1.wasm"
# shellcheck disable=SC2086
"$WO" -Oz $F "$O/bg1/pin_web_bg.wasm" -o "$O/opt_default2.wasm"
# shellcheck disable=SC2086
BINARYEN_CORES=1 "$WO" -Oz $F "$O/bg1/pin_web_bg.wasm" -o "$O/opt_cores1.wasm"
# shellcheck disable=SC2086
BINARYEN_CORES=4 "$WO" -Oz $F "$O/bg2 long dir/pin_web_bg.wasm" -o "$O/opt_cores4.wasm"
for f in opt_default1 opt_default2 opt_cores1 opt_cores4; do echo "$f $(sh256 "$O/$f.wasm")"; done
echo "== host paths left in the final module (a_plain = S9 flags; a_remap = remap recipe)"
for v in a_plain a_remap b_remap; do
  m=$S11/results/repro/$v/pin_web_bg.opt.wasm
  [ -f "$m" ] || continue
  printf '%s: ' "$v"
  strings -n 4 "$m" | grep -o -E '(/root/[^ ]*|/home/[^ ]*|/tmp/[^ ]*|/Users/[^ ]*|/cargo/registry/src/[^/]+/|/rustc/[0-9a-f]+/|/khz/|registry/src/[^/]+/)' |
    sed -E 's#(registry/src/[^/]+/).*#\1#; s#^(/tmp/|/home/|/root/|/Users/)[^ ]*#\1...#' | sort | uniq -c | tr '\n' ';'
  echo
done
