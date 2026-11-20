"""CLI contract of scripts/package.py: usage errors, version, layouts, dry-run, list, archives."""
import plistlib
import re
import stat
import sys
import tarfile
import unittest
import zipfile

from support import (ARCHIVE_EXT, BRIDGE_BYTES, EXE_BYTES, LIN, MAC, PACKAGE_PY, REPO, TARGETS,
                     WIN, PackageCase, cargo_version, run_script, sha256_file)

posix = unittest.skipUnless(sys.platform != "win32", "Unix file modes")


class UsageErrors(PackageCase):
    def test_script_exists(self):
        self.assertTrue(PACKAGE_PY.is_file(), "scripts/package.py missing")

    def test_unknown_target_exit_2_names_allowed_values(self):
        r = self.package("aarch64-unknown-linux-gnu", with_bridge=False)
        self.assertEqual(r.returncode, 2, r.stderr)
        for t in TARGETS:
            self.assertIn(t, r.stderr)

    def test_missing_target_exit_2(self):
        r = run_script(PACKAGE_PY, ["--out", self.tmp / "out", "--exe", self.exe,
                                    "--bundle-dir", self.bundles])
        self.assertEqual(r.returncode, 2)
        self.assertTrue(r.stderr.strip())

    def test_missing_out_exit_2(self):
        r = run_script(PACKAGE_PY, ["--target", LIN, "--exe", self.exe,
                                    "--bundle-dir", self.bundles])
        self.assertEqual(r.returncode, 2)
        self.assertTrue(r.stderr.strip())

    def test_dof_bridge_on_non_windows_targets_exit_2(self):
        for t in (LIN, MAC):
            with self.subTest(target=t):
                r = self.package(t, with_bridge=True, out=self.tmp / ("o_" + t))
                self.assertEqual(r.returncode, 2, r.stderr)

    def test_windows_bridge_dir_without_exe_exit_1(self):
        empty = self.tmp / "emptybridge"
        empty.mkdir()
        r = run_script(PACKAGE_PY, ["--target", WIN, "--out", self.tmp / "out", "--exe", self.exe,
                                    "--bundle-dir", self.bundles, "--dof-bridge", empty,
                                    "--version", self.VERSION])
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertTrue(r.stderr.strip())

    def test_exit_1_names_missing_exe(self):
        missing = self.tmp / "no-such-exe"
        r = run_script(PACKAGE_PY, ["--target", LIN, "--out", self.tmp / "out", "--exe", missing,
                                    "--bundle-dir", self.bundles, "--version", self.VERSION])
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertIn("no-such-exe", r.stderr)


class Version(PackageCase):
    def test_default_version_is_workspace_package_version(self):
        v = cargo_version()
        self.ok(LIN, version=None)
        self.assertTrue((self.tmp / "out" / ("kilohertz-pinball-%s-linux-x64" % v)).is_dir())
        self.assertEqual(self.manifest()["version"], v)

    def test_explicit_version_overrides(self):
        self.ok(LIN)
        self.assertTrue(self.pkgdir(LIN).is_dir())
        self.assertEqual(self.manifest()["version"], self.VERSION)


class Layouts(PackageCase):
    def test_windows_layout(self):
        self.ok(WIN)
        p = self.pkgdir(WIN)
        self.assertEqual((p / "kilohertz-pinball.exe").read_bytes(), EXE_BYTES)
        for t in ("greybox", "apex_circuit", "sky_deck", "feature_lab"):
            self.assertTrue((p / "tables" / (t + ".pinbundle")).is_file(), t)
        for f in ("LICENSE-MIT", "LICENSE-APACHE", "README.md"):
            self.assertTrue((p / f).is_file(), f)
        self.assertEqual((p / "dof_bridge" / "KilohertzDofBridge.exe").read_bytes(), BRIDGE_BYTES)
        self.assertIn("unverified", (p / "dof_bridge" / "UNVERIFIED.txt").read_text("utf-8").lower())
        self.assertEqual(len(list((p / "tables").iterdir())), 4)

    def test_linux_layout_has_no_dof_bridge(self):
        self.ok(LIN)
        p = self.pkgdir(LIN)
        self.assertEqual((p / "kilohertz-pinball").read_bytes(), EXE_BYTES)
        for t in ("greybox", "apex_circuit", "sky_deck", "feature_lab"):
            self.assertTrue((p / "tables" / (t + ".pinbundle")).is_file(), t)
        for f in ("LICENSE-MIT", "LICENSE-APACHE", "README.md"):
            self.assertTrue((p / f).is_file(), f)
        self.assertFalse([x for x in p.rglob("*") if "dof_bridge" in x.as_posix()])
        self.assertNotIn("dof_bridge", (self.tmp / "out" / "PACKAGE_MANIFEST.json").read_text("utf-8"))

    def test_macos_app_layout_and_plist(self):
        self.ok(MAC)
        p = self.pkgdir(MAC)
        app = p / "Kilohertz Pinball.app" / "Contents"
        self.assertEqual((app / "MacOS" / "kilohertz-pinball").read_bytes(), EXE_BYTES)
        for t in ("greybox", "apex_circuit", "sky_deck", "feature_lab"):
            self.assertTrue((app / "MacOS" / "tables" / (t + ".pinbundle")).is_file(), t)
        with open(app / "Info.plist", "rb") as fh:
            plist = plistlib.load(fh)
        self.assertEqual(plist["CFBundleExecutable"], "kilohertz-pinball")
        self.assertEqual(plist["CFBundleIdentifier"], "com.kilohertz.pinball")
        self.assertEqual(plist["CFBundleShortVersionString"], self.VERSION)
        self.assertEqual(str(plist["LSMinimumSystemVersion"]), "12.0")
        for f in ("LICENSE-MIT", "LICENSE-APACHE", "README.md"):
            self.assertTrue((p / f).is_file(), f)
        self.assertFalse([x for x in p.rglob("*") if "dof_bridge" in x.as_posix()])

    @posix
    def test_executable_mode_0755(self):
        for t, rel in ((WIN, "kilohertz-pinball.exe"), (LIN, "kilohertz-pinball"),
                       (MAC, "Kilohertz Pinball.app/Contents/MacOS/kilohertz-pinball")):
            with self.subTest(target=t):
                out = self.tmp / ("out_" + t)
                self.ok(t, out=out)
                mode = stat.S_IMODE((out / self.stem(t) / rel).stat().st_mode)
                self.assertEqual(mode, 0o755)

    @posix
    def test_executable_mode_0755_even_if_source_is_0644(self):
        self.exe.chmod(0o644)
        self.ok(LIN)
        mode = stat.S_IMODE((self.pkgdir(LIN) / "kilohertz-pinball").stat().st_mode)
        self.assertEqual(mode, 0o755)

    def test_repo_has_files_the_package_copies(self):
        for f in ("LICENSE-MIT", "LICENSE-APACHE", "README.md"):
            self.assertTrue((REPO / f).is_file())


