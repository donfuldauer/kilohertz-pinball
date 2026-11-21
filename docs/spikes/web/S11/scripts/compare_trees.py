#!/usr/bin/env python3
"""S11: compare file trees by relative path and sha256 (read-only; run with `python3 -I`).

usage: compare_trees.py <label=dir> <label=dir> [<label=dir> ...] [--sub <relative subdir>]... [--out file.json]
For each --sub (default: the whole tree) prints: number of entries per tree, entries only in some trees,
and entries whose content (sha256, or symlink target) differs. The first tree is the reference.
"""
import hashlib
import json
import os
import sys


def walk(root):
    out = {}
    for dp, dn, fn in os.walk(root):
        dn.sort()
        for n in sorted(fn) + [d for d in dn if os.path.islink(os.path.join(dp, d))]:
            p = os.path.join(dp, n)
            rel = os.path.relpath(p, root)
            if os.path.islink(p):
                out[rel] = "link:" + os.readlink(p)
            elif os.path.isfile(p):
                h = hashlib.sha256()
                with open(p, "rb") as f:
                    for chunk in iter(lambda: f.read(1 << 20), b""):
                        h.update(chunk)
                out[rel] = h.hexdigest()
    return out


def main():
    args = sys.argv[1:]
    subs, trees, out = [], [], None
    i = 0
    while i < len(args):
        if args[i] == "--sub":
            subs.append(args[i + 1]); i += 2
        elif args[i] == "--out":
            out = args[i + 1]; i += 2
        else:
            lab, d = args[i].split("=", 1); trees.append((lab, d)); i += 1
    subs = subs or ["."]
    report = {}
    for sub in subs:
        maps = [(lab, walk(os.path.join(d, sub))) for lab, d in trees]
        keys = sorted(set().union(*[set(m) for _, m in maps]))
        only = {k: [lab for lab, m in maps if k in m] for k in keys if not all(k in m for _, m in maps)}
        diff = [k for k in keys if all(k in m for _, m in maps) and len(set(m[k] for _, m in maps)) > 1]
        same = sum(1 for k in keys if all(k in m for _, m in maps) and len(set(m[k] for _, m in maps)) == 1)
        r = {"entries": {lab: len(m) for lab, m in maps}, "identical_in_all": same, "differing": diff,
             "only_in_some": only}
        report[sub] = r
        print(f"== {sub}: entries {r['entries']}; identical in all {same}; differing {len(diff)}; only in some {len(only)}")
        for k in diff[:60]:
            print("  DIFF", k, " ".join(f"{lab}:{m[k][:16]}" for lab, m in maps))
        for k, labs in list(only.items())[:60]:
            print("  ONLY", k, "in", ",".join(labs))
    if out:
        with open(out, "w") as f:
            json.dump(report, f, indent=1)


if __name__ == "__main__":
    main()
