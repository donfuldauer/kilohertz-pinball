#!/bin/sh
# S8 deliverable 1: the 32/64-bit audit probes (probes/*.lua, each prefixed
# with probes/_lib.lua) through the engine's LuaScriptBackend, natively
# (x86_64, run twice: ASLR), in wasm under Node and in Playwright Chromium;
# outputs diffed. Also the C sizeof probe. Prints a PASS/FAIL table with the
# audit verdict per probe.
# usage: run_audit.sh [v0|v1|v2]   (default v2 = all S8 patches)
# env: SKIP_BROWSER=1 skips Chromium; SKIP_BUILD=1 reuses runs/bin/<v>.
set -eu
S=${WEB_SPIKES}/S8
V=${1:-v2}
B=$S/runs/bin/$V
[ "${SKIP_BUILD:-0}" = 1 ] || "$S/harness/build_variant.sh" "$V" > "$S/logs/build_$V.log" 2>&1
O=$S/runs/audit/$V; rm -rf "$O"; mkdir -p "$O/src" "$O/native" "$O/native2" "$O/node" "$O/chromium" "$O/arm64"
# ARM=1: also the wasm module on V8's ARM64 backend (official Node 22.23.5 linux-arm64, sha256-checked, qemu-aarch64)
ARMNODE=$S/tools/node_arm64/node-v22.23.5-linux-arm64/bin/node
cfg() { case $1 in a_unpack_engine) echo "probe engine-bigbudget 1" ;; b_gc_engine) echo "probe engine 3000" ;;
        b_memstep_engine) echo "memtrace engine 400" ;; *) echo "probe big 1" ;; esac; }
verdict() { case $1 in
  sizes_c) echo "differs by design (struct sizes): neutralised by KHZ 33 logical sizes; limits reachable from scripts are identical" ;;
  a_limits) echo "identical (counts, not bytes)" ;;
  a_unpack_engine) echo "outcome identical before and after (M in the same callback, same units): the stack grows to the cap on both; accounted heap differs before KHZ 33" ;;
  b_gc_engine) echo "differs before KHZ 33 (cap point, KHZ 19 charge); fixed by KHZ 33" ;;
  b_memstep_engine) echo "differs before KHZ 33 (GC schedule in real bytes); fixed by KHZ 33" ;;
  c_tostring) echo "texts identical (KHZ 5/10: no addresses); budget units differ before KHZ 33: finalizer C calls (32 each) follow the GC schedule" ;;
  d_format) echo "identical on x86 hosts; NaN sign differs across CPUs (x86 vs ARM): fixed by KHZ 36" ;;
  e_numbers) echo "identical (64-bit lua_Integer/double everywhere; libm via KHZ 32)" ;;
  f_length) echo "identical (counts)" ;;
  g_pairs) echo "identical for value keys; ADDR lines differ (address hash, also native run to run): fixed by KHZ 34" ;;
  h_sort) echo "identical (KHZ 2 fixed pivot, unsigned int indices)" ;;
  i_misc) echo "differs before KHZ 35 (string positions >= 2^32 truncated by size_t on wasm32); fixed by KHZ 35" ;;
  j_api) echo "texts identical (mlua i64/f64, Rust float formatting); budget units differ before KHZ 33 (finalizers of host error userdata)" ;;
  k_strcache) echo "differs before KHZ 37 (luaS_new cache indexed by C pointer: long __name hits/misses change budget units and heap); fixed by KHZ 37" ;;
  nan_sign) echo "identical on x86 hosts; ARM64 V8 / aarch64 print the other NaN sign before KHZ 36; fixed by KHZ 36" ;;
  *) echo "-" ;; esac; }
