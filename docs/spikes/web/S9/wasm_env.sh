# Source after env.sh: every flag/env the combined engine module needs (S9 BOM).
# Lua C (S2 route b): wasi-sdk 32 clang 22.1.0 + wasi-libc sysroot, legacy Wasm EH sjlj.
export WASI_SDK=$S2/tools/wasi-sdk/wasi-sdk-32.0-x86_64-linux
export CC_wasm32_unknown_unknown=$WASI_SDK/bin/clang
export AR_wasm32_unknown_unknown=$WASI_SDK/bin/llvm-ar
export KHZ_WASI_SYSROOT=$WASI_SDK/share/wasi-sysroot
export KHZ_WASM_LEGACY_EH=true
# S2 recommended "umath": Lua transcendental math through Rust libm on every target (KHZ 32).
export KHZ_LUA_MATH=1
# S1: +simd128 (glam parity with x86_64 SSE2); S1/S2/S4: 4 MiB shadow stack.
export RUSTFLAGS="-C target-feature=+simd128 -C link-arg=-zstack-size=4194304"
# S3 size recommendation: opt-level 3, fat LTO, 1 CGU, panic=abort (target default), no debug info.
export CARGO_PROFILE_RELEASE_OPT_LEVEL=3
export CARGO_PROFILE_RELEASE_LTO=fat
export CARGO_PROFILE_RELEASE_CODEGEN_UNITS=1
export CARGO_PROFILE_RELEASE_PANIC=abort
export CARGO_PROFILE_RELEASE_DEBUG=false
export CARGO_PROFILE_RELEASE_STRIP=false
