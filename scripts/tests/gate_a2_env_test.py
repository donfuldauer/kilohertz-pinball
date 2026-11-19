#!/usr/bin/env python3
"""T3: gate_a2.sh reads its numeric env vars as decimal (CodeRabbit finding 3).

Run: ``python3 -I scripts/tests/gate_a2_env_test.py`` (no build, no
simulation: ``cargo`` on PATH is a stub that fails, and TABLES names a table
directory that does not exist, so the runner prints its header, records the
run parameters and stops at the verdict).
"""

import os
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
GATE = os.path.join(REPO, "scripts", "gate_a2.sh")
NO_TABLE = "no_such_table_gate_a2_env_test"


class EnvTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        bindir = os.path.join(self.tmp.name, "bin")
        os.makedirs(bindir)
        cargo = os.path.join(bindir, "cargo")
        with open(cargo, "w", encoding="utf-8") as f:
            f.write("#!/bin/sh\necho 'cargo stub: no build in gate_a2_env_test' >&2\nexit 1\n")
        os.chmod(cargo, 0o755)
        self.path = bindir + os.pathsep + os.environ.get("PATH", "/usr/bin:/bin")
        self.out = os.path.join(self.tmp.name, "out")

    def tearDown(self):
        self.tmp.cleanup()

    def run_gate(self, **env):
        e = {"PATH": self.path, "HOME": os.environ.get("HOME", self.tmp.name),
             "DRY": "1", "READINESS": "0", "TABLES": NO_TABLE, "OUT": self.out}
        e.update(env)
        return subprocess.run(["bash", GATE], env=e, capture_output=True, text=True, timeout=120)

    def header(self, p):
        lines = [x for x in p.stdout.splitlines() if x.startswith("== Gate A2:")]
        self.assertEqual(len(lines), 1, p.stdout + p.stderr)
        return lines[0]

    def run_txt(self):
        with open(os.path.join(self.out, "gate_a2_run.txt"), encoding="utf-8") as f:
            return dict(x.split("=", 1) for x in f.read().splitlines() if "=" in x)

    def test_seed_08_is_8(self):
        p = self.run_gate(SEED="08")
        self.assertNotIn("value too great for base", p.stderr)
        self.assertIn(" SEED=8 ", self.header(p))
        self.assertEqual(self.run_txt()["seed"], "8")
        # The runner reached the verdict (the missing table FAILs A2.0).
        self.assertTrue(p.stdout.rstrip("\n").split("\n")[-1].startswith("GATE A2 (DRY RUN): "), p.stdout)

    def test_seed_010_is_10(self):
        p = self.run_gate(SEED="010")
        self.assertIn(" SEED=10 ", self.header(p))
        self.assertEqual(self.run_txt()["seed"], "10")

    def test_games_08_is_8(self):
        p = self.run_gate(GAMES="08", REPLAYS="03", THREADS="01", PATTERN_TICKS="0090")
        h = self.header(p)
        self.assertIn(" GAMES=8 REPLAYS=3 ", h)
        self.assertIn(" THREADS=1 ", h)
        run = self.run_txt()
        self.assertEqual((run["games"], run["replays"], run["threads"], run["pattern_ticks"]),
                         ("8", "3", "1", "90"))

    def test_19_digits_rejected(self):
        for var in ("SEED", "GAMES", "PATTERN_TICKS"):
            p = self.run_gate(**{var: "1" + "0" * 18})
            self.assertEqual(p.returncode, 2, (var, p.stdout, p.stderr))
            self.assertIn("more than 18 digits", p.stderr)
            self.assertFalse(os.path.exists(os.path.join(self.out, "gate_a2_run.txt")))

    def test_18_digits_accepted(self):
        p = self.run_gate(SEED="9" * 18)
        self.assertIn(" SEED=%s " % ("9" * 18), self.header(p))

    def test_replays_above_games_rejected(self):
        p = self.run_gate(GAMES="05", REPLAYS="6")
        self.assertEqual(p.returncode, 2)
        self.assertIn("REPLAYS (6) cannot exceed GAMES (5)", p.stderr)


if __name__ == "__main__":
    unittest.main(verbosity=2)
