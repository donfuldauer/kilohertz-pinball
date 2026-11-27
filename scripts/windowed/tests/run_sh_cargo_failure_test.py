"""run.sh keeps its failure-summary fallback through the Cargo invocation of
the windowed test target (gate review round 2 of #163, finding 4697795305):
a Cargo failure leaves a `verdict: FAIL (windowed setup: ...)` summary and
Cargo's exit status, and a summary the driver wrote is never overwritten.
Standard library only; fake tools on PATH, a temp target dir and out dir."""

import os
import pathlib
import shutil
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
RUN_SH = ROOT / "scripts" / "windowed" / "run.sh"

FAKE_CARGO = """#!/bin/sh
case "$1" in
  test)
    case "$FAKE_CASE" in
      1) exit 101 ;;
      2) printf 'verdict: PASS\\n' >"$KP_WINDOWED_OUT/summary.txt"; exit 1 ;;
      3) printf 'verdict: PASS\\n' >"$KP_WINDOWED_OUT/summary.txt"; exit 0 ;;
    esac ;;
esac
exit 0
"""


class RunShCargoFailureTest(unittest.TestCase):
    def run_case(self, case):
        with tempfile.TemporaryDirectory() as tmp:
            tmp = pathlib.Path(tmp)
            bindir = tmp / "bin"
            bindir.mkdir()
            for tool in ["sh", "dirname", "cat", "rm", "mkdir", "printf", "env", "cp"]:
                found = shutil.which(tool, path="/usr/bin:/bin")
                self.assertIsNotNone(found, tool)
                (bindir / tool).symlink_to(found)
            for tool in ["Xvfb", "xdotool", "xwd", "convert"]:
                (bindir / tool).write_text("#!/bin/sh\nexit 0\n")
                (bindir / tool).chmod(0o755)
            (bindir / "cargo").write_text(FAKE_CARGO)
            (bindir / "cargo").chmod(0o755)
            out = tmp / "out"
            env = {
                "PATH": str(bindir),
                "CARGO_TARGET_DIR": str(tmp / "t"),
                "FAKE_CASE": str(case),
            }
            r = subprocess.run(
                ["sh", str(RUN_SH), str(out)],
                env=env,
                capture_output=True,
                text=True,
                timeout=60,
            )
            summary = out / "summary.txt"
            text = summary.read_text() if summary.exists() else None
            return r, text

    def test_cargo_test_failure_without_a_driver_summary_publishes_a_failure(self):
        r, text = self.run_case(1)
        self.assertEqual(r.returncode, 101, r.stderr)
        self.assertIsNotNone(text)
        self.assertTrue(text.startswith("verdict: FAIL (windowed setup:"), text)
        self.assertIn("cargo test", text)

    def test_driver_summary_is_kept_and_cargo_status_preserved_on_failure(self):
        r, text = self.run_case(2)
        self.assertEqual(r.returncode, 1, r.stderr)
        self.assertEqual(text, "verdict: PASS\n")

    def test_passing_run_is_unchanged(self):
        r, text = self.run_case(3)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertEqual(text, "verdict: PASS\n")


if __name__ == "__main__":
    unittest.main()
