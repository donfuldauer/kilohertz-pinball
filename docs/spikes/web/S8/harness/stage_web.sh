#!/bin/sh
# S8: stages the job page + a variant's pkg_web + S1 bundles/recordings/native hashes.
# usage: stage_web.sh <variant> <web_dir> <jobs.json>
set -eu
S=${WEB_SPIKES}/S8
S1=$S/../S1
V=$1; W=$2; J=$3
rm -rf "$W"; mkdir -p "$W/data"; cp "$S/web/index.html" "$S/web/main.js" "$W/"; cp -r "$S/runs/bin/$V/pkg_web" "$W/pkg"; cp "$J" "$W/jobs.json"
for t in apex_circuit sky_deck feature_lab replay_table; do cp "$S1/bundles/$t.pinbundle" "$W/data/"; done
for f in "$S1"/rec/*.scn "$S1"/rec/*.scr "$S1"/native/*.hashes; do ln -s "$f" "$W/data/$(basename "$f")"; done
