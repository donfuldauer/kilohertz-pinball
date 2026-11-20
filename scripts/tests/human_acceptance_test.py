#!/usr/bin/env python3
"""Tests for scripts/human_acceptance.py (PLAN 7.2, B).

Run: ``python3 -I -m unittest discover -s scripts/tests -p '*_test.py'`` from
the repository root. Each case builds a fixture repository in a temp dir and
runs the script on it with ``--repo``.
"""

import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent
SCRIPT = REPO / "scripts" / "human_acceptance.py"

HEADINGS = [
    "## 1. Review the run",
    "## 2. Blocked and open items",
    "## 3. Logged decisions",
    "## 4. D33 flags",
    "## 5. Gate reports",
    "## 6. Phase 8 checklist",
]

RUN_LOG = """# Run Log

| Wave | Subphase | Role | Model | Attempt | Status | Gate results | Notes |
|---|---|---|---|---|---|---|---|
| 0 | 0.1 | Implementer | Opus | 1 | done | all pass | commit a |
| 3 | 2.1 | Orchestrator | — | — | **blocked** (D29) | perf missed | see queue |

**Wave 0 gate: green** (single subphase; CI green on all three OSes).
| 6 | Gate A1 | Verifier | Sonnet | 1 | **pass** | every row PASS | gate cleared |
| 7 | 7.1 | Implementer | Opus | 1 | done | soak ok | DOF bridge stays D33-unverified |
| 8 | Gate A2 | Verifier | Sonnet | 1 | blocked | stuck | needs a person |
Paragraph naming gate A1 in lower case.
A line about D330 and xD33 only.
"""

QUEUE_A = """# Queue: a

- [ ] [a-1] first open item
- [x] [a-2] resolved item
- [ ] [a-3] mentions D33 flags
not a queue line
- [ ] no id here
"""

QUEUE_B = """# Queue: b

- [ ] [b-1] second file open item
"""

FROZEN_QUEUE = """# Human Queue

- [0.1] old style open item
- ~~[0.2] struck through item~~ Resolved.
- [0.3] ~~struck text~~ resolved too
- **Also review:** continuation line, not an item
- [x] [0.4] checkbox resolved in frozen file
"""

DECISIONS_A = """# Decisions: a

## a — first topic

**Decision:** x

## not a decision heading
## a2 —missing space
## a — second topic, with D33 in it
"""

FROZEN_DECISIONS = """# Decisions

## Orchestrator — Branching
## 0.1 — Spike results
"""


def write(path, text, crlf=False):
    path.parent.mkdir(parents=True, exist_ok=True)
    data = text.replace("\n", "\r\n") if crlf else text
    path.write_bytes(data.encode("utf-8"))


def fixture(root, crlf=False, skip=()):
    files = {
        "docs/RUN_LOG.md": RUN_LOG,
        "docs/queue/a.md": QUEUE_A,
        "docs/queue/b.md": QUEUE_B,
        "docs/queue/README.md": "# Human queue\n\n- [ ] [readme] must be ignored\n",
        "docs/decisions/2026-01-01-a.md": DECISIONS_A,
        "docs/decisions/README.md": "## readme — ignored heading\n",
        "docs/HUMAN_QUEUE.md": FROZEN_QUEUE,
        "docs/DECISIONS.md": FROZEN_DECISIONS,
    }
    for rel, text in files.items():
        if not any(rel.startswith(s) for s in skip):
            write(root / rel, text, crlf)
    return root


