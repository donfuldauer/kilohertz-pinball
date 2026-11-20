#!/usr/bin/env python3
"""Check that every requirement ID in docs/SPEC.md is traced by the plan (and optionally by tests).

The plan is the set of GitHub issues labelled `work-unit` (docs/PROCESS.md P1). This script reads
their bodies with the GitHub search API through the `gh` CLI
(`gh api -X GET search/issues`, label `work-unit`, explicit paging), or a local draft file with
`--plan-file`. Requirement IDs have the form `REQ-<AREA>-<NNN>` (for example `REQ-LAT-004`); a
line of SPEC that defines an ID and contains `WITHDRAWN` retires that ID.

Usage (from the repository root):
  python3 -I scripts/req_coverage.py                          # SPEC vs work-unit issues
  python3 -I scripts/req_coverage.py --milestone 79           # only sub-issues of tracking issue #79
  python3 -I scripts/req_coverage.py --milestone "M1 — Play experience"   # by the body's Milestone field
  python3 -I scripts/req_coverage.py --allow REQ-VIS-005      # IDs deferred by a recorded decision
  python3 -I scripts/req_coverage.py --tests                  # also require each ID in test code
  python3 -I scripts/req_coverage.py --milestone 79 --tests crates tests scripts/tests
  python3 -I scripts/req_coverage.py --plan-file draft.md     # check a local draft instead of issues

`--milestone` takes a tracking issue (`79` or `#79`), matched against each issue's parent issue
(or, when the API gives none, the "tracking issue #N" in its Plan ID line), or a milestone title,
matched against the **Milestone:** field of the Plan ID line. A milestone covers a subset of
SPEC, so only the IDs it traces are reported (and, with `--tests`, checked in tests). With
`--plan-file`, `--milestone` only labels the output: every SPEC ID is still required in the
draft and, with `--tests`, in tests.

What `--tests` counts (only test code, never production source); with no roots it scans
`crates`, `tests` and `scripts/tests`:
  a. a file with a path component named tests, test, spec, e2e or __tests__, or whose name
     matches *_test.*, *.test.*, *.spec.*, test_*.py or *Tests.cs, counts in full;
  b. any other .rs file counts only its inline test modules: from each `#[cfg(test)]` line to
     the closing brace of the annotated item (text after it on that line is excluded; brace depth from the first `{` back to zero; an attribute
     with no following `{` is ignored). The scan is lexically aware: braces and semicolons
     inside comments, strings, raw strings and char literals are ignored, and an attribute
     inside a comment or string is not an attribute;
  c. any other file does not count.
Files are visited in sorted order; missing roots are skipped silently.

Exit status: 0 when every ID is traced, 1 when an ID is missing (or a plan names an ID that is
not in SPEC), 2 on a usage, input or GitHub error. Standard library plus the `gh` CLI.
"""
import argparse
import fnmatch
import json
import pathlib
import re
import subprocess
import sys

REQ = re.compile(r"\bREQ-[A-Z0-9]+-\d{3}\b")
REPO = "donfuldauer/kilohertz-pinball"
PER_PAGE = 100
SEARCH_CAP = 1000  # the search API returns at most this many results per query
TEST_SUFFIXES = {".rs", ".cs", ".c", ".h", ".cpp", ".py", ".sh", ".lua", ".wat",
                 ".json", ".toml", ".yaml", ".yml"}
TEST_DIRS = {"tests", "test", "spec", "e2e", "__tests__"}
TEST_NAMES = ("*_test.*", "*.test.*", "*.spec.*", "test_*.py", "*Tests.cs")
DEFAULT_TEST_ROOTS = ["crates", "tests", "scripts/tests"]
CFG_TEST = re.compile(r"^\s*#\[cfg\(test\)\]")
# The Plan ID line: `> **Plan ID:** KP-001 · **Phase:** … · **Milestone:** <title> (tracking issue #79) · …`
MILESTONE_FIELD = re.compile(r"\*\*Milestone:\*\*\s*([^·\n]*)")
TRACKING_REF = re.compile(r"\(tracking issue #(\d+)\)")


