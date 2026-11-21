#!/bin/sh
# S7: play the S1 scenarios with a wasm-bindgen (--target nodejs) build of the harness, under the
# host Node (x86_64, V8 x64 backend) or the linux-arm64 Node under qemu-aarch64 (V8 ARM64 backend),
# and compare every tick with native x86_64 (S1/native/<scn>.hashes).
# usage: wasm_run.sh <pkg_dir> <label> <x86|arm> [node flags...]   env: SCENARIOS to limit.
set -u
S7=${WEB_SPIKES}/S7
S1=${WEB_SPIKES}/S1
PKG=$1; LB=$2; ARCH=$3; shift 3
if [ "$ARCH" = arm ]; then NODE="nice qemu-aarch64 -L /usr/aarch64-linux-gnu $S7/dl/node_arm64/x/node-v22.23.5-linux-arm64/bin/node"; else NODE=node; fi
OUT=$S7/hashes/wasm_$LB; mkdir -p "$OUT" "$S7/logs/wasm_run"
ALL="replay_table_fixture10k apex_circuit_bot60 apex_circuit_pat60 apex_circuit_mb8 sky_deck_bot60 sky_deck_pat60 sky_deck_mb8 feature_lab_bot60 feature_lab_pat60 feature_lab_mb8 apex_circuit_bot600"
for s in ${SCENARIOS:-$ALL}; do
  t=${s%_*}
  $NODE "$@" "$S1/harness/run_bindgen.mjs" "$PKG" play "$S1/bundles/$t.pinbundle" "$S1/rec/$s.scn" "$S1/rec/$s.scr" "$OUT/$s.hashes" > "$S7/logs/wasm_run/${LB}_$s.log" 2>&1
  echo "INFO $LB $s: $(tail -1 "$S7/logs/wasm_run/${LB}_$s.log")"
  python3 -I "$S1/harness/compare.py" "wasm [$LB] vs native x86_64 $s" "$S1/native/$s.hashes" "$OUT/$s.hashes"
done
