#!/bin/bash
# S10: one unit of work (one module x engine/tier x scenario/probe), with its
# comparison against the native reference. Writes one result line to
# runs/res/<id>.res:  STATUS|section|engine|tier|item|wall_s|maxrss_kb|detail
# usage: job.sh <kind> <args...>   (kinds below; engine "v8" = Node control)
set -u
W=${WEB_SPIKES}
S1=$W/S1; S4=$W/S4; S8=$W/S8; S10=$W/S10
H=$S10/harness; M=$S10/mods; R=$S10/runs; REF=$R/ref
mkdir -p "$R/res" "$R/out" "$R/manifest"
kind=$1; shift
PY="python3 -I"
# Lua module under test (default S8 v2; LUAMOD=s10_v2_legacy|s10_v2_exnref for the EH-encoding comparison)
LM=${LUAMOD:-s8_v2}; LSEC=${LUASEC:-lua}

res() { # status section engine tier item wall rss detail
  echo "$1|$2|$3|$4|$5|$6|$7|$8" > "$R/res/$2__$3__$4__$(echo "$5" | tr '/ ' '__').res"
}
run() { # outfile cmd...   -> sets WALL RSS RC
  local o=$1; shift
  set -- $($PY "$H/rss.py" "$o" "$@")
  WALL=$1; RSS=$2; RC=$3
}
engcmd() { # engine tier runner -> prints the command prefix
  if [ "$1" = v8 ]; then echo "node"; else echo "$H/eng.sh $1 $2 $3"; fi
}

case "$kind" in
sim) # sim <module> <engine> <tier> <scenario>
  mod=$1 e=$2 t=$3 s=$4; tb=${s%_*}
  d=$R/out/sim/$mod/${e}_$t; mkdir -p "$d"; h=$d/$s.hashes; log=$d/$s.log
  if [ "$e" = v8 ]; then
    run "$log" node "$S1/harness/run_bindgen.mjs" "$M/$mod" play "$S1/bundles/$tb.pinbundle" "$S1/rec/$s.scn" "$S1/rec/$s.scr" "$h"
  else
    run "$log" $H/eng.sh "$e" "$t" "$H/run_s1.js" "$M/$mod" play "$S1/bundles/$tb.pinbundle" "$S1/rec/$s.scn" "$S1/rec/$s.scr" "$h"
  fi
  line=$($PY "$H/compare.py" "$mod $e/$t $s" "$S1/native/$s.hashes" "$h" 2>&1)
  st=${line%% *}; [ -s "$h" ] || st=FAIL
  [ -f "$h" ] && echo "$(sha256sum < "$h" | cut -c1-64) $mod/${e}_$t/$s.hashes" >> "$R/manifest/sim_$mod.txt"
  summ=$(grep -a -o 'state_hash=[0-9a-f]*\|wall_ms=[0-9]*\|load_ms=[0-9.]*\|TRAP.*\|Error.*' "$log" | tr '\n' ' ')
  res "$st" sim_$mod "$e" "$t" "$s" "$WALL" "$RSS" "rc=$RC ${line#* } | $summ"
  [ "$st" = PASS ] && rm -f "$h"
  ;;
