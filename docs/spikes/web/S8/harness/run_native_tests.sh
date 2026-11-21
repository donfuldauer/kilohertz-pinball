#!/bin/sh
# S8: the repo's own native pin_script test suite on the scratch tree, for the
# baseline (S2 base: no knobs, default features) and for the S8 variant v2
# (KHZ_LUA_MATH + KHZ 33-36 knobs, default features, so collectgarbage stays).
# usage: run_native_tests.sh base|v2
set -u
S=${WEB_SPIKES}/S8
. "$S/env.sh"
unset KHZ_LUA_MATH KHZ_S8_FIXES KHZ_LOGICAL_MEM KHZ_KEY_ORDER
case $1 in
  base) ;;
  v2) export KHZ_LUA_MATH=1 KHZ_S8_FIXES=1 KHZ_LOGICAL_MEM=1 KHZ_KEY_ORDER=1 ;;
esac
cd "$S/repo" && CARGO_TARGET_DIR=$CARGO_TARGET_DIR/tests cargo test -p pin_script --no-fail-fast 2>&1 | tee "$S/logs/native_tests_$1.log" | grep -E '^test result|FAILED|panicked|^failures:|^    [a-z_:]+$' | head -80
