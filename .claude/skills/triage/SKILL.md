---
name: triage
description: "The Orchestrator's inline fast path for classifying obviously small Kilohertz Pinball work items (T0/T1 fixes, CodeRabbit nits, docs and run-log edits, CI tweaks) under docs/PROCESS.md P4, and the procedure for batching one review round or push (P5). Do NOT use it for issue implementations, Independent Tester tasks, investigations, gate failures, or anything that fails the eligibility checks; those go to the work-classifier."
---

# triage (PROCESS.md P4 "Fast path", P5 "Batching")

The Orchestrator runs this skill itself to classify small items without spawning a
work-classifier. It is for the cheap classification step only; the work is still done by a
task-executor and checked by the work-verifier. The fast path never routes T2 or T3.

## 1. Eligibility (ALL must hold)

- The item is a fix, a CodeRabbit finding, a docs or run-log edit or a CI tweak (never an issue
  implementation, an Independent Tester task, an investigation, or a gate failure).
- The tier is clearly T0 (Haiku 5.5 / Flash-Lite) or T1 (Sonnet 5.5 / Flash).
- It touches one module or docs only.
- No `pin_core` contract change and no change to another contract (PROCESS.md P3).
- No change to simulation output (`pin_physics` world/ball/flipper numerics, the sim tick, the
  triple buffer, the physics defaults, replay digests).
- No security boundary (R11: Lua/Wasm sandbox, budgets, memory caps, zip extraction, vendored
  Lua patches).
- Validity and cause are clear (otherwise issue-investigator first).
- The fix is spelled out by the comment or obvious.

If the validity or cause is unclear, the item goes to the issue-investigator first and is then
handled by the investigator's verdict: `valid-defect` is triaged again from its report (fast
path if it now passes every check, else the work-classifier); `valid-but-deferred` or
`pre-existing` is "Deferred" (PROCESS.md P6 "Gate reviews" step 4): open the investigator's new `work-unit`
issue (attached to its tracking issue) or commit its `docs/DECISIONS.md` entry, reply with the
link and resolve, with no Fixer, unless it is an unfixed security issue or a verified unfixed
correctness defect (P6 "Gate reviews" step 4 exceptions): then the thread stays unresolved and the item goes
to the work-classifier for a fix; `not-a-defect` is "Not a defect": reply with its evidence
and resolve per P6 "Gate reviews" step 4, with no Fixer. Any other failing check, or an unsure Orchestrator,
sends the item to the work-classifier.

## 2. Batching procedure (one gate-review round, or one push)

1. List every new finding (inline, nitpick, outside-diff and summary items).
2. Triage each one as fast-path T0, fast-path T1, investigator, or classifier.
3. Send findings whose validity or cause is unclear to the issue-investigator first (in
   parallel when independent). Map by verdict as in §1: `valid-defect` is triaged again from its
   report; `valid-but-deferred` or `pre-existing` is "Deferred" (new issue or DECISIONS.md entry,
   reply with the link, resolve; but an unfixed security issue or verified unfixed correctness
   defect stays unresolved and goes to the classifier for a fix); `not-a-defect` is "Not a
   defect" (reply with its evidence, resolve). Neither of the last two gets a Fixer.
4. Before the classifier call, move any fast-path item that touches a file also touched by a
   classifier-bound item into the classifier batch, so the classifier groups same-file items
   into one brief at the higher tier. Send all classifier-bound items to ONE work-classifier
   call as a batch.
5. Group the remaining fast-path items by tier and module directory: one Fixer brief per
   (tier, module) group, with all docs-only items (`docs/`, `.claude/`, `.agents/agents/`,
   `.agents/skills/`, `AGENTS.md`, `CLAUDE.md`, `GEMINI.md`, `README.md`) counting as one docs
   group (R1). Executable setup files (`.agents/setup-env.sh`, `.agents/environment.json`) are
   **not** docs-only and keep their code verification and regression-test requirements. When items
   of different tiers touch the same file, put them in one brief at the higher tier.
6. Spawn executors, in parallel when owned paths do not overlap.
7. Run ONE work-verifier on the resulting head commit covering every item in the round whenever
   the round changed the head (executor fixes, or a commit such as a `docs/DECISIONS.md` entry
   for a deferred finding; a round that only appends to the shared files uses the verifier's
   log-only case). Skip it only when the head is unchanged (for example every finding was
   answered as not a defect): the previous verifier pass on that head still satisfies P6 routine step 3,
   and the replies carry the investigator's evidence.
8. Reply to and resolve threads per PROCESS.md P6 "Gate reviews" step 4.

## 3. Fixer brief template

A valid subset of `docs/BRIEF_TEMPLATE.md`; field names match it. Fill it in and pass it to the
task-executor with the tier's model override.

```text
WORK ITEM <#issue / PR finding id>: <title>          AGENT: task-executor
ROLE: Fixer
MODEL: <Haiku 5.5 | Sonnet 5.5 | Flash-Lite | Flash>   TIER: T0 | T1
BRANCH: <branch>   PR: <number>
ISSUE: #<n>   PLAN ID: KP-<NNN>   REQUIREMENTS: <SPEC sections / REQ IDs touched, or none>

OWNED PATHS (you may modify only these):
  <every file the findings touch, plus the regression test file for each code finding, nothing else>

GOAL
  <one paragraph: the findings in this brief are fixed with the smallest change>

REQUIREMENTS
  Per finding, one numbered item:
  N. Quoted comment: "<comment text>"
     Thread URL: <url>
     File:lines: <path>:<a>-<b>
     Fixed means: <exact observable condition>
  For each finding, report the thread URL and what changed. Do not reply to or resolve
  threads; the Orchestrator does (PROCESS.md P6 "Gate reviews" step 4).

TESTS REQUIRED
  A regression test for each finding that changes code, in the test path listed under OWNED
  PATHS; for docs/config-only findings write "none (docs only)". Also list the existing suites
  that must still pass.

RULES
  <PROCESS.md P2, R1–R12, inlined in full>

COMMANDS
  <the "Agent environment" block from docs/DEVELOPMENT.md (CARGO_BUILD_JOBS=2,
   CARGO_TARGET_DIR=<repo-root>/target/<issue number>, CARGO_INCREMENTAL=0,
   CARGO_PROFILE_DEV_DEBUG=line-tables-only, CARGO_PROFILE_TEST_DEBUG=line-tables-only, the disk
   check), then the exact commands, e.g. cargo fmt --all --check;
   cargo clippy -p <crate> --all-targets -- -D warnings; cargo test -p <crate>>

DONE WHEN
  <each finding's "fixed" condition holds; the commands above pass; the diff touches only
  OWNED PATHS>

REPORT FORMAT
  status: done | blocked
  requirements: <each item -> met (test or evidence) | not met>
  gate results: <per command>
  threads: <thread URL -> what changed, commit>
  decisions logged: <DECISIONS.md section titles, or none>
  contract-change requests: <list, or none>
```

## 4. Run-log convention

Rows routed by the fast path start the Notes column with `triage`, followed by the tier and a
one-line reason (the Model column keeps the model; Status as usual), for example:
`triage T0: docs typo fix spelled out by the comment`.