class InputError(Exception):
    """A usage, input or GitHub error: reported on stderr with exit status 2."""


def read_text(path: pathlib.Path) -> str:
    try:
        return path.read_text(encoding="utf-8", errors="replace")
    except OSError as e:
        raise InputError(f"cannot read {path}: {e.strerror or e}") from e


def ids_in_text(text: str) -> set:
    return set(REQ.findall(text))


def ids_in(path: pathlib.Path) -> set:
    return ids_in_text(read_text(path))


def withdrawn_in(text: str) -> set:
    """IDs whose defining line is marked WITHDRAWN; they need no tracing."""
    out = set()
    for line in text.splitlines():
        ids = REQ.findall(line)
        if "WITHDRAWN" in line and ids:
            out.add(ids[0])  # the ID the line defines; later IDs are cross-references
    return out


def milestone_matches(issue: dict, milestone: str) -> bool:
    """True if `issue` belongs to `milestone` (a tracking issue number or a milestone title)."""
    body = issue.get("body") or ""
    wanted = milestone.strip()
    number = wanted[1:] if wanted.startswith("#") else wanted
    if number.isdigit():
        parent = issue.get("parent_issue_url") or ""
        if parent:
            return parent.rstrip("/").rsplit("/", 1)[-1] == number
        m = TRACKING_REF.search(body)
        return bool(m) and m.group(1) == number
    m = MILESTONE_FIELD.search(body)
    if not m:
        return False
    title = TRACKING_REF.sub("", m.group(1)).strip()
    return title == wanted


def search_page(repo: str, page: int) -> dict:
    """One page of `work-unit` issues (open and closed) from the GitHub search API."""
    query = f"repo:{repo} is:issue label:work-unit"
    cmd = ["gh", "api", "-X", "GET", "search/issues", "-f", f"q={query}",
           "-F", f"per_page={PER_PAGE}", "-F", f"page={page}"]
    try:
        proc = subprocess.run(cmd, check=False, capture_output=True, text=True)
    except FileNotFoundError as e:
        raise InputError("the `gh` CLI is not installed (or use --plan-file)") from e
    if proc.returncode != 0:
        detail = (proc.stderr or proc.stdout).strip().splitlines()
        raise InputError("gh api search/issues failed: " + (detail[-1] if detail else f"exit {proc.returncode}"))
    try:
        data = json.loads(proc.stdout)
    except json.JSONDecodeError as e:
        raise InputError(f"gh api search/issues returned invalid JSON: {e}") from e
    if not isinstance(data, dict) or not isinstance(data.get("items"), list):
        raise InputError("gh api search/issues returned an unexpected response")
    if data.get("incomplete_results"):
        raise InputError("the search API returned incomplete results; run again")
    return data


def ids_in_issues(repo: str, milestone) -> tuple:
    """REQ IDs in the bodies of all work-unit issues (open and closed), and the issue count."""
    found, count, page, seen = set(), 0, 1, set()
    while True:
        # Explicit pages: `gh api --paginate` follows numeric-ID links that some proxies block.
        data = search_page(repo, page)
        items = data["items"]
        total = data.get("total_count", 0)
        if isinstance(total, int) and total > SEARCH_CAP:
            raise InputError(f"{total} work-unit issues exceed the search API cap of {SEARCH_CAP}")
        for issue in items:
            if "pull_request" in issue or issue.get("number") in seen:
                continue
            seen.add(issue.get("number"))
            if milestone and not milestone_matches(issue, milestone):
                continue
            count += 1
            found |= ids_in_text(issue.get("body") or "")
        if len(items) < PER_PAGE:
            break
        page += 1
    return found, count


def is_test_path(p: pathlib.Path) -> bool:
    """Rule (a): a path component or file name that marks the file as test code."""
    if any(part in TEST_DIRS for part in p.parts[:-1]):
        return True
    return any(fnmatch.fnmatchcase(p.name, pat) for pat in TEST_NAMES)


