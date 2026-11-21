#!/usr/bin/env bash
# Per-callback cost of the production rules on their real callbacks (bot60
# scenarios), worst-case budget-exhausting callbacks and module load times,
# per design, native / Node / Chromium. Output: out/bench/*.
# usage: bench_rules.sh [runs]
set -u
HERE="$(cd "$(dirname "$0")/.." && pwd)"
. "$HERE/env.sh"
cd "$S4"
RUNS=${1:-3}
BIN="$CARGO_TARGET_DIR/release/s4"
export S4_PKG="$S4/pkg"
mkdir -p out/bench
NOTRAP="--disable-features=WebAssemblyTrapHandler"
for r in $(seq 1 "$RUNS"); do
  for t in apex_circuit sky_deck feature_lab; do
    zip=bundles/${t}_wasm.pinbundle; scn=$S1/rec/${t}_bot60.scn
    for d in native native-instr-depth wasmi-instr-depth wasmi-fuel; do
      "$BIN" bench "$d" "$zip" "$scn" >"out/bench/cb.native.$d.$t.$r.json"
    done
    for d in jit-depth wasmi-instr-depth wasmi-fuel; do
      node node/run_web.mjs mode=cache bench "$d" "$zip" "$scn" >"out/bench/cb.node.$d.$t.$r.json"
    done
    for d in jit-depth wasmi-instr-depth; do
      S4_CHROMIUM_ARGS="$NOTRAP" node node/chromium.cjs 'web/index.html?iso=1' bench \
        "{\"design\":\"$d\",\"bundle\":\"/$zip\",\"scn\":\"/s1rec/${t}_bot60.scn\",\"mode\":\"cache\"}" \
        "out/bench/cb.chromium.$d.$t.$r.json" 2>/dev/null
    done
  done
done
for d in native native-instr-depth wasmi-instr-depth wasmi-fuel; do "$BIN" worst "$d" 5 >"out/bench/worst.native.$d.txt"; done
for d in jit-depth wasmi-instr-depth wasmi-fuel; do node node/run_web.mjs worst "$d" 5 >"out/bench/worst.node.$d.txt"; done
for d in jit-depth wasmi-instr-depth; do
  S4_CHROMIUM_ARGS="$NOTRAP" node node/chromium.cjs 'web/index.html?iso=1' worst "{\"design\":\"$d\",\"reps\":5}" "out/bench/worst.chromium.$d.txt" 2>/dev/null
done
for t in apex_circuit sky_deck feature_lab; do
  w=repo/tables/$t/rules.wasm
  for d in native native-instr-depth wasmi-instr-depth wasmi-fuel; do "$BIN" loadtime "$d" "$w" 20; done >"out/bench/load.native.$t.txt"
  { for d in wasmi-instr-depth wasmi-fuel jit-depth; do node node/run_web.mjs loadtime "$d" "$w" 20; done
    node node/run_web.mjs mode=cache loadtime jit-depth "$w" 20; } >"out/bench/load.node.$t.txt"
  for m in sync cache; do
    for d in jit-depth wasmi-instr-depth; do
      S4_CHROMIUM_ARGS="$NOTRAP" node node/chromium.cjs 'web/index.html?iso=1' loadtime "{\"design\":\"$d\",\"rules\":\"/$w\",\"reps\":20,\"mode\":\"$m\"}" - 2>/dev/null
    done
  done >"out/bench/load.chromium.$t.txt"
done
echo done
