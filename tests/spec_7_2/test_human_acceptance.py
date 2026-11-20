"""Black-box contract tests of scripts/human_acceptance.py using fixture repos."""
import re
import unittest
from pathlib import Path

from support import HUMAN_PY, TmpCase, run_script

HEADINGS = [
    "## 1. Review the run",
    "## 2. Blocked and open items",
    "## 3. Logged decisions",
    "## 4. D33 flags",
    "## 5. Gate reports",
    "## 6. Phase 8 checklist",
]
LONG_GATE = "Gate A1 passed long: " + " ".join("w%03d" % i for i in range(120))  # > 400 chars

RUN_LOG = "\n".join([
    "# Run log",
    "",
    "| Unit | Status | Notes |",
    "|------|--------|-------|",
    "| 1.1 | done | zzdone1 |",
    "| 2.1 | BLOCKED | zzblockedA |",
    "| 3.1 | done | zzdone3 |",
    "| 4.1 | blocked: waiting on person | zzblockedB |",
    "| 5.1 | done | zzdone5 |",
    "",
    LONG_GATE,
    "Some unrelated line zzunrelated",
    "Wave 3 gate green with notes zzwave3",
    "Gate A2 verdict ok zzgatea2",
    "Run note: D33 flag zzrunlogd33",
    "",
])

QUEUE_A = "\n".join([
    "# Human queue — 1.1",
    "",
    "- [ ] [Q-A1] open alpha text zzqa1",
    "- [x] [Q-A2] resolved alpha text zzqa2",
    "- [x] [Q-A3] resolved alpha three zzqa3",
    "",
])
QUEUE_B = "\n".join([
    "# Human queue — 2.2",
    "",
    "- [ ] [Q-B1] open beta text zzqb1",
    "- [ ] [Q-B2] open beta two with D33 zzqb2d33",
    "- [x] [Q-B3] resolved beta three zzqb3",
    "- [x] [Q-B4] resolved beta four zzqb4",
    "",
])
QUEUE_README = "- [ ] [Q-README] ignored readme open zzqreadme\n"
DEC_A = "\n".join([
    "## D101 — First topic zzt1",
    "Decision: x",
    "## D102 — Second topic zzt2",
    "## D103 — Third topic about D33 zzt3d33",
    "",
])
DEC_B = "\n".join([
    "## 7.2-x — Fourth topic zzt4",
    "Rationale: y",
    "## D201 — Fifth topic zzt5",
    "",
])
DEC_README = "## D999 — ignored topic zzdreadme\n"
DECISIONS_MD = "## D1 — Old plan thing zzt6\nbody\n"
HUMAN_QUEUE = "\n".join([
    "# Human queue",
    "",
    "- [HQ-1] old style open item text zzhq1",
    "- ~~[HQ-2] struck item text zzhq2~~",
    "- [HQ-3] another old open zzhq3 mentions D33",
    "",
])


def files(human_queue=True, run_log=True, crlf=False):
    f = [
        ("docs/queue/1.1-alpha.md", QUEUE_A),
        ("docs/queue/2.2-beta.md", QUEUE_B),
        ("docs/queue/README.md", QUEUE_README),
        ("docs/decisions/unit-a.md", DEC_A),
        ("docs/decisions/unit-b.md", DEC_B),
        ("docs/decisions/README.md", DEC_README),
        ("docs/DECISIONS.md", DECISIONS_MD),
    ]
    if run_log:
        f.append(("docs/RUN_LOG.md", RUN_LOG))
    if human_queue:
        f.append(("docs/HUMAN_QUEUE.md", HUMAN_QUEUE))
    if crlf:
        f = [(p, t.replace("\n", "\r\n")) for p, t in f]
    return f


def write_fixture(root, file_list):
    root = Path(root)
    (root / "docs" / "queue").mkdir(parents=True, exist_ok=True)
    (root / "docs" / "decisions").mkdir(parents=True, exist_ok=True)
    for rel, text in file_list:
        p = root / rel
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(text.encode("utf-8"))
    return root


