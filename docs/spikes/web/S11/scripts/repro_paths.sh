#!/bin/sh
# S11 req 2 (d): does the web module depend on host paths? Builds the S9 engine module (same pins,
# same Cargo.lock, --locked) in two checkouts that differ in every host path we control:
#   a: work/a/kp (short), CARGO_HOME=/root/.cargo (the S9 cache), rustup toolchain, S2's wasi-sdk,
#      target dir target/web_s11/a
#   b: work/b/Users/macuser/src/kilohertz-pinball-web-checkout (Mac-style, 161 chars), a NEW
#      CARGO_HOME work/b/Users/macuser/.cargo (fresh registry download), HOME/USER/TMPDIR of a fake
#      "macuser", a hard-linked copy of the toolchain at work/b/Users/macuser/.rustup/... (so the
#      rustc sysroot path differs), a hard-linked wasi-sdk inside the checkout, target dir
#      target/web_s11/b_a_much_longer_target_directory_name
#   c: work/c/kp on a tmpfs mounted at work/c (root needed; mounted and unmounted here): the same
#      files, but the directory enumeration order differs from ext4's hash order (tmpfs lists the
#      Lua sources in reverse creation order), so lua-src's unsorted fs::read_dir compiles and
#      archives the 33 Lua objects in a different order; CARGO_HOME etc. as a, target web_s11/c
#   d: as a (ext4) but with patches/0001-lua-src-sorted-source-order.patch applied (work/d/kp)
#   e: as c (tmpfs order) with the same patch (work/c/kp_sorted); d_remap == e_remap shows the fix
# each without (plain) and with (remap) the remap recipe of RESULTS.md §4.4, and prints the sha256
# of the cargo output, the wasm-bindgen output, the final wasm-opt -Oz module and the JS glue.
# usage: scripts/repro_paths.sh [a_plain] [b_plain] [a_remap] [b_remap] [c_remap] [d_remap] [e_remap]
#   (default: all seven; about 6 minutes each on 2 jobs)
#   KEEP=1 keeps each target dir (default: removed after the variant, to save disk)
set -eu
S11=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
SPK=$(dirname "$S11")
W=$S11/work
TBASE=/home/user/kilohertz-pinball/target/web_s11
R=$S11/results/repro
mkdir -p "$R"
WB=$SPK/S1/tools/bin/wasm-bindgen
WO=$SPK/S9/tools/binaryen-version_132/bin/wasm-opt
[ -x "$WB" ] && [ -x "$WO" ] || { echo "missing $WB or $WO" >&2; exit 1; }
variants=${*:-a_plain b_plain a_remap b_remap c_remap d_remap e_remap}
PATCH=$S11/patches/0001-lua-src-sorted-source-order.patch

prepare() { # create the checkout copies if missing (S9's scratch tree is the source; never edited)
  [ -d "$W/a/kp/crates" ] || { mkdir -p "$W/a/kp"; cp -a "$SPK/S9/repo/." "$W/a/kp/"; }
  bh=$W/b/Users/macuser; bc=$bh/src/kilohertz-pinball-web-checkout
  [ -d "$bc/crates" ] || { mkdir -p "$bc"; cp -a "$SPK/S9/repo/." "$bc/"; }
  [ -d "$bh/.rustup/toolchains/1.97.0-x86_64-unknown-linux-gnu" ] || {
    mkdir -p "$bh/.rustup/toolchains"; cp -al "$(rustc --print sysroot)" "$bh/.rustup/toolchains/1.97.0-x86_64-unknown-linux-gnu"; }
  [ -d "$bc/target/web-tools/wasi-sdk-32.0-x86_64-linux" ] || {
    mkdir -p "$bc/target/web-tools"; cp -al "$SPK/S2/tools/wasi-sdk/wasi-sdk-32.0-x86_64-linux" "$bc/target/web-tools/"; }
}
mount_c() {
  mkdir -p "$W/c"
  if ! df -T "$W/c" | tail -1 | grep -q tmpfs; then mount -t tmpfs -o size=512m s11tmpfs "$W/c"; fi
  [ -d "$W/c/kp/crates" ] || { mkdir -p "$W/c/kp"; cp -a "$SPK/S9/repo/." "$W/c/kp/"; }
  echo "c: Lua source order on tmpfs: $(ls -f "$W/c/kp/third_party/lua-src/lua-5.4.9" | grep '\.c$' | tr '\n' ' ')"
}
patched_copy() { # dir: a copy of S9's tree with the sorted-order patch
  [ -d "$1/crates" ] || { mkdir -p "$1"; cp -a "$SPK/S9/repo/." "$1/"; patch -s -p1 -d "$1" < "$PATCH"; }
  grep -q 'paths.sort();' "$1/third_party/lua-src/src/lib.rs" || { echo "patch not applied in $1" >&2; return 1; }
}
prepare

