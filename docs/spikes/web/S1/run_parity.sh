#!/bin/sh
# web-spike-sim (S1): native x86_64 vs wasm per-tick state-hash parity.
#
# usage: run_parity.sh [simd|base|scalar]   (default: simd, the recommended build)
#   simd   : the repo + the gating patches; wasm built with -C target-feature=+simd128
#            (no source or native change). Expected: PASS everywhere.
#   base   : same, wasm built with rustc's default wasm32 features (no simd128).
#            Expected: FAIL, first divergence = a ball's orientation quaternion.
#   scalar : glam feature `scalar-math` on every target (patches/0005, changes native);
#            native re-recorded in that tree; wasm without simd128. Expected: PASS.
# env: SCENARIOS="apex_circuit_bot60 ..." to limit; SKIP_BUILD=1 to reuse artifacts;
#      SKIP_BROWSER=1 to skip the Chromium run.
#
# Prints one PASS/FAIL line per scenario and route (wasip1 under node:wasi; wasm-bindgen
# under Node; and, for bot60, Chromium), with the first diverging tick on FAIL.
# Exit 0 only when every line is PASS.
set -eu
S=${WEB_SPIKES}/S1
. "$S/env.sh"
FL=${1:-simd}
TD0=$CARGO_TARGET_DIR
case "$FL" in
  simd)   TREE=$S/repo;        WRF="-C target-feature=+simd128"; export CARGO_TARGET_DIR=$TD0/simd; NTD=$TD0;          REC=$S/rec;              NAT=$S/native ;;
  base)   TREE=$S/repo;        WRF="";                           export CARGO_TARGET_DIR=$TD0;      NTD=$TD0;          REC=$S/rec;              NAT=$S/native ;;
  scalar) TREE=$S/repo_scalar; WRF="";                           export CARGO_TARGET_DIR=$TD0/scalar; NTD=$TD0/scalar; REC=$S/runs/scalar/rec;  NAT=$S/runs/scalar/native ;;
  *) echo "usage: $0 [simd|base|scalar]" >&2; exit 2 ;;
esac
OUT=$S/runs/$FL
mkdir -p "$OUT/wasi" "$OUT/bindgen" "$REC" "$NAT" "$S/logs"
ALL="replay_table_fixture10k apex_circuit_bot60 apex_circuit_pat60 apex_circuit_mb8 sky_deck_bot60 sky_deck_pat60 sky_deck_mb8 feature_lab_bot60 feature_lab_pat60 feature_lab_mb8 apex_circuit_bot600"
SCN=${SCENARIOS:-$ALL}
NT=$NTD/release
WASI=$CARGO_TARGET_DIR/wasm32-wasip1/release/s1.wasm
CDY=$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/s1_harness.wasm

if [ "${SKIP_BUILD:-0}" != 1 ]; then
  echo "== build ($FL)"
  ( cd "$TREE"
    CARGO_TARGET_DIR=$NTD cargo build -q --release -p s1_harness --features native --bins
    CARGO_TARGET_DIR=$NTD cargo build -q --release -p pin_sim --bin pin_sim
    RUSTFLAGS="$WRF" cargo build -q --release --target wasm32-wasip1 -p s1_harness --bin s1
    RUSTFLAGS="$WRF" cargo build -q --release --target wasm32-unknown-unknown -p s1_harness --lib )
  wasm-bindgen --version
  wasm-bindgen --target nodejs --out-dir "$OUT/pkg_node" "$CDY"
  wasm-bindgen --target web --out-dir "$OUT/pkg_web" "$CDY"
  cp "$WASI" "$OUT/s1_wasip1.wasm"
fi

# Bundles (generated natively, shared by every route).
for t in apex_circuit sky_deck feature_lab; do
  if [ ! -f "$S/bundles/$t.pinbundle" ]; then
    rm -rf "$S/bundles/$t"; mkdir -p "$S/bundles/$t"
    if [ "$t" = feature_lab ]; then
      "$NT/s1_build_feature_lab" "$S/bundles/$t"
      cp "$TREE/tables/feature_lab/rules.wasm" "$S/bundles/$t/"
      ( cd "$TREE" && CARGO_TARGET_DIR=$NTD cargo run -q --release -p pin_tablegen -- validate "$S/bundles/$t" )
    else
      ( cd "$TREE" && CARGO_TARGET_DIR=$NTD sh "tables/$t/build.sh" "$S/bundles/$t" )
    fi
    python3 -I "$S/harness/make_zip.py" "$S/bundles/$t" "$S/bundles/$t.pinbundle"
  fi
done

# The repo's CI replay-hash fixture (pin_sim --ticks 10000 pattern; documented golden
# state_hash=25097c21c2c3adc1 in docs/decisions/2026-11-19-w8g-e.md).
[ -f "$S/bundles/replay_table.pinbundle" ] || python3 -I "$S/harness/make_zip.py" \
  "$TREE/crates/pin_sim/tests/fixtures/replay_table" "$S/bundles/replay_table.pinbundle"

