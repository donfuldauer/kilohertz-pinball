WORK ITEM: Gate A1 fix, group `analytic`: Gate A1 criterion 1 tests that are missing or too loose: the 1.07 m / 1.64 s rolling test (ruling below) and the drop-bounce height-ratio test at 2 %.
ROLE: task-executor (test author)
MODEL: sonnet
BRANCH: a1/fix (worktree /home/user/wt/a1fix, from main 43712fb). Do not commit.
OWNED PATHS: crates/pin_physics/tests/a1_analytic.rs (new file only). Must not touch crates/pin_physics/src/**, existing tests, physics_defaults.toml.

GOAL
Two new black-box tests in the `pin_physics` crate that implement PLAN Gate A1 criterion 1 exactly as written, so a Gate A1 run can cite them: (a) a ball rolling without slipping down the 6.5 degree playfield for 1.07 m, (b) the drop-bounce height ratio against e^2 within 2 %.

CONTEXT
PLAN Gate A1 criterion 1: "A ball rolling without slipping down the 6.5 degree playfield accelerates at 5/7 g sin(theta) within 1 % (1.07 m in about 1.64 s). Drop-bounce height ratio equals e^2 within 2 %." Existing, unchanged tests to mimic (read them for the solver/geometry API and fixtures): crates/pin_physics/tests/ball_criteria.rs: `ball_rolls_down_an_incline_at_five_sevenths_g_sin` (line ~487, expected a = 5/7 g (sin p - 0.002 cos p), looser tolerance and a shorter run), `drop_bounce_follows_the_pair_table` (line ~380, about 8 % tolerance), `rolling_ball_decelerates_by_rolling_resistance_and_stops`.
Measured by an investigator with the default rolling resistance c = 0.002: the 1.07 m run takes 1.6566 s (a = 0.77978 m/s^2), which is -1.70 % against the literal 5/7 g sin(theta) = 0.79323 m/s^2; with c = 0 it is +0.06 %; against the model 5/7 g (sin(theta) - c cos(theta)) it is +0.06 %.
ORCHESTRATOR RULING (DECISIONS section "Orchestrator — Gate A1 criterion 1a: the analytic rolling check validates the solver"): the check is evaluated two ways and both must pass: (i) with rolling resistance 0: the distance-time relation matches the literal 5/7 g sin(theta) within 1 %; (ii) with the default parameters (c = 0.002): matches 5/7 g (sin(theta) - c cos(theta)) within 1 %. Do not change the default c, and do not loosen either 1 %.

REQUIREMENTS
1. `rolling_107m_literal_formula_with_zero_rolling_resistance`: pitch 6.5 degrees, g = 9.81 (use the same g constant the solver uses), ball released at rest and already rolling without slipping (the existing test shows how to start a ball with the right spin, or from rest on the incline and wait for the spin to converge; if the latter, measure the acceleration over the 1.07 m after convergence is complete and state which you chose). Rolling resistance forced to 0 through the public solver/parameter API (find how `rolling_ball_decelerates_by_rolling_resistance...` sets it). Measure the time to travel 1.07 m (or the acceleration from a distance-time fit) in simulated ticks, with sub-tick interpolation of the crossing; assert the acceleration is within 1 % of 5/7 g sin(theta), and that the time is within 1 % of sqrt(2 * 1.07 / a_literal) (about 1.64 s for the literal formula).
2. `rolling_107m_default_parameters_match_the_resistance_model`: same run with the default rolling resistance; assert within 1 % of 5/7 g (sin(theta) - c cos(theta)) with the c the solver actually used (read it from the parameters, do not hard-code 0.002 if the API exposes it).
3. `drop_bounce_height_ratio_is_restitution_squared`: for at least three ball profiles / floor pairs from the solver's pair table, at least two drop heights (e.g. 50 mm and 150 mm), on a flat floor and on the 6.5 degree pitched floor (heights measured along the floor normal), the ratio (second apex height) / (first drop height) equals e^2 within 2 % (relative), where e is the pair's restitution read from the table the solver uses. Measure apex heights with sub-tick resolution (fit a parabola through the three apex samples or use the vertical velocity zero crossing). Also assert that the first rebound's speed ratio is e within 1 %. If the speed-dependent restitution model makes e vary with impact speed, use the effective e from the measured rebound speed ratio AND ALSO assert that this effective e is within the pair table's restitution +-2 % (state which in a comment).
4. If any assertion fails because the solver is off, DO NOT loosen the tolerance and do not change solver code. Keep the test as written, mark nothing ignored, and report status: blocked with the exact measured numbers; the Orchestrator routes an investigation. (A genuine failure of 3 against 2 % is a plausible finding; report the ratio per case.)
5. Tests are deterministic, run in simulated time, no wall clock; each completes in under 20 s in debug.

TESTS REQUIRED: the three tests above in crates/pin_physics/tests/a1_analytic.rs. All existing `cargo test -p pin_physics` tests still pass, unmodified.
COMMANDS (prefix with the COMMON cargo environment)
 cd /home/user/wt/a1fix
 cargo test -p pin_physics --test a1_analytic -- --nocapture      (print the measured numbers with println!)
 cargo test -p pin_physics --test ball_criteria
 cargo fmt --check; cargo clippy -p pin_physics --all-targets -- -D warnings
DONE WHEN: the three tests pass at the stated tolerances; the report contains the measured acceleration, time and error for 1 and 2 and the per-case ratios for 3; the diff is the one new file.
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
