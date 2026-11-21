# shellcheck shell=sh
# Every flag and variable the web engine module needs (S9 BOM §3), for any supported build host.
# S11 portable rewrite of S9's wasm_env.sh; proposed scripts/web/wasm_env.sh (W20).
# Sourced by build_web.sh after lib.sh and <tools>/tools.env. Inputs:
#   KHZ_WASI_SDK   wasi-sdk 32.0 root (tools.env)
#   KHZ_WEB_REPO   physical path of the workspace root
#   KHZ_WEB_REMAP  1 (default) = map host paths to fixed spellings, 0 = S9's exact flags
# Output: exported variables, plus KHZ_ENV_UNSET = names build_web.sh removes with `env -u`
# (hyphenated names cannot be unset by a POSIX shell).

[ -n "${KHZ_WASI_SDK:-}" ] || khz_die "KHZ_WASI_SDK not set (run fetch_tools.sh, then source tools.env)"
[ -n "${KHZ_WEB_REPO:-}" ] || khz_die "KHZ_WEB_REPO not set"

# Host C/C++ settings must not reach the wasm32 clang: cc-rs 1.11 appends CFLAGS, TARGET_CFLAGS,
# CFLAGS_wasm32_unknown_unknown and CFLAGS_wasm32-unknown-unknown together (envflags), and clang
# reads CPATH/C_INCLUDE_PATH/... and CCC_OVERRIDE_OPTIONS. A Homebrew-style CFLAGS="-I/opt/homebrew/include"
# or CPATH would otherwise change the Lua objects. RUSTFLAGS from the shell is replaced, not merged.
for khz_v in CFLAGS CXXFLAGS CPPFLAGS TARGET_CFLAGS TARGET_CXXFLAGS ARFLAGS TARGET_ARFLAGS \
  CFLAGS_wasm32_unknown_unknown CXXFLAGS_wasm32_unknown_unknown ARFLAGS_wasm32_unknown_unknown \
  TARGET_CC TARGET_AR CPATH C_INCLUDE_PATH CPLUS_INCLUDE_PATH OBJC_INCLUDE_PATH OBJCPLUS_INCLUDE_PATH \
  CCC_OVERRIDE_OPTIONS COMPILER_PATH CRATE_CC_NO_DEFAULTS CC_SHELL_ESCAPED_FLAGS CC_FORCE_DISABLE \
  RUSTFLAGS CARGO_BUILD_RUSTFLAGS CARGO_TARGET_WASM32_UNKNOWN_UNKNOWN_RUSTFLAGS CARGO_ENCODED_RUSTFLAGS \
  CARGO_BUILD_TARGET CARGO_PROFILE_RELEASE_BUILD_OVERRIDE_DEBUG; do
  unset "$khz_v"
done
# shellcheck disable=SC2034  # read by build_web.sh after sourcing
KHZ_ENV_UNSET="CC_wasm32-unknown-unknown AR_wasm32-unknown-unknown CFLAGS_wasm32-unknown-unknown CXXFLAGS_wasm32-unknown-unknown ARFLAGS_wasm32-unknown-unknown"

# Lua C (S2 route b): wasi-sdk 32 clang + wasi-libc sysroot, legacy Wasm EH setjmp/longjmp.
export CC_wasm32_unknown_unknown="$KHZ_WASI_SDK/bin/clang"
export AR_wasm32_unknown_unknown="$KHZ_WASI_SDK/bin/llvm-ar"
export KHZ_WASI_SYSROOT="$KHZ_WASI_SDK/share/wasi-sysroot"
export KHZ_WASM_LEGACY_EH=true
export KHZ_LUA_MATH=1

# Profile (S3/S9): opt-level 3, 1 CGU, panic=abort, no debug info, LTO as S9 set it (cargo
# ignores it for the [cdylib, rlib] crate, S9 C7; W20 declares lto = false in the manifest).
export CARGO_PROFILE_RELEASE_OPT_LEVEL=3
export CARGO_PROFILE_RELEASE_LTO=fat
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export CARGO_PROFILE_RELEASE_PANIC=abort
export CARGO_PROFILE_RELEASE_DEBUG=false
export CARGO_PROFILE_RELEASE_STRIP=false
export CARGO_INCREMENTAL=0

# Target rustflags (S1: +simd128 for glam parity; 4 MiB shadow stack), passed as
# CARGO_ENCODED_RUSTFLAGS (0x1f-separated) so that paths with spaces survive.
khz_us=$(printf '\037')
khz_rf="-C${khz_us}target-feature=+simd128${khz_us}-C${khz_us}link-arg=-zstack-size=4194304"

# Host-path remap (S11 §4): the module embeds panic-location paths of registry crates
# ($CARGO_HOME/registry/src/...), so the bytes depend on where CARGO_HOME is unless remapped.
# Later --remap-path-prefix entries win in rustc, so the most specific prefixes come last.
if [ "${KHZ_WEB_REMAP:-1}" = 1 ]; then
  khz_ch=${CARGO_HOME:-$HOME/.cargo}
  khz_td=${CARGO_TARGET_DIR:-$KHZ_WEB_REPO/target}
  khz_sysroot=$(rustc --print sysroot)
  khz_commit=$(rustc -vV | sed -n 's/^commit-hash: //p')
  khz_map() { # from to: add the given and the physical spelling of a directory
    khz_rf="$khz_rf${khz_us}--remap-path-prefix=$1=$2"
    if [ -d "$1" ]; then
      khz_p=$(khz_abs_dir "$1")
      [ "$khz_p" = "$1" ] || khz_rf="$khz_rf${khz_us}--remap-path-prefix=$khz_p=$2"
    fi
  }
  khz_map "$khz_sysroot/lib/rustlib/src/rust" "/rustc/$khz_commit"
  khz_map "$KHZ_WEB_REPO" /khz
  khz_map "$khz_td" /khz/target
  khz_map "$khz_ch" /cargo
  # C side: no -g and no __FILE__ in the Lua build today; map anyway so a future assert stays
  # portable. Shell-quoted (CC_SHELL_ESCAPED_FLAGS=1) so that paths with spaces survive.
  export CC_SHELL_ESCAPED_FLAGS=1
  export CFLAGS_wasm32_unknown_unknown="'-ffile-prefix-map=$KHZ_WEB_REPO=/khz' '-ffile-prefix-map=$khz_td=/khz/target' '-ffile-prefix-map=$khz_ch=/cargo'"
fi
export CARGO_ENCODED_RUSTFLAGS="$khz_rf"
