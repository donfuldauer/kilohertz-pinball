#!/bin/sh
# S11 req 1: download the macOS toolchain assets (and their upstream checksum files, if any)
# into dl/<name>/ (one new directory per asset; untrusted data: nothing here is executed).
# Records HTTP status of every probed URL (incl. checksum-file candidates) in dl/PROBES.txt
# and sha256 + size of every downloaded file in dl/SHA256.txt.
# usage: scripts/download_assets.sh [probe-only]
set -eu
here=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
DL=$here/dl
mkdir -p "$DL"
probe_only=${1:-}
GH=https://github.com
WS=$GH/WebAssembly/wasi-sdk/releases/download/wasi-sdk-32
BY=$GH/WebAssembly/binaryen/releases/download/version_132
WB=$GH/wasm-bindgen/wasm-bindgen/releases/download/0.2.130
WT=$GH/bytecodealliance/wasm-tools/releases/download/v1.263.0
NODEV=v22.23.5
ND=https://nodejs.org/dist/$NODEV
RS=https://static.rust-lang.org

code() { curl -sS -o /dev/null -I -L --max-time 60 -w '%{http_code}' "$1" 2>/dev/null || echo ERR; }
probe() { printf '%s %s\n' "$(code "$1")" "$1" | tee -a "$DL/PROBES.txt"; }
get() { # name url  -> dl/<name>/<basename>
  d=$DL/$1; f=$d/$(basename "$2")
  if [ -s "$f" ]; then echo "have $f"; return 0; fi
  mkdir -p "$d"
  if curl -sS -f -L --retry 3 --max-time 1800 -o "$f.part" "$2"; then mv "$f.part" "$f"; echo "got $f";
  else rm -f "$f.part"; echo "FAILED $2"; fi
}

: > "$DL/PROBES.txt"
# wasi-sdk 32.0: the two macOS archives + every checksum-file spelling we know of + the Linux one (reference)
for a in arm64-macos x86_64-macos x86_64-linux arm64-linux x86_64-windows; do
  probe "$WS/wasi-sdk-32.0-$a.tar.gz"
  for s in .sha256 .sha256sum .sha256.txt; do probe "$WS/wasi-sdk-32.0-$a.tar.gz$s"; done
done
for s in SHA256SUMS SHA256SUMS.txt sha256sums.txt checksums.txt wasi-sdk-32.0.sha256; do probe "$WS/$s"; done
# binaryen version_132
for a in arm64-macos x86_64-macos x86_64-linux aarch64-linux; do
  probe "$BY/binaryen-version_132-$a.tar.gz"; probe "$BY/binaryen-version_132-$a.tar.gz.sha256"
done
# wasm-bindgen 0.2.130 and wasm-tools 1.263.0 (release binaries)
for a in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-musl; do
  probe "$WB/wasm-bindgen-0.2.130-$a.tar.gz"; probe "$WB/wasm-bindgen-0.2.130-$a.tar.gz.sha256sum"
done
for a in aarch64-macos x86_64-macos x86_64-linux; do
  probe "$WT/wasm-tools-1.263.0-$a.tar.gz"; probe "$WT/wasm-tools-1.263.0-$a.tar.gz.sha256"
done
# Node and Rust manifests
probe "$ND/SHASUMS256.txt"; probe "$ND/SHASUMS256.txt.asc"; probe "$ND/node-$NODEV-darwin-arm64.tar.gz"; probe "$ND/node-$NODEV-darwin-x64.tar.gz"
probe "$RS/dist/channel-rust-1.97.0.toml"; probe "$RS/dist/channel-rust-1.97.0.toml.sha256"
for a in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-gnu; do
  probe "$RS/rustup/dist/$a/rustup-init"; probe "$RS/rustup/dist/$a/rustup-init.sha256"
done
[ "$probe_only" = probe-only ] && exit 0

get wasi-sdk-32-arm64-macos "$WS/wasi-sdk-32.0-arm64-macos.tar.gz"
get wasi-sdk-32-x86_64-macos "$WS/wasi-sdk-32.0-x86_64-macos.tar.gz"
get binaryen-132-arm64-macos "$BY/binaryen-version_132-arm64-macos.tar.gz"
get binaryen-132-arm64-macos "$BY/binaryen-version_132-arm64-macos.tar.gz.sha256"
get binaryen-132-x86_64-macos "$BY/binaryen-version_132-x86_64-macos.tar.gz"
get binaryen-132-x86_64-macos "$BY/binaryen-version_132-x86_64-macos.tar.gz.sha256"
get binaryen-132-x86_64-linux-sha "$BY/binaryen-version_132-x86_64-linux.tar.gz.sha256"
for a in aarch64-apple-darwin x86_64-apple-darwin x86_64-unknown-linux-musl; do
  get "wasm-bindgen-0.2.130-$a" "$WB/wasm-bindgen-0.2.130-$a.tar.gz"
  get "wasm-bindgen-0.2.130-$a" "$WB/wasm-bindgen-0.2.130-$a.tar.gz.sha256sum"
done
for a in aarch64-macos x86_64-macos x86_64-linux; do
  get "wasm-tools-1.263.0-$a" "$WT/wasm-tools-1.263.0-$a.tar.gz"
done
get node-$NODEV-shasums "$ND/SHASUMS256.txt"
get node-$NODEV-shasums "$ND/SHASUMS256.txt.asc"
get node-$NODEV-darwin-arm64 "$ND/node-$NODEV-darwin-arm64.tar.gz"
get node-$NODEV-darwin-x64 "$ND/node-$NODEV-darwin-x64.tar.gz"
get rust-1.97.0-manifest "$RS/dist/channel-rust-1.97.0.toml"
get rust-1.97.0-manifest "$RS/dist/channel-rust-1.97.0.toml.sha256"
for a in aarch64-apple-darwin x86_64-apple-darwin; do get "rustup-init-$a" "$RS/rustup/dist/$a/rustup-init.sha256"; done

# record sha256 + size of every downloaded file (not the extracted x/ trees), incl. files other
# S11 steps put under dl/ (Linux archives for the mirror, arm64-linux wasi-sdk, busybox debs,
# bash 3.2 source, crates.io index lines, LLVM/cargo source files)
( cd "$DL" && find . -type f ! -path '*/x/*' ! -name PROBES.txt ! -name SHA256.txt ! -name '*.part' ! -name listing.txt | LC_ALL=C sort | while read -r f; do
    printf '%s  %s  %s\n' "$(sha256sum "$f" | cut -c1-64)" "$(wc -c < "$f" | tr -d ' ')" "$f"; done ) > "$DL/SHA256.txt"
cat "$DL/SHA256.txt"
