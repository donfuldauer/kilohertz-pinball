WORK ITEM: Gate A1 fix, group `bundle`: passive-energy and feel-window checks on the grey-box bundle itself (today all such checks are rig tests, not the Gate A1 target).
ROLE: task-executor (test author)
MODEL: opus
BRANCH: a1/fix (worktree /home/user/wt/a1fix, from main 43712fb). Do not commit.
OWNED PATHS: crates/pin_app/tests/a1_bundle_passive.rs (new), crates/pin_app/tests/a1_bundle_feel.rs (new). Must not touch anything else (not the grey-box data, not the `support` module: if you need a helper, put it in your own file or in a new `crates/pin_app/tests/a1_support/mod.rs` that you also own).
RUN ORDER: this group runs AFTER groups `greybox` and `deadspot` have landed in the worktree, because it must test the final table and final physics defaults. If the worktree does not yet contain `crates/pin_app/tests/a1_inlane.rs`, stop and report status: blocked.

GOAL
Gate A1 is defined "on the grey-box". Two new test files load the real grey-box bundle into the headless `Simulation` and check (1) that energy never increases in passive scenes on that table and (2) that the Appendix C feel windows hold on the bundle's own flippers, bumpers and slings.

CONTEXT
PLAN Gate A1: "Energy never increases in any passive scene." and "Provisional feel windows (Appendix C): unloaded flipper stroke time and the exit speed of a full shot from a cradled ball fall inside their windows." Appendix C windows: unloaded flipper stroke time (rest to EOS) 20-35 ms; exit speed of a full shot from a cradled standard ball 3-7 m/s; bumper kick exit speed 1.5-4 m/s; slingshot kick exit speed 1.5-4 m/s. DECISIONS (2.4): the bumper and sling windows are guaranteed for a coil fired on the tick after the switch edge (edges collected in tick N reach the script in tick N+1, whose coil commands apply in that tick); a later firing is outside the guarantee.
Existing machinery: crates/pin_app/tests/greybox_game.rs and spec_6_0_greybox.rs load the bundle through `support::greybox_bundle()` and drive `pin_sim::Simulation` (`spawn_ball`, `set_ball_state`, `set_switch`, `advance`, ball and flipper state accessors; see crates/pin_sim/src/lib.rs and crates/pin_sim/tests/accessors_tick_outputs.rs for the accessors). Existing rig tests for these windows (in crates/pin_physics/tests/flipper_criteria.rs and the 2.4 bumper/sling tests) show how the measurements are defined; the new tests measure the same quantities on the bundle. The grey-box flippers are `left_flipper`, `right_flipper`, `left_upper`; its switches include `lower_left_eos`, `lower_right_eos`, `upper_left_eos`, `sling_left_sw`, `sling_right_sw`, `bumper_*_ring` (names in table.toml).

REQUIREMENTS
1. `a1_bundle_passive.rs`: at least these passive scenes on the bundle, with the cabinet inputs idle and no coil fired by the test (rules.lua may stay attract-mode; if it fires a coil on its own, say so and account for the energy it adds by only asserting over windows where no coil edge happens, which you detect from the sim's coil/command outputs): (a) a ball released at rest at 5 positions on the open playfield, total mechanical energy E = m g h_along_pitch + 1/2 m v^2 + 1/2 I w^2 never exceeds its start value by more than a tolerance of 1e-6 J + numerical noise bound you derive (state it) at any tick over 5 simulated seconds, with collisions with static geometry counted (they only dissipate); (b) a ball fired at 1, 2 and 3 m/s at a bumper body, a sling face at rest (the sling coil NOT fired) and a wall, same assertion; (c) two balls colliding. Energy is computed from the sim's public ball state accessors; if the spin is not exposed, use translational + potential energy with a rolling-ball upper bound 7/5 factor stated in the comment, and say so in the report. Use `proptest` only if it is already a dev-dependency of pin_app; otherwise use fixed seeded sweeps via `pin_core::Pcg32` (rule D9).
2. `a1_bundle_feel.rs`: (a) unloaded stroke time of each of the three flippers on the bundle: fire the coil with no ball present, measure the time from the first tick of motion (button edge) to the EOS switch closing, in ticks (1 ms each): inside 20-35 ms for every flipper; (b) full shot from a cradled standard ball: a ball placed at rest on each lower flipper at the cradle point (derive the point from the flipper geometry: the ball resting against the held-down flipper at its low end near the pivot), flipper fired, exit speed taken as the ball speed when it has moved 100 mm from the tip (or leaves the flipper's contact): inside 3-7 m/s for both lower flippers; (c) bumper kick: ball rolled at each of the 3 bumpers at 1, 2 and 3 m/s from three directions, bumper coil fired on the tick after the ring-switch edge, exit speed inside 1.5-4 m/s; (d) sling kick: likewise for both slings. Report every measured number (min/max per measure).
3. If a window fails on the bundle: do not loosen it and do not change any table or physics data (they are not yours). Keep the failing assertion, report status: blocked with the numbers per object. If the failure is because the rules script or table coils differ from the rig (for example the sling kick is out of window at some phases), say exactly which.
4. All tests in simulated time (rule 5) and deterministic (D9); each file completes in under 60 s in debug on 1 core.
5. Add nothing to Cargo.toml unless a dev-dependency is truly missing; if so, exact-pin it under a `# Wave 6 additions (a1fix bundle)` comment and report it as a request instead if the crate is not already in the workspace lock (do not add new crates).

TESTS REQUIRED: the two new files; existing pin_app greybox suites still pass (`cargo test -p pin_app --test greybox_game --test spec_6_0_greybox --test a1_inlane --test a1_ramp`).
COMMANDS (prefix with the COMMON cargo environment)
 cd /home/user/wt/a1fix
 cargo test -p pin_app --test a1_bundle_passive --test a1_bundle_feel -- --nocapture
 cargo test -p pin_app --test greybox_game --test spec_6_0_greybox --test a1_inlane --test a1_ramp
 cargo fmt --check; cargo clippy -p pin_app --all-targets -- -D warnings
DONE WHEN: both files pass at the stated windows on the final grey-box, or the report is blocked with measured numbers; the diff is only the owned files; every measured number is in the report.
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
