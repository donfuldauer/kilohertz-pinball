#!/bin/bash
# S11 req 3 tests: run the portable web scripts (scripts/web/) under several shells with a
# BSD-like PATH and shimmed `uname`/`sysctl`, and print a PASS/FAIL table.
#   shells:  dash 0.5.12, bash 5.2 --posix (sh-mode symlink), busybox 1.36.1 sh (Ubuntu's dynamic
#            `busybox` package, S11/tools/bin/busybox-dyn: it looks commands up on PATH; the static
#            build prefers its own applets, so PATH shims cannot stand in for uname etc. there);
#            bash 3.2 if S11/tools/bash32/bin/bash exists
#            (it does not: building it was not permitted in this session; macOS /bin/sh = bash
#            3.2.57 is exercised by the proposed web-build-macos CI job instead)
#   fakebin: only the tools the scripts may use. GNU-only tools are absent (sha256sum, nproc,
#            realpath) or trapped when called with a GNU-only flag (readlink -f, stat -c, sed -i/-r,
#            grep -P, xargs -r, date -d, sort -V, du -b, find -printf, cp --parents): a trap is
#            logged and fails the "no GNU-only flags" row. uname/sysctl/xattr/xcode-select are shims.
#   Every shell runs the scripts itself (`<shell> script args`; `sh` on PATH = that shell).
# usage: scripts/test_portability.sh [quick]   (quick: skip the real-archive fetches, ~1.3 GB each)
# Exit 0 iff every row PASSes. Table: results/portability/table.md; logs under work/tp/.
set -u
S11=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd -P)
SPK=$(dirname "$S11")
WEB=$S11/scripts/web
TP=$S11/work/tp
OUT=$S11/results/portability
quick=${1:-}
rm -rf "$TP"; mkdir -p "$TP/home" "$OUT"
TABLE=$OUT/table.md; : > "$TABLE.rows"
FAILS=0
row() { printf '| %-11s | %-66s | %-4s | %s |\n' "$1" "$2" "$3" "$4" >> "$TABLE.rows"; [ "$3" = FAIL ] && FAILS=$((FAILS + 1)); return 0; }

# ---- fakebin: the BSD-like PATH ----------------------------------------------------------------
FB=$TP/fakebin; mkdir -p "$FB"
for t in awk cat chmod cut dirname basename env grep gzip head ln ls mkdir mktemp mv rm sort tail tar \
  tr wc getconf node shasum perl id curl touch; do
  p=$(command -v "$t") && ln -s "$p" "$FB/$t"
done
TRAPLOG=$TP/traps.log; : > "$TRAPLOG"
trapwrap() { # name real-binary forbidden-args-ERE (matched against " <args> ")
  rm -f "$FB/$1"
  printf '#!/bin/sh\nif printf "%%s\\n" " $* " | /usr/bin/grep -qE -- %s; then echo "TRAP %s $*" >> %s; exit 99; fi\nexec %s "$@"\n' \
    "'$3'" "$1" "'$TRAPLOG'" "$2" > "$FB/$1"
  chmod +x "$FB/$1"
}
trapwrap readlink /usr/bin/readlink ' -[a-zA-Z]*f '
trapwrap stat /usr/bin/stat ' (-c|--format|--printf)'
trapwrap sed /usr/bin/sed ' (-[a-zA-Z]*r[a-zA-Z]* |-i |-i$|--in-place|--regexp-extended)'
trapwrap grep /usr/bin/grep ' -[a-zA-Z]*P'
trapwrap xargs /usr/bin/xargs ' (-r|--no-run-if-empty) '
trapwrap date /usr/bin/date ' (-d|--date)'
trapwrap sort /usr/bin/sort ' -[a-zA-Z]*V'
trapwrap du /usr/bin/du ' -[a-zA-Z]*b'
trapwrap find /usr/bin/find ' -printf '
trapwrap cp /usr/bin/cp ' --parents '
cat > "$FB/uname" <<'EOF'
#!/bin/sh
case ${1:--s} in -m) echo "${KHZ_FAKE_UNAME_M:?}" ;; *) echo "${KHZ_FAKE_UNAME_S:?}" ;; esac
EOF
cat > "$FB/sysctl" <<'EOF'
#!/bin/sh
case "$*" in
  "-n sysctl.proc_translated") if [ -n "${KHZ_FAKE_TRANSLATED:-}" ]; then echo "$KHZ_FAKE_TRANSLATED"; exit 0; fi
    echo "sysctl: unknown oid 'sysctl.proc_translated'" >&2; exit 1 ;;
  "-n hw.ncpu") echo 4 ;;
  *) exit 1 ;;
