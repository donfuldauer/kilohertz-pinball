#!/bin/sh
# S9 build: ONE wasm32-unknown-unknown engine module (sim + real Lua + wasmi rules +
# wgpu WebGPU/WebGL2 renderer + bundle loader) -> wasm-bindgen --target web ->
# wasm-opt -Oz (native binaryen 132) -> dist/ (module, glue, page, bundles, server).
#
# usage: build_web.sh [--features <list>] [--no-default-features] [--out <dir>] [--no-opt]
#   default: features webgpu,webgl,harness -> dist/
# Every flag and env the combination needs is set in env.sh + wasm_env.sh (see BOM.md).
set -eu
here=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
. "$here/env.sh"
. "$here/wasm_env.sh"
feat_args=""
out="$S9/dist"
opt=1
while [ $# -gt 0 ]; do
  case $1 in
    --features) feat_args="$feat_args --features $2"; shift 2 ;;
    --no-default-features) feat_args="$feat_args --no-default-features"; shift ;;
    --out) out=$2; shift 2 ;;
    --no-opt) opt=0; shift ;;
    *) echo "unknown arg $1" >&2; exit 2 ;;
  esac
done
WO=$S9/tools/binaryen-version_132/bin/wasm-opt
[ -x "$WO" ] || { echo "missing $WO (scripts/get_binaryen.sh)" >&2; exit 1; }
mkdir -p "$S9/logs" "$out/pkg"
log=$S9/logs/build_web.$(basename "$out").log
{
  echo "== versions"; rustc -vV | sed -n '1p;/^LLVM/p'; cargo -V; wasm-bindgen --version; wasm-tools --version
  "$WO" --version; "$CC_wasm32_unknown_unknown" --version | head -1; node --version
  echo "== env"; env | grep -E '^(RUSTFLAGS|CC_wasm32|AR_wasm32|KHZ_|CARGO_PROFILE_RELEASE|CARGO_BUILD_JOBS|CARGO_TARGET_DIR|CARGO_INCREMENTAL)=' | sort
} > "$log" 2>&1
t0=$(date +%s)
cd "$S9/repo"
# shellcheck disable=SC2086
cargo build --release --target wasm32-unknown-unknown -p pin_web $feat_args >> "$log" 2>&1 || { grep -E "^error" -A15 "$log" | head -60; exit 1; }
t1=$(date +%s)
W=$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/pin_web.wasm
raw_dir=$S9/out/$(basename "$out")_bindgen
rm -rf "$raw_dir"; mkdir -p "$raw_dir"
cp "$W" "$raw_dir/pin_web.cargo.wasm"
wasm-bindgen --target web --out-name pin_web --out-dir "$raw_dir" "$W" >> "$log" 2>&1
t2=$(date +%s)
rm -rf "$out/pkg"; mkdir -p "$out/pkg"
cp "$raw_dir/pin_web.js" "$raw_dir/pin_web.d.ts" "$out/pkg/"
if [ "$opt" = 1 ]; then
  # Features of the module (wasm-bindgen keeps them): simd128 (S1), legacy EH (S2: Lua
  # setjmp/longjmp + wasm-bindgen's wrappers), the rustc wasm32 defaults.
  "$WO" -Oz --enable-simd --enable-exception-handling --enable-bulk-memory \
    --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals \
    --enable-reference-types --enable-multivalue --strip-debug --strip-producers \
    "$raw_dir/pin_web_bg.wasm" -o "$out/pkg/pin_web_bg.wasm" >> "$log" 2>&1
else
  cp "$raw_dir/pin_web_bg.wasm" "$out/pkg/pin_web_bg.wasm"
fi
t3=$(date +%s)
wasm-tools validate --features=mvp,simd,exceptions,legacy-exceptions,bulk-memory,saturating-float-to-int,sign-extension,mutable-global,reference-types,multi-value \
  "$out/pkg/pin_web_bg.wasm" >> "$log" 2>&1
cp "$S9/web/index.html" "$S9/web/main.js" "$S9/web/style.css" "$S9/web/serve.cjs" "$out/"
{
  echo "== timings (s): cargo $((t1 - t0)) wasm-bindgen $((t2 - t1)) wasm-opt $((t3 - t2))"
  echo "== sizes"; ls -l "$W" "$raw_dir/pin_web_bg.wasm" "$out/pkg/pin_web_bg.wasm" "$out/pkg/pin_web.js"
  echo "== sha256"; sha256sum "$out/pkg/pin_web_bg.wasm" "$out/pkg/pin_web.js"
} >> "$log"
tail -6 "$log"
echo "built $out (log $log)"
