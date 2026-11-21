#!/bin/sh
# S3 size table, part 1: build + wasm-bindgen for each "<variant>:<opt>" (lto=fat, cgu=1, panic=abort).
set -eu
here=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
. "$here/env.sh"
for spec in "$@"; do
  variant=${spec%%:*}; opt=${spec##*:}
  start=$(date +%s)
  "$here/build_wasm.sh" "$variant" "$opt" > "$S/logs/size-$variant-$opt.log" 2>&1
  echo "$variant $opt cargo_secs=$(( $(date +%s) - start ))" >> "$S/logs/size_build_times.txt"
  raw="$S/wasm/web_probe-$variant-$opt.wasm"
  bdir="$S/wasm/sizes/$variant-$opt"
  rm -rf "$bdir"; mkdir -p "$bdir"
  wasm-bindgen --target web --out-name web_probe --out-dir "$bdir" "$raw"
  touch "$bdir/.bindgen-done"
done
