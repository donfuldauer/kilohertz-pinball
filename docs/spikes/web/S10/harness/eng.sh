#!/bin/sh
# S10: run a shell runner on one engine/tier.
# usage: eng.sh <engine> <tier> <runner.js> [args...]
#   engine: jsc | sm160 | sm153 | sm160a | sm153a   (…a = linux-aarch64 build under qemu-aarch64)
#   tier:   jsc: default | ipint | bbq | omg | nojit
#           sm*: default | baseline | ion
#   runner: path of a .js runner (run after harness/shellenv.js), args are passed through.
# env: EXTRA="<more engine flags>" (e.g. stack options), ULIMIT_S=<KiB> (ulimit -s for the shell)
set -eu
S10=${WEB_SPIKES}/S10
T=$S10/tools
H=$S10/harness
E=$1; TIER=$2; RUNNER=$3; shift 3
case "$E:$TIER" in
  jsc:default) F="" ;;
  jsc:ipint)   F="--useBBQJIT=0 --useOMGJIT=0" ;;
  jsc:bbq)     F="--useWasmIPInt=0 --useOMGJIT=0" ;;
  jsc:omg)     F="--useWasmIPInt=0 --thresholdForOMGOptimizeAfterWarmUp=0 --thresholdForOMGOptimizeSoon=0" ;;
  jsc:nojit)   F="--useJIT=0" ;;
  sm*:default) F="" ;;
  sm*:baseline) F="--wasm-compiler=baseline" ;;
  sm*:ion)     F="--wasm-compiler=optimizing" ;;
  *) echo "eng.sh: unknown engine/tier $E:$TIER" >&2; exit 2 ;;
esac
F="$F ${EXTRA:-}"
[ -n "${ULIMIT_S:-}" ] && ulimit -s "$ULIMIT_S"
cd /tmp
case "$E" in
  jsc)    exec "$T/jsc_327409/jsc" $F "$H/shellenv.js" "$RUNNER" -- "$@" ;;
  sm160)  exec "$T/sm_160.0.1_x64/js" $F -f "$H/shellenv.js" "$RUNNER" "$@" ;;
  sm153)  exec "$T/sm_153.7.0esr_x64/js" $F -f "$H/shellenv.js" "$RUNNER" "$@" ;;
  sm160a) exec qemu-aarch64 -L /usr/aarch64-linux-gnu -E LD_LIBRARY_PATH="$T/sm_160.0.1_arm64" "$T/sm_160.0.1_arm64/js" $F -f "$H/shellenv.js" "$RUNNER" "$@" ;;
  sm153a) exec qemu-aarch64 -L /usr/aarch64-linux-gnu -E LD_LIBRARY_PATH="$T/sm_153.7.0esr_arm64" "$T/sm_153.7.0esr_arm64/js" $F -f "$H/shellenv.js" "$RUNNER" "$@" ;;
  *) echo "eng.sh: unknown engine $E" >&2; exit 2 ;;
esac
