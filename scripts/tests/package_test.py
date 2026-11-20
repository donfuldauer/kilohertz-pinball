#!/usr/bin/env python3
"""Tests for scripts/package.py and .github/workflows/release.yml (PLAN 7.2).

Run: ``python3 -I -m unittest discover -s scripts/tests -p '*_test.py'`` from
the repository root. Hermetic: every case builds fake bundle folders, a fake
executable and a fake DOF bridge publish dir in a temp dir. Builds are
exercised with fake ``cargo`` / ``sh`` / ``lipo`` / ``dotnet`` / ``codesign``
scripts first on PATH that log their arguments; the real tools are never run.
"""

import hashlib
import importlib.util
import io
import json
import os
import plistlib
import re
import shutil
import stat
import struct
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
PKG = REPO / "scripts" / "package.py"
WORKFLOW = REPO / ".github" / "workflows" / "release.yml"

WIN = "x86_64-pc-windows-msvc"
LINUX = "x86_64-unknown-linux-gnu"
MAC = "universal-apple-darwin"
TABLES = ("greybox", "apex_circuit", "sky_deck", "feature_lab")
PORTED = ("apex_circuit", "sky_deck", "feature_lab")
APP = "Kilohertz Pinball.app"


def load_module():
    spec = importlib.util.spec_from_file_location("package_under_test", PKG)
    mod = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(mod)
    return mod


pkgmod = load_module()


def cargo_version():
    text = (REPO / "Cargo.toml").read_text(encoding="utf-8")
    section = text.split("[workspace.package]", 1)[1]
    return re.search(r'^version\s*=\s*"([^"]+)"', section, re.M).group(1)


def make_bundle(folder, name, wasm=None):
    """A fake bundle folder: table.toml, rules.lua, a nested file, rules.wasm
    for ported tables (unless ``wasm`` says otherwise)."""
    folder.mkdir(parents=True)
    (folder / "table.toml").write_text(f'name = "{name}"\n', encoding="utf-8")
    (folder / "rules.lua").write_text(f"-- rules for {name}\n", encoding="utf-8")
    (folder / "display").mkdir()
    (folder / "display" / "attract.toml").write_text("frames = 3\n" * 50, encoding="utf-8")
    (folder / "playfield.glb").write_bytes(bytes(range(256)) * 16)
    if wasm if wasm is not None else name in PORTED:
        (folder / "rules.wasm").write_bytes(b"\0asm\x01\0\0\0" + name.encode())


def make_bundles(root, skip_wasm=(), extra_wasm=()):
    for name in TABLES:
        wasm = (name in PORTED and name not in skip_wasm) or name in extra_wasm
        make_bundle(root / name, name, wasm)
    return root


def make_dof(root):
    root.mkdir(parents=True)
    (root / "KilohertzDofBridge.exe").write_bytes(b"MZ fake bridge")
    (root / "KilohertzDofBridge.dll").write_bytes(b"fake dll")
    (root / "runtimes" / "win").mkdir(parents=True)
    (root / "runtimes" / "win" / "native.dll").write_bytes(b"native")
    return root


FAKE_TOOL = """#!/bin/sh
echo "$(basename "$0") $*" >> "$TOOL_LOG"
case "$(basename "$0")" in
  cargo)
    t=""; prev=""
    for a in "$@"; do [ "$prev" = "--target" ] && t="$a"; prev="$a"; done
    mkdir -p "$CARGO_TARGET_DIR/$t/release"
    printf 'exe %s' "$t" > "$CARGO_TARGET_DIR/$t/release/pin_app"
    printf 'exe %s' "$t" > "$CARGO_TARGET_DIR/$t/release/pin_app.exe"
    ;;
  lipo)
    out=""; prev=""
    for a in "$@"; do [ "$prev" = "-output" ] && out="$a"; prev="$a"; done
    printf 'universal' > "$out"
    ;;
  sh)
    # sh tables/<name>/build.sh <out>
    name=$(basename "$(dirname "$1")"); out="$2"
    mkdir -p "$out"
    printf 'name = "%s"\\n' "$name" > "$out/table.toml"
    printf -- '-- %s\\n' "$name" > "$out/rules.lua"
    [ "$name" = greybox ] || printf 'wasm' > "$out/rules.wasm"
    ;;
  dotnet)
    out=""; prev=""
    for a in "$@"; do [ "$prev" = "-o" ] && out="$a"; prev="$a"; done
    mkdir -p "$out"; printf 'MZ' > "$out/KilohertzDofBridge.exe"
    ;;
  codesign)
    [ -n "$CODESIGN_FAIL" ] && exit 3
    ;;
esac
exit 0
"""


