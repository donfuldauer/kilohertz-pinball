#!/bin/sh
# web-spike-lua (S2): command-stream parity of the REAL Lua rules (pin_script's
# LuaScriptBackend over the vendored Lua 5.4.9 + KHZ patches) on wasm against
# native, for the three production tables and the CI fixture.
#
# usage: run_lua_parity.sh [all|base|umath]     (default: all)
#   base  : S2 patches without unified math (Lua math -> glibc natively, musl on wasm)
#   umath : + KHZ_LUA_MATH (Lua math -> Rust libm on both; the recommended build)
# env: SKIP_BUILD=1 reuse artifacts; SKIP_BROWSER=1 skip Chromium; SCENARIOS="..." to limit.
#
# Reference = S1's native recordings with the real Lua (S1/rec/<scn>.scr: per script step
# tick, RNG draws, digest of the switch edges + HostView seen, encoded command queue) and
# S1's native per-tick hashes (S1/native/<scn>.hashes: state_hash + ext_hash every tick).
# Every script step and every tick is compared. Prints one PASS/FAIL line per scenario and
# route; exit 0 only if every line is PASS.
set -eu
S=${WEB_SPIKES}/S2
. "$S/env.sh"
VAR=${1:-all}
ALL="replay_table_fixture10k apex_circuit_bot60 apex_circuit_pat60 apex_circuit_mb8 sky_deck_bot60 sky_deck_pat60 sky_deck_mb8 feature_lab_bot60 feature_lab_pat60 feature_lab_mb8 apex_circuit_bot600"
SCN=${SCENARIOS:-$ALL}
mkdir -p "$S/logs" "$S/runs/parity"
FAILS=0
bundle() { case $1 in replay_table_fixture10k) echo "$S1/bundles/replay_table.pinbundle" ;; *) echo "$S1/bundles/${1%_*}.pinbundle" ;; esac; }
check() { # check <label> <summary line>
  if echo "$2" | grep -q "steps=" && echo "$2" | grep -q " tick_mm=0 draw_mm=0 digest_mm=0 queue_mm=0 first_step_mm=none exhausted=0 " \
     && echo "$2" | grep -q " hash_mm=0 first_hash_mm=none "; then
    echo "PASS $1 $(echo "$2" | grep -o 'steps=[0-9]*') $(echo "$2" | grep -o 'queue_bytes=[0-9]*') $(echo "$2" | grep -o 'hash_lines=[0-9]*') $(echo "$2" | grep -o 'state_hash=[0-9a-f]*')"
  else
    echo "FAIL $1 $(echo "$2" | head -1 | cut -c1-300)"; FAILS=$((FAILS + 1))
  fi
}
run_variant() { # run_variant <base|umath>
  v=$1
  if [ "$v" = base ]; then NT=$CARGO_TARGET_DIR/release/s2; PN=$S/runs/pkg_node; PW=$S/runs/pkg_web; WD=$S/runs/web
  else NT=$CARGO_TARGET_DIR/umath/release/s2; PN=$S/runs/pkg_node_umath; PW=$S/runs/pkg_web_umath; WD=$S/runs/web_umath; fi
  if [ "${SKIP_BUILD:-0}" != 1 ]; then
    echo "== build ($v)"
    if [ "$v" = base ]; then "$S/harness/build.sh"; else "$S/harness/build_umath.sh"; fi
  fi
  wasm-tools print "$PN/s2_harness_bg.wasm" | grep -E '^\s*\(import' | sed 's/ (func.*//; s/ (tag.*//' > "$S/runs/parity/imports_$v.txt"
  echo "== $v: native x86_64 (real Lua) vs S1 native recordings"
  for s in $SCN; do
    b=$(bundle "$s")
    check "$v $s native" "$("$NT" play "$b" "$S1/rec/$s.scn" "$S1/rec/$s.scr" "$S1/native/$s.hashes")"
  done
  echo "== $v: wasm32-unknown-unknown + wasm-bindgen under Node (real Lua) vs S1 native recordings"
  for s in $SCN; do
    b=$(bundle "$s")
    check "$v $s wasm-node" "$(node "$S/harness/run_node.cjs" "$PN" play "$b" "$S1/rec/$s.scn" "$S1/rec/$s.scr" "$S1/native/$s.hashes")"
  done
  echo "== $v: fault injection (runtime / budget / memory / switch fault): native vs wasm, every tick"
  for t in apex_circuit sky_deck feature_lab; do
    s=${t}_bot60
    for i in runtime_tick budget_tick memory_tick runtime_switch; do
      h=$S/runs/parity/$v.$s.$i.native.hashes
      nl=$("$NT" play-inject "$S1/bundles/$t.pinbundle" "$S1/rec/$s.scn" "$S1/rec/$s.scr" "$S/cases/inject/$i.lua" "$h")
      wl=$(node "$S/harness/run_node.cjs" "$PN" play-inject "$S1/bundles/$t.pinbundle" "$S1/rec/$s.scn" "$S1/rec/$s.scr" "$h" "$S/cases/inject/$i.lua")
      nf=$(echo "$nl" | grep -o 'faults=[0-9]*'); wf=$(echo "$wl" | grep -o 'faults=[0-9]*')
      nsm=$(echo "$nl" | grep -o 'first_step_mm=[0-9a-z]*'); wsm=$(echo "$wl" | grep -o 'first_step_mm=[0-9a-z]*')
      if echo "$wl" | grep -q " hash_mm=0 first_hash_mm=none " && [ "$nf" = "$wf" ] && [ "$nf" = faults=1 ] && [ "$nsm" = "$wsm" ]; then
        echo "PASS $v $s inject=$i native==wasm every tick ($nf, stream diverges from the unfaulted recording at $nsm on both) $(echo "$wl" | grep -o 'state_hash=[0-9a-f]*')"
      else
        echo "FAIL $v $s inject=$i native: $nf $nsm wasm: $(echo "$wl" | cut -c1-200)"; FAILS=$((FAILS + 1))
      fi
    done
  done
  if [ "${SKIP_BROWSER:-0}" != 1 ]; then
    echo "== $v: wasm-bindgen in Playwright Chromium (bot60 + mb8 x 3 tables), COOP/COEP"
    "$S/harness/stage_web.sh" "$PW" "$WD"
    port=$((8750 + $(echo "$v" | wc -c)))
    node "$S/harness/chromium.cjs" "$WD" "$port" 1 "$S/runs/parity/chromium_$v.json" "" > "$S/logs/chromium_parity_$v.log" 2>&1 || true
    grep '^browser ' "$S/logs/chromium_parity_$v.log" || true
    for t in apex_circuit sky_deck feature_lab; do for k in bot60 mb8; do
      l=$(grep "^${t}_$k: " "$S/logs/chromium_parity_$v.log" | head -1 | sed "s/^${t}_$k: //")
      check "$v ${t}_$k chromium" "${l:-no result}"
    done; done
  fi
}
case $VAR in
  all) run_variant base; run_variant umath ;;
  base|umath) run_variant "$VAR" ;;
  *) echo "usage: $0 [all|base|umath]" >&2; exit 2 ;;
esac
echo "== imports of the wasm module (base): $(tr '\n' ';' < "$S/runs/parity/imports_base.txt" 2>/dev/null || echo n/a)"
echo "== $VAR: $FAILS failing line(s)"
[ "$FAILS" -eq 0 ]
