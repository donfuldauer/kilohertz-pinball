"""S9: assembles the static test data next to dist/ (run with python3 -I).

usage: make_test_data.py <S9 root> <wasm-tools path>

* dist/bundles/<t>.pinbundle (S1's zipped production bundles, rules.lua) and
  dist/bundles/<t>_wasm.pinbundle (S4's: same files, script_entry rules.wasm):
  the bundles the native references were recorded from, byte for byte.
* dist/test/rec/: S1's scenarios (.scn) and script-output streams (.scr).
* dist/test/refs/<scn>.lua.ref: S1's native per-tick (state_hash, ext_hash);
  <scn>.wasm.ref: S4's native wasmtime engine-path (state, ext, cmd hash);
  feature_lab_bot60.<inject>.lua.ref: S2's native fault-injection runs.
* dist/test/player/: player bundles built from feature_lab (modified rules,
  S2's fault injections appended to rules.lua, a hostile rules.wasm).
* dist/test/zip/: S4's hostile zip corpus + S4's native classification.
"""

import hashlib
import os
import shutil
import struct
import subprocess
import sys
import zipfile

S9 = sys.argv[1]
WASM_TOOLS = sys.argv[2]
SPK = os.path.dirname(S9)
S1, S2, S4 = (os.path.join(SPK, x) for x in ("S1", "S2", "S4"))
DIST = os.path.join(S9, "dist")
TABLES = ["apex_circuit", "sky_deck", "feature_lab"]
SCENARIOS = [f"{t}_{k}" for t in TABLES for k in ("bot60", "pat60", "mb8")] + [
    "apex_circuit_bot600"
]
INJECTS = ["runtime_tick", "budget_tick", "memory_tick", "runtime_switch"]


def mkdir(*p):
    d = os.path.join(*p)
    os.makedirs(d, exist_ok=True)
    return d


def write_ref(src, dst, cols):
    """Text hash lines (tick h1 h2 [h3]) -> S9REF binary, ticks 0..n."""
    rows = []
    with open(src) as f:
        for i, line in enumerate(f):
            parts = line.split()
            if int(parts[0]) != i:
                raise SystemExit(f"{src}: tick {parts[0]} at line {i}")
            rows.append([int(x, 16) for x in parts[1 : 1 + cols]])
    with open(dst, "wb") as f:
        f.write(b"S9REF\0\0\0" + struct.pack("<IIQ", cols, 0, len(rows)))
        for r in rows:
            f.write(struct.pack(f"<{cols}Q", *r))
    return len(rows)


def rezip(src, dst, replace):
    """Copies zip `src` to `dst` with entries in `replace` (name -> bytes)."""
    with zipfile.ZipFile(src) as zin, zipfile.ZipFile(
        dst, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9
    ) as zout:
        for info in zin.infolist():
            data = replace.get(info.filename, None)
            if data is None:
                data = zin.read(info.filename)
            zi = zipfile.ZipInfo(info.filename, date_time=info.date_time)
            zi.compress_type = zipfile.ZIP_DEFLATED
            zi.external_attr = 0o644 << 16
            zout.writestr(zi, data)


def main():
    b = mkdir(DIST, "bundles")
    for t in TABLES:
        shutil.copyfile(os.path.join(S1, "bundles", f"{t}.pinbundle"), os.path.join(b, f"{t}.pinbundle"))
        shutil.copyfile(
            os.path.join(S4, "bundles", f"{t}_wasm.pinbundle"), os.path.join(b, f"{t}_wasm.pinbundle")
        )
    rec = mkdir(DIST, "test", "rec")
    refs = mkdir(DIST, "test", "refs")
    for s in SCENARIOS:
        for ext in ("scn", "scr"):
            shutil.copyfile(os.path.join(S1, "rec", f"{s}.{ext}"), os.path.join(rec, f"{s}.{ext}"))
        n1 = write_ref(os.path.join(S1, "native", f"{s}.hashes"), os.path.join(refs, f"{s}.lua.ref"), 2)
        n2 = write_ref(
            os.path.join(S4, "out", "native", f"{s}.engine.hashes"), os.path.join(refs, f"{s}.wasm.ref"), 3
        )
        print(f"refs {s}: lua {n1} rows, wasm {n2} rows")
    for i in INJECTS:
        n = write_ref(
            os.path.join(S2, "runs", "inject", f"feature_lab_bot60.{i}.native.hashes"),
            os.path.join(refs, f"feature_lab_bot60.{i}.lua.ref"),
            2,
        )
        print(f"refs inject {i}: {n} rows")
    # Player bundles from feature_lab.
    p = mkdir(DIST, "test", "player")
    fl = os.path.join(S1, "bundles", "feature_lab.pinbundle")
    with zipfile.ZipFile(fl) as z:
        rules = z.read("rules.lua")
        table = z.read("table.toml")
    mod = (
        rules
        + b"\n-- S9 player mod: a marker score every tick (proves the player's rules run).\n"
        + b"do local f = on_tick; on_tick = function(...) local r = f(...) "
        + b"pinball.set_score(1, 900009) return r end end\n"
    )
    table_mod = table.replace(b'name = "Feature Lab"', b'name = "Feature Lab (S9 player mod)"')
    if table_mod == table:
        raise SystemExit("feature_lab table.toml: meta name not found")
    rezip(fl, os.path.join(p, "feature_lab_player.pinbundle"), {"rules.lua": mod, "table.toml": table_mod})
    for i in INJECTS:
        with open(os.path.join(S2, "cases", "inject", f"{i}.lua"), "rb") as f:
            inj = f.read()
        # Exactly S2's in-memory injection: source + "\n" + inject.
        rezip(fl, os.path.join(p, f"feature_lab_{i}.pinbundle"), {"rules.lua": rules + b"\n" + inj})
    wasm = os.path.join(p, "fuelkill.wasm")
    subprocess.run([WASM_TOOLS, "parse", os.path.join(S9, "scripts", "fuelkill.wat"), "-o", wasm], check=True)
    with open(wasm, "rb") as f:
        fk = f.read()
    table_w = table.replace(b'script_entry = "rules.lua"', b'script_entry = "rules.wasm"')
    if table_w == table:
        raise SystemExit("feature_lab table.toml: script_entry not found")
    rezip(fl, os.path.join(p, "feature_lab_fuelkill.pinbundle"), {"rules.wasm": fk, "table.toml": table_w})
    # Hostile zips (S4) + S4's native classification (web limits).
    zd = mkdir(DIST, "test", "zip")
    for f in sorted(os.listdir(os.path.join(S4, "zipcorpus"))):
        if f.endswith(".zip"):
            shutil.copyfile(os.path.join(S4, "zipcorpus", f), os.path.join(zd, f))
    shutil.copyfile(os.path.join(S4, "out", "zip", "native.web.txt"), os.path.join(zd, "native.web.txt"))
    # The production rules.wasm (S4's adversarial mutation base).
    with zipfile.ZipFile(os.path.join(S1, "bundles", "apex_circuit.pinbundle")) as z:
        prod = z.read("rules.wasm")
    with open(os.path.join(DIST, "test", "apex_rules.wasm"), "wb") as f:
        f.write(prod)
    print("apex rules.wasm sha256", hashlib.sha256(prod).hexdigest())


main()