class PackageCase(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory(prefix="pkgtest-")
        self.tmp = Path(self._tmp.name)
        self.bundles = make_bundles(self.tmp / "bundles")
        self.exe = self.tmp / "engine-bin"
        self.exe.write_bytes(b"\x7fELF fake engine")
        self.dof = make_dof(self.tmp / "dof")
        self.out = self.tmp / "out"
        # Fake tools: every external command the script may run logs here.
        self.bin = self.tmp / "bin"
        self.bin.mkdir()
        self.log = self.tmp / "tools.log"
        self.log.write_text("")
        for tool in ("cargo", "lipo", "sh", "dotnet", "codesign", "git", "gh", "rustup", "curl"):
            p = self.bin / tool
            p.write_text(FAKE_TOOL)
            p.chmod(0o755)

    def tearDown(self):
        self._tmp.cleanup()

    def env(self, **extra):
        e = dict(os.environ)
        e["PATH"] = f"{self.bin}{os.pathsep}{e.get('PATH', '')}"
        e["TOOL_LOG"] = str(self.log)
        e["CARGO_TARGET_DIR"] = str(self.tmp / "cargo-target")
        e.pop("CODESIGN_FAIL", None)
        e.update(extra)
        return e

    def run_pkg(self, *args, env=None):
        return subprocess.run(
            [sys.executable, "-I", str(PKG), *map(str, args)],
            capture_output=True, text=True, env=env or self.env(), cwd=str(self.tmp), timeout=120,
        )

    def package(self, target, *extra, out=None, prebuilt=True, expect=0):
        args = ["--target", target, "--out", out or self.out, "--version", "1.2.3"]
        if prebuilt:
            args += ["--exe", self.exe, "--bundle-dir", self.bundles]
            if target == WIN:
                args += ["--dof-bridge", self.dof]
        p = self.run_pkg(*args, *extra)
        self.assertEqual(p.returncode, expect, f"stdout:\n{p.stdout}\nstderr:\n{p.stderr}")
        return p

    def tools_called(self):
        return [line.split()[0] for line in self.log.read_text().splitlines() if line.strip()]

    def manifest(self, out=None):
        return json.loads(((out or self.out) / "PACKAGE_MANIFEST.json").read_text())

    def pkgdir(self, target, out=None):
        plat = {WIN: "windows-x64", LINUX: "linux-x64", MAC: "macos-universal"}[target]
        return (out or self.out) / f"kilohertz-pinball-1.2.3-{plat}"


class LayoutTest(PackageCase):
    def test_linux_layout(self):
        self.package(LINUX)
        pkg = self.pkgdir(LINUX)
        paths = [f["path"] for f in self.manifest()["files"]]
        want = {"kilohertz-pinball", "LICENSE-MIT", "LICENSE-APACHE", "README.md"}
        want |= {f"tables/{t}.pinbundle" for t in TABLES}
        if (REPO / "docs" / "HUMAN_ACCEPTANCE.md").is_file():
            want.add("docs/HUMAN_ACCEPTANCE.md")
        self.assertEqual(set(paths), want)
        self.assertEqual(stat.S_IMODE((pkg / "kilohertz-pinball").stat().st_mode), 0o755)
        self.assertEqual((pkg / "kilohertz-pinball").read_bytes(), self.exe.read_bytes())
        self.assertFalse(any("dof_bridge" in p for p in paths))
        self.assertFalse(any("dof_bridge" in str(p) for p in pkg.rglob("*")))
        self.assertTrue((self.out / "kilohertz-pinball-1.2.3-linux-x64.tar.gz").is_file())
        self.assertFalse((self.out / "kilohertz-pinball-1.2.3-linux-x64.zip").exists())

    def test_windows_layout_has_dof_bridge(self):
        self.package(WIN)
        pkg = self.pkgdir(WIN)
        paths = set(f["path"] for f in self.manifest()["files"])
        for p in ("kilohertz-pinball.exe", "dof_bridge/KilohertzDofBridge.exe", "dof_bridge/KilohertzDofBridge.dll",
                  "dof_bridge/runtimes/win/native.dll", "dof_bridge/UNVERIFIED.txt", "LICENSE-MIT",
                  "LICENSE-APACHE", "README.md"):
            self.assertIn(p, paths)
        for t in TABLES:
            self.assertIn(f"tables/{t}.pinbundle", paths)
        self.assertNotIn("kilohertz-pinball", paths)
        note = (pkg / "dof_bridge" / "UNVERIFIED.txt").read_text()
        self.assertIn("unverified on hardware", note.lower())
        self.assertIn("D33", note)
        archive = self.out / "kilohertz-pinball-1.2.3-windows-x64.zip"
        with zipfile.ZipFile(archive) as zf:
            names = zf.namelist()
        self.assertEqual(names, sorted(names))
        self.assertTrue(all(n.startswith("kilohertz-pinball-1.2.3-windows-x64/") for n in names))
        self.assertIn("kilohertz-pinball-1.2.3-windows-x64/dof_bridge/KilohertzDofBridge.exe", names)

    def test_macos_layout(self):
        self.package(MAC)
        pkg = self.pkgdir(MAC)
        paths = set(f["path"] for f in self.manifest()["files"])
        exe = f"{APP}/Contents/MacOS/kilohertz-pinball"
        self.assertIn(exe, paths)
        for t in TABLES:
            self.assertIn(f"{APP}/Contents/MacOS/tables/{t}.pinbundle", paths)
        for d in ("LICENSE-MIT", "LICENSE-APACHE", "README.md"):
            self.assertIn(d, paths)
        self.assertFalse(any("dof_bridge" in p for p in paths))
        self.assertEqual(stat.S_IMODE((pkg / exe).stat().st_mode), 0o755)
        plist = plistlib.loads((pkg / APP / "Contents" / "Info.plist").read_bytes())
        self.assertEqual(plist["CFBundleExecutable"], "kilohertz-pinball")
        self.assertEqual(plist["CFBundleIdentifier"], "com.kilohertz.pinball")
        self.assertEqual(plist["CFBundleShortVersionString"], "1.2.3")
        self.assertEqual(plist["LSMinimumSystemVersion"], "12.0")
        archive = self.out / "kilohertz-pinball-1.2.3-macos-universal.zip"
        with zipfile.ZipFile(archive) as zf:
            zi = zf.getinfo(f"kilohertz-pinball-1.2.3-macos-universal/{exe}")
            self.assertEqual((zi.external_attr >> 16) & 0o777, 0o755)
            other = zf.getinfo("kilohertz-pinball-1.2.3-macos-universal/README.md")
            self.assertEqual((other.external_attr >> 16) & 0o777, 0o644)

    def test_no_dof_bridge_flag_outside_windows(self):
        for target in (LINUX, MAC):
            p = self.run_pkg("--target", target, "--out", self.out, "--exe", self.exe,
                             "--bundle-dir", self.bundles, "--dof-bridge", self.dof)
            self.assertEqual(p.returncode, 2, p.stderr)
            self.assertIn("--dof-bridge", p.stderr)
            self.assertFalse(self.out.exists())

    def test_version_defaults_to_cargo_toml(self):
        p = self.run_pkg("--target", LINUX, "--out", self.out, "--exe", self.exe, "--bundle-dir", self.bundles,
                         "--dry-run")
        self.assertEqual(p.returncode, 0, p.stderr)
        v = cargo_version()
        self.assertTrue((self.out / f"kilohertz-pinball-{v}-linux-x64").is_dir())
        self.assertEqual(self.manifest()["version"], v)

    def test_bad_version_is_usage_error(self):
        p = self.run_pkg("--target", LINUX, "--out", self.out, "--version", "../x", "--exe", self.exe,
                         "--bundle-dir", self.bundles)
        self.assertEqual(p.returncode, 2, p.stderr)

    def test_rerun_replaces_package_dir(self):
        self.package(LINUX)
        stray = self.pkgdir(LINUX) / "stray.txt"
        stray.write_text("old")
        self.package(LINUX)
        self.assertFalse(stray.exists())


class UsageTest(PackageCase):
    def test_bad_target(self):
        p = self.run_pkg("--target", "aarch64-apple-darwin", "--out", self.out)
        self.assertEqual(p.returncode, 2)
        for t in (WIN, LINUX, MAC):
            self.assertIn(t, p.stderr)

    def test_missing_out_or_target(self):
        self.assertEqual(self.run_pkg("--target", LINUX).returncode, 2)
        self.assertEqual(self.run_pkg("--out", self.out).returncode, 2)

    def test_help_documents_the_flags(self):
        p = self.run_pkg("--help")
        self.assertEqual(p.returncode, 0)
        for flag in ("--target", "--out", "--version", "--exe", "--bundle-dir", "--dof-bridge", "--dry-run", "--list"):
            self.assertIn(flag, p.stdout)

    def test_missing_exe(self):
        p = self.run_pkg("--target", LINUX, "--out", self.out, "--exe", self.tmp / "nope", "--bundle-dir", self.bundles)
        self.assertEqual(p.returncode, 1)
        self.assertIn(str(self.tmp / "nope"), p.stderr)

    def test_missing_bridge_in_prebuilt_dir(self):
        (self.dof / "KilohertzDofBridge.exe").unlink()
        p = self.run_pkg("--target", WIN, "--out", self.out, "--exe", self.exe, "--bundle-dir", self.bundles,
                         "--dof-bridge", self.dof)
        self.assertEqual(p.returncode, 1)
        self.assertIn("KilohertzDofBridge.exe", p.stderr)
        self.assertIn(str(self.dof), p.stderr)


class BundleTest(PackageCase):
    def bundle_names(self, target=LINUX, out=None):
        names = {}
        tables = self.pkgdir(target, out) / "tables"
        for t in TABLES:
            with zipfile.ZipFile(tables / f"{t}.pinbundle") as zf:
                names[t] = zf.namelist()
        return names

    def test_wasm_only_in_ported_tables(self):
        self.package(LINUX)
        names = self.bundle_names()
        self.assertIn("rules.lua", names["greybox"])
        self.assertNotIn("rules.wasm", names["greybox"])
        for t in PORTED:
            self.assertIn("rules.lua", names[t])
            self.assertIn("rules.wasm", names[t])
        for t in TABLES:
            self.assertIn("table.toml", names[t])
            self.assertIn("display/attract.toml", names[t])

    def test_greybox_wasm_is_left_out(self):
        shutil.rmtree(self.bundles)
        make_bundles(self.bundles, extra_wasm=("greybox",))
        p = self.package(LINUX)
        self.assertNotIn("rules.wasm", self.bundle_names()["greybox"])
        self.assertIn("rules.wasm", p.stderr)

    def test_ported_table_without_wasm_fails(self):
        for t in PORTED:
            with self.subTest(t=t):
                shutil.rmtree(self.bundles)
                make_bundles(self.bundles, skip_wasm=(t,))
                p = self.package(LINUX, expect=1)
                self.assertIn(str(self.bundles / t / "rules.wasm"), p.stderr)

    def test_required_files(self):
        for missing in ("table.toml", "rules.lua"):
            with self.subTest(missing=missing):
                shutil.rmtree(self.bundles)
                make_bundles(self.bundles)
                (self.bundles / "sky_deck" / missing).unlink()
                p = self.package(LINUX, expect=1)
                self.assertIn(str(self.bundles / "sky_deck" / missing), p.stderr)
        shutil.rmtree(self.bundles)
        make_bundles(self.bundles)
        shutil.rmtree(self.bundles / "feature_lab")
        p = self.package(LINUX, expect=1)
        self.assertIn(str(self.bundles / "feature_lab"), p.stderr)

    def test_symlink_in_bundle_fails(self):
        (self.bundles / "greybox" / "link.toml").symlink_to(self.bundles / "greybox" / "table.toml")
        p = self.package(LINUX, expect=1)
        self.assertIn("link.toml", p.stderr)

    def test_os_junk_is_skipped(self):
        (self.bundles / "greybox" / ".DS_Store").write_bytes(b"junk")
        self.package(LINUX)
        self.assertNotIn(".DS_Store", self.bundle_names()["greybox"])

    def test_bundles_are_byte_identical_across_runs(self):
        out2 = self.tmp / "out2"
        self.package(LINUX)
        self.package(LINUX, out=out2)
        for t in TABLES:
            a = (self.pkgdir(LINUX) / "tables" / f"{t}.pinbundle").read_bytes()
            b = (self.pkgdir(LINUX, out2) / "tables" / f"{t}.pinbundle").read_bytes()
            self.assertEqual(a, b, t)
        self.assertEqual((self.out / "PACKAGE_MANIFEST.json").read_bytes(),
                         (out2 / "PACKAGE_MANIFEST.json").read_bytes())
        name = "kilohertz-pinball-1.2.3-linux-x64"
        self.assertEqual((self.out / f"{name}.tar.gz").read_bytes(), (out2 / f"{name}.tar.gz").read_bytes())
        self.assertEqual((self.out / f"{name}.sha256").read_bytes(), (out2 / f"{name}.sha256").read_bytes())

    def test_release_zip_is_byte_identical_across_runs(self):
        out2 = self.tmp / "out2"
        self.package(WIN)
        self.package(WIN, out=out2)
        name = "kilohertz-pinball-1.2.3-windows-x64.zip"
        self.assertEqual((self.out / name).read_bytes(), (out2 / name).read_bytes())

    def test_bundle_zip_entries_are_plain_and_fixed(self):
        self.package(LINUX)
        for t in TABLES:
            path = self.pkgdir(LINUX) / "tables" / f"{t}.pinbundle"
            with zipfile.ZipFile(path) as zf:
                infos = zf.infolist()
            names = [zi.filename for zi in infos]
            self.assertEqual(names, sorted(names))
            for zi in infos:
                self.assertFalse(zi.filename.startswith("/"))
                self.assertNotIn("\\", zi.filename)
                self.assertNotIn("..", zi.filename.split("/"))
                self.assertFalse(zi.is_dir())
                self.assertFalse(re.match(r"^[A-Za-z]:", zi.filename))
                self.assertEqual(zi.date_time, (1980, 1, 1, 0, 0, 0))
                self.assertEqual(zi.external_attr >> 16, stat.S_IFREG | 0o644)
                self.assertFalse(stat.S_ISLNK(zi.external_attr >> 16))
                self.assertEqual(zi.extra, b"")
                self.assertEqual(zi.compress_type, zipfile.ZIP_DEFLATED)
                self.assertEqual(zi.flag_bits & 0x1, 0)
            raw = path.read_bytes()
            self.assert_no_extra_fields(raw)

    def assert_no_extra_fields(self, raw):
        """Central directory and local headers carry no extra fields."""
        pos = 0
        while True:
            pos = raw.find(b"PK\x01\x02", pos)
            if pos < 0:
                break
            name_len, extra_len, comment_len = struct.unpack("<HHH", raw[pos + 28:pos + 34])
            self.assertEqual(extra_len, 0)
            local = struct.unpack("<I", raw[pos + 42:pos + 46])[0]
            self.assertEqual(raw[local:local + 4], b"PK\x03\x04")
            self.assertEqual(struct.unpack("<H", raw[local + 28:local + 30])[0], 0)
            pos += 46 + name_len + extra_len + comment_len

    def test_bundles_pass_the_loader_name_rules(self):
        self.package(LINUX)
        for t in TABLES:
            path = self.pkgdir(LINUX) / "tables" / f"{t}.pinbundle"
            pkgmod.validate_bundle_zip(path)
            with zipfile.ZipFile(path) as zf:
                for n in zf.namelist():
                    self.assertEqual(pkgmod.normalize_entry_name(n), n)

    def test_highly_compressible_entry_is_stored(self):
        big = b"\0" * (3 << 20)
        (self.bundles / "greybox" / "silence.wav").write_bytes(big)
        self.package(LINUX)
        with zipfile.ZipFile(self.pkgdir(LINUX) / "tables" / "greybox.pinbundle") as zf:
            zi = zf.getinfo("silence.wav")
            self.assertEqual(zi.compress_type, zipfile.ZIP_STORED)
            self.assertEqual(zf.read("silence.wav"), big)
            self.assertEqual(zf.getinfo("rules.lua").compress_type, zipfile.ZIP_DEFLATED)


class LoaderRulesTest(unittest.TestCase):
    """The Python port of pin_assets::zip::normalize_path and the archive checks."""

    def test_normalize_rejects_like_pin_assets(self):
        for bad in ["a:", "C:/table.toml", "../table.toml", "/table.toml", "\\x", "a/../b", "x\0y", "./c:"]:
            with self.subTest(bad=bad):
                with self.assertRaises(ValueError):
                    pkgmod.normalize_entry_name(bad)

    def test_normalize_accepts(self):
        self.assertEqual(pkgmod.normalize_entry_name("a/./b//c"), "a/b/c")
        self.assertEqual(pkgmod.normalize_entry_name("a\\b"), "a/b")
        self.assertEqual(pkgmod.normalize_entry_name("display/x.toml"), "display/x.toml")

    def test_check_entry_name_requires_normal_form(self):
        pkgmod.check_entry_name("table.toml")
        for bad in ["a\\b", "a/./b", "dir/", ""]:
            with self.assertRaises(ValueError):
                pkgmod.check_entry_name(bad)

    def _zip(self, tmp, entries, **kw):
        path = Path(tmp) / "t.pinbundle"
        with zipfile.ZipFile(path, "w") as zf:
            for name, data, mode in entries:
                zi = zipfile.ZipInfo(name)
                zi.create_system = 3
                zi.external_attr = mode << 16
                zf.writestr(zi, data, compress_type=kw.get("method", zipfile.ZIP_DEFLATED))
        return path

    def test_validate_rejects_unsafe_archives(self):
        good = [("table.toml", b"x", stat.S_IFREG | 0o644), ("rules.lua", b"y", stat.S_IFREG | 0o644)]
        cases = {
            "traversal": good + [("../evil", b"z", stat.S_IFREG | 0o644)],
            "absolute": good + [("/evil", b"z", stat.S_IFREG | 0o644)],
            "symlink": good + [("link", b"table.toml", stat.S_IFLNK | 0o777)],
            "directory": good + [("dir/", b"", stat.S_IFDIR | 0o755)],
            "no rules.lua": good[:1],
            "ratio": good + [("zeros", b"\0" * (2 << 20), stat.S_IFREG | 0o644)],
        }
        with tempfile.TemporaryDirectory() as tmp:
            pkgmod.validate_bundle_zip(self._zip(tmp, good))
            for label, entries in cases.items():
                with self.subTest(label=label):
                    with self.assertRaises(pkgmod.PackagingError):
                        pkgmod.validate_bundle_zip(self._zip(tmp, entries))


class ManifestTest(PackageCase):
    def test_manifest_is_sorted_and_hash_correct(self):
        self.package(WIN)
        m = self.manifest()
        self.assertEqual(list(m), ["version", "target", "platform", "files"])
        self.assertEqual((m["version"], m["target"], m["platform"]), ("1.2.3", WIN, "windows-x64"))
        paths = [f["path"] for f in m["files"]]
        self.assertEqual(paths, sorted(paths))
        pkg = self.pkgdir(WIN)
        on_disk = sorted(p.relative_to(pkg).as_posix() for p in pkg.rglob("*") if p.is_file())
        self.assertEqual(paths, on_disk)
        for f in m["files"]:
            data = (pkg / f["path"]).read_bytes()
            self.assertEqual(f["size"], len(data))
            self.assertEqual(f["sha256"], hashlib.sha256(data).hexdigest())
            self.assertEqual(set(f), {"path", "size", "sha256"})
        text = (self.out / "PACKAGE_MANIFEST.json").read_text()
        self.assertNotIn(str(self.tmp), text)

    def test_sha256_file(self):
        self.package(LINUX)
        name = "kilohertz-pinball-1.2.3-linux-x64"
        archive = self.out / f"{name}.tar.gz"
        line = (self.out / f"{name}.sha256").read_text()
        self.assertEqual(line, f"{hashlib.sha256(archive.read_bytes()).hexdigest()}  {name}.tar.gz\n")

    def test_tarball_is_normalised(self):
        self.package(LINUX)
        name = "kilohertz-pinball-1.2.3-linux-x64"
        raw = (self.out / f"{name}.tar.gz").read_bytes()
        self.assertEqual(raw[4:8], b"\0\0\0\0", "gzip mtime must be 0")
        with tarfile.open(fileobj=io.BytesIO(raw), mode="r:gz") as tar:
            members = tar.getmembers()
        names = [m.name for m in members]
        self.assertEqual(names, sorted(names))
        for m in members:
            self.assertTrue(m.name == name or m.name.startswith(name + "/"))
            self.assertEqual((m.mtime, m.uid, m.gid, m.uname, m.gname), (0, 0, 0, "", ""))
            self.assertTrue(m.isfile() or m.isdir())
        exe = next(m for m in members if m.name == f"{name}/kilohertz-pinball")
        self.assertEqual(exe.mode, 0o755)
        readme = next(m for m in members if m.name == f"{name}/README.md")
        self.assertEqual(readme.mode, 0o644)


class DryRunAndListTest(PackageCase):
    def test_dry_run_keeps_dir_and_manifest_only(self):
        for target in (LINUX, WIN, MAC):
            with self.subTest(target=target):
                p = self.package(target, "--dry-run")
                self.assertIn("DRY RUN OK", p.stdout)
                base = self.pkgdir(target).name
                self.assertTrue(self.pkgdir(target).is_dir())
                self.assertTrue((self.out / "PACKAGE_MANIFEST.json").is_file())
                for ext in (".zip", ".tar.gz", ".sha256"):
                    self.assertFalse((self.out / f"{base}{ext}").exists(), ext)

    def test_dry_run_with_prebuilt_inputs_runs_no_tool(self):
        for target in (LINUX, WIN, MAC):
            self.package(target, "--dry-run")
        self.assertEqual(self.tools_called(), [])

    def test_windows_dry_run_has_dof_bridge_and_others_do_not(self):
        self.package(WIN, "--dry-run")
        self.assertTrue((self.pkgdir(WIN) / "dof_bridge" / "KilohertzDofBridge.exe").is_file())
        self.assertTrue((self.pkgdir(WIN) / "dof_bridge" / "UNVERIFIED.txt").is_file())
        for target in (LINUX, MAC):
            self.package(target, "--dry-run")
            self.assertFalse(any("dof_bridge" in str(p) for p in self.pkgdir(target).rglob("*")))

    def test_list_prints_paths_and_writes_nothing(self):
        for target in (LINUX, WIN, MAC):
            with self.subTest(target=target):
                p = self.package(target, "--list")
                self.assertFalse(self.out.exists())
                listed = p.stdout.splitlines()
                self.package(target, "--dry-run")
                paths = [f["path"] for f in self.manifest()["files"]]
                self.assertEqual(listed, paths)
                shutil.rmtree(self.out)
        self.assertEqual(self.tools_called(), [])

    def test_list_validates_given_inputs(self):
        (self.bundles / "sky_deck" / "rules.wasm").unlink()
        p = self.package(LINUX, "--list", expect=1)
        self.assertIn("rules.wasm", p.stderr)
        self.assertFalse(self.out.exists())


class BuildCommandTest(PackageCase):
    """Without prebuilt inputs the script runs cargo / build.sh / lipo / dotnet
    (here: fakes) and nothing else."""

    def log_lines(self):
        return self.log.read_text().splitlines()

    def test_linux_build_commands(self):
        p = self.package(LINUX, "--dry-run", prebuilt=False)
        self.assertIn("DRY RUN OK", p.stdout)
        lines = self.log_lines()
        self.assertIn(f"cargo build --release -p pin_app --target {LINUX}", lines)
        for t in TABLES:
            self.assertTrue(any(l.startswith(f"sh tables/{t}/build.sh ") for l in lines), lines)
        self.assertEqual(set(self.tools_called()), {"cargo", "sh"})
        self.assertEqual((self.pkgdir(LINUX) / "kilohertz-pinball").read_bytes(), f"exe {LINUX}".encode())

    def test_macos_builds_both_arches_and_lipo(self):
        self.package(MAC, "--dry-run", prebuilt=False)
        lines = self.log_lines()
        self.assertIn("cargo build --release -p pin_app --target aarch64-apple-darwin", lines)
        self.assertIn("cargo build --release -p pin_app --target x86_64-apple-darwin", lines)
        lipo = [l for l in lines if l.startswith("lipo ")]
        self.assertEqual(len(lipo), 1)
        self.assertIn("-create -output", lipo[0])
        self.assertNotIn("rustup", self.tools_called())
        self.assertNotIn("codesign", self.tools_called(), "dry-run must not sign")
        exe = self.pkgdir(MAC) / APP / "Contents" / "MacOS" / "kilohertz-pinball"
        self.assertEqual(exe.read_bytes(), b"universal")

    def test_windows_publishes_the_bridge(self):
        self.package(WIN, "--dry-run", prebuilt=False)
        lines = self.log_lines()
        pub = [l for l in lines if l.startswith("dotnet ")]
        self.assertEqual(len(pub), 1)
        self.assertIn("publish tools/dof_bridge/src/KilohertzDofBridge/KilohertzDofBridge.csproj -c Release "
                      "-r win-x64 --self-contained false -o ", pub[0])
        self.assertTrue((self.pkgdir(WIN) / "dof_bridge" / "KilohertzDofBridge.exe").is_file())

    def test_macos_adhoc_codesign_only(self):
        self.package(MAC)
        lines = [l for l in self.log_lines() if l.startswith("codesign ")]
        self.assertEqual(len(lines), 1)
        self.assertTrue(lines[0].startswith("codesign --force --deep -s - "), lines[0])
        self.assertTrue(lines[0].endswith(APP))

    def test_macos_codesign_failure_is_a_warning(self):
        p = self.run_pkg("--target", MAC, "--out", self.out, "--version", "1.2.3", "--exe", self.exe,
                         "--bundle-dir", self.bundles, env=self.env(CODESIGN_FAIL="1"))
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertIn("warning", p.stderr)
        self.assertTrue((self.out / "kilohertz-pinball-1.2.3-macos-universal.zip").is_file())

    def test_no_signing_or_network_tools_on_other_targets(self):
        self.package(LINUX)
        self.package(WIN)
        called = set(self.tools_called())
        for tool in ("codesign", "git", "gh", "rustup", "curl"):
            self.assertNotIn(tool, called)


class SourceTest(unittest.TestCase):
    def test_package_script_names_no_publishing_command(self):
        src = PKG.read_text(encoding="utf-8")
        code = src.split('"""', 2)[2]  # skip the module docstring
        for forbidden in ('"git"', '"gh"', "notarytool", "signtool", '"curl"', "urllib", "http.client",
                          "import socket", '"rustup"', '"scp"', '"rsync"'):
            self.assertNotIn(forbidden, code, forbidden)


def workflow_text():
    return WORKFLOW.read_text(encoding="utf-8")


class WorkflowTextTest(unittest.TestCase):
    """release.yml text checks (C1–C9); YAML cannot be parsed with the stdlib."""

    def test_forbidden_strings(self):
        text = workflow_text()
        for s in ("secrets.", "codesign", "notarytool", "signtool", "cargo publish", "docker push", "git push",
                  "git tag", "gh release edit", "--latest"):
            self.assertNotIn(s, text, s)

    def test_release_is_always_a_draft(self):
        lines = [l for l in workflow_text().splitlines() if "gh release create" in l]
        self.assertEqual(len(lines), 1)
        for flag in ("--draft", "--prerelease", "--verify-tag", '"$GITHUB_REF_NAME"'):
            self.assertIn(flag, lines[0])

    def test_one_contents_write(self):
        text = workflow_text()
        self.assertEqual(text.count("contents: write"), 1)
        self.assertRegex(text, r"(?m)^permissions:\n  contents: read$")

    def test_name_and_triggers(self):
        text = workflow_text()
        self.assertRegex(text, r"(?m)^name: release$")
        on = text.split("\non:\n", 1)[1].split("\nconcurrency:", 1)[0]
        self.assertRegex(on, r"(?m)^  pull_request:$")
        self.assertRegex(on, r"(?m)^  workflow_dispatch:$")
        self.assertRegex(on, r"(?m)^  push:\n    tags: \['v\*'\]$")
        self.assertNotIn("branches", on)
        for path in ("scripts/package.py", "scripts/human_acceptance.py", "scripts/tests/**", "tables/**",
                     "tools/dof_bridge/**", "crates/pin_app/**", ".github/workflows/release.yml",
                     "docs/HUMAN_ACCEPTANCE.md", "docs/queue/**", "docs/decisions/**", "docs/RUN_LOG.md"):
            self.assertIn(f'      - "{path}"', on)
        self.assertRegex(on, r"(?m)^      version:$")
        self.assertIn("group: ${{ github.workflow }}-${{ github.ref }}\n", text)
        self.assertIn("cancel-in-progress: ${{ github.event_name == 'pull_request' }}", text)

    def jobs(self):
        text = workflow_text().split("\njobs:\n", 1)[1]
        parts = re.split(r"(?m)^  ([a-z][a-z0-9-]*):\n", text)
        return dict(zip(parts[1::2], parts[2::2]))

    def test_jobs(self):
        jobs = self.jobs()
        self.assertEqual(set(jobs), {"dry-run", "verify", "version", "package", "draft-release"})
        self.assertIn("if: github.event_name == 'pull_request'", jobs["dry-run"])
        for j in ("verify", "version", "package"):
            self.assertIn("if: github.event_name != 'pull_request'", jobs[j], j)
        self.assertIn("python3 -m unittest discover -s scripts/tests -p '*_test.py'", jobs["dry-run"])
        self.assertIn("scripts/human_acceptance.py --check", jobs["dry-run"])
        self.assertIn("continue-on-error: true", jobs["dry-run"])
        self.assertIn("::warning", jobs["dry-run"])
        self.assertIn("--target x86_64-unknown-linux-gnu", jobs["dry-run"])
        self.assertIn("--dry-run", jobs["dry-run"])
        self.assertNotIn("upload-artifact", jobs["dry-run"])
        self.assertNotIn("windows-latest", jobs["dry-run"])
        v = jobs["verify"]
        for s in ("ubuntu-latest", "windows-latest", "macos-latest", "cargo build --release --workspace",
                  "cargo test --release --workspace", 'RUSTFLAGS: "-D warnings"', "SDL_VIDEO_DRIVER: dummy",
                  "x86_64-apple-darwin", "aarch64-apple-darwin"):
            self.assertIn(s, v)
        pk = jobs["package"]
        self.assertRegex(pk, r"needs: \[verify, version\]|needs:\n      - verify")
        for s in ("actions/setup-dotnet@v4", "8.0.x", "actions/upload-artifact@v4", "retention-days: 14",
                  "scripts/package.py --target", "PACKAGE_MANIFEST.json", "universal-apple-darwin"):
            self.assertIn(s, pk)
        self.assertNotIn("--dry-run", pk)
        dr = jobs["draft-release"]
        self.assertIn("github.ref_type == 'tag'", dr)
        self.assertIn("github.event_name == 'push'", dr)
        self.assertIn("contents: write", dr)
        self.assertIn("SHA256SUMS", dr)
        self.assertIn("GH_TOKEN: ${{ github.token }}", dr)
        self.assertIn("actions/download-artifact@v4", dr)
        self.assertIn("unsigned", dr)
        self.assertIn("unverified on hardware", dr)
        for name, body in jobs.items():
            if name != "draft-release":
                self.assertNotIn("contents: write", body)

    def test_action_refs_are_the_pinned_ones(self):
        allowed = {"actions/checkout@v5", "Swatinem/rust-cache@v2", "dtolnay/rust-toolchain@1.97.0",
                   "actions/setup-dotnet@v4", "actions/upload-artifact@v4", "actions/download-artifact@v4"}
        used = set(re.findall(r"uses: (\S+)", workflow_text()))
        self.assertTrue(used <= allowed, used - allowed)
        self.assertIn('RUST_VERSION: "1.97.0"', workflow_text())

    def test_no_tabs_and_final_newline(self):
        text = workflow_text()
        self.assertNotIn("\t", text)
        self.assertTrue(text.endswith("\n"))


if __name__ == "__main__":
    unittest.main()
