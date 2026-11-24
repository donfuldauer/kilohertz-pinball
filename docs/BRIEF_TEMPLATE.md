# Delegation Brief Template

The work-classifier fills this in for each delegation (PROCESS.md P4, P5), and the Orchestrator
passes it verbatim to the named agent. A brief must be usable without access to `docs/SPEC.md`,
the GitHub issue, `docs/DEVELOPMENT.md` or `docs/PROCESS.md`: everything the agent needs is
pasted in. Briefs worth keeping (a phase-wide `COMMON.md`, long briefs) are saved under
`docs/briefs/<phase>/`. This template replaces SPEC Appendix D (formerly PLAN Appendix D).

```text
WORK ITEM <#issue or finding id>: <title>   AGENT: task-executor | work-verifier | issue-investigator
ROLE: Implementer | Independent Tester | Fixer | Verifier | Investigator
MODEL: <Haiku 5.5 | Sonnet 5.5 | Opus 5.5 | Fable 5.2 — or Flash-Lite | Flash | Pro | T3>   TIER: T0–T3 (+ P4 T3/Fable evidence if T3)
BRANCH: <branch>   BASE: main   PR: <number or "not yet open">

ISSUE: #<n> <url>   PLAN ID: KP-<NNN>   PHASE: <phase:NN-name>   TRACKING ISSUE: <#79 M1 | #80 M2>
REQUIREMENTS: <the issue's Requirements list: SPEC sections (for example "SPEC §4 Subphase 2.5,
               Verification 1") and REQ IDs (for example "REQ-LAT-004, SPEC §7.5")>

OWNED PATHS (you may modify only these):
  <paths from the issue; split so Implementer and Independent Tester do not overlap>
  Shared append-only files every issue may append to: docs/DECISIONS.md, docs/HUMAN_QUEUE.md
  (P8 lines only), docs/RUN_LOG.md (Orchestrator), contracts/CHANGE_REQUESTS.md.
DO NOT TOUCH:
  <paths, especially contracts owned by other units: crates/pin_core/, SPEC Appendix A schemas,
   physics_defaults.toml, the spec-test paths of the other role>
READ-ONLY CONTEXT:
  <public APIs of dependency issues and subphases, with signatures>

GOAL
  <one paragraph on the result, not the method>

REQUIREMENTS / DELIVERABLES
  <numbered, each testable, each tagged with the SPEC section or REQ ID it implements.
   For a CodeRabbit finding: the quoted comment, thread URL, file:lines, and what "fixed" means.>

CONTRACTS YOU MUST HONOUR
  <SPEC §3 and Appendix A–C excerpts (and §7, Appendix E–F for presentation work), and the
   contracts from docs/DEVELOPMENT.md "Contracts and owners", inlined in full>

RULES
  <PROCESS.md P2 rules R1–R12, inlined in full>

TESTS REQUIRED
  <tests to add or update, each naming the SPEC section or REQ ID it covers; property tests for
   stated invariants; bounded random-bytes runs for touched parsers; existing suites that must
   still pass>

COMMANDS
  <the "Agent environment" block from docs/DEVELOPMENT.md (CARGO_BUILD_JOBS=2,
   CARGO_TARGET_DIR=<repo-root>/target/<issue number> (verifier: <n>_ver),
   CARGO_INCREMENTAL=0, CARGO_PROFILE_DEV_DEBUG=line-tables-only,
   CARGO_PROFILE_TEST_DEBUG=line-tables-only, the disk check), then the exact format / lint /
   test / coverage / replay / gate commands from docs/DEVELOPMENT.md for the touched crates>

VERIFICATION CRITERIA
  <the issue's acceptance criteria, verbatim, with tolerances>

VISUAL CHECKLIST (render and presentation work only; otherwise "not applicable")
  <what a correct image must show (SPEC Appendix F.4 where it applies), the analytic pixel
   assertions that come first, the images the verifier must open, and the windowed-app run
   (Xvfb + lavapipe) for input, timing or presentation changes (R12)>

GATE / DONE WHEN
  <the unit gate from PROCESS.md P5, inlined, or the checks the work-verifier will run; Gates A1,
   A2 or A3 when the change touches what they measure>

WHEN SOMETHING IS UNCLEAR
  Choose the option most consistent with SPEC, the issue and the decision-log excerpts below,
  record it in docs/DECISIONS.md as `## <work item> — <title>` with Decision / Rationale /
  Alternatives (the recommendation is the decision, R8) and continue. Do not wait.
  Do not add a docs/HUMAN_QUEUE.md line unless it is a PROCESS.md P8 item
  (`- [P8 <kind>] [<id>] <text>`, kinds setting, hardware, assets, release, spec, legal).
  <relevant DECISIONS.md excerpts and SPEC §0 decision rows (D1–D33)>

ROLE-SPECIFIC
  Implementer:        deliver code + unit tests; public rustdoc for every public item; do not
                      read the Independent Tester's spec-test paths.
  Independent Tester: you have NOT seen the implementation and must not ask for it. Write
                      black-box and property tests under the spec-test paths from this brief
                      alone; report any API mismatch instead of reading the code.
  Fixer:              smallest change that meets "fixed"; regression test; name the thread(s).
                      Do not reply to or resolve threads; the Orchestrator does (P6 "Gate reviews" step 4).
  Verifier:           re-run every gate step; trace each requirement and REQ ID to a passing
                      test; open the rendered images and walk the visual checklist; no style or
                      design opinions (CodeRabbit reviews the PR).
  Investigator:       reproduce, root-cause, verdict, fix plan; change no repository files.

REPORT FORMAT
  <the report block from the receiving agent's definition in .claude/agents/ (forwarded by .agents/agents/)>
```
