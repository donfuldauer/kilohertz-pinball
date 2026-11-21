#!/usr/bin/env bash
# Runs pin_assets' zip/VFS hardening test binaries compiled for wasm32-wasip1
# under Node (node/wasi_test_runner.mjs), one test per process (panic=abort
# would otherwise stop the binary at the first panic), and classifies each:
# PASS, FS (needs the host filesystem / temp_dir: folder bundles, which the
# web does not have), FAIL (anything else). Prints a per-binary summary.
# usage: run_wasip1_tests.sh   (after the cargo test build in RESULTS §zip)
set -u
HERE="$(cd "$(dirname "$0")/.." && pwd)"
. "$HERE/env.sh"
W=$CARGO_TARGET_DIR/wasm32-wasip1/debug/deps
OUT=$S4/out/zip/wasip1_tests.txt
: >"$OUT"
for bin in pin_assets hardening hardening_round2 vfs_zip spec_vfs fuzz spec_fuzz review_pr4 bundle; do
  f=$(ls -t "$W"/"$bin"-*.wasm 2>/dev/null | head -1)
  [ -z "$f" ] && { echo "$bin: no binary" | tee -a "$OUT"; continue; }
  p=0; fs=0; fl=0
  for t in $(node --no-warnings "$S4/node/wasi_test_runner.mjs" "$f" --list 2>/dev/null | sed -n 's/: test$//p'); do
    o=$(node --no-warnings "$S4/node/wasi_test_runner.mjs" "$f" --exact "$t" --nocapture 2>&1)
    if echo "$o" | grep -q "test result: ok. 1 passed"; then
      p=$((p + 1)); echo "PASS $bin::$t" >>"$OUT"
    elif echo "$o" | grep -q "1 ignored"; then
      echo "IGN  $bin::$t" >>"$OUT"
    elif echo "$o" | grep -qE "not supported by WASI yet|operation not supported|No such file|Unsupported|unsupported on this platform|Os \{ code: 5[0-9]"; then
      fs=$((fs + 1)); echo "FS   $bin::$t :: $(echo "$o" | grep -m1 -E 'panicked|not supported|Error' | cut -c1-160)" >>"$OUT"
    else
      fl=$((fl + 1)); echo "FAIL $bin::$t :: $(echo "$o" | grep -m2 -E 'panicked|assert|TRAP' | tr '\n' ' ' | cut -c1-200)" >>"$OUT"
    fi
  done
  echo "$bin: PASS=$p FS(host-fs, not on the web path)=$fs FAIL=$fl" | tee -a "$OUT"
done
