#!/bin/sh
# Stages the S2 page + pkg_web + S1's bundles/recordings/native hashes into runs/web.
set -eu
S=${WEB_SPIKES}/S2
S1=$S/../S1
# usage: stage_web.sh [pkg_dir] [web_dir]
PKG=${1:-$S/runs/pkg_web}
W=${2:-$S/runs/web}; rm -rf "$W"; mkdir -p "$W/data"
cp "$S/web/index.html" "$S/web/main.js" "$W/"; cp -r "$PKG" "$W/pkg"
for t in apex_circuit sky_deck feature_lab; do
  cp "$S1/bundles/$t.pinbundle" "$W/data/"
  for k in bot60 mb8; do
    cp "$S1/rec/${t}_$k.scn" "$S1/rec/${t}_$k.scr" "$W/data/"
    cp "$S1/native/${t}_$k.hashes" "$W/data/${t}_$k.hashes"
  done
done
