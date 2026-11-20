"""Bundle (.pinbundle) and manifest contract of scripts/package.py."""
import hashlib
import json
import stat
import unittest
import zipfile

from support import (LIN, MAC, PORTED, TABLES, TARGETS, WIN, PackageCase, make_bundle_dir)


class Bundles(PackageCase):
    def bundle(self, name, target=LIN, out=None):
        return self.pkgdir(target, out) / "tables" / (name + ".pinbundle")

    def test_bundle_is_zip_with_top_level_files_and_exact_contents(self):
        self.ok(LIN)
        for t in TABLES:
            with self.subTest(table=t):
                with zipfile.ZipFile(self.bundle(t)) as zf:
                    names = zf.namelist()
                    self.assertIn("table.toml", names)
                    self.assertIn("rules.lua", names)
                    self.assertEqual(zf.read("table.toml"), (self.bundles / t / "table.toml").read_bytes())
                    self.assertEqual(zf.read("rules.lua"), (self.bundles / t / "rules.lua").read_bytes())
                    if t in PORTED:
                        self.assertEqual(zf.read("rules.wasm"), (self.bundles / t / "rules.wasm").read_bytes())

    def test_rules_wasm_iff_source_had_it_and_greybox_has_none(self):
        self.ok(LIN)
        for t in PORTED:
            with zipfile.ZipFile(self.bundle(t)) as zf:
                self.assertIn("rules.wasm", zf.namelist(), t)
        with zipfile.ZipFile(self.bundle("greybox")) as zf:
            self.assertNotIn("rules.wasm", zf.namelist())

    def test_entries_sorted_forward_slash_safe_paths(self):
        self.ok(LIN)
        for t in TABLES:
            with zipfile.ZipFile(self.bundle(t)) as zf:
                names = zf.namelist()
                self.assertEqual(names, sorted(names), t)
                for n in names:
                    self.assertNotIn("\\", n)
                    self.assertFalse(n.startswith("/"), n)
                    self.assertNotIn("..", n.split("/"))
                    self.assertFalse(len(n) > 1 and n[1] == ":", n)

    def test_entry_timestamps_are_1980_and_no_symlinks(self):
        self.ok(LIN)
        for t in TABLES:
            with zipfile.ZipFile(self.bundle(t)) as zf:
                for info in zf.infolist():
                    self.assertEqual(info.date_time, (1980, 1, 1, 0, 0, 0), (t, info.filename))
                    self.assertFalse(stat.S_ISLNK(info.external_attr >> 16), (t, info.filename))

    def test_bundles_byte_identical_across_runs_in_different_out_dirs(self):
        a, b = self.tmp / "run_a", self.tmp / "run_b"
        self.ok(LIN, out=a)
        self.ok(LIN, out=b)
        for t in TABLES:
            self.assertEqual(self.bundle(t, out=a).read_bytes(), self.bundle(t, out=b).read_bytes(), t)

    def test_bundle_bytes_independent_of_source_mtime_and_dir_creation_order(self):
        import os
        other = self.tmp / "other_bundles"
        make_bundle_dir(other)
        for p in other.rglob("*"):
            os.utime(p, (1_000_000_000, 1_000_000_000))
        a, b = self.tmp / "ra", self.tmp / "rb"
        self.ok(LIN, out=a)
        self.ok(LIN, out=b, bundles=other)
        for t in TABLES:
            self.assertEqual(self.bundle(t, out=a).read_bytes(), self.bundle(t, out=b).read_bytes(), t)

    def test_ported_table_without_wasm_exit_1_names_path(self):
        for t in PORTED:
            with self.subTest(table=t):
                src = make_bundle_dir(self.tmp / ("nowasm_" + t), skip_files=["%s/rules.wasm" % t])
                r = self.package(LIN, bundles=src, out=self.tmp / ("o_" + t))
                self.assertEqual(r.returncode, 1, r.stderr)
                self.assertIn(t, r.stderr)
                self.assertIn("rules.wasm", r.stderr)

    def test_bundle_missing_table_toml_or_rules_lua_exit_1(self):
        for t in TABLES:
            for fn in ("table.toml", "rules.lua"):
                with self.subTest(table=t, missing=fn):
                    src = make_bundle_dir(self.tmp / ("miss_%s_%s" % (t, fn)),
                                          skip_files=["%s/%s" % (t, fn)])
                    r = self.package(LIN, bundles=src, out=self.tmp / ("o_%s_%s" % (t, fn)))
                    self.assertEqual(r.returncode, 1, r.stderr)
                    self.assertIn(t, r.stderr)
                    self.assertIn(fn, r.stderr)


