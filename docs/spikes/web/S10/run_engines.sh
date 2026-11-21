#!/bin/bash
# S10 web-spike-engines: JavaScriptCore and SpiderMonkey parity of the wasm modules.
#
# usage: run_engines.sh [all|fetch|verify|refs|jobs|summary]   (default: all)
# env:   PAR=<parallel jobs> (default 2)   ARM=0 to skip the aarch64 SpiderMonkey shells (qemu)
#        ONLY="sim lua rules micro" to restrict the job sections
#        ENGINES="jsc:default ..." to override the x86_64 engine/tier list
#        KEEP=1 with "jobs": keep runs/res and skip jobs that already have a result line (resume)
#
# 1. fetch:  download (or reuse) the shells, verify sha256 against pinned values and
#            the upstream checksum files, extract each into its own new directory.
# 2. verify: sha256 of the module copies in S10/mods (the bytes the V8 runs used).
# 3. refs:   native x86_64 references (harness/make_refs.sh).
# 4. jobs:   every (module x engine/tier x scenario/probe) unit through harness/job.sh,
#            each compared with the native reference -> runs/res/*.res
#              2a sim:   S1 +simd128 module, S7 portable +simd128, S7 portable no-simd;
#                        11 scenarios per tier, per-tick state_hash and ext_hash vs S1 native
#              2b lua:   S8 v2 (KHZ 29-37): tickstats (bot60/mb8 x 3 tables + fixture, cap off and
#                        200000), 13 audit probes + pairs order, S2 sandbox corpus, budget-edge,
#                        adversarial ticks
#              2c rules: S4 wasmi-instr-depth (bot60 x 3, adversarial corpus, depth counter),
#                        jit-depth on the engine itself, S8 per-tick cap (Wasm rules)
#              probe:    S1/S7 op-level probes (glam_ops, glam_iso, quat_ops_iso; NaN-bit probes as info)
#              eh:       the v2 Lua module rebuilt with legacy EH and with exnref (lua_eh_*)
#              fix:      KHZ 38 candidate (LUAI_MAXCCALLS 100) replays + corpus (lua_ccalls100), if built
#              micro:    115-op float micro-test vs native reference
#            plus the same jobs on Node (V8 control, the unchanged S1/S4/S8 Node harnesses).
# 5. summary: PASS/FAIL tables -> runs/summary.md; exit 0 iff no FAIL.
set -u
S10=${WEB_SPIKES}/S10
H=$S10/harness; R=$S10/runs; DL=$S10/dl; T=$S10/tools
PAR=${PAR:-2}; ARM=${ARM:-1}
WHAT=${1:-all}
mkdir -p "$R" "$S10/logs"

