# shellcheck shell=sh
# Shared helpers for the web build scripts (S11 portable rewrite; proposed scripts/web/lib.sh).
# POSIX sh only: runs under dash, bash --posix, bash 3.2 (macOS /bin/sh) and busybox sh.
# No GNU-only flags (no sha256sum/readlink -f/stat -c/sed -i/sed -r/grep -P/nproc/realpath).
# Sourced, never executed. Defines khz_* functions only.

khz_die() { # message [exit code]
  printf 'error: %s\n' "$1" >&2
  exit "${2:-1}"
}

khz_note() { printf '%s\n' "$*" >&2; }

# Physical absolute path of a directory (resolves symlinks such as macOS /tmp -> /private/tmp).
khz_abs_dir() {
  (CDPATH='' cd -- "$1" 2>/dev/null && pwd -P) || return 1
}

# Build-host platform key: linux-x86_64 | darwin-arm64 | darwin-x86_64. Anything else is refused
# (exit 3) with a message. An x86_64 shell under Rosetta 2 on Apple Silicon
# (sysctl.proc_translated = 1) gets darwin-arm64: those tools run natively there.
khz_platform() {
  khz_s=$(uname -s 2>/dev/null) || khz_s=unknown
  khz_m=$(uname -m 2>/dev/null) || khz_m=unknown
  case "$khz_s:$khz_m" in
    Linux:x86_64 | Linux:amd64)
      echo linux-x86_64 ;;
    Darwin:arm64 | Darwin:aarch64)
      echo darwin-arm64 ;;
    Darwin:x86_64)
      if [ "$(sysctl -n sysctl.proc_translated 2>/dev/null)" = 1 ]; then
        khz_note "note: x86_64 shell under Rosetta 2 on Apple Silicon: using the darwin-arm64 tools"
        echo darwin-arm64
      else
        echo darwin-x86_64
      fi ;;
    MINGW* | MSYS* | CYGWIN* | Windows*)
      khz_note "error: Windows is not a supported build host for the web version; use WSL2 (Linux x86_64)"
      return 3 ;;
    *)
      khz_note "error: unsupported build host $khz_s/$khz_m; supported: Linux x86_64 (incl. WSL2), macOS arm64 (Apple Silicon), macOS x86_64 (Intel)"
      return 3 ;;
  esac
}

# sha256 of a file, lower-case hex. Node (a prerequisite) first, then shasum -a 256 (macOS base
# system, perl), then sha256sum (GNU coreutils). KHZ_SHA256_TOOL=node|shasum|sha256sum forces one.
khz_sha256() {
  khz_t=${KHZ_SHA256_TOOL:-}
  if [ -z "$khz_t" ]; then
    if command -v node >/dev/null 2>&1; then khz_t=node
    elif command -v shasum >/dev/null 2>&1; then khz_t=shasum
    elif command -v sha256sum >/dev/null 2>&1; then khz_t=sha256sum
    else khz_die "no sha256 tool: need node, shasum or sha256sum"; fi
  fi
  case $khz_t in
    node) node -e 'const h=require("crypto").createHash("sha256");const s=require("fs").createReadStream(process.argv[1]);s.on("error",(e)=>{console.error(e.message);process.exit(1)});s.on("data",(d)=>h.update(d));s.on("end",()=>console.log(h.digest("hex")))' "$1" ;;
    shasum) shasum -a 256 "$1" | cut -c1-64 ;;
    sha256sum) sha256sum "$1" | cut -c1-64 ;;
    *) khz_die "unknown KHZ_SHA256_TOOL=$khz_t" ;;
  esac
}

# Size of a file in bytes (wc -c pads with spaces on BSD: strip them).
khz_size() {
  wc -c < "$1" | tr -d ' \t'
}

# CPU count: getconf works on Linux (glibc, musl) and macOS; sysctl on BSD; 2 as a fallback.
khz_ncpu() {
  getconf _NPROCESSORS_ONLN 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 2
}
