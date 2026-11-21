#!/usr/bin/env bash
# S4 adversarial rules modules: the corpus (crates/s4_rules/src/adversarial.rs,
# 82 lines: 79 cases + 3 fuel-boundary bisections) on every design, native,
# Node (V8) and Playwright Chromium, compared with
#   (1) native wasmtime as the engine runs it today (pin_script backend), and
#   (2) native wasmtime running the instrumented module with the depth counter
#       (what the recommendation makes native run).
# Per line: EXACT (load result, every call's outcome, fuel, queue, RNG,
# guest memory and scores identical), CLASS (same outcome classes, different
# kill point/fuel/NaN bits), DIFF (different classification).
# usage: run_rules_adversarial.sh [nobuild]
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/env.sh"
cd "$S4"
mkdir -p out/adv logs
BIN="$CARGO_TARGET_DIR/release/s4"
export S4_PKG="$S4/pkg"
PROD=repo/tables/apex_circuit/rules.wasm
if [ "${1:-}" != "nobuild" ]; then
  (cd repo && cargo build --release -p s4_rules --features native --bin s4) >logs/build_native.log 2>&1 || { echo "native build failed"; exit 2; }
  (cd repo && RUSTFLAGS="-C target-feature=+simd128 -C link-arg=-zstack-size=4194304" \
    cargo build --release --target wasm32-unknown-unknown -p s4_rules --lib) >logs/build_wasm.log 2>&1 || { echo "wasm build failed"; exit 2; }
  wasm-bindgen --target web --out-dir "$S4/pkg" "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/s4_rules.wasm" || exit 2
fi
for d in native native-instr-depth wasmi-fuel wasmi-instr wasmi-instr-depth; do
  "$BIN" adv "$d" "$PROD" >"out/adv/native.$d.txt" 2>&1
done
for d in wasmi-fuel wasmi-instr wasmi-instr-depth jit jit-depth hybrid; do
  node node/run_web.mjs adv "$d" "$PROD" >"out/adv/node.$d.txt" 2>&1
done
for d in jit-depth wasmi-instr-depth hybrid; do
  node node/chromium.cjs 'web/index.html?iso=1' adv "{\"design\":\"$d\",\"prod\":\"/$PROD\"}" "out/adv/chromium.$d.txt" 2>"logs/chromium_adv.$d.err"
done
# Environment finding (RESULTS F-env): in this VM the Playwright Chromium's
# signal-based wasm trap handler crashes the renderer on any out-of-bounds
# memory access (bare V8, no engine code involved: web/oob.html); with the
# trap handler off, V8 bounds-checks explicitly and traps normally. The JIT
# design runs guest code directly on V8, so it is also run with that flag.
# (wasmi checks guest bounds in Rust and needs no flag.)
S4_CHROMIUM_ARGS="--disable-features=WebAssemblyTrapHandler" node node/chromium.cjs 'web/index.html?iso=1' adv \
  "{\"design\":\"jit-depth\",\"prod\":\"/$PROD\"}" "out/adv/chromium.jit-depth.notraphandler.txt" 2>"logs/chromium_adv.jit-depth.notraphandler.err"
{
  echo "== vs (1) native wasmtime today"
  python3 -I harness/adv_compare.py out/adv/native.native.txt out/adv/native.native-instr-depth.txt out/adv/native.wasmi-*.txt out/adv/node.*.txt out/adv/chromium.*.txt
  echo
  echo "== vs (2) native wasmtime + instrumentation with depth counter"
  python3 -I harness/adv_compare.py out/adv/native.native-instr-depth.txt out/adv/native.wasmi-instr-depth.txt out/adv/node.wasmi-instr-depth.txt out/adv/node.jit-depth.txt out/adv/node.hybrid.txt out/adv/chromium.*.txt
} | tee logs/adversarial.log
python3 -I harness/adv_table.py logs/adversarial.log
