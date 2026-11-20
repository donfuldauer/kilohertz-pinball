"""Seeded property-style test: random fixture content never crashes human_acceptance.py."""
import random
import shutil
import unittest
from pathlib import Path

from support import HUMAN_PY, TmpCase, run_script

PIECES = [
    "alpha", "beta", "D33", "Gate A1", "Gate A2", "Wave 4 gate", "blocked", "BLOCKED", "done",
    "- [ ] [x1] ", "- [x] [y2] ", "- [id3] ", "- ~~", "~~", "| a | blocked | b |", "|", "## D7 — topic ",
    "## D8 - nodash", "## ", " — ", "‮", "́", "\U0001F600", "日本語", "\t", "\x0b",
    " ", " ", "[", "]", "`", "\\", "%s", "{}", "<b>", "&amp;", "",
]
FILES = ["RUN_LOG.md", "HUMAN_QUEUE.md", "DECISIONS.md"]


def rand_line(rng):
    n = rng.choice([0, 1, 2, 4, 8, 20])
    line = "".join(rng.choice(PIECES) + rng.choice(["", " ", "x"]) for _ in range(n))
    if rng.random() < 0.05:
        line += "L" * rng.randint(2000, 6000)
    return line


def rand_text(rng):
    nl = rng.choice(["\n", "\r\n"])
    lines = [rand_line(rng) for _ in range(rng.randint(0, 25))]
    return nl.join(lines) + rng.choice(["", nl, nl + nl])


class Fuzz(TmpCase):
    def test_random_content_never_crashes_and_is_idempotent(self):
        rng = random.Random(7202)
        root = self.tmp / "fx"
        for i in range(200):
            if root.exists():
                shutil.rmtree(root)
            (root / "docs" / "queue").mkdir(parents=True)
            (root / "docs" / "decisions").mkdir(parents=True)
            for k in range(rng.randint(0, 3)):
                (root / "docs" / "queue" / ("q%d.md" % k)).write_bytes(rand_text(rng).encode("utf-8"))
            for k in range(rng.randint(0, 3)):
                (root / "docs" / "decisions" / ("d%d.md" % k)).write_bytes(rand_text(rng).encode("utf-8"))
            for name in FILES:
                if rng.random() < 0.7:
                    (root / "docs" / name).write_bytes(rand_text(rng).encode("utf-8"))
            out = root / "out.md"
            r = run_script(HUMAN_PY, ["--repo", root, "--out", out])
            self.assertEqual(r.returncode, 0, "iteration %d: %s" % (i, r.stderr[-500:]))
            first = out.read_bytes()
            r2 = run_script(HUMAN_PY, ["--repo", root, "--out", out, "--check"])
            self.assertEqual(r2.returncode, 0, "iteration %d not idempotent: %s" % (i, r2.stderr[-500:]))
            self.assertEqual(out.read_bytes(), first)
            text = first.decode("utf-8")
            self.assertTrue(text.startswith("# Human Acceptance"), i)
            self.assertTrue(text.endswith("\n") and not text.endswith("\n\n"), i)
            for h in ("## 1. Review the run", "## 6. Phase 8 checklist"):
                self.assertIn("\n" + h + "\n", text, i)


if __name__ == "__main__":
    unittest.main()
