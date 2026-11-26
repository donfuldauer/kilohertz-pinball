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
import shutil
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
                "crates/**",
                "Cargo.toml",
                "Cargo.lock",
                "tables/**",
                "scripts/windowed/**",
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

    def paths(self, env_target, *args, env_bundle=None):
        env = {
            k: v
            for k, v in os.environ.items()
            if k not in ("CARGO_TARGET_DIR", "CARGO_BUILD_TARGET", "KP_GREYBOX_BUNDLE")
        }
        if env_bundle is not None:
            env["KP_GREYBOX_BUNDLE"] = env_bundle
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

    def test_bundle_defaults_to_the_target_greybox_and_honours_the_override(self):
        p = self.paths("/abs/t", "/abs/o")
        self.assertEqual(p["bundle"], "/abs/t/greybox")
        p = self.paths("/abs/t", "/abs/o", env_bundle="/abs/other")
        self.assertEqual(p["bundle"], "/abs/other")
        p = self.paths("/abs/t", "/abs/o", env_bundle="rel/bundle")
        self.assertEqual(p["bundle"], str(ROOT) + "/rel/bundle")
        self.assertEqual(p["app"], "/abs/t/release/pin_app")

    def test_exports_the_effective_bundle_for_the_driver(self):
        text = RUN_SH.read_text(encoding="utf-8")
        self.assertIn("bundle=${KP_GREYBOX_BUNDLE:-$target/greybox}", text)
        self.assertIn('export KP_GREYBOX_BUNDLE="$bundle"', text)

    def test_exports_the_absolute_target_before_cargo(self):
        text = RUN_SH.read_text(encoding="utf-8")
        export = text.index('export CARGO_TARGET_DIR="$target"')
        self.assertLess(export, text.index("cargo build --release -p pin_app"))
        self.assertLess(text.index("*) target=$root/$target ;;"), export)


