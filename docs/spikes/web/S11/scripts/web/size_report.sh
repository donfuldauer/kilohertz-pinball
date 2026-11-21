#!/bin/sh
# Module size report (S11 portable rewrite of S9's scripts/size_report.sh; proposed
# scripts/web/size_report.sh, W20): raw / gzip -9 / brotli -11 (lgwin 24, as S3/S9) via Node's
# zlib, so no python, no GNU du/stat. Optional budgets fail the run (exit 1) when exceeded.
# usage: size_report.sh [--json FILE] [--budget NAME=MiB]... NAME=FILE...
#   e.g. size_report.sh --budget webgpu=2.5 --budget webgl=3.0 webgpu=dist/webgpu/pkg/pin_web_bg.wasm webgl=...
set -eu
here=$(CDPATH='' cd -- "$(dirname -- "$0")" && pwd -P)
. "$here/lib.sh"
json=""; budgets=""
while [ $# -gt 0 ]; do
  case $1 in
    --json) json=$2; shift 2 ;;
    --budget) budgets="$budgets $2"; shift 2 ;;
    -h | --help) sed -n '2,7p' "$0"; exit 0 ;;
    *=*) break ;;  # the remaining arguments are NAME=FILE pairs (paths may contain spaces)
    *) khz_die "expected NAME=FILE, got: $1" 2 ;;
  esac
done
[ $# -gt 0 ] || khz_die "no NAME=FILE given" 2
for p in "$@"; do case $p in *=*) [ -f "${p#*=}" ] || khz_die "no such file: ${p#*=}" ;; *) khz_die "expected NAME=FILE, got: $p" 2 ;; esac; done
command -v node >/dev/null 2>&1 || khz_die "Node 22 not found"
# shellcheck disable=SC2016  # JavaScript, not shell
node -e '
const fs = require("fs"); const zlib = require("zlib");
const [json, budgetArg, ...pairs] = process.argv.slice(1);
const budgets = Object.fromEntries(budgetArg.split(" ").filter(Boolean).map((b) => { const [n, v] = b.split("="); return [n, Number(v)]; }));
const rows = []; let fail = 0;
for (const p of pairs) {
  const i = p.indexOf("="); const name = p.slice(0, i); const file = p.slice(i + 1);
  const b = fs.readFileSync(file);
  const gz = zlib.gzipSync(b, { level: 9 }).length;
  const br = zlib.brotliCompressSync(b, { params: { [zlib.constants.BROTLI_PARAM_QUALITY]: 11,
    [zlib.constants.BROTLI_PARAM_LGWIN]: 24, [zlib.constants.BROTLI_PARAM_SIZE_HINT]: b.length } }).length;
  const r = { name, file, raw: b.length, gzip9: gz, brotli11: br };
  if (name in budgets) { r.budget_mib = budgets[name]; r.within_budget = br <= budgets[name] * 1048576; if (!r.within_budget) fail++; }
  rows.push(r);
}
const M = (x) => (x / 1048576).toFixed(2);
console.log("| module | raw B | raw MiB | gzip -9 MiB | brotli -11 MiB | budget |");
console.log("|---|---|---|---|---|---|");
for (const r of rows) console.log(`| ${r.name} | ${r.raw} | ${M(r.raw)} | ${M(r.gzip9)} | ${M(r.brotli11)} | ${r.budget_mib === undefined ? "-" : (r.within_budget ? "<= " : "OVER ") + r.budget_mib} |`);
console.log(`node ${process.version}, zlib ${process.versions.zlib}, brotli ${process.versions.brotli}`);
if (json) fs.writeFileSync(json, JSON.stringify({ node: process.version, brotli: process.versions.brotli, rows }, null, 1));
process.exit(fail ? 1 : 0);
' "$json" "$budgets" "$@"
