"""Structure of .github/workflows/windowed.yml (KP-002, #82).

Run: python3 -I -m unittest discover -s scripts/windowed/tests -p '*_test.py'

The workflow is read as text (no YAML module: standard library only). The
checks: the triggers and their paths, read-only permissions, the exact Rust
version check, the LP_NUM_THREADS cap, the forced fallback adapter, the
artifact upload on every outcome, and that ci.yml's pr-checks job does not
depend on it. RunShPathsTest runs scripts/windowed/run.sh --print-paths
(nothing is built) to check its path resolution.
"""

import os
import pathlib
import re
import subprocess
import tempfile
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[3]
WORKFLOW = ROOT / ".github" / "workflows" / "windowed.yml"
CI = ROOT / ".github" / "workflows" / "ci.yml"
RUN_SH = ROOT / "scripts" / "windowed" / "run.sh"


def block(text, key, indent):
    """The lines under `<indent>key:` up to the next key at that indent."""
    lines = text.splitlines()
    pat = re.compile(r"^" + " " * indent + re.escape(key) + r":\s*(#.*)?$")
    for i, line in enumerate(lines):
        if pat.match(line):
            out = []
            for nxt in lines[i + 1:]:
                if nxt.strip() and len(nxt) - len(nxt.lstrip()) <= indent:
                    break
                out.append(nxt)
            return out
    raise AssertionError(f"no `{key}:` at indent {indent}")


def steps(text):
    """The job's steps as lists of lines (split on `      - `)."""
    out, cur = [], None
    for line in block(text, "steps", 4):
        if line.startswith("      - "):
            cur = [line]
            out.append(cur)
        elif cur is not None:
            cur.append(line)
    return out


class WindowedWorkflowTest(unittest.TestCase):
    def setUp(self):
        self.text = WORKFLOW.read_text(encoding="utf-8")

    def test_name(self):
        self.assertRegex(self.text, r"(?m)^name: windowed$")

    def test_triggers_and_paths(self):
        on = block(self.text, "on", 0)
        joined = "\n".join(on)
        self.assertIn("  pull_request:", joined)
        self.assertIn("  workflow_dispatch:", joined)
        paths = [
            m.group(1)
            for m in re.finditer(r'^\s+- "([^"]+)"$', "\n".join(block(joined, "pull_request", 2)), re.M)
        ]
        self.assertEqual(
            paths,
            [
                "crates/pin_app/**",
                "crates/pin_render/**",
                "crates/pin_input/**",
                "tables/**",
                ".github/workflows/windowed.yml",
            ],
        )
        push = "\n".join(block(joined, "push", 2))
        self.assertIn("branches: [main]", push)
        self.assertNotIn("paths", push, "push to main has no path filter")

    def test_permissions_read_only(self):
        self.assertEqual(
            [l.strip() for l in block(self.text, "permissions", 0) if l.strip()],
            ["contents: read"],
        )

    def test_concurrency_cancels_only_pull_requests(self):
        conc = "\n".join(block(self.text, "concurrency", 0))
        self.assertIn("group: ${{ github.workflow }}-${{ github.ref }}-${{ github.event_name }}", conc)
        self.assertIn("cancel-in-progress: ${{ github.event_name == 'pull_request' }}", conc)

    def test_exact_rust_version_check(self):
        self.assertIn('RUST_VERSION: "1.97.0"', self.text)
        self.assertIn("uses: dtolnay/rust-toolchain@1.97.0", self.text)
        self.assertIn('if [ "$version" != "$RUST_VERSION" ]; then', self.text)
        self.assertIn("version=$(rustc --version | awk '{print $2}')", self.text)

    def test_actions_and_timeout(self):
        self.assertIn("uses: actions/checkout@v5", self.text)
        self.assertIn("uses: Swatinem/rust-cache@v2", self.text)
        m = re.search(r"(?m)^    timeout-minutes: (\d+)$", self.text)
        self.assertIsNotNone(m)
        self.assertTrue(30 <= int(m.group(1)) <= 45)

    def test_lp_num_threads_cap(self):
        m = re.search(r'(?m)^  LP_NUM_THREADS: "(\d+)"$', self.text)
        self.assertIsNotNone(m, "LP_NUM_THREADS set in the workflow env")
        self.assertTrue(1 <= int(m.group(1)) <= 3, "leaves a core of 4 vCPUs to the sim")
        self.assertIn("WGPU_BACKEND: vulkan", self.text)

    def test_packages_include_the_xtest_tools(self):
        for pkg in ["mesa-vulkan-drivers", "xvfb", "xdotool", "x11-apps", "imagemagick", "libxtst-dev"]:
            self.assertIn(pkg, self.text)

    def test_runs_the_driver(self):
        self.assertIn("tables/greybox/build.sh target/greybox", self.text)
        self.assertIn("scripts/windowed/run.sh", self.text)
        self.assertIn("cargo build --release -p pin_app", self.text)

    def test_run_step_forces_the_fallback_adapter(self):
        runs = [s for s in steps(self.text) if any("scripts/windowed/run.sh" in l for l in s)]
        self.assertEqual(len(runs), 1)
        self.assertIn('PIN_FORCE_FALLBACK_ADAPTER: "1"', "\n".join(runs[0]))
        self.assertIn("export PIN_FORCE_FALLBACK_ADAPTER=1", RUN_SH.read_text(encoding="utf-8"))

    def test_upload_artifact_always(self):
        uploads = [s for s in steps(self.text) if any("actions/upload-artifact@v4" in l for l in s)]
        self.assertEqual(len(uploads), 1)
        up = "\n".join(uploads[0])
        self.assertIn("if: always()", up)
        self.assertIn("name: windowed-latency", up)
        for f in ["latency_trace.csv", "summary.txt", "pin_app_stderr.log", "frame_*.png"]:
            self.assertIn(f, up)

    def test_not_in_pr_checks(self):
        ci = CI.read_text(encoding="utf-8")
        needs = "\n".join(block("\n".join(block(ci, "pr-checks", 2)), "needs", 4))
        self.assertNotIn("windowed", needs)
        self.assertNotIn("windowed", "\n".join(block(ci, "pr-checks", 2)))


