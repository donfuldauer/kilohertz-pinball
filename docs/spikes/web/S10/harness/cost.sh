#!/bin/sh
# S10 req 4 (indicative only, no assertion): per-tick sim cost (S1 `bench`), per-callback Lua
# cost (S2 `play-timed`), per-callback wasmi cost (S4 `bench`), module load (compile +
# instantiate) and peak RSS, per engine/tier, interleaved with Node (V8) on the same machine.
# usage: cost.sh [reps=3]  -> runs/cost/*.txt ; summary: python3 -I harness/cost_table.py runs/cost
set -u
W=${WEB_SPIKES}
S1=$W/S1; S4=$W/S4; S8=$W/S8; S10=$W/S10; H=$S10/harness; M=$S10/mods
O=$S10/runs/cost; mkdir -p "$O"
REPS=${1:-3}
ETS=${ETS:-"v8:default jsc:default jsc:omg jsc:bbq jsc:ipint jsc:nojit sm160:default sm160:ion sm160:baseline sm153:default"}
SIM="apex_circuit_bot60 sky_deck_mb8 feature_lab_bot60"
LUA="apex_circuit_bot60 sky_deck_mb8"
RUL="apex_circuit_bot60 sky_deck_bot60"
echo "load before: $(cat /proc/loadavg)" > "$O/load.txt"
for r in $(seq 1 "$REPS"); do
  for et in $ETS; do
    e=${et%:*}; t=${et#*:}; k=${e}_$t
    for s in $SIM; do
      tb=${s%_*}
      if [ "$e" = v8 ]; then
        python3 -I "$H/rss.py" "$O/sim__${k}__${s}__r$r.txt" node "$S1/harness/run_bindgen.mjs" "$M/s1_simd" bench "$S1/bundles/$tb.pinbundle" "$S1/rec/$s.scn" "$S1/rec/$s.scr" 16 5000 > "$O/sim__${k}__${s}__r$r.rss"
      else
        python3 -I "$H/rss.py" "$O/sim__${k}__${s}__r$r.txt" "$H/eng.sh" "$e" "$t" "$H/run_s1.js" "$M/s1_simd" bench "$S1/bundles/$tb.pinbundle" "$S1/rec/$s.scn" "$S1/rec/$s.scr" 16 5000 > "$O/sim__${k}__${s}__r$r.rss"
      fi
    done
    for s in $LUA; do
      tb=${s%_*}
      if [ "$e" = v8 ]; then
        python3 -I "$H/rss.py" "$O/lua__${k}__${s}__r$r.txt" node "$S8/harness/run_node.cjs" "$M/s8_v2/pkg_node" play-timed "$S1/bundles/$tb.pinbundle" "$S1/rec/$s.scn" "$S1/rec/$s.scr" "$S1/native/$s.hashes" > "$O/lua__${k}__${s}__r$r.rss"
      else
        python3 -I "$H/rss.py" "$O/lua__${k}__${s}__r$r.txt" "$H/eng.sh" "$e" "$t" "$H/run_s8.js" "$M/s8_v2/pkg_node" play-timed "$S1/bundles/$tb.pinbundle" "$S1/rec/$s.scn" "$S1/rec/$s.scr" "$S1/native/$s.hashes" > "$O/lua__${k}__${s}__r$r.rss"
      fi
    done
    for s in $RUL; do
      tb=${s%_*}
      if [ "$e" = v8 ]; then
        python3 -I "$H/rss.py" "$O/rules__${k}__${s}__r$r.txt" env S4_PKG="$M/s4_pkg" node "$S4/node/run_web.mjs" bench wasmi-instr-depth "$S4/bundles/${tb}_wasm.pinbundle" "$S1/rec/$s.scn" > "$O/rules__${k}__${s}__r$r.rss"
      else
        python3 -I "$H/rss.py" "$O/rules__${k}__${s}__r$r.txt" "$H/eng.sh" "$e" "$t" "$H/run_s4.js" "$M/s4_pkg" "$M/s4_khz_rules.js" bench wasmi-instr-depth "$S4/bundles/${tb}_wasm.pinbundle" "$S1/rec/$s.scn" > "$O/rules__${k}__${s}__r$r.rss"
      fi
    done
    # worst-case rules callback (S4 `worst`: every hostile body runs to the 100 000-fuel budget)
    if [ "$e" = v8 ]; then
      env S4_PKG="$M/s4_pkg" node "$S4/node/run_web.mjs" worst wasmi-instr-depth 3 > "$O/worst__${k}__all__r$r.txt" 2>&1
    else
      "$H/eng.sh" "$e" "$t" "$H/run_s4.js" "$M/s4_pkg" "$M/s4_khz_rules.js" worst wasmi-instr-depth 3 > "$O/worst__${k}__all__r$r.txt" 2>&1
    fi
    # module load: compile + instantiate of the three modules (S1 glue, S8 glue, S4 glue)
    if [ "$e" != v8 ]; then
      "$H/eng.sh" "$e" "$t" "$H/run_s1.js" "$M/s1_simd" load > "$O/load__${k}__s1__r$r.txt" 2>&1
      "$H/eng.sh" "$e" "$t" "$H/run_s8.js" "$M/s8_v2/pkg_node" load > "$O/load__${k}__s8__r$r.txt" 2>&1
      "$H/eng.sh" "$e" "$t" "$H/run_s4.js" "$M/s4_pkg" "$M/s4_khz_rules.js" load > "$O/load__${k}__s4__r$r.txt" 2>&1
    else
      node -e "const t0=performance.now(); require('$M/s1_simd/s1_harness.js'); console.log('load_ms='+(performance.now()-t0).toFixed(1))" > "$O/load__${k}__s1__r$r.txt" 2>&1
      node -e "const t0=performance.now(); require('$M/s8_v2/pkg_node/s2_harness.js'); console.log('load_ms='+(performance.now()-t0).toFixed(1))" > "$O/load__${k}__s8__r$r.txt" 2>&1
      node --input-type=module -e "import fs from 'node:fs'; const t0=performance.now(); const m=await import('$M/s4_pkg/s4_rules.js'); globalThis.khzRules={}; m.initSync({module: fs.readFileSync('$M/s4_pkg/s4_rules_bg.wasm')}); console.log('load_ms='+(performance.now()-t0).toFixed(1))" > "$O/load__${k}__s4__r$r.txt" 2>&1
    fi
  done
done
echo "load after: $(cat /proc/loadavg)" >> "$O/load.txt"
