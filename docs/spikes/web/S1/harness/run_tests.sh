#!/bin/sh
# Native test suites of the five sim crates (COMMON env), one crate at a time; test
# executables are deleted after each crate so the disk does not fill (a single
# `cargo test -p ... x5` run produced 12 GB of test binaries and filled the disk).
# usage: run_tests.sh <tree> <target_dir> <log>
set -u
S=${WEB_SPIKES}/S1
. "$S/env.sh"
TREE=$1; export CARGO_TARGET_DIR=$2; LOG=$3
: > "$LOG"
cd "$TREE"
rc=0
for c in ${CRATES:-pin_core pin_input pin_assets pin_physics pin_sim}; do
  avail=$(df --output=avail -k /home/user | tail -1)
  if [ "$avail" -lt 4000000 ]; then echo "ABORT: less than 4 GB free before $c" >> "$LOG"; exit 3; fi
  echo "### cargo test -p $c" >> "$LOG"
  cargo test -p "$c" >> "$LOG" 2>&1 || rc=1
  find "$CARGO_TARGET_DIR/debug/deps" -maxdepth 1 -type f -perm -u+x ! -name '*.so' -delete 2>/dev/null
  rm -rf "$CARGO_TARGET_DIR/debug/build/"*/out/*.o 2>/dev/null
done
echo "exit=$rc" >> "$LOG"
