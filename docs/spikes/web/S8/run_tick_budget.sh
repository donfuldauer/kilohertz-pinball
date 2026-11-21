#!/bin/sh
# S8 deliverable 4: per-tick aggregate script budget (Lua).
# (a) per-tick totals on every S1 recording: callbacks, hook-charged units
#     (the repo's budget units) and exact units (+ the final partial hook
#     interval), natively and in Node (must be identical: units are
#     deterministic with KHZ 33);
# (b) the worst-case adversarial tick: 512 switch edges + on_tick, every
#     callback just under the 100 000-unit budget, for 6 instruction mixes,
#     natively, in Node and in Chromium (wall time per tick, median of 3);
# (c)/(d) the prototype cap (200 000 units per tick, pin_script
#     LuaScriptBackend::s8_set_tick_cap): production replays unchanged (per
#     step and per tick vs S1's native recordings), adversarial outcome
#     identical native / Node / Chromium.
# (e) the same for Wasm rules with the S8 copy of S4's harness (s4repo,
#     wasmi + instrumentation): per-tick fuel, replay parity with the cap off
#     and on, adversarial tick natively and in Node.
# usage: run_tick_budget.sh [variant=v2]   env: SKIP_BROWSER=1, CAP=200000, ONLY_WASM=1
set -eu
S=${WEB_SPIKES}/S8
. "$S/env.sh"
V=${1:-v2}; CAP=${CAP:-200000}; B=$S/runs/bin/$V
O=$S/runs/tick; mkdir -p "$O"
FAIL=0
if [ "${ONLY_WASM:-0}" = 1 ]; then SCS=""; MIXES_SKIP=1; else MIXES_SKIP=0; fi
SCS="${SCS-}"; [ "${ONLY_WASM:-0}" = 1 ] || SCS="replay_table_fixture10k apex_circuit_bot60 apex_circuit_pat60 apex_circuit_mb8 sky_deck_bot60 sky_deck_pat60 sky_deck_mb8 feature_lab_bot60 feature_lab_pat60 feature_lab_mb8 apex_circuit_bot600"
echo "## (a)+(d) per-tick script totals (Lua) and replay parity with the cap off and on (cap=$CAP)"
echo "| scenario | callbacks/tick max (tick) p99 p50 | hook-charged units/tick max (tick) p99 p50 | exact units/tick max (tick) p99 p50 | native = Node (stats) | hashes cap off native/Node | hashes cap on native/Node |"
echo "|---|---|---|---|---|---|---|"
for sc in $SCS; do
  t=${sc%_*}; a="@$S1/bundles/$t.pinbundle @$S1/rec/$sc.scn @$S1/rec/$sc.scr @$S1/native/$sc.hashes"
  "$B/s2" s8 tickstats $a off > "$O/${sc}_off_native.txt"; node "$S/node/run.cjs" "$B/pkg_node" tickstats $a off > "$O/${sc}_off_node.txt"
  "$B/s2" s8 tickstats $a "$CAP" > "$O/${sc}_cap_native.txt"; node "$S/node/run.cjs" "$B/pkg_node" tickstats $a "$CAP" > "$O/${sc}_cap_node.txt"
  python3 -I - "$O" "$sc" <<'PY' || FAIL=$((FAIL + 1))
import re, sys
o, sc = sys.argv[1:3]
rd = lambda k: open(f"{o}/{sc}_{k}.txt").read()
def ok(s):
    g = lambda k: re.search(r'\b' + k + r'=([^ ]+)', s).group(1)
    return int(g('queue_mm')) + int(g('digest_mm')) + int(g('draw_mm')) + int(g('hash_mm')) == 0 and g('faults') == '0' and int(g('hash_lines')) > 0
