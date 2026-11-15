WORK ITEM: Gate A1 fix, group `runner`: a Gate A1 runner that applies every A1 criterion (a `pin_sim soak --gate a1` verdict, a script, and a manual CI job), because today soak `passed()` ignores the ball-time window and TickBudget is report-only.
ROLE: task-executor (Implementer)
MODEL: sonnet
BRANCH: a1/fix (worktree /home/user/wt/a1fix, from main 43712fb). Do not commit.
OWNED PATHS: crates/pin_sim/src/autoplay/report.rs, crates/pin_sim/src/cli/mod.rs, crates/pin_sim/src/autoplay/mod.rs (re-exports only), crates/pin_sim/tests/a1_gate_verdict.rs (new), scripts/gate_a1.sh (new), .github/workflows/ci.yml (add one job only). Append-only: docs/DECISIONS.md, docs/HUMAN_QUEUE.md (only if you make a ruling).
MUST NOT TOUCH: any other file. In particular do not change `SoakReport::passed()` semantics, the existing JSON/Markdown output of `to_json_deterministic` / `to_markdown_deterministic` (byte-identical for existing inputs), or other crates.

GOAL
One command, `scripts/gate_a1.sh`, runs the whole of PLAN Gate A1 on the grey-box and prints a per-criterion PASS/FAIL table, exiting non-zero on any failure. The soak-related criteria are judged by a new, tested verdict function in `pin_sim`, so that the ball-time window and the tick budget are applied by code, not by someone reading a report.

CONTEXT
PLAN Gate A1: "1. Analytic checks: rolling 5/7 g sin(theta) within 1 % (1.07 m in ~1.64 s); drop-bounce height ratio = e^2 within 2 %; tilt bob period within 2 %; energy never increases in any passive scene. 2. Provisional feel windows (Appendix C): unloaded flipper stroke time (20-35 ms) and exit speed of a full shot from a cradled ball fall inside their windows. 3. Soak: 1,000 autoplay games: zero stuck, escaped or non-finite balls; 100 % switch coverage; median ball time between 15 s and 120 s; 99.9th-percentile tick under 0.35 ms. Also flipper dead-spot depth <= 25 % (DECISIONS 'Orchestrator — 2.3 criterion 1: the 25 % depth moves to Gate A1')."
Current state (crates/pin_sim/src/autoplay/report.rs lines 1-22): `SoakReport::passed()` is false on any failing-kind finding (StuckBall, EscapedBall, NonFinite, ScriptError, QueueOverflow) or incomplete coverage (UnreachableSwitch, ShotCoverage, ModeCoverage). TickBudget is wall-clock, lives in the report's timing section, is reported but never fails the soak. The ball-time distribution is `ball_time_ms: Distribution` with `median` (report.rs lines 145-175, 352). CLI: crates/pin_sim/src/cli/mod.rs `pin_sim soak <bundle> [--games N] [--seed S] [--skill novice|standard|expert] [--threads T] [--max-ticks N] [--no-tick-budget] --out DIR` (usage at line 348).
DECISIONS ruling (to be recorded by another group; follow it): the median-ball-time window 15-120 s is judged on the soak at skill `standard`; novice and expert medians are reported only. Zero stuck/escaped/non-finite, 100 % coverage and the 0.35 ms p99.9 tick are judged at every skill.
Test names the script must run (these files are being written by other groups; the script must tolerate a missing one by FAILING loudly with "missing test target <name>", never by skipping it): pin_physics: `a1_analytic`, `ball_criteria`, `flipper_criteria`, `spec_2_3_criteria`, `mech_tilt`, `a1_dead_spot`; pin_app: `a1_inlane`, `a1_ramp`, `a1_bundle_passive`, `a1_bundle_feel`. Before wiring them, run `cargo test -p pin_physics --test mech_tilt --test ball_criteria -- --list` and similar to confirm which existing tests cover the tilt-bob period (2 %), passive-scene energy and feel windows, and list the names in the script's comments. If a criterion has no covering test target yet, the script prints "NO TEST for <criterion>" and counts it as a FAIL.