lua_tick) # lua_tick <engine> <tier> <scenario> <cap|off>
  e=$1 t=$2 s=$3 c=$4; tb=${s%_*}
  d=$R/out/$LSEC/${e}_$t; mkdir -p "$d"; o=$d/tick_${s}_$c.txt
  a="@$S1/bundles/$tb.pinbundle @$S1/rec/$s.scn @$S1/rec/$s.scr @$S1/native/$s.hashes $c"
  if [ "$e" = v8 ]; then run "$o" node "$S8/node/run.cjs" "$M/$LM/pkg_node" tickstats $a
  else run "$o" $H/eng.sh "$e" "$t" "$H/run_s8.js" "$M/$LM/pkg_node" s8 tickstats $a; fi
  ok=$($PY - "$o" <<'PY'
import re, sys
s = open(sys.argv[1]).read()
g = lambda k: (re.search(r'\b' + k + r'=([^ ]+)', s) or [None, 'x'])[1]
try:
    good = int(g('queue_mm')) + int(g('digest_mm')) + int(g('draw_mm')) + int(g('hash_mm')) == 0 and g('faults') == '0' and int(g('hash_lines')) > 0
except ValueError:
    good = False
print('ok' if good else 'bad')
PY
)
  same=$(cmp -s "$o" "$REF/lua/native/tick_${s}_$c.txt" && echo same || echo differs)
  st=FAIL; [ "$ok" = ok ] && [ "$same" = same ] && st=PASS
  res "$st" $LSEC "$e" "$t" "tick $s cap=$c" "$WALL" "$RSS" "rc=$RC internal_mismatches=$ok text_vs_native=$same $(grep -a -o 'state_hash=[0-9a-f]*\|units_per_tick max=[0-9]*\|TRAP.*' "$o" | tr '\n' ' ')"
  ;;
lua_probe) # lua_probe <engine> <tier> <probe>
  e=$1 t=$2 n=$3
  case $n in a_unpack_engine) cfg="probe engine-bigbudget 1" ;; b_gc_engine) cfg="probe engine 3000" ;;
    b_memstep_engine) cfg="memtrace engine 400" ;; pairs_order) cfg="pairs" ;; *) cfg="probe big 1" ;; esac
  set -- $cfg; c1=$1 c2=${2:-} c3=${3:-}
  d=$R/out/$LSEC/${e}_$t; mkdir -p "$d"; o=$d/probe_$n.txt
  if [ "$n" = pairs_order ]; then src=$S8/probes/pairs/order.lua; c2=4; c3=""; else src=$REF/lua/src/$n.lua; fi
  if [ "$e" = v8 ]; then run "$o" node "$S8/node/run.cjs" "$M/$LM/pkg_node" $c1 "@$src" $c2 $c3
  else  # the probe text can hold NUL bytes: written to the file by the runner (--out), stdout kept as .log
    rm -f "$o"; run "$o.log" $H/eng.sh "$e" "$t" "$H/run_s8.js" "--out=$o" "$M/$LM/pkg_node" s8 $c1 "@$src" $c2 $c3
    [ -s "$o" ] || cp "$o.log" "$o"; fi
  r=$REF/lua/native/probe_$n.txt
  obs() { grep -a -v -E '^(HEAP|mem|REAL) ' "$1" || true; }
  heap() { grep -a -E '^(HEAP|mem) ' "$1" || true; }
  so=$(cmp -s <(obs "$o") <(obs "$r") && echo same || echo differs)
  sh=$(cmp -s <(heap "$o") <(heap "$r") && echo same || echo differs)
  dl=$(diff -a <(obs "$r") <(obs "$o") | grep -a -c '^<' || true)
  st=FAIL; [ "$so" = same ] && [ "$sh" = same ] && st=PASS
  res "$st" $LSEC "$e" "$t" "probe $n" "$WALL" "$RSS" "rc=$RC lines=$(wc -l < "$o") observable_vs_native=$so heap_vs_native=$sh differing_lines=$dl $(grep -a -o 'TRAP.*' "$o" | head -1)"
  ;;
