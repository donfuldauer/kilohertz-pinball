"""Tabulates S7 mini probe outputs (probes/mini/<variant>.txt) against x86_64_default.
usage: python3 -I mini_table.py <dir> <variant>..."""
import os
import sys

d, cols = sys.argv[1], sys.argv[2:]
data = {}
for c in cols:
    p = os.path.join(d, c + ".txt")
    data[c] = {}
    if os.path.exists(p):
        for l in open(p):
            if "=" in l and not l.startswith(("target", "portable")):
                k, v = l.strip().split("=", 1)
                data[c][k] = v
ref, scal, a64 = data[cols[0]], data.get("x86_64_scalar_math", {}), data.get("aarch64_default", {})
print("%-25s" % "op" + "".join("%-19s" % c[:18] for c in cols))
for k in ref:
    row = "%-25s" % k + "%-19s" % ref[k]
    for c in cols[1:]:
        v = data[c].get(k)
        if v is None:
            t = "(no output)"
        elif v == ref[k]:
            t = "= x86 SSE2"
        elif v == scal.get(k) and c != "x86_64_scalar_math":
            t = "= scalar-math"
        elif v == a64.get(k) and c != "aarch64_default":
            t = "= aarch64 NEON"
        else:
            t = v
        row += "%-19s" % t
    print(row)
for c in cols:
    p = os.path.join(d, c + ".txt")
    l = [x.strip() for x in open(p) if x.startswith("portable")] if os.path.exists(p) else []
    print(f"{c}: {l[0] if l else '(no output)'}")
