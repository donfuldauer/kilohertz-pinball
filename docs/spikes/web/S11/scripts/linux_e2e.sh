#!/bin/sh
# S11 req 3: the portable scripts on Linux x86_64, end to end, under dash (the strictest shell):
#   1. fetch_tools.sh: wasi-sdk 32.0 + binaryen 132 from a local mirror of the real archives
#      (sha256-pinned), `cargo install --locked` wasm-bindgen-cli 0.2.130 + wasm-tools 1.263.0
#      from crates.io, smoke checks;
#   2. build_web.sh --no-remap  -> must equal S9's shipped module 88e06b16... / glue 7b77282c...
#   3. build_web.sh (remap)     -> must equal repro_paths.sh's a_remap/b_remap 9f137f8a...
#   4. check_imports.sh, size_report.sh and S9's Node parity check (20 runs) on both modules.
# Uses S9's scratch tree copy work/a/kp (repro_paths.sh creates it) and S9's dist/ test data.
# usage: scripts/linux_e2e.sh      (about 25 minutes on 2 jobs; KEEP_TOOLS=1 skips the re-fetch)
set -eu
S11=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
SPK=$(dirname "$S11")
WEB=$S11/scripts/web
O=$S11/results/linux_e2e
T=$S11/work/e2e-tools
TBASE=/home/user/kilohertz-pinball/target/web_s11
mkdir -p "$O" "$S11/work/e2e-mirror"
[ "${KEEP_TOOLS:-0}" = 1 ] || rm -rf "$T"   # KEEP_TOOLS=1 reuses a previous fetch (cache-hit path)
for f in "$S11"/dl/wasi-sdk-32-x86_64-linux/*.tar.gz "$S11"/dl/binaryen-132-x86_64-linux/*.tar.gz; do
  ln -f "$f" "$S11/work/e2e-mirror/$(basename "$f")"
done
export CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_TEST_DEBUG=line-tables-only
export CARGO_HOME=/root/.cargo
echo "== 1 fetch_tools.sh (dash)"
CARGO_TARGET_DIR=$TBASE/e2e_install KHZ_WEB_MIRROR=$S11/work/e2e-mirror dash "$WEB/fetch_tools.sh" --tools "$T" 2>&1 | tee "$O/fetch_tools.log"
rm -rf "$TBASE/e2e_install"
for mode in plain remap; do
  echo "== build_web.sh ($mode, dash)"
  flag=""; [ $mode = plain ] && flag=--no-remap
  CARGO_TARGET_DIR=$TBASE/e2e_$mode dash "$WEB/build_web.sh" --repo "$S11/work/a/kp" --tools "$T" --out "$O/dist_$mode" $flag 2>&1 | tee "$O/build_$mode.log"
  cp "$O/dist_$mode/build.log" "$O/build_web_$mode.full.log"
  PATH=$T/cargo/bin:$PATH dash "$WEB/check_imports.sh" "$O/dist_$mode/pkg/pin_web_bg.wasm" "$O/audit_$mode" | tee "$O/check_imports_$mode.log"
  dash "$WEB/size_report.sh" --budget "s9_$mode=3.0" "s9_$mode=$O/dist_$mode/pkg/pin_web_bg.wasm" | tee "$O/size_$mode.md"
  node "$S11/work/node/parity_node.mjs" "$SPK/S9/dist" all lua,wasm "$O/dist_$mode/pkg" > "$O/parity_node_$mode.txt" 2>&1 || true
  echo "parity: $(grep -c '^PASS' "$O/parity_node_$mode.txt") PASS, $(grep -c '^FAIL' "$O/parity_node_$mode.txt") FAIL; $(tail -1 "$O/parity_node_$mode.txt")"
  rm -rf "$TBASE/e2e_$mode"
done
echo "== results"
echo "plain: $(cat "$O/dist_plain/pkg/SHA256SUMS" | tr '\n' ' ')"
echo "remap: $(cat "$O/dist_remap/pkg/SHA256SUMS" | tr '\n' ' ')"
grep -q '^88e06b166af4781bacd199d136ea8059077fdd191ddadd023b07b1e1ae16e95b  pin_web_bg.wasm$' "$O/dist_plain/pkg/SHA256SUMS" && echo "PASS plain module == S9 shipped module 88e06b16..." || echo "FAIL plain module != S9"
grep -q '^7b77282c74b37c04826ce0e8489e12604a570a4ba187ae5c187632c2d05ccae8  pin_web.js$' "$O/dist_plain/pkg/SHA256SUMS" && echo "PASS plain glue == S9 glue 7b77282c..." || echo "FAIL plain glue != S9"
grep -q '^9f137f8aa14a3252a184d8bcbf40566ef1c6aa2cbb7058bc3e424d6055f56a73  pin_web_bg.wasm$' "$O/dist_remap/pkg/SHA256SUMS" && echo "PASS remap module == repro a_remap/b_remap 9f137f8a..." || echo "FAIL remap module != a_remap"
