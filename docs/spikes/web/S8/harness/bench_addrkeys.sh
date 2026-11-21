#!/bin/sh
# KHZ 34 micro-benchmark: wall time of probes/pairs/bench_addrkeys.lua (median of 5), v1 vs v2, native and Node.
S=${WEB_SPIKES}/S8
P=$S/probes/pairs/bench_addrkeys.lua
for V in v1 v2; do
  for plat in native node; do
    ts=""
    for r in 1 2 3 4 5; do
      t0=$(date +%s%N)
      if [ $plat = native ]; then "$S/runs/bin/$V/s2" s8 probe "@$P" big 1 > /dev/null; else node "$S/node/run.cjs" "$S/runs/bin/$V/pkg_node" probe "@$P" big 1 > /dev/null; fi
      ts="$ts $(( ($(date +%s%N) - t0) / 1000000 ))"
    done
    echo "$V $plat ms:$ts median=$(echo $ts | tr ' ' '\n' | sort -n | sed -n 3p)"
  done
done
