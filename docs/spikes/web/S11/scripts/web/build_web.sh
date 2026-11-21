#!/bin/sh
# Build the web engine module (S11 portable rewrite of S9's build_web.sh; proposed
# scripts/web/build_web.sh, W20). Linux x86_64 (incl. WSL2), macOS arm64 and macOS x86_64.
#   cargo build (wasm32-unknown-unknown, pin_web) -> wasm-bindgen --target web -> wasm-opt -Oz
#   -> wasm-tools validate -> <out>/pkg/{pin_web_bg.wasm,pin_web.js} + SHA256SUMS + build.log
#
# usage: build_web.sh [--repo DIR] [--out DIR] [--tools DIR] [--features LIST]
#                     [--no-default-features] [--no-opt] [--no-remap] [--dry-run]
#   --repo DIR   workspace root (default: two directories above this script)
#   --out DIR    output directory (default: <repo>/target/web/dist)
#   --tools DIR  fetch_tools.sh install root (default: $KHZ_WEB_TOOLS, else <repo>/target/web-tools)
#   --no-remap   S9's exact flags (no --remap-path-prefix): module bytes then depend on CARGO_HOME
#   --dry-run    check prerequisites, print the environment and commands, build nothing
# Prerequisites (checked): rustc/cargo 1.97.0 with target wasm32-unknown-unknown, Node 22,
# fetch_tools.sh run for this platform, and on macOS the Xcode Command Line Tools (host linker
# for build scripts and proc macros).
set -eu
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
. "$here/lib.sh"

repo=""; out=""; tools=${KHZ_WEB_TOOLS:-}; feat=""; opt=1; dry=0
KHZ_WEB_REMAP=${KHZ_WEB_REMAP:-1}
while [ $# -gt 0 ]; do
  case $1 in
    --repo) repo=$2; shift 2 ;;
    --out) out=$2; shift 2 ;;
    --tools) tools=$2; shift 2 ;;
    --features) feat="$feat --features $2"; shift 2 ;;
    --no-default-features) feat="$feat --no-default-features"; shift ;;
    --no-opt) opt=0; shift ;;
    --no-remap) KHZ_WEB_REMAP=0; shift ;;
    --dry-run) dry=1; shift ;;
    -h | --help) sed -n '2,18p' "$0"; exit 0 ;;
    *) khz_die "unknown argument: $1" 2 ;;
  esac
done
[ -n "$repo" ] || repo=$here/../..
KHZ_WEB_REPO=$(khz_abs_dir "$repo") || khz_die "no such directory: $repo"
[ -f "$KHZ_WEB_REPO/Cargo.toml" ] || khz_die "$KHZ_WEB_REPO is not a cargo workspace"
[ -n "$tools" ] || tools=$KHZ_WEB_REPO/target/web-tools
[ -n "$out" ] || out=$KHZ_WEB_REPO/target/web/dist
platform=$(khz_platform) || exit 3

# --- prerequisites ---------------------------------------------------------------------------
[ -f "$tools/tools.env" ] || khz_die "no $tools/tools.env: run fetch_tools.sh first"
# shellcheck source=/dev/null
. "$tools/tools.env"
[ "${KHZ_WEB_PLATFORM:-}" = "$platform" ] ||
  khz_die "tools in $tools are for ${KHZ_WEB_PLATFORM:-?}, this host is $platform: run fetch_tools.sh"
[ -n "${KHZ_CARGO_TOOLS:-}" ] && PATH=$KHZ_CARGO_TOOLS/bin:$PATH
[ -n "${KHZ_NODE:-}" ] && PATH=$KHZ_NODE/bin:$PATH
export PATH
if [ "$(uname -s)" = Darwin ]; then
  xcode-select -p >/dev/null 2>&1 ||
    khz_die "Xcode Command Line Tools missing (host build scripts and proc macros link with cc): run xcode-select --install"
fi
command -v cargo >/dev/null 2>&1 || khz_die "cargo not found (install rustup)"
rv=$(rustc --version 2>/dev/null | awk '{print $2}')
[ "$rv" = 1.97.0 ] || khz_die "rustc $rv found, 1.97.0 required (rustup toolchain install 1.97.0)"
[ -d "$(rustc --print sysroot)/lib/rustlib/wasm32-unknown-unknown" ] ||
  khz_die "target missing: rustup target add wasm32-unknown-unknown"
