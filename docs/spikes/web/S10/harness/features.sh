#!/bin/sh
# S10 req 3: which Wasm features each shipped module needs (wasm-tools 1.263.0 validation
# with each feature turned off one at a time, from the full set), its imports, memory and
# table limits, tags and start function; plus the minimal feature set that validates.
# usage: features.sh > logs/features.md
set -u
W=${WEB_SPIKES}
S10=$W/S10; WT=$W/S1/tools/bin/wasm-tools; M=$S10/mods
FEATS="mutable-global saturating-float-to-int sign-extension reference-types multi-value bulk-memory simd relaxed-simd threads shared-everything-threads tail-call floats multi-memory exceptions legacy-exceptions memory64 extended-const function-references memory-control gc custom-page-sizes stack-switching wide-arithmetic component-model gc-types"
MODS="s1_simd/s1_harness_bg.wasm s7_portable_simd/s1_harness_bg.wasm s7_portable_nosimd/s1_harness_bg.wasm s8_v2/pkg_node/s2_harness_bg.wasm s4_pkg/s4_rules_bg.wasm s1_base/s1_harness_bg.wasm"
[ -f "$M/s10_v2_exnref/pkg_node/s2_harness_bg.wasm" ] && MODS="$MODS s10_v2_exnref/pkg_node/s2_harness_bg.wasm"
[ -f "$M/s10_v2_legacy/pkg_node/s2_harness_bg.wasm" ] && MODS="$MODS s10_v2_legacy/pkg_node/s2_harness_bg.wasm"
echo "# Feature requirements per module (wasm-tools $($WT --version | cut -d' ' -f2))"
echo
echo "Method: \`wasm-tools validate --features=all,-<f>\` for each feature f; 'needs' = validation fails without it."
echo "Minimal set = \`--features=-all,<needed...>\` (plus floats, which wasmparser models as a feature) validated."
echo
for m in $MODS; do
  f=$M/$m
  [ -f "$f" ] || continue
  needs=""
  for x in $FEATS; do
    if ! $WT validate --features="all,-$x" "$f" >/dev/null 2>&1; then needs="$needs $x"; fi
  done
  min="floats$(for x in $needs; do printf ',%s' "$x"; done)"
  if $WT validate --features="-all,$min" "$f" >/dev/null 2>&1; then minok=validates; else minok="FAILS: $($WT validate --features="-all,$min" "$f" 2>&1 | head -1)"; fi
  P=$($WT print "$f" 2>/dev/null)
  imports=$(echo "$P" | grep -E '^\s*\(import ' | sed -E 's/^\s*\(import "([^"]*)" "([^"]*)" \(([a-z]+).*/\1.\2(\3)/' | sed -E 's/__wbg_([a-z_]+)_[0-9a-f]{16}/__wbg_\1_*/' | tr '\n' ' ')
  mem=$(echo "$P" | grep -E '^\s*\(memory ' | head -2 | sed 's/^ *//' | tr '\n' ' ')
  tab=$(echo "$P" | grep -E '^\s*\(table ' | sed 's/^ *//' | tr '\n' ' ')
  tags=$(echo "$P" | grep -c -E '^\s*\(tag ')
  start=$(echo "$P" | grep -c -E '^\s*\(start ')
  eh_legacy=$(echo "$P" | grep -c -E '^\s+(try|catch|catch_all|rethrow|delegate)\b')
  eh_new=$(echo "$P" | grep -c -E '^\s+(try_table|throw_ref)\b')
  simd_ops=$(echo "$P" | grep -c -E '^\s+(v128|[if](8|16|32|64)x[0-9]+)\.')
  relaxed=$(echo "$P" | grep -c -E '^\s+[a-z0-9]+x[0-9]+\.relaxed_')
  rcall=$(echo "$P" | grep -c -E '^\s+return_call')
  echo "## $m ($(wc -c < "$f") bytes, sha256 $(sha256sum < "$f" | cut -c1-16)…)"
  echo "* needs:$needs"
  echo "* minimal set ($min): $minok"
  echo "* EH instructions: legacy (try/catch/catch_all/rethrow/delegate) $eh_legacy, exnref (try_table/throw_ref) $eh_new; tags $tags"
  echo "* SIMD instructions $simd_ops (relaxed $relaxed); return_call $rcall; start function: $start"
  echo "* memory: $mem; tables: $tab"
  echo "* imports: $imports"
  echo
done
