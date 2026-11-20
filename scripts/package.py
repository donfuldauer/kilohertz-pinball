#!/usr/bin/env python3
"""Release packaging for Kilohertz Pinball (PLAN 7.2).

Builds one platform package: the engine executable (renamed
``kilohertz-pinball[.exe]``), the four tables as deterministic ``.pinbundle``
zip archives in ``tables/`` beside the executable, the licences, the README,
``docs/HUMAN_ACCEPTANCE.md`` (when present) and, for Windows only, the DOF
bridge in ``dof_bridge/`` marked "unverified on hardware" (D33).

Usage::

    python3 scripts/package.py --target <T> --out <dir> [--version <V>]
        [--exe <path>] [--bundle-dir <dir>] [--dof-bridge <dir>]
        [--dry-run] [--list]

Targets and package layouts (``<out>/kilohertz-pinball-<version>-<platform>/``):

* ``x86_64-pc-windows-msvc`` -> ``windows-x64``: ``kilohertz-pinball.exe``,
  ``tables/<name>.pinbundle``, ``dof_bridge/`` (publish output plus
  ``UNVERIFIED.txt``), ``LICENSE-MIT``, ``LICENSE-APACHE``, ``README.md``,
  ``docs/HUMAN_ACCEPTANCE.md``; archived as ``.zip``.
* ``x86_64-unknown-linux-gnu`` -> ``linux-x64``: the same without
  ``dof_bridge/`` and without ``.exe``; archived as ``.tar.gz``.
* ``universal-apple-darwin`` -> ``macos-universal``:
  ``Kilohertz Pinball.app/Contents/{Info.plist,MacOS/kilohertz-pinball,
  MacOS/tables/<name>.pinbundle}`` and the licence/README/docs files beside
  the app; archived as ``.zip``.

Beside the package directory the script writes the archive, a
``<archive base>.sha256`` file (``<hex>  <name>``) and ``PACKAGE_MANIFEST.json``
(every file of the package directory with size and SHA-256, sorted by path).
Everything is byte-reproducible: sorted entries, fixed timestamps and modes,
no host paths (D9).

Inputs are prebuilt (``--exe``, ``--bundle-dir``, ``--dof-bridge``) or built
here: ``cargo build --release -p pin_app --target <triple>`` (macOS: both
arches, then ``lipo``), ``sh tables/<name>/build.sh`` (``bash`` on Windows)
and ``dotnet publish`` for the bridge. The script never runs ``rustup``, never
touches the network on its own, and never signs (apart from an ad-hoc macOS
``codesign -s -`` when that tool exists, which is skipped in ``--dry-run``),
notarizes, uploads, tags or pushes.

``--dry-run`` builds everything, then deletes the archive and ``.sha256``
(keeping the package directory and manifest) and prints ``DRY RUN OK``.
``--list`` prints the package's file paths, one per line, and writes nothing
(no builds; inputs that are given are validated).

Exit codes: 0 success, 1 packaging failure (missing or invalid input, failed
build), 2 usage error. Python 3 standard library only.
"""

import argparse
import gzip
import hashlib
import io
import json
import os
import plistlib
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import zipfile
import zlib
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent

TARGETS = (
    "x86_64-pc-windows-msvc",
    "x86_64-unknown-linux-gnu",
    "universal-apple-darwin",
)
PLATFORMS = {
    "x86_64-pc-windows-msvc": "windows-x64",
    "x86_64-unknown-linux-gnu": "linux-x64",
    "universal-apple-darwin": "macos-universal",
}
WINDOWS = "x86_64-pc-windows-msvc"
LINUX = "x86_64-unknown-linux-gnu"
MACOS = "universal-apple-darwin"
MACOS_ARCHES = ("aarch64-apple-darwin", "x86_64-apple-darwin")

