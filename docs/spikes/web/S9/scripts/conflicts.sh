#!/bin/sh
# S9 req 2 (f): reproduces each combination conflict found while integrating, and shows the
# minimal fix is what build_web.sh / the manifests do. A line "REPRODUCED" means the broken
# combination fails exactly as recorded in RESULTS.md; "FIXED" means the shipped combination
# does not have the problem. Exit 1 if any expectation does not hold.
# usage: scripts/conflicts.sh [quick]   (quick: skip C5, the clang-18 rebuild in its own target dir)
set -u
here=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
. "$here/env.sh"
. "$here/wasm_env.sh"
O=$here/results/conflicts; mkdir -p "$O"
cd "$here/repo"
bad=0
expect() { # name logfile pattern
  if grep -qE "$3" "$2"; then echo "REPRODUCED $1: $(grep -m1 -oE "$3" "$2")"; else echo "NOT-REPRODUCED $1 (see $2)"; bad=$((bad + 1)); fi
}
T="--target wasm32-unknown-unknown -p pin_web"

# C1: two #[global_allocator]s (s1_harness's CountingAlloc and s4_rules's PeakAlloc, both on by
# default in the spike crates) + pin_web's own. Fix: one allocator, defined by the top crate only.
cargo check $T --features s1_harness/global-alloc,s4_rules/global-alloc > "$O/c1_alloc.log" 2>&1
expect C1-duplicate-global-allocator "$O/c1_alloc.log" 'the `#\[global_allocator\]` in this crate conflicts with global allocator in: [a-z0-9_]+'

# C2: two crates exporting the same wasm-bindgen names (s1_harness and s4_rules both export
# play/bench/alloc_check/install_panic_hook). cargo's link SUCCEEDS (rlibs are archives: the first
# definition wins silently); wasm-bindgen 0.2.130 then refuses the module. Fix: library crates do
# not carry exports; only the engine crate does (feature-gated `exports` off).
bindgen_of() { rm -rf "$O/$1_pkg"; wasm-bindgen --target web --out-name pin_web --out-dir "$O/$1_pkg" \
  "$CARGO_TARGET_DIR/wasm32-unknown-unknown/release/pin_web.wasm" >> "$O/$1.log" 2>&1; echo "wasm-bindgen exit $?" >> "$O/$1.log"; rm -rf "$O/$1_pkg"; }
cargo build --release $T --features s1_harness/exports,s4_rules/exports > "$O/c2_exports.log" 2>&1 && echo "cargo build exit 0" >> "$O/c2_exports.log"
bindgen_of c2_exports
expect C2-duplicate-wasm-exports "$O/c2_exports.log" 'the name `[a-z_]+` is exported by multiple crates in this build'

# C3: web_probe's own exports in the same module (a second #[wasm_bindgen(start)] and a clashing
# `build_info`): same failure mode as C2.
cargo build --release $T --features web_probe/exports > "$O/c3_start.log" 2>&1 && echo "cargo build exit 0" >> "$O/c3_start.log"
bindgen_of c3_start
expect C3-clashing-export-and-second-start "$O/c3_start.log" '(the name `[a-z_]+` is exported by multiple crates in this build|start function)'

# C4: Lua C for wasm32-unknown-unknown without the wasi-libc sysroot (KHZ 29 needs it).
env -u KHZ_WASI_SYSROOT cargo check $T > "$O/c4_sysroot.log" 2>&1
expect C4-no-wasi-sysroot "$O/c4_sysroot.log" 'KHZ_WASI_SYSROOT must name a wasi-libc sysroot'

# C6: pin_script's default features (native-backends = wasmtime + notify) unified into the web
# build: wasmtime's dependencies do not build for wasm32-unknown-unknown (S1 R1).
cargo check $T --features pin_script/native-backends > "$O/c6_native_backends.log" 2>&1
expect C6-std-only-crates-via-feature-unification "$O/c6_native_backends.log" '(target OS is unknown|error\[E[0-9]+\].*(rustix|errno|wasmtime|cranelift|mmap)|could not compile `(errno|rustix|wasmtime[a-z_-]*|cranelift[a-z_-]*)`)'

# C7: cargo silently drops -C lto for a crate built as [cdylib, rlib] (rustc command has no
# -C lto although CARGO_PROFILE_RELEASE_LTO=fat); `cargo rustc --crate-type cdylib` applies it.
touch crates/pin_web/src/lib.rs
cargo build --release $T -v > "$O/c7_nolto.log" 2>&1
if grep -E 'Running .*--crate-name pin_web ' "$O/c7_nolto.log" | grep -q -- '-C lto'; then echo "NOT-REPRODUCED C7"; bad=$((bad + 1)); else echo "REPRODUCED C7-lto-not-applied-to-cdylib+rlib: rustc for pin_web has no -C lto with CARGO_PROFILE_RELEASE_LTO=$CARGO_PROFILE_RELEASE_LTO"; fi

if [ "${1:-}" != quick ]; then
  # C5: Lua C compiled by the system clang-18 (cc-rs default `clang`) instead of wasi-sdk 32: LLVM 18
  # lowers setjmp/longjmp to saveSetjmp/testSetjmp/getTempRet0 (S2 route a); rustc's wasm link does
  # NOT fail on undefined C symbols, it turns them into `env` imports -> the import audit fails.
  CT=$CARGO_TARGET_DIR/c5_clang18
  env CC_wasm32_unknown_unknown=clang CARGO_TARGET_DIR="$CT" \
    cargo build --release $T > "$O/c5_clang18.log" 2>&1
  # With S2's lua-src arm the wrong compiler fails loudly: clang-18 does not know the LLVM 19+
  # option the KHZ 29 arm passes. (Without that option, S2 route (a) showed LLVM 18 links
  # silently with env.saveSetjmp/testSetjmp/getTempRet0 imports: check_imports.sh is the guard.)
  expect C5-system-clang18-instead-of-wasi-sdk "$O/c5_clang18.log" "Unknown command line argument '-wasm-use-legacy-eh=true'"
  rm -rf "$CT" "$O/c5_pkg"
fi
# The shipped combination: the import audit passes and build_info reports the features.
"$here/check_imports.sh" > "$O/shipped_audit.log" 2>&1 && echo "FIXED shipped module: import audit PASS" || { echo "FAIL shipped module audit"; bad=$((bad + 1)); }
echo "== conflicts: $bad unexpected outcome(s)"
exit $([ $bad -eq 0 ] && echo 0 || echo 1)