lua_sandbox) # lua_sandbox <engine> <tier>
  e=$1 t=$2; d=$R/out/$LSEC/${e}_$t; mkdir -p "$d"; o=$d/sandbox.txt
  if [ "$e" = v8 ]; then run "$o" node "$S8/harness/run_node.cjs" "$M/$LM/pkg_node" sandbox
  else run "$o" $H/eng.sh "$e" "$t" "$H/run_s8.js" "$M/$LM/pkg_node" sandbox; fi
  strip() { sed 's/ | mem=[0-9]*$//' "$1"; }
  SREF=${LUAREF_SANDBOX:-$REF/lua/native/sandbox.txt}   # the KHZ 38 candidate is compared with its own V8 output
  n=$(wc -l < "$SREF")
  same=$(paste -d'\n' <(strip "$SREF") <(strip "$o") | awk 'NR%2==1{a=$0;next} a==$0{c++} END{print c+0}')
  vsnode=$(cmp -s "$o" "$REF/lua/v8/sandbox.txt" && echo same || echo differs)
  st=FAIL; [ "$same" = "$n" ] && [ "$(wc -l < "$o")" = "$n" ] && st=PASS
  res "$st" $LSEC "$e" "$t" "sandbox corpus" "$WALL" "$RSS" "rc=$RC cases_identical_to_$(basename "$(dirname "$SREF")")=$same/$n (mem= field stripped) full_lines_vs_node=$vsnode traps=$(grep -a -c 'TRAP' "$o")"
  ;;
lua_budget) # lua_budget <engine> <tier>
  e=$1 t=$2; d=$R/out/$LSEC/${e}_$t; mkdir -p "$d"; o=$d/budget_edge.txt
  if [ "$e" = v8 ]; then run "$o" node "$S8/harness/run_node.cjs" "$M/$LM/pkg_node" budget-edge
  else run "$o" $H/eng.sh "$e" "$t" "$H/run_s8.js" "$M/$LM/pkg_node" budget-edge; fi
  st=$(cmp -s "$o" "$REF/lua/native/budget_edge.txt" && echo PASS || echo FAIL)
  res "$st" $LSEC "$e" "$t" "budget-edge" "$WALL" "$RSS" "rc=$RC $(tr '\n' ' ' < "$o")"
  ;;
lua_adv) # lua_adv <engine> <tier> <mix> <edges> <cap> <reps>
  e=$1 t=$2 mx=$3 ed=$4 c=$5 rp=$6; d=$R/out/$LSEC/${e}_$t; mkdir -p "$d"; o=$d/adv_${mx}_${ed}_$c.txt
  if [ "$e" = v8 ]; then run "$o" node "$S8/node/run.cjs" "$M/$LM/pkg_node" advtick "$mx" "$ed" "$c" "$rp"
  else run "$o" $H/eng.sh "$e" "$t" "$H/run_s8.js" "$M/$LM/pkg_node" s8 advtick "$mx" "$ed" "$c" "$rp"; fi
  a=$(head -1 "$o"); b=$(head -1 "$REF/lua/native/adv_${mx}_${ed}_$c.txt")
  st=FAIL; [ -n "$a" ] && [ "$a" = "$b" ] && st=PASS
  res "$st" $LSEC "$e" "$t" "advtick $mx edges=$ed cap=$c" "$WALL" "$RSS" "rc=$RC $(echo "$a" | cut -c1-200) | $(grep -a -o 'median=[0-9.]*' "$o" | head -1)"
  ;;
rules_play) # rules_play <engine> <tier> <design> <scenario>
  e=$1 t=$2 dz=$3 s=$4; tb=${s%_*}
  d=$R/out/rules/${e}_$t; mkdir -p "$d"; h=$d/${dz}_$s.hashes; log=$d/${dz}_$s.log
  if [ "$e" = v8 ]; then run "$log" env S4_PKG=$M/s4_pkg node "$S4/node/run_web.mjs" play "$dz" "$S4/bundles/${tb}_wasm.pinbundle" "$S1/rec/$s.scn" "$h"
  else run "$log" $H/eng.sh "$e" "$t" "$H/run_s4.js" "$M/s4_pkg" "$M/s4_khz_rules.js" play "$dz" "$S4/bundles/${tb}_wasm.pinbundle" "$S1/rec/$s.scn" "$h"; fi
  st=FAIL; at=""
  if [ -s "$h" ] && cmp -s "$h" "$S4/out/native/$s.engine.hashes"; then st=PASS; else at=$(cmp "$h" "$S4/out/native/$s.engine.hashes" 2>&1 | head -1); fi
  res "$st" rules "$e" "$t" "play $dz $s" "$WALL" "$RSS" "rc=$RC vs S4 native engine hashes (tick state ext cmd): ${at:-identical} | $(grep -a -o 'state_hash=[0-9a-f]*\|cmd_stream=[0-9a-f]*\|fuel_digest=[0-9a-f]*\|max_fuel=[0-9]*\|load_ms=[0-9.]*\|ERROR.*' "$log" | tr '\n' ' ')"
  [ "$st" = PASS ] && rm -f "$h"
  ;;