esac
EOF
printf '#!/bin/sh\necho "xattr $*" >> "%s"\n[ "${KHZ_FAKE_XATTR_FAIL:-0}" = 1 ] && exit 1\nexit 0\n' "$TP/xattr.log" > "$FB/xattr"
chmod +x "$FB/uname" "$FB/sysctl" "$FB/xattr"
XC=$TP/xcbin; mkdir -p "$XC"; printf '#!/bin/sh\necho /Library/Developer/CommandLineTools\n' > "$XC/xcode-select"; chmod +x "$XC/xcode-select"
RB=$TP/rustbin; mkdir -p "$RB"; SYSROOT=$(rustc --print sysroot)
ln -s "$SYSROOT/bin/rustc" "$RB/rustc"; ln -s "$SYSROOT/bin/cargo" "$RB/cargo"
CT=$TP/cargo-tools; mkdir -p "$CT/bin"   # stands in for fetch_tools.sh's `cargo install --root`
ln -s "$SPK/S1/tools/bin/wasm-bindgen" "$CT/bin/wasm-bindgen"; ln -s "$SPK/S1/tools/bin/wasm-tools" "$CT/bin/wasm-tools"
for v in shasum sha256sum none; do mkdir -p "$TP/min-$v"; ln -s "$(command -v cut)" "$TP/min-$v/cut"; done
ln -s "$(command -v shasum)" "$TP/min-shasum/shasum"; ln -s "$(command -v sha256sum)" "$TP/min-sha256sum/sha256sum"

# rsh SHELLNAME S M TRANSLATED [--] args...  : run "<shell> args" in the fake environment.
# XENV="K=V ..." adds variables (no spaces in values); XPATH appends PATH entries.
rsh() {
  local n=$1 s=$2 m=$3 tr=$4; shift 5
  local -a envs=(HOME="$TP/home" PATH="$TP/$n/shbin:$FB${XPATH:+:$XPATH}" LC_ALL=C KHZ_FAKE_UNAME_S="$s" KHZ_FAKE_UNAME_M="$m")
  [ "$tr" != - ] && envs+=(KHZ_FAKE_TRANSLATED="$tr")
  if [ -n "${XENV:-}" ]; then local -a xe; read -r -a xe <<< "$XENV"; envs+=("${xe[@]}"); fi
  env -i "${envs[@]}" "${SH_CMD[@]}" "$@"
}

# ---- fixtures ----------------------------------------------------------------------------------
TF="$TP/data with space.bin"; head -c 1000003 /dev/urandom > "$TF"
REF_SHA=$(sha256sum "$TF" | cut -c1-64); REF_SIZE=$(stat -c %s "$TF")
FM=$TP/mirror-small; mkdir -p "$FM/src/toolA-1/bin" "$FM/src/toolB-2/bin"
echo 'echo A' > "$FM/src/toolA-1/bin/a"; echo 'echo B' > "$FM/src/toolB-2/bin/b"
(cd "$FM/src" && tar -czf "$FM/toolA-1.tar.gz" toolA-1 && tar -czf "$FM/toolB-2.tar.gz" toolB-2)
SA=$(sha256sum "$FM/toolA-1.tar.gz" | cut -c1-64); SB=$(sha256sum "$FM/toolB-2.tar.gz" | cut -c1-64)
ZA=$(stat -c %s "$FM/toolA-1.tar.gz"); ZB=$(stat -c %s "$FM/toolB-2.tar.gz")
printf 'wasi-sdk darwin-arm64 archive 1 %s %s toolA-1 https://example.invalid/toolA-1.tar.gz none\nbinaryen darwin-arm64 archive 2 %s %s toolB-2 https://example.invalid/toolB-2.tar.gz none\n' \
  "$SA" "$ZA" "$SB" "$ZB" > "$TP/small.lock"
sed "s/$SB/$(printf '%064d' 0)/" "$TP/small.lock" > "$TP/bad.lock"
FULLM=$TP/mirror-full; mkdir -p "$FULLM"
for f in "$S11"/dl/wasi-sdk-32-*/wasi-sdk-32.0-*.tar.gz "$S11"/dl/binaryen-132-*/binaryen-version_132-*.tar.gz "$S11"/dl/node-v22.23.5-*/node-*.tar.gz; do
  [ -f "$f" ] && ln "$f" "$FULLM/$(basename "$f")"
