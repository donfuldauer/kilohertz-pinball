#!/bin/sh
# Builds the Sky Deck table bundle (Subphase 6.2, variant B) into <out_dir>:
#   tables/sky_deck/build.sh <out_dir>
# Run from anywhere inside the repository. Steps: pin_tablegen generates
# playfield.glb and the merged table.toml, the rules, backglass, layout
# (pin_sim reads its wall_height), glass.toml (the glass underside height)
# and display scenes are copied unchanged, pin_assetgen writes the
# placeholder kit (sounds, fonts, backglass art), and pin_tablegen validate
# checks the clearances.
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
# The Wasm port of the rules (Subphase 6.3) rides along on request
# (PIN_BUNDLE_RULES_WASM=1) when it is committed; table.toml still selects
# rules.lua. Not by default: the bundle must stay file-for-file equal to the
# in-process helper bundle (docs/decisions/2026-11-19-6.3.md).
if [ "${PIN_BUNDLE_RULES_WASM:-0}" = 1 ] && [ -f "$src/rules.wasm" ]; then
  cp "$src/rules.wasm" "$out/"
else
  rm -f "$out/rules.wasm"
fi
cargo run -q -p pin_assetgen -- --title "Sky Deck" --palette "#0b1d3a,#3fa7d6,#b388ff" --out "$out"
cargo run -q -p pin_tablegen -- validate "$out"
echo "sky_deck bundle: $out"
