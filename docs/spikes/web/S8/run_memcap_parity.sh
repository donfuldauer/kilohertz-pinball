#!/bin/sh
# S8 deliverable 2: platform-independent memory accounting (KHZ 33).
# 1. S2's sandbox corpus (79 cases, LuaConfig::engine) natively, in Node and
#    in Chromium, before (v0 = S2's recommended build) and after (v1 = KHZ 33
#    + audit fixes, v2 = v1 + KHZ 34): PASS = identical lines (the harness-only
#    `mem=` real-byte field stripped); plus the budget-edge probe.
# 2. Production replays (S1 recordings: 3 tables x bot60/pat60/mb8, the CI
#    fixture, the 10-minute apex soak) under v2, natively and in Node (Chromium:
#    bot60 + mb8 per table): every script step and every tick's
#    state_hash/ext_hash against S1's native recordings; peak accounted heap.
# usage: run_memcap_parity.sh     env: SKIP_BROWSER=1
set -eu
S=${WEB_SPIKES}/S8
. "$S/env.sh"
O=$S/runs/memcap; mkdir -p "$O"
FAIL=0
strip() { sed 's/ | mem=[0-9]*$//' "$1"; }
for V in v0 v1 v2; do
  B=$S/runs/bin/$V
  "$B/s2" sandbox > "$O/$V.native.txt"
  node "$S/harness/run_node.cjs" "$B/pkg_node" sandbox > "$O/$V.node.txt"
  "$B/s2" budget-edge > "$O/$V.budget_edge.native"; node "$S/harness/run_node.cjs" "$B/pkg_node" budget-edge > "$O/$V.budget_edge.node"
  if [ "${SKIP_BROWSER:-0}" != 1 ]; then
    printf '[{"id":"sandbox","cmd":"sandbox","args":[]},{"id":"budget_edge","cmd":"budget-edge","args":[]}]' > "$O/jobs_$V.json"
    "$S/harness/stage_web.sh" "$V" "$O/web" "$O/jobs_$V.json"
    node "$S/node/chromium.cjs" "$O/web" 8873 "$O/chromium_$V" > "$O/chromium_$V.log" 2>&1 || true
    cp "$O/chromium_$V/sandbox.txt" "$O/$V.chromium.txt" 2>/dev/null || : > "$O/$V.chromium.txt"
    cp "$O/chromium_$V/budget_edge.txt" "$O/$V.budget_edge.chromium" 2>/dev/null || : > "$O/$V.budget_edge.chromium"
    rm -rf "$O/web"
  fi
done
echo "## sandbox corpus: native vs Node vs Chromium, before (v0) and after (v1, v2)"
echo "| case | v0 native = Node | v0 native = Chromium | v1 native = Node | v1 native = Chromium | v2 native = Node | v2 native = Chromium | native outcome changed v0 -> v2 |"
echo "|---|---|---|---|---|---|---|---|"
for V in v0 v1 v2; do strip "$O/$V.native.txt" > "$O/$V.n"; strip "$O/$V.node.txt" > "$O/$V.w"; strip "$O/$V.chromium.txt" > "$O/$V.c"; done
n=$(wc -l < "$O/v0.n"); i=1; P0=0; P2=0
while [ $i -le "$n" ]; do
  row=""; name=$(sed -n "${i}p" "$O/v0.n" | cut -d' ' -f1)
  for V in v0 v1 v2; do
    a=$(sed -n "${i}p" "$O/$V.n"); w=$(sed -n "${i}p" "$O/$V.w"); c=$(sed -n "${i}p" "$O/$V.c")
    ew=$([ "$a" = "$w" ] && echo PASS || echo FAIL)
    if [ -s "$O/$V.c" ]; then ec=$([ "$a" = "$c" ] && echo PASS || echo FAIL); else ec="-"; fi
    row="$row | $ew | $ec"
    [ "$V" = v0 ] && [ "$ew" = PASS ] && P0=$((P0 + 1))
    if [ "$V" = v2 ]; then [ "$ew" = PASS ] && P2=$((P2 + 1)); [ "$ew" = PASS ] || FAIL=$((FAIL + 1)); [ "$ec" = FAIL ] && FAIL=$((FAIL + 1)); fi
  done
  ch=$([ "$(sed -n "${i}p" "$O/v0.n")" = "$(sed -n "${i}p" "$O/v2.n")" ] && echo "no" || echo "YES")
  echo "| $name$row | $ch |"
  i=$((i + 1))
