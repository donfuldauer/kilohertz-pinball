# Source me: COMMON cargo env for spike S8 (web-spike-lua-audit).
export S=${WEB_SPIKES}/S8
export S1=${WEB_SPIKES}/S1
export S2=${WEB_SPIKES}/S2
export S4=${WEB_SPIKES}/S4
export CARGO_BUILD_JOBS=2
export CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/web_s8
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=line-tables-only
export CARGO_PROFILE_TEST_DEBUG=line-tables-only
# read-only use of S2's tool dirs (wasm-bindgen 0.2.130, wasm-tools 1.263.0 copied from S1)
export PATH=$S2/tools/bin:$PATH