# Native recordings with the real Lua rules (reference hashes in $NAT).
for s in $SCN; do
  t=${s%_*}; kind=${s##*_}
  if [ ! -f "$REC/$s.scn" ]; then
    case $kind in
      bot60)  "$NT/s1" record-bot "$S/bundles/$t" "$S/bundles/$t.pinbundle" 0x4B485A 60000 "$REC/$s" ;;
      bot600) "$NT/s1" record-bot "$S/bundles/$t" "$S/bundles/$t.pinbundle" 0x4B485A 600000 "$REC/$s" ;;
      pat60)  "$NT/s1" record-pattern "$S/bundles/$t.pinbundle" 0x4B485A 60000 0 "$REC/$s" ;;
      mb8)    "$NT/s1" record-pattern "$S/bundles/$t.pinbundle" 0x4B485A 20000 8 "$REC/$s" ;;
      fixture10k) "$NT/s1" record-pattern "$S/bundles/$t.pinbundle" 0x4B485A 10000 0 "$REC/$s" nolaunch ;;
    esac
    mv "$REC/$s.native_lua.hashes" "$NAT/$s.hashes"
  fi
done

FAILS=0
report() { if python3 -I "$S/harness/compare.py" "$@"; then :; else FAILS=$((FAILS + 1)); fi; }

echo "== native controls"
for s in $SCN; do
  t=${s%_*}
  "$NT/s1" play "$S/bundles/$t.pinbundle" "$REC/$s.scn" "$REC/$s.scr" "$OUT/$s.native_play.hashes" > "$S/logs/${FL}_${s}_native_play.log"
  report "$FL $s native real-Lua vs native playback" "$NAT/$s.hashes" "$OUT/$s.native_play.hashes"
  if [ -f "$REC/$s.khzrec" ]; then
    want=$(tail -1 "$NAT/$s.hashes" | cut -d' ' -f2)
    got=$("$NT/pin_sim" "$S/bundles/$t.pinbundle" --replay-game "$REC/$s.khzrec" | sed -n 's/^state_hash=//p')
    if [ "$want" = "$got" ]; then echo "PASS $FL $s pin_sim --replay-game final state_hash $got"; else echo "FAIL $FL $s pin_sim --replay-game $got != $want"; FAILS=$((FAILS + 1)); fi
  fi
done

if [ -f "$NAT/replay_table_fixture10k.hashes" ]; then
  fx=$("$NT/pin_sim" "$TREE/crates/pin_sim/tests/fixtures/replay_table" --ticks 10000 | sed -n 's/^state_hash=//p')
  echo "INFO $FL pin_sim fixture --ticks 10000: state_hash=$fx (main at 995935f documents 25097c21c2c3adc1)"
fi

echo "== wasm32-wasip1 under node:wasi"
for s in $SCN; do
  t=${s%_*}
  S1_DIR=$S node --no-warnings "$S/harness/run_wasi.mjs" "$OUT/s1_wasip1.wasm" play "/s/bundles/$t.pinbundle" \
    "/s/${REC#$S/}/$s.scn" "/s/${REC#$S/}/$s.scr" "/s/runs/$FL/wasi/$s.hashes" > "$S/logs/${FL}_${s}_wasi.log" 2>&1 || true
  report "$FL $s native vs wasip1" "$NAT/$s.hashes" "$OUT/wasi/$s.hashes"
done

echo "== wasm32-unknown-unknown + wasm-bindgen under Node"
for s in $SCN; do
  t=${s%_*}
  node "$S/harness/run_bindgen.mjs" "$OUT/pkg_node" play "$S/bundles/$t.pinbundle" "$REC/$s.scn" "$REC/$s.scr" \
    "$OUT/bindgen/$s.hashes" > "$S/logs/${FL}_${s}_bindgen.log" 2>&1 || true
  report "$FL $s native vs wasm-bindgen" "$NAT/$s.hashes" "$OUT/bindgen/$s.hashes"
done

if [ "${SKIP_BROWSER:-0}" != 1 ]; then
  echo "== wasm-bindgen in Playwright Chromium (bot60, no bench)"
  W=$OUT/web; rm -rf "$W"; mkdir -p "$W/data"
  cp "$S/web/index.html" "$S/web/main.js" "$W/"; cp -r "$OUT/pkg_web" "$W/pkg"
  for t in apex_circuit sky_deck feature_lab; do
    cp "$S/bundles/$t.pinbundle" "$W/data/"
    cp "$REC/${t}_bot60.scn" "$REC/${t}_bot60.scr" "$W/data/"
    cp "$NAT/${t}_bot60.hashes" "$W/data/${t}_bot60.native.hashes"
  done
  node "$S/harness/chromium.cjs" "$W" 8731 1 "$OUT/chromium_parity.json" "bench=0" > "$S/logs/${FL}_chromium.log" 2>&1 || true
  for t in apex_circuit sky_deck feature_lab; do
    if grep -q "^${t}_bot60: identical=true" "$S/logs/${FL}_chromium.log"; then
      echo "PASS $FL ${t}_bot60 native vs Chromium (every tick, state_hash and ext_hash lines)"
    else
      echo "FAIL $FL ${t}_bot60 native vs Chromium: $(grep "^${t}_bot60" "$S/logs/${FL}_chromium.log" || echo 'no result')"
      FAILS=$((FAILS + 1))
    fi
  done
fi

echo "== $FL: $FAILS failing line(s)"
[ "$FAILS" -eq 0 ]
