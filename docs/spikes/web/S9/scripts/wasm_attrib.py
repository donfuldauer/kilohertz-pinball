"""S3: attributes a wasm module's code bytes to crates using the name section.

Usage: python3 -I wasm_attrib.py <module.wasm> [top_n]
Prints JSON: total code bytes, data bytes, and code bytes per crate (from
the mangled function names; legacy `_ZN<len><crate>` and v0 `_R` forms).
"""
import json
import re
import sys


def leb(b, i):
    r = s = 0
    while True:
        x = b[i]; i += 1
        r |= (x & 0x7F) << s; s += 7
        if x < 0x80:
            return r, i


def main():
    b = open(sys.argv[1], "rb").read()
    top = int(sys.argv[2]) if len(sys.argv) > 2 else 25
    i = 8
    n_imports_fn = 0
    bodies = []
    names = {}
    data_bytes = 0
    custom = {}
    while i < len(b):
        sid = b[i]; i += 1
        size, i = leb(b, i)
        end = i + size
        if sid == 2:  # imports
            n, j = leb(b, i)
            for _ in range(n):
                l, j = leb(b, j); j += l
                l, j = leb(b, j); j += l
                kind = b[j]; j += 1
                if kind == 0:
                    _, j = leb(b, j); n_imports_fn += 1
                elif kind == 1:
                    j += 1; fl, j = leb(b, j); _, j = leb(b, j)
                    if fl & 1: _, j = leb(b, j)
                elif kind == 2:
                    fl, j = leb(b, j); _, j = leb(b, j)
                    if fl & 1: _, j = leb(b, j)
                elif kind == 3:
                    j += 2
                elif kind == 4:
                    j += 1; _, j = leb(b, j)
        elif sid == 10:  # code
            n, j = leb(b, i)
            for _ in range(n):
                l, j = leb(b, j)
                bodies.append(l)
                j += l
        elif sid == 11:
            data_bytes += size
        elif sid == 0:
            l, j = leb(b, i)
            nm = b[j:j + l].decode("utf8", "replace"); j += l
            custom[nm] = size
            if nm == "name":
                while j < end:
                    sub = b[j]; j += 1
                    ssz, j = leb(b, j)
                    send = j + ssz
                    if sub == 1:
                        cnt, k = leb(b, j)
                        for _ in range(cnt):
                            idx, k = leb(b, k)
                            l2, k = leb(b, k)
                            names[idx] = b[k:k + l2].decode("utf8", "replace"); k += l2
                    j = send
        i = end
    per = {}
    for n, sz in enumerate(bodies):
        name = names.get(n + n_imports_fn, "")
        crate = "?"
        m = re.match(r"_ZN(\d+)(\w+)", name)
        if m:
            crate = m.group(2)[: int(m.group(1))]
        elif name.startswith("_R"):
            m = re.search(r"C(?:s[0-9A-Za-z]*_)?(\d+)([A-Za-z_]\w*)", name)
            if m:
                crate = m.group(2)[: int(m.group(1))]
        elif name:
            crate = "<" + name.split("::")[0][:40] + ">"
        if crate.startswith("_"):
            crate = crate.lstrip("_")
        per[crate] = per.get(crate, 0) + sz
    total = sum(bodies)
    out = {
        "file": sys.argv[1], "bytes": len(b), "code_bytes": total, "data_bytes": data_bytes,
        "functions": len(bodies), "named": len(names), "custom_sections": custom,
        "per_crate": sorted(((k, v) for k, v in per.items()), key=lambda kv: -kv[1])[:top],
    }
    print(json.dumps(out, indent=1))


if __name__ == "__main__":
    main()