class DryRunAndList(PackageCase):
    def test_dry_run_writes_dir_and_manifest_but_no_archive(self):
        for t in TARGETS:
            with self.subTest(target=t):
                out = self.tmp / ("o_" + t)
                r = self.ok(t, out=out, extra=["--dry-run"])
                self.assertIn("DRY RUN OK", r.stdout)
                self.assertTrue((out / self.stem(t)).is_dir())
                self.assertTrue((out / "PACKAGE_MANIFEST.json").is_file())
                names = [p.name for p in out.iterdir()]
                self.assertFalse([n for n in names if n.endswith((".zip", ".tar.gz", ".sha256"))], names)

    def test_list_prints_manifest_paths_and_writes_nothing(self):
        for t in TARGETS:
            with self.subTest(target=t):
                full = self.tmp / ("full_" + t)
                self.ok(t, out=full)
                expected = [f["path"] for f in self.manifest(full)["files"]]
                out = self.tmp / ("list_" + t)
                r = self.ok(t, out=out, extra=["--list"])
                lines = [ln for ln in r.stdout.splitlines() if ln.strip()]
                self.assertEqual(lines, expected)
                self.assertTrue(not out.exists() or not any(out.iterdir()))


class Archives(PackageCase):
    def test_archive_names_per_platform(self):
        for t, ext in ARCHIVE_EXT.items():
            with self.subTest(target=t):
                out = self.tmp / ("o_" + t)
                self.ok(t, out=out)
                self.assertTrue((out / (self.stem(t) + ext)).is_file())
                other = ".tar.gz" if ext == ".zip" else ".zip"
                self.assertFalse((out / (self.stem(t) + other)).exists())

    def test_sha256_file_matches_archive(self):
        for t, ext in ARCHIVE_EXT.items():
            with self.subTest(target=t):
                out = self.tmp / ("o_" + t)
                self.ok(t, out=out)
                name = self.stem(t) + ext
                text = (out / (self.stem(t) + ".sha256")).read_text("utf-8")
                self.assertRegex(text, r"^[0-9a-f]{64}  %s\n?$" % re.escape(name))
                self.assertEqual(text.split()[0], sha256_file(out / name))

    def test_archive_contents_per_platform(self):
        out = self.tmp / "o_lin"
        self.ok(LIN, out=out)
        with tarfile.open(out / (self.stem(LIN) + ".tar.gz")) as tf:
            names = tf.getnames()
        self.assertTrue([n for n in names if n.endswith("kilohertz-pinball")])
        self.assertEqual(len([n for n in names if n.endswith(".pinbundle")]), 4)
        self.assertFalse([n for n in names if "dof_bridge" in n])
        out = self.tmp / "o_win"
        self.ok(WIN, out=out)
        with zipfile.ZipFile(out / (self.stem(WIN) + ".zip")) as zf:
            names = zf.namelist()
        self.assertTrue([n for n in names if n.endswith("dof_bridge/KilohertzDofBridge.exe")])
        self.assertTrue([n for n in names if n.endswith("dof_bridge/UNVERIFIED.txt")])
        out = self.tmp / "o_mac"
        self.ok(MAC, out=out)
        with zipfile.ZipFile(out / (self.stem(MAC) + ".zip")) as zf:
            names = zf.namelist()
        self.assertTrue([n for n in names if n.endswith("Kilohertz Pinball.app/Contents/Info.plist")])
        self.assertFalse([n for n in names if "dof_bridge" in n])

    def test_archives_and_checksums_byte_identical_across_runs(self):
        for t, ext in ARCHIVE_EXT.items():
            with self.subTest(target=t):
                a, b = self.tmp / ("a_" + t), self.tmp / ("b_" + t)
                self.ok(t, out=a)
                self.ok(t, out=b)
                for name in (self.stem(t) + ext, self.stem(t) + ".sha256", "PACKAGE_MANIFEST.json"):
                    self.assertEqual((a / name).read_bytes(), (b / name).read_bytes(), name)


if __name__ == "__main__":
    unittest.main()