done
echo "== sandbox: v0 $P0/$n identical native=Node, v2 $P2/$n identical native=Node"
echo "## changed native lines v0 -> v2 (expected: cap-filling cases only)"
diff "$O/v0.n" "$O/v2.n" | grep '^[<>]' | cut -c1-260 || true
echo "## mem_gradual_keep (fault point)"
for V in v0 v1 v2; do for P in n w c; do echo "$V.$P: $(grep '^mem_gradual_keep' "$O/$V.$P" | cut -d'|' -f2 | cut -c1-60)"; done; done
echo "## budget-edge (largest n per operation within the budget)"
for V in v0 v1 v2; do
  r=$(cmp -s "$O/$V.budget_edge.native" "$O/$V.budget_edge.node" && echo PASS || echo FAIL)
  rc=$( [ -s "$O/$V.budget_edge.chromium" ] && (cmp -s "$O/$V.budget_edge.native" "$O/$V.budget_edge.chromium" && echo PASS || echo FAIL) || echo -)
  echo "$V native=Node $r native=Chromium $rc: $(tr '\n' ' ' < "$O/$V.budget_edge.native")"
  [ "$V" = v2 ] && [ "$r" = FAIL ] && FAIL=$((FAIL + 1))
done
echo "## production replays under v2 (KHZ 33-37): per step and per tick vs S1 native recordings"
echo "| scenario | platform | steps | queue/digest/draw mismatches | hash lines | hash mismatches | final state_hash | peak accounted heap (v2, logical) | peak (v0, real) | result |"
echo "|---|---|---|---|---|---|---|---|---|---|"
JOBS=$O/jobs_replay.json; printf '[' > "$JOBS"; first=1
for sc in replay_table_fixture10k apex_circuit_bot60 apex_circuit_pat60 apex_circuit_mb8 sky_deck_bot60 sky_deck_pat60 sky_deck_mb8 feature_lab_bot60 feature_lab_pat60 feature_lab_mb8 apex_circuit_bot600; do
  t=${sc%_*}
  args="@$S1/bundles/$t.pinbundle @$S1/rec/$sc.scn @$S1/rec/$sc.scr @$S1/native/$sc.hashes off"
  "$S/runs/bin/v2/s2" s8 tickstats $args > "$O/replay_${sc}_native.txt"
  "$S/runs/bin/v0/s2" s8 tickstats $args > "$O/replay_${sc}_native_v0.txt"
  node "$S/node/run.cjs" "$S/runs/bin/v2/pkg_node" tickstats $args > "$O/replay_${sc}_node.txt"
  case $sc in *bot60|*mb8) [ $first = 1 ] || printf ',' >> "$JOBS"; first=0
    printf '{"id":"%s","cmd":"tickstats","args":["@%s.pinbundle","@%s.scn","@%s.scr","@%s.hashes","off"]}' "$sc" "$t" "$sc" "$sc" "$sc" >> "$JOBS" ;; esac
done
printf ']' >> "$JOBS"
if [ "${SKIP_BROWSER:-0}" != 1 ]; then
  "$S/harness/stage_web.sh" v2 "$O/web" "$JOBS"
  node "$S/node/chromium.cjs" "$O/web" 8874 "$O/chromium_replay" > "$O/chromium_replay.log" 2>&1 || true
  rm -rf "$O/web"
  for f in "$O"/chromium_replay/*.txt; do [ -f "$f" ] && cp "$f" "$O/replay_$(basename "$f" .txt)_chromium.txt"; done
fi
for sc in replay_table_fixture10k apex_circuit_bot60 apex_circuit_pat60 apex_circuit_mb8 sky_deck_bot60 sky_deck_pat60 sky_deck_mb8 feature_lab_bot60 feature_lab_pat60 feature_lab_mb8 apex_circuit_bot600; do
  for P in native node chromium; do
    f="$O/replay_${sc}_$P.txt"; [ -f "$f" ] || continue
    python3 -I - "$f" "$O/replay_${sc}_native_v0.txt" "$sc" "$P" <<'PY' || FAIL=$((FAIL + 1))
import re, sys
s = open(sys.argv[1]).read(); s0 = open(sys.argv[2]).read()
g = lambda k, t=s: (re.search(r'\b' + k + r'=([^ ]+)', t) or [None, '?'])[1]
mm = int(g('queue_mm')) + int(g('digest_mm')) + int(g('draw_mm'))
ok = mm == 0 and g('hash_mm') == '0' and g('steps') != '?' and g('hash_lines') != '0' and g('faults') == '0'
print(f"| {sys.argv[3]} | {sys.argv[4]} | {g('steps')} | {mm} | {g('hash_lines')} | {g('hash_mm')} | {g('state_hash')} | {g('peak_accounted_heap')} | {g('peak_accounted_heap', s0)} | {'PASS' if ok else 'FAIL'} |")
sys.exit(0 if ok else 1)
PY
  done
done
echo "== run_memcap_parity: $FAIL failing check(s)"
[ "$FAIL" -eq 0 ]