fetch_one() { # name url sha256 zipname tooldir
  local name=$1 url=$2 sha=$3 zip=$4 tdir=$5
  mkdir -p "$DL/$name"
  if [ ! -f "$DL/$name/$zip" ] || [ "$(sha256sum < "$DL/$name/$zip" | cut -c1-64)" != "$sha" ]; then
    echo "fetch $name: $url"
    curl -sS --fail --max-time 600 -o "$DL/$name/$zip.part" "$url" || { echo "FETCH-FAILED $name $url"; return 1; }
    mv "$DL/$name/$zip.part" "$DL/$name/$zip"
  fi
  local got; got=$(sha256sum < "$DL/$name/$zip" | cut -c1-64)
  if [ "$got" != "$sha" ]; then echo "SHA256-MISMATCH $name $got != $sha"; return 1; fi
  echo "sha256 OK $name/$zip $got"
  if [ ! -d "$T/$tdir" ]; then mkdir -p "$T/$tdir" && unzip -q "$DL/$name/$zip" -d "$T/$tdir"; fi
}
fetch() {
  # JSC: WebKitGTK JSC CLI bundle (trunk 327409@main); upstream checksum file 327409@main.sha256sum
  fetch_one jsc_327409 'https://webkitgtk.org/jsc-built-products/x86_64/release/327409@main.zip' \
    c21a415deebd6e3eb867037d4ca290b9dd21bec12cc9c71f13403da7d8d042f0 327409@main.zip jsc_327409 || return 1
  # SpiderMonkey jsshell: Firefox 160.0.1 (release) and 153.7.0esr; upstream SHA256SUMS in the release dir
  local B=https://archive.mozilla.org/pub/firefox/releases
  fetch_one sm_160.0.1 "$B/160.0.1/jsshell/jsshell-linux-x86_64.zip" b547d9ed0bc564ba4387a32cc09aa7aa490c4bdc32998a807c38d36a8078ad69 jsshell-linux-x86_64.zip sm_160.0.1_x64 || return 1
  fetch_one sm_160.0.1 "$B/160.0.1/jsshell/jsshell-linux-aarch64.zip" 1c2bd0e53de20a391f0ae9f9163f79479c441fe6c464a45ebe3244c7080a4ddb jsshell-linux-aarch64.zip sm_160.0.1_arm64 || return 1
  fetch_one sm_153.7.0esr "$B/153.7.0esr/jsshell/jsshell-linux-x86_64.zip" b453f9a0cf38a7d5087d51710b9d260effff11d09b7a89282897de7f06440f5c jsshell-linux-x86_64.zip sm_153.7.0esr_x64 || return 1
  fetch_one sm_153.7.0esr "$B/153.7.0esr/jsshell/jsshell-linux-aarch64.zip" b0731ae3f67000a0f8a3c3126de0126274b1aa33dfee4e20055d41b5192612a2 jsshell-linux-aarch64.zip sm_153.7.0esr_arm64 || return 1
  # cross-check against the upstream checksum files kept next to the archives
  (cd "$DL/jsc_327409" && sha256sum -c 327409@main.sha256sum) || return 1
  for v in 160.0.1 153.7.0esr; do
    (cd "$DL/sm_$v" && grep -E 'jsshell-linux-(x86_64|aarch64).zip' SHA256SUMS | sed 's#jsshell/##' | sha256sum -c -) || return 1
  done
  echo "jsc: $("$T/jsc_327409/jsc" -e 'print(1)' >/dev/null && sed -n 's/.*WebKit Revision: //p' "$T/jsc_327409/README.txt")"
  echo "sm160: $("$T/sm_160.0.1_x64/js" --help | sed -n 's/^Version: //p')  sm153: $("$T/sm_153.7.0esr_x64/js" --help | sed -n 's/^Version: //p')"
}
verify() {
  (cd "$S10/mods" && sha256sum -c "$S10/mods.sha256") || return 1
}
jobs() {
  local X=${ENGINES:-"jsc:default jsc:ipint jsc:bbq jsc:omg jsc:nojit sm160:default sm160:baseline sm160:ion sm153:default sm153:baseline sm153:ion"}
  local A=""; [ "$ARM" = 1 ] && A="sm160a:default sm160a:baseline sm160a:ion sm153a:default"
  local ONLY_=${ONLY:-"sim probe lua eh fix rules micro"}
  local L=$R/joblist${ONLY:+_$(echo "$ONLY" | tr " " "_")}.txt; : > "$L"
  local SC="replay_table_fixture10k apex_circuit_bot60 apex_circuit_pat60 apex_circuit_mb8 sky_deck_bot60 sky_deck_pat60 sky_deck_mb8 feature_lab_bot60 feature_lab_pat60 feature_lab_mb8 apex_circuit_bot600"
  local SC10=${SC% apex_circuit_bot600}
  local SC7="replay_table_fixture10k apex_circuit_bot60 apex_circuit_mb8 sky_deck_bot60 sky_deck_mb8 feature_lab_bot60 feature_lab_mb8"
  local PROBES="a_limits a_unpack_engine b_gc_engine b_memstep_engine c_tostring d_format e_numbers f_length g_pairs h_sort i_misc j_api k_strcache nan_sign pairs_order"
  local ALL="v8:default $X $A"
  # Job plan per engine class (the wasm interpreters and qemu-emulated shells are 30-60x slower):
  #   F (V8, JSC default/bbq/omg, SM x86 all tiers): everything.
  #   I (JSC ipint = in-place interpreter): everything except the uncapped adversarial ticks
  #     (513 callbacks x 100 000 units) and the uncapped Wasm adversarial ticks; the 600 000-tick
  #     soak for the S1 module only (the S7 modules run the other 10 scenarios).
  #   N (JSC nojit: the same wasm interpreter with the JS JITs off, = Safari without JIT):
  #     S1 module 10 scenarios, Lua corpus/probes/one replay, rules corpus/stack/one replay.
  #   AD (aarch64 SM default tier, qemu): S1 module 11 scenarios, S7 modules 7 scenarios, Lua and
  #     rules as F but capped adversarial ticks only.   AO (aarch64 SM baseline/ion): subsets.
  for et in $ALL; do
    e=${et%:*}; t=${et#*:}
    case "$e:$t" in jsc:ipint) C=I ;; jsc:nojit) C=N ;; sm1*a:default) C=AD ;; sm1*a:*) C=AO ;; *) C=F ;; esac
    case " $ONLY_ " in *" micro "*) echo "$H/job.sh micro $e $t" >> "$L" ;; esac
    case " $ONLY_ " in *" sim "*)
      case $C in F) S1S=$SC; S7S=$SC ;; I) S1S=$SC; S7S=$SC10 ;; N) S1S=$SC10; S7S="" ;; AD) S1S=$SC; S7S=$SC7 ;; AO) S1S=$SC10; S7S="replay_table_fixture10k apex_circuit_bot60 sky_deck_bot60 feature_lab_bot60" ;; esac
      for s in $S1S; do echo "$H/job.sh sim s1_simd $e $t $s" >> "$L"; done
      for mod in s7_portable_simd s7_portable_nosimd; do for s in $S7S; do echo "$H/job.sh sim $mod $e $t $s" >> "$L"; done; done ;; esac
    case " $ONLY_ " in *" probe "*)
      for mp in s1_simd:glam_ops s1_simd:nan_bits s7_portable_simd:glam_ops s7_portable_simd:glam_iso s7_portable_simd:quat_ops_iso s7_portable_simd:fp_ties s7_portable_simd:nan_bits s7_portable_nosimd:glam_iso s7_portable_nosimd:quat_ops_iso s7_portable_nosimd:fp_ties; do
        echo "$H/job.sh probe ${mp%%:*} $e $t ${mp#*:}" >> "$L"; done ;; esac
    case " $ONLY_ " in *" lua "*)
      case $C in F|I) TK="$SC7"; CAPS="off 200000" ;; AD) TK="$SC7"; CAPS="off" ;; *) TK="apex_circuit_bot60"; CAPS="off" ;; esac
      for s in $TK; do for c in $CAPS; do echo "$H/job.sh lua_tick $e $t $s $c" >> "$L"; done; done
      [ "$C" = AD ] && echo "$H/job.sh lua_tick $e $t apex_circuit_bot60 200000" >> "$L"
      for p in $PROBES; do echo "$H/job.sh lua_probe $e $t $p" >> "$L"; done
      echo "$H/job.sh lua_sandbox $e $t" >> "$L"; echo "$H/job.sh lua_budget $e $t" >> "$L"; echo "$H/job.sh lua_sp $e $t 20" >> "$L"
      case $C in F|I|AD) LD="format parse ctype" ;; *) LD="format ctype" ;; esac
      for k in $LD; do echo "$H/job.sh lua_dump $e $t $k" >> "$L"; done
      case $C in F|I) for fn in sin cos tan asin acos atan atan2 exp log log2 log10 logb pow sqrt fmod floor cosh sinh tanh frexp ldexp fmodint; do echo "$H/job.sh lua_dump $e $t libm $fn" >> "$L"; done ;;
        *) for fn in sin exp pow; do echo "$H/job.sh lua_dump $e $t libm $fn" >> "$L"; done ;; esac
      case $C in
        F) for mix in loop alloc strings ccalls errors pattern; do for ed in 0 512; do echo "$H/job.sh lua_adv $e $t $mix $ed 200000 1" >> "$L"; done
             echo "$H/job.sh lua_adv $e $t $mix 512 off 1" >> "$L"; done ;;
        I|AD) for mix in loop alloc strings ccalls errors pattern; do echo "$H/job.sh lua_adv $e $t $mix 512 200000 1" >> "$L"; done ;;
        *) echo "$H/job.sh lua_adv $e $t alloc 512 200000 1" >> "$L" ;;
      esac ;; esac
    case " $ONLY_ " in *" eh "*)
      # EH encoding comparison: the same v2 Lua module rebuilt here with legacy EH and with exnref
      # (harness/build_eh_variants.sh; code identical to S8's v2 except embedded source paths)
      case "$C:$e" in F:*|I:*|AD:sm160a)
        for v in legacy exnref; do
          P="env LUAMOD=s10_v2_$v LUASEC=lua_eh_$v $H/job.sh"
          echo "$P lua_sandbox $e $t" >> "$L"; echo "$P lua_budget $e $t" >> "$L"; echo "$P lua_sp $e $t 20" >> "$L"
          case $C in F) EHT="apex_circuit_bot60 apex_circuit_mb8 sky_deck_bot60 sky_deck_mb8 feature_lab_bot60 feature_lab_mb8" ;; *) EHT="apex_circuit_bot60 sky_deck_mb8" ;; esac
          for s in $EHT; do echo "$P lua_tick $e $t $s off" >> "$L"; done
          for p in a_limits b_gc_engine c_tostring d_format e_numbers g_pairs i_misc j_api k_strcache nan_sign; do echo "$P lua_probe $e $t $p" >> "$L"; done
          echo "$P lua_adv $e $t errors 512 200000 1" >> "$L"; echo "$P lua_adv $e $t pattern 512 200000 1" >> "$L"
        done ;; esac ;; esac
    case " $ONLY_ " in *" fix "*)
      # KHZ 38 candidate (Lua C-call limit 100; harness/build_ccalls_variant.sh): production replays per step and
      # per tick vs S1 native, and the sandbox corpus identical to its own V8 output, on the fast tiers
      if [ -f "$S10/mods/s10_v2_ccalls100/pkg_node/s2_harness_bg.wasm" ]; then case "$e:$t" in v8:default|jsc:default|jsc:bbq|jsc:omg|sm160:default|sm160:baseline|sm153:default)
        P="env LUAMOD=s10_v2_ccalls100 LUASEC=lua_ccalls100 $H/job.sh"
        for s in $SC7; do echo "$P lua_tick $e $t $s off" >> "$L"; done
        echo "env LUAREF_SANDBOX=$R/ref/lua/v8_ccalls100/sandbox.txt $P lua_sandbox $e $t" >> "$L" ;; esac; fi ;; esac
    case " $ONLY_ " in *" rules "*)
      case $C in F|I|AD) RS="apex_circuit_bot60 sky_deck_bot60 feature_lab_bot60" ;; *) RS="apex_circuit_bot60" ;; esac
      for s in $RS; do
        echo "$H/job.sh rules_play $e $t wasmi-instr-depth $s" >> "$L"
        case $C in N) ;; *) echo "$H/job.sh rules_play $e $t jit-depth $s" >> "$L" ;; esac
        case $C in F|I) echo "$H/job.sh rules_cap $e $t $s" >> "$L" ;; esac
      done
      [ "$C" = AD ] && echo "$H/job.sh rules_cap $e $t apex_circuit_bot60" >> "$L"
      echo "$H/job.sh rules_adv $e $t wasmi-instr-depth" >> "$L"
      echo "$H/job.sh rules_adv $e $t jit-depth" >> "$L"
      echo "$H/job.sh rules_stack $e $t wasmi-instr-depth 2" >> "$L"
      echo "$H/job.sh rules_stack $e $t jit-depth 2" >> "$L"
      echo "$H/job.sh rules_stack $e $t jit 3" >> "$L"
      case $C in F) CS="0 200000" ;; I|AD) CS="200000" ;; *) CS="" ;; esac
      for mix in nop host_tick; do for c in $CS; do for ed in 0 512; do
        echo "$H/job.sh rules_s8adv $e $t $mix $ed $c" >> "$L"; done; done; done ;; esac
  done
  # slow (interpreter / emulated) units first so the tail of the run stays parallel
  { grep -E ' (jsc (ipint|nojit)|sm1[0-9]+a) ' "$L"; grep -v -E ' (jsc (ipint|nojit)|sm1[0-9]+a) ' "$L"; } > "$L.sorted"
  local n=$(wc -l < "$L")
  if [ "${KEEP:-0}" = 1 ]; then  # resume: keep runs/res and skip jobs that already have a result line
    python3 -I "$H/skip_done.py" "$L.sorted" "$R/res" > "$L.todo" && mv "$L.todo" "$L.sorted"
  fi
  echo "== $n jobs planned, $(wc -l < "$L.sorted") to run, PAR=$PAR, started $(date -u +%FT%TZ)"
  xargs -P "$PAR" -I{} sh -c '{}' < "$L.sorted"
  echo "== jobs finished $(date -u +%FT%TZ)"
}
summary() {
  python3 -I "$H/summarize.py" "$R/res" > "$R/summary.md"
  local rc=$?
  tail -5 "$R/summary.md"
  return $rc
}
case "$WHAT" in
  fetch) fetch ;;
  verify) verify ;;
  refs) "$H/make_refs.sh" ;;
  jobs) jobs ;;
  summary) summary ;;
  all) fetch && verify && "$H/make_refs.sh" && rm -rf "$R/res" "$R/out" "$R/manifest" && jobs && summary ;;
  *) echo "usage: $0 [all|fetch|verify|refs|jobs|summary]" >&2; exit 2 ;;
esac