def code_mask(text: str) -> list:
    """Per character: True where `text` is Rust code, False inside comments, strings or chars.

    One forward scan. Skipped regions: `//` comments, nested `/* */` comments, `"..."` and
    `b"..."` strings (backslash escapes), raw strings `r#"..."#` (any number of `#`, with or
    without a `b` prefix), and char/byte-char literals. A `'` that does not open a char literal
    is a lifetime or label and stays code.
    """
    n = len(text)
    mask = [True] * n
    ident = lambda c: c.isalnum() or c == "_"  # noqa: E731
    i = 0
    while i < n:
        c = text[i]
        end = None  # exclusive end of a skipped region starting at i
        if c == "/" and text.startswith("//", i):
            end = text.find("\n", i)
            end = n if end < 0 else end
        elif c == "/" and text.startswith("/*", i):
            depth, k = 1, i + 2
            while k < n and depth:
                if text.startswith("/*", k):
                    depth, k = depth + 1, k + 2
                elif text.startswith("*/", k):
                    depth, k = depth - 1, k + 2
                else:
                    k += 1
            end = k
        elif c == '"':
            k = i + 1
            while k < n and text[k] != '"':
                k += 2 if text[k] == "\\" else 1
            end = min(k + 1, n)
        elif c in "rb" and (i == 0 or not ident(text[i - 1])):
            k = i + 1
            if c == "b" and k < n and text[k] == "r":
                k += 1
            if text[k - 1] == "r":  # `r` or `br`: raw string if `#`* then `"`
                h = k
                while h < n and text[h] == "#":
                    h += 1
                if h < n and text[h] == '"':
                    close = '"' + "#" * (h - k)
                    e = text.find(close, h + 1)
                    end = n if e < 0 else e + len(close)
        elif c == "'":
            if i + 1 < n and text[i + 1] == "\\":
                k = i + 3  # skip the escaped character, then find the closing quote
                while k < n and text[k] != "'" and text[k] != "\n":
                    k += 1
                end = min(k + 1, n)
            elif i + 2 < n and text[i + 2] == "'" and text[i + 1] != "'":
                end = i + 3
        if end is None:
            i += 1
        else:
            for k in range(i, end):
                mask[k] = False
            i = end
    return mask


def cfg_test_text(text: str) -> str:
    """Rule (b): the text of inline `#[cfg(test)]` items, attribute to closing brace.

    The annotated item ends at the first `;` before any `{` (a brace-less item contributes
    nothing), else where brace depth returns to zero. Braces and semicolons inside comments,
    strings and char literals are ignored (see `code_mask`), and an attribute inside a comment
    or string is not an attribute. The returned text is the full source span, comments included.
    """
    lines = text.split("\n")
    offs, pos = [], 0
    for ln in lines:
        offs.append(pos)
        pos += len(ln) + 1
    mask = code_mask(text)
    out = []
    i = 0
    while i < len(lines):
        m = CFG_TEST.match(lines[i])
        if not m or not mask[offs[i] + m.end() - 1]:
            i += 1
            continue
        start, depth, opened, ended, j = i, 0, False, False, i
        attr_col = m.end() - len(m.group(0).lstrip())  # the attribute's column on its line
        end_col = 0
        col = m.end()  # scan after the attribute itself; later lines scan from column 0
        while j < len(lines) and not ended:
            for k in range(col, len(lines[j])):
                if not mask[offs[j] + k]:
                    continue
                ch = lines[j][k]
                if ch == "{":
                    depth += 1
                    opened = True
                elif ch == "}":
                    depth -= 1
                    if opened and depth <= 0:
                        ended, end_col = True, k + 1
                        break
                elif ch == ";" and not opened:
                    ended = True  # brace-less item: ignore the attribute
                    break
            if ended:
                break
            col = 0
            j += 1
        if ended and opened:
            item = lines[start:j + 1]
            item[-1] = item[-1][:end_col]  # nothing after the closing brace
            item[0] = item[0][attr_col:]  # nothing before the attribute
            out.extend(item)
        i = start + 1 if not (ended and opened) else j + 1
    return "\n".join(out)