def sections(text):
    """Map heading line -> section body text."""
    out, cur = {}, None
    for line in text.split("\n"):
        if line.startswith("## "):
            cur = line
            out[cur] = []
        elif cur is not None:
            out[cur].append(line)
    return {k: "\n".join(v) for k, v in out.items()}


class HumanCase(TmpCase):
    def render(self, root, extra=(), out=None):
        out = out or (Path(root) / "out.md")
        r = run_script(HUMAN_PY, ["--repo", root, "--out", out] + list(extra))
        self.assertEqual(r.returncode, 0, "stdout=%s\nstderr=%s" % (r.stdout, r.stderr))
        return Path(out).read_bytes().decode("utf-8")

    def fixture(self, name="fx", **kw):
        return write_fixture(self.tmp / name, files(**kw))


class Structure(HumanCase):
    def test_script_exists(self):
        self.assertTrue(HUMAN_PY.is_file(), "scripts/human_acceptance.py missing")

    def test_title_notice_and_exact_h2_headings_in_order(self):
        text = self.render(self.fixture())
        self.assertTrue(text.startswith("# Human Acceptance"))
        first_h2 = text.index("\n## ")
        head = text[:first_h2]
        self.assertIn("scripts/human_acceptance.py", head)
        self.assertRegex(head.lower(), r"must not be edited by hand|do not edit|not be edited by hand")
        self.assertIn("generated", head.lower())
        self.assertEqual([l for l in text.split("\n") if l.startswith("## ")], HEADINGS)

    def test_sections_present_with_missing_optional_inputs(self):
        cases = {
            "no_human_queue": dict(human_queue=False),
            "no_run_log": dict(run_log=False),
        }
        for name, kw in cases.items():
            with self.subTest(case=name):
                text = self.render(self.fixture(name, **kw))
                self.assertEqual([l for l in text.split("\n") if l.startswith("## ")], HEADINGS)
        empty = self.tmp / "empty"
        (empty / "docs" / "queue").mkdir(parents=True)
        text = self.render(empty)
        self.assertEqual([l for l in text.split("\n") if l.startswith("## ")], HEADINGS)
        bare = self.tmp / "bare"
        (bare / "docs").mkdir(parents=True)
        text = self.render(bare)
        self.assertEqual([l for l in text.split("\n") if l.startswith("## ")], HEADINGS)


class Section1Counts(HumanCase):
    def line_with(self, body, label_re, number):
        pat = re.compile(r"\b%d\b" % number)
        lab = re.compile(label_re, re.I)
        return any(lab.search(l) and pat.search(l) for l in body.split("\n"))

    def test_counts_match_fixture(self):
        # No HUMAN_QUEUE.md here: counts are about docs/queue, decisions and the run log.
        text = self.render(self.fixture(human_queue=False))
        body = sections(text)[HEADINGS[0]]
        self.assertTrue(self.line_with(body, r"open", 3), body)
        self.assertTrue(self.line_with(body, r"resolved", 4), body)
        self.assertTrue(self.line_with(body, r"decision", 6), body)
        self.assertTrue(self.line_with(body, r"run.?log|row", 5), body)


class Section2Blocked(HumanCase):
    def setUp(self):
        super().setUp()
        self.body = sections(self.render(self.fixture()))[HEADINGS[1]]

    def test_open_queue_lines_appear_with_id_text_and_file(self):
        for id_, tok, fname in (("Q-A1", "zzqa1", "1.1-alpha.md"), ("Q-B1", "zzqb1", "2.2-beta.md"),
                                ("Q-B2", "zzqb2d33", "2.2-beta.md")):
            lines = [l for l in self.body.split("\n") if id_ in l]
            self.assertTrue(lines, id_)
            self.assertTrue(any(tok in l and fname in l for l in lines), (id_, lines))
        self.assertIn("open alpha text", self.body)

    def test_resolved_and_readme_lines_do_not_appear(self):
        for tok in ("zzqa2", "zzqa3", "zzqb3", "zzqb4", "zzqreadme", "Q-README"):
            self.assertNotIn(tok, self.body)

    def test_human_queue_old_style_lines_appear_struck_do_not(self):
        for id_, tok in (("HQ-1", "zzhq1"), ("HQ-3", "zzhq3")):
            lines = [l for l in self.body.split("\n") if id_ in l]
            self.assertTrue(any(tok in l and "HUMAN_QUEUE" in l for l in lines), (id_, lines))
        self.assertNotIn("zzhq2", self.body)
        self.assertNotIn("HQ-2", self.body)

    def test_blocked_run_log_rows_appear_done_rows_do_not(self):
        self.assertIn("zzblockedA", self.body)
        self.assertIn("zzblockedB", self.body)
        for tok in ("zzdone1", "zzdone3", "zzdone5"):
            self.assertNotIn(tok, self.body)


