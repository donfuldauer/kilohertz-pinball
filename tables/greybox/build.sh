#!/bin/sh
# Builds the grey-box table bundle (Subphase 6.0) into <out_dir>:
#   tables/greybox/build.sh <out_dir>
# Run from anywhere inside the repository. Steps: pin_tablegen generates
# playfield.glb and the merged table.toml, the rules, backglass, layout
# (pin_sim reads its wall_height), glass.toml (the glass underside height)
# and display scenes are copied, pin_assetgen writes the placeholder kit (sounds, fonts,
# backglass art), and pin_tablegen validate checks the clearances.
set -eu
if [ "$#" -ne 1 ]; then
  echo "usage: $0 <out_dir>" >&2
  exit 2
fi
src=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$1"
out=$(cd "$1" && pwd)
cargo run -q -p pin_tablegen -- "$src/layout.toml" --table "$src/table.toml" --out "$out"
mkdir -p "$out/display"
cp "$src/rules.lua" "$src/backglass.toml" "$src/layout.toml" "$src/glass.toml" "$out/"
cp "$src"/display/*.toml "$out/display/"
cargo run -q -p pin_assetgen -- --title "Grey Box" --palette "#2b2d42,#8d99ae,#ef233c" --out "$out"
cargo run -q -p pin_tablegen -- validate "$out"
echo "greybox bundle: $out"