def ids_in_test_code(p: pathlib.Path) -> set:
    """REQ IDs in `p` that count as test coverage under the module-docstring rules."""
    if p.suffix in TEST_SUFFIXES and is_test_path(p):
        return ids_in(p)
    if p.suffix == ".rs":
        return ids_in_text(cfg_test_text(read_text(p)))
    return set()


def ids_in_tree(roots) -> set:
    found = set()
    for root in roots:
        root = pathlib.Path(root)
        if root.is_file():
            found |= ids_in_test_code(root)
            continue
        if not root.is_dir():
            continue
        for p in sorted(root.rglob("*")):
            if p.is_file() and "target" not in p.parts:
                found |= ids_in_test_code(p)
    return found


def run(args) -> int:
    spec = pathlib.Path(args.spec)
    if not spec.is_file():
        raise InputError(f"{spec} not found (run from the repository root, or pass --spec)")
    spec_text = read_text(spec)
    defined = ids_in_text(spec_text)
    withdrawn = withdrawn_in(spec_text)
    unknown_allowed = sorted(set(args.allow) - defined)
    if unknown_allowed:
        raise InputError("--allow names IDs that are not in the spec: " + ", ".join(unknown_allowed))
    plan = pathlib.Path(args.plan_file) if args.plan_file else None
    if plan is not None and not plan.is_file():
        raise InputError(f"{plan} not found")
    required = defined - set(args.allow) - withdrawn
    print(f"{len(required)} requirement IDs in {spec} "
          f"({len(args.allow)} allowed as deferred, {len(withdrawn)} withdrawn)")

    failed = False
    if plan is not None:
        planned, source = ids_in(plan), str(plan)
    else:
        planned, n_issues = ids_in_issues(args.repo, args.milestone)
        source = f"work-unit issues ({n_issues})"
    # Unknown IDs are an error in every mode, including a milestone-scoped run.
    unknown = sorted(planned - defined)
    scoped = bool(args.milestone) and plan is None  # a local draft always gets full validation
    if scoped:
        # A milestone covers a subset; only report what it does trace.
        print(f"{source} in {args.milestone!r}: {len(planned & required)} IDs traced")
    else:
        missing = sorted(required - planned)
        tag = f" (milestone {args.milestone!r})" if args.milestone else ""
        print(f"{source}{tag}: {len(required) - len(missing)}/{len(required)} traced")
        for rid in missing:
            print(f"  MISSING in plan: {rid}")
        failed |= bool(missing)
    for rid in unknown:
        print(f"  UNKNOWN in plan (not in {spec}): {rid}")
    failed |= bool(unknown)

    if args.tests is not None:
        tested = ids_in_tree(args.tests or DEFAULT_TEST_ROOTS)
        # With issues and --milestone, compare only the IDs that milestone traces.
        scope = (planned & required) if scoped else required
        label = f"tests (milestone {args.milestone!r})" if args.milestone else "tests"
        missing = sorted(scope - tested)
        print(f"{label}: {len(scope) - len(missing)}/{len(scope)} referenced")
        for rid in missing:
            print(f"  MISSING in tests: {rid}")
        failed |= bool(missing)

    return 1 if failed else 0


def main(argv=None) -> int:
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--spec", default="docs/SPEC.md", help="the specification (default: docs/SPEC.md)")
    ap.add_argument("--repo", default=REPO, help=f"GitHub repository (default: {REPO})")
    ap.add_argument("--milestone", default=None,
                    help="only issues of this tracking issue (79 or #79) or milestone title")
    ap.add_argument("--plan-file", default=None, help="check a local draft file instead of issues")
    ap.add_argument("--allow", nargs="*", default=[], help="IDs deliberately deferred by a recorded decision")
    ap.add_argument("--tests", nargs="*", default=None,
                    help="test roots that must mention every ID (default roots: crates tests scripts/tests)")
    args = ap.parse_args(argv)
    try:
        return run(args)
    except InputError as e:
        sys.stdout.flush()
        print(f"error: {e}", file=sys.stderr)
        return 2


if __name__ == "__main__":
    sys.exit(main())
