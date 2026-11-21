#!/bin/sh
# S11 req 3: list Linux-only / GNU-only / bash-4-only constructs in the archived web scripts.
# Read-only grep over the files given (default: the S9 and S10 archive in the real tree).
# Prints  file:line: [construct] text   and a per-construct count. A hit is a candidate: RESULTS.md
# §5.1 classifies each (real problem on macOS / fine on BSD userland / not used on the Mac path).
# usage: scripts/scan_linuxisms.sh [file...]
set -eu
A=/home/user/kilohertz-pinball/docs/spikes/web
if [ $# -eq 0 ]; then
  set -- "$A/S9/build_web.sh" "$A/S9/wasm_env.sh" "$A/S9/env.sh" "$A/S9/check_imports.sh" \
    "$A/S9/run_integration.sh" "$A/S9/scripts/get_binaryen.sh" "$A/S9/scripts/size_report.sh" \
    "$A/S9/scripts/conflicts.sh" "$A/S9/web/serve.cjs" "$A"/S9/node/*.cjs "$A"/S9/node/*.mjs \
    "$A/S10/run_engines.sh"
fi
# name|ERE (applied per line; comments are not excluded on purpose: they may document usage)
PATTERNS='sha256sum|sha256sum
sed -i|sed[[:space:]]+(-[a-zA-Z]*i|--in-place)
sed -r/-E ext|sed[[:space:]]+-[a-zA-Z]*r[[:space:]]
GNU sed BRE escapes (plus, qmark, alternation)|sed[[:space:]].*[\\][+?|]
grep -P|grep[[:space:]]+-[a-zA-Z]*P
grep \s (GNU class escape)|grep[^|]*\\s
stat -c|stat[[:space:]]+-c
readlink -f|readlink[[:space:]]+-f
realpath|realpath
date -d|date[[:space:]]+-d
tar GNU-only flags|tar[^|]*(--wildcards|--transform|--xattrs|[[:space:]]-a[[:space:]])
cp -a/--parents/-al|cp[[:space:]]+(-a|--parents|-al)
xargs -r / -P|xargs[[:space:]]+(-r|-P|--no-run-if-empty)
find -printf|find[^|]*-printf
sort -V|sort[[:space:]]+-[a-zA-Z]*V
du -b|du[[:space:]]+-[a-zA-Z]*b
nproc|nproc
/proc|/proc/
apt / dpkg|(apt-get|apt |dpkg)
ldd|ldd[[:space:]]
ulimit|ulimit
mktemp|mktemp
bash shebang|^#!.*bash
local (not POSIX)|^[[:space:]]*local[[:space:]]
[[ ]] (bash)|\[\[
bash arrays / mapfile|(declare[[:space:]]+-[aA]|mapfile|readarray|\$\{[A-Za-z_]+\[)
${x,,} / ${x^^}|\$\{[A-Za-z_]+(,,|\^\^)
bash-4 redirections (&>> and pipe-amp)|(&>>|\|&)
$'...' (bash)|\$'"'"'
echo -e / -n|echo[[:space:]]+-[en]
timeout (coreutils)|(^|[[:space:];&])timeout[[:space:]]+[0-9]
/home/user or /root or /tmp literal|(/home/user|/root/|/tmp/)
unzip|unzip
wc -l < (BSD pads)|wc[[:space:]]+-[lc]
env -u|env[[:space:]]+-u
uname|uname
python3|python3
/opt/pw-browsers or VK ICD|(pw-browsers|VK_ICD|swiftshader)
process.platform/os.platform|(process\.platform|os\.platform|process\.arch)
path.sep / backslash|path\.sep'
for f in "$@"; do
  [ -f "$f" ] || continue
  case $f in *.sh) kind=sh ;; *) kind=js ;; esac
  printf '%s\n' "$PATTERNS" | while IFS='|' read -r name re; do
    case $kind:$name in
      js:process*|js:path.sep*|js:/home*|js:/opt*|sh:*) ;;
      *) continue ;;
    esac
    grep -n -E -e "$re" "$f" 2>/dev/null | while IFS= read -r hit; do
      printf '%s:%s: [%s]\n' "${f#"$A"/}" "$(printf '%s' "$hit" | cut -c1-160)" "$name"
    done
  done
done
