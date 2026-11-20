#!/usr/bin/env python3
"""scripts/req_coverage.py: inline-test extraction and `--plan-file --milestone` validation
(CodeRabbit PR #142 round 1).

Run: ``python3 -I -m unittest discover -s scripts/tests -p '*_test.py'``. No network, no gh.
"""

import contextlib
import importlib.util
import io
import os
import pathlib
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(os.path.dirname(HERE), "req_coverage.py")
_spec = importlib.util.spec_from_file_location("req_coverage", SCRIPT)
rc = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(rc)


class CfgTestTextTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.src = pathlib.Path(self.tmp.name) / "src"
        self.src.mkdir()

    def ids(self, text):
        p = self.src / "lib.rs"
        p.write_text(text, encoding="utf-8")
        return rc.ids_in_test_code(p)

    def test_text_after_closing_brace_is_production(self):
        text = '#[cfg(test)] mod tests { /* REQ-AAA-001 */ } const NOTE: &str = "REQ-AAA-002";\n'
        self.assertEqual(self.ids(text), {"REQ-AAA-001"})

    def test_multiline_module_counts_inside_ids(self):
        text = ("fn prod() {}\n#[cfg(test)]\nmod tests {\n    // REQ-AAA-001\n"
                "    fn t() {} // REQ-AAA-003\n    /* REQ-AAA-004 */ }\n// REQ-AAA-002\n")
        self.assertEqual(self.ids(text), {"REQ-AAA-001", "REQ-AAA-003", "REQ-AAA-004"})


class PlanFileMilestoneTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        d = pathlib.Path(self.tmp.name)
        self.spec = d / "SPEC.md"
        self.spec.write_text("REQ-AAA-001 one\nREQ-AAA-002 two\n", encoding="utf-8")
        self.plan = d / "plan.md"
        self.plan.write_text("covers REQ-AAA-001\n", encoding="utf-8")
        self.tests = d / "tests"
        self.tests.mkdir()
        (self.tests / "a_test.py").write_text("# REQ-AAA-001\n", encoding="utf-8")

    def run_main(self, *extra):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            code = rc.main(["--spec", str(self.spec), "--plan-file", str(self.plan),
                            "--milestone", "M1", *extra])
        return code, out.getvalue()

    def test_plan_file_with_milestone_still_reports_missing_plan_ids(self):
        code, out = self.run_main()
        self.assertEqual(code, 1, out)
        self.assertIn("MISSING in plan: REQ-AAA-002", out)

    def test_plan_file_with_milestone_still_reports_missing_test_ids(self):
        code, out = self.run_main("--tests", str(self.tests))
        self.assertEqual(code, 1, out)
        self.assertIn("MISSING in plan: REQ-AAA-002", out)
        self.assertIn("MISSING in tests: REQ-AAA-002", out)


if __name__ == "__main__":
    unittest.main()
