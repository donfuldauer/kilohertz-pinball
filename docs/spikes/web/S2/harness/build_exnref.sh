#!/bin/sh
# A25: the base build with the new exnref EH encoding for Lua's setjmp/longjmp
# (KHZ_WASM_LEGACY_EH=false), own target dir; wasm-bindgen nodejs + web.
set -eu
S=${WEB_SPIKES}/S2
. "$S/env.sh"; . "$S/wasm_env.sh"
export KHZ_WASM_LEGACY_EH=false
cd "$S/repo"
RUSTFLAGS="$S2_WASM_RUSTFLAGS" CARGO_TARGET_DIR=$CARGO_TARGET_DIR/exnref \
  cargo build -q --release --target wasm32-unknown-unknown -p s2_harness --lib
W=$CARGO_TARGET_DIR/exnref/wasm32-unknown-unknown/release/s2_harness.wasm
rm -rf "$S/runs/pkg_node_exnref" "$S/runs/pkg_web_exnref"
wasm-bindgen --target nodejs --out-dir "$S/runs/pkg_node_exnref" "$W"
wasm-bindgen --target web --out-dir "$S/runs/pkg_web_exnref" "$W"
echo "raw module EH ops: try_table=$(wasm-tools print "$W" | grep -cE '^\s+try_table') legacy_try=$(wasm-tools print "$W" | grep -cE '^\s+try\b') throw_ref=$(wasm-tools print "$W" | grep -cE '^\s+throw_ref')"
echo "bindgen module EH ops: try_table=$(wasm-tools print "$S/runs/pkg_node_exnref/s2_harness_bg.wasm" | grep -cE '^\s+try_table') legacy_try=$(wasm-tools print "$S/runs/pkg_node_exnref/s2_harness_bg.wasm" | grep -cE '^\s+try\b')"
