---
name: triage
description: "The Orchestrator's inline fast path for classifying obviously small work items (T0/T1 fixes, CodeRabbit nits, docs/run-log/queue edits, CI tweaks) under PLAN §2.5 and D38, and for batching one review round or push. Do NOT use it for subphase implementations, Independent Tester tasks, investigations, gate failures, or anything that fails the eligibility checks; those go to the work-classifier."
---

# triage (PLAN §2.5 "Fast path", D38)

The Orchestrator runs this skill itself to classify small items without spawning a
work-classifier. It is for the cheap classification step only; the work is still done by a
task-executor and checked by the work-verifier. The fast path never routes T2 or T3.

## 1. Eligibility (ALL must hold)

- The item is a fix, a CodeRabbit finding, a docs/run-log/queue edit or a CI tweak (never a
  subphase implementation, an Independent Tester task, an investigation, or a gate failure).
- The tier is clearly T0 (Haiku 5.5) or T1 (Sonnet 5.5).
- It touches one module or docs only.
- No `pin_core` contract change.
- No change to simulation output (pin_physics world/ball/flipper numerics, sim tick, triple
  buffer, replay digests).
- No security boundary (Lua/Wasm sandbox, budgets, memory caps, zip extraction, vendored Lua
  patches).
- Validity and cause are clear (otherwise issue-investigator first).
- The fix is spelled out by the comment or obvious.

If any check fails, or the Orchestrator is unsure, send the item to the work-classifier.

## 2. Batching procedure (one CodeRabbit review round, or one push)

1. List every new finding.
2. Triage each one as fast-path T0, fast-path T1, or classifier.
3. Send all non-fast-path items to ONE work-classifier call as a batch.
4. Group fast-path items by tier into one Fixer brief per tier; items that touch the same file
   go in the same brief.
5. Spawn executors, in parallel when owned paths do not overlap.
6. Run ONE work-verifier on the resulting head commit covering every item in the round.
7. Reply to and resolve threads per §2.7 step 3 / D37.

## 3. Fixer brief template

A valid subset of PLAN Appendix D; field names match it. Fill it in and pass it to the
task-executor with the tier's model as the `model` override.

```text
WORK ITEM <id>: <title>          AGENT: task-executor
ROLE: Fixer
MODEL: <Haiku 5.5 | Sonnet 5.5>   TIER: T0 | T1
BRANCH: <branch>   PR: <number>

OWNED PATHS (you may modify only these):
  <paths; every file the findings touch, nothing else>

GOAL
  <one paragraph: the findings in this brief are fixed with the smallest change>

REQUIREMENTS
  Per finding, one numbered item:
  N. Quoted comment: "<comment text>"
     Thread URL: <url>
     File:lines: <path>:<a>-<b>
     Fixed means: <exact observable condition>
  For each finding, report the thread URL and what changed. Do not reply to or resolve
  threads; the Orchestrator does (D37).

RULES
1. **Crate and module boundaries.** An agent edits only its target crate and module directory. Where two subphases share a crate in one wave, each owns a module directory and the earlier subphase pre-stubs the `mod` declarations.
2. **Contract-change protocol (D4).** Only Subphase 1.1 writes `pin_core` freely. Any later agent that needs a contract change appends a request to `contracts/CHANGE_REQUESTS.md` and submits it as a separate `pin_core`-only PR. Additive changes merge immediately; breaking changes merge only between waves.
3. **GPU-free core tests.** `pin_core`, `pin_physics`, `pin_input`, `pin_script`, `pin_assets`, `pin_tablegen` and `pin_sim` pass `cargo test` with no display server and no GPU. Render tests live behind the `gpu-tests` feature and run in their own CI job (D20).
4. **No blocking on the sim thread (D5).** No heap allocation, mutex, or file I/O in `step_1ms()` in Rust engine code. Tests wrap the tick in a panicking-allocator guard (e.g. `assert_no_alloc`). Two explicit exemptions: the Lua VM allocates from its own capped, pre-reserved arena; and a hot-reload swap tick may allocate and overrun (developer-only event).
5. **Simulated time in tests (D8).** Headless tests never sleep and never assert on wall-clock time.
6. **No test-shaped special cases.** Solver code must not branch on named behaviours ("live catch", "powerball"). Tests assert emergent results across parameter sweeps.
7. **Agent roles (D34, D35).** Every unit of work is classified by the work-classifier (or, for items that pass the triage checks in §2.5, by the `triage` skill, D38), done by a task-executor (as Implementer, Independent Tester or Fixer), checked by the work-verifier, and diagnosed by the issue-investigator when something fails for a reason that is not obvious (§2.6). The model is whatever the classifier picked (§2.5).
8. **Never wait on a person (D28).** When the brief is ambiguous, choose the option most consistent with the decision log, record it in `docs/DECISIONS.md`, add a line to `docs/HUMAN_QUEUE.md`, and continue.
9. **No self-review (D36, D37).** No agent reviews code for style or design, its own or another agent's. The work-verifier checks objective facts only; code review happens on the pull request by CodeRabbit (§2.7). The Orchestrator resolves every review thread itself when justified and never leaves one for a person (§2.7 step 3).

COMMANDS
  Every cargo command runs with these environment variables (see
  docs/briefs/wave5/COMMON.md "ENVIRONMENT" for the rest, including the disk-space check):
    CARGO_BUILD_JOBS=2
    CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/<dir for this brief>
    CARGO_INCREMENTAL=0
    CARGO_PROFILE_DEV_DEBUG=line-tables-only
    CARGO_PROFILE_TEST_DEBUG=line-tables-only
  <exact cargo commands, e.g. cargo fmt --check; cargo clippy -p <crate> --all-targets -- -D warnings; cargo test -p <crate>>

DONE WHEN
  <each finding's "fixed" condition holds; the commands above pass; the diff touches only
  OWNED PATHS>

REPORT FORMAT
  status: done | blocked
  requirements: <each item -> met (test or evidence) | not met>
  gate results: <per command>
  threads: <thread URL -> what changed, commit>
  decisions logged: <list>
  contract-change requests: <list>
```

## 4. Run-log convention

Rows routed by the fast path show `triage` in the routed column, followed by the tier and a
one-line reason, for example: `triage T0: docs typo fix spelled out by the comment`.
