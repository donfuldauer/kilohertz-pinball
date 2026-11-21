#!/bin/sh
# Import audit of the engine module (S11 portable rewrite of S9's check_imports.sh; part of the
# proposed scripts/web/check_module.sh, W20). Fails (exit 1) on any import that is not
# wasm-bindgen glue (module "./pin_web_bg.js", names __wbg_* / __wbindgen_*), on any env.* or
# wasi_* import, and on a module with no imports. Checked with wasm-tools print and with Node's
# WebAssembly.Module.imports. Portable: POSIX classes instead of GNU grep's \s; wc output trimmed.
# usage: check_imports.sh MODULE.wasm [OUT_DIR]
set -eu
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
. "$here/lib.sh"
W=${1:?usage: check_imports.sh MODULE.wasm [OUT_DIR]}
O=${2:-$(dirname -- "$W")/audit}
mkdir -p "$O"
wasm-tools print "$W" | grep -E '^[[:space:]]*\(import ' > "$O/imports.wat.txt" || true
n_wat=$(wc -l < "$O/imports.wat.txt" | tr -d ' \t')
bad_wat=$(grep -cvE '^[[:space:]]*\(import "\./pin_web_bg\.js" "(__wbg_|__wbindgen_)' "$O/imports.wat.txt" || true)
env_wat=$(grep -cE '\(import "(env|wasi_[a-z_0-9]*)"' "$O/imports.wat.txt" || true)
rc=0
# shellcheck disable=SC2016  # JavaScript, not shell
node -e '
const fs = require("fs"); const [W, O] = process.argv.slice(1);
const m = new WebAssembly.Module(fs.readFileSync(W));
const imp = WebAssembly.Module.imports(m); const exp = WebAssembly.Module.exports(m);
const bad = imp.filter((i) => i.module !== "./pin_web_bg.js" || !/^(__wbg_|__wbindgen_)/.test(i.name));
const byKind = {}; for (const i of imp) byKind[i.kind] = (byKind[i.kind] || 0) + 1;
fs.writeFileSync(`${O}/imports.json`, JSON.stringify({ count: imp.length, byKind, bad, imports: imp }, null, 1));
fs.writeFileSync(`${O}/exports.txt`, exp.map((e) => `${e.kind} ${e.name}`).join("\n") + "\n");
console.log(`node: imports=${imp.length} kinds=${JSON.stringify(byKind)} non-glue=${bad.length} exports=${exp.length}`);
process.exit(bad.length ? 1 : 0);
' "$W" "$O" || rc=$?
echo "wasm-tools: imports=$n_wat non-glue=$bad_wat env/wasi=$env_wat"
if [ "$rc" -ne 0 ] || [ "$bad_wat" -ne 0 ] || [ "$env_wat" -ne 0 ] || [ "$n_wat" -eq 0 ]; then
  echo "IMPORT AUDIT: FAIL ($W)"; exit 1
fi
echo "IMPORT AUDIT: PASS ($W)"