done
printf '(module (import "env" "saveSetjmp" (func)) (import "./pin_web_bg.js" "__wbg_x" (func)))' > "$TP/bad_env.wat"
"$SPK/S1/tools/bin/wasm-tools" parse "$TP/bad_env.wat" -o "$TP/bad_env.wasm"
REPO_SP="$TP/repo with space"; mkdir -p "$REPO_SP"
TB=$TP/btools; mkdir -p "$TB/linux" "$TB/mac"
printf "KHZ_WEB_PLATFORM='linux-x86_64'\nKHZ_WASI_SDK='%s'\nKHZ_BINARYEN='%s'\nKHZ_CARGO_TOOLS='%s'\n" \
  "$SPK/S2/tools/wasi-sdk/wasi-sdk-32.0-x86_64-linux" "$SPK/S9/tools/binaryen-version_132" "$CT" > "$TB/linux/tools.env"
sed "s/linux-x86_64/darwin-arm64/" "$TB/linux/tools.env" > "$TB/mac/tools.env"

# static check: ShellCheck 0.9.0 (Ubuntu package, S11/tools/bin/shellcheck) in POSIX sh mode
if [ -x "$S11/tools/bin/shellcheck" ]; then
  (cd "$WEB" && "$S11/tools/bin/shellcheck" --shell=sh -x lib.sh wasm_env.sh fetch_tools.sh build_web.sh size_report.sh check_imports.sh) > "$TP/shellcheck.txt" 2>&1 \
    && row "all" "ShellCheck --shell=sh -x on scripts/web/*.sh: no findings" PASS "$("$S11/tools/bin/shellcheck" --version | sed -n 2p)" \
    || row "all" "ShellCheck --shell=sh" FAIL "$(grep -c '^In ' "$TP/shellcheck.txt") findings (work/tp/shellcheck.txt)"
fi
SHELL_LIST="dash bash-posix busybox-sh"
[ -x "$S11/tools/bash32/bin/bash" ] && SHELL_LIST="$SHELL_LIST bash-3.2"
[ -x "$S11/tools/bash32/bin/bash" ] || row "bash-3.2" "bash 3.2.57 (= macOS /bin/sh)" SKIP "not built here (not permitted in this session); web-build-macos CI runs /bin/sh"

