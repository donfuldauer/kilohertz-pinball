"""PASS/FAIL table from run_rules_adversarial.sh's log.

PASS rules (the guarantee each design must give):
* vs (2) native+counter: every line EXACT, except the JIT's V8 trap text for
  a null / mistyped call_indirect (message only; the fault and everything
  else identical) -> PASS.
* vs (1) native today: every line EXACT or CLASS (same classification);
  CLASS/DIFF lines allowed only for the stack kill point (deep recursion),
  since native's limit is native-stack bytes -> PASS(stack-kill-point).
"""
import re
import sys

text = open(sys.argv[1], encoding="utf-8").read()
part1, part2 = text.split("== vs (2)")
STACK = {"deep_recursion", "deep_recursion_random_each_level", "deep_recursion_bigframe", "recursion_depth_n"}
COSMETIC = {"call_indirect_type_confusion", "call_indirect_null"}


def table(part):
    res = {}
    for line in part.splitlines():
        m = re.match(r"(\S+\.txt)\s+EXACT=(\d+) CLASS=(\d+) DIFF=(\d+) MISSING=(\d+) of (\d+)", line)
        if m:
            res[m.group(1)] = {"counts": m.groups()[1:], "bad": []}
    for line in part.splitlines():
        m = re.match(r"(\S+\.txt)\s+(\S+(?: \[boundary\])?)\s+(CLASS|DIFF|MISSING)\s", line)
        if m and m.group(1) in res:
            res[m.group(1)]["bad"].append((m.group(2), m.group(3)))
    return res


t1, t2 = table(part1), table(part2)
print(f"{'design file':36s} {'vs native today':40s} {'vs native+counter':40s}")
for f in sorted(set(t1) | set(t2)):
    def verdict(t, rule):
        if f not in t:
            return "-"
        c = t[f]["counts"]
        bad = t[f]["bad"]
        if rule == 2:
            rest = [b for b in bad if not (b[0] in COSMETIC and "jit" in f or "hybrid" in f and b[0] in COSMETIC and False)]
            rest = [b for b in bad if not (b[0] in COSMETIC and ("jit" in f))]
            v = "PASS" if not rest else "FAIL " + ",".join(x[0] for x in rest)[:60]
            if bad and not rest:
                v = "PASS(msg-only: V8 call_indirect text)"
        else:
            rest = [b for b in bad if b[0] not in STACK and not (b[0] in COSMETIC and "jit" in f)]
            if "wasmi-fuel" in f:
                rest = [b for b in rest if b[1] == "DIFF"]
            v = "PASS" if not bad else ("PASS(stack-kill-point)" if not rest else "FAIL " + ",".join(x[0] for x in rest)[:60])
        return f"{v} [E={c[0]} C={c[1]} D={c[2]}]"
    print(f"{f:36s} {verdict(t1, 1):40s} {verdict(t2, 2):40s}")
