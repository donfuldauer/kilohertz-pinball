#!/bin/sh
# The windowed latency harness (KP-002, SPEC §7.5; docs/DEVELOPMENT.md
# "Windowed latency harness"): builds the grey-box bundle and the release
# pin_app, then runs the 60 s driver (crates/pin_app/tests/windowed) under
# Xvfb on lavapipe:
#   scripts/windowed/run.sh [<out_dir>]
# Environment: LP_NUM_THREADS (default 2, the documented cap),
# KP_WINDOWED_RUN_MS (default 60000), KP_WINDOWED_SEED, CARGO_TARGET_DIR.
# Artifacts in <out_dir> (default <target>/windowed): latency_trace.csv,
# pin_app_stderr.log, summary.txt, frame_*.png. Exits non-zero when a check
# fails. Orchestration only: the checks live in the Rust test target.
set -eu
root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$root"
target=${CARGO_TARGET_DIR:-$root/target}
out=${1:-$target/windowed}
bundle=$target/greybox
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
export SDL_VIDEODRIVER=x11
export KP_GREYBOX_BUNDLE="$bundle"
export KP_WINDOWED_OUT="$out"
export KP_PIN_APP="$target/release/pin_app"
exec cargo test --release -p pin_app --test windowed -- --ignored --nocapture