rules_adv) # rules_adv <engine> <tier> <design>
  e=$1 t=$2 dz=$3; d=$R/out/rules/${e}_$t; mkdir -p "$d"; o=$d/adv_$dz.txt
  P=$S4/repo/tables/apex_circuit/rules.wasm
  if [ "$e" = v8 ]; then run "$o" env S4_PKG=$M/s4_pkg node "$S4/node/run_web.mjs" adv "$dz" "$P"
  else  # error texts in the corpus echo module bytes incl. NUL: written to the file by the runner (--out)
    rm -f "$o"; run "$o.log" $H/eng.sh "$e" "$t" "$H/run_s4.js" "--out=$o" "$M/s4_pkg" "$M/s4_khz_rules.js" adv "$dz" "$P"
    [ -s "$o" ] || cp "$o.log" "$o"; fi
  c=$($PY "$S4/harness/adv_compare.py" "$S4/out/adv/native.native-instr-depth.txt" "$o" 2>&1)
  head=$(echo "$c" | head -1 | sed 's/  */ /g'); rest=$(echo "$c" | tail -n +3 | sed 's/  */ /g' | tr '\n' ';' | cut -c1-600)
  st=FAIL; echo "$head" | grep -q 'EXACT=82 CLASS=0 DIFF=0 MISSING=0' && st=PASS
  res "$st" rules "$e" "$t" "adversarial $dz" "$WALL" "$RSS" "rc=$RC vs native wasmtime+instr+depth: $head $rest"
  ;;
rules_stack) # rules_stack <engine> <tier> <design> <reps>
  e=$1 t=$2 dz=$3 rp=$4; d=$R/out/rules/${e}_$t; mkdir -p "$d"; o=$d/stack_$dz.txt
  if [ "$e" = v8 ]; then run "$o" env S4_PKG=$M/s4_pkg node "$S4/node/run_stack.mjs" "$dz" "$rp"
  else run "$o" $H/eng.sh "$e" "$t" "$H/run_s4.js" "$M/s4_pkg" "$M/s4_khz_rules.js" stack "$dz" "$rp"; fi
  exp=$(grep "^native-instr-depth stack" "$S4/out/stack_all.txt" | head -5 | sed 's/^native-instr-depth //; s/\[\([0-9]*\), [0-9]*\]/\1/')
  got=$(grep -a ' stack locals=' "$o" | sed 's/^[^ ]* //; s/\[\([0-9]*\)\(, [0-9]*\)*\]/\1/')
  if [ "$dz" = jit ]; then st=INFO; else st=$([ "$got" = "$exp" ] && echo PASS || echo FAIL); fi
  res "$st" rules "$e" "$t" "stack $dz" "$WALL" "$RSS" "rc=$RC $(grep -a ' stack locals=' "$o" | sed 's/^[^ ]* stack //' | tr '\n' ';') $(grep -a -o 'ERROR.*' "$o" | head -1)"
  ;;
