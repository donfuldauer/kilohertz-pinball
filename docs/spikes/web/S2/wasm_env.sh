# Source after env.sh: C toolchain for the wasm32-unknown-unknown Lua build (route b, wasi-sdk 32).
export WASI_SDK=$S/tools/wasi-sdk/wasi-sdk-32.0-x86_64-linux
export CC_wasm32_unknown_unknown=$WASI_SDK/bin/clang
export AR_wasm32_unknown_unknown=$WASI_SDK/bin/llvm-ar
export KHZ_WASI_SYSROOT=$WASI_SDK/share/wasi-sysroot
# S1's recommended web flags (+simd128 for glam parity, 4 MiB shadow stack).
export S2_WASM_RUSTFLAGS="-C target-feature=+simd128 -C link-arg=-zstack-size=4194304"
