#!/bin/sh
# S3 size table (A24): for each "<variant>:<opt>" argument builds web_probe
# (lto=fat, codegen-units=1, panic=abort), runs wasm-bindgen and
# wasm-opt -Oz, and prints raw / gzip -9 / brotli -11 sizes.
#   size_matrix.sh webgpu:z webgl:z both:z webgpu:s webgpu:3
set -eu
here=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
. "$here/env.sh"
WASM_OPT="node $S/tools/npm-binaryen/node_modules/binaryen/bin/wasm-opt"
mkdir -p "$S/wasm/sizes"
for spec in "$@"; do
  variant=${spec%%:*}; opt=${spec##*:}
  start=$(date +%s)
  "$here/build_wasm.sh" "$variant" "$opt" > "$S/logs/size-$variant-$opt.log" 2>&1
  secs=$(( $(date +%s) - start ))
  raw="$S/wasm/web_probe-$variant-$opt.wasm"
  bdir="$S/wasm/sizes/$variant-$opt"
  rm -rf "$bdir"; mkdir -p "$bdir"
  wasm-bindgen --target web --out-name web_probe --out-dir "$bdir" "$raw"
  $WASM_OPT -Oz "$bdir/web_probe_bg.wasm" -o "$bdir/web_probe_bg.opt.wasm" 2> "$bdir/wasm-opt.log" || echo "wasm-opt failed (see $bdir/wasm-opt.log)"
  echo "== $variant opt-level=$opt (cargo build ${secs}s)"
  node "$S/node/sizes.js" "$raw" "$bdir/web_probe_bg.wasm" "$bdir/web_probe_bg.opt.wasm" "$bdir/web_probe.js"
  printf 'naga source-path strings in raw wasm: '; grep -a -o 'naga-30.0.1/src' "$raw" | wc -l
done
