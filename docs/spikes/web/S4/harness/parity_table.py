"""Markdown parity table from logs/parity_full.log."""
import re
import sys

rows, cols, final = {}, [], {}
for line in open(sys.argv[1], encoding="utf-8"):
    m = re.match(r"(PASS|FAIL) (\S+) (\S+) (\S+)(.*)", line)
    if not m:
        b = re.match(r"BASE (\S+) .*state_hash=(\S+) ext_hash=(\S+) cmd_stream=(\S+) .*max_balls=(\d+)", line)
        if b:
            final[b.group(1)] = b.groups()[1:]
        continue
    v, scn, route, design, rest = m.groups()
    if route == "lua-vs-wasm":
        col = "native Lua (S1) = wasm rules"
    else:
        col = f"{route} {design}"
    if col not in cols:
        cols.append(col)
    fuel = " fuel=" + ("exact" if "wasmtime-exact" in rest else "differs") if "fuel=" in rest and "fuel_digest=0000" not in rest else ""
    rows.setdefault(scn, {})[col] = v + ("*" if "differs" in fuel else "")
print("| scenario (ticks) | final state_hash / cmd-stream hash | " + " | ".join(cols) + " |")
print("|---|---|" + "---|" * len(cols))
for scn, d in rows.items():
    f = final.get(scn, ("?", "?", "?", "?"))
    print(f"| {scn} | {f[0]} / {f[2]} | " + " | ".join(d.get(c, "–") for c in cols) + " |")
n_pass = sum(v.startswith("PASS") for d in rows.values() for v in d.values())
n_fail = sum(v.startswith("FAIL") for d in rows.values() for v in d.values())
print(f"\n{n_pass} PASS, {n_fail} FAIL. `*` = per-tick hashes identical but per-callback fuel differs from wasmtime (wasmi's own units).")