class HumanAcceptanceTest(unittest.TestCase):
    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory(prefix="hatest-")
        self.root = Path(self._tmp.name) / "repo"
        self.root.mkdir()

    def tearDown(self):
        self._tmp.cleanup()

    def run_ha(self, *args):
        return subprocess.run([sys.executable, "-I", str(SCRIPT), "--repo", str(self.root), *map(str, args)],
                              capture_output=True, text=True, timeout=60)

    def render(self, **kw):
        fixture(self.root, **kw)
        p = self.run_ha()
        self.assertEqual(p.returncode, 0, p.stderr)
        return (self.root / "docs" / "HUMAN_ACCEPTANCE.md").read_bytes().decode("utf-8")

    def section(self, text, n):
        start = text.index(HEADINGS[n - 1])
        end = text.index(HEADINGS[n]) if n < len(HEADINGS) else len(text)
        return text[start:end]

    def test_header_and_section_order(self):
        text = self.render()
        lines = text.splitlines()
        self.assertTrue(text.startswith("# Human Acceptance\n"))
        self.assertEqual(lines[0], "# Human Acceptance")
        self.assertNotIn("<!--", text)
        self.assertIn("scripts/human_acceptance.py", lines[2])
        self.assertIn("not edit", lines[2].lower())
        self.assertTrue(lines[2].startswith("> "))
        self.assertEqual([l for l in lines if l.startswith("# ")], ["# Human Acceptance"])
        self.assertEqual([l for l in lines if l.startswith("## ")], HEADINGS)
        self.assertTrue(text.endswith("\n") and not text.endswith("\n\n"))
        self.assertNotIn("\r", text)
        self.assertNotIn("Generated from", text)

    def test_counts(self):
        s1 = self.section(self.render(), 1)
        # open: a-1, a-3, b-1, 0.1 ; resolved: a-2, 0.2, 0.3, 0.4
        self.assertIn("- Open queue lines: 4", s1)
        self.assertIn("- Resolved queue lines: 4", s1)
        self.assertIn("- Decision entries: 4", s1)
        self.assertIn("- Run-log rows: 5", s1)
        self.assertIn("docs/queue/", s1)
        self.assertIn("docs/decisions/", s1)

    def test_open_items_both_styles(self):
        s2 = self.section(self.render(), 2)
        self.assertIn("- [ ] [a-1] first open item (docs/queue/a.md)", s2)
        self.assertIn("- [ ] [a-3] mentions D33 flags (docs/queue/a.md)", s2)
        self.assertIn("- [ ] [b-1] second file open item (docs/queue/b.md)", s2)
        self.assertIn("- [ ] [0.1] old style open item (docs/HUMAN_QUEUE.md)", s2)
        for gone in ("a-2", "0.2", "0.3", "0.4", "readme", "no id here", "Also review"):
            self.assertNotIn(gone, s2)
        # Grouped by file, files in sorted order.
        self.assertLess(s2.index("#### docs/HUMAN_QUEUE.md"), s2.index("#### docs/queue/a.md"))
        self.assertLess(s2.index("#### docs/queue/a.md"), s2.index("#### docs/queue/b.md"))
        self.assertLess(s2.index("[a-1]"), s2.index("[a-3]"))

    def test_blocked_rows(self):
        s2 = self.section(self.render(), 2)
        self.assertIn("| 3 | 2.1 | Orchestrator | — | — | **blocked** (D29) | perf missed | see queue |", s2)
        self.assertIn("| 8 | Gate A2 | Verifier | Sonnet | 1 | blocked | stuck | needs a person |", s2)
        self.assertNotIn("| 0 | 0.1 |", s2)

    def test_decision_headings(self):
        s3 = self.section(self.render(), 3)
        items = [l for l in s3.splitlines() if l.startswith("- ")]
        self.assertEqual(items, [
            "- Orchestrator: Branching (docs/DECISIONS.md)",
            "- 0.1: Spike results (docs/DECISIONS.md)",
            "- a: first topic (docs/decisions/2026-01-01-a.md)",
            "- a: second topic, with D33 in it (docs/decisions/2026-01-01-a.md)",
        ])

    def test_d33_flags(self):
        s4 = self.section(self.render(), 4)
        self.assertIn("Ray-query reflections ship off by default", s4)
        self.assertIn("unverified on hardware", s4)
        self.assertIn("unsigned", s4)
        self.assertIn("- docs/queue/a.md: - [ ] [a-3] mentions D33 flags", s4)
        self.assertIn("- docs/decisions/2026-01-01-a.md: ## a — second topic, with D33 in it", s4)
        self.assertIn("- docs/RUN_LOG.md: | 7 | 7.1 |", s4)
        self.assertNotIn("D330", s4)

    def test_gate_rows_verbatim_in_order(self):
        s5 = self.section(self.render(), 5)
        want = [
            "**Wave 0 gate: green** (single subphase; CI green on all three OSes).",
            "| 6 | Gate A1 | Verifier | Sonnet | 1 | **pass** | every row PASS | gate cleared |",
            "| 8 | Gate A2 | Verifier | Sonnet | 1 | blocked | stuck | needs a person |",
            "Paragraph naming gate A1 in lower case.",
        ]
        got = [l for l in s5.splitlines()[1:] if l.strip()]
        self.assertEqual(got, want)
        self.assertNotIn("...", s5)
        self.assertNotIn("…", s5)

    def test_checklist(self):
        s6 = self.section(self.render(), 6)
        top = [l for l in s6.splitlines() if l.startswith("- [ ] ")]
        self.assertEqual(len(top), 5)
        for word in ("Review the run", "Play each table", "Check hardware", "Check visuals", "Replace placeholders"):
            self.assertTrue(any(word in l for l in top), word)
        for table in ("greybox", "apex_circuit", "sky_deck", "feature_lab"):
            self.assertIn(f"  - [ ] {table}: flipper feel, ball speed, catches, nudge", s6)
        for item in ("X-Arcade", "plunger", "Accelerometer nudge", "Rumble", "SSF", "Multi-monitor", "DOF devices",
                     "physics_defaults.toml", "Gates A1 and A2", "bloom", "DMD", "backglass", "Ray-query",
                     "sounds, music, callouts and art by asset ID"):
            self.assertIn(item, s6)

    def test_idempotent(self):
        first = self.render()
        p = self.run_ha()
        self.assertEqual(p.returncode, 0)
        self.assertEqual((self.root / "docs" / "HUMAN_ACCEPTANCE.md").read_bytes().decode("utf-8"), first)

    def test_crlf_input_renders_the_same(self):
        lf = self.render()
        other = Path(self._tmp.name) / "crlf"
        other.mkdir()
        fixture(other, crlf=True)
        out = Path(self._tmp.name) / "crlf.md"
        p = subprocess.run([sys.executable, "-I", str(SCRIPT), "--repo", str(other), "--out", str(out)],
                           capture_output=True, text=True)
        self.assertEqual(p.returncode, 0, p.stderr)
        self.assertEqual(out.read_bytes().decode("utf-8"), lf)

    def test_check_exit_codes(self):
        fixture(self.root)
        p = self.run_ha("--check")
        self.assertEqual(p.returncode, 1)
        self.assertIn("HUMAN_ACCEPTANCE.md", p.stderr)
        self.assertEqual(self.run_ha().returncode, 0)
        self.assertEqual(self.run_ha("--check").returncode, 0)
        write(self.root / "docs" / "queue" / "c.md", "- [ ] [c-1] new item\n")
        p = self.run_ha("--check")
        self.assertEqual(p.returncode, 1)
        self.assertIn("+- [ ] [c-1] new item (docs/queue/c.md)", p.stderr)
        self.assertFalse("c-1" in (self.root / "docs" / "HUMAN_ACCEPTANCE.md").read_text(), "--check must not write")

    def test_out_and_commit(self):
        fixture(self.root)
        out = Path(self._tmp.name) / "x" / "ha.md"
        p = self.run_ha("--out", out, "--commit", "abc1234")
        self.assertEqual(p.returncode, 0, p.stderr)
        text = out.read_text()
        self.assertEqual(text.count("Generated from abc1234"), 1)
        self.assertEqual(self.run_ha("--out", out, "--commit", "abc1234", "--check").returncode, 0)
        self.assertEqual(self.run_ha("--out", out, "--check").returncode, 1)
        self.assertFalse((self.root / "docs" / "HUMAN_ACCEPTANCE.md").exists())

    def test_missing_inputs_tolerated(self):
        p = self.run_ha("--out", Path(self._tmp.name) / "empty.md")
        self.assertEqual(p.returncode, 0, p.stderr)
        text = (Path(self._tmp.name) / "empty.md").read_text()
        self.assertEqual([l for l in text.splitlines() if l.startswith("## ")], HEADINGS)
        self.assertGreaterEqual(text.count("not present"), 5)
        self.assertIn("- Open queue lines: 0", text)

    def test_partial_inputs(self):
        text = self.render(skip=("docs/RUN_LOG.md", "docs/HUMAN_QUEUE.md", "docs/decisions/"))
        self.assertIn("`docs/RUN_LOG.md`: not present", text)
        self.assertIn("`docs/HUMAN_QUEUE.md` (frozen): not present", text)
        self.assertIn("`docs/decisions/`: not present", text)
        self.assertIn("- Decision entries: 2", text)
        self.assertIn("- [ ] [a-1] first open item (docs/queue/a.md)", text)

    def test_real_repo_renders(self):
        """The real repository renders without error; a stale committed file is
        reported as a warning, not a failure (B5)."""
        p = subprocess.run([sys.executable, "-I", str(SCRIPT), "--check"], capture_output=True, text=True)
        if p.returncode != 0:
            # Stale is a warning on PRs, not a failure (decision 2026-11-20-7.2); report it.
            print(f"\nwarning: docs/HUMAN_ACCEPTANCE.md is stale:\n{p.stderr}", file=sys.stderr)
        self.assertIn(p.returncode, (0, 1))
        self.assertNotIn("Traceback", p.stderr)


if __name__ == "__main__":
    unittest.main()