JOBS=$O/jobs.json; printf '[' > "$JOBS"; first=1
for f in "$S"/probes/*.lua; do
  n=$(basename "$f" .lua); [ "$n" = _lib ] && continue
  cat "$S/probes/_lib.lua" "$f" > "$O/src/$n.lua"
  set -- $(cfg "$n")
  "$B/s2" s8 "$1" "@$O/src/$n.lua" "$2" "$3" > "$O/native/$n.txt"
  "$B/s2" s8 "$1" "@$O/src/$n.lua" "$2" "$3" > "$O/native2/$n.txt"
  node "$S/node/run.cjs" "$B/pkg_node" "$1" "@$O/src/$n.lua" "$2" "$3" > "$O/node/$n.txt"
  if [ "${ARM:-0}" = 1 ]; then qemu-aarch64 -L /usr/aarch64-linux-gnu "$ARMNODE" "$S/node/run.cjs" "$B/pkg_node" "$1" "@$O/src/$n.lua" "$2" "$3" > "$O/arm64/$n.txt"; fi
  [ $first = 1 ] || printf ',' >> "$JOBS"; first=0
  printf '{"id":"%s","cmd":"%s","args":["@%s.lua","%s","%s"]}' "$n" "$1" "$n" "$2" "$3" >> "$JOBS"
done
printf ']' >> "$JOBS"
if [ "${SKIP_BROWSER:-0}" != 1 ]; then
  W=$O/web; rm -rf "$W"; mkdir -p "$W/data"; cp "$S/web/index.html" "$S/web/main.js" "$W/"; cp -r "$B/pkg_web" "$W/pkg"
  cp "$O"/src/*.lua "$W/data/"; cp "$JOBS" "$W/jobs.json"
  node "$S/node/chromium.cjs" "$W" 8871 "$O/chromium" > "$O/chromium.log" 2>&1 || true
  rm -rf "$W"
fi
"$S/probes/c/build_sizes.sh" "$S/repo/third_party/lua-src/lua-5.4.9" > /dev/null 2>&1 || true
cp "$S/runs/probes/sizes.native.txt" "$O/native/sizes_c.txt"; cp "$S/runs/probes/sizes.native.txt" "$O/native2/sizes_c.txt"
cp "$S/runs/probes/sizes.wasm32.txt" "$O/node/sizes_c.txt"
echo "## run_audit $V ($("$B/s2" s8 features | tr -d '\n'))"
echo "| probe | lines | native run1 = run2 | native = Node | native = Chromium | native = ARM64 V8 | differing lines (node) | accounted heap native = Node | verdict |"
echo "|---|---|---|---|---|---|---|---|---|"
obs() { grep -a -v -E '^(HEAP|mem|REAL) ' "$1" || true; }  # script-observable lines
heap() { grep -a -E '^(HEAP|mem) ' "$1" || true; }     # harness-only: accounted heap
for d in native2 node chromium arm64; do for g in "$O/$d"/*.txt; do [ -f "$g" ] || continue
  grep -a -v -E '^(HEAP|mem|REAL) ' "$g" > "$g.obs" || true; grep -a -E '^(HEAP|mem) ' "$g" > "$g.heap" || true; done; done
FAIL=0
for f in "$O"/native/*.txt; do
  n=$(basename "$f" .txt)
  if [ "$n" = sizes_c ]; then
    r2=PASS; nd=$(cmp -s "$f" "$O/node/$n.txt" && echo PASS || echo FAIL); ch="-"; he="-"; ar="-"
    dl=$(diff -a "$f" "$O/node/$n.txt" | grep -a -c '^<' || true)
  else
    r2=$(obs "$f" | cmp -s - "$O/native2/$n.txt.obs" && echo PASS || echo FAIL)
    nd=$(obs "$f" | cmp -s - "$O/node/$n.txt.obs" && echo PASS || echo FAIL)
    if [ -f "$O/chromium/$n.txt" ]; then ch=$(obs "$f" | cmp -s - "$O/chromium/$n.txt.obs" && echo PASS || echo FAIL); else ch="-"; fi
    if [ -f "$O/arm64/$n.txt" ]; then ar=$(obs "$f" | cmp -s - "$O/arm64/$n.txt.obs" && echo PASS || echo FAIL); else ar="-"; fi
    dl=$(obs "$f" | diff -a - "$O/node/$n.txt.obs" | grep -a -c '^<' || true)
    he=$(heap "$f" | cmp -s - "$O/node/$n.txt.heap" && echo PASS || echo FAIL)
  fi
  [ "$nd" = PASS ] || [ "$n" = sizes_c ] || FAIL=$((FAIL + 1))
  echo "| $n | $(wc -l < "$f") | $r2 | $nd | $ch | $ar | $dl | $he | $(verdict "$n") |"
done
echo "== $V: $FAIL probe(s) with script-observable differences native vs Node (sizes_c excluded: it is the C layout itself)"
