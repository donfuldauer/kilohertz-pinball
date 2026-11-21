# Source me: COMMON cargo env for spike S1.
export S=${WEB_SPIKES}/S1
export CARGO_BUILD_JOBS=2
export CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/web_s1
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=line-tables-only
export CARGO_PROFILE_TEST_DEBUG=line-tables-only
export PATH=$S/tools/bin:$PATH
