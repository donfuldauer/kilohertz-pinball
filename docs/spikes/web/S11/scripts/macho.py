#!/usr/bin/env python3
"""S11: read-only Mach-O inspector (never executes anything). Run with `python3 -I`.

usage: macho.py <root-dir> [--json out.json]
Walks <root-dir>, and for every Mach-O file (thin or fat) prints:
  path, file type, per-slice arch, LC_BUILD_VERSION / LC_VERSION_MIN_MACOSX (minos, sdk),
  dylib dependencies (LC_LOAD_DYLIB & co.), rpaths, and the code signature:
  none | ad-hoc (incl. linker-signed) | CMS (with the certificate subject strings, team id).
"""
import json
import os
import re
import struct
import sys

FAT_MAGIC = 0xCAFEBABE
FAT_MAGIC_64 = 0xCAFEBABF
MH_MAGIC_64 = 0xFEEDFACF
MH_MAGIC = 0xFEEDFACE
CPU = {0x0100000C: "arm64", 0x01000007: "x86_64", 0x7: "i386", 0xC: "arm", 0x0200000C: "arm64_32"}
FILETYPE = {1: "object", 2: "execute", 6: "dylib", 8: "bundle", 7: "dylinker", 0xB: "dsym"}
PLATFORM = {1: "macos", 2: "ios", 3: "tvos", 4: "watchos", 6: "maccatalyst", 7: "iossim"}
LC_DYLIBS = {0xC: "load", 0x80000018: "weak", 0x8000001F: "reexport", 0x20: "lazy", 0x80000023: "upward"}
LC_RPATH = 0x8000001C
LC_BUILD_VERSION = 0x32
LC_VERSION_MIN_MACOSX = 0x24
LC_CODE_SIGNATURE = 0x1D
LC_ID_DYLIB = 0xD


def ver(v):
    return f"{v >> 16}.{(v >> 8) & 0xFF}.{v & 0xFF}"


def cstr(b, off):
    end = b.find(b"\0", off)
    return b[off:end if end >= 0 else len(b)].decode("utf-8", "replace")


def parse_sig(data):
    """Parse an embedded code signature SuperBlob (big-endian)."""
    out = {"kind": "invalid"}
    if len(data) < 12:
        return out
    magic, length, count = struct.unpack(">III", data[:12])
    if magic != 0xFADE0CC0:
        return out
    cd = None
    cms_len = 0
    cms = b""
    slots = []
    for i in range(count):
        typ, off = struct.unpack(">II", data[12 + 8 * i:20 + 8 * i])
        slots.append(hex(typ))
        bmagic, blen = struct.unpack(">II", data[off:off + 8])
        blob = data[off:off + blen]
        if typ == 0 and bmagic == 0xFADE0C02:
            cd = blob
        elif typ == 0x10000 and bmagic == 0xFADE0B01:
            cms_len = blen - 8
            cms = blob[8:]
    out = {"slots": slots}
    if cd is not None:
        (version, flags, hash_off, ident_off, n_special, n_code, code_limit, hash_size, hash_type, platform,
         page_size) = struct.unpack(">IIIIIIIBBBB", cd[8:40])
        out["cd_version"] = hex(version)
        out["flags"] = hex(flags)
        out["identifier"] = cstr(cd, ident_off)
        out["hash"] = {1: "sha1", 2: "sha256", 3: "sha256_trunc", 4: "sha384"}.get(hash_type, hash_type)
        team = ""
        if version >= 0x20200 and len(cd) >= 52:
            team_off = struct.unpack(">I", cd[48:52])[0]
            if team_off:
                team = cstr(cd, team_off)
        out["team_id"] = team
        out["adhoc"] = bool(flags & 0x2)
        out["linker_signed"] = bool(flags & 0x20000)
        out["hardened_runtime"] = bool(flags & 0x10000)
    out["cms_bytes"] = cms_len
    if cms_len > 0:
        subj = sorted(set(m.decode("latin-1") for m in re.findall(rb"Developer ID [A-Za-z ]+: [^\x00-\x1f]{1,80}?\([A-Z0-9]{10}\)", cms)))
        if not subj:
            subj = sorted(set(m.decode("latin-1") for m in re.findall(rb"(Developer ID[^\x00-\x1f]{0,80})", cms)))
        out["cms_subjects"] = subj[:4]
        out["apple_root"] = b"Apple Root CA" in cms
    if cd is None:
        out["kind"] = "no-codedirectory"
    elif cms_len > 0:
        out["kind"] = "CMS (Developer ID)" if out.get("cms_subjects") else "CMS"
    elif out.get("linker_signed"):
        out["kind"] = "ad-hoc (linker-signed)"
    elif out.get("adhoc"):
        out["kind"] = "ad-hoc"
    else:
        out["kind"] = "codedirectory-without-cms"
    return out


