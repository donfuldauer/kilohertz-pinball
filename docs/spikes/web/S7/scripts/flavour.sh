#!/bin/sh
# S7: switch the scratch tree S7/repo between flavours IN PLACE (one tree, one target dir, so only
# the changed crates rebuild). Flavours:
#   base     : S1 repo (patches 0001-0004) + S7 harness probes; pin_physics untouched.
#   portable : base + patches/0006-pin_physics-portable-quat-ops.patch (the proposed fix).
#   scalar   : base + glam `scalar-math` on aarch64 only (experiment (i); patches/exp-glam-scalar-math-aarch64.patch).
# usage: flavour.sh <base|portable|scalar>
set -eu
S7=${WEB_SPIKES}/S7
cd "$S7/repo"
cur=$(cat "$S7/repo/.s7_flavour" 2>/dev/null || echo base)
case "$cur" in
  portable) patch -s -R -p1 < "$S7/patches/0006-pin_physics-portable-quat-ops.patch"
            patch -s -R -p1 < "$S7/patches/harness-portable-probe.patch" ;;
  scalar)   patch -s -R -p1 < "$S7/patches/exp-glam-scalar-math-aarch64.patch" ;;
  base) ;;
esac
case "$1" in
  portable) patch -s -p1 < "$S7/patches/0006-pin_physics-portable-quat-ops.patch"
            patch -s -p1 < "$S7/patches/harness-portable-probe.patch" ;;
  scalar)   patch -s -p1 < "$S7/patches/exp-glam-scalar-math-aarch64.patch" ;;
  base) ;;
  *) echo "usage: $0 <base|portable|scalar>" >&2; exit 2 ;;
esac
echo "$1" > "$S7/repo/.s7_flavour"
echo "S7/repo flavour: $cur -> $1"