# The four shipped tables, in packaging order, and the ones with a Wasm port (6.3).
TABLES = ("greybox", "apex_circuit", "sky_deck", "feature_lab")
PORTED = frozenset(("apex_circuit", "sky_deck", "feature_lab"))
REQUIRED_BUNDLE_FILES = ("table.toml", "rules.lua")

APP_NAME = "Kilohertz Pinball.app"
EXE_STEM = "kilohertz-pinball"
BUNDLE_ID = "com.kilohertz.pinball"
MACOS_MIN = "12.0"
BRIDGE_EXE = "KilohertzDofBridge.exe"
BRIDGE_CSPROJ = "tools/dof_bridge/src/KilohertzDofBridge/KilohertzDofBridge.csproj"
UNVERIFIED_TEXT = (
    "Kilohertz Pinball DOF bridge: UNVERIFIED ON HARDWARE (D33).\n"
    "\n"
    "This build of KilohertzDofBridge.exe has been tested only against recorded\n"
    "telemetry fixtures and a mock DOF sink. It has not been verified with real\n"
    "DirectOutput Framework devices. Use it at your own risk and report results.\n"
)
DOC_FILES = ("LICENSE-MIT", "LICENSE-APACHE", "README.md")
OPTIONAL_DOC_FILES = ("docs/HUMAN_ACCEPTANCE.md",)
MANIFEST_NAME = "PACKAGE_MANIFEST.json"

# Deterministic archive settings (A7, A8).
ZIP_TIME = (1980, 1, 1, 0, 0, 0)
ZIP_LEVEL = 9
FILE_MODE = 0o644
EXEC_MODE = 0o755
DIR_MODE = 0o755

# pin_assets zip limits (crates/pin_assets/src/zip/mod.rs, ZipLimits::default).
LIMIT_TOTAL = 512 * 1024 * 1024
LIMIT_FILE = 256 * 1024 * 1024
LIMIT_RATIO = 100
LIMIT_ENTRIES = 10_000
RATIO_FLOOR = 1 << 20
ARCHIVE_RATIO_FLOOR = 8 << 20

# OS metadata the loader ignores; never packed into a bundle.
OS_JUNK_NAMES = frozenset((".DS_Store", "Thumbs.db", "desktop.ini"))
OS_JUNK_DIRS = frozenset(("__MACOSX",))

VERSION_RE = re.compile(r"^[0-9A-Za-z][0-9A-Za-z.+_-]*$")


class UsageError(Exception):
    """A command-line usage error (exit 2)."""


class PackagingError(Exception):
    """A packaging failure: missing or invalid input, or a failed build (exit 1)."""


def warn(msg):
    """Prints a warning on stderr."""
    print(f"package.py: warning: {msg}", file=sys.stderr)


def info(msg):
    """Prints a progress line on stderr (stdout is kept for --list and results)."""
    print(f"package.py: {msg}", file=sys.stderr)


# ---------------------------------------------------------------------------
# Names and the pin_assets zip rules
# ---------------------------------------------------------------------------


def normalize_entry_name(name):
    """Python port of ``pin_assets::zip::normalize_path``.

    Returns the normalised path or raises ``ValueError`` for a name the loader
    refuses (NUL, absolute, drive prefix, ``..``).
    """
    if "\0" in name:
        raise ValueError(f"{name!r}: NUL in name")
    s = name.replace("\\", "/")
    if s.startswith("/") or (len(s) >= 2 and s[0].isascii() and s[0].isalpha() and s[1] == ":"):
        raise ValueError(f"{name!r}: absolute path")
    parts = []
    for component in s.split("/"):
        if component in ("", "."):
            continue
        if component == "..":
            raise ValueError(f"{name!r}: path traversal")
        parts.append(component)
    out = "/".join(parts)
    if len(out) >= 2 and out[0].isascii() and out[0].isalpha() and out[1] == ":":
        raise ValueError(f"{name!r}: absolute path")
    return out


