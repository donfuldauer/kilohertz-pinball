#!/bin/sh
# Rebuilds tables/feature_lab/rules.wasm from this crate (needs `rustup target add
# wasm32-unknown-unknown`). Paths are remapped so the bytes do not depend on
# where the repository is checked out (the model is
# crates/pin_script/tests/fixtures/wasm/rebuild.sh). CI
# (.github/workflows/rules-wasm.yml) builds into a scratch file and cmp's it
# with the committed rules.wasm:
#   rebuild.sh             writes ../rules.wasm
#   rebuild.sh <out.wasm>  writes <out.wasm> instead
set -eu
here=$(cd "$(dirname "$0")" && pwd)
sdk=$(cd "$here/../../../crates/pin_script/guest-sdk" && pwd)
out="${1:-$here/../rules.wasm}"
target="${CARGO_TARGET_DIR:-$here/target}"
# Flags separated by 0x1f, so checkout paths with spaces stay one argument.
unset RUSTFLAGS
CARGO_ENCODED_RUSTFLAGS=$(printf '%s\037%s' \
  "--remap-path-prefix=$sdk=/pin_guest_sdk" \
  "--remap-path-prefix=$here=/feature_lab_rules") \
  cargo build --release --offline --locked --target wasm32-unknown-unknown \
  --manifest-path "$here/Cargo.toml" --target-dir "$target"
cp "$target/wasm32-unknown-unknown/release/feature_lab_rules.wasm" "$out"
