#!/bin/sh
# S7: wasm-bindgen build of the S1 harness on V8's ARM64 backend (Node v22.23.5 linux-arm64,
# official tarball, SHA-256 verified) under qemu-aarch64 user mode.
# usage: wasm_arm.sh <flavour simd|base> <label> [node flags...]
#   simd: S1/runs/simd/pkg_node (+simd128), compared with native x86_64 S1/native/<scn>.hashes
#   base: S1/runs/base/pkg_node (no simd128), compared with native AND with the x86_64-V8 run of
#         the same module (S1/runs/base/bindgen/<scn>.hashes)
# env: SCENARIOS to limit.
set -u
S7=${WEB_SPIKES}/S7
S1=${WEB_SPIKES}/S1
NODE=$S7/dl/node_arm64/x/node-v22.23.5-linux-arm64/bin/node
FL=$1; LB=$2; shift 2
PKG=$S1/runs/$FL/pkg_node
OUT=$S7/hashes/wasm_arm/${FL}_$LB
mkdir -p "$OUT" "$S7/logs/wasm_arm"
ALL="replay_table_fixture10k apex_circuit_bot60 apex_circuit_pat60 apex_circuit_mb8 sky_deck_bot60 sky_deck_pat60 sky_deck_mb8 feature_lab_bot60 feature_lab_pat60 feature_lab_mb8 apex_circuit_bot600"
for s in ${SCENARIOS:-$ALL}; do
  t=${s%_*}
  nice qemu-aarch64 -L /usr/aarch64-linux-gnu "$NODE" "$@" "$S1/harness/run_bindgen.mjs" "$PKG" play \
    "$S1/bundles/$t.pinbundle" "$S1/rec/$s.scn" "$S1/rec/$s.scr" "$OUT/$s.hashes" \
    > "$S7/logs/wasm_arm/${FL}_${LB}_$s.log" 2>&1
  echo "INFO $FL $LB $s: $(tail -1 "$S7/logs/wasm_arm/${FL}_${LB}_$s.log")"
  python3 -I "$S1/harness/compare.py" "wasm-$FL V8-arm64 [$LB] vs native x86_64 $s" "$S1/native/$s.hashes" "$OUT/$s.hashes"
  if [ "$FL" = base ]; then
    python3 -I "$S1/harness/compare.py" "wasm-$FL V8-arm64 [$LB] vs wasm-$FL V8-x86_64 $s" "$S1/runs/base/bindgen/$s.hashes" "$OUT/$s.hashes"
  fi
done