nv=$(node --version 2>/dev/null) || khz_die "Node 22 not found"
case $nv in v22.*) ;; *) khz_die "Node $nv found, Node 22 required" ;; esac
for t in wasm-bindgen wasm-tools; do command -v $t >/dev/null 2>&1 || khz_die "$t not found: run fetch_tools.sh"; done
WO=$KHZ_BINARYEN/bin/wasm-opt
[ -x "$WO" ] || khz_die "missing $WO: run fetch_tools.sh"

. "$here/wasm_env.sh"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-$(khz_ncpu)}"

mkdir -p "$out/pkg"
out=$(khz_abs_dir "$out")
log=$out/build.log
{
  echo "== host $platform ($(uname -s) $(uname -m)), remap=$KHZ_WEB_REMAP"
  echo "== versions"; rustc -vV | sed -n '1p;/^host/p;/^LLVM/p'; cargo -V; wasm-bindgen --version; wasm-tools --version
  "$WO" --version; "$CC_wasm32_unknown_unknown" --version | head -1; node --version
  echo "== env"; env | grep -E '^(CC_wasm32|AR_wasm32|KHZ_|CARGO_PROFILE_RELEASE|CARGO_BUILD_JOBS|CARGO_TARGET_DIR|CARGO_INCREMENTAL|CFLAGS_wasm32|CC_SHELL)' | LC_ALL=C sort
  echo "CARGO_ENCODED_RUSTFLAGS=$(printf '%s' "$CARGO_ENCODED_RUSTFLAGS" | tr '\037' ' ')"
} > "$log" 2>&1
# hyphenated variables cannot be unset by sh; env -u works in GNU, BSD and busybox env
unset_args=""
for v in $KHZ_ENV_UNSET; do unset_args="$unset_args -u $v"; done
# shellcheck disable=SC2086
set -- env $unset_args cargo build --locked --release --target wasm32-unknown-unknown -p pin_web $feat
if [ "$dry" = 1 ]; then
  cat "$log"; echo "== would run (in $KHZ_WEB_REPO): $*"
  echo "== then wasm-bindgen --target web --out-name pin_web, wasm-opt -Oz, wasm-tools validate -> $out/pkg"
  exit 0
fi
t0=$(date +%s)
(cd "$KHZ_WEB_REPO" && "$@") >> "$log" 2>&1 || { grep -E '^error' -A15 "$log" | head -60; khz_die "cargo build failed (log $log)"; }
t1=$(date +%s)
W=${CARGO_TARGET_DIR:-$KHZ_WEB_REPO/target}/wasm32-unknown-unknown/release/pin_web.wasm
raw=$out/raw
rm -rf "$raw"; mkdir -p "$raw"
cp "$W" "$raw/pin_web.cargo.wasm"
wasm-bindgen --target web --out-name pin_web --out-dir "$raw" "$raw/pin_web.cargo.wasm" >> "$log" 2>&1
t2=$(date +%s)
cp "$raw/pin_web.js" "$raw/pin_web.d.ts" "$out/pkg/"
if [ "$opt" = 1 ]; then
  "$WO" -Oz --enable-simd --enable-exception-handling --enable-bulk-memory \
    --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals \
    --enable-reference-types --enable-multivalue --strip-debug --strip-producers \
    "$raw/pin_web_bg.wasm" -o "$out/pkg/pin_web_bg.wasm" >> "$log" 2>&1
else
  cp "$raw/pin_web_bg.wasm" "$out/pkg/pin_web_bg.wasm"
fi
t3=$(date +%s)
wasm-tools validate --features=mvp,simd,exceptions,legacy-exceptions,bulk-memory,saturating-float-to-int,sign-extension,mutable-global,reference-types,multi-value \
  "$out/pkg/pin_web_bg.wasm" >> "$log" 2>&1 || khz_die "wasm-tools validate failed (log $log)"
: > "$out/pkg/SHA256SUMS"
for f in pin_web_bg.wasm pin_web.js; do
  printf '%s  %s\n' "$(khz_sha256 "$out/pkg/$f")" "$f" >> "$out/pkg/SHA256SUMS"
done
{
  echo "== timings (s): cargo $((t1 - t0)) wasm-bindgen $((t2 - t1)) wasm-opt $((t3 - t2))"
  echo "== sizes (bytes): cargo $(khz_size "$raw/pin_web.cargo.wasm") bindgen $(khz_size "$raw/pin_web_bg.wasm") final $(khz_size "$out/pkg/pin_web_bg.wasm") glue $(khz_size "$out/pkg/pin_web.js")"
  echo "== sha256"; cat "$out/pkg/SHA256SUMS"
} >> "$log"
tail -5 "$log"
echo "built $out/pkg (log $log)"
