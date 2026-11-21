#!/bin/sh
# S3: builds the web_probe wasm for one backend variant and profile, then
# (optionally) runs wasm-bindgen into web/pkg/<variant>/.
#   build_wasm.sh <webgpu|webgl|both> <fast|z|s|3> [bindgen]
# fast = opt-level 1, no LTO (harness iteration); z/s/3 = opt-level with
# lto=fat, codegen-units=1, panic=abort (the size table).
set -eu
here=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
. "$here/env.sh"
variant=$1; prof=$2; bind=${3:-}
case $variant in
  webgpu) feats=webgpu,fragile-send-sync ;;
  webgl) feats=webgl,fragile-send-sync ;;
  both) feats=webgpu,webgl,fragile-send-sync ;;
  *) echo "variant?" >&2; exit 2 ;;
esac
case $prof in
  fast) opt=1; lto=false; cgu=16 ;;
  z|s|3) opt=$prof; lto=fat; cgu=1 ;;
  *) echo "profile?" >&2; exit 2 ;;
esac
cd "$S/repo"
CARGO_PROFILE_RELEASE_OPT_LEVEL=$opt CARGO_PROFILE_RELEASE_LTO=$lto \
CARGO_PROFILE_RELEASE_CODEGEN_UNITS=$cgu CARGO_PROFILE_RELEASE_PANIC=abort \
CARGO_PROFILE_RELEASE_DEBUG=false CARGO_PROFILE_RELEASE_STRIP=false \
  cargo build --release --target wasm32-unknown-unknown -p web_probe --features "$feats"
mkdir -p "$S/wasm"
out="$S/wasm/web_probe-$variant-$prof.wasm"
cp "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/web_probe.wasm" "$out"
echo "built $out ($(stat -c %s "$out") bytes)"
if [ "$bind" = bindgen ]; then
  rm -rf "$S/web/pkg/$variant"
  wasm-bindgen --target web --out-name web_probe --out-dir "$S/web/pkg/$variant" "$out"
  echo "bindgen -> $S/web/pkg/$variant"
fi
