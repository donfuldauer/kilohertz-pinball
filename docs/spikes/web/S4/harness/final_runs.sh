#!/usr/bin/env bash
# The final evidence runs, in order (logs in logs/, outputs in out/).
set -u
HERE="$(cd "$(dirname "$0")/.." && pwd)"
. "$HERE/env.sh"
cd "$S4"
export S4_PKG="$S4/pkg"
./run_rules_parity.sh full > logs/parity_full.stdout 2>&1; echo "parity rc=$?"
./run_rules_adversarial.sh nobuild > logs/adversarial.stdout 2>&1; echo "adversarial rc=$?"
B=$CARGO_TARGET_DIR/release/s4
{ for t in apex_circuit sky_deck feature_lab; do
    for d in native-instr-depth wasmi-instr-depth wasmi-fuel; do $B alloc $d bundles/${t}_wasm.pinbundle $S1/rec/${t}_bot60.scn; done
    for d in jit-depth wasmi-instr-depth; do node node/run_web.mjs alloc $d bundles/${t}_wasm.pinbundle $S1/rec/${t}_bot60.scn; done
  done; } > out/alloc/alloc_guard.txt 2>&1
{ for d in native native-instr-depth wasmi-fuel wasmi-instr-depth; do $B stack $d 2; done
  for d in jit jit-depth wasmi-fuel wasmi-instr-depth; do node node/run_stack.mjs $d 3; done
  for d in jit jit-depth wasmi-instr-depth; do
    node node/chromium.cjs 'web/index.html?iso=1' stack "{\"design\":\"$d\",\"reps\":3}" - 2>/dev/null | sed "s/^/chromium /"
  done; } > out/stack_all.txt 2>&1
for t in apex_circuit sky_deck feature_lab; do echo "== $t"; $B maxdepth bundles/${t}_wasm.pinbundle $S1/rec/${t}_bot60.scn; done > out/maxdepth.txt 2>&1
echo final-done