rules_cap) # rules_cap <engine> <tier> <scenario>   (S8 copy of the S4 module, per-tick cap 200000)
  e=$1 t=$2 s=$3; tb=${s%_*}
  d=$R/out/rules/${e}_$t; mkdir -p "$d"; h=$d/cap_$s.hashes; log=$d/cap_$s.log
  if [ "$e" = v8 ]; then run "$log" env S4_PKG=$M/s8_s4/pkg node "$S8/node/s4/run_s4.mjs" 200000 play wasmi-instr-depth "$S4/bundles/${tb}_wasm.pinbundle" "$S1/rec/$s.scn" "$h"
  else run "$log" $H/eng.sh "$e" "$t" "$H/run_s4.js" "$M/s8_s4/pkg" "$M/s4_khz_rules.js" capplay 200000 wasmi-instr-depth "$S4/bundles/${tb}_wasm.pinbundle" "$S1/rec/$s.scn" "$h"; fi
  a=$(cut -d' ' -f1-3 "$h" | cmp -s - "$S1/native/$s.hashes" && echo same || echo differs)
  b=$(cmp -s "$log" "$REF/rules/native_cap_$s.txt" && echo same || echo differs)
  st=FAIL; [ "$a" = same ] && [ "$b" = same ] && st=PASS
  res "$st" rules "$e" "$t" "tick-cap 200000 $s" "$WALL" "$RSS" "rc=$RC state/ext vs S1 native=$a summary_vs_native=$b | $(grep -a -o 's8_fuel_per_tick max=[0-9]*\|faulted=[a-z]*\|state_hash=[0-9a-f]*' "$log" | tr '\n' ' ')"
  rm -f "$h"
  ;;
rules_s8adv) # rules_s8adv <engine> <tier> <mix> <edges> <cap>
  e=$1 t=$2 mx=$3 ed=$4 c=$5; d=$R/out/rules/${e}_$t; mkdir -p "$d"; o=$d/s8adv_${mx}_${ed}_$c.txt
  if [ "$e" = v8 ]; then run "$o" env S4_PKG=$M/s8_s4/pkg node "$S8/node/s4/run_s4.mjs" "$c" s8adv wasmi-instr-depth "$mx" "$ed" 1
  else run "$o" $H/eng.sh "$e" "$t" "$H/run_s4.js" "$M/s8_s4/pkg" "$M/s4_khz_rules.js" s8adv "$c" wasmi-instr-depth "$mx" "$ed" 1; fi
  a=$(head -1 "$o"); b=$(head -1 "$REF/rules/native_s8adv_${mx}_${ed}_$c.txt")
  st=FAIL; [ -n "$a" ] && [ "$a" = "$b" ] && st=PASS
  res "$st" rules "$e" "$t" "adv-tick $mx edges=$ed cap=$c" "$WALL" "$RSS" "rc=$RC $(echo "$a" | cut -c1-160)"
  ;;
lua_sp) # lua_sp <engine> <tier> <n>   (S2 G14: shadow-stack pointer delta after n pcall errors + faults)
  e=$1 t=$2 n=$3; d=$R/out/$LSEC/${e}_$t; mkdir -p "$d"; o=$d/sp_after_errors.txt
  if [ "$e" = v8 ]; then run "$o" node "$S8/harness/run_node.cjs" "$M/$LM/pkg_node" sp-after-errors "$n"
  else run "$o" $H/eng.sh "$e" "$t" "$H/run_s8.js" "$M/$LM/pkg_node" sp-after-errors "$n"; fi
  st=FAIL; grep -a -q -E 'delta=0\b|delta 0\b|sp_delta=0\b' "$o" && ! grep -a -q TRAP "$o" && st=PASS
  res "$st" $LSEC "$e" "$t" "sp-after-errors $n" "$WALL" "$RSS" "rc=$RC $(tr '\n' ' ' < "$o" | cut -c1-300)"
  ;;
