#!/bin/sh
# S7: re-record S1's scenarios on aarch64 (qemu) with the REAL Lua rules (vendored C Lua built by
# aarch64-linux-gnu-gcc) and compare: the scenario (.scn) and the script-output stream (.scr) must
# be byte-identical to S1's x86_64 recordings (the script saw the same inputs and produced the same
# commands), and the real-Lua per-tick hashes must equal the aarch64 playback hashes.
# usage: lua_record_arm.sh <bin_dir under S7/bin>
set -u
S7=${WEB_SPIKES}/S7
S1=${WEB_SPIKES}/S1
BD=$1; OUT=$S7/rec_$BD; mkdir -p "$OUT"
Q="nice qemu-aarch64 -L /usr/aarch64-linux-gnu $S7/bin/$BD/s1"
for s in ${SCENARIOS:-replay_table_fixture10k apex_circuit_bot60 sky_deck_bot60 feature_lab_bot60 apex_circuit_pat60 sky_deck_pat60 feature_lab_pat60 apex_circuit_mb8 sky_deck_mb8 feature_lab_mb8}; do
  t=${s%_*}; kind=${s##*_}
  case $kind in
    bot60) $Q record-bot "$S1/bundles/$t" "$S1/bundles/$t.pinbundle" 0x4B485A 60000 "$OUT/$s" ;;
    pat60) $Q record-pattern "$S1/bundles/$t.pinbundle" 0x4B485A 60000 0 "$OUT/$s" ;;
    mb8) $Q record-pattern "$S1/bundles/$t.pinbundle" 0x4B485A 20000 8 "$OUT/$s" ;;
    fixture10k) $Q record-pattern "$S1/bundles/$t.pinbundle" 0x4B485A 10000 0 "$OUT/$s" nolaunch ;;
  esac > "$OUT/$s.log" 2>&1
  echo "INFO $BD $s: $(tail -1 "$OUT/$s.log")"
  for x in scn scr khzrec; do
    [ -f "$S1/rec/$s.$x" ] || continue
    if cmp -s "$OUT/$s.$x" "$S1/rec/$s.$x"; then echo "PASS $BD $s .$x byte-identical to S1 x86_64 recording"; else echo "FAIL $BD $s .$x differs from S1 x86_64 recording"; fi
  done
  python3 -I "$S1/harness/compare.py" "$BD real-Lua vs $BD playback $s" "$S7/hashes/$BD/$s.hashes" "$OUT/$s.native_lua.hashes"
  rm -f "$OUT/$s.scr"   # 1.4 MB each; identity already checked
done
