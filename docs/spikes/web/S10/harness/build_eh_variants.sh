#!/bin/sh
# S10: rebuild the S8 v2 Lua module (KHZ 29-37, pin_script web-parity) from a copy of
# S8's scratch tree (S10/repo_exnref), twice: legacy EH (KHZ_WASM_LEGACY_EH=true, S8's
# setting; checks the rebuild reproduces S8's bytes) and exnref (KHZ_WASM_LEGACY_EH=false).
# Same flags as S8 harness/build_variant.sh v2 wasm. Output: S10/mods/s10_v2_{legacy,exnref}/pkg_node
set -eu
W=${WEB_SPIKES}
S10=$W/S10; S2=$W/S2
export CARGO_BUILD_JOBS=2 CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/web_s10 CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_TEST_DEBUG=line-tables-only
export PATH=$W/S1/tools/bin:$PATH
export WASI_SDK=$S2/tools/wasi-sdk/wasi-sdk-32.0-x86_64-linux
export CC_wasm32_unknown_unknown=$WASI_SDK/bin/clang AR_wasm32_unknown_unknown=$WASI_SDK/bin/llvm-ar
export KHZ_WASI_SYSROOT=$WASI_SDK/share/wasi-sysroot
export KHZ_LUA_MATH=1 KHZ_S8_FIXES=1 KHZ_LOGICAL_MEM=1 KHZ_KEY_ORDER=1
df -h /home/user | tail -1
cd "$S10/repo_exnref"
for eh in legacy exnref; do
  if [ $eh = legacy ]; then export KHZ_WASM_LEGACY_EH=true; else export KHZ_WASM_LEGACY_EH=false; fi
  RUSTFLAGS="-C target-feature=+simd128 -C link-arg=-zstack-size=4194304" \
    cargo build -q --release --target wasm32-unknown-unknown -p s2_harness --lib --features pin_script/web-parity
  WM=$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/s2_harness.wasm
  O=$S10/mods/s10_v2_$eh; rm -rf "$O"; mkdir -p "$O"
  wasm-bindgen --target nodejs --out-dir "$O/pkg_node" "$WM"
  echo "$eh: raw try_table=$(wasm-tools print "$WM" | grep -cE '^\s+try_table') legacy_try=$(wasm-tools print "$WM" | grep -cE '^\s+try\b') throw_ref=$(wasm-tools print "$WM" | grep -cE '^\s+throw_ref') | bindgen try_table=$(wasm-tools print "$O/pkg_node/s2_harness_bg.wasm" | grep -cE '^\s+try_table') legacy_try=$(wasm-tools print "$O/pkg_node/s2_harness_bg.wasm" | grep -cE '^\s+try\b') sha256=$(sha256sum < "$O/pkg_node/s2_harness_bg.wasm" | cut -c1-64)"
done
df -h /home/user | tail -1
