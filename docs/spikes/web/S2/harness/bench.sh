#!/bin/sh
# Per-callback cost of the real Lua rules (on_tick / on_switch_edge), native
# vs wasm in-module (route b, Node) vs route (f) separate instance (Node);
# Chromium numbers come from the parity page (runs/parity/chromium_*.json).
# usage: bench.sh [reps] (default 3) -> runs/bench/<route>.<scn>.<rep>.txt and a median table
set -eu
S=${WEB_SPIKES}/S2
. "$S/env.sh"
R=${1:-3}
O=$S/runs/bench; mkdir -p "$O"
NT=$CARGO_TARGET_DIR/umath/release/s2
for rep in $(seq 1 "$R"); do
  for s in apex_circuit_bot60 sky_deck_bot60 feature_lab_bot60 apex_circuit_mb8 sky_deck_mb8 feature_lab_mb8; do
    t=${s%_*}; b=$S1/bundles/$t.pinbundle
    "$NT" play-timed "$b" "$S1/rec/$s.scn" "$S1/rec/$s.scr" - | grep '^TIMING' > "$O/native.$s.$rep.txt"
    node "$S/harness/run_node.cjs" "$S/runs/pkg_node_umath" play-timed "$b" "$S1/rec/$s.scn" "$S1/rec/$s.scr" - | grep '^TIMING' > "$O/wasm_node.$s.$rep.txt"
    S2_TIMING=1 node "$S/harness/run_node.cjs" "$S/runs/pkg_node" play-remote "$S/runs/s2_lua_module.wasm" "$b" "$S1/rec/$s.scn" "$S1/rec/$s.scr" "$S1/native/$s.hashes" | grep '^TIMING' > "$O/remote_node.$s.$rep.txt"
  done
done
python3 -I "$S/harness/bench_table.py" "$O" "$S/runs/parity/chromium_umath.json"
