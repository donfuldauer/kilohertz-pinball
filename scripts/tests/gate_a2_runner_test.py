#!/usr/bin/env python3
"""Runner-level test: gate_a2.sh and recording support (CodeRabbit PR #61, r4625928023).

Run: ``python3 -I -B scripts/tests/gate_a2_runner_test.py``. No build and no
real simulation: the runner and verdict script are copied into a scratch
repo with a stub table build and stub ``cargo`` / ``pin_sim``. The stub
``pin_sim soak`` usage text mentions ``--record-games`` only when asked to.
"""

import json
import os
import shutil
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))


def write_exe(path, text):
    with open(path, "w", encoding="utf-8") as f:
        f.write(text)
    os.chmod(path, 0o755)


class RunnerRecordingTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        root = self.tmp.name
        self.repo = os.path.join(root, "repo")
        os.makedirs(os.path.join(self.repo, "scripts"))
        for n in ("gate_a2.sh", "gate_a2_verdict.py"):
            shutil.copy(os.path.join(REPO, "scripts", n), os.path.join(self.repo, "scripts", n))
        tdir = os.path.join(self.repo, "tables", "stub_table")
        os.makedirs(tdir)
        write_exe(os.path.join(tdir, "build.sh"), '#!/bin/sh\nmkdir -p "$1"\n')
        self.bin = os.path.join(root, "bin")
        os.makedirs(self.bin)
        self.pin_sim = os.path.join(self.bin, "pin_sim")
        msg = json.dumps({"reason": "compiler-artifact", "target": {"name": "pin_sim", "kind": ["bin"]},
                          "executable": self.pin_sim})
        write_exe(os.path.join(self.bin, "cargo"), "#!/bin/sh\necho '%s'\n" % msg)
        self.out = os.path.join(root, "out")

    def tearDown(self):
        self.tmp.cleanup()

    def stub_pin_sim(self, supports_recording):
        usage = "usage: pin_sim soak BUNDLE --games N" + (" [--record-games SPEC]" if supports_recording else "")
        write_exe(self.pin_sim,
                  '#!/bin/sh\nif [ "$#" -eq 1 ] && [ "$1" = soak ]; then echo "%s"; exit 2; fi\n'
                  'echo "$@" >>"%s/calls.txt"\nexit 1\n' % (usage, self.bin))

    def run_gate(self):
        env = {"PATH": self.bin + os.pathsep + "/usr/bin:/bin", "HOME": self.tmp.name,
               "DRY": "1", "READINESS": "0", "TABLES": "stub_table", "OUT": self.out,
               "GAMES": "4", "REPLAYS": "2", "PYTHONDONTWRITEBYTECODE": "1"}
        return subprocess.run(["bash", os.path.join(self.repo, "scripts", "gate_a2.sh")], env=env,
                              capture_output=True, text=True, timeout=120)

    def run_txt(self):
        with open(os.path.join(self.out, "gate_a2_run.txt"), encoding="utf-8") as f:
            return f.read()

    def calls(self):
        try:
            with open(os.path.join(self.bin, "calls.txt"), encoding="utf-8") as f:
                return f.read()
        except FileNotFoundError:
            return ""

    def a24c_rows(self):
        with open(os.path.join(self.out, "gate_a2_verdict.txt"), encoding="utf-8") as f:
            return [x for x in f.read().splitlines() if "A2.4c" in x]

    def test_unsupported_recording_creates_no_tsv_and_reports(self):
        self.stub_pin_sim(False)
        p = self.run_gate()
        self.assertIn("record_games=0", self.run_txt(), p.stdout + p.stderr)
        self.assertFalse(os.path.exists(os.path.join(self.out, "stub_table", "replay_games.tsv")), p.stdout)
        self.assertNotIn("--replay-game", self.calls())
        rows = self.a24c_rows()
        self.assertTrue(rows, p.stdout)
        self.assertTrue(all(r.startswith("REPORT") for r in rows), rows)

    def test_supported_recording_keeps_tsv_with_empty_fields(self):
        self.stub_pin_sim(True)
        p = self.run_gate()
        self.assertIn("record_games=1", self.run_txt(), p.stdout + p.stderr)
        tsv = os.path.join(self.out, "stub_table", "replay_games.tsv")
        self.assertTrue(os.path.exists(tsv), p.stdout)
        with open(tsv, encoding="utf-8") as f:
            self.assertEqual(len(f.read().splitlines()), 2)
        self.assertTrue(any(r.startswith("FAIL") for r in self.a24c_rows()), self.a24c_rows())


if __name__ == "__main__":
    unittest.main()
