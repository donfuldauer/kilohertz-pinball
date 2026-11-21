# Source me: S7 cargo env (COMMON env with the S7 overrides: 1 job, target/web_s7).
export S7=${WEB_SPIKES}/S7
export S1=${WEB_SPIKES}/S1
export CARGO_BUILD_JOBS=1
export CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/web_s7
export CARGO_INCREMENTAL=0
export CARGO_PROFILE_DEV_DEBUG=line-tables-only
export CARGO_PROFILE_TEST_DEBUG=line-tables-only
# aarch64 cross: linker, C compiler (vendored Lua, zstd), runner (qemu-user).
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc
export CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_RUNNER="qemu-aarch64 -L /usr/aarch64-linux-gnu"
export CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc
export AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar
export CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++
export QEMU_LD_PREFIX=/usr/aarch64-linux-gnu
export PATH=$S1/tools/bin:$PATH