for name in $SHELL_LIST; do
  L=$TP/$name; mkdir -p "$L/shbin"
  case $name in
    dash) SH_CMD=(/usr/bin/dash); ln -s /usr/bin/dash "$L/shbin/sh" ;;
    bash-posix) SH_CMD=(/usr/bin/bash --posix); ln -s /usr/bin/bash "$L/shbin/sh" ;;
    busybox-sh) SH_CMD=("$S11/tools/bin/busybox-dyn" sh); ln -s "$S11/tools/bin/busybox-dyn" "$L/shbin/sh" ;;
    bash-3.2) SH_CMD=("$S11/tools/bash32/bin/bash"); ln -s "$S11/tools/bash32/bin/bash" "$L/shbin/sh" ;;
  esac
  : > "$TRAPLOG"

  # P: platform detection
  pcase() { # want-out want-rc S M translated label
    out=$(rsh "$name" "$3" "$4" "$5" -- -c ". '$WEB/lib.sh'; khz_platform" 2> "$L/p.err"); rc=$?
    if [ "$out" = "$1" ] && [ "$rc" = "$2" ]; then row "$name" "platform $6 -> ${1:-refused, exit $2}" PASS "$(tr '\n' ' ' < "$L/p.err" | cut -c1-100)"
    else row "$name" "platform $6" FAIL "got '$out' rc=$rc, want '$1' rc=$2"; fi
  }
  pcase linux-x86_64 0 Linux x86_64 - "Linux/x86_64 (incl. WSL2)"
  pcase darwin-arm64 0 Darwin arm64 - "Darwin/arm64"
  pcase darwin-x86_64 0 Darwin x86_64 0 "Darwin/x86_64, proc_translated=0 (Intel)"
  pcase darwin-x86_64 0 Darwin x86_64 - "Darwin/x86_64, no proc_translated key"
  pcase darwin-arm64 0 Darwin x86_64 1 "Darwin/x86_64, proc_translated=1 (Rosetta 2)"
  pcase "" 3 Linux aarch64 - "Linux/aarch64"
  pcase "" 3 MINGW64_NT-10.0-26100 x86_64 - "MINGW64 (Git Bash on Windows)"
  pcase "" 3 FreeBSD amd64 - "FreeBSD/amd64"

  # S: sha256 helper per backend, auto selection, size, cpu count
  for t in node shasum sha256sum; do
    got=$(XENV="KHZ_SHA256_TOOL=$t" XPATH="$TP/min-sha256sum" rsh "$name" Linux x86_64 - -- -c ". '$WEB/lib.sh'; khz_sha256 '$TF'" 2>&1)
    [ "$got" = "$REF_SHA" ] && row "$name" "khz_sha256 via $t (file name with a space)" PASS "${got:0:16}" || row "$name" "khz_sha256 via $t" FAIL "got $got"
  done
  for v in shasum sha256sum; do
    got=$(env -i PATH="$L/shbin:$TP/min-$v" "${SH_CMD[@]}" -c ". '$WEB/lib.sh'; khz_sha256 '$TF'" 2>&1)
    [ "$got" = "$REF_SHA" ] && row "$name" "khz_sha256 auto without node -> $v" PASS "" || row "$name" "khz_sha256 auto -> $v" FAIL "got $got"
  done
  got=$(env -i PATH="$L/shbin:$TP/min-none" "${SH_CMD[@]}" -c ". '$WEB/lib.sh'; khz_sha256 '$TF'" 2>&1); rc=$?
  [ $rc = 1 ] && echo "$got" | grep -q 'no sha256 tool' && row "$name" "khz_sha256 with no tool: clear error" PASS "$got" || row "$name" "khz_sha256 none" FAIL "rc=$rc $got"
  got=$(rsh "$name" Linux x86_64 - -- -c ". '$WEB/lib.sh'; echo \$(khz_size '$TF') \$(khz_ncpu)" 2>&1)
  [ "${got%% *}" = "$REF_SIZE" ] && [ -n "${got#* }" ] && row "$name" "khz_size / khz_ncpu without stat -c / nproc" PASS "$got" || row "$name" "khz_size/khz_ncpu" FAIL "$got"

  # F1: pin selection per platform
  for pk in "Linux x86_64 - linux-x86_64 wasi-sdk-32.0-x86_64-linux" "Darwin arm64 - darwin-arm64 wasi-sdk-32.0-arm64-macos" "Darwin x86_64 0 darwin-x86_64 wasi-sdk-32.0-x86_64-macos"; do
    set -- $pk
    out=$(rsh "$name" "$1" "$2" "$3" -- "$WEB/fetch_tools.sh" --print --node 2>&1)
    want=$(awk -v p="$4" '$1=="binaryen" && $2==p {print $5}' "$WEB/tools.lock")
    if echo "$out" | grep -qx "platform $4" && echo "$out" | grep -q " $5 " && echo "$out" | grep -q "$want" \
       && [ "$(echo "$out" | grep -c ' archive ')" = 3 ] && [ "$(echo "$out" | grep -c ' cargo ')" = 2 ]; then
      row "$name" "fetch_tools --print selects the $4 pins" PASS "wasi-sdk, binaryen ${want:0:12}, node + 2 cargo tools"
    else row "$name" "fetch_tools --print $4" FAIL "$(echo "$out" | head -2 | tr '\n' ' ')"; fi
  done

  # F3: fetch logic on a small mirror (Darwin shim): extract, quarantine, cache, wrong pin, spaces
  T3="$TP/$name tools"; rm -f "$TP/xattr.log"
  out=$(XENV="KHZ_WEB_MIRROR=$FM" rsh "$name" Darwin arm64 - -- "$WEB/fetch_tools.sh" --lock "$TP/small.lock" --tools "$T3" --no-cargo --no-smoke 2>&1); rc=$?
  if [ $rc = 0 ] && [ -f "$T3/darwin-arm64/toolA-1/bin/a" ] && [ -f "$T3/darwin-arm64/toolB-2.sha256" ] && grep -q "KHZ_WASI_SDK='$T3/darwin-arm64/toolA-1'" "$T3/tools.env"; then
    row "$name" "fetch, sha256 verify, extract; tools dir with a space" PASS "tools.env written"
  else row "$name" "fetch+verify+extract" FAIL "rc=$rc $(echo "$out" | tail -2 | tr '\n' ' ')"; fi
  grep -q -- "-dr com.apple.quarantine $T3/darwin-arm64/toolA-1" "$TP/xattr.log" 2>/dev/null &&
    row "$name" "macOS: com.apple.quarantine removed after verification" PASS "xattr -dr ... called per tool" ||
    row "$name" "macOS quarantine removal" FAIL "$(cat "$TP/xattr.log" 2>/dev/null)"
  out=$(XENV="KHZ_WEB_MIRROR=$FM" rsh "$name" Darwin arm64 - -- "$WEB/fetch_tools.sh" --lock "$TP/small.lock" --tools "$T3" --no-cargo --no-smoke 2>&1); rc=$?
  [ $rc = 0 ] && [ "$(echo "$out" | grep -c 'already verified')" = 2 ] && row "$name" "second run: cache hit, nothing fetched" PASS "2 x already verified" || row "$name" "cache hit" FAIL "rc=$rc"
  T4="$TP/$name bad"
  out=$(XENV="KHZ_WEB_MIRROR=$FM KHZ_FAKE_XATTR_FAIL=1" rsh "$name" Darwin arm64 - -- "$WEB/fetch_tools.sh" --lock "$TP/bad.lock" --tools "$T4" --no-cargo --no-smoke 2>&1); rc=$?
  if [ $rc != 0 ] && echo "$out" | grep -q 'checksum mismatch for toolB-2.tar.gz' && [ ! -e "$T4/darwin-arm64/toolB-2" ] && [ ! -e "$T4/dl/toolB-2.tar.gz" ] && [ -d "$T4/darwin-arm64/toolA-1" ]; then
    row "$name" "wrong pin: exit 1, archive deleted, not extracted (xattr failing ok)" PASS "$(echo "$out" | grep -m1 -o 'checksum mismatch for [^:]*')"
  else row "$name" "wrong pin" FAIL "rc=$rc $(echo "$out" | tail -1)"; fi

  # F2: the real pinned archives for all three platforms (dash and busybox; about 1.3 GB each)
  if [ "$quick" != quick ] && { [ $name = dash ] || [ $name = busybox-sh ]; }; then
    T2=$TP/$name-real
    for pk in "Linux x86_64 - linux-x86_64" "Darwin arm64 - darwin-arm64" "Darwin x86_64 0 darwin-x86_64"; do
      set -- $pk
      # all three archives incl. Node, no binary run; then (Linux only) the toolchain smoke check
      # (clang, llvm-ar, wasm-opt: the same files S2/S9 ran) from the cache. The fetched Node
      # binary is not executed here.
      XENV="KHZ_WEB_MIRROR=$FULLM" rsh "$name" "$1" "$2" "$3" -- "$WEB/fetch_tools.sh" --tools "$T2" --no-cargo --no-smoke --node > "$L/fetch-real-$4.log" 2>&1; rc=$?
      if [ $rc = 0 ] && [ "$4" = linux-x86_64 ]; then
        XENV="KHZ_WEB_MIRROR=$FULLM" rsh "$name" "$1" "$2" "$3" -- "$WEB/fetch_tools.sh" --tools "$T2" --no-cargo >> "$L/fetch-real-$4.log" 2>&1 || rc=$?
      fi
      ok=1; [ $rc = 0 ] || ok=0
      for top in $(awk -v p="$4" '$2==p && $3=="archive" {print $7}' "$WEB/tools.lock"); do [ -d "$T2/$4/$top" ] || ok=0; done
      nsha=$(grep -c '^sha256 OK' "$L/fetch-real-$4.log"); nsm=$(grep -c '^smoke OK' "$L/fetch-real-$4.log")
      [ $ok = 1 ] && row "$name" "real pins $4: wasi-sdk, binaryen, node verified, extracted" PASS "sha256 OK x$nsha; smoke OK x$nsm" ||
        row "$name" "real pins $4" FAIL "rc=$rc $(tail -1 "$L/fetch-real-$4.log")"
    done
    python3 -I "$S11/scripts/compare_trees.py" fetched="$T2/linux-x86_64/wasi-sdk-32.0-x86_64-linux" s2="$SPK/S2/tools/wasi-sdk/wasi-sdk-32.0-x86_64-linux" > "$L/cmp_wasi_linux.txt" 2>&1
    python3 -I "$S11/scripts/compare_trees.py" fetched="$T2/linux-x86_64/binaryen-version_132" s9="$SPK/S9/tools/binaryen-version_132" > "$L/cmp_binaryen_linux.txt" 2>&1
    python3 -I "$S11/scripts/compare_trees.py" fetched="$T2/darwin-arm64/wasi-sdk-32.0-arm64-macos" dl="$S11/dl/wasi-sdk-32-arm64-macos/x/wasi-sdk-32.0-arm64-macos" > "$L/cmp_wasi_arm64.txt" 2>&1
    w=$(head -1 "$L/cmp_wasi_linux.txt"); b=$(head -1 "$L/cmp_binaryen_linux.txt"); m=$(head -1 "$L/cmp_wasi_arm64.txt")
    if echo "$w" | grep -q 'differing 0; only in some 0' && echo "$b" | grep -q 'differing 0; only in some 1' && echo "$m" | grep -q 'differing 0; only in some 0'; then
      row "$name" "fetched Linux trees == S2 wasi-sdk / S9 binaryen (same pins)" PASS "binaryen: +binaryen-unittests (S9 deleted it)"
    else row "$name" "fetched trees == references" FAIL "$w | $b | $m"; fi
    rm -rf "$T2"
  fi

  # W: wasm_env.sh drops the host C environment and keeps remap entries with spaces intact
  out=$(XENV="CFLAGS=-I/opt/homebrew/include CPATH=/opt/homebrew/include RUSTFLAGS=-Cfoo CARGO_HOME=$TP/home/.cargo" XPATH="$RB" rsh "$name" Linux x86_64 - -- -c "
    . '$WEB/lib.sh'; KHZ_WASI_SDK='$SPK/S2/tools/wasi-sdk/wasi-sdk-32.0-x86_64-linux'; KHZ_WEB_REPO='$REPO_SP'; . '$WEB/wasm_env.sh';
    echo \"CFLAGS=\${CFLAGS-unset} CPATH=\${CPATH-unset} RUSTFLAGS=\${RUSTFLAGS-unset}\";
    printf '%s' \"\$CARGO_ENCODED_RUSTFLAGS\" | tr '\\037' '\\n'; echo; echo \"C=\$CFLAGS_wasm32_unknown_unknown\"" 2>&1)
  echo "$out" > "$L/wasm_env.txt"
  if echo "$out" | grep -qx 'CFLAGS=unset CPATH=unset RUSTFLAGS=unset' && echo "$out" | grep -qxF -- "--remap-path-prefix=$REPO_SP=/khz" \
     && echo "$out" | grep -qxF -- "--remap-path-prefix=$TP/home/.cargo=/cargo" && echo "$out" | grep -qx 'target-feature=+simd128' \
     && echo "$out" | grep -qx 'link-arg=-zstack-size=4194304' && echo "$out" | grep -qF "'-ffile-prefix-map=$REPO_SP=/khz'"; then
    row "$name" "wasm_env: host CFLAGS/CPATH/RUSTFLAGS dropped, remap intact with spaces" PASS "$(echo "$out" | grep -c remap-path-prefix) remap entries"
  else row "$name" "wasm_env" FAIL "$(echo "$out" | head -2 | tr '\n' ' ')"; fi

  # B: build_web.sh prerequisite checks (dry run; the real Linux build is in linux_e2e.sh)
  out=$(XENV="CARGO_HOME=/root/.cargo" XPATH="$RB" rsh "$name" Linux x86_64 - -- "$WEB/build_web.sh" --repo "$S11/work/a/kp" --tools "$TB/linux" --dry-run 2>&1); rc=$?
  echo "$out" > "$L/build_dry_linux.txt"
  [ $rc = 0 ] && echo "$out" | grep -q 'would run' && echo "$out" | grep -q -- '-u CC_wasm32-unknown-unknown' && echo "$out" | grep -q 'wasm-opt version 132' &&
    row "$name" "build_web.sh --dry-run on Linux: all checks pass" PASS "plan + env printed" || row "$name" "build_web dry-run Linux" FAIL "rc=$rc $(echo "$out" | tail -1)"
  out=$(XPATH="$RB" rsh "$name" Darwin arm64 - -- "$WEB/build_web.sh" --repo "$S11/work/a/kp" --tools "$TB/linux" --dry-run 2>&1); rc=$?
  [ $rc != 0 ] && echo "$out" | grep -q 'are for linux-x86_64, this host is darwin-arm64' && row "$name" "build_web.sh refuses another platform's tools" PASS "${out:7:70}" || row "$name" "foreign tools" FAIL "rc=$rc $out"
  out=$(XPATH="$RB" rsh "$name" Darwin arm64 - -- "$WEB/build_web.sh" --repo "$S11/work/a/kp" --tools "$TB/mac" --dry-run 2>&1); rc=$?
  [ $rc != 0 ] && echo "$out" | grep -q 'Xcode Command Line Tools missing' && row "$name" "macOS without CLT: refused with xcode-select --install hint" PASS "" || row "$name" "CLT check" FAIL "rc=$rc $out"
  out=$(XENV="CARGO_HOME=/root/.cargo" XPATH="$RB:$XC" rsh "$name" Darwin arm64 - -- "$WEB/build_web.sh" --repo "$S11/work/a/kp" --tools "$TB/mac" --dry-run 2>&1); rc=$?
  [ $rc = 0 ] && echo "$out" | grep -q 'host darwin-arm64 (Darwin arm64)' && row "$name" "macOS (shimmed) with CLT: dry run passes" PASS "" || row "$name" "macOS dry run" FAIL "rc=$rc $(echo "$out" | tail -1)"

  # Z/C: size report and import audit on S9's shipped module
  out=$(rsh "$name" Linux x86_64 - -- "$WEB/size_report.sh" --budget s9=3.0 s9="$SPK/S9/dist/pkg/pin_web_bg.wasm" 2>&1); rc=$?
  echo "$out" > "$L/size.txt"
  [ $rc = 0 ] && echo "$out" | grep -qF '| s9 | 11140164 | 10.62 | 3.93 | 2.79 | <= 3 |' && row "$name" "size_report.sh on S9's module = S9 §8 numbers" PASS "10.62 / 3.93 / 2.79 MiB" || row "$name" "size_report" FAIL "rc=$rc $(echo "$out" | sed -n 3p)"
  out=$(rsh "$name" Linux x86_64 - -- "$WEB/size_report.sh" --budget s9=2.5 s9="$SPK/S9/dist/pkg/pin_web_bg.wasm" 2>&1); rc=$?
  [ $rc = 1 ] && echo "$out" | grep -q 'OVER 2.5' && row "$name" "size_report.sh budget exceeded -> exit 1" PASS "" || row "$name" "size budget" FAIL "rc=$rc"
  out=$(XPATH="$CT/bin" rsh "$name" Linux x86_64 - -- "$WEB/check_imports.sh" "$SPK/S9/dist/pkg/pin_web_bg.wasm" "$L/audit" 2>&1); rc=$?
  [ $rc = 0 ] && echo "$out" | grep -q 'imports=623 non-glue=0 env/wasi=0' && row "$name" "check_imports.sh: S9 module PASS (623 glue imports)" PASS "" || row "$name" "check_imports S9" FAIL "rc=$rc $(echo "$out" | tail -2 | tr '\n' ' ')"
  out=$(XPATH="$CT/bin" rsh "$name" Linux x86_64 - -- "$WEB/check_imports.sh" "$TP/bad_env.wasm" "$L/audit_bad" 2>&1); rc=$?
  [ $rc = 1 ] && echo "$out" | grep -q 'IMPORT AUDIT: FAIL' && row "$name" "check_imports.sh: env.saveSetjmp import -> FAIL" PASS "" || row "$name" "check_imports bad" FAIL "rc=$rc $(echo "$out" | tail -1)"

  if [ -s "$TRAPLOG" ]; then row "$name" "no GNU-only tool flag used" FAIL "$(tr '\n' ';' < "$TRAPLOG")"
  else row "$name" "no GNU-only tool flag used (trap log empty)" PASS ""; fi
done

{ echo "| shell | check | result | evidence |"; echo "|---|---|---|---|"; cat "$TABLE.rows"; } > "$TABLE"; rm -f "$TABLE.rows"
cat "$TABLE"
echo "== test_portability: $( [ $FAILS = 0 ] && echo ALL PASS || echo "FAILURES ($FAILS)" ) ($(grep -c '| PASS |' "$TABLE") PASS, $(grep -c '| SKIP |' "$TABLE") SKIP)"
exit $([ $FAILS = 0 ] && echo 0 || echo 1)
