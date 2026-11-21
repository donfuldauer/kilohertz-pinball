#!/bin/sh
# Writes S2's proposed changes as diff -u files (paths relative to the repo root).
#   0001: third_party/lua-src (KHZ patches 29-32 + KHZ_PATCHES.md) against main (995935f).
#   0002: crates/pin_script against main + S1's patch 0001 (apply S1 0001-0003 first).
#   0003: spike tooling (s2_harness, s2_lua_module) — not for main.
set -u
S=${WEB_SPIKES}/S2
S1=$S/../S1
P=$S/patches; rm -f "$P"/*.patch; mkdir -p "$P"
cd "$S"
d() { # d <base_root> <file relative path>
  if [ -f "$1/$2" ]; then diff -u --label "a/$2" --label "b/$2" "$1/$2" "repo/$2"
  else diff -u --label /dev/null --label "b/$2" /dev/null "repo/$2"; fi
}
{
  echo "# lua-src: wasm32-unknown-unknown arm + shim (KHZ 29), portable hex numerals and %a (KHZ 30, lkhznum.c), fixed LUAL_BUFFERSIZE (KHZ 31), KHZ_LUA_MATH transcendental routing (KHZ 32). Native x86_64 output unchanged for the 3 tables (S2 RESULTS)."
  for f in third_party/lua-src/KHZ_PATCHES.md third_party/lua-src/src/lib.rs third_party/lua-src/khz-wasm/khz_wasm_shim.c \
           third_party/lua-src/lua-5.4.9/lkhznum.c third_party/lua-src/lua-5.4.9/luaconf.h \
           third_party/lua-src/lua-5.4.9/lmathlib.c third_party/lua-src/lua-5.4.9/llimits.h; do
    d pristine "$f"
  done
} > "$P/0001-lua-src-web-khz-29-32.patch"
{
  echo "# pin_script: feature 'lua' (mlua only) apart from 'native-backends' (wasmtime + notify hot reload); Rust-libm ABI for KHZ_LUA_MATH (lua/libm_abi.rs); feature 'web-parity' drops collectgarbage. Apply after S1 patches 0001-0003."
  for f in crates/pin_script/Cargo.toml crates/pin_script/src/lib.rs crates/pin_script/src/lua/mod.rs \
           crates/pin_script/src/lua/sandbox.rs crates/pin_script/src/lua/libm_abi.rs; do
    d "$S1/repo" "$f"
  done
} > "$P/0002-pin_script-lua-feature-libm-web-parity.patch"
{
  echo "# spike tooling only (not for main): s2_harness (parity/sandbox/floats/probes/remote) and s2_lua_module (route f)."
  for f in $(cd repo && find crates/s2_harness crates/s2_lua_module -type f | sort); do
    d "$S1/repo" "$f"
  done
} > "$P/0003-s2-spike-tooling.patch"
for p in "$P"/*.patch; do echo "$(basename "$p"): $(grep -c '^+++ ' "$p") files, $(grep -c '^[+-][^+-]' "$p") changed lines"; done