class RunShSetupTest(unittest.TestCase):
    """CARGO_BUILD_TARGET is rejected and a failed setup never leaves the
    previous run's artifacts (gate review of #163, findings M11 and M12)."""

    def run_sh(self, env, *args, cwd=None):
        return subprocess.run(
            ["sh", str(RUN_SH), *args],
            cwd=cwd,
            env=env,
            capture_output=True,
            text=True,
            timeout=30,
        )

    def test_build_target_is_rejected_with_and_without_print_paths(self):
        env = dict(os.environ, CARGO_BUILD_TARGET="x86_64-unknown-linux-gnu")
        with tempfile.TemporaryDirectory() as out:
            for args in (["--print-paths", out], [out]):
                r = self.run_sh(env, *args)
                self.assertEqual(r.returncode, 2, r.stderr)
                self.assertIn(
                    "run.sh: CARGO_BUILD_TARGET is not supported (the app is measured at ",
                    r.stderr,
                )
                self.assertIn("/release/pin_app); unset it", r.stderr)
                self.assertEqual(r.stdout, "")
            self.assertEqual(os.listdir(out), [], "rejection touches nothing")

    def test_print_paths_without_build_target_is_unchanged_and_read_only(self):
        env = {
            k: v
            for k, v in os.environ.items()
            if k not in ("CARGO_BUILD_TARGET", "KP_GREYBOX_BUNDLE")
        }
        env["CARGO_TARGET_DIR"] = "/abs/t"
        with tempfile.TemporaryDirectory() as tmp:
            out = pathlib.Path(tmp) / "new_out"
            stale = pathlib.Path(tmp) / "keep"
            stale.mkdir()
            (stale / "summary.txt").write_text("verdict: PASS\n")
            r = self.run_sh(env, "--print-paths", str(stale))
            self.assertEqual(r.returncode, 0, r.stderr)
            self.assertEqual(
                r.stdout,
                f"target=/abs/t\nout={stale}\nbundle=/abs/t/greybox\napp=/abs/t/release/pin_app\n",
            )
            self.assertEqual((stale / "summary.txt").read_text(), "verdict: PASS\n")
            self.run_sh(env, "--print-paths", str(out))
            self.assertFalse(out.exists(), "--print-paths creates nothing")

    def tool_dir(self, tmp, rm_stub=None):
        """A PATH holding only the tools run.sh needs up to the tool check;
        Xvfb is a stub, xdotool is absent, so setup fails at `xdotool`."""
        bindir = pathlib.Path(tmp) / "bin"
        bindir.mkdir()
        for tool in ["sh", "dirname", "cat", "rm", "mkdir", "printf", "env"]:
            found = shutil.which(tool, path="/usr/bin:/bin")
            self.assertIsNotNone(found, tool)
            if tool == "rm" and rm_stub is not None:
                real = found
                (bindir / "rm").write_text(rm_stub.replace("@REAL@", real))
                (bindir / "rm").chmod(0o755)
            else:
                (bindir / tool).symlink_to(found)
        stub = bindir / "Xvfb"
        stub.write_text("#!/bin/sh\nexit 0\n")
        stub.chmod(0o755)
        return bindir

    def test_failed_setup_clears_stale_artifacts_and_publishes_a_failure(self):
        with tempfile.TemporaryDirectory() as tmp:
            bindir = self.tool_dir(tmp)
            out = pathlib.Path(tmp) / "out"
            out.mkdir()
            (out / "summary.txt").write_text("verdict: PASS\n")
            (out / "latency_trace.csv").write_text("stale\n")
            (out / "pin_app_stderr.log").write_text("stale\n")
            (out / "frame_0.png").write_text("stale\n")
            (out / "unrelated.txt").write_text("keep\n")
            env = {"PATH": str(bindir), "CARGO_TARGET_DIR": str(pathlib.Path(tmp) / "t")}
            r = self.run_sh(env, str(out))
            self.assertNotEqual(r.returncode, 0)
            self.assertFalse((out / "latency_trace.csv").exists())
            self.assertFalse((out / "pin_app_stderr.log").exists())
            self.assertFalse((out / "frame_0.png").exists())
            self.assertTrue((out / "unrelated.txt").exists())
            summary = (out / "summary.txt").read_text()
            self.assertTrue(
                summary.startswith("verdict: FAIL (windowed setup:"), summary
            )
            self.assertIn("xdotool", summary)
            self.assertNotIn("PASS", summary)

    def test_leftover_directories_with_artifact_names_are_removed_cleanly(self):
        with tempfile.TemporaryDirectory() as tmp:
            bindir = self.tool_dir(tmp)
            out = pathlib.Path(tmp) / "out"
            (out / "frame_debug.png").mkdir(parents=True)
            (out / "frame_debug.png" / "inner").write_text("x\n")
            (out / "pin_app_stderr.log").mkdir()
            (out / "summary.txt").write_text("verdict: PASS\n")
            (out / "unrelated.txt").write_text("keep\n")
            env = {"PATH": str(bindir), "CARGO_TARGET_DIR": str(pathlib.Path(tmp) / "t")}
            r = self.run_sh(env, str(out))
            self.assertNotEqual(r.returncode, 0)
            self.assertEqual(
                sorted(os.listdir(out)), ["summary.txt", "unrelated.txt"]
            )
            self.assertEqual(
                (out / "summary.txt").read_text(),
                "verdict: FAIL (windowed setup: xdotool not found)\n",
            )

    def test_cleanup_failure_never_leaves_the_old_summary(self):
        # rm refuses to remove frame_* entries; the stale summary is removed
        # first and the trap reports the cleanup stage.
        stub = (
            "#!/bin/sh\n"
            'for a in "$@"; do case $a in */frame_*) exit 1;; esac; done\n'
            'exec @REAL@ "$@"\n'
        )
        with tempfile.TemporaryDirectory() as tmp:
            bindir = self.tool_dir(tmp, rm_stub=stub)
            out = pathlib.Path(tmp) / "out"
            (out / "frame_debug.png").mkdir(parents=True)
            (out / "summary.txt").write_text("verdict: PASS\n")
            env = {"PATH": str(bindir), "CARGO_TARGET_DIR": str(pathlib.Path(tmp) / "t")}
            r = self.run_sh(env, str(out))
            self.assertNotEqual(r.returncode, 0)
            self.assertEqual(
                (out / "summary.txt").read_text(),
                "verdict: FAIL (windowed setup: cleanup)\n",
            )


if __name__ == "__main__":
    unittest.main()
