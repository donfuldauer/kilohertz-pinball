#!/bin/sh
# The repo's own Lua test suites (pin_script, lua feature only) compiled for
# wasm32-wasip1 (the same Lua C through lua-src's wasi arm: wasi-sdk 32 clang,
# exnref EH sjlj, wasi-libc) and run under node:wasi, one test binary at a time,
# --test-threads=1 (no threads on wasip1). Suites that need the hot-reload
# watcher (notify) or wasmtime cannot be built for wasip1 and are not listed.
# usage: run_wasm_suites.sh [base|umath]
set -u
S=${WEB_SPIKES}/S2
. "$S/env.sh"; . "$S/wasm_env.sh"
v=${1:-base}
export CC_wasm32_wasip1=$WASI_SDK/bin/clang AR_wasm32_wasip1=$WASI_SDK/bin/llvm-ar
export CARGO_TARGET_DIR=$CARGO_TARGET_DIR/wasip1_tests_$v
F="--features lua"
if [ "$v" = umath ]; then export KHZ_LUA_MATH=1; F="--features lua,web-parity"; fi
SUITES="cr_lua_budget cr_weak_tables lua_alloc spec_api spec_crossproc spec_determinism spec_faults spec_stall"
cd "$S/repo_wasip1"  # copy of repo with proptest fork/timeout off (wait-timeout does not build for wasip1)
T=""; for t in $SUITES; do T="$T --test $t"; done
RUSTFLAGS="-L native=$KHZ_WASI_SYSROOT/lib/wasm32-wasip1" \
  cargo test -q --target wasm32-wasip1 -p pin_script --no-default-features $F --lib $T --no-run 2>&1 | tail -5
for t in lib $SUITES; do
  if [ "$t" = lib ]; then pat="pin_script-*.wasm"; else pat="$t-*.wasm"; fi
  w=$(ls -t "$CARGO_TARGET_DIR"/wasm32-wasip1/debug/deps/$pat 2>/dev/null | head -1)
  if [ -z "$w" ]; then echo "SKIP $t (not built)"; continue; fi
  # Tests whose harness needs a thread (watchdog) or a child process: unsupported on wasip1.
  case $t in
    cr_lua_budget) SK="--skip looping_xpcall_message_handler_faults --skip ordinary_xpcall_still_works" ;;
    spec_crossproc) SK="--skip pairs_order_and_random_draws_are_identical_across_processes --skip table_sort_with_ties_is_identical_across_processes" ;;
    *) SK="" ;;
  esac
  r=$(node --no-warnings "$S/harness/run_wasi_test.mjs" "$w" --test-threads=1 $SK 2>&1 | tee "$S/logs/wasm_suite_${v}_$t.log" | grep '^test result' | tail -1)
  echo "$v $t: ${r:-no result (see logs/wasm_suite_${v}_$t.log)}${SK:+ (skipped: $SK)}"
done
