# Common cargo environment for the S3 spike (COMMON block of the brief).
export S=${WEB_SPIKES}/S3
export CARGO_BUILD_JOBS=2
export CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/web_s3
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=line-tables-only
export CARGO_PROFILE_TEST_DEBUG=line-tables-only
export PATH=$S/tools/wbg-0.2.130/bin:$PATH
