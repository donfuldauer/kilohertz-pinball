#!/bin/sh
# S8 probe (a): builds probes/c/sizes.c natively and for wasm32-wasip1 (wasi-sdk 32), runs both, diffs.
set -eu
S=${WEB_SPIKES}/S8
WS=$S/../S2/tools/wasi-sdk/wasi-sdk-32.0-x86_64-linux
L=${1:-$S/repo/third_party/lua-src/lua-5.4.9}
D=$S/probes/c; O=$S/runs/probes; mkdir -p "$O"
cc -O0 -I"$L" -o "$O/sizes_native" "$D/sizes.c"
"$O/sizes_native" > "$O/sizes.native.txt"
"$WS/bin/clang" --target=wasm32-wasip1 --sysroot="$WS/share/wasi-sysroot" -D_WASI_EMULATED_SIGNAL -O0 -I"$L" -o "$O/sizes.wasm" "$D/sizes.c"
node "$S/harness/run_wasi_test.mjs" "$O/sizes.wasm" > "$O/sizes.wasm32.txt"
paste "$O/sizes.native.txt" "$O/sizes.wasm32.txt" | awk -F'\t' '{d=($2!=$4)?"DIFF":"same"; printf "%-42s %22s %22s %s\n",$1,$2,$4,d}' > "$O/sizes.table.txt"
cat "$O/sizes.table.txt"
