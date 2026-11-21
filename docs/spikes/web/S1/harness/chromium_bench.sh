#!/bin/sh
# Chromium (Playwright) parity + tick-cost run of a wasm-bindgen --target web package.
# usage: chromium_bench.sh <pkg_web dir> <out.json> <isolated 0|1> [scenarios csv]
set -eu
S=${WEB_SPIKES}/S1
PKG=$1; OUTJ=$2; ISO=$3
SC=${4:-apex_circuit_bot60,sky_deck_bot60,feature_lab_bot60,apex_circuit_mb8,sky_deck_mb8,feature_lab_mb8}
W=$(mktemp -d "$S/logs/web_XXXX")
mkdir -p "$W/data"
cp "$S/web/index.html" "$S/web/main.js" "$W/"; cp -r "$PKG" "$W/pkg"
for t in apex_circuit sky_deck feature_lab; do cp "$S/bundles/$t.pinbundle" "$W/data/"; done
for s in $(echo "$SC" | tr ',' ' '); do
  cp "$S/rec/$s.scn" "$S/rec/$s.scr" "$W/data/"; cp "$S/native/$s.hashes" "$W/data/$s.native.hashes"
done
node "$S/harness/chromium.cjs" "$W" $((8800 + $$ % 100)) "$ISO" "$OUTJ" "scn=$SC&bench=1"
rm -rf "$W"
