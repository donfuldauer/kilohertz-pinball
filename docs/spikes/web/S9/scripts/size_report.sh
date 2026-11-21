#!/bin/sh
# S9 req 7 (e): module sizes raw / gzip -9 / brotli -11 (Node zlib, lgwin 24, as S3) for the
# combined module, its engine-only variants, and S1/S2/S3/S4's modules (each passed through
# the same wasm-opt -Oz, native binaryen 132), plus name-section attribution of the code.
set -eu
here=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
. "$here/env.sh"
WO=$here/tools/binaryen-version_132/bin/wasm-opt
O=$here/results/sizes; mkdir -p "$O/opt"
FEAT="--enable-simd --enable-exception-handling --enable-bulk-memory --enable-nontrapping-float-to-int --enable-sign-ext --enable-mutable-globals --enable-reference-types --enable-multivalue"
opt() { [ -f "$O/opt/$2.wasm" ] || "$WO" -Oz $FEAT --strip-debug --strip-producers "$1" -o "$O/opt/$2.wasm"; echo "$O/opt/$2.wasm"; }
list=$O/list.txt; : > "$list"
add() { [ -f "$2" ] && echo "$1 $2" >> "$list" || echo "missing $2" >&2; }
add s9_cargo_raw "$here/out/dist_bindgen/pin_web.cargo.wasm"
add s9_bindgen "$here/out/dist_bindgen/pin_web_bg.wasm"
add s9_final_Oz "$here/dist/pkg/pin_web_bg.wasm"
add s9_glue_js "$here/dist/pkg/pin_web.js"
for v in both webgpu webgl; do add "s9_engine_only_${v}_Oz" "$here/out/v_engine_$v/pkg/pin_web_bg.wasm"; add "s9_engine_only_${v}_glue_js" "$here/out/v_engine_$v/pkg/pin_web.js"; done
add s9_lto_fat_noegui_Oz "$here/out/lto_probe/pin_web_opt.wasm"
add s3_both_z_Oz "$S3/web/pkg/both-z-opt/web_probe_bg.wasm"
add s3_webgpu_z_Oz "$S3/web/pkg/webgpu-z-opt/web_probe_bg.wasm"
add s3_webgl_z_Oz "$S3/web/pkg/webgl-z-opt/web_probe_bg.wasm"
add s2_umath_bindgen_Oz "$(opt "$S2/runs/pkg_web_umath/s2_harness_bg.wasm" s2_umath)"
add s4_pkg_bindgen_Oz "$(opt "$S4/pkg/s4_rules_bg.wasm" s4_pkg)"
s1=$(ls "$S1"/runs/simd/pkg_web/*_bg.wasm 2>/dev/null | head -1)
[ -n "$s1" ] && add s1_simd_bindgen_Oz "$(opt "$s1" s1_simd)"
node - "$list" "$O/sizes.json" <<'JS'
const fs = require('fs'); const zlib = require('zlib');
const [list, out] = process.argv.slice(2); const rows = [];
for (const l of fs.readFileSync(list, 'utf8').trim().split('\n')) {
  const [name, file] = l.split(' '); const b = fs.readFileSync(file);
  const gz = zlib.gzipSync(b, { level: 9 }).length;
  const br = zlib.brotliCompressSync(b, { params: { [zlib.constants.BROTLI_PARAM_QUALITY]: 11, [zlib.constants.BROTLI_PARAM_LGWIN]: 24, [zlib.constants.BROTLI_PARAM_SIZE_HINT]: b.length } }).length;
  rows.push({ name, file, raw: b.length, gzip9: gz, brotli11: br });
}
fs.writeFileSync(out, JSON.stringify(rows, null, 1));
const M = (x) => (x / 1048576).toFixed(2);
console.log('| module | raw B | raw MiB | gzip -9 MiB | brotli -11 MiB |'); console.log('|---|---|---|---|---|');
for (const r of rows) console.log(`| ${r.name} | ${r.raw} | ${M(r.raw)} | ${M(r.gzip9)} | ${M(r.brotli11)} |`);
JS
python3 -I "$here/scripts/wasm_attrib.py" "$here/out/dist_bindgen/pin_web.cargo.wasm" 1000 > "$O/attrib_dist.json"
python3 -I "$here/scripts/s9_groups.py" "$O/attrib_dist.json" > "$O/attrib_groups_dist.md"
cat "$O/attrib_groups_dist.md"
