#!/bin/sh
# Rebuilds rules.wasm from parity_guest/ (needs `rustup target add
# wasm32-unknown-unknown`). Paths are remapped so the bytes do not depend on
# where the repository is checked out; tests/wasm_parity.rs repeats this build
# and compares the bytes when the target is installed.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
sdk=$(cd "$here/../../../guest-sdk" && pwd)
target="${CARGO_TARGET_DIR:-$here/parity_guest/target}"
unset CARGO_ENCODED_RUSTFLAGS
RUSTFLAGS="--remap-path-prefix=$sdk=/pin_guest_sdk --remap-path-prefix=$here/parity_guest=/parity_guest" \
  cargo build --release --offline --target wasm32-unknown-unknown \
  --manifest-path "$here/parity_guest/Cargo.toml" --target-dir "$target"
cp "$target/wasm32-unknown-unknown/release/parity_rules.wasm" "$here/rules.wasm"
