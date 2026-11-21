#!/usr/bin/env bash
# S4 rules parity: per-tick state_hash, supplementary ext_hash and the rules
# command-queue hash of every web design against the native engine path
# (pin_sim Script::Wasm = wasmtime, untouched config), for the three
# production tables x {bot60, pat60, mb8} (S1 recordings, real rules.wasm
# execution), in native, Node (V8) and Playwright Chromium.
#
# usage: run_rules_parity.sh [quick|full] [nobuild]
#   quick: bot60 only; full: + pat60, mb8 and the 10-minute apex soak.
# Prints one PASS/FAIL line per scenario x route x design, then a summary;
# exit 0 only if every line PASSes.
set -u
HERE="$(cd "$(dirname "$0")" && pwd)"
. "$HERE/env.sh"
MODE="${1:-full}"
cd "$S4"
LOG="$S4/logs/parity_$MODE.log"
mkdir -p out/native out/node out/chromium logs
BIN="$CARGO_TARGET_DIR/release/s4"
export S4_PKG="$S4/pkg"

if [ "${2:-}" != "nobuild" ]; then
  (cd repo && cargo build --release -p s4_rules --features native --bin s4) >"$S4/logs/build_native.log" 2>&1 || { echo "native build failed"; exit 2; }
  (cd repo && RUSTFLAGS="-C target-feature=+simd128 -C link-arg=-zstack-size=4194304" \
    cargo build --release --target wasm32-unknown-unknown -p s4_rules --lib) >"$S4/logs/build_wasm.log" 2>&1 || { echo "wasm build failed"; exit 2; }
  wasm-bindgen --target web --out-dir "$S4/pkg" "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/s4_rules.wasm" || exit 2
fi

SCNS=()
for t in apex_circuit sky_deck feature_lab; do
  SCNS+=("${t}_bot60")
  if [ "$MODE" = full ]; then SCNS+=("${t}_pat60" "${t}_mb8"); fi
done
[ "$MODE" = full ] && SCNS+=("apex_circuit_bot600")

fail=0
pass=0
: >"$LOG"
say() { echo "$*" | tee -a "$LOG"; }

compare() { # name route design file baseline summary
  local name=$1 route=$2 design=$3 f=$4 base=$5 summ=$6
  local fuel
  fuel=$(echo "$summ" | grep -o 'fuel_digest=[0-9a-f]*' | cut -d= -f2)
  local maxf
  maxf=$(echo "$summ" | grep -o 'max_fuel=[0-9]*' | cut -d= -f2)
  local fnote="fuel_digest=$fuel max_fuel=$maxf"
  if [ -n "$NATIVE_FUEL" ] && [ "$fuel" != "0000000000000000" ]; then
    if [ "$fuel" = "$NATIVE_FUEL" ]; then fnote="$fnote fuel=wasmtime-exact"; else fnote="$fnote fuel=differs"; fi
  fi
  if [ -s "$f" ] && cmp -s "$f" "$base"; then
    say "PASS $name $route $design ticks=$(($(wc -l <"$f") - 1)) final=$(tail -1 "$f" | cut -d' ' -f2-4) $fnote"
    pass=$((pass + 1))
  else
    local at
    at=$(cmp "$f" "$base" 2>/dev/null | grep -o 'line [0-9]*' | cut -d' ' -f2)
    say "FAIL $name $route $design first_diverging_tick=$((${at:-1} - 1)) native=[$(sed -n "${at:-1}p" "$base")] web=[$(sed -n "${at:-1}p" "$f" 2>/dev/null)] $fnote"
    fail=$((fail + 1))
  fi
}

for name in "${SCNS[@]}"; do
  t=${name%_*}
  scn="$S1/rec/$name.scn"
  zip="bundles/${t}_wasm.pinbundle"
  base="out/native/$name.engine.hashes"
  s=$("$BIN" play engine "$zip" "$scn" "$base") || { say "FAIL $name native engine load"; fail=$((fail + 1)); continue; }
  say "BASE $name $s"
  # rules.wasm vs rules.lua (S1's native Lua run): state_hash and ext_hash columns.
  if [ -f "$S1/native/$name.hashes" ]; then
    if cmp -s <(cut -d' ' -f1-3 "$base") "$S1/native/$name.hashes"; then
      say "PASS $name lua-vs-wasm native-lua(S1) rules.lua==rules.wasm (state+ext every tick)"; pass=$((pass + 1))
    else
      at=$(cmp <(cut -d' ' -f1-3 "$base") "$S1/native/$name.hashes" | grep -o 'line [0-9]*' | cut -d' ' -f2)
      say "FAIL $name lua-vs-wasm first_diverging_tick=$((${at:-1} - 1))"; fail=$((fail + 1))
    fi
  fi
  NATIVE_FUEL=""
  designs_native="native native-instr-depth wasmi-fuel wasmi-instr wasmi-instr-depth"
  designs_node="wasmi-fuel wasmi-instr wasmi-instr-depth jit jit-depth hybrid"
  designs_chromium="jit jit-depth wasmi-instr-depth hybrid"
  if [ "$name" = apex_circuit_bot600 ]; then
    designs_native="native native-instr-depth wasmi-instr-depth"; designs_node="wasmi-instr-depth jit-depth"; designs_chromium="jit-depth"
  fi
  for d in $designs_native; do
    f="out/native/$name.$d.hashes"
    s=$("$BIN" play "$d" "$zip" "$scn" "$f" 2>&1)
    [ "$d" = native ] && NATIVE_FUEL=$(echo "$s" | grep -o 'fuel_digest=[0-9a-f]*' | cut -d= -f2)
    compare "$name" native "$d" "$f" "$base" "$s"
  done
  for d in $designs_node; do
    f="out/node/$name.$d.hashes"
    s=$(node node/run_web.mjs play "$d" "$zip" "$scn" "$f" 2>&1)
    compare "$name" node "$d" "$f" "$base" "$s"
  done
  # The async-compile route (WebAssembly.compile ahead, then sync instantiate).
  f="out/node/$name.jit-cache.hashes"
  s=$(node node/run_web.mjs mode=cache play jit "$zip" "$scn" "$f" 2>&1)
  compare "$name" node jit-asynccompile "$f" "$base" "$s"
  for d in $designs_chromium; do
    f="out/chromium/$name.$d.out"
    node node/chromium.cjs 'web/index.html?iso=1' play \
      "{\"design\":\"$d\",\"bundle\":\"/$zip\",\"scn\":\"/s1rec/$name.scn\",\"mode\":\"cache\"}" "$f" >/dev/null 2>"$S4/logs/chromium_$name.$d.err"
    s=$(head -1 "$f")
    tail -n +2 "$f" >"out/chromium/$name.$d.hashes"
    compare "$name" chromium "$d" "out/chromium/$name.$d.hashes" "$base" "$s"
  done
done
say "== parity $MODE: $pass PASS, $fail FAIL"
[ "$fail" -eq 0 ]
