#!/usr/bin/env python3
"""Renders docs/HUMAN_ACCEPTANCE.md, the Phase 8 checklist (PLAN 7.2).

Usage::

    python3 scripts/human_acceptance.py [--repo <root>] [--out <file>] [--check] [--commit <sha>]

Inputs (all optional; a missing one is reported as "not present"):
``docs/RUN_LOG.md``, every ``docs/queue/*.md`` and ``docs/decisions/*.md``
except their README.md, and the frozen ``docs/HUMAN_QUEUE.md`` and
``docs/DECISIONS.md``. Files are read as UTF-8 (CRLF tolerated) and processed
in sorted path order.

Output sections, in order:

1. Review the run: input presence, counts and pointers.
2. Blocked and open items: open queue lines (``- [ ] [id] text`` in
   docs/queue, plus the old ``- [id] text`` style that is not struck through),
   grouped by file, and run-log rows whose Status column says blocked.
3. Logged decisions: every ``## <id> — <topic>`` heading.
4. D33 flags: the fixed release flags plus every input line naming D33.
5. Gate reports: every run-log line naming Gate A1/A2 or a Wave N gate, verbatim.
6. Phase 8 checklist.

The output is a pure function of the input files (no date, host or commit
unless ``--commit`` is given), so ``--check`` is stable on CI: it renders in
memory and exits 1 with a diff head on stderr when the file differs or is
missing, 0 when it matches. Without ``--check`` the file is written.
Python 3 standard library only.
"""

import argparse
import difflib
import re
import sys
from pathlib import Path

DEFAULT_REPO = Path(__file__).resolve().parent.parent

RUN_LOG = "docs/RUN_LOG.md"
QUEUE_DIR = "docs/queue"
DECISIONS_DIR = "docs/decisions"
FROZEN_QUEUE = "docs/HUMAN_QUEUE.md"
FROZEN_DECISIONS = "docs/DECISIONS.md"

CHECKBOX_RE = re.compile(r"^- \[( |x|X)\] \[([^\]]+)\] (\S.*)$")
OLD_STYLE_RE = re.compile(r"^- \[([^\]]+)\] (\S.*)$")
STRUCK_RE = re.compile(r"^- ~~")
DECISION_RE = re.compile(r"^## (\S.*?) — (\S.*)$")
D33_RE = re.compile(r"(?<![A-Za-z0-9])D33(?![0-9])")
GATE_RE = re.compile(r"\b[Gg]ate A[0-9]+\b|\b[Ww]ave [0-9]+ gate\b")
SEPARATOR_RE = re.compile(r"^\|(\s*:?-+:?\s*\|)+\s*$")

TABLES = ("greybox", "apex_circuit", "sky_deck", "feature_lab")

def read_lines(path):
    """Lines of a UTF-8 file with CRLF/CR normalised and trailing blanks
    stripped; ``None`` when the file does not exist."""
    if not path.is_file():
        return None
    text = path.read_bytes().decode("utf-8", errors="replace")
    if text.startswith("﻿"):
        text = text[1:]
    text = text.replace("\r\n", "\n").replace("\r", "\n")
    return [line.rstrip() for line in text.split("\n")]


def md_files(repo, rel_dir):
    """Sorted relative paths of ``<rel_dir>/*.md`` except README.md; ``None``
    when the folder does not exist."""
    d = repo / rel_dir
    if not d.is_dir():
        return None
    names = sorted(p.name for p in d.iterdir() if p.is_file() and p.suffix == ".md" and p.name != "README.md")
    return [f"{rel_dir}/{n}" for n in names]


class Inputs:
    """Every input file, read once, keyed by relative path."""

    def __init__(self, repo):
        self.repo = repo
        self.queue_files = md_files(repo, QUEUE_DIR)
        self.decision_files = md_files(repo, DECISIONS_DIR)
        self.lines = {}
        for rel in [RUN_LOG, FROZEN_QUEUE, FROZEN_DECISIONS] + (self.queue_files or []) + (self.decision_files or []):
            lines = read_lines(repo / rel)
            if lines is not None:
                self.lines[rel] = lines

    def present(self, rel):
        """Whether an input file exists."""
        return rel in self.lines

    def queue_sources(self):
        """Queue inputs in sorted path order (frozen file included)."""
        rels = list(self.queue_files or []) + ([FROZEN_QUEUE] if self.present(FROZEN_QUEUE) else [])
        return sorted(r for r in rels if self.present(r))

    def decision_sources(self):
        """Decision inputs in sorted path order (frozen file included)."""
        rels = list(self.decision_files or []) + ([FROZEN_DECISIONS] if self.present(FROZEN_DECISIONS) else [])
        return sorted(r for r in rels if self.present(r))

    def all_sources(self):
        """Every present input, sorted by path."""
        return sorted(self.lines)


