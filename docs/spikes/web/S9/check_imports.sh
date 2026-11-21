#!/bin/sh
# S9 req 3 (d): import audit of the final engine module. Fails (exit 1) on any import that is
# not wasm-bindgen glue: allowed = module "./pin_web_bg.js" with a name __wbg_* / __wbindgen_*
# (functions and the two EH tags). Zero env.*, zero wasi_*. Checked two ways: wasm-tools print
# and WebAssembly.Module.imports (Node). Also lists the exports.
# usage: check_imports.sh [module.wasm] [out_dir]
set -eu
here=$(CDPATH= cd -- "$(dirname "$0")" && pwd)
. "$here/env.sh"
W=${1:-$here/dist/pkg/pin_web_bg.wasm}
O=${2:-$here/results}
mkdir -p "$O"
wasm-tools print "$W" | grep -E '^\s*\(import ' > "$O/imports.wat.txt" || true
n_wat=$(wc -l < "$O/imports.wat.txt")
bad_wat=$(grep -cvE '^\s*\(import "\./pin_web_bg\.js" "(__wbg_|__wbindgen_)' "$O/imports.wat.txt" || true)
env_wat=$(grep -cE '\(import "(env|wasi_[a-z_0-9]*)"' "$O/imports.wat.txt" || true)
node - "$W" "$O" <<'JS'
const fs = require('fs'); const [W, O] = process.argv.slice(2);
const m = new WebAssembly.Module(fs.readFileSync(W));
const imp = WebAssembly.Module.imports(m); const exp = WebAssembly.Module.exports(m);
const bad = imp.filter((i) => i.module !== './pin_web_bg.js' || !/^(__wbg_|__wbindgen_)/.test(i.name));
const byKind = {}; for (const i of imp) byKind[i.kind] = (byKind[i.kind] || 0) + 1;
const modules = [...new Set(imp.map((i) => i.module))];
fs.writeFileSync(`${O}/imports.json`, JSON.stringify({ count: imp.length, modules, byKind, bad, imports: imp }, null, 1));
fs.writeFileSync(`${O}/exports.txt`, exp.map((e) => `${e.kind} ${e.name}`).join('\n') + '\n');
console.log(`node: imports=${imp.length} modules=${JSON.stringify(modules)} kinds=${JSON.stringify(byKind)} non-glue=${bad.length} exports=${exp.length}`);
console.log(`node: tags=${JSON.stringify(imp.filter((i) => i.kind === 'tag').map((i) => i.name))}`);
console.log(`node: env/wasi imports=${imp.filter((i) => i.module === 'env' || i.module.startsWith('wasi')).length}`);
process.exit(bad.length ? 1 : 0);
JS
rc=$?
echo "wasm-tools: imports=$n_wat non-glue=$bad_wat env/wasi=$env_wat"
echo "exports: $(wc -l < "$O/exports.txt") (list in $O/exports.txt)"
if [ "$rc" -ne 0 ] || [ "$bad_wat" -ne 0 ] || [ "$env_wat" -ne 0 ] || [ "$n_wat" -eq 0 ]; then
  echo "IMPORT AUDIT: FAIL"; exit 1
fi
echo "IMPORT AUDIT: PASS ($W)"
