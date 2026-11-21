#!/bin/sh
# S3 TESTS REQUIRED: runs the render probe pages (WebGPU, then WebGL2) in
# Playwright's headless Chromium and prints per-backend status plus the
# screenshot paths. Needs the wasm-bindgen output in web/pkg/<pkg>/
# (build_wasm.sh <pkg> <profile> bindgen), the zipped bundles in bundles/
# and the frame dumps in dumps/ (s3_dump).
#   run_render_probe.sh [pkg=both] [flags=minimal]
set -eu
here=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
pkg=${1:-both}
flags=${2:-minimal}
export NODE_PATH=${NODE_PATH:-/usr/local/lib/node_modules_global}:/opt/node-tools/node_modules
cd "$here/node"
node run_probe.js render --pkg "$pkg" --flags "$flags" --backends webgpu,webgl2
echo "screenshots:"
ls -1 "$here"/shots/web_*_"${pkg}_${flags}"_*.png