def classify_queue_line(line):
    """``("open", id, text)``, ``("resolved", id, text)`` or ``None``."""
    m = CHECKBOX_RE.match(line)
    if m:
        state = "open" if m.group(1) == " " else "resolved"
        return state, m.group(2), m.group(3)
    if STRUCK_RE.match(line):
        return "resolved", None, line[2:]
    m = OLD_STYLE_RE.match(line)
    if m and m.group(1) not in (" ", "x", "X") and not m.group(2).startswith("~~"):
        return "open", m.group(1), m.group(2)
    if m and m.group(2).startswith("~~"):
        return "resolved", m.group(1), m.group(2)
    return None


def queue_items(inputs):
    """``(open, resolved_count)``: open items as ``(file, id, text)`` in file order."""
    open_items = []
    resolved = 0
    for rel in inputs.queue_sources():
        for line in inputs.lines[rel]:
            c = classify_queue_line(line)
            if c is None:
                continue
            if c[0] == "open":
                open_items.append((rel, c[1], c[2]))
            else:
                resolved += 1
    return open_items, resolved


def decisions(inputs):
    """Every ``## <id> — <topic>`` heading as ``(file, id, topic)``."""
    out = []
    for rel in inputs.decision_sources():
        for line in inputs.lines[rel]:
            m = DECISION_RE.match(line)
            if m:
                out.append((rel, m.group(1), m.group(2)))
    return out


def run_log_rows(lines):
    """Data rows of every table in the run log as ``(line, status_cell)``;
    header and separator rows are skipped. The status cell comes from the
    column named ``Status`` in the latest header row (rows after a paragraph
    keep it; ``None`` before any header or when the header has none)."""
    rows = []
    status_col = None
    for i, line in enumerate(lines):
        if not line.startswith("|"):
            continue
        if SEPARATOR_RE.match(line):
            continue
        nxt = lines[i + 1] if i + 1 < len(lines) else ""
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if SEPARATOR_RE.match(nxt):
            lowered = [c.lower() for c in cells]
            status_col = lowered.index("status") if "status" in lowered else None
            continue
        status = cells[status_col] if status_col is not None and status_col < len(cells) else None
        rows.append((line, status))
    return rows


def render(repo, commit=None):
    """The whole document as a string (LF, final newline)."""
    inputs = Inputs(repo)
    open_items, resolved = queue_items(inputs)
    decision_list = decisions(inputs)
    run_log = inputs.lines.get(RUN_LOG)
    rows = run_log_rows(run_log) if run_log is not None else []
    blocked = [line for line, status in rows if status is not None and "blocked" in status.lower()]

    out = ["# Human Acceptance", ""]
    out.append(
        "> Generated by `scripts/human_acceptance.py` from `docs/RUN_LOG.md`, `docs/queue/`, "
        "`docs/decisions/`, `docs/HUMAN_QUEUE.md` and `docs/DECISIONS.md`. Do not edit this file "
        "by hand; rerun the script. This is the Phase 8 acceptance checklist (PLAN.md Phase 8)."
    )
    if commit:
        out += ["", f"Generated from {commit}"]
    out.append("")

    # 1. Review the run
    out += ["## 1. Review the run", "", "Inputs:", ""]
    for rel, label in ((RUN_LOG, ""), (FROZEN_QUEUE, " (frozen)"), (FROZEN_DECISIONS, " (frozen)")):
        out.append(f"- `{rel}`{label}: {'present' if inputs.present(rel) else 'not present'}")
    for rel_dir, files in ((QUEUE_DIR, inputs.queue_files), (DECISIONS_DIR, inputs.decision_files)):
        state = "not present" if files is None else f"{len(files)} files"
        out.append(f"- `{rel_dir}/`: {state}")
    out += [
        "",
        "Counts:",
        "",
        f"- Open queue lines: {len(open_items)}",
        f"- Resolved queue lines: {resolved}",
        f"- Decision entries: {len(decision_list)}",
        f"- Run-log rows: {len(rows)}",
        "",
        "Open items for a person live in one file per unit under `docs/queue/` (older ones in the "
        "frozen `docs/HUMAN_QUEUE.md`); every choice an agent made is under `docs/decisions/` (older "
        "ones in the frozen `docs/DECISIONS.md`). The run history is `docs/RUN_LOG.md`.",
        "",
    ]

    # 2. Blocked and open items
    out += ["## 2. Blocked and open items", "", "### Open queue lines", ""]
    if not open_items:
        out += ["None.", ""]
    current = None
    for rel, item_id, text in open_items:
        if rel != current:
            if current is not None:
                out.append("")
            out += [f"#### {rel}", ""]
            current = rel
        out.append(f"- [ ] [{item_id}] {text} ({rel})")
    if open_items:
        out.append("")
    out += ["### Blocked run-log rows", ""]
    if run_log is None:
        out += [f"`{RUN_LOG}` not present.", ""]
    elif not blocked:
        out += ["None.", ""]
    for line in blocked:
        out += [line, ""]

    # 3. Logged decisions
    out += ["## 3. Logged decisions", ""]
    if not decision_list:
        out += ["None.", ""]
    else:
        out += [f"- {d_id}: {topic} ({rel})" for rel, d_id, topic in decision_list] + [""]

    # 4. D33 flags
    out += [
        "## 4. D33 flags",
        "",
        "- Ray-query reflections ship off by default unless validated by a ray-tracing runner.",
        "- The DOF bridge (Windows package only) ships marked \"unverified on hardware\" "
        "(`dof_bridge/UNVERIFIED.txt`).",
        "- Release artifacts are unsigned (no Authenticode, no macOS Developer ID or notarization; "
        "macOS gets an ad-hoc signature only).",
        "",
        "Mentions of D33 in the run log, queue and decisions:",
        "",
    ]
    mentions = [
        (rel, line.strip())
        for rel in inputs.all_sources()
        for line in inputs.lines[rel]
        if D33_RE.search(line)
    ]
    if not mentions:
        out += ["None.", ""]
    else:
        out += [f"- {rel}: {text}" for rel, text in mentions] + [""]

    # 5. Gate reports
    out += ["## 5. Gate reports", ""]
    if run_log is None:
        out += [f"`{RUN_LOG}` not present.", ""]
    else:
        gates = [line for line in run_log if GATE_RE.search(line)]
        if not gates:
            out += ["None.", ""]
        for line in gates:
            out += [line, ""]

    # 6. Phase 8 checklist
    out += ["## 6. Phase 8 checklist", ""]
    out += [
        "- [ ] **1. Review the run.** Read sections 1 to 5, every open line in `docs/queue/` and the "
        "decisions in `docs/decisions/`; rule on each open item.",
        "- [ ] **2. Play each table.** Check flipper feel, ball speed, catches and nudge. Tune only data "
        "(`physics_defaults.toml` or the table TOML), rerun CI, and confirm Gates A1 and A2 revalidate.",
    ]
    for name in TABLES:
        out.append(f"  - [ ] {name}: flipper feel, ball speed, catches, nudge")
    out += [
        "  - [ ] Any tuning is data-only (`physics_defaults.toml` / table TOML); CI rerun; Gates A1 and A2 revalidate",
        "- [ ] **3. Check hardware.**",
        "  - [ ] X-Arcade in each mode",
        "  - [ ] Analog plunger",
        "  - [ ] Accelerometer nudge",
        "  - [ ] Rumble",
        "  - [ ] SSF channels",
        "  - [ ] Multi-monitor layout",
        "  - [ ] DOF devices (the bridge ships unverified on hardware, D33)",
        "- [ ] **4. Check visuals.**",
        "  - [ ] Reflections, bloom, DMD and backglass on a real display",
        "  - [ ] Ray-query reflections on a capable GPU, then decide whether to enable them by default (D33)",
        "- [ ] **5. Replace placeholders.** Swap in real sounds, music, callouts and art by asset ID.",
    ]
    return "\n".join(out).rstrip("\n") + "\n"


