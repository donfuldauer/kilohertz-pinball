#!/bin/sh
# S7: the S1 harness `s1 play` (recorded script stream, every tick hashed) on aarch64 under
# qemu-aarch64 (or natively for an x86_64 binary dir), compared tick by tick with the S1
# native x86_64 reference hashes (S1/native/<scn>.hashes).
# usage: native_arm.sh <bin_dir under S7/bin> [x86]   (x86: run the binary directly)
# env: SCENARIOS to limit. Output: S7/hashes/<bin_dir>/<scn>.hashes, one PASS/FAIL line each.
set -u
S7=${WEB_SPIKES}/S7
S1=${WEB_SPIKES}/S1
BD=$1; MODE=${2:-qemu}
OUT=$S7/hashes/$BD
mkdir -p "$OUT" "$S7/logs/native"
ALL="replay_table_fixture10k apex_circuit_bot60 apex_circuit_pat60 apex_circuit_mb8 sky_deck_bot60 sky_deck_pat60 sky_deck_mb8 feature_lab_bot60 feature_lab_pat60 feature_lab_mb8 apex_circuit_bot600"
for s in ${SCENARIOS:-$ALL}; do
  t=${s%_*}
  if [ "$MODE" = x86 ]; then RUN=""; else RUN="nice qemu-aarch64 -L /usr/aarch64-linux-gnu"; fi
  t0=$(date +%s.%N)
  $RUN "$S7/bin/$BD/s1" play "$S1/bundles/$t.pinbundle" "$S1/rec/$s.scn" "$S1/rec/$s.scr" "$OUT/$s.hashes" \
    > "$S7/logs/native/${BD}_$s.log" 2>&1
  t1=$(date +%s.%N)
  echo "INFO $BD $s: $(tail -1 "$S7/logs/native/${BD}_$s.log") wall_s=$(echo "$t1 - $t0" | bc)"
  python3 -I "$S1/harness/compare.py" "$BD vs native x86_64 $s" "$S1/native/$s.hashes" "$OUT/$s.hashes"
done