run_variant() {
  v=$1
  case $v in
    a_*) ROOT=$W/a/kp; CH=/root/.cargo; HOMEV=$HOME; USERV=${USER:-root}; TMPV=${TMPDIR:-/tmp}
         TC=$(rustc --print sysroot); TD=$TBASE/a; WASI=$SPK/S2/tools/wasi-sdk/wasi-sdk-32.0-x86_64-linux ;;
    c_*) mount_c; ROOT=$W/c/kp; CH=/root/.cargo; HOMEV=$HOME; USERV=${USER:-root}; TMPV=${TMPDIR:-/tmp}
         TC=$(rustc --print sysroot); TD=$TBASE/c; WASI=$SPK/S2/tools/wasi-sdk/wasi-sdk-32.0-x86_64-linux ;;
    d_*) patched_copy "$W/d/kp"; ROOT=$W/d/kp; CH=/root/.cargo; HOMEV=$HOME; USERV=${USER:-root}; TMPV=${TMPDIR:-/tmp}
         TC=$(rustc --print sysroot); TD=$TBASE/d; WASI=$SPK/S2/tools/wasi-sdk/wasi-sdk-32.0-x86_64-linux ;;
    e_*) mount_c; patched_copy "$W/c/kp_sorted"; ROOT=$W/c/kp_sorted; CH=/root/.cargo; HOMEV=$HOME; USERV=${USER:-root}
         TMPV=${TMPDIR:-/tmp}; TC=$(rustc --print sysroot); TD=$TBASE/e; WASI=$SPK/S2/tools/wasi-sdk/wasi-sdk-32.0-x86_64-linux ;;
    b_*) HOMEV=$W/b/Users/macuser; ROOT=$HOMEV/src/kilohertz-pinball-web-checkout; CH=$HOMEV/.cargo
         USERV=macuser; TMPV=$W/b/tmp; TC=$HOMEV/.rustup/toolchains/1.97.0-x86_64-unknown-linux-gnu
         TD=$TBASE/b_a_much_longer_target_directory_name; WASI=$ROOT/target/web-tools/wasi-sdk-32.0-x86_64-linux ;;
    *) echo "unknown variant $v" >&2; return 2 ;;
  esac
  mkdir -p "$TMPV" "$CH"
  base_rf="-C target-feature=+simd128 -C link-arg=-zstack-size=4194304"
  SYSROOT_SRC=$TC/lib/rustlib/src/rust
  COMMIT=$("$TC/bin/rustc" -vV | sed -n 's/^commit-hash: //p')
  case $v in
    *_plain) rf=$base_rf; cflags_set=0 ;;
    *_remap) # the recipe: every host path that can reach the module, mapped to a fixed spelling
      rf="$base_rf --remap-path-prefix=$ROOT=/khz --remap-path-prefix=$CH=/cargo --remap-path-prefix=$SYSROOT_SRC=/rustc/$COMMIT --remap-path-prefix=$TD=/khz/target"
      cflags_set=1 ;;
  esac
  log=$R/$v.log
  {
    echo "== variant $v"; echo "ROOT=$ROOT"; echo "CARGO_HOME=$CH"; echo "HOME=$HOMEV USER=$USERV TMPDIR=$TMPV"
    echo "toolchain=$TC"; echo "target=$TD"; echo "wasi-sdk=$WASI"; echo "RUSTFLAGS=$rf"
    [ $cflags_set = 1 ] && echo "CFLAGS_wasm32_unknown_unknown=-ffile-prefix-map=$ROOT=/khz -ffile-prefix-map=$CH=/cargo -ffile-prefix-map=$TD=/khz/target"
    "$TC/bin/rustc" -vV
  } > "$log"
  t0=$(date +%s)
  (
    cd "$ROOT"
    export HOME="$HOMEV" USER="$USERV" LOGNAME="$USERV" TMPDIR="$TMPV" CARGO_HOME="$CH" CARGO_TARGET_DIR="$TD"
    export PATH="$TC/bin:$PATH" RUSTC="$TC/bin/rustc" RUSTDOC="$TC/bin/rustdoc"
    export CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_TEST_DEBUG=line-tables-only
    export CC_wasm32_unknown_unknown="$WASI/bin/clang" AR_wasm32_unknown_unknown="$WASI/bin/llvm-ar"
    export KHZ_WASI_SYSROOT="$WASI/share/wasi-sysroot" KHZ_WASM_LEGACY_EH=true KHZ_LUA_MATH=1
    export RUSTFLAGS="$rf"
    export CARGO_PROFILE_RELEASE_OPT_LEVEL=3 CARGO_PROFILE_RELEASE_LTO=fat CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
    export CARGO_PROFILE_RELEASE_PANIC=abort CARGO_PROFILE_RELEASE_DEBUG=false CARGO_PROFILE_RELEASE_STRIP=false
    export CC_ENABLE_DEBUG_OUTPUT=1
    if [ $cflags_set = 1 ]; then
      export CFLAGS_wasm32_unknown_unknown="-ffile-prefix-map=$ROOT=/khz -ffile-prefix-map=$CH=/cargo -ffile-prefix-map=$TD=/khz/target"
    fi
    "$TC/bin/cargo" build --locked --release --target wasm32-unknown-unknown -p pin_web
  ) >> "$log" 2>&1 || { echo "BUILD FAILED $v (see $log)"; tail -20 "$log"; return 1; }
  t1=$(date +%s)
  o=$R/$v; rm -rf "$o"; mkdir -p "$o"
  cp "$TD/wasm32-unknown-unknown/release/pin_web.wasm" "$o/pin_web.cargo.wasm"
  # the exact clang command lines cc-rs ran for Lua (CC_ENABLE_DEBUG_OUTPUT=1)
  cp "$(ls -d "$TD"/wasm32-unknown-unknown/release/build/mlua-sys-*/output | head -1)" "$o/mlua-sys.build-output.txt"
  "$WB" --target web --out-name pin_web --out-dir "$o" "$o/pin_web.cargo.wasm" >> "$log" 2>&1
  t2=$(date +%s)
  "$WO" -Oz --enable-simd --enable-exception-handling --enable-bulk-memory \
    --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals \
    --enable-reference-types --enable-multivalue --strip-debug --strip-producers \
    "$o/pin_web_bg.wasm" -o "$o/pin_web_bg.opt.wasm" >> "$log" 2>&1
  t3=$(date +%s)
  echo "timings: cargo $((t1 - t0)) s, wasm-bindgen $((t2 - t1)) s, wasm-opt $((t3 - t2)) s" >> "$log"
  for f in pin_web.cargo.wasm pin_web_bg.wasm pin_web_bg.opt.wasm pin_web.js; do
    printf '%s %s %s %s\n' "$v" "$f" "$(sha256sum < "$o/$f" | cut -c1-64)" "$(wc -c < "$o/$f" | tr -d ' ')"
  done | tee -a "$R/SHA256.txt"
  [ "${KEEP:-0}" = 1 ] || rm -rf "$TD"
}

for v in $variants; do
  sed -i.bak "/^$v /d" "$R/SHA256.txt" 2>/dev/null || true
  run_variant "$v"
done
rm -f "$R/SHA256.txt.bak"
if df -T "$W/c" 2>/dev/null | tail -1 | grep -q tmpfs; then umount "$W/c"; fi
echo "== sha256 (variant file sha256 bytes)"; sort "$R/SHA256.txt"
