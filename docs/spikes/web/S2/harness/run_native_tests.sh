#!/bin/sh
# Rule 3 / brief: the repo's native pin_script suites (incl. every Lua sandbox
# suite) on the scratch copy WITH the S2 patches, one crate, own target dir.
# usage: run_native_tests.sh [base|umath]
#   base : S2 patches (lua feature split, lua-src wasm arm (inactive natively),
#          lkhznum.c, LUAL_BUFFERSIZE=1024), default features
#   umath: + KHZ_LUA_MATH=1 + pin_script feature web-parity (the recommended build)
set -eu
S=${WEB_SPIKES}/S2
. "$S/env.sh"
v=${1:-base}
cd "$S/repo"
export CARGO_TARGET_DIR=$CARGO_TARGET_DIR/tests
if [ "$v" = umath ]; then export KHZ_LUA_MATH=1; F="--features web-parity"; else F=""; fi
cargo test -p pin_script $F --no-fail-fast 2>&1 | tee "$S/logs/native_tests_$v.log" | grep -E "^test result|FAILED|panicked|^error" || true
echo "== $v: $(grep -c '^test result: ok' "$S/logs/native_tests_$v.log") ok result lines, $(grep -c '^test result: FAILED' "$S/logs/native_tests_$v.log") failed; passed total $(grep '^test result' "$S/logs/native_tests_$v.log" | sed 's/.* \([0-9]*\) passed.*/\1/' | paste -sd+ | bc), failed total $(grep '^test result' "$S/logs/native_tests_$v.log" | sed 's/.*; \([0-9]*\) failed.*/\1/' | paste -sd+ | bc)"
