#!/bin/sh
# Fetches native binaryen version_132 (wasm-opt) into tools/, checksum-pinned.
set -eu
here=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
SHA=195ddc94f9bc89f45abdabb0b9eea86023d727ba90eac8b35b80f2544fc30572
URL=https://github.com/WebAssembly/binaryen/releases/download/version_132/binaryen-version_132-x86_64-linux.tar.gz
[ -x "$here/tools/binaryen-version_132/bin/wasm-opt" ] && { echo "present"; exit 0; }
mkdir -p "$here/dl/binaryen" "$here/tools"
curl -sSL --max-time 600 -o "$here/dl/binaryen/b.tar.gz" "$URL"
echo "$SHA  $here/dl/binaryen/b.tar.gz" | sha256sum -c -
tar -xzf "$here/dl/binaryen/b.tar.gz" -C "$here/tools"
rm -f "$here/dl/binaryen/b.tar.gz" "$here/tools/binaryen-version_132/bin/binaryen-unittests"
