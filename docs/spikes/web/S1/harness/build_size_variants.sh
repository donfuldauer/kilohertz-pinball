#!/bin/sh
# Builds the wasm-bindgen sim module in size/speed variants (A27/A24 inputs).
# Output: $S/runs/variants/<name>/{pkg_node,pkg_web}
set -eu
S=${WEB_SPIKES}/S1
. "$S/env.sh"
TD0=$CARGO_TARGET_DIR
cd "$S/repo"
build() { # name, target-dir suffix, extra env...
  name=$1; td=$2; shift 2
  env CARGO_TARGET_DIR="$TD0/$td" RUSTFLAGS="-C target-feature=+simd128" "$@" \
    cargo build -q --release --target wasm32-unknown-unknown -p s1_harness --lib
  out=$S/runs/variants/$name; mkdir -p "$out"
  wasm-bindgen --target nodejs --out-dir "$out/pkg_node" "$TD0/$td/wasm32-unknown-unknown/release/s1_harness.wasm"
  wasm-bindgen --target web --out-dir "$out/pkg_web" "$TD0/$td/wasm32-unknown-unknown/release/s1_harness.wasm"
}
build o3 simd
build o3_lto simd_lto CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
build os_lto simd_os_lto CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1 CARGO_PROFILE_RELEASE_OPT_LEVEL=s
WOPT="node $S/tools/npm/node_modules/binaryen/bin/wasm-opt"
for v in o3_lto os_lto; do
  for lvl in O3 Oz; do
    d=$S/runs/variants/${v}_wopt_$lvl; rm -rf "$d"; cp -r "$S/runs/variants/$v" "$d"
    for p in pkg_node pkg_web; do
      $WOPT --enable-simd --enable-bulk-memory --enable-sign-ext --enable-nontrapping-float-to-int \
        --enable-mutable-globals --enable-multivalue --enable-reference-types \
        -$lvl --strip-debug --strip-producers "$d/$p/s1_harness_bg.wasm" -o "$d/$p/s1_harness_bg.wasm"
    done
  done
done
echo done
