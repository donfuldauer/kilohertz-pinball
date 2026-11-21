#!/bin/sh
# Builds the S2 harness: native (x86_64, release) and the web module
# (wasm32-unknown-unknown, release, +simd128, 4 MiB shadow stack, Lua via
# wasi-sdk 32 clang + wasi-libc), then wasm-bindgen (nodejs + web targets).
# usage: build.sh [native|wasm|all]   (default all)
set -eu
S=${WEB_SPIKES}/S2
. "$S/env.sh"; . "$S/wasm_env.sh"
what=${1:-all}
cd "$S/repo"
if [ "$what" != wasm ]; then
  cargo build -q --release -p s2_harness --bin s2
fi
if [ "$what" != native ]; then
  RUSTFLAGS="$S2_WASM_RUSTFLAGS" CARGO_TARGET_DIR=$CARGO_TARGET_DIR/simd \
    cargo build -q --release --target wasm32-unknown-unknown -p s2_harness --lib
  W=$CARGO_TARGET_DIR/simd/wasm32-unknown-unknown/release/s2_harness.wasm
  rm -rf "$S/runs/pkg_node" "$S/runs/pkg_web"
  wasm-bindgen --target nodejs --out-dir "$S/runs/pkg_node" "$W"
  wasm-bindgen --target web --out-dir "$S/runs/pkg_web" "$W"
fi
if [ "$what" != native ]; then
  # Route (f): the standalone Lua module (own instance).
  CARGO_TARGET_DIR=$CARGO_TARGET_DIR/simd cargo build -q --release --target wasm32-unknown-unknown -p s2_lua_module
  cp "$CARGO_TARGET_DIR/simd/wasm32-unknown-unknown/release/s2_lua_module.wasm" "$S/runs/s2_lua_module.wasm"
fi
