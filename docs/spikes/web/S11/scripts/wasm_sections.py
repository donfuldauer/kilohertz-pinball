#!/usr/bin/env python3
"""S11: compare two wasm modules section by section (read-only; run with `python3 -I`).

usage: wasm_sections.py A.wasm B.wasm [--strip-prefix A_PREFIX=B_PREFIX]...
Prints, per section id (and custom section name), size and sha256 in A and B. For the data
section, also lists the embedded path prefixes (strings like /x/.cargo/registry/src/) with counts, and
whether A's data equals B's data after replacing each A_PREFIX with its B_PREFIX (i.e. whether the
only data difference is the spelling of those host paths).
"""
import hashlib
import re
import sys


def leb(b, i):
    r = s = 0
    while True:
        x = b[i]; i += 1
        r |= (x & 0x7F) << s; s += 7
        if x < 0x80:
            return r, i


def sections(path):
    b = open(path, "rb").read()
    assert b[:4] == b"\0asm", path
    i, out = 8, []
    while i < len(b):
        sid = b[i]; i += 1
        n, i = leb(b, i)
        body = b[i:i + n]
        name = str(sid)
        if sid == 0:
            ln, j = leb(body, 0)
            name = "custom:" + body[j:j + ln].decode("utf-8", "replace")
        out.append((name, body))
        i += n
    return b, out


def main():
    a, b = sys.argv[1], sys.argv[2]
    maps = []
    if "--strip-prefix" in sys.argv:
        for k, v in enumerate(sys.argv):
            if v == "--strip-prefix":
                x, y = sys.argv[k + 1].split("=", 1)
                maps.append((x.encode(), y.encode()))
    ba, sa = sections(a)
    bb, sb = sections(b)
    print(f"A {a}: {len(ba)} B, sha256 {hashlib.sha256(ba).hexdigest()}")
    print(f"B {b}: {len(bb)} B, sha256 {hashlib.sha256(bb).hexdigest()}")
    names = {"1": "type", "2": "import", "3": "function", "4": "table", "5": "memory", "6": "global",
             "7": "export", "8": "start", "9": "element", "10": "code", "11": "data", "12": "datacount", "13": "tag"}
    da = dict(sa); db = dict(sb)
    for n in [x for x, _ in sa] + [x for x, _ in sb if x not in da]:
        xa, xb = da.get(n), db.get(n)
        ha = hashlib.sha256(xa).hexdigest()[:16] if xa is not None else "-"
        hb = hashlib.sha256(xb).hexdigest()[:16] if xb is not None else "-"
        print(f"  section {names.get(n, n):>22}: A {len(xa) if xa is not None else '-':>9} {ha}  B {len(xb) if xb is not None else '-':>9} {hb}  {'SAME' if ha == hb else 'DIFF'}")
    pat = re.compile(rb"(/[A-Za-z0-9._+-]+)+/registry/src/[a-z0-9.-]+/")
    for lab, d in (("A", da.get("11", b"")), ("B", db.get("11", b""))):
        c = {}
        for m in pat.finditer(d):
            c[m.group(0)] = c.get(m.group(0), 0) + 1
        print(f"  {lab} data: registry-path prefixes {dict((k.decode(), v) for k, v in c.items())}")
        r = sorted(set(m.group(0).decode() for m in re.finditer(rb"/rustc/[0-9a-f]{40}/", d)))
        print(f"  {lab} data: /rustc/<commit>/ prefixes {r}")
    if maps:
        x = da.get("11", b"")
        for p, q in maps:
            x = x.replace(p, q)
        print(f"  data(A) with {[(p.decode(), q.decode()) for p, q in maps]} replaced == data(B): {x == db.get('11', b'')}")


if __name__ == "__main__":
    main()