REQUIREMENTS
1. Add to `report.rs` an additive, documented API (public, with rustdoc): `GateA1Verdict` and `SoakReport::gate_a1(&self, skill: SkillLevel) -> GateA1Verdict` (or the nearest equivalent matching the file's style) that returns one named check per soak criterion: `no_stuck`, `no_escaped`, `no_nonfinite`, `switch_coverage_100`, `median_ball_time_15_120s` (judged only when skill is standard, otherwise `Reported` with the value), `p999_tick_under_0_35ms` (judged from the timing section; if the soak ran with `--no-tick-budget` or no timing is present the check is `Fail("tick timing not measured")`, never a silent pass), and an overall `passed()`. Median bounds are inclusive: 15_000 ms and 120_000 ms. Use whatever ticks/ns units the timing section holds; the 0.35 ms limit is 350_000 ns. Also report the number of ball-time samples; a median over zero balls is a Fail.
2. CLI: `--gate a1` flag on `pin_sim soak`: after the soak, print the verdict lines to stdout (one per check: `PASS|FAIL|REPORTED  <name>  <value>`), exit 1 if the soak did not pass or the verdict failed, else 0. Without the flag behaviour and output are byte-identical to today. Update the usage constant and the module doc.
3. Tests in crates/pin_sim/tests/a1_gate_verdict.rs on crafted `SoakReport` values (build them the way crates/pin_sim/tests/autoplay_report.rs does): median 14_999 ms fails, 15_000 passes, 120_000 passes, 120_001 fails (standard); novice with median 5_000 is `Reported` and does not fail; p99.9 = 349_999 ns passes, 350_000 ns fails (state which side is the inclusive boundary: strict "under"); missing timing fails; a StuckBall, EscapedBall, NonFinite finding each fail their own check; incomplete coverage fails; zero ball-time samples fails; `passed()` of the existing report is unchanged by the new code (existing autoplay_report tests untouched and green).
4. `scripts/gate_a1.sh` (bash, `set -euo pipefail`, no wall-clock reads of its own): takes optional `OUT` dir (default `target/gate_a1`) and `GAMES` (default 1000) and `SEED` (default 1). Steps: build the grey-box bundle (`tables/greybox/build.sh "$OUT/bundle"`); `cargo test` each test target listed above (continue after a failure so all results are printed); `cargo build --release -p pin_sim --bin pin_sim`; soak `GAMES` games at each of novice, standard, expert with `--gate a1`, `--out "$OUT/soak_<skill>"`; print a final table with each criterion and PASS/FAIL; exit non-zero if anything failed. The cargo environment variables (CARGO_TARGET_DIR etc.) are taken from the caller's environment; the script sets none of its own except `CARGO_INCREMENTAL=0` default.
5. CI: add ONE job `gate-a1` to .github/workflows/ci.yml, ubuntu-latest only, `if: github.event_name == 'workflow_dispatch'` (the 1,000-game x 3-skill soak is too long for every push; the push-time CI already runs the analytic and rig tests in `core-tests`), `timeout-minutes: 180`, same toolchain/rust-cache steps as `replay-hash`, runs `scripts/gate_a1.sh`, and uploads `target/gate_a1/soak_*/` reports with `actions/upload-artifact` (use the version already pinned elsewhere in the file, or the same major as `actions/checkout@v5`'s sibling steps if none). Do not change any other job. Add a comment explaining why it is manual. Before writing it, check how long a 1,000-game soak takes on this machine in release (run `--games 50 --skill standard` once and extrapolate), and write the extrapolated figure into the job comment.
6. Run the script end-to-end once in a reduced form (`GAMES=20`) to prove it works; the verdict may be FAIL on this table until the other groups land; report what it printed. Missing test targets are expected to be reported as FAIL at this point.
7. Determinism: nothing in the new verdict code reads a wall clock; it only reads values already in the report.

TESTS REQUIRED: crates/pin_sim/tests/a1_gate_verdict.rs (new); all existing `cargo test -p pin_sim` (including autoplay_report, spec_2_9_report, autoplay_determinism, spec_2_9_determinism) pass unchanged. `cargo test -p pin_sim --test autoplay_alloc --test spec_2_9_clean` still green.
COMMANDS (prefix with the COMMON cargo environment)
 cd /home/user/wt/a1fix
 cargo test -p pin_sim
 cargo fmt --check; cargo clippy -p pin_sim --all-targets -- -D warnings
 bash -n scripts/gate_a1.sh; GAMES=20 OUT=<scratch>/gate scripts/gate_a1.sh
 python3 -c "import yaml,sys; yaml.safe_load(open('.github/workflows/ci.yml'))"   (syntax check only, if PyYAML exists; otherwise read it carefully)
DONE WHEN: new tests pass; existing pin_sim tests unchanged and green; `pin_sim soak ... --gate a1` prints the verdict and exits as specified; the script runs end-to-end; ci.yml has exactly one added job.
REPORT FORMAT: see COMMON. Include the 50-game timing and the script's final table.

## COMMON (binding for every Gate A1 fix brief)

RULES (PLAN §2.3, in full)
 1. Crate and module boundaries. An agent edits only its OWNED PATHS. Anything else you need is a REQUEST in your report; do not edit it.
 2. Contract-change protocol (D4). Only Subphase 1.1 writes `pin_core` freely. A later agent that needs a contract change appends a request to `contracts/CHANGE_REQUESTS.md` and stops that part; do not edit `pin_core` in this work.
 3. GPU-free core tests. `pin_core`, `pin_physics`, `pin_input`, `pin_script`, `pin_assets`, `pin_tablegen` and `pin_sim` pass `cargo test` with no display server and no GPU. Render tests live behind `gpu-tests` (D20).
 4. No blocking on the sim thread (D5). No heap allocation, mutex or file I/O in `step_1ms()` in Rust engine code. Tests wrap the tick in a panicking-allocator guard. Exemptions: the Lua VM arena; a hot-reload swap tick.
 5. Simulated time in tests (D8). Headless tests never sleep and never assert on wall-clock time. (The Gate A1 runner script, not a cargo test, may read the soak's wall-clock tick-time report.)
 6. No test-shaped special cases. Solver code must not branch on named behaviours. Tests assert emergent results across parameter sweeps. Tests are never weakened, loosened, deleted or `#[ignore]`d to get green; a failing existing test is a finding to report.
 7. Agent roles (D34, D35). Work is classified by the work-classifier, done by a task-executor, checked by the work-verifier, diagnosed by the issue-investigator.
 8. Never wait on a person (D28). When the brief is ambiguous, choose the option most consistent with the decision log (PLAN D1–D38, docs/DECISIONS.md), append a `## <id> — <topic>` section (Decision / Rationale / Alternatives) to docs/DECISIONS.md and one line to docs/HUMAN_QUEUE.md, and continue. Append only; never rewrite other entries. (Only if docs/DECISIONS.md and docs/HUMAN_QUEUE.md are not in your OWNED PATHS: put the text in your report instead.)
 9. No self-review (D36, D37). Do not review code for style or design. The work-verifier checks objective facts only; CodeRabbit reviews the PR; the Orchestrator resolves review threads, not you.

DETERMINISM (D9): same binary + same machine => identical replay. No fast-math, no FMA contraction, `libm` for transcendentals, fixed iteration order (no HashMap iteration), one seeded PCG. UNITS (D17): SI internally; TOML uses mm / g / deg, converted at load.

WORKTREE AND GIT: work only in `/home/user/wt/a1fix` (local branch `a1/fix`, based on main 43712fb). Do NOT commit, push, stash, reset, check out, or create branches; the Orchestrator does git. Report the list of changed files. Several executors may work in this worktree at once on disjoint OWNED PATHS: touch nothing outside yours, and if another executor's file breaks your build, report it instead of editing it.

ENVIRONMENT (disk is tight: about 7.5 GB free)
 * 4 cores, 15 GB RAM, shared. EVERY cargo command is prefixed with:
     CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_TEST_DEBUG=line-tables-only CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/sp_6_0
   Use target dir `sp_6_0` for everything: debug for tests, `--release` for soaks. Build only what you need (`-p <crate>`, `--test <name>`); never `cargo build --workspace`.
 * Check `df -h /home/user` before big builds. If free space is under 3 GB, stop building and say so in your report.
 * CLEANUP RULE: never delete build artifacts (`cargo clean`, `rm -rf target/...`, removing test executables) while another executor may be building in `sp_6_0`. You do not clean up at all; the Orchestrator cleans up between groups. Put scratch files and logs in `/tmp/claude-0/-home-user-kilohertz-pinball/fdff3db3-77a1-5457-bdd1-308f21a8b3a8/scratchpad/a1fix_work/<your group>/`, not in the repo.
 * Toolchain: rustc 1.97.0, clippy, rustfmt. Do not install anything. No new dependencies.
 * Build the grey-box bundle with `tables/greybox/build.sh <out dir>` (out dir under your scratch directory) or the `support::build_greybox_bundle` helper in crates/pin_app/tests/support/mod.rs.
 * Reference scratch from the investigations (read-only, may be copied from): /tmp/claude-0/-home-user-kilohertz-pinball/fdff3db3-77a1-5457-bdd1-308f21a8b3a8/scratchpad/inv1/ and .../a1inv/.

GATE / DONE WHEN (the work-verifier re-runs these)
 1. `cargo fmt --check` and `cargo clippy --all-targets -- -D warnings` clean on every touched crate.
 2. Every REQUIREMENT traces to a passing test or a recorded measurement.
 3. Existing suites of the touched crates still pass, unmodified.
 4. Line coverage for touched crates is not lowered below 85 % (pin_sim, pin_physics) or 60 % (pin_app) by the change.
 5. Path check: the diff touches only OWNED PATHS.

WHEN SOMETHING IS UNCLEAR: choose per the decision log, record it (rule 8), continue.

REPORT FORMAT (final message)
 status: done | blocked
 gate results: <per step, with numbers: tests run/passed, measurements, timings>
 files changed: <list>
 decisions logged: <DECISIONS.md section titles, or text for the Orchestrator>
 contract-change requests: <none>
 requests outside owned paths: <list or none>
 findings: <anything the verifier or Orchestrator must know; every number asked for in the brief>
 For any CodeRabbit follow-up the Orchestrator resolves threads (D37).
