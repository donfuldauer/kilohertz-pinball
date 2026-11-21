#!/bin/sh
# The windowed latency harness (KP-002, SPEC §7.5; docs/DEVELOPMENT.md
# "Windowed latency harness"): builds the grey-box bundle and the release
# pin_app, then runs the 60 s driver (crates/pin_app/tests/windowed) under
# Xvfb on lavapipe:
#   scripts/windowed/run.sh [<out_dir>]
#   scripts/windowed/run.sh --print-paths [<out_dir>]
# --print-paths prints the resolved target, out, bundle and app paths (one
# `name=path` per line) and exits before building anything.
# A relative CARGO_TARGET_DIR or <out_dir> is taken relative to the
# repository root, and the absolute CARGO_TARGET_DIR is what Cargo sees.
# Environment: LP_NUM_THREADS (default 2, the documented cap),
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
bundle=$target/greybox
app=$target/release/pin_app
if [ "$print_paths" = 1 ]; then
  printf 'target=%s\nout=%s\nbundle=%s\napp=%s\n' "$target" "$out" "$bundle" "$app"
  exit 0
fi
export CARGO_TARGET_DIR="$target"
for tool in Xvfb xdotool xwd convert; do
  command -v "$tool" >/dev/null 2>&1 || {
    echo "run.sh: $tool not found (apt-get install xvfb xdotool x11-apps imagemagick)" >&2
    exit 2
  }
done
tables/greybox/build.sh "$bundle"
cargo build --release -p pin_app
mkdir -p "$out"
# The driver starts its own Xvfb when DISPLAY is unset.
unset DISPLAY
export LP_NUM_THREADS="${LP_NUM_THREADS:-2}"
export WGPU_BACKEND=vulkan
export PIN_FORCE_FALLBACK_ADAPTER=1
export SDL_VIDEODRIVER=x11
export KP_GREYBOX_BUNDLE="$bundle"
export KP_WINDOWED_OUT="$out"
export KP_PIN_APP="$app"
exec cargo test --release -p pin_app --test windowed -- --ignored --nocapture
