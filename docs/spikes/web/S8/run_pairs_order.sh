#!/bin/sh
# S8 deliverable 3: deterministic pairs order for address-keyed tables (KHZ 34).
# (c) correctness: probes/pairs/order.lua, 4 ticks with GC churn, natively
#     twice (ASLR), in Node and in Chromium, for v0 (before) and v2 (KHZ 34);
# (a) header bytes: the C sizeof probe with and without KHZ_KEY_ORDER on
#     x86_64 and wasm32;
# (b) CPU: per-callback timing of the production tables, v1 (no KHZ 34) vs v2
#     (KHZ 34), natively and in Node (median of 3 runs);
# (d) production replays: hash parity is in run_memcap_parity.sh / run_tick_budget.sh (v2).
# usage: run_pairs_order.sh     env: SKIP_BROWSER=1, SKIP_BENCH=1
set -eu
S=${WEB_SPIKES}/S8
. "$S/env.sh"
O=$S/runs/pairs; mkdir -p "$O"
FAIL=0
echo "## (c) pairs order of address-keyed tables (digest of every ORDER line, 4 ticks)"
echo "| variant | native run 1 | native run 2 | Node | Chromium | native 1 = 2 | native = Node | native = Chromium |"
echo "|---|---|---|---|---|---|---|---|"
for V in v0 v2; do
  B=$S/runs/bin/$V
  "$B/s2" s8 pairs "@$S/probes/pairs/order.lua" 4 > "$O/$V.native1.txt"
  "$B/s2" s8 pairs "@$S/probes/pairs/order.lua" 4 > "$O/$V.native2.txt"
  node "$S/node/run.cjs" "$B/pkg_node" pairs "@$S/probes/pairs/order.lua" 4 > "$O/$V.node.txt"
  if [ "${SKIP_BROWSER:-0}" != 1 ]; then
    W=$O/web_$V; rm -rf "$W"; mkdir -p "$W/data"; cp "$S/web/index.html" "$S/web/main.js" "$W/"; cp -r "$B/pkg_web" "$W/pkg"
    cp "$S/probes/pairs/order.lua" "$W/data/"; printf '[{"id":"pairs","cmd":"pairs","args":["@order.lua","4"]}]' > "$W/jobs.json"
    node "$S/node/chromium.cjs" "$W" 8872 "$O/chromium_$V" > "$O/chromium_$V.log" 2>&1 || true
    cp "$O/chromium_$V/pairs.txt" "$O/$V.chromium.txt" 2>/dev/null || echo "none" > "$O/$V.chromium.txt"; rm -rf "$W"
  else echo "skipped" > "$O/$V.chromium.txt"; fi
  d() { tail -1 "$1" | cut -d' ' -f2; }
  e12=$(cmp -s "$O/$V.native1.txt" "$O/$V.native2.txt" && echo PASS || echo FAIL)
  en=$(cmp -s "$O/$V.native1.txt" "$O/$V.node.txt" && echo PASS || echo FAIL)
  if [ "${SKIP_BROWSER:-0}" = 1 ]; then ec=-; else ec=$(cmp -s "$O/$V.native1.txt" "$O/$V.chromium.txt" && echo PASS || echo FAIL); fi
  echo "| $V | $(d "$O/$V.native1.txt") | $(d "$O/$V.native2.txt") | $(d "$O/$V.node.txt") | $(d "$O/$V.chromium.txt") | $e12 | $en | $ec |"
  if [ "$V" = v2 ]; then for x in $e12 $en $ec; do [ "$x" = FAIL ] && FAIL=$((FAIL + 1)); done; fi
done
echo "## (a) header bytes (sizeof, real) without / with KHZ_KEY_ORDER"
WS=$S/../S2/tools/wasi-sdk/wasi-sdk-32.0-x86_64-linux; L=$S/repo/third_party/lua-src/lua-5.4.9
for K in "" "-DKHZ_KEY_ORDER"; do
  cc -O0 $K -I"$L" -o "$O/sizes_n" "$S/probes/c/sizes.c" && "$O/sizes_n" > "$O/sizes_native$K.txt"
  "$WS/bin/clang" --target=wasm32-wasip1 --sysroot="$WS/share/wasi-sysroot" -D_WASI_EMULATED_SIGNAL -O0 $K -I"$L" -o "$O/sizes.wasm" "$S/probes/c/sizes.c"
  node "$S/harness/run_wasi_test.mjs" "$O/sizes.wasm" > "$O/sizes_wasm32$K.txt" 2>/dev/null
done
echo "| struct | x86_64 before | x86_64 KHZ 34 | wasm32 before | wasm32 KHZ 34 |"
echo "|---|---|---|---|---|"
for t in 'sizeof(Table)' 'sizeof(LClosure)' 'offsetof(LClosure,upvals)' 'sizeof(CClosure)' 'offsetof(CClosure,upvalue)' 'offsetof(Udata0,bindata)' 'offsetof(Udata,uv)' 'sizeof(lua_State)'; do
  g() { grep -F "$t	" "$1" | cut -f2; }
  echo "| $t | $(g "$O/sizes_native.txt") | $(g "$O/sizes_native-DKHZ_KEY_ORDER.txt") | $(g "$O/sizes_wasm32.txt") | $(g "$O/sizes_wasm32-DKHZ_KEY_ORDER.txt") |"
done
if [ "${SKIP_BENCH:-0}" != 1 ]; then
  echo "## (b) per-callback cost, v1 (no KHZ 34) vs v2 (KHZ 34), median of 3 runs (us)"
  echo "| scenario | platform | v1 on_tick mean | v2 on_tick mean | v1 on_switch mean | v2 on_switch mean | v1 wall ms | v2 wall ms |"
  echo "|---|---|---|---|---|---|---|---|"
  for sc in apex_circuit_bot60 sky_deck_bot60 feature_lab_bot60 apex_circuit_mb8; do
    t=${sc%_*}
    for P in native node; do
      for V in v1 v2; do
        for r in 1 2 3; do
          if [ $P = native ]; then "$S/runs/bin/$V/s2" play-timed "$S1/bundles/$t.pinbundle" "$S1/rec/$sc.scn" "$S1/rec/$sc.scr" -
          else node "$S/harness/run_node.cjs" "$S/runs/bin/$V/pkg_node" play-timed "$S1/bundles/$t.pinbundle" "$S1/rec/$sc.scn" "$S1/rec/$sc.scr" -; fi
        done | grep '^TIMING' | sed 's/^TIMING //' > "$O/bench_${sc}_${P}_$V.jsonl"
      done
      python3 -I - "$O/bench_${sc}_${P}_v1.jsonl" "$O/bench_${sc}_${P}_v2.jsonl" "$sc" "$P" <<'PY'
import json, statistics, sys
def med(p, k, f):
    v = [json.loads(l) for l in open(p) if l.strip()]
    return statistics.median([(x[k][f] if f else x[k]) for x in v])
a, b, sc, p = sys.argv[1:5]
print(f"| {sc} | {p} | {med(a,'on_tick','mean_us'):.3f} | {med(b,'on_tick','mean_us'):.3f} | {med(a,'on_switch','mean_us'):.3f} | {med(b,'on_switch','mean_us'):.3f} | {med(a,'wall_ms',None):.1f} | {med(b,'wall_ms',None):.1f} |")
PY
    done
  done
fi
echo "== run_pairs_order: $FAIL failing v2 determinism check(s)"
[ "$FAIL" -eq 0 ]