s = rd('off_native')
m = lambda k: re.search(k + r'_per_tick max=(\d+) \(tick (\d+)\) p99=(\d+) p50=(\d+)', s).groups()
f = lambda t: f"{t[0]} ({t[1]}) {t[2]} {t[3]}"
same = rd('off_native') == rd('off_node') and rd('cap_native') == rd('cap_node')
off = ok(rd('off_native')) and ok(rd('off_node')); on = ok(rd('cap_native')) and ok(rd('cap_node'))
print(f"| {sc} | {f(m('callbacks'))} | {f(m('charged'))} | {f(m('units'))} | {'PASS' if same else 'FAIL'} | {'PASS' if off else 'FAIL'} | {'PASS' if on else 'FAIL'} |")
sys.exit(0 if (same and off and on) else 1)
PY
done
echo "## (b)+(d) worst-case adversarial tick: 512 edges + on_tick, each just under the per-callback budget"
MIXES="loop alloc strings ccalls errors pattern"; [ "${ONLY_WASM:-0}" = 1 ] && MIXES=""
JOBS=$O/jobs.json; printf '[' > "$JOBS"; first=1
for mix in $MIXES; do for c in off "$CAP"; do for e in 0 512; do
  id="adv_${mix}_${e}_$c"
  "$B/s2" s8 advtick "$mix" "$e" "$c" 3 > "$O/${id}_native.txt"
  node "$S/node/run.cjs" "$B/pkg_node" advtick "$mix" "$e" "$c" 3 > "$O/${id}_node.txt"
  [ $first = 1 ] || printf ',' >> "$JOBS"; first=0
  printf '{"id":"%s","cmd":"advtick","args":["%s","%s","%s","3"]}' "$id" "$mix" "$e" "$c" >> "$JOBS"
