"""Zips a bundle folder deterministically (sorted paths, fixed times, deflate).

usage: python3 -I make_zip.py <bundle_dir> <out.pinbundle>
"""
import os
import sys
import zipfile

src, out = sys.argv[1], sys.argv[2]
names = []
for root, _dirs, files in os.walk(src):
    for f in files:
        full = os.path.join(root, f)
        names.append(os.path.relpath(full, src).replace(os.sep, "/"))
with zipfile.ZipFile(out, "w") as z:
    for n in sorted(names):
        zi = zipfile.ZipInfo(n, date_time=(1980, 1, 1, 0, 0, 0))
        zi.compress_type = zipfile.ZIP_DEFLATED
        zi.external_attr = 0o644 << 16
        with open(os.path.join(src, n), "rb") as f:
            z.writestr(zi, f.read())
print(f"{out}: {len(names)} files, {os.path.getsize(out)} bytes")
