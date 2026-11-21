#!/bin/sh
# The recommended variant "umath": KHZ_LUA_MATH=1 (Lua's transcendental
# functions through pin_script's Rust libm on every target) + pin_script
# feature web-parity (no collectgarbage in the sandbox), in its own target dirs.
# Outputs: native $CARGO_TARGET_DIR/umath/release/s2, wasm runs/pkg_node_umath.
set -eu
S=${WEB_SPIKES}/S2
. "$S/env.sh"; . "$S/wasm_env.sh"
export KHZ_LUA_MATH=1
T=$CARGO_TARGET_DIR
cd "$S/repo"
CARGO_TARGET_DIR=$T/umath cargo build -q --release -p s2_harness --bin s2 --features pin_script/web-parity
RUSTFLAGS="$S2_WASM_RUSTFLAGS" CARGO_TARGET_DIR=$T/umath_simd \
  cargo build -q --release --target wasm32-unknown-unknown -p s2_harness --lib --features pin_script/web-parity
rm -rf "$S/runs/pkg_node_umath"
wasm-bindgen --target nodejs --out-dir "$S/runs/pkg_node_umath" "$T/umath_simd/wasm32-unknown-unknown/release/s2_harness.wasm"
rm -rf "$S/runs/pkg_web_umath"
wasm-bindgen --target web --out-dir "$S/runs/pkg_web_umath" "$T/umath_simd/wasm32-unknown-unknown/release/s2_harness.wasm"