def check_entry_name(name):
    """Raises ``ValueError`` unless ``name`` is a plain bundle entry name the
    loader accepts unchanged (normalises to itself, not a directory)."""
    if not name or name.endswith("/"):
        raise ValueError(f"{name!r}: empty or directory name")
    if normalize_entry_name(name) != name:
        raise ValueError(f"{name!r}: not in normal form (backslash, '.' or empty component)")


def is_os_junk(rel):
    """True for OS metadata files and folders the loader ignores."""
    parts = rel.split("/")
    return parts[0] in OS_JUNK_DIRS or parts[-1] in OS_JUNK_NAMES


def validate_bundle_zip(path):
    """Checks an archive against the pin_assets loader rules (names, symlinks,
    encryption, compression methods, size and ratio caps) and the bundle root
    files. Raises ``PackagingError`` naming the archive and entry."""
    try:
        size = path.stat().st_size
        with zipfile.ZipFile(path) as zf:
            infos = zf.infolist()
    except (OSError, zipfile.BadZipFile) as e:
        raise PackagingError(f"{path}: unreadable archive: {e}") from e
    if size > LIMIT_TOTAL + (1 << 20):
        raise PackagingError(f"{path}: archive is {size} bytes, over the loader cap")
    if len(infos) > LIMIT_ENTRIES:
        raise PackagingError(f"{path}: {len(infos)} entries, over the loader cap {LIMIT_ENTRIES}")
    names = set()
    total = 0
    for zi in infos:
        where = f"{path}: entry {zi.filename!r}"
        try:
            check_entry_name(zi.filename)
        except ValueError as e:
            raise PackagingError(f"{where}: {e}") from e
        mode = zi.external_attr >> 16
        if stat.S_ISLNK(mode) or stat.S_ISDIR(mode):
            raise PackagingError(f"{where}: not a plain file (mode {mode:o})")
        if zi.flag_bits & 0x1:
            raise PackagingError(f"{where}: encrypted")
        if zi.compress_type not in (zipfile.ZIP_STORED, zipfile.ZIP_DEFLATED):
            raise PackagingError(f"{where}: unsupported compression {zi.compress_type}")
        if zi.file_size > LIMIT_FILE:
            raise PackagingError(f"{where}: {zi.file_size} bytes, over the per-file cap")
        if zi.file_size > RATIO_FLOOR and zi.file_size // max(zi.compress_size, 1) > LIMIT_RATIO:
            raise PackagingError(f"{where}: compression ratio over {LIMIT_RATIO}")
        if zi.filename in names:
            raise PackagingError(f"{where}: duplicate entry")
        names.add(zi.filename)
        total += zi.file_size
    if total > LIMIT_TOTAL:
        raise PackagingError(f"{path}: {total} bytes uncompressed, over the loader cap")
    if total > ARCHIVE_RATIO_FLOOR and total // max(size, 1) > LIMIT_RATIO:
        raise PackagingError(f"{path}: whole-archive compression ratio over {LIMIT_RATIO}")
    for required in REQUIRED_BUNDLE_FILES:
        if required not in names:
            raise PackagingError(f"{path}: missing {required}")


# ---------------------------------------------------------------------------
# Deterministic archives
# ---------------------------------------------------------------------------


def _deflated_size(data):
    co = zlib.compressobj(ZIP_LEVEL, zlib.DEFLATED, -15)
    return len(co.compress(data) + co.flush())


def write_zip(dest, entries, ratio_guard=False):
    """Writes a deterministic zip.

    ``entries`` is an iterable of ``(arcname, data_bytes, mode)``; they are
    written sorted by name with a fixed 1980-01-01 timestamp, Unix attributes
    of a regular file with ``mode``, no directory entries and no extra fields.
    With ``ratio_guard`` an entry over 1 MiB that would deflate past the
    loader's 100:1 ratio cap is stored instead.
    """
    entries = sorted(entries, key=lambda e: e[0])
    with open(dest, "wb") as raw, zipfile.ZipFile(raw, "w", allowZip64=True) as zf:
        for name, data, mode in entries:
            zi = zipfile.ZipInfo(name, date_time=ZIP_TIME)
            zi.create_system = 3
            zi.external_attr = (stat.S_IFREG | mode) << 16
            method = zipfile.ZIP_DEFLATED
            if ratio_guard and len(data) > RATIO_FLOOR:
                if len(data) // max(_deflated_size(data), 1) > LIMIT_RATIO:
                    method = zipfile.ZIP_STORED
            zi.compress_type = method
            zf.writestr(zi, data, compress_type=method, compresslevel=ZIP_LEVEL)


