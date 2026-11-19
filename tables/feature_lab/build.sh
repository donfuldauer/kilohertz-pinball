#!/bin/sh
# Builds the Feature Lab table bundle (Subphase 6.4) into <out_dir>:
#   tables/feature_lab/build.sh <out_dir>
# Run from anywhere inside the repository; CARGO_TARGET_DIR is inherited.
# The pin_tablegen command line cannot generate the backbox, so the
# generation runs in the build_feature_lab example (pin_app), through the
# pin_tablegen library: layout.toml -> playfield.glb, backbox_layout.toml ->
# backbox.glb + backbox_colliders.glb, both merged into one table.toml; the
# rules, backglass, layout, glass and display scenes are copied and
# pin_assetgen's placeholder kit (title "Feature Lab") is written. Then
# pin_tablegen validate checks the clearances of every declared ball.
set -eu
if [ "$#" -ne 1 ]; then
  echo "usage: $0 <out_dir>" >&2
  exit 2
fi
mkdir -p "$1"
out=$(cd "$1" && pwd)
cd "$(dirname "$0")"
cargo run -q -p pin_app --example build_feature_lab -- "$out"
# The Wasm port of the rules (Subphase 6.3) rides along when it is
# committed; table.toml still selects rules.lua, so it is used only by a
# bundle whose script_entry names rules.wasm.
if [ -f rules.wasm ]; then
  cp rules.wasm "$out/"
fi
cargo run -q -p pin_tablegen -- validate "$out"
echo "feature_lab bundle: $out"
