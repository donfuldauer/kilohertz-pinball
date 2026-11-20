"""package.py must never run git, gh, curl, signing, notarization, cargo, dotnet or network tools."""
import os
import shlex
import sys
import unittest

from support import LIN, MAC, TARGETS, WIN, PackageCase

STUBS = ("git", "gh", "curl", "codesign", "notarytool", "signtool", "cargo", "dotnet")


@unittest.skipIf(sys.platform == "win32", "shell stubs need a POSIX shell")
class NoExternalTools(PackageCase):
    def make_env(self):
        stubdir = self.tmp / "stubs"
        stubdir.mkdir()
        self.log = self.tmp / "calls.log"
        self.log.write_text("")
        for name in STUBS:
            p = stubdir / name
            code = 0 if name == "codesign" else 1
            p.write_text("#!/bin/sh\necho \"%s $*\" >> %s\nexit %d\n" % (name, shlex.quote(str(self.log)), code))
            p.chmod(0o755)
        # PATH contains only the stubs (sh is invoked by absolute path in the shebang).
        return {"PATH": str(stubdir), "HOME": str(self.tmp)}

    def calls(self):
        return [ln for ln in self.log.read_text().splitlines() if ln.strip()]

    def test_no_stub_called_on_any_target(self):
        env = self.make_env()
        for t in TARGETS:
            with self.subTest(target=t):
                self.ok(t, out=self.tmp / ("o_" + t), env=env)
        calls = self.calls()
        for c in calls:
            # codesign is the only tool allowed, ad hoc only (-s -), and macOS only
            self.assertTrue(c.startswith("codesign "), "forbidden tool called: " + c)
            self.assertIn("-s -", c)
        if calls:
            # codesign may only have been used for the macOS package, never for Windows/Linux
            self.assertEqual(len(calls), len([c for c in calls if c.startswith("codesign ")]))

    def test_no_stub_called_for_windows_and_linux_even_codesign(self):
        env = self.make_env()
        for t in (WIN, LIN):
            with self.subTest(target=t):
                self.ok(t, out=self.tmp / ("o_" + t), env=env)
                self.assertEqual(self.calls(), [])

    def test_dry_run_and_list_call_nothing(self):
        env = self.make_env()
        for t in TARGETS:
            for flag in ("--dry-run", "--list"):
                with self.subTest(target=t, flag=flag):
                    self.ok(t, out=self.tmp / ("o_%s%s" % (t, flag)), env=env, extra=[flag])
        self.assertEqual(self.calls(), [])

    def test_failure_paths_also_call_nothing(self):
        env = self.make_env()
        r = self.package(LIN, out=self.tmp / "o", env=env, version="1.2.3",
                         bundles=self.tmp / "no-such-bundles")
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertEqual(self.calls(), [])


if __name__ == "__main__":
    unittest.main()
