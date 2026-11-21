# Source me: COMMON cargo env for spike S9 (web-spike-integration).
export S9=${WEB_SPIKES}/S9
export SPK=${WEB_SPIKES}
export S1=$SPK/S1 S2=$SPK/S2 S3=$SPK/S3 S4=$SPK/S4
export CARGO_BUILD_JOBS=2
export CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/web_s9
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=line-tables-only
export CARGO_PROFILE_TEST_DEBUG=line-tables-only
# wasm-bindgen CLI 0.2.130 + wasm-tools 1.263.0 (S1's cargo-install'ed binaries, read-only use).
export PATH=$S1/tools/bin:$PATH
export NODE_PATH=${NODE_PATH:-/usr/local/lib/node_modules_global}
