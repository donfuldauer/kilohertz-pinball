#!/bin/sh
# S11 req 1: Mach-O facts of every macOS asset (never executed): architecture, minimum macOS
# (LC_BUILD_VERSION / LC_VERSION_MIN_MACOSX), SDK, code signature (none / ad-hoc / linker-signed
# ad-hoc / CMS with Developer ID + team), hardened runtime, dylib dependencies (system or bundled
# @rpath) and rpaths, plus the archive sha256 against the pin and the upstream checksum file.
# Reads dl/<asset>/x/ (extracted by hand: `tar -xzf <archive> -C dl/<asset>/x`, done by this
# script if missing). Cross-check with LLVM's reader: llvm-otool-18 -l / llvm-objdump-18 --macho.
# usage: scripts/inspect_macho.sh   -> results/macho/*.txt|json, results/macho/TABLE.md
set -eu
S11=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
D=$S11/dl; O=$S11/results/macho; mkdir -p "$O"
ASSETS="wasi-sdk-32-arm64-macos wasi-sdk-32-x86_64-macos binaryen-132-arm64-macos binaryen-132-x86_64-macos
wasm-bindgen-0.2.130-aarch64-apple-darwin wasm-bindgen-0.2.130-x86_64-apple-darwin wasm-tools-1.263.0-aarch64-macos
wasm-tools-1.263.0-x86_64-macos node-v22.23.5-darwin-arm64 node-v22.23.5-darwin-x64"
KEY="bin/clang-22 bin/llvm-ar bin/lld lib/libLLVM.dylib lib/libclang-cpp.dylib lib/libedit.0.dylib bin/wasm-opt lib/libbinaryen.dylib wasm-bindgen wasm-tools bin/node"
{
  echo "| asset | archive sha256 (= pin?) | upstream checksum | file | arch | minos | sdk | signature (flags) | team | dylibs (non-system) |"
  echo "|---|---|---|---|---|---|---|---|---|---|"
} > "$O/TABLE.md"
for a in $ASSETS; do
  arc=$(ls "$D/$a"/*.tar.gz)
  [ -d "$D/$a/x" ] || { mkdir -p "$D/$a/x"; tar -xzf "$arc" -C "$D/$a/x"; }
  sha=$(sha256sum "$arc" | cut -c1-64)
  pin=$(awk -v s="$sha" '$5 == s {print "pin"}' "$S11/scripts/web/tools.lock" | head -1)
  [ -n "$pin" ] || pin=$(grep -q "$sha" "$S11/scripts/web/tools.lock" && echo "recorded" || echo "-")
  up="none published"
  for c in "$D/$a"/*.sha256 "$D/$a"/*.sha256sum; do
    [ -f "$c" ] && { [ "$(cut -c1-64 "$c")" = "$sha" ] && up="$(basename "$c") equal" || up="$(basename "$c") DIFFERS"; }
  done
  case $a in node-*) up="SHASUMS256.txt $(grep -q "^$sha  $(basename "$arc")\$" "$D/node-v22.23.5-shasums/SHASUMS256.txt" && echo equal || echo DIFFERS)" ;; esac
  python3 -I "$S11/scripts/macho.py" "$D/$a/x" --json "$O/$a.json" > "$O/$a.txt"
  python3 -I - "$O/$a.json" "$a" "$sha ($pin)" "$up" "$KEY" >> "$O/TABLE.md" <<'PY'
import json, sys
d = json.load(open(sys.argv[1])); a, sha, up, keys = sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5].split()
for f in d["files"]:
    rel = f["path"].split("/", 1)[1] if "/" in f["path"] else f["path"]
    if rel not in keys:
        continue
    for s in f["slices"]:
        sig = s["signature"]
        dl = [x for _, x in s["dylibs"] if not x.startswith(("/usr/lib/", "/System/"))]
        print(f"| {a} | {sha[:16]}… {sha.split(' ',1)[1]} | {up} | {rel} | {s['arch']} | {s['minos']} | {s['sdk']} | {sig['kind']} ({sig.get('flags','-')}) | {sig.get('team_id','') or '-'} | {', '.join(dl) or 'system only'} |")
sm = d["summary"]
print(f"| {a} | | | *all {sm['macho_files']} Mach-O files* | {'/'.join(sm['archs'])} | {'/'.join(sm['minos'])} | {'/'.join(sm['sdk'])} | {'/'.join(sm['signature_kinds'])}; unsigned {len(sm['unsigned_files'])} | {'/'.join(sm['team_ids']) or '-'} | {', '.join(sm['non_system_dylibs']) or 'system only'} |")
PY
done
# cross-check two binaries with LLVM's own Mach-O reader
for b in "$D/wasi-sdk-32-arm64-macos/x/wasi-sdk-32.0-arm64-macos/bin/clang-22" "$D/binaryen-132-x86_64-macos/x/binaryen-version_132/bin/wasm-opt"; do
  echo "== llvm-otool-18 -l $(basename "$b") (LC_BUILD_VERSION / LC_CODE_SIGNATURE)"
  llvm-otool-18 -l "$b" | grep -A5 -E 'cmd LC_(BUILD_VERSION|VERSION_MIN_MACOSX|CODE_SIGNATURE)' | head -16
  file "$b"
done > "$O/llvm_crosscheck.txt" 2>&1
cat "$O/TABLE.md"
