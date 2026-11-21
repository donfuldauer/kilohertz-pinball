"""Compares two hash files (lines `tick state_hash ext_hash`).

usage: python3 -I compare.py <label> <reference.hashes> <candidate.hashes>
Prints one PASS/FAIL line; on FAIL the first diverging tick and both lines.
Only ticks present in both files are compared; differing tick sets FAIL.
"""
import sys


def load(p):
    d = {}
    with open(p) as f:
        for line in f:
            parts = line.split()
            if len(parts) == 3:
                d[int(parts[0])] = (parts[1], parts[2])
    return d


label, ra, rb = sys.argv[1], load(sys.argv[2]), load(sys.argv[3])
ticks = sorted(set(ra) | set(rb))
missing = [t for t in ticks if t not in ra or t not in rb]
first_state = next((t for t in ticks if t in ra and t in rb and ra[t][0] != rb[t][0]), None)
first_ext = next((t for t in ticks if t in ra and t in rb and ra[t][1] != rb[t][1]), None)
n = sum(1 for t in ticks if t in ra and t in rb)
if first_state is None and first_ext is None and not missing:
    print(f"PASS {label}: {n} ticks (0..{ticks[-1]}) state_hash and ext_hash identical; final {ra[ticks[-1]][0]}")
    sys.exit(0)
msg = f"FAIL {label}: compared {n} ticks"
if missing:
    msg += f"; {len(missing)} ticks missing on one side (first {missing[0]})"
if first_state is not None:
    msg += f"; first state_hash divergence at tick {first_state}: {ra[first_state][0]} vs {rb[first_state][0]}"
if first_ext is not None:
    msg += f"; first ext_hash divergence at tick {first_ext}: {ra[first_ext][1]} vs {rb[first_ext][1]}"
print(msg)
sys.exit(1)
