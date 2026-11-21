#!/bin/sh
# Regenerates patches/*.patch: diff -u of the scratch trees against the pristine copy of
# main (BASE_COMMIT.txt), with a/ b/ labels so `git apply` / `patch -p1` work at the root.
set -eu
S=${WEB_SPIKES}/S1
cd "$S"
mk() { # out tree rationale files...
  out=$1; tree=$2; why=$3; shift 3
  : > "patches/$out"
  echo "# $why" > "patches/$out"
  for f in "$@"; do
    a="pristine/$f"; b="$tree/$f"
    [ -e "$a" ] || a=/dev/null
    diff -u --label "a/$f" --label "b/$f" "$a" "$b" >> "patches/$out" || true
  done
}
rm -f patches/*.patch
mk 0001-pin_script-native-backends-feature.patch repo \
  "pin_script: mlua/notify/wasmtime/wat optional behind default feature native-backends; LuaConfig/WasmConfig always built (so the sim crates build for wasm32 without C Lua or wasmtime)" \
  crates/pin_script/Cargo.toml crates/pin_script/src/lib.rs crates/pin_script/src/lua/mod.rs \
  crates/pin_script/src/lua/config.rs crates/pin_script/src/wasm/mod.rs crates/pin_script/src/wasm/config.rs
mk 0002-pin_sim-external-script.patch repo \
  "pin_sim: forwards native-backends; ExternalScript trait + Simulation::load_from_vfs_with_script so a web host supplies the rules backend (the Lua/Wasm variants stay as they are natively)" \
  crates/pin_sim/Cargo.toml crates/pin_sim/src/lib.rs crates/pin_sim/src/sim/mod.rs \
  crates/pin_sim/src/tick/mod.rs crates/pin_sim/src/load/mod.rs
mk 0003-pin_sim-apply-game-op.patch repo \
  "pin_sim: public apply_game_op so a driver that steps the sim itself (per-tick hashes, a web frame loop) replays recorded bot games exactly like GameRecording::play" \
  crates/pin_sim/src/replay/game.rs crates/pin_sim/src/replay/mod.rs
mk 0004-s1_harness-spike-tooling.patch repo \
  "SPIKE TOOLING ONLY (not for main): the s1_harness crate (record/playback/hash/bench/probes) and its Cargo.lock entry; needed to reproduce RESULTS.md" \
  Cargo.lock crates/s1_harness/Cargo.toml crates/s1_harness/src/lib.rs crates/s1_harness/src/io.rs \
  crates/s1_harness/src/playback.rs crates/s1_harness/src/record.rs crates/s1_harness/src/driver.rs \
  crates/s1_harness/src/hashes.rs crates/s1_harness/src/run.rs crates/s1_harness/src/probes.rs \
  crates/s1_harness/src/web.rs crates/s1_harness/src/bin/s1.rs
mk 0005-option-a-glam-scalar-math.patch repo_scalar \
  "EVALUATED, NOT RECOMMENDED (option a): glam scalar-math on every target; gives native/wasm parity without simd128 but changes native sim output (orientation bits) for every table" \
  Cargo.toml
ls -la patches; head -1 patches/*.patch