done; done; done
printf ']' >> "$JOBS"
if [ "${SKIP_BROWSER:-0}" != 1 ] && [ "${ONLY_WASM:-0}" != 1 ]; then
  "$S/harness/stage_web.sh" "$V" "$O/web" "$JOBS"
  node "$S/node/chromium.cjs" "$O/web" 8875 "$O/chromium" > "$O/chromium.log" 2>&1 || true
  rm -rf "$O/web"
  for f in "$O"/chromium/*.txt; do [ -f "$f" ] && cp "$f" "$O/$(basename "$f" .txt)_chromium.txt"; done
fi
echo "| mix | edges | cap | callbacks run | units | outcome | native ms | Node ms | Chromium ms | outcome native = Node = Chromium |"
echo "|---|---|---|---|---|---|---|---|---|---|"
for mix in $MIXES; do for c in off "$CAP"; do for e in 0 512; do
  id="adv_${mix}_${e}_$c"
  python3 -I - "$O" "$id" <<'PY' || FAIL=$((FAIL + 1))
import os, re, sys
o, i = sys.argv[1:3]
def rd(p):
    f = f"{o}/{i}_{p}.txt"
    return open(f).read() if os.path.exists(f) else ''
n, w, c = rd('native'), rd('node'), rd('chromium')
first = lambda s: s.split('\n')[0]
ms = lambda s: (re.search(r'median=([0-9.]+)', s) or [None, '-'])[1]
g = lambda k, s=n: (re.search(r'\b' + k + r'=([^ ]+)', s) or [None, '?'])[1]
same = first(n) == first(w) and (not c or first(n) == first(c))
out = 'fault ' + g('last_error') if g('faulted') == 'true' else 'ok'
print(f"| {g('mix')} | {g('edges')} | {g('cap')} | {g('callbacks_run')} | {g('units')} | {out} | {ms(n)} | {ms(w)} | {ms(c) if c else '-'} | {'PASS' if same else 'FAIL'} |")
sys.exit(0 if same else 1)
PY
done; done; done
echo "## Wasm rules (S8 copy of S4's harness: wasmi + wasmtime-exact instrumentation, design wasmi-instr-depth)"
echo "| scenario | platform | cap | fuel/tick max (tick) p99 p50 | callbacks/tick max | per-tick state/ext hashes vs S1 native | final state_hash |"
echo "|---|---|---|---|---|---|---|"
X=$S/runs/bin/s4/s4; export S4_PKG=$S/runs/bin/s4/pkg
for sc in apex_circuit_bot60 apex_circuit_pat60 apex_circuit_mb8 sky_deck_bot60 sky_deck_pat60 sky_deck_mb8 feature_lab_bot60 feature_lab_pat60 feature_lab_mb8 apex_circuit_bot600; do
  t=${sc%_*}; bun=$S4/bundles/${t}_wasm.pinbundle
  for c in 0 "$CAP"; do
    for P in native node; do
      case $sc in *pat60|*bot600) [ $P = node ] && continue ;; esac
      h=$O/w_${sc}_${c}_$P.hashes
      if [ $P = native ]; then S8_TICK_CAP=$c "$X" play wasmi-instr-depth "$bun" "$S1/rec/$sc.scn" "$h" > "$O/w_${sc}_${c}_$P.txt"
      else node "$S/node/s4/run_s4.mjs" "$c" play wasmi-instr-depth "$bun" "$S1/rec/$sc.scn" "$h" > "$O/w_${sc}_${c}_$P.txt"; fi
      cut -d' ' -f1-3 "$h" | cmp -s - "$S1/native/$sc.hashes" && hp=PASS || { hp=FAIL; FAIL=$((FAIL + 1)); }
      python3 -I - "$O/w_${sc}_${c}_$P.txt" "$sc" "$P" "$c" "$hp" <<'PY2'
import re, sys
s = open(sys.argv[1]).read()
f = re.search(r's8_fuel_per_tick max=(\d+) \(tick (\d+)\) p99=(\d+) p50=(\d+)', s)
cb = re.search(r's8_callbacks_per_tick max=(\d+)', s)
st = re.search(r'state_hash=([0-9a-f]+)', s)
fl = re.search(r'faulted=(\w+)', s)
print(f"| {sys.argv[2]} | {sys.argv[3]} | {sys.argv[4] if sys.argv[4] != '0' else 'off'} | {f.group(1)} ({f.group(2)}) {f.group(3)} {f.group(4)} | {cb.group(1)} | {sys.argv[5]} (faulted={fl.group(1)}) | {st.group(1)} |")
PY2
    done
  done
done
echo "| mix | edges | cap | outcome native | outcome Node | native ms | Node ms | native = Node |"
echo "|---|---|---|---|---|---|---|---|"
for mix in nop host_tick; do for c in 0 "$CAP"; do for e in 0 512; do
  S8_TICK_CAP=$c "$X" s8adv wasmi-instr-depth "$mix" "$e" 3 > "$O/wadv_${mix}_${e}_${c}_native.txt"
  node "$S/node/s4/run_s4.mjs" "$c" s8adv wasmi-instr-depth "$mix" "$e" 3 > "$O/wadv_${mix}_${e}_${c}_node.txt"
  python3 -I - "$O/wadv_${mix}_${e}_${c}_native.txt" "$O/wadv_${mix}_${e}_${c}_node.txt" <<'PY2' || FAIL=$((FAIL + 1))
import re, sys
n, w = open(sys.argv[1]).read(), open(sys.argv[2]).read()
first = lambda s: s.split('\n')[0]
g = lambda k, s: (re.search(r'\b' + k + r'=([^ ]+)', s) or [None, '?'])[1]
ms = lambda s: (re.search(r'median=([0-9.]+)', s) or [None, '-'])[1]
o = lambda s: f"run={g('callbacks_run', s)} fuel={g('fuel', s)} faulted={g('faulted', s)}"
same = first(n) == first(w)
print(f"| {g('mix', n)} | {g('edges', n)} | {g('cap', n)} | {o(n)} | {o(w)} | {ms(n)} | {ms(w)} | {'PASS' if same else 'FAIL'} |")
sys.exit(0 if same else 1)
PY2
done; done; done
echo "== run_tick_budget: $FAIL failing check(s)"
[ "$FAIL" -eq 0 ]
