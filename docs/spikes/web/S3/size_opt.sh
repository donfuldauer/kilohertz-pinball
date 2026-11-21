#!/bin/sh
# S3 size table, part 2: wasm-opt -Oz (binaryen 132 via npm) and raw/gzip/brotli sizes,
# for each "<variant>:<opt>" once size_build.sh has produced its wasm-bindgen output.
set -eu
here=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
. "$here/env.sh"
WASM_OPT="node $S/tools/npm-binaryen/node_modules/binaryen/bin/wasm-opt"
for spec in "$@"; do
  variant=${spec%%:*}; opt=${spec##*:}
  bdir="$S/wasm/sizes/$variant-$opt"
  raw="$S/wasm/web_probe-$variant-$opt.wasm"
  while [ ! -f "$bdir/.bindgen-done" ]; do sleep 5; done
  if [ ! -f "$bdir/web_probe_bg.opt.wasm" ]; then
    t=$(date +%s)
    $WASM_OPT -Oz "$bdir/web_probe_bg.wasm" -o "$bdir/web_probe_bg.opt.wasm" 2> "$bdir/wasm-opt.log"
    echo "wasm-opt secs $(( $(date +%s) - t ))" >> "$bdir/wasm-opt.log"
  fi
  secs=$(sed -n "s/^$variant $opt cargo_secs=//p" "$S/logs/size_build_times.txt" | tail -1)
  echo "== $variant opt-level=$opt (cargo build ${secs:-?}s)"
  node "$S/node/sizes.js" "$raw" "$bdir/web_probe_bg.wasm" "$bdir/web_probe_bg.opt.wasm" "$bdir/web_probe.js"
  printf 'naga source-path strings in raw wasm: '; grep -a -o 'naga-30.0.1/src' "$raw" | wc -l
done
