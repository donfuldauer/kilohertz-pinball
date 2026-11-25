#!/usr/bin/env python3
"""Regression tests for .agents/setup-env.sh section 1 (~/.bashrc persistence).

Run: ``python3 -I scripts/tests/setup_env_test.py`` or
``python3 -I -m unittest discover -s scripts/tests -p '*_test.py'``.

Uses an isolated temporary HOME and never modifies the real $HOME/.bashrc.
Tests extract and execute section 1 of ``.agents/setup-env.sh`` directly in a
clean bash subprocess (plus verify ``bash -n`` and full-script invocation with
isolated HOME and stubbed external tools).
"""

import os
import stat
import subprocess
import tempfile
import unittest

HERE = os.path.dirname(os.path.abspath(__file__))
REPO = os.path.dirname(os.path.dirname(HERE))
SETUP_ENV = os.path.join(REPO, ".agents", "setup-env.sh")


def read_setup_env():
    with open(SETUP_ENV, encoding="utf-8") as f:
        return f.read()


def extract_section1_script():
    """Return the executable bash prefix of setup-env.sh through section 1."""
    lines = read_setup_env().splitlines(keepends=True)
    out = []
    found_section2 = False
    for line in lines:
        if line.startswith("# 2. Linux system packages"):
            found_section2 = True
            # Drop the section 2 separator banner line already appended.
            if out and out[-1].startswith("# ---"):
                out.pop()
            break
        out.append(line)
    assert found_section2, "section 2 delimiter not found in setup-env.sh"
    script = "".join(out)
    assert "# 1. Non-interactive ~/.bashrc persistence" in script
    return script


class SetupEnvScriptTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self.tmp.cleanup)
        self.home = os.path.join(self.tmp.name, "home")
        os.makedirs(self.home)
        self.cargo_home = os.path.join(self.tmp.name, "cargo")
        self.cargo_bin = os.path.join(self.cargo_home, "bin")
        self.bashrc = os.path.join(self.home, ".bashrc")
        self.section1_script = os.path.join(self.tmp.name, "section1.sh")
        with open(self.section1_script, "w", encoding="utf-8") as f:
            f.write(extract_section1_script())
        os.chmod(self.section1_script, 0o755)

    def run_section1(self, extra_env=None):
        env = {
            "PATH": "/usr/local/bin:/usr/bin:/bin",
            "HOME": self.home,
            "CARGO_HOME": self.cargo_home,
        }
        if extra_env:
            env.update(extra_env)
        p = subprocess.run(
            ["bash", self.section1_script],
            cwd=REPO,
            env=env,
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(p.returncode, 0, f"stdout={p.stdout}\nstderr={p.stderr}")
        return p

    def read_bashrc(self):
        with open(self.bashrc, encoding="utf-8") as f:
            return f.read()

    def expected_export_line(self):
        return f'export PATH="{self.cargo_bin}:$PATH"'

    def noninteractive_path(self, extra_env=None):
        env = {
            "PATH": "/usr/bin:/bin",
            "HOME": self.home,
            "BASH_ENV": self.bashrc,
        }
        if extra_env:
            env.update(extra_env)
        p = subprocess.run(
            ["bash", "-c", 'printf "%s" "$PATH"'],
            env=env,
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(p.returncode, 0, f"stdout={p.stdout}\nstderr={p.stderr}")
        return p.stdout

    def test_bash_syntax_and_executable_mode(self):
        p = subprocess.run(
            ["bash", "-n", SETUP_ENV],
            cwd=REPO,
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(p.returncode, 0, p.stderr)
        mode = stat.S_IMODE(os.stat(SETUP_ENV).st_mode)
        self.assertEqual(mode & 0o111, 0o111, f"expected executable mode, got {oct(mode)}")
        text = read_setup_env()
        self.assertNotIn("sed -i '/case \\$- in/,/esac/d'", text)
        self.assertIn('FIRST_LINE=$(head -n 1 "$HOME/.bashrc" || true)', text)

    def test_missing_bashrc_created_with_line1_export(self):
        self.assertFalse(os.path.exists(self.bashrc))
        self.run_section1()
        content = self.read_bashrc()
        self.assertEqual(content, self.expected_export_line() + "\n")
        self.assertIn(self.cargo_bin, self.noninteractive_path().split(":"))

    def test_empty_bashrc_gets_line1_export(self):
        with open(self.bashrc, "w", encoding="utf-8") as f:
            f.write("")
        self.run_section1()
        self.assertEqual(self.read_bashrc(), self.expected_export_line() + "\n")
        self.assertIn(self.cargo_bin, self.noninteractive_path().split(":"))

    def test_idempotency_does_not_duplicate_line1(self):
        initial = "case $- in *i*) ;; *) return;; esac\nalias ll='ls -l'\n"
        with open(self.bashrc, "w", encoding="utf-8") as f:
            f.write(initial)

        self.run_section1()
        first = self.read_bashrc()
        self.run_section1()
        second = self.read_bashrc()

        self.assertEqual(first, second)
        lines = second.splitlines()
        self.assertEqual(lines[0], self.expected_export_line())
        self.assertEqual(lines.count(self.expected_export_line()), 1)
        self.assertEqual("\n".join(lines[1:]) + "\n", initial)

    def test_single_line_guard_preserved_and_path_exported(self):
        initial = (
            "case $- in *i*) ;; *) return;; esac\n"
            "export AFTER_GUARD=1\n"
        )
        with open(self.bashrc, "w", encoding="utf-8") as f:
            f.write(initial)

        self.run_section1()
        content = self.read_bashrc()
        self.assertEqual(content, self.expected_export_line() + "\n" + initial)
        self.assertIn("case $- in *i*) ;; *) return;; esac", content)
        self.assertIn("export AFTER_GUARD=1", content)
        path_entries = self.noninteractive_path().split(":")
        self.assertEqual(path_entries[0], self.cargo_bin)

    def test_multiline_guard_preserved_and_path_exported(self):
        initial = (
            "# ~/.bashrc\n"
            "case $- in\n"
            "  *i*) ;;\n"
            "  *) return;;\n"
            "esac\n"
            "export TRAILING_LINE=yes\n"
        )
        with open(self.bashrc, "w", encoding="utf-8") as f:
            f.write(initial)

        self.run_section1()
        content = self.read_bashrc()
        self.assertEqual(content, self.expected_export_line() + "\n" + initial)
        self.assertIn("export TRAILING_LINE=yes", content)
        path_entries = self.noninteractive_path().split(":")
        self.assertEqual(path_entries[0], self.cargo_bin)

    def test_conditional_export_before_guard_preserved_when_foo_unset(self):
        initial = (
            f'if [ -n "${{FOO:-}}" ]; then export PATH="{self.cargo_bin}:$PATH"; fi\n'
            "case $- in *i*) ;; *) return;; esac\n"
            "export STILL_HERE=1\n"
        )
        with open(self.bashrc, "w", encoding="utf-8") as f:
            f.write(initial)

        self.run_section1()
        content = self.read_bashrc()
        self.assertEqual(content, self.expected_export_line() + "\n" + initial)

        # Ensure FOO is unset when sourcing via BASH_ENV.
        path_entries = self.noninteractive_path().split(":")
        self.assertEqual(path_entries[0], self.cargo_bin)
        # Also verify the preserved conditional works cleanly when FOO is set.
        path_entries_set = self.noninteractive_path({"FOO": "1"}).split(":")
        self.assertEqual(path_entries_set[0], self.cargo_bin)

    def test_export_as_sole_if_body_remains_syntactically_valid(self):
        initial = (
            'if [ -n "${FOO:-}" ]; then\n'
            f'  export PATH="{self.cargo_bin}:$PATH"\n'
            "fi\n"
            "case $- in *i*) ;; *) return;; esac\n"
        )
        with open(self.bashrc, "w", encoding="utf-8") as f:
            f.write(initial)

        self.run_section1()
        content = self.read_bashrc()
        self.assertEqual(content, self.expected_export_line() + "\n" + initial)

        # Verify bash -n passes on the resulting ~/.bashrc and sourcing works.
        syntax = subprocess.run(
            ["bash", "-n", self.bashrc],
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(syntax.returncode, 0, syntax.stderr)
        path_entries = self.noninteractive_path().split(":")
        self.assertEqual(path_entries[0], self.cargo_bin)

    def test_preserves_bashrc_permissions(self):
        initial = "case $- in *i*) ;; *) return;; esac\n"
        for mode in (0o600, 0o644, 0o444):
            with self.subTest(mode=oct(mode)):
                if os.path.exists(self.bashrc):
                    os.chmod(self.bashrc, 0o600)
                with open(self.bashrc, "w", encoding="utf-8") as f:
                    f.write(initial)
                os.chmod(self.bashrc, mode)

                self.run_section1()
                content = self.read_bashrc()
                self.assertEqual(content, self.expected_export_line() + "\n" + initial)
                self.assertEqual(content.splitlines()[0], self.expected_export_line())
                actual = stat.S_IMODE(os.stat(self.bashrc).st_mode)
                self.assertEqual(actual, mode)

                # Confirm running twice (idempotency) succeeds and preserves content and mode.
                self.run_section1()
                self.assertEqual(self.read_bashrc(), content)
                self.assertEqual(stat.S_IMODE(os.stat(self.bashrc).st_mode), mode)

    def test_full_setup_env_invocation_with_stubs(self):
        # Invoke the actual .agents/setup-env.sh inside a stubbed PATH and isolated
        # HOME/CARGO_HOME so sections 2-6 exit cleanly without mutating the host.
        stub_bin = os.path.join(self.tmp.name, "stub_bin")
        os.makedirs(stub_bin)
        rust_ver = None
        with open(os.path.join(REPO, "versions.toml"), encoding="utf-8") as f:
            for line in f:
                if line.startswith("rust = "):
                    rust_ver = line.split('"')[1]
                    break
        self.assertIsNotNone(rust_ver)

        sysroot = os.path.join(self.tmp.name, "sysroot")
        os.makedirs(os.path.join(sysroot, "lib", "rustlib", "wasm32-unknown-unknown"))

        stubs = {
            "apt-get": "#!/bin/sh\necho 'apt-get should not run' >&2\nexit 99\n",
            "dpkg-query": "#!/bin/sh\necho 'install ok installed'\n",
            "dotnet": "#!/bin/sh\necho '8.0.100'\n",
            "rustc": (
                "#!/bin/sh\n"
                'if [ "${1:-}" = "--print" ] && [ "${2:-}" = "sysroot" ]; then\n'
                f'  echo "{sysroot}"\n'
                "else\n"
                f'  echo "rustc {rust_ver} (stub)"\n'
                "fi\n"
            ),
            "rustup": "#!/bin/sh\nprintf 'rustfmt-x86_64\\nclippy-x86_64\\nllvm-tools-x86_64\\n'\n",
            "cargo-llvm-cov": "#!/bin/sh\necho 'cargo-llvm-cov 0.6.0'\n",
            "cargo": "#!/bin/sh\nif [ \"${1:-}\" = \"llvm-cov\" ]; then echo 'cargo-llvm-cov 0.6.0'; else echo 'cargo 1.97.0'; fi\n",
            "cmake": "#!/bin/sh\necho 'cmake version 3.28.0'\n",
            "xvfb-run": "#!/bin/sh\nexit 0\n",
            "valgrind": "#!/bin/sh\necho 'valgrind-3.22.0'\n",
        }
        for name, body in stubs.items():
            path = os.path.join(stub_bin, name)
            with open(path, "w", encoding="utf-8") as f:
                f.write(body)
            os.chmod(path, 0o755)

        initial = "case $- in *i*) ;; *) return;; esac\nexport KEEP=1\n"
        with open(self.bashrc, "w", encoding="utf-8") as f:
            f.write(initial)
        os.chmod(self.bashrc, 0o600)

        env = {
            "PATH": stub_bin + os.pathsep + "/usr/bin:/bin",
            "HOME": self.home,
            "CARGO_HOME": self.cargo_home,
        }
        p = subprocess.run(
            ["bash", SETUP_ENV],
            cwd=REPO,
            env=env,
            capture_output=True,
            text=True,
            timeout=30,
        )
        self.assertEqual(p.returncode, 0, f"stdout={p.stdout}\nstderr={p.stderr}")
        self.assertEqual(self.read_bashrc(), self.expected_export_line() + "\n" + initial)
        self.assertEqual(stat.S_IMODE(os.stat(self.bashrc).st_mode), 0o600)


if __name__ == "__main__":
    unittest.main(verbosity=2)
