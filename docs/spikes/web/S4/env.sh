# Source me: COMMON cargo env for spike S4.
export SP=${WEB_SPIKES}
export S4=$SP/S4
export S1=$SP/S1
export CARGO_BUILD_JOBS=2
export CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/web_s4
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=line-tables-only
export CARGO_PROFILE_TEST_DEBUG=line-tables-only
export PATH=$S1/tools/bin:$S4/tools/bin:$PATH
