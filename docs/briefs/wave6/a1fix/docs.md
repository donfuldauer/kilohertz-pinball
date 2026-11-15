WORK ITEM: Gate A1 fix wave, group `docs`: record two Orchestrator rulings and three queue lines. Runs FIRST, alone, before any other a1fix group.
ROLE: task-executor (docs only)
MODEL: haiku
BRANCH: a1/fix (worktree /home/user/wt/a1fix, from main 43712fb). Do not commit.
OWNED PATHS: docs/DECISIONS.md, docs/HUMAN_QUEUE.md (append only). Must not touch anything else.

GOAL
Append the two DECISIONS sections and the three HUMAN_QUEUE lines below, verbatim, at the end of the respective files. Nothing else changes.

REQUIREMENTS
1. Append to the END of docs/DECISIONS.md (after a blank line) exactly this text:

## Orchestrator — Gate A1 criterion 1a: the analytic rolling check validates the solver
**Decision:** The 5/7·g·sinθ rolling-down-the-incline check (6.5°, 1.07 m, about 1.64 s) is evaluated two ways. (a) With rolling resistance set to 0, the acceleration matches the literal 5/7·g·sinθ (0.79323 m/s²) within 1 %. (b) With the default parameters (rolling resistance c = 0.002), the run matches 5/7·g·(sinθ − c·cosθ) within 1 %. Both must pass. The shipped default c is not changed.
**Rationale:** The literal formula has no rolling resistance. With the default c the 1.07 m run takes 1.6566 s (a = 0.77978 m/s², −1.70 % against the literal formula), but with c = 0 it is +0.06 % and against the model that includes c·cosθ it is +0.06 %. So the solver is exact and the 1.7 % comes from a deliberate default that PLAN Appendix C sets for feel. The check exists to validate the solver (Gate A1 criterion 1), so it separates the solver from the tunable. Retuning c to hit the literal number would change feel for no physical reason.
**Alternatives:** Setting default c to 0 (loses the rolling-stop behaviour that the 2.2 tests require). Widening the 1 % tolerance to 2 % (weakens the gate). Gating only the default-parameter run against the literal formula (fails with no solver defect).

## Orchestrator — Gate A1 criterion 3: median ball time is measured after the fixes, judged on skill standard, and fixed in the grey-box layout
**Decision:** (1) The 15–120 s median-ball-time window is unchanged and is not relaxed for the grey-box. It is judged on the soak at skill `standard`, the reference player; the novice and expert medians are reported. Zero stuck, escaped and non-finite balls, 100 % switch coverage and the 0.35 ms p99.9 tick are judged at all three skills. (2) Order of work: the inlane, ramp-escape and flipper dead-spot fixes land first; then the median is re-measured on 1,000 standard games; nothing else is tuned before that number exists. (3) If the re-measured median is still below 15 s, the grey-box layout (Subphase 6.0 data, `tables/greybox`) is changed to keep balls in play longer, by adding table features and guides only, such as outlane-style guards, posts, rollovers or an upfield feature the bot reaches. Physics tunables, the 6.5° slope, the 1.07 m length and the ball are not changed for this purpose, because Gate A1 holds them fixed. (4) The bot flip policy (2.9) is not tuned for ball time. (5) If a layout change moves the median without reaching the window, the next step is another layout iteration (a further 6.0 fix task), and Gate A1 stays red until the soak passes; the window is never narrowed or bypassed.
**Rationale:** The measured median is 6.7 s (about 8.7–10 s after the inlane fix), and about 3 s of every ball is plunge and travel. The causes are properties of the table: a 1.07 m table with no outlanes and few features, and escapes that end balls early (336 of 1,000 games). Gate A1 and D24 exist to validate tables with the bot, so the window describes how long a table keeps a competent player's ball. Making the standard bot flip worse to stretch ball time would be tuning to the test (rule 6 in spirit), and the bot is shared with Gate A2 and the real tables. A ruling that the window does not apply to the grey-box would remove the only soak check of play length before tables start. The grey-box is project-owned data, so adding features to it is ordinary design work. Measuring first matters because two of the three causes (escapes and stuck inlanes) are defects, and they may close much of the gap on their own.
**Alternatives:** Tuning the bot flip policy to miss more shots (tuning to the test; contaminates A2). Ruling that the window applies only to the real tables (weakens Gate A1; real tables start after A1 is green). Judging the median at the novice skill (the window then proves nothing about a competent player). Changing slope or table length (breaks the analytic checks and the Appendix C physics).

2. Append to the END of docs/HUMAN_QUEUE.md three lines, each as its own bullet in the file's existing style:

- [Orchestrator ruling, Gate A1 1a] The 1.07 m rolling check is judged twice: with rolling resistance 0 it must match the literal 5/7·g·sinθ within 1 % (measured +0.06 %), and with the default c = 0.002 it must match 5/7·g·(sinθ − c·cosθ) within 1 % (+0.06 %). The default run against the literal formula is −1.70 % (1.6566 s against about 1.64 s) because of c. Please confirm, or ask for c = 0.
- [Orchestrator ruling, Gate A1 3] Median ball time (15–120 s) is judged at skill standard, measured only after the inlane, ramp and flipper fixes, and any shortfall is fixed by adding features to the grey-box layout, not by tuning the bot or changing the window. Before the fixes the median was 6.7 s (about 8.7–10 s after the inlane fix). Please confirm, or choose a different owner for this.
- [Gate A1 follow-up, pin_tablegen, queued] The generated 0→2 ramp entry zone (`pin_tablegen/src/layout/mod.rs` lines 701–725) has no layer-2 support before the rails start and can overlap a 2→0 fallback zone, which is the root of the grey-box escapes (fixed in the grey-box data only). Proposed: lay the entry zone along the spline tangent starting at the rails, and add a validator warning for a target layer without an exit zone over unsupported area. It changes generated ramp output and the pinned tests `src/layout/tests.rs:239-250` and `spec_4_3`, so it was not done for Gate A1. Do it before 6.1/6.2 use generated wireform ramps, or hand-fix their ramp data as the grey-box was.

3. Do not change any other line of either file.

TESTS REQUIRED: none (docs). Check `git diff --stat` shows only the two files, additions only.
COMMANDS: none needed (no cargo).
DONE WHEN: the diff of the two files is additions only and equals the text above.
REPORT FORMAT: see COMMON.

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