def parse_args(argv):
    """Parses the fixed CLI (B)."""
    p = argparse.ArgumentParser(
        prog="human_acceptance.py",
        description="Render docs/HUMAN_ACCEPTANCE.md (the Phase 8 checklist) from the run log, "
        "queue and decision files. The output is deterministic.",
    )
    p.add_argument("--repo", type=Path, default=DEFAULT_REPO,
                   help="repository root (default: the parent of scripts/)")
    p.add_argument("--out", type=Path, help="output file (default: <repo>/docs/HUMAN_ACCEPTANCE.md)")
    p.add_argument("--check", action="store_true",
                   help="compare the rendering with --out; exit 1 with a diff head when it differs or is missing")
    p.add_argument("--commit", help="add a 'Generated from <sha>' line (default: no commit id)")
    return p.parse_args(argv)


def main(argv=None):
    """Entry point; returns the exit code."""
    args = parse_args(sys.argv[1:] if argv is None else argv)
    if args.commit is not None and (not args.commit.strip() or any(c in args.commit for c in "\r\n")):
        print("human_acceptance.py: error: --commit must be one non-empty line", file=sys.stderr)
        return 2
    repo = args.repo
    if not repo.is_dir():
        print(f"human_acceptance.py: error: {repo}: repository root not found", file=sys.stderr)
        return 2
    out = args.out if args.out is not None else repo / "docs" / "HUMAN_ACCEPTANCE.md"
    text = render(repo, args.commit)
    if args.check:
        if not out.is_file():
            print(f"human_acceptance.py: {out}: missing; run python3 scripts/human_acceptance.py", file=sys.stderr)
            return 1
        current = out.read_bytes().decode("utf-8", errors="replace")
        if current == text:
            print(f"{out}: up to date")
            return 0
        diff = list(difflib.unified_diff(
            current.splitlines(), text.splitlines(), "committed", "rendered", lineterm="", n=1))
        print(f"human_acceptance.py: {out}: stale; run python3 scripts/human_acceptance.py", file=sys.stderr)
        for line in diff[:40]:
            print(line, file=sys.stderr)
        if len(diff) > 40:
            print(f"... ({len(diff) - 40} more diff lines)", file=sys.stderr)
        return 1
    out.parent.mkdir(parents=True, exist_ok=True)
    with open(out, "w", encoding="utf-8", newline="\n") as f:
        f.write(text)
    print(f"wrote {out}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
