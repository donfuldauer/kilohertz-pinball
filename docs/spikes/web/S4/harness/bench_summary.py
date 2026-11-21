"""Per-callback cost table from out/bench/cb.<route>.<design>.<table>.<run>.json
(median over runs of each statistic). usage: python3 -I bench_summary.py <dir>"""
import glob
import json
import os
import statistics
import sys

d = sys.argv[1]
acc = {}
for f in glob.glob(os.path.join(d, "cb.*.json")):
    parts = os.path.basename(f).split(".")
    route, design, table = parts[1], parts[2], parts[3]
    txt = open(f, encoding="utf-8").read().strip().splitlines()
    if not txt or not txt[0].startswith("{"):
        continue
    j = json.loads(txt[0])
    acc.setdefault((route, design, table), []).append(j)
print(f"{'route':9s} {'design':19s} {'table':13s} {'cb mean us':>10s} {'p50':>7s} {'p99':>7s} {'p99.9':>7s} {'max':>8s} {'tick p99':>8s} {'rules ms/60k':>12s} {'run ms':>8s} {'load ms':>8s} {'maxfuel':>7s}")
for (route, design, table), js in sorted(acc.items()):
    med = lambda f: statistics.median(f(j) for j in js)
    print(f"{route:9s} {design:19s} {table:13s} {med(lambda j: j['all']['mean_us']):10.3f} {med(lambda j: j['all']['p50_us']):7.2f} {med(lambda j: j['all']['p99_us']):7.2f} {med(lambda j: j['all']['p999_us']):7.2f} {med(lambda j: j['all']['max_us']):8.1f} {med(lambda j: j['tick']['p99_us']):8.2f} {med(lambda j: j['rules_ms']):12.1f} {med(lambda j: j['run_ms']):8.0f} {med(lambda j: j['load_ms']):8.2f} {int(med(lambda j: j['max_fuel'])):7d}")
