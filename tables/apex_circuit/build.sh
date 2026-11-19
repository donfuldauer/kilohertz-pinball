#!/bin/sh
# Builds the "Apex Circuit" table bundle (Subphase 6.1, variant A) into
# <out_dir>:
#   tables/apex_circuit/build.sh <out_dir>
# Run from anywhere inside the repository. The same steps as
# tables/greybox/build.sh: pin_tablegen generates playfield.glb and the
# merged table.toml, the rules, backglass, layout (pin_sim reads its
# wall_height), glass.toml (the glass underside height) and display scenes
# are copied, pin_assetgen writes the placeholder kit (sounds, fonts,
# backglass art; title "Apex Circuit", the palette of table.toml [meta]),
# and pin_tablegen validate checks the clearances.
set -eu
if [ "$#" -ne 1 ]; then
  echo "usage: $0 <out_dir>" >&2
  exit 2
fi
src=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
mkdir -p "$1"
out=$(CDPATH= cd -- "$1" && pwd)
cargo run -q -p pin_tablegen -- "$src/layout.toml" --table "$src/table.toml" --out "$out"
mkdir -p "$out/display"
cp "$src/rules.lua" "$src/backglass.toml" "$src/layout.toml" "$src/glass.toml" "$out/"
cp "$src"/display/*.toml "$out/display/"
# The Wasm port of the rules (Subphase 6.3) rides along when it is
# committed; table.toml still selects rules.lua, so it is used only by a
# bundle whose script_entry names rules.wasm.
if [ -f "$src/rules.wasm" ]; then
  cp "$src/rules.wasm" "$out/"
else
  rm -f "$out/rules.wasm"
fi
cargo run -q -p pin_assetgen -- --title "Apex Circuit" --palette "#0b132b,#1c2541,#ff6b35" --out "$out"
cargo run -q -p pin_tablegen -- validate "$out"
echo "apex_circuit bundle: $out"