class RunShPathsTest(unittest.TestCase):
    """run.sh resolves a relative CARGO_TARGET_DIR and out_dir against the
    repository root before Cargo runs (--print-paths stops before building)."""

    def paths(self, env_target, *args):
        env = {k: v for k, v in os.environ.items() if k != "CARGO_TARGET_DIR"}
        if env_target is not None:
            env["CARGO_TARGET_DIR"] = env_target
        with tempfile.TemporaryDirectory() as cwd:
            out = subprocess.run(
                ["sh", str(RUN_SH), "--print-paths", *args],
                cwd=cwd,
                env=env,
                capture_output=True,
                text=True,
                timeout=30,
                check=True,
            ).stdout
        return dict(line.split("=", 1) for line in out.splitlines())

    def test_relative_paths_resolve_under_the_root(self):
        p = self.paths("target/rel", "target/out_rel")
        root = str(ROOT)
        self.assertEqual(p["target"], root + "/target/rel")
        self.assertEqual(p["out"], root + "/target/out_rel")
        self.assertEqual(p["bundle"], root + "/target/rel/greybox")
        self.assertEqual(p["app"], root + "/target/rel/release/pin_app")

    def test_absolute_paths_unchanged_and_defaults(self):
        p = self.paths("/abs/t", "/abs/o")
        self.assertEqual((p["target"], p["out"]), ("/abs/t", "/abs/o"))
        p = self.paths(None)
        self.assertEqual(p["target"], str(ROOT) + "/target")
        self.assertEqual(p["out"], str(ROOT) + "/target/windowed")

    def test_exports_the_absolute_target_before_cargo(self):
        text = RUN_SH.read_text(encoding="utf-8")
        export = text.index('export CARGO_TARGET_DIR="$target"')
        self.assertLess(export, text.index("cargo build --release -p pin_app"))
        self.assertLess(text.index("*) target=$root/$target ;;"), export)


if __name__ == "__main__":
    unittest.main()
