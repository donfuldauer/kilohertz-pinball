WORK ITEM: Gate A1 fix, group `deadspot`: flipper dead-spot depth <= 25 % at both 50 % and 70 % bat contact (26.3 % today at 70 %), by a bounded parameter search in physics_defaults.toml plus a regression test.
ROLE: task-executor (Fixer)
MODEL: opus
BRANCH: a1/fix (worktree /home/user/wt/a1fix, from main 43712fb). Do not commit.
OWNED PATHS: physics_defaults.toml, crates/pin_physics/tests/a1_dead_spot.rs (new). Append-only: docs/DECISIONS.md, docs/HUMAN_QUEUE.md.
MUST NOT TOUCH: crates/pin_physics/src/** (no solver code changes), existing tests, tables/**, any other crate.
BLAST RADIUS NOTE: physics_defaults.toml changes simulation output of every table. Anything you change must keep the PLAN Appendix C bounds, the 1.07 m analytic checks and the feel windows.

GOAL
Gate A1 includes "the 25 % flipper dead-spot depth" (DECISIONS: "Orchestrator — 2.3 criterion 1: the 25 % depth moves to Gate A1": "Meeting the 25 % depth itself becomes a Gate A1 check. Gate A1 may tune flipper inertia, torques and stop restitution within the Appendix C bounds, and an end-stop compliance if a later subphase adds one."). Today the depth is 24.3 % at 50 % contact and 26.3 % at 70 % contact in the Tester's rig (16.8 % at 65 %, 30.1 % at 75 %). Make the minimum rebound in the -20..+20 ms flip-timing sweep at most 25 % of the held-flipper rebound (depth = minimum / held <= 25 %), at 50 % and at 70 % contact, in every rig, with parameters inside the Appendix C bounds, without breaking any other check.

CONTEXT
PLAN 2.3 criterion 1 (timing sweep): ball arriving at 3.5 m/s, flip timed from -20 ms to +20 ms in 1 ms steps; rebound speed is continuous across the sweep and its minimum is at most 25 % of the rebound against a held flipper. Rigs: crates/pin_physics/tests/spec_2_3_criteria.rs (the Independent Tester's rig: 26.3 % at 70 %), crates/pin_physics/tests/flipper_criteria.rs (the Implementer's 14 variants: 0-21 % at 70 %), crates/pin_physics/tests/banana_dead_spot.rs and spec_2_6_criteria.rs (curved bat, oblique incidence: 68.7 % down-table; handed to Gate A1 for judgement, NOT part of this gate: report the number only). The depth depends on sub-tick phase and rig; it is the flip whose stroke ends in the tick the ball arrives. The rubber model is e(u) = rubber_restitution / (1 + rubber_damping * u / 4 m/s). Appendix C flipper tunables and bounds: inertia (76 mm bat) 1.0e-4 kg m^2 default, 0.5-3.0e-4; power torque 1.2 N m, 0.6-3.0; hold torque 0.15, 0.05-0.4; return spring 0.03, 0.01-0.08; end-stop restitution 0.15, 0.0-0.4; rubber damping 0.35, 0.1-0.7. Feel windows that must stay satisfied: unloaded stroke time 20-35 ms; full shot from a cradled standard ball 3-7 m/s; bumper and sling kick 1.5-4 m/s.
Rule 6 (no tuning to the test) applies: choose parameters from a sweep judged across ALL rigs and contact points, not by minimising one rig's 70 % number.

REQUIREMENTS
1. Measure first, change second: run the existing rigs and record, for each rig and contact point (50 %, 65 %, 70 %, 75 %), the depth with the current defaults. Report the table.
2. Search the Appendix C flipper tunables (inertia, power torque, hold torque, end-stop restitution, rubber damping, within bounds) with a coarse grid (at least 4 values per parameter for end-stop restitution, hold torque and inertia; fewer for the rest), scoring each candidate on: depth <= 25 % at 50 % and 70 % in every straight-bat rig, no unexplained continuity step (the existing rig tests already assert this), stroke time in 20-35 ms, cradled full shot in 3-7 m/s, and the 1.07 m analytic numbers (they do not depend on flipper parameters; confirm unchanged). Among the passing candidates pick the one closest to the current defaults (smallest relative change, per parameter, in log space) and also check it is not at a knife edge: its neighbours at +-10 % in each changed parameter must also give depth <= 25 % at 50 % and 70 % (report which neighbours pass). Put the search script/harness in your scratch directory, not the repo.
3. Edit only the changed values in physics_defaults.toml (keep comments and layout; the file is parsed by tests in pin_core: run them). Do not change any value outside the Appendix C bounds.
4. New test crates/pin_physics/tests/a1_dead_spot.rs: a black-box sweep at the final defaults with the Tester's rig geometry (copy the rig setup from spec_2_3_criteria.rs; do not import from it) asserting depth <= 25 % at 50 % and 70 % contact and 65 % and 75 % reported by println only, plus an assertion that every parameter read from the defaults file lies within its Appendix C bounds. Deterministic, simulated time.
5. Run every existing test in pin_physics and pin_core, pin_sim and the pin_app greybox tests; none may change status. If some existing test pins a number that moves with the flipper defaults (for example a stroke time, a shot speed, or a replay/state hash), do NOT edit it: report it with the test name and the old and new value, so that the Orchestrator decides (it is legitimate for an exact-number test to need re-blessing only by a separate, reviewed change).
6. If no parameter combination within the bounds gives depth <= 25 % at both contacts while keeping the feel windows, STOP and report status: blocked with the grid results (best candidate and its depths). This is the evidence the Orchestrator needs to decide a model change (a compliant end stop or stiffer hold, i.e. a solver change in Subphase 2.3, T2/T3 work) instead of a data tweak. Do not edit solver code.
7. Append `## Gate A1 — flipper dead-spot depth tuning` to docs/DECISIONS.md (Decision / Rationale / Alternatives) with the old and new values and the sweep summary, and one HUMAN_QUEUE line.

TESTS REQUIRED: new a1_dead_spot.rs; unchanged green: `cargo test -p pin_physics` (all), `cargo test -p pin_core`, `cargo test -p pin_sim`, `cargo test -p pin_app --test greybox_game --test spec_6_0_greybox`.
COMMANDS (prefix with the COMMON cargo environment)
 cd /home/user/wt/a1fix
 cargo test -p pin_physics --test spec_2_3_criteria --test flipper_criteria --test spec_2_6_criteria --test banana_dead_spot -- --nocapture
 cargo test -p pin_physics --test a1_dead_spot -- --nocapture
 cargo test -p pin_physics; cargo test -p pin_core; cargo test -p pin_sim
 cargo fmt --check; cargo clippy -p pin_physics --all-targets -- -D warnings
 Note: physics tests are slow in debug; prefer `--release` for the search sweeps and debug for the final gate run. Disk: at most one additional build profile in sp_6_0 at a time.
DONE WHEN: a1_dead_spot.rs passes; depth table (before/after, per rig and contact) is in the report; no existing test changed status; changed parameters inside the bounds; neighbour check reported.
REPORT FORMAT: see COMMON. Include the before/after depth table, the chosen values, and any pinned-number test that moved.

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