def parse_thin(b, base=0):
    magic = struct.unpack("<I", b[base:base + 4])[0]
    if magic not in (MH_MAGIC_64, MH_MAGIC):
        return None
    is64 = magic == MH_MAGIC_64
    cputype, cpusub, filetype, ncmds, sizeofcmds, flags = struct.unpack("<iiIIII", b[base + 4:base + 28])
    off = base + (32 if is64 else 28)
    s = {"arch": CPU.get(cputype & 0xFFFFFFFF, hex(cputype & 0xFFFFFFFF)), "filetype": FILETYPE.get(filetype, filetype),
         "minos": None, "sdk": None, "platform": None, "dylibs": [], "rpaths": [], "signature": {"kind": "none"}}
    for _ in range(ncmds):
        cmd, cmdsize = struct.unpack("<II", b[off:off + 8])
        if cmd == LC_BUILD_VERSION:
            platform, minos, sdk = struct.unpack("<III", b[off + 8:off + 20])
            s["platform"] = PLATFORM.get(platform, platform)
            s["minos"] = ver(minos)
            s["sdk"] = ver(sdk)
        elif cmd == LC_VERSION_MIN_MACOSX:
            v, sdk = struct.unpack("<II", b[off + 8:off + 16])
            s["platform"] = "macos(version_min)"
            s["minos"] = ver(v)
            s["sdk"] = ver(sdk)
        elif cmd in LC_DYLIBS:
            name_off = struct.unpack("<I", b[off + 8:off + 12])[0]
            s["dylibs"].append((LC_DYLIBS[cmd], cstr(b, off + name_off)))
        elif cmd == LC_RPATH:
            p_off = struct.unpack("<I", b[off + 8:off + 12])[0]
            s["rpaths"].append(cstr(b, off + p_off))
        elif cmd == LC_ID_DYLIB:
            name_off = struct.unpack("<I", b[off + 8:off + 12])[0]
            s["id"] = cstr(b, off + name_off)
        elif cmd == LC_CODE_SIGNATURE:
            dataoff, datasize = struct.unpack("<II", b[off + 8:off + 16])
            s["signature"] = parse_sig(b[base + dataoff:base + dataoff + datasize])
        off += cmdsize
    return s


def parse(path):
    with open(path, "rb") as f:
        head = f.read(8)
        if len(head) < 8:
            return None
        m_be = struct.unpack(">I", head[:4])[0]
        m_le = struct.unpack("<I", head[:4])[0]
        if m_le not in (MH_MAGIC_64, MH_MAGIC) and m_be not in (FAT_MAGIC, FAT_MAGIC_64):
            return None
        f.seek(0)
        b = f.read()
    if m_be in (FAT_MAGIC, FAT_MAGIC_64):
        nfat = struct.unpack(">I", b[4:8])[0]
        if nfat > 16:  # Java class files share 0xCAFEBABE
            return None
        slices = []
        for i in range(nfat):
            if m_be == FAT_MAGIC:
                cputype, cpusub, offset, size, align = struct.unpack(">iiIII", b[8 + 20 * i:28 + 20 * i])
            else:
                cputype, cpusub, offset, size, align, _r = struct.unpack(">iiQQII", b[8 + 32 * i:40 + 32 * i])
            t = parse_thin(b, offset)
            if t:
                slices.append(t)
        return {"fat": True, "slices": slices}
    return {"fat": False, "slices": [parse_thin(b)]}


SYSTEM_PREFIXES = ("/usr/lib/", "/System/Library/")


def main():
    root = sys.argv[1]
    out_json = sys.argv[sys.argv.index("--json") + 1] if "--json" in sys.argv else None
    rows = []
    for dp, dn, fn in os.walk(root):
        dn.sort()
        for n in sorted(fn):
            p = os.path.join(dp, n)
            if os.path.islink(p) or not os.path.isfile(p):
                continue
            r = parse(p)
            if not r:
                continue
            st = os.stat(p)
            r["path"] = os.path.relpath(p, root)
            r["mode"] = oct(st.st_mode & 0o777)
            r["size"] = st.st_size
            rows.append(r)
    nonsys = set()
    for r in rows:
        for s in r["slices"]:
            for kind, d in s["dylibs"]:
                if not d.startswith(SYSTEM_PREFIXES):
                    nonsys.add(d)
    summary = {
        "macho_files": len(rows),
        "archs": sorted(set("+".join(s["arch"] for s in r["slices"]) for r in rows)),
        "minos": sorted(set(s["minos"] or "none" for r in rows for s in r["slices"])),
        "sdk": sorted(set(s["sdk"] or "none" for r in rows for s in r["slices"])),
        "signature_kinds": sorted(set(s["signature"]["kind"] for r in rows for s in r["slices"])),
        "unsigned_files": [r["path"] for r in rows if any(s["signature"]["kind"] == "none" for s in r["slices"])],
        "non_system_dylibs": sorted(nonsys),
        "rpaths": sorted(set(p for r in rows for s in r["slices"] for p in s["rpaths"])),
        "team_ids": sorted(set(s["signature"].get("team_id", "") for r in rows for s in r["slices"]) - {""}),
        "cms_subjects": sorted(set(x for r in rows for s in r["slices"] for x in s["signature"].get("cms_subjects", []))),
        "hardened_runtime_files": sum(1 for r in rows if any(s["signature"].get("hardened_runtime") for s in r["slices"])),
    }
    print(json.dumps(summary, indent=1))
    for r in rows:
        for s in r["slices"]:
            sig = s["signature"]
            dl = ",".join(d for _, d in s["dylibs"])
            print(f"{r['path']}\t{r['mode']}\t{s['arch']}{' (fat)' if r['fat'] else ''}\t{s['filetype']}\tminos={s['minos']}\tsdk={s['sdk']}"
                  f"\tsig={sig['kind']}\tflags={sig.get('flags', '-')}\tteam={sig.get('team_id', '') or '-'}\tdylibs={dl}"
                  f"\trpaths={','.join(s['rpaths']) or '-'}")
    if out_json:
        with open(out_json, "w") as f:
            json.dump({"summary": summary, "files": rows}, f, indent=1)


if __name__ == "__main__":
    main()