probe) # probe <module> <engine> <tier> <name>   (S1/S7 op-level probes of the real module code)
  mod=$1 e=$2 t=$3 n=$4; d=$R/out/probe/$mod/${e}_$t; mkdir -p "$d"; o=$d/$n.txt
  if [ "$e" = v8 ]; then run "$o" node "$S1/harness/run_bindgen.mjs" "$M/$mod" probe "$n"
  else run "$o" $H/eng.sh "$e" "$t" "$H/run_s1.js" "$M/$mod" probe "$n"; fi
  ref=$REF/probe/${mod}__$n.txt
  if cmp -s "$o" "$ref"; then st=PASS; dd="identical to V8 (Node) output"
  else
    dd="differs from V8: $($PY - "$ref" "$o" <<'PY'
import sys
a = dict(x.split('=', 1) for x in open(sys.argv[1]).read().split()[1:] if '=' in x)
b = dict(x.split('=', 1) for x in open(sys.argv[2]).read().split()[1:] if '=' in x)
print(' '.join(f'{k}:{a.get(k)}->{b.get(k)}' for k in a if a.get(k) != b.get(k)) or open(sys.argv[2]).read()[:200])
PY
)"
    # NaN-bit probes are informative (sign/payload of a NaN is nondeterministic in the Wasm spec)
    case $n in nan_bits|fp_ties) st=INFO ;; *) st=FAIL ;; esac
  fi
  res "$st" probe_$mod "$e" "$t" "$n" "$WALL" "$RSS" "rc=$RC $dd"
  ;;
lua_dump) # lua_dump <engine> <tier> <format|parse|ctype|libm> [fn]   (S2 run_lua_sandbox.sh number/libm/ctype dumps)
  e=$1 t=$2 k=$3 fn=${4:-}; d=$R/out/$LSEC/${e}_$t; mkdir -p "$d"
  case $k in format|parse) a="$k 10000"; nm=$k ;; ctype) a=ctype; nm=ctype ;; libm) a="libm $fn 100000"; nm=libm_$fn ;; esac
  o=$d/dump_$nm.txt
  if [ "$e" = v8 ]; then run "$o" node "$S8/harness/run_node.cjs" "$M/$LM/pkg_node" $a
  else run "$o" $H/eng.sh "$e" "$t" "$H/run_s8.js" "$M/$LM/pkg_node" $a; fi
  r=$REF/lua/native/dump_$nm.txt
  if cmp -s "$o" "$r"; then st=PASS; dd="identical to native ($(wc -l < "$r") lines)"; else st=FAIL; dd="$(diff "$r" "$o" | grep -c '^<') of $(wc -l < "$r") lines differ; first: $(diff "$r" "$o" | grep -m1 '^[<>]' | cut -c1-120)"; fi
  res "$st" $LSEC "$e" "$t" "dump $nm" "$WALL" "$RSS" "rc=$RC $dd"
  ;;
micro) # micro <engine> <tier>
  e=$1 t=$2; d=$R/micro; mkdir -p "$d"; o=$d/${e}_$t.bin
  if [ "$e" = v8 ]; then run "$d/${e}_$t.log" node "$S10/micro/run_micro.js" "$S10/micro" "$o"
  else run "$d/${e}_$t.log" $H/eng.sh "$e" "$t" "$S10/micro/run_micro.js" "$S10/micro" "$o"; fi
  c=$($PY "$S10/micro/compare_micro.py" "$S10/micro" "${e}_$t=$o" 2>&1 | grep -E '^\| '"${e}_$t"' |^== ')
  st=FAIL; echo "$c" | grep -q '== micro: 0 ' && st=PASS
  res "$st" micro "$e" "$t" "115 ops x 6144" "$WALL" "$RSS" "rc=$RC $(echo "$c" | head -1 | cut -d'|' -f4-7 | tr -s ' ') $(grep -a TIER-INSTABILITY "$d/${e}_$t.log" | tr '\n' ' ')"
  ;;
*) echo "job.sh: unknown kind $kind" >&2; exit 2 ;;
esac
