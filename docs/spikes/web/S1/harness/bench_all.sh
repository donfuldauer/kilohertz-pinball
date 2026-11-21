#!/bin/sh
# A27 tick-cost measurements. Writes $S/bench/<route>__<scenario>.json (one JSON object each).
# Run on an otherwise idle machine; prints the load average before and after.
set -u
S=${WEB_SPIKES}/S1
. "$S/env.sh"
B=$S/bench; mkdir -p "$B"
TD=/home/user/kilohertz-pinball/target/web_s1
SCN=${SCN:-"apex_circuit_bot60 sky_deck_bot60 feature_lab_bot60 apex_circuit_mb8 sky_deck_mb8 feature_lab_mb8"}
echo "load before: $(cat /proc/loadavg)"
cd "$S"
for s in $SCN; do
  t=${s%_*}
  $TD/release/s1 bench bundles/$t.pinbundle rec/$s.scn rec/$s.scr 16 5000 > "$B/native__$s.json"
  $TD/release/s1 bench-lua bundles/$t rec/$s.scn > "$B/native_lua__$s.json"
  [ -x $TD/scalar/release/s1 ] && $TD/scalar/release/s1 bench bundles/$t.pinbundle rec/$s.scn rec/$s.scr 16 5000 > "$B/native_scalar__$s.json"
  S1_DIR=$S node --no-warnings harness/run_wasi.mjs runs/simd/s1_wasip1.wasm bench /s/bundles/$t.pinbundle /s/rec/$s.scn /s/rec/$s.scr 16 5000 > "$B/wasip1_simd__$s.json"
  for v in o3 o3_lto o3_lto_wopt_O3 os_lto_wopt_Oz; do
    node harness/run_bindgen.mjs $S/runs/variants/$v/pkg_node bench bundles/$t.pinbundle rec/$s.scn rec/$s.scr 16 5000 > "$B/bindgen_${v}__$s.json"
  done
  node harness/run_bindgen.mjs $S/runs/base/pkg_node bench bundles/$t.pinbundle rec/$s.scn rec/$s.scr 16 5000 > "$B/bindgen_nosimd__$s.json"
done
echo "load after: $(cat /proc/loadavg)"
