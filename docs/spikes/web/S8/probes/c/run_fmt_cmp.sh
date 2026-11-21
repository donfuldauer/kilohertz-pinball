#!/bin/sh
# S8 probe (d)/(e): glibc (x86_64) vs wasi-libc (wasm32) snprintf/strtod on N samples.
set -eu
S=${WEB_SPIKES}/S8
WS=$S/../S2/tools/wasi-sdk/wasi-sdk-32.0-x86_64-linux; O=$S/runs/probes; N=${1:-1000000}
cc -O2 -o "$O/fmt_cmp_native" "$S/probes/c/fmt_cmp.c"
"$WS/bin/clang" --target=wasm32-wasip1 --sysroot="$WS/share/wasi-sysroot" -O2 -o "$O/fmt_cmp.wasm" "$S/probes/c/fmt_cmp.c"
"$O/fmt_cmp_native" "$N" > "$O/fmt_cmp.native.txt"
node "$S/harness/run_wasi_test.mjs" "$O/fmt_cmp.wasm" "$N" > "$O/fmt_cmp.wasm32.txt" 2>/dev/null
if cmp -s "$O/fmt_cmp.native.txt" "$O/fmt_cmp.wasm32.txt"; then echo "PASS fmt_cmp: $(wc -l < "$O/fmt_cmp.native.txt") digests identical (N=$N)"; else echo "FAIL fmt_cmp"; diff "$O/fmt_cmp.native.txt" "$O/fmt_cmp.wasm32.txt"; exit 1; fi
