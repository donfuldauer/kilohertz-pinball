"""Hostile and edge-case bundle archives for the zip/VFS hardening check.

usage: python3 -I make_zip_corpus.py <out_dir> <production.pinbundle>
Every file is written from scratch (stdlib zipfile + byte patches); nothing
is downloaded.
"""
import io
import os
import struct
import sys
import zipfile
import zlib

out, prod = sys.argv[1], sys.argv[2]
os.makedirs(out, exist_ok=True)


def write(name, data):
    with open(os.path.join(out, name + ".zip"), "wb") as f:
        f.write(data)


def zipof(entries, compression=zipfile.ZIP_DEFLATED, attrs=None, force64=False):
    b = io.BytesIO()
    with zipfile.ZipFile(b, "w", compression) as z:
        for i, (n, d) in enumerate(entries):
            zi = zipfile.ZipInfo(n, date_time=(1980, 1, 1, 0, 0, 0))
            zi.compress_type = compression
            zi.external_attr = (attrs[i] if attrs else 0o644) << 16
            if attrs:
                zi.create_system = 3
            with z.open(zi, "w", force_zip64=force64) as w:
                w.write(d)
    return b.getvalue()


def patch_all(data, sig, off, fmt, value):
    """Patches field `off` of every record with signature `sig`."""
    b = bytearray(data)
    i = 0
    while True:
        i = b.find(sig, i)
        if i < 0:
            break
        struct.pack_into(fmt, b, i + off, value)
        i += 4
    return bytes(b)


LOCAL, CENTRAL = b"PK\x03\x04", b"PK\x01\x02"
ok_table = [("table.toml", b"[meta]\nname='x'\n")]

write("00_production_bundle", open(prod, "rb").read())
write("01_small_ok", zipof(ok_table + [("a/b.txt", b"hello")]))
# Bombs.
write("10_bomb_ratio_single_50MiB", zipof([("zeros.bin", b"\0" * (50 << 20))]))
write("11_bomb_total_600x1MiB", zipof([(f"z/{i}.bin", b"\0" * (1 << 20)) for i in range(600)]))
write("12_bomb_archive_ratio_100x1MiB", zipof([(f"z/{i}.bin", b"\0" * (1 << 20)) for i in range(100)]))
write("13_bomb_70MiB_ratio_ok_web_cap", zipof([(f"r/{i}.bin", os.urandom(1 << 20)) for i in range(70)], compression=zipfile.ZIP_STORED))
lie = zipof([("lie.bin", b"\0" * (4 << 20))])
lie = patch_all(lie, CENTRAL, 24, "<I", 1000)  # central uncompressed size
lie = patch_all(lie, LOCAL, 22, "<I", 1000)
write("14_bomb_declared_size_lie", lie)
# Paths.
write("20_traversal_dotdot", zipof(ok_table + [("../evil.txt", b"x")]))
write("21_traversal_nested_dotdot", zipof(ok_table + [("a/../../evil.txt", b"x")]))
write("22_absolute_path", zipof(ok_table + [("/etc/passwd", b"x")]))
write("23_backslash_traversal", zipof(ok_table + [("..\\evil.txt", b"x")]))
write("24_drive_letter", zipof(ok_table + [("C:\\evil.txt", b"x")]))
nul = zipof(ok_table + [("aXb.txt", b"x")]).replace(b"aXb.txt", b"a\0b.txt")
write("25_nul_in_name", nul)
bad_utf8 = zipof(ok_table + [("aXb.txt", b"x")]).replace(b"aXb.txt", b"a\xffb.txt")
write("26_non_utf8_name", bad_utf8)
# Entries.
write("30_too_many_entries_10001", zipof([(f"e/{i}", b"") for i in range(10001)], compression=zipfile.ZIP_STORED))
dup = zipof(ok_table + [("a.txt", b"one"), ("b.txt", b"two")]).replace(b"b.txt", b"a.txt")
write("31_duplicate_names", dup)
write("32_case_collision", zipof(ok_table + [("A.txt", b"one"), ("a.txt", b"two")]))
write("33_symlink_entry", zipof(ok_table + [("link", b"/etc/passwd")], attrs=[0o644, 0o120777]))
inner = zipof([("zeros.bin", b"\0" * (20 << 20))])
write("34_nested_zip_bomb_inside", zipof(ok_table + [("inner.zip", inner)], compression=zipfile.ZIP_STORED))
enc = zipof(ok_table + [("secret.txt", b"x" * 100)])
enc = patch_all(enc, LOCAL, 6, "<H", 1)
enc = patch_all(enc, CENTRAL, 8, "<H", 1)
write("35_encrypted_flag", enc)
crc = zipof(ok_table + [("c.txt", b"crc-me" * 100)])
crc = patch_all(crc, LOCAL, 14, "<I", 0xDEADBEEF)
crc = patch_all(crc, CENTRAL, 16, "<I", 0xDEADBEEF)
write("36_bad_crc", crc)
write("37_stored_method_ok", zipof(ok_table + [("s.txt", b"stored")], compression=zipfile.ZIP_STORED))
bz = zipof(ok_table + [("m.txt", b"method")])
bz = patch_all(bz, LOCAL, 8, "<H", 12)  # bzip2
bz = patch_all(bz, CENTRAL, 10, "<H", 12)
write("38_unsupported_method_bzip2", bz)
# Structure.
write("40_zip64_forced_ok", zipof(ok_table + [("z64.txt", b"zip64 entry")], force64=True))
z = zipof(ok_table, force64=True)
w = bytearray(zipof(ok_table))
# A saturated EOCD with a locator pointing outside the archive.
eocd = w.rfind(b"PK\x05\x06")
w[eocd + 10:eocd + 12] = b"\xff\xff"
w[eocd + 8:eocd + 10] = b"\xff\xff"
loc = b"PK\x06\x07" + struct.pack("<IQI", 0, 0xFFFFFFFF00, 1)
write("41_zip64_bad_locator", bytes(w[:eocd]) + loc + bytes(w[eocd:]))
p = open(prod, "rb").read()
write("42_truncated_central_directory", p[: len(p) - 40])
write("43_truncated_half", p[: len(p) // 2])
write("44_prepended_junk", b"MZ" + b"\0" * 1000 + zipof(ok_table))
write("45_trailing_junk", zipof(ok_table) + b"junk" * 10)
cm = bytearray(zipof(ok_table))
e = cm.rfind(b"PK\x05\x06")
cm[e + 20:e + 22] = struct.pack("<H", 0xFFFF)
write("46_comment_length_lie", bytes(cm))
write("47_empty_file", b"")
write("48_not_a_zip", b"this is not a zip file at all" * 10)
two = zipof(ok_table)
write("49_two_eocd_records", two + two[two.rfind(b"PK\x05\x06"):])
print(f"{len(os.listdir(out))} files in {out}")