class Section3Decisions(HumanCase):
    def test_every_decision_heading_listed_in_format(self):
        body = sections(self.render(self.fixture()))[HEADINGS[2]]
        expect = [("D101", "First topic zzt1", "unit-a.md"), ("D102", "Second topic zzt2", "unit-a.md"),
                  ("D103", "Third topic about D33 zzt3d33", "unit-a.md"),
                  ("7.2-x", "Fourth topic zzt4", "unit-b.md"), ("D201", "Fifth topic zzt5", "unit-b.md"),
                  ("D1", "Old plan thing zzt6", "DECISIONS.md")]
        for id_, topic, fname in expect:
            pat = r"^- %s: %s \([^)]*%s\)\s*$" % (re.escape(id_), re.escape(topic), re.escape(fname))
            self.assertRegex(body, re.compile(pat, re.M), (id_, body))
        self.assertNotIn("zzdreadme", body)

    def test_heading_without_em_dash_is_skipped_without_failing(self):
        root = self.fixture("odd")
        (root / "docs" / "decisions" / "unit-c.md").write_text(
            "## D5 - hyphen only zzhyphen\n## Overview zznodash\n## D6 — Good one zzgood6\n", encoding="utf-8")
        body = sections(self.render(root))[HEADINGS[2]]
        self.assertNotIn("zzhyphen", body)
        self.assertNotIn("zznodash", body)
        self.assertIn("D6: Good one zzgood6", body)


class Section4D33(HumanCase):
    def setUp(self):
        super().setUp()
        self.body = sections(self.render(self.fixture()))[HEADINGS[3]]

    def test_fixed_items(self):
        low = self.body.lower()
        self.assertRegex(low, r"ray.?quer[^\n]*(off|disabled)|(off|disabled)[^\n]*ray.?quer")
        self.assertRegex(low, r"dof[^\n]*unverified|unverified[^\n]*dof")
        self.assertRegex(low, r"unsigned")

    def test_every_d33_line_listed_with_file_name(self):
        for tok, fname in (("zzrunlogd33", "RUN_LOG.md"), ("zzqb2d33", "2.2-beta.md"),
                           ("zzhq3", "HUMAN_QUEUE.md"), ("zzt3d33", "unit-a.md")):
            lines = [l for l in self.body.split("\n") if tok in l]
            self.assertTrue(any(fname in l for l in lines), (tok, fname, lines))


class Section5Gates(HumanCase):
    def test_gate_lines_verbatim_and_in_file_order(self):
        body = sections(self.render(self.fixture()))[HEADINGS[4]]
        self.assertGreater(len(LONG_GATE), 400)
        wanted = [LONG_GATE, "Wave 3 gate green with notes zzwave3", "Gate A2 verdict ok zzgatea2"]
        pos = -1
        for w in wanted:
            i = body.find(w)
            self.assertGreater(i, -1, w[:60])
            self.assertGreater(i, pos, "out of file order: " + w[:40])
            pos = i
        self.assertNotIn("zzunrelated", body)

    def test_wave_n_gate_for_other_wave_numbers(self):
        root = self.fixture("waves")
        log = root / "docs" / "RUN_LOG.md"
        log.write_text(log.read_text() + "Wave 12 gate passed zzwave12\n", encoding="utf-8")
        body = sections(self.render(root))[HEADINGS[4]]
        self.assertIn("Wave 12 gate passed zzwave12", body)


