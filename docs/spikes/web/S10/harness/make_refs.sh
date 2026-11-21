#!/bin/bash
# S10: native references (x86_64) for the Lua and rules jobs, made with the S8
# native binaries copied into S10/mods (s8_v2/s2 = KHZ 29-37 Lua harness; s8_s4/s4
# = S8's copy of the S4 rules harness) and the S8 probe sources (read only).
# Also two native runs of each audit probe (ASLR check, as S8 did).
set -eu
W=${WEB_SPIKES}
S1=$W/S1; S4=$W/S4; S8=$W/S8; S10=$W/S10; M=$S10/mods; REF=$S10/runs/ref
mkdir -p "$REF/lua/src" "$REF/lua/native" "$REF/lua/native2" "$REF/lua/v8" "$REF/rules"
cd /tmp
for f in "$S8"/probes/*.lua; do n=$(basename "$f" .lua); [ "$n" = _lib ] && continue; cat "$S8/probes/_lib.lua" "$f" > "$REF/lua/src/$n.lua"; done
cfg() { case $1 in a_unpack_engine) echo "probe engine-bigbudget 1" ;; b_gc_engine) echo "probe engine 3000" ;; b_memstep_engine) echo "memtrace engine 400" ;; *) echo "probe big 1" ;; esac; }
for f in "$REF"/lua/src/*.lua; do
  n=$(basename "$f" .lua); set -- $(cfg "$n")
  "$M/s8_v2/s2" s8 "$1" "@$f" "$2" "$3" > "$REF/lua/native/probe_$n.txt"
  "$M/s8_v2/s2" s8 "$1" "@$f" "$2" "$3" > "$REF/lua/native2/probe_$n.txt"
done
"$M/s8_v2/s2" s8 pairs "@$S8/probes/pairs/order.lua" 4 > "$REF/lua/native/probe_pairs_order.txt"
"$M/s8_v2/s2" s8 pairs "@$S8/probes/pairs/order.lua" 4 > "$REF/lua/native2/probe_pairs_order.txt"
"$M/s8_v2/s2" sandbox > "$REF/lua/native/sandbox.txt"
"$M/s8_v2/s2" budget-edge > "$REF/lua/native/budget_edge.txt"
# S2 run_lua_sandbox.sh number formatting / parsing / ctype / libm dumps (native glibc build of the v2 tree)
"$M/s8_v2/s2" format 10000 > "$REF/lua/native/dump_format.txt"; "$M/s8_v2/s2" parse 10000 > "$REF/lua/native/dump_parse.txt"; "$M/s8_v2/s2" ctype > "$REF/lua/native/dump_ctype.txt"
for fn in sin cos tan asin acos atan atan2 exp log log2 log10 logb pow sqrt fmod floor cosh sinh tanh frexp ldexp fmodint; do "$M/s8_v2/s2" libm "$fn" 100000 > "$REF/lua/native/dump_libm_$fn.txt"; done
node "$S8/harness/run_node.cjs" "$M/s8_v2/pkg_node" sandbox > "$REF/lua/v8/sandbox.txt"
for s in replay_table_fixture10k apex_circuit_bot60 apex_circuit_mb8 sky_deck_bot60 sky_deck_mb8 feature_lab_bot60 feature_lab_mb8; do
  t=${s%_*}
  for c in off 200000; do
    "$M/s8_v2/s2" s8 tickstats "@$S1/bundles/$t.pinbundle" "@$S1/rec/$s.scn" "@$S1/rec/$s.scr" "@$S1/native/$s.hashes" "$c" > "$REF/lua/native/tick_${s}_$c.txt"
  done
done
for mix in loop alloc strings ccalls errors pattern; do for c in off 200000; do for e in 0 512; do
  "$M/s8_v2/s2" s8 advtick "$mix" "$e" "$c" 1 > "$REF/lua/native/adv_${mix}_${e}_$c.txt"
done; done; done
for s in apex_circuit_bot60 sky_deck_bot60 feature_lab_bot60 apex_circuit_mb8 sky_deck_mb8 feature_lab_mb8; do
  t=${s%_*}
  S8_TICK_CAP=200000 "$M/s8_s4/s4" play wasmi-instr-depth "$S4/bundles/${t}_wasm.pinbundle" "$S1/rec/$s.scn" "$REF/rules/native_cap_$s.hashes" > "$REF/rules/native_cap_$s.txt" 2>&1
  rm -f "$REF/rules/native_cap_$s.hashes"
done
for mix in nop host_tick; do for c in 0 200000; do for e in 0 512; do
  S8_TICK_CAP=$c "$M/s8_s4/s4" s8adv wasmi-instr-depth "$mix" "$e" 1 > "$REF/rules/native_s8adv_${mix}_${e}_$c.txt"
done; done; done
# V8 (Node) outputs of the op-level probes (references for the probe jobs)
mkdir -p "$REF/probe"
for mp in s1_simd:glam_ops s1_simd:nan_bits s7_portable_simd:glam_ops s7_portable_simd:glam_iso s7_portable_simd:quat_ops_iso s7_portable_simd:fp_ties s7_portable_simd:nan_bits s7_portable_nosimd:glam_iso s7_portable_nosimd:quat_ops_iso s7_portable_nosimd:fp_ties; do
  node "$S1/harness/run_bindgen.mjs" "$M/${mp%%:*}" probe "${mp#*:}" > "$REF/probe/${mp%%:*}__${mp#*:}.txt"
done
# KHZ 38 candidate (LUAI_MAXCCALLS=100, harness/build_ccalls_variant.sh): its own V8 sandbox output is the reference
if [ -f "$M/s10_v2_ccalls100/pkg_node/s2_harness_bg.wasm" ]; then
  mkdir -p "$REF/lua/v8_ccalls100"; node "$S8/harness/run_node.cjs" "$M/s10_v2_ccalls100/pkg_node" sandbox > "$REF/lua/v8_ccalls100/sandbox.txt"
fi
# ASLR check of the native references (S8 gate 3): run 1 = run 2 for every probe
for f in "$REF"/lua/native/probe_*.txt; do cmp -s "$f" "$REF/lua/native2/$(basename "$f")" || echo "NATIVE-RUN-TO-RUN DIFF $(basename "$f")"; done
echo "refs done: $(ls "$REF"/lua/native | wc -l) Lua native files, $(ls "$REF"/rules | wc -l) rules native files"
