#!/bin/sh
# S9 web-spike-integration: full build + all checks; prints the PASS/FAIL table for
# (a) replay with rendering, (b) player bundles, (c) both backends, (d) import audit,
# (e) size + load time (numbers), (f) flag compatibility, (g) mobile smoke, req 9 failures.
# usage: ./run_integration.sh [nobuild] [quick]
#   nobuild: reuse dist/ (and the variant modules) instead of rebuilding
#   quick:   skip the 10-minute soak in the browser, the WebGL2 replay pass and conflict C5
# Logs: logs/int_*.log; results: results/*.json, shots/*.png. Exit 0 iff every row PASSes.
set -u
here=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
. "$here/env.sh"
cd "$here"
nobuild=0; quick=0
for a in "$@"; do case $a in nobuild) nobuild=1 ;; quick) quick=1 ;; esac; done
L=$here/logs; R=$here/results; mkdir -p "$L" "$R" "$here/shots"
df -h /home/user | tail -1 > "$L/int_disk.txt"
row() { printf '| %-4s | %-58s | %-4s | %s |\n' "$1" "$2" "$3" "$4" >> "$R/integration_table.md"; }
: > "$R/integration_table.md"
st() { [ "$1" -eq 0 ] && echo PASS || echo FAIL; }

if [ $nobuild = 0 ]; then
  ./build_web.sh > "$L/int_build.log" 2>&1; b=$?
  for v in "both:webgpu,webgl" "webgpu:webgpu" "webgl:webgl"; do n=${v%%:*}; f=${v#*:}
    ./build_web.sh --no-default-features --features "$f" --out "$here/out/v_engine_$n" >> "$L/int_build_variants.log" 2>&1 || b=1; done
  ./build_web.sh >> "$L/int_build.log" 2>&1 || b=1   # leave the target dir on the shipped build
else b=0; fi
python3 -I scripts/make_test_data.py "$here" "$S1/tools/bin/wasm-tools" > "$L/int_testdata.log" 2>&1 || b=1
cp web/index.html web/main.js web/style.css web/serve.cjs dist/

# (d) import audit
./check_imports.sh > "$L/int_imports.log" 2>&1; d=$?
# (f) flag compatibility: build OK with the full set + build_info + the conflict reproductions
node -e "const fs=require('fs');(async()=>{const g=await import(process.argv[1]+'/dist/pkg/pin_web.js');g.initSync({module:fs.readFileSync(process.argv[1]+'/dist/pkg/pin_web_bg.wasm')});const i=JSON.parse(g.build_info());console.log(JSON.stringify(i));process.exit(i.target_feature_simd128&&i.feature_webgpu&&i.feature_webgl&&i.panic==='abort'?0:1)})()" "$here" > "$R/build_info.json" 2> "$L/int_build_info.err"; f1=$?
if [ $quick = 1 ]; then scripts/conflicts.sh quick > "$L/int_conflicts.log" 2>&1; else scripts/conflicts.sh > "$L/int_conflicts.log" 2>&1; fi; f2=$?
./build_web.sh > "$L/int_build_restore.log" 2>&1 || f2=1   # conflicts.sh rebuilt crates: restore dist exactly
f=$(( b + f1 + f2 ))
# S1/S2/S4 harnesses inside the combined module (Node)
node node/parity_node.mjs dist all lua,wasm > "$R/parity_node_dist.txt" 2>&1; pn=$?
node node/probes_node.mjs dist "$R/probes_node.json" > "$L/int_probes_node.log" 2>&1; pr=$?
# (a) replay while rendering (WebGPU: 3 tables x lua/wasm bot60 + 10-min soak; WebGL2: bot60 x 3 x lua/wasm)
soak="apex_circuit:wasm:apex_circuit_bot600:1000"; [ $quick = 1 ] && soak=""
node node/t_replay.cjs webgpu 360 640 min 200 "$R/replay_webgpu.json" apex_circuit:lua sky_deck:lua feature_lab:lua apex_circuit:wasm sky_deck:wasm feature_lab:wasm $soak > "$L/int_replay_webgpu.log" 2>&1; a1=$?
if [ $quick = 0 ]; then
  node node/t_replay.cjs webgl2 360 640 min 200 "$R/replay_webgl2.json" apex_circuit:lua sky_deck:lua feature_lab:lua apex_circuit:wasm sky_deck:wasm feature_lab:wasm > "$L/int_replay_webgl2.log" 2>&1; a2=$?
else a2=0; fi
a=$(( a1 + a2 + pn ))
# (b) player bundles
node node/t_player.cjs webgpu "$R/player_webgpu.json" > "$L/int_player.log" 2>&1; bb=$?
# (c) both backends
node node/t_backends.cjs "$R/backends.json" 540 960 > "$L/int_backends.log" 2>&1; c=$?
# req 9
node node/t_failures.cjs webgpu "$R/failures_webgpu.json" > "$L/int_failures.log" 2>&1; r9=$?
# (g) mobile
node node/t_mobile.cjs "$R/mobile.json" > "$L/int_mobile.log" 2>&1; g=$?
# (e) sizes + load time (numbers; PASS = measured)
scripts/size_report.sh > "$R/sizes.md" 2> "$L/int_sizes.err"; e1=$?
node node/t_loadtime.cjs webgpu "$R/loadtime_webgpu.json" 5 > "$L/int_loadtime_webgpu.log" 2>&1; e2=$?
node node/t_loadtime.cjs webgl2 "$R/loadtime_webgl2.json" 5 > "$L/int_loadtime_webgl2.log" 2>&1; e3=$?
e=$(( e1 + e2 + e3 ))

cnt() { n=$(grep -cE "^$1 " "$2" 2>/dev/null); echo "${n:-0}"; }
{
  echo "| item | what | result | evidence |"; echo "|---|---|---|---|"
} > "$R/integration_table.md"
row a "replay S1 recordings while rendering, per-tick parity" "$(st $a)" "webgpu $(cnt PASS "$L/int_replay_webgpu.log") PASS/$(cnt FAIL "$L/int_replay_webgpu.log") FAIL; webgl2 $(cnt PASS "$L/int_replay_webgl2.log" ) PASS; node $(cnt PASS "$R/parity_node_dist.txt")/20 PASS"
row b "player bundles: input + drop, hostile rejected as native" "$(st $bb)" "$(cnt PASS "$L/int_player.log") PASS / $(cnt FAIL "$L/int_player.log") FAIL (logs/int_player.log)"
row c "WebGPU + WebGL2 frames, auto selection, no GPU errors" "$(st $c)" "$(cnt PASS "$L/int_backends.log") PASS / $(cnt FAIL "$L/int_backends.log") FAIL; shots/frame_*.png"
row d "import audit: glue only, 0 env.*, 0 wasi_*" "$(st $d)" "$(tail -1 "$L/int_imports.log")"
row e "sizes + load-to-first-frame (numbers)" "$(st $e)" "results/sizes.md, results/loadtime_*.json"
row f "flag/feature combination builds; conflicts reproduced" "$(st $f)" "$(tail -1 "$L/int_conflicts.log"); build_info $(st $f1)"
row g "mobile emulation: layout, touch->inputs, frame" "$(st $g)" "$(cnt PASS "$L/int_mobile.log") PASS / $(cnt FAIL "$L/int_mobile.log") FAIL"
row 9 "failure/recovery: Lua error+budget, fuel kill, trap, stack" "$(st $r9)" "$(cnt PASS "$L/int_failures.log") PASS / $(cnt FAIL "$L/int_failures.log") FAIL"
row h "S1/S2/S4 harness checks in the combined module (Node)" "$(st $pr)" "$(tail -1 "$L/int_probes_node.log")"
cat "$R/integration_table.md"
total=$(( a + bb + c + d + e + f + g + r9 + pr ))
echo "== run_integration: $( [ $total -eq 0 ] && echo ALL PASS || echo "FAILURES ($total)" )"
exit $([ $total -eq 0 ] && echo 0 || echo 1)
