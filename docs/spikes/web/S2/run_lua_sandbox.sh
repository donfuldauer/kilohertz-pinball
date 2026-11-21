#!/bin/sh
# web-spike-lua (S2): sandbox re-verification on the web. Runs the adversarial
# cases (crates/s2_harness/src/sandbox.rs, through the engine's own
# LuaScriptBackend with LuaConfig::engine()) natively, in wasm under Node and in
# Chromium, and prints a PASS/FAIL table native vs wasm per case; then the
# number-conversion / libm / ctype parity probes and the stack/trap probes.
#
# usage: run_lua_sandbox.sh [all|base|umath]   (default: all)
# env: SKIP_BUILD=1 reuse artifacts (built by run_lua_parity.sh or harness/build*.sh);
#      SKIP_BROWSER=1 skip Chromium.
set -eu
S=${WEB_SPIKES}/S2
. "$S/env.sh"
VAR=${1:-all}
O=$S/runs/sandbox; mkdir -p "$O" "$S/logs"
TOTAL_FAIL=0
KNOWN_FAIL=0
# Known, explained divergences (RESULTS §6): the memory-cap fault point depends on pointer size (both
# variants); in base only, collectgarbage('count') and glibc-vs-musl libm (fixed in umath).
known() { # known <variant> <line>
  case "$2" in
    *"| mem_gradual_keep |"*) return 0 ;;
  esac
  if [ "$1" = base ]; then
    case "$2" in
      *"| collectgarbage_count |"*|*"| math_transcendental |"*|"FAIL base math."*) return 0 ;;
    esac
  fi
  return 1
}
count_fails() { # count_fails <variant> <file>
  while IFS= read -r l; do
    case "$l" in *FAIL*) if known "$1" "$l"; then KNOWN_FAIL=$((KNOWN_FAIL + 1)); else TOTAL_FAIL=$((TOTAL_FAIL + 1)); echo "UNEXPECTED: $l"; fi ;; esac
  done < "$2"
}
run_variant() {
  v=$1
  if [ "$v" = base ]; then NT=$CARGO_TARGET_DIR/release/s2; PN=$S/runs/pkg_node; PW=$S/runs/pkg_web; WD=$S/runs/web
  else NT=$CARGO_TARGET_DIR/umath/release/s2; PN=$S/runs/pkg_node_umath; PW=$S/runs/pkg_web_umath; WD=$S/runs/web_umath; fi
  if [ "${SKIP_BUILD:-0}" != 1 ]; then
    if [ "$v" = base ]; then "$S/harness/build.sh"; else "$S/harness/build_umath.sh"; fi
  fi
  "$NT" sandbox-describe > "$O/describe.txt"
  "$NT" sandbox > "$O/$v.native.txt"
  node "$S/harness/run_node.cjs" "$PN" sandbox > "$O/$v.node.txt"
  CH=""
  if [ "${SKIP_BROWSER:-0}" != 1 ]; then
    "$S/harness/stage_web.sh" "$PW" "$WD"
    port=$((8760 + $(echo "$v" | wc -c)))
    node "$S/harness/chromium.cjs" "$WD" "$port" 1 "$O/$v.chromium.json" "scen=," > "$S/logs/chromium_sandbox_$v.log" 2>&1 || true
    node -e 'const r=require(process.argv[1]); require("fs").writeFileSync(process.argv[2], r.sandbox||""); console.log("chromium "+r.browserVersion+" "+(r.sp_after_errors||r.error))' "$O/$v.chromium.json" "$O/$v.chromium.txt"
    CH="$O/$v.chromium.txt"
  fi
  if grep -q '^TRAP' "$O/$v.node.txt"; then echo "FAIL $v: a case trapped the wasm instance under Node"; TOTAL_FAIL=$((TOTAL_FAIL + 1)); fi
  python3 -I "$S/harness/sandbox_table.py" "$v" "$O/describe.txt" "$O/$v.native.txt" "$O/$v.node.txt" $CH | tee "$O/$v.table.md"
  grep '| FAIL' "$O/$v.table.md" > "$O/$v.fails.txt" || true
  count_fails "$v" "$O/$v.fails.txt"

  echo "== $v: number formatting (10 000 floats x 23 conversions + %q + 2 round trips), parsing (10 000 strings), ctype (256 bytes)"
  "$NT" format 10000 > "$O/$v.format.native"; node "$S/harness/run_node.cjs" "$PN" format 10000 > "$O/$v.format.wasm"
  "$NT" parse 10000 > "$O/$v.parse.native"; node "$S/harness/run_node.cjs" "$PN" parse 10000 > "$O/$v.parse.wasm"
  "$NT" ctype > "$O/$v.ctype.native"; node "$S/harness/run_node.cjs" "$PN" ctype > "$O/$v.ctype.wasm"
  for k in format parse ctype; do
    if cmp -s "$O/$v.$k.native" "$O/$v.$k.wasm"; then echo "PASS $v $k native==wasm ($(wc -l < "$O/$v.$k.native") lines)"
    else echo "FAIL $v $k: $(diff "$O/$v.$k.native" "$O/$v.$k.wasm" | grep -c '^<') lines differ"; TOTAL_FAIL=$((TOTAL_FAIL + 1)); echo "UNEXPECTED: $k"; fi
  done
  echo "== $v: Lua math functions, 100 000 inputs each, exact bits native vs wasm"
  for fn in sin cos tan asin acos atan atan2 exp log log2 log10 logb pow sqrt fmod floor cosh sinh tanh frexp ldexp fmodint; do
    "$NT" libm "$fn" 100000 > "$O/$v.libm.$fn.native"; node "$S/harness/run_node.cjs" "$PN" libm "$fn" 100000 > "$O/$v.libm.$fn.wasm"
    d=$(paste -d' ' "$O/$v.libm.$fn.native" "$O/$v.libm.$fn.wasm" | awk '$1!=$2' | wc -l)
    if [ "$d" -eq 0 ]; then echo "PASS $v math.$fn 100000/100000 identical"
    else
      l="FAIL $v math.$fn $d/100000 differ (native glibc vs wasm musl)"; echo "$l"
      if known "$v" "$l"; then KNOWN_FAIL=$((KNOWN_FAIL + 1)); else TOTAL_FAIL=$((TOTAL_FAIL + 1)); echo "UNEXPECTED: $l"; fi
    fi
  done
  echo "== $v: budget-edge probe (largest n per operation that fits the budget) native vs wasm"
  "$NT" budget-edge > "$O/$v.budget_edge.native"; node "$S/harness/run_node.cjs" "$PN" budget-edge > "$O/$v.budget_edge.wasm"
  if cmp -s "$O/$v.budget_edge.native" "$O/$v.budget_edge.wasm"; then echo "PASS $v budget-edge native==wasm ($(tr '\n' ' ' < "$O/$v.budget_edge.native"))"
  else echo "FAIL $v budget-edge: native [$(tr '\n' ' ' < "$O/$v.budget_edge.native")] wasm [$(tr '\n' ' ' < "$O/$v.budget_edge.wasm")]"; TOTAL_FAIL=$((TOTAL_FAIL + 1)); fi
  echo "== $v: stack / trap probes"
  echo "native: $("$NT" sp-after-errors 20)"
  echo "wasm-node: $(node "$S/harness/run_node.cjs" "$PN" sp-after-errors 20)"
  node "$S/harness/run_node.cjs" "$PN" case-stack > "$O/$v.case_stack.txt"
  echo "wasm-node shadow stack per case, top 3: $(sed -n 's/^\([a-z0-9_]*\) stack_used=\([0-9]*\).*/\2 \1/p' "$O/$v.case_stack.txt" | sort -nr | head -3 | tr '\n' ';')"
}
case $VAR in
  all) run_variant base; run_variant umath ;;
  base|umath) run_variant "$VAR" ;;
  *) echo "usage: $0 [all|base|umath]" >&2; exit 2 ;;
esac
echo "== $VAR: $TOTAL_FAIL unexpected failing line(s), $KNOWN_FAIL known-divergence FAIL line(s) (RESULTS §6)"
[ "$TOTAL_FAIL" -eq 0 ]
