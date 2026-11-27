#!/bin/sh
# The windowed latency harness (KP-002, SPEC §7.5; docs/DEVELOPMENT.md
# "Windowed latency harness"): builds the grey-box bundle and the release
# pin_app, then runs the 60 s driver (crates/pin_app/tests/windowed) under
# Xvfb on lavapipe:
#   scripts/windowed/run.sh [<out_dir>]
#   scripts/windowed/run.sh --print-paths [<out_dir>]
# --print-paths prints the resolved target, out, bundle and app paths (one
# `name=path` per line) and exits before building anything; it creates and
# deletes nothing. CARGO_BUILD_TARGET is rejected (exit 2): it moves the
# release app under <target>/<triple>/release, and the app is measured at
# <target>/release/pin_app.
# A run first clears the previous run's artifacts from <out_dir>; a setup
# failure (missing tool, grey-box build, cargo build) or a cargo failure to
# compile or launch the windowed test target leaves a
# `verdict: FAIL (windowed setup: ...)` summary.txt there, never a stale PASS.
# The fallback never overwrites a summary the driver wrote, and cargo's exit
# status is the script's exit status.
# KP_GREYBOX_BUNDLE overrides the bundle (default <target>/greybox) that
# tables/greybox/build.sh writes and the driver loads; --print-paths shows it.
# A relative CARGO_TARGET_DIR, KP_GREYBOX_BUNDLE or <out_dir> is taken relative to the
# repository root, and the absolute CARGO_TARGET_DIR is what Cargo sees.
# Environment: LP_NUM_THREADS (default 2, the documented cap), KP_GREYBOX_BUNDLE,
# KP_WINDOWED_RUN_MS (default 60000), KP_WINDOWED_SEED,
# KP_WINDOWED_FRAME_MS, CARGO_TARGET_DIR.
# Artifacts in <out_dir> (default <target>/windowed): latency_trace.csv,
# pin_app_stderr.log, summary.txt, frame_*.png. Exits non-zero when a check
# fails. Orchestration only: the checks live in the Rust test target.
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
print_paths=0
if [ "${1:-}" = "--print-paths" ]; then
  print_paths=1
  shift
fi
target=${CARGO_TARGET_DIR:-$root/target}
case $target in
  /*) ;;
  *) target=$root/$target ;;
esac
out=${1:-$target/windowed}
case $out in
  /*) ;;
  *) out=$root/$out ;;
esac
bundle=${KP_GREYBOX_BUNDLE:-$target/greybox}
case $bundle in
  /*) ;;
  *) bundle=$root/$bundle ;;
esac
app=$target/release/pin_app
if [ -n "${CARGO_BUILD_TARGET:-}" ]; then
  echo "run.sh: CARGO_BUILD_TARGET is not supported (the app is measured at $target/release/pin_app); unset it" >&2
  exit 2
fi
if [ "$print_paths" = 1 ]; then
  printf 'target=%s\nout=%s\nbundle=%s\napp=%s\n' "$target" "$out" "$bundle" "$app"
  exit 0
fi
export CARGO_TARGET_DIR="$target"
# Publish a failure summary if anything below fails before the driver runs;
# the trap is installed before the cleanup so a cleanup failure is reported too.
mkdir -p "$out"
stage="cleanup"
cleaned=0
setup_failed() {
  status=$?
  # Before the cleanup finished, any summary is the previous run's: overwrite it.
  if [ "$status" -ne 0 ] && { [ "$cleaned" = 0 ] || [ ! -s "$out/summary.txt" ]; }; then
    printf 'verdict: FAIL (windowed setup: %s)\n' "$stage" >"$out/summary.txt" 2>/dev/null || :
  fi
}
trap setup_failed EXIT
# Drop the previous run's artifacts (the driver removes the same list). The old
# summary goes first and alone, so a later failure never leaves it behind.
rm -rf -- "$out/summary.txt"
rm -rf -- "$out/latency_trace.csv" "$out/pin_app_stderr.log"
for f in "$out"/frame_*.png; do
  [ -e "$f" ] || [ -L "$f" ] || continue
  rm -rf -- "$f"
done
cleaned=1
for tool in Xvfb xdotool xwd convert; do
  stage="$tool not found"
  command -v "$tool" >/dev/null 2>&1 || {
    echo "run.sh: $tool not found (apt-get install xvfb xdotool x11-apps imagemagick)" >&2
    exit 2
  }
done
stage="tables/greybox/build.sh failed"
tables/greybox/build.sh "$bundle"
stage="cargo build --release -p pin_app failed"
cargo build --release -p pin_app
# The driver starts its own Xvfb when DISPLAY is unset.
unset DISPLAY
export LP_NUM_THREADS="${LP_NUM_THREADS:-2}"
export WGPU_BACKEND=vulkan
export PIN_FORCE_FALLBACK_ADAPTER=1
export SDL_VIDEODRIVER=x11
export KP_GREYBOX_BUNDLE="$bundle"
export KP_WINDOWED_OUT="$out"
export KP_PIN_APP="$app"
# The driver owns summary.txt once it runs: the EXIT trap writes the fallback
# only when no summary exists, and it does not change the exit status.
stage="cargo test --release -p pin_app --test windowed (build or launch failed)"
cargo test --release -p pin_app --test windowed -- --ignored --nocapture