def write_tar_gz(dest, root, prefix, exec_paths):
    """Writes a deterministic ``.tar.gz`` of ``root`` under ``prefix/``:
    sorted entries (directories included), mtime 0, uid/gid 0, no user or
    group names, fixed modes, and a gzip header with mtime 0 and no name."""
    rels = _walk_all(root)
    with open(dest, "wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0, compresslevel=9) as gz:
            with tarfile.open(fileobj=gz, mode="w", format=tarfile.PAX_FORMAT) as tar:
                top = tarfile.TarInfo(prefix)
                _tar_fix(top, tarfile.DIRTYPE, DIR_MODE, 0)
                tar.addfile(top)
                for rel, is_dir in rels:
                    ti = tarfile.TarInfo(f"{prefix}/{rel}")
                    if is_dir:
                        _tar_fix(ti, tarfile.DIRTYPE, DIR_MODE, 0)
                        tar.addfile(ti)
                        continue
                    data = (root / rel).read_bytes()
                    mode = EXEC_MODE if rel in exec_paths else FILE_MODE
                    _tar_fix(ti, tarfile.REGTYPE, mode, len(data))
                    tar.addfile(ti, io.BytesIO(data))


def _tar_fix(ti, kind, mode, size):
    ti.type = kind
    ti.mode = mode
    ti.size = size
    ti.mtime = 0
    ti.uid = ti.gid = 0
    ti.uname = ti.gname = ""


def _walk_all(root):
    """Every path under ``root`` as ``(posix relative path, is_dir)``, sorted;
    refuses symlinks."""
    out = []
    for dirpath, dirnames, filenames in os.walk(root):
        dirnames.sort()
        base = Path(dirpath)
        for d in dirnames:
            p = base / d
            if p.is_symlink():
                raise PackagingError(f"{p}: symlinks are not packaged")
            out.append((p.relative_to(root).as_posix(), True))
        for f in filenames:
            p = base / f
            if p.is_symlink() or not p.is_file():
                raise PackagingError(f"{p}: not a plain file")
            out.append((p.relative_to(root).as_posix(), False))
    out.sort()
    return out


def _files(root):
    """Sorted posix relative paths of the plain files under ``root``."""
    return [rel for rel, is_dir in _walk_all(root) if not is_dir]


def sha256_file(path):
    """Hex SHA-256 of a file."""
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


# ---------------------------------------------------------------------------
# Bundles
# ---------------------------------------------------------------------------


def bundle_entries(name, folder):
    """The ``(arcname, bytes, mode)`` entries of one table bundle folder,
    after validation (required files, Wasm port, plain files, loader names)."""
    if not folder.is_dir():
        raise PackagingError(f"{folder}: bundle folder for {name} is missing")
    for required in REQUIRED_BUNDLE_FILES:
        if not (folder / required).is_file():
            raise PackagingError(f"{folder / required}: required bundle file is missing ({name})")
    if name in PORTED and not (folder / "rules.wasm").is_file():
        raise PackagingError(
            f"{folder / 'rules.wasm'}: {name} is a ported table but its bundle has no rules.wasm"
        )
    entries = []
    for rel in _files(folder):
        if is_os_junk(rel):
            continue
        if name not in PORTED and rel == "rules.wasm":
            warn(f"{folder / rel}: {name} is not a ported table; rules.wasm left out (Lua rules ship)")
            continue
        try:
            check_entry_name(rel)
            rel.encode("utf-8")
        except (ValueError, UnicodeEncodeError) as e:
            raise PackagingError(f"{folder / rel}: name the bundle loader refuses: {e}") from e
        entries.append((rel, (folder / rel).read_bytes(), FILE_MODE))
    return entries


def write_bundle(name, folder, dest):
    """Zips one bundle folder into ``dest`` deterministically and checks it
    against the loader rules."""
    write_zip(dest, bundle_entries(name, folder), ratio_guard=True)
    validate_bundle_zip(dest)


# ---------------------------------------------------------------------------
# Inputs
# ---------------------------------------------------------------------------


def workspace_version(repo=REPO):
    """``[workspace.package] version`` from the root Cargo.toml."""
    path = repo / "Cargo.toml"
    try:
        text = path.read_text(encoding="utf-8")
    except OSError as e:
        raise PackagingError(f"{path}: cannot read: {e}") from e
    try:
        import tomllib  # Python 3.11+

        version = tomllib.loads(text).get("workspace", {}).get("package", {}).get("version")
    except ImportError:
        version = None
        section = None
        for line in text.splitlines():
            stripped = line.strip()
            if stripped.startswith("["):
                section = stripped
                continue
            m = re.match(r'^version\s*=\s*"([^"]+)"', stripped)
            if section == "[workspace.package]" and m:
                version = m.group(1)
                break
    if not isinstance(version, str) or not version:
        raise PackagingError(f"{path}: no [workspace.package] version")
    return version


def run(cmd, cwd=REPO):
    """Runs a build command; a failure is a PackagingError naming it."""
    info("run: " + " ".join(str(c) for c in cmd))
    try:
        subprocess.run([str(c) for c in cmd], cwd=str(cwd), check=True)
    except FileNotFoundError as e:
        raise PackagingError(f"{cmd[0]}: not found ({e})") from e
    except subprocess.CalledProcessError as e:
        raise PackagingError(f"{cmd[0]}: failed with exit code {e.returncode}: {' '.join(map(str, cmd))}") from e


def cargo_target_dir():
    """The cargo target directory (``CARGO_TARGET_DIR`` or ``<repo>/target``)."""
    env = os.environ.get("CARGO_TARGET_DIR")
    if env:
        p = Path(env)
        return p if p.is_absolute() else REPO / p
    return REPO / "target"


def build_exe(target, tmp):
    """Builds the engine for ``target`` and returns the executable path."""
    td = cargo_target_dir()
    if target == MACOS:
        parts = []
        for triple in MACOS_ARCHES:
            run(["cargo", "build", "--release", "-p", "pin_app", "--target", triple])
            parts.append(td / triple / "release" / "pin_app")
        for p in parts:
            if not p.is_file():
                raise PackagingError(f"{p}: cargo did not produce the executable")
        out = tmp / "pin_app-universal"
        run(["lipo", "-create", "-output", out] + parts)
        if not out.is_file():
            raise PackagingError(f"{out}: lipo did not produce the universal executable")
        return out
    run(["cargo", "build", "--release", "-p", "pin_app", "--target", target])
    exe = td / target / "release" / ("pin_app.exe" if target == WINDOWS else "pin_app")
    if not exe.is_file():
        raise PackagingError(f"{exe}: cargo did not produce the executable")
    return exe


def build_bundles(tmp):
    """Runs every table's build.sh into ``tmp/bundles/<name>``."""
    shell = "sh"
    if os.name == "nt":
        shell = shutil.which("bash")
        if shell is None:
            raise PackagingError("bash: not found on PATH (Git for Windows provides it)")
    out = tmp / "bundles"
    for name in TABLES:
        script = REPO / "tables" / name / "build.sh"
        if not script.is_file():
            raise PackagingError(f"{script}: table build script is missing")
        run([shell, f"tables/{name}/build.sh", (out / name).as_posix()])
    return out


def build_dof_bridge(tmp):
    """``dotnet publish`` of the bridge into ``tmp/dof_bridge``."""
    out = tmp / "dof_bridge"
    run([
        "dotnet", "publish", BRIDGE_CSPROJ, "-c", "Release", "-r", "win-x64",
        "--self-contained", "false", "-o", out,
    ])
    return out


def check_exe(path):
    """A prebuilt engine executable must be a regular file."""
    if not path.is_file():
        raise PackagingError(f"{path}: engine executable not found")
    return path


def check_dof_dir(path):
    """A prebuilt bridge publish dir must hold KilohertzDofBridge.exe."""
    if not path.is_dir():
        raise PackagingError(f"{path}: DOF bridge publish directory not found")
    if not (path / BRIDGE_EXE).is_file():
        raise PackagingError(f"{path / BRIDGE_EXE}: DOF bridge executable is missing")
    return path


# ---------------------------------------------------------------------------
# Layout
# ---------------------------------------------------------------------------


def package_name(version, target):
    """``kilohertz-pinball-<version>-<platform>``."""
    return f"{EXE_STEM}-{version}-{PLATFORMS[target]}"


def exe_relpath(target):
    """Where the engine executable lives inside the package directory."""
    if target == MACOS:
        return f"{APP_NAME}/Contents/MacOS/{EXE_STEM}"
    if target == WINDOWS:
        return f"{EXE_STEM}.exe"
    return EXE_STEM


def tables_relpath(target):
    """The folder beside the executable that holds the .pinbundle files."""
    return f"{APP_NAME}/Contents/MacOS/tables" if target == MACOS else "tables"


def info_plist(version):
    """The macOS app's Info.plist (deterministic XML, sorted keys)."""
    return plistlib.dumps(
        {
            "CFBundleDevelopmentRegion": "en",
            "CFBundleDisplayName": "Kilohertz Pinball",
            "CFBundleExecutable": EXE_STEM,
            "CFBundleIdentifier": BUNDLE_ID,
            "CFBundleInfoDictionaryVersion": "6.0",
            "CFBundleName": "Kilohertz Pinball",
            "CFBundlePackageType": "APPL",
            "CFBundleShortVersionString": version,
            "CFBundleVersion": version,
            "LSMinimumSystemVersion": MACOS_MIN,
            "NSHighResolutionCapable": True,
        },
        fmt=plistlib.FMT_XML,
        sort_keys=True,
    )


def plan(target, version, dof_dir):
    """Sorted relative paths of the package directory before any ad-hoc
    signature. ``dof_dir`` (Windows) may be ``None`` when not yet built: the
    fixed bridge entries are listed then."""
    paths = [exe_relpath(target)]
    paths += [f"{tables_relpath(target)}/{name}.pinbundle" for name in TABLES]
    paths += list(DOC_FILES)
    paths += [d for d in OPTIONAL_DOC_FILES if (REPO / d).is_file()]
    if target == MACOS:
        paths.append(f"{APP_NAME}/Contents/Info.plist")
    if target == WINDOWS:
        bridge = {"dof_bridge/UNVERIFIED.txt", f"dof_bridge/{BRIDGE_EXE}"}
        if dof_dir is not None:
            bridge |= {f"dof_bridge/{rel}" for rel in _files(dof_dir)}
        paths += sorted(bridge)
    return sorted(set(paths))


def assemble(pkg, target, version, exe, bundles, dof_dir):
    """Creates the package directory ``pkg`` from the resolved inputs."""
    if pkg.exists():
        shutil.rmtree(pkg)
    pkg.mkdir(parents=True)
    exe_dest = pkg / exe_relpath(target)
    exe_dest.parent.mkdir(parents=True, exist_ok=True)
    shutil.copyfile(exe, exe_dest)
    if os.name != "nt":
        os.chmod(exe_dest, EXEC_MODE)
    tables = pkg / tables_relpath(target)
    tables.mkdir(parents=True, exist_ok=True)
    for name in TABLES:
        write_bundle(name, bundles / name, tables / f"{name}.pinbundle")
    for doc in DOC_FILES:
        src = REPO / doc
        if not src.is_file():
            raise PackagingError(f"{src}: required release file is missing")
        shutil.copyfile(src, pkg / doc)
    for doc in OPTIONAL_DOC_FILES:
        src = REPO / doc
        if src.is_file():
            (pkg / doc).parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(src, pkg / doc)
    if target == MACOS:
        (pkg / APP_NAME / "Contents" / "Info.plist").write_bytes(info_plist(version))
    if target == WINDOWS:
        bridge = pkg / "dof_bridge"
        bridge.mkdir()
        for rel in _files(dof_dir):
            dest = bridge / rel
            dest.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(dof_dir / rel, dest)
        (bridge / "UNVERIFIED.txt").write_bytes(UNVERIFIED_TEXT.encode("utf-8"))


def adhoc_codesign(app):
    """Ad-hoc ``codesign --force --deep -s -`` of the .app when the tool
    exists. Never a real identity; a failure is only a warning (A10)."""
    tool = shutil.which("codesign")
    if tool is None:
        warn("codesign not found; the .app is not ad-hoc signed")
        return
    try:
        subprocess.run([tool, "--force", "--deep", "-s", "-", str(app)], check=True)
    except (OSError, subprocess.CalledProcessError) as e:
        warn(f"{app}: ad-hoc codesign failed ({e}); continuing unsigned")


def manifest(pkg, version, target):
    """PACKAGE_MANIFEST.json text for the package directory."""
    files = [
        {"path": rel, "size": (pkg / rel).stat().st_size, "sha256": sha256_file(pkg / rel)}
        for rel in _files(pkg)
    ]
    doc = {
        "version": version,
        "target": target,
        "platform": PLATFORMS[target],
        "files": files,
    }
    return json.dumps(doc, indent=2) + "\n"


def write_archive(out, pkg, target):
    """Writes the release archive of ``pkg`` and its .sha256; returns both paths."""
    name = pkg.name
    exec_paths = {exe_relpath(target)}
    if target == LINUX:
        archive = out / f"{name}.tar.gz"
        write_tar_gz(archive, pkg, name, exec_paths)
    else:
        archive = out / f"{name}.zip"
        entries = []
        for rel in _files(pkg):
            mode = EXEC_MODE if rel in exec_paths else FILE_MODE
            entries.append((f"{name}/{rel}", (pkg / rel).read_bytes(), mode))
        write_zip(archive, entries)
    sums = out / f"{name}.sha256"
    sums.write_bytes(f"{sha256_file(archive)}  {archive.name}\n".encode("utf-8"))
    return archive, sums


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------


class _Parser(argparse.ArgumentParser):
    def error(self, message):
        self.print_usage(sys.stderr)
        self.exit(2, f"{self.prog}: error: {message}\n")


def parse_args(argv):
    """Parses the fixed CLI (A); argparse exits 2 on usage errors."""
    p = _Parser(
        prog="package.py",
        description="Build a Kilohertz Pinball release package for one target (PLAN 7.2). "
        "Never signs (except an ad-hoc macOS codesign), notarizes, uploads, tags or pushes.",
    )
    p.add_argument("--target", required=True, choices=TARGETS,
                   help="target triple; one of: " + ", ".join(TARGETS))
    p.add_argument("--out", required=True, type=Path, help="output directory")
    p.add_argument("--version", help="package version (default: [workspace.package] version in Cargo.toml)")
    p.add_argument("--exe", type=Path,
                   help="prebuilt engine executable (macOS: an already lipo'd universal binary); "
                   "default: build it with cargo")
    p.add_argument("--bundle-dir", type=Path,
                   help="directory with prebuilt bundle folders " + "/ ".join(TABLES)
                   + "/; default: run tables/<name>/build.sh")
    p.add_argument("--dof-bridge", type=Path,
                   help="prebuilt `dotnet publish` output holding KilohertzDofBridge.exe "
                   "(Windows target only); default on Windows: run dotnet publish")
    p.add_argument("--dry-run", action="store_true",
                   help="build the package, then delete the archive and .sha256; prints DRY RUN OK")
    p.add_argument("--list", action="store_true",
                   help="print the package's file paths one per line; write nothing")
    return p.parse_args(argv)


def resolve_version(args):
    """The package version (flag or Cargo.toml), validated for file names."""
    version = args.version if args.version is not None else workspace_version()
    if not VERSION_RE.match(version):
        raise UsageError(f"--version {version!r}: use letters, digits, '.', '+', '_' or '-'")
    return version


def do_list(args, version):
    """--list: validate the given inputs, print planned paths, write nothing."""
    if args.exe is not None:
        check_exe(args.exe)
    if args.bundle_dir is not None:
        for name in TABLES:
            bundle_entries(name, args.bundle_dir / name)
    dof = check_dof_dir(args.dof_bridge) if args.dof_bridge is not None else None
    for rel in plan(args.target, version, dof):
        print(rel)
    return 0


def do_package(args, version):
    """Builds or takes the inputs, assembles, archives and writes the manifest."""
    target = args.target
    # Prebuilt inputs are checked before anything is built.
    exe = check_exe(args.exe) if args.exe is not None else None
    if args.bundle_dir is not None:
        for name in TABLES:
            bundle_entries(name, args.bundle_dir / name)
    dof = check_dof_dir(args.dof_bridge) if args.dof_bridge is not None else None

    out = args.out
    out.mkdir(parents=True, exist_ok=True)
    pkg = out / package_name(version, target)
    base = pkg.name
    for stale in (out / f"{base}.zip", out / f"{base}.tar.gz", out / f"{base}.sha256", out / MANIFEST_NAME):
        if stale.is_file():
            stale.unlink()

    with tempfile.TemporaryDirectory(prefix="khz-package-") as tmp_name:
        tmp = Path(tmp_name)
        if exe is None:
            exe = build_exe(target, tmp)
        bundles = args.bundle_dir if args.bundle_dir is not None else build_bundles(tmp)
        if target == WINDOWS and dof is None:
            dof = check_dof_dir(build_dof_bridge(tmp))
        assemble(pkg, target, version, exe, bundles, dof)

    if target == MACOS and not args.dry_run:
        adhoc_codesign(pkg / APP_NAME)

    if target != WINDOWS:
        leaked = [rel for rel in _files(pkg) if "dof_bridge" in rel]
        if leaked:
            raise PackagingError(f"{pkg / leaked[0]}: DOF bridge path in a non-Windows package")

    (out / MANIFEST_NAME).write_bytes(manifest(pkg, version, target).encode("utf-8"))
    archive, sums = write_archive(out, pkg, target)
    print(f"package: {pkg}")
    print(f"manifest: {out / MANIFEST_NAME}")
    if args.dry_run:
        archive.unlink()
        sums.unlink()
        print("DRY RUN OK")
    else:
        print(f"archive: {archive}")
        print(f"sha256: {sums}")
    return 0


def main(argv=None):
    """Entry point; returns the exit code."""
    args = parse_args(sys.argv[1:] if argv is None else argv)
    try:
        if args.dof_bridge is not None and args.target != WINDOWS:
            raise UsageError(f"--dof-bridge {args.dof_bridge}: only valid with --target {WINDOWS}")
        version = resolve_version(args)
        if args.list:
            return do_list(args, version)
        return do_package(args, version)
    except UsageError as e:
        print(f"package.py: error: {e}", file=sys.stderr)
        return 2
    except PackagingError as e:
        print(f"package.py: error: {e}", file=sys.stderr)
        return 1
    except OSError as e:
        name = e.filename if e.filename is not None else "?"
        print(f"package.py: error: {name}: {e.strerror or e}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    sys.exit(main())