class Manifest(PackageCase):
    def test_manifest_schema_and_sorted_files(self):
        for t, plat in TARGETS.items():
            with self.subTest(target=t):
                out = self.tmp / ("o_" + t)
                self.ok(t, out=out)
                m = self.manifest(out)
                self.assertEqual(sorted(m.keys()), ["files", "platform", "target", "version"])
                self.assertEqual(m["version"], self.VERSION)
                self.assertEqual(m["target"], t)
                self.assertEqual(m["platform"], plat)
                paths = [f["path"] for f in m["files"]]
                self.assertEqual(paths, sorted(paths))
                self.assertEqual(len(paths), len(set(paths)))
                for f in m["files"]:
                    self.assertEqual(sorted(f.keys()), ["path", "sha256", "size"])

    def test_manifest_entries_match_files_on_disk(self):
        for t in TARGETS:
            with self.subTest(target=t):
                out = self.tmp / ("o_" + t)
                self.ok(t, out=out)
                pkg = self.pkgdir(t, out)
                m = self.manifest(out)
                on_disk = sorted(p.relative_to(pkg).as_posix() for p in pkg.rglob("*") if p.is_file())
                self.assertEqual([f["path"] for f in m["files"]], on_disk)
                for f in m["files"]:
                    data = (pkg / f["path"]).read_bytes()
                    self.assertEqual(f["size"], len(data), f["path"])
                    self.assertEqual(f["sha256"], hashlib.sha256(data).hexdigest(), f["path"])

    def test_manifest_has_no_timestamps_or_absolute_paths(self):
        out = self.tmp / "o"
        self.ok(WIN, out=out)
        text = (out / "PACKAGE_MANIFEST.json").read_text("utf-8")
        self.assertNotIn(str(out), text)
        self.assertNotIn(str(self.tmp), text)
        self.assertNotRegex(text, r"\d{4}-\d{2}-\d{2}")
        for f in json.loads(text)["files"]:
            self.assertFalse(f["path"].startswith("/"))
            self.assertNotIn("\\", f["path"])

    def test_manifest_lists_platform_specific_files(self):
        out_w, out_m = self.tmp / "w", self.tmp / "m"
        self.ok(WIN, out=out_w)
        self.ok(MAC, out=out_m)
        wp = [f["path"] for f in self.manifest(out_w)["files"]]
        self.assertIn("dof_bridge/KilohertzDofBridge.exe", wp)
        self.assertIn("dof_bridge/UNVERIFIED.txt", wp)
        self.assertIn("kilohertz-pinball.exe", wp)
        mp = [f["path"] for f in self.manifest(out_m)["files"]]
        self.assertIn("Kilohertz Pinball.app/Contents/MacOS/kilohertz-pinball", mp)
        self.assertIn("Kilohertz Pinball.app/Contents/Info.plist", mp)
        self.assertIn("Kilohertz Pinball.app/Contents/MacOS/tables/greybox.pinbundle", mp)

    def test_manifest_byte_identical_across_runs(self):
        a, b = self.tmp / "a", self.tmp / "b"
        self.ok(WIN, out=a)
        self.ok(WIN, out=b)
        self.assertEqual((a / "PACKAGE_MANIFEST.json").read_bytes(),
                         (b / "PACKAGE_MANIFEST.json").read_bytes())


if __name__ == "__main__":
    unittest.main()