class Section6Checklist(HumanCase):
    def test_checkboxes_cover_required_topics(self):
        body = sections(self.render(self.fixture()))[HEADINGS[5]]
        boxes = [l for l in body.split("\n") if l.lstrip().startswith("- [ ]")]
        self.assertTrue(boxes)
        topics = [r"review the run", r"greybox", r"apex_circuit", r"sky_deck", r"feature_lab",
                  r"x-arcade", r"plunger", r"nudge", r"rumble", r"ssf", r"multi-monitor", r"dof",
                  r"reflections", r"bloom", r"dmd", r"backglass", r"ray.?quer",
                  r"sounds", r"music", r"callouts", r"\bart\b"]
        for t in topics:
            self.assertTrue(any(re.search(t, l, re.I) for l in boxes), t)

    def test_checklist_present_when_inputs_missing(self):
        empty = self.tmp / "e"
        (empty / "docs").mkdir(parents=True)
        body = sections(self.render(empty))[HEADINGS[5]]
        self.assertIn("- [ ]", body)


class Determinism(HumanCase):
    def test_two_runs_identical_bytes(self):
        root = self.fixture()
        a = self.render(root, out=self.tmp / "a.md")
        b = self.render(root, out=self.tmp / "b.md")
        self.assertEqual(a, b)

    def test_single_final_newline_lf_only_and_no_date_or_commit(self):
        text = self.render(self.fixture())
        self.assertTrue(text.endswith("\n"))
        self.assertFalse(text.endswith("\n\n"))
        self.assertNotIn("\r", text)
        self.assertNotRegex(text, r"\d{4}-\d{2}-\d{2}")
        self.assertNotRegex(text, r"\b[0-9a-f]{40}\b")

    def test_crlf_input_gives_lf_output_equal_to_lf_input_output(self):
        lf = self.render(self.fixture("lf"))
        crlf = self.render(self.fixture("crlf", crlf=True))
        self.assertNotIn("\r", crlf)
        self.assertEqual(lf, crlf)

    def test_commit_appears_exactly_once_when_given(self):
        sha = "0123456789abcdef0123456789abcdef01234567"
        text = self.render(self.fixture(), extra=["--commit", sha])
        self.assertEqual(text.count(sha), 1)
        self.assertTrue(text.endswith("\n") and not text.endswith("\n\n"))

    def test_file_creation_order_does_not_change_output(self):
        fl = files()
        a = self.render(write_fixture(self.tmp / "fwd", fl))
        b = self.render(write_fixture(self.tmp / "rev", list(reversed(fl))))
        self.assertEqual(a, b)

    def test_output_independent_of_repo_location(self):
        a = self.render(self.fixture("loc_one"))
        b = self.render(self.fixture("deeper/loc_two"))
        self.assertEqual(a, b)


class CheckMode(HumanCase):
    def test_check_passes_when_up_to_date(self):
        root = self.fixture()
        self.render(root)
        r = run_script(HUMAN_PY, ["--repo", root, "--out", root / "out.md", "--check"])
        self.assertEqual(r.returncode, 0, r.stderr)

    def test_check_fails_when_differs_and_does_not_write(self):
        root = self.fixture()
        out = root / "out.md"
        self.render(root)
        out.write_bytes(out.read_bytes() + b"hand edit\n")
        before = out.read_bytes()
        r = run_script(HUMAN_PY, ["--repo", root, "--out", out, "--check"])
        self.assertEqual(r.returncode, 1)
        self.assertTrue(r.stderr.strip())
        self.assertEqual(out.read_bytes(), before)

    def test_check_fails_when_missing_and_does_not_create(self):
        root = self.fixture()
        out = root / "never.md"
        r = run_script(HUMAN_PY, ["--repo", root, "--out", out, "--check"])
        self.assertEqual(r.returncode, 1)
        self.assertTrue(r.stderr.strip())
        self.assertFalse(out.exists())

    def test_check_fails_after_fixture_change(self):
        root = self.fixture()
        self.render(root)
        (root / "docs" / "queue" / "9.9-new.md").write_text("- [ ] [N-1] new open item\n", encoding="utf-8")
        r = run_script(HUMAN_PY, ["--repo", root, "--out", root / "out.md", "--check"])
        self.assertEqual(r.returncode, 1)


if __name__ == "__main__":
    unittest.main()
