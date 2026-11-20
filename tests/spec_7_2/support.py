"""Shared helpers for the 7.2 black-box tests (stdlib only, hermetic)."""
import hashlib
import json
import re
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
PACKAGE_PY = REPO / "scripts" / "package.py"
HUMAN_PY = REPO / "scripts" / "human_acceptance.py"
RELEASE_YML = REPO / ".github" / "workflows" / "release.yml"

WIN = "x86_64-pc-windows-msvc"
LIN = "x86_64-unknown-linux-gnu"
MAC = "universal-apple-darwin"
TARGETS = {WIN: "windows-x64", LIN: "linux-x64", MAC: "macos-universal"}
ARCHIVE_EXT = {WIN: ".zip", MAC: ".zip", LIN: ".tar.gz"}
TABLES = ("greybox", "apex_circuit", "sky_deck", "feature_lab")
PORTED = ("apex_circuit", "sky_deck", "feature_lab")
EXE_BYTES = b"#!fake-engine\x00\x01\x02 bytes\n"
BRIDGE_BYTES = b"MZ fake dof bridge\n"


def cargo_version():
    text = (REPO / "Cargo.toml").read_text(encoding="utf-8")
    block = text.split("[workspace.package]", 1)[1]
    m = re.search(r'^version\s*=\s*"([^"]+)"', block, re.M)
    return m.group(1)


def sha256_file(path):
    return hashlib.sha256(Path(path).read_bytes()).hexdigest()


class TmpCase(unittest.TestCase):
    """TestCase with a per-test temp directory in self.tmp (a resolved Path)."""

    def setUp(self):
        td = tempfile.TemporaryDirectory(prefix="spec72_")
        self.addCleanup(td.cleanup)
        self.tmp = Path(td.name).resolve()


def make_bundle_dir(root, wasm_for=PORTED, skip_files=()):
    """Fake bundle folders. skip_files: 'table/file' names to omit."""
    root = Path(root)
    for t in TABLES:
        d = root / t
        d.mkdir(parents=True, exist_ok=True)
        files = {
            "table.toml": ('name = "%s"\n' % t).encode(),
            "rules.lua": ("-- rules for %s\n" % t).encode(),
        }
        if t in wasm_for:
            files["rules.wasm"] = b"\x00asm\x01\x00\x00\x00" + t.encode()
        for fn, data in files.items():
            if "%s/%s" % (t, fn) in skip_files:
                continue
            (d / fn).write_bytes(data)
    return root


def make_exe(root):
    p = Path(root) / "fake-engine"
    p.write_bytes(EXE_BYTES)
    p.chmod(0o755)
    return p


def make_bridge_dir(root):
    d = Path(root) / "bridge"
    d.mkdir(parents=True, exist_ok=True)
    (d / "KilohertzDofBridge.exe").write_bytes(BRIDGE_BYTES)
    return d


def run_script(script, args, env=None, cwd=None):
    return subprocess.run(
        [sys.executable, str(script)] + [str(a) for a in args],
        capture_output=True, text=True, encoding="utf-8", env=env, cwd=cwd, timeout=120,
    )


class PackageCase(TmpCase):
    VERSION = "9.8.7"

    def setUp(self):
        super().setUp()
        self.bundles = make_bundle_dir(self.tmp / "bundles")
        self.exe = make_exe(self.tmp)
        self.bridge = make_bridge_dir(self.tmp)

    def package(self, target, out=None, extra=(), version="default", bundles=None,
                with_bridge=None, env=None):
        out = out if out is not None else self.tmp / "out"
        args = ["--target", target, "--out", out,
                "--exe", self.exe, "--bundle-dir", bundles or self.bundles]
        if version == "default":
            version = self.VERSION
        if version is not None:
            args += ["--version", version]
        if with_bridge is None:
            with_bridge = target == WIN
        if with_bridge:
            args += ["--dof-bridge", self.bridge]
        args += list(extra)
        return run_script(PACKAGE_PY, args, env=env)

    def stem(self, target, version=None):
        return "kilohertz-pinball-%s-%s" % (version or self.VERSION, TARGETS[target])

    def ok(self, target, out=None, **kw):
        r = self.package(target, out=out, **kw)
        self.assertEqual(r.returncode, 0, "stdout=%s\nstderr=%s" % (r.stdout, r.stderr))
        return r

    def pkgdir(self, target, out=None):
        return (out or self.tmp / "out") / self.stem(target)

    def manifest(self, out=None):
        return json.loads(((out or self.tmp / "out") / "PACKAGE_MANIFEST.json").read_text("utf-8"))


def strip_yaml_comments(text):
    out = []
    for line in text.splitlines():
        if line.lstrip().startswith("#"):
            continue
        out.append(re.sub(r"\s#.*$", "", line))
    return "\n".join(out) + "\n"
