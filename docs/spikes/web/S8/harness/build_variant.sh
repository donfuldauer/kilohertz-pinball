#!/bin/sh
# S8: builds one variant of the harness natively (x86_64) and for
# wasm32-unknown-unknown (S2 route b), runs wasm-bindgen, and copies the
# artifacts to runs/bin/<variant>/ (s2, pkg_node, pkg_web).
#   v0 = S2's recommended build (KHZ 29-32 + KHZ_LUA_MATH + web-parity)
#   v1 = v0 + KHZ_S8_FIXES (audit fixes) + KHZ_LOGICAL_MEM (KHZ 33)
#   v2 = v1 + KHZ_KEY_ORDER (KHZ 34)
# usage: build_variant.sh v0|v1|v2 [native|wasm|all]
set -eu
S=${WEB_SPIKES}/S8
. "$S/env.sh"; . "$S/wasm_env.sh"
V=$1; WHAT=${2:-all}
export KHZ_LUA_MATH=1
unset KHZ_S8_FIXES KHZ_LOGICAL_MEM KHZ_KEY_ORDER
case $V in
  v0) ;;
  v1) export KHZ_S8_FIXES=1 KHZ_LOGICAL_MEM=1 ;;
  v2) export KHZ_S8_FIXES=1 KHZ_LOGICAL_MEM=1 KHZ_KEY_ORDER=1 ;;
  *) echo "unknown variant $V" >&2; exit 2 ;;
esac
df -h /home/user | tail -1
O=$S/runs/bin/$V; mkdir -p "$O"
cd "$S/repo"
if [ "$WHAT" != wasm ]; then
  CARGO_TARGET_DIR=$CARGO_TARGET_DIR/native cargo build -q --release -p s2_harness --bin s2 --features pin_script/web-parity
  cp "$CARGO_TARGET_DIR/native/release/s2" "$O/s2"
fi
if [ "$WHAT" != native ]; then
  RUSTFLAGS="$S8_WASM_RUSTFLAGS" CARGO_TARGET_DIR=$CARGO_TARGET_DIR/wasm \
    cargo build -q --release --target wasm32-unknown-unknown -p s2_harness --lib --features pin_script/web-parity
  W=$CARGO_TARGET_DIR/wasm/wasm32-unknown-unknown/release/s2_harness.wasm
  rm -rf "$O/pkg_node" "$O/pkg_web"
  wasm-bindgen --target nodejs --out-dir "$O/pkg_node" "$W"
  wasm-bindgen --target web --out-dir "$O/pkg_web" "$W"
fi
echo "built $V: $("$O/s2" s8 features 2>/dev/null || true) wasm=$(wc -c < "$O/pkg_web/s2_harness_bg.wasm" 2>/dev/null || echo -)"
