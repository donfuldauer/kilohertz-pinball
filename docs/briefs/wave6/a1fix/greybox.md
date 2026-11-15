WORK ITEM: Gate A1 fix, group `greybox`: (1) inlane stuck balls, (2) ramp escapes (Fix A), (3) the ramp is never made (Fix B), (4) the novice StuckBall at trough_2, with regression tests.
ROLE: task-executor (Fixer)
MODEL: opus
BRANCH: a1/fix (worktree /home/user/wt/a1fix, from main 43712fb). Do not commit.
OWNED PATHS: tables/greybox/layout.toml, tables/greybox/table.toml, tables/greybox/README.md (design notes only), crates/pin_app/tests/a1_inlane.rs (new), crates/pin_app/tests/a1_ramp.rs (new). Append-only: docs/DECISIONS.md, docs/HUMAN_QUEUE.md.
MUST NOT TOUCH: any other file, in particular crates/pin_tablegen/**, crates/pin_physics/**, crates/pin_sim/**, physics_defaults.toml, existing tests in crates/pin_app/tests/ (add new files only), tables/greybox/rules.lua.

GOAL
The grey-box table (`tables/greybox`, the Gate A1 soak target) loses no balls to the inlane pockets, none to the wireform ramp (no escapes, no fall-through), and a ball shot from the left flipper's line can make the ramp. After the fix, 100 autoplay games at each of novice, standard and expert produce zero StuckBall and zero EscapedBall findings and the `ramp_made` switch is hit. Only data and new tests change; no engine code.

CONTEXT (investigator findings, on main 43712fb; coordinates are mm in table.toml/layout.toml, +y is up the table, z is height)
Gate A1 soak of 1,000 games failed: 618 StuckBall, 336 EscapedBall, `ramp_made` never hit.

1. Stuck balls (618/1000). The inlane wall end points in tables/greybox/layout.toml lines 27-34 leave a V-pocket between the wall end and the back of the flipper pivot cap (flipper pivots (157,150) and (357,150) in table.toml). Balls come to rest there. Candidate fix, already tested in scratch: left inlane wall [[0,330],[152,172]], right inlane wall [[470,330],[362,172]] (instead of the current end points). With it: 0 StuckBall at standard and expert over 100 games each. Novice: 1 StuckBall at trough_2, position (318,14,14), which is outside the right drain funnel wall (357,150)-(299,10). Scratch: .../scratchpad/inv1/ (a1trace.rs, a1drain.rs, fix/layout.toml, bundle_fix, soak reports).
   Regression test to add (crates/pin_app/tests/a1_inlane.rs): spawn a ball at rest about 120 mm up each inlane (left and right), advance 4 simulated seconds; the ball must pass below the flipper tips or close a trough switch. It must fail on main 43712fb's layout and pass with the fix (check by running it against the unfixed layout first and note the result).
2. Escapes (336/1000), all from wireform ramp layer handling. The generated 0->2 entry zone (pin_tablegen layout/mod.rs lines 701-725) is a box x/y 408..432, z 0..14.9 with no layer-2 support before the rails start at y 420. The table's only 2->0 zone, `ramp_fallback` (table.toml lines 355-358, y 350..418), overlaps it. TransitionSet fires only on outside->inside (pin_physics world/transition.rs:65, DECISIONS line ~199), so balls promoted inside the fallback zone never demote and fall through the floor or pass through walls. Fix A (data), tested: shrink `ramp_fallback` to y < 408; add 2->0 catch zones beside the ramp (x < 408 and x > 432, y 408..930, tall z) and under the wire (z <= 9); add a layer-2 deck (z = 0) under the entry (x 405..435, y 395..421). With Fix A escapes went 336 -> 18; the remaining 18 are balls launched off the blunt rail start (cause 3). Scratch: .../scratchpad/a1inv/ (trace patch, sweep a1_ramp_sweep.rs, bundle_fixA, soak_fixA).
3. `ramp_made` never hit. The ramp can only be made within +-2 mm and +-10 degrees of the wire axis, which is x = 420 straight up the table above the right inlane. No flipper shoots along that axis. The blunt rail start at z = 2.9 mm launches off-centre balls. Fix B is a geometry DESIGN task: re-shape the wire so the ramp is reachable from the left flipper's shot line, and prove it with tests and the soak. Suggested design (you may vary it if the tests demand): turn the wire's first section to face the left flipper's shot line (about 30-40 degrees right of +y from around (220,180)), then curve it up the right side; layer-0 funnel walls narrowing to about 30 mm before the entry; lower or bury the rail start; close layer 0 under the low wire section; fix the code comment that says "1 mm below contact height" so it is true. The wireform spline and the layer-transition zones live in table.toml; the walls in layout.toml. Keep the other mechanisms (orbit, VUK, upper deck, magnet, Newton, drop target, bumpers, slings) working: the existing tests in crates/pin_app/tests (greybox_game.rs, spec_6_0_greybox.rs, greybox_load.rs, greybox_reload.rs, integration_table_sim.rs and others) inject balls at fixed positions and count switches. If one of them depends on the old ramp position, do NOT edit the test; report it as a finding with the failing assertion and keep the table behaviour that the test requires unless it is impossible (then report as blocked).
4. Do not re-bless anything: there are no pinned grey-box digests in crates/pin_app/tests (the replay tests record and replay in-run). Check with `grep -rn "state_hash" crates/*/tests tables/greybox` and report if you find any pinned grey-box hash; if one exists, say which, and do not change it.

REQUIREMENTS
1. Inlane geometry fixed (stage 1) so that a ball at rest 120 mm up either inlane reaches the trough or passes below the flipper tips within 4 s, tested in `a1_inlane.rs` for both sides, plus a sweep of at least 5 start heights (80..200 mm) per side.
2. Ramp Fix A applied (stage 2): `ramp_fallback` ends below y = 408, catch zones as described, entry deck in place. Test (`a1_ramp.rs`): an offset/heading sweep at the ramp entry (offsets +-0..30 mm across the entry, headings +-0..40 degrees, speeds 2, 3 and 4 m/s, balls injected on the approach from layer 0 with `spawn_ball`/`set_ball_state`) gives `balls_escaped == 0` (or whatever the sim's escaped counter is called; use the same notion as the soak's EscapedBall detector in crates/pin_sim/src/autoplay/monitor.rs) over 4 simulated seconds each.
3. Ramp Fix B applied (stage 3): a ball shot from the left flipper's line (about (220,180) at 3-4 m/s, headings spanning what the left flipper can actually produce: derive them by firing a cradled ball with the flipper at several timings, not by hard-coding one heading) makes the ramp: `ramp_made` closes exactly once, for a sweep of at least 3 speeds in 3..4 m/s and at least 3 headings inside the flipper's real range. Report the range of headings that make the ramp (so the verifier sees it is not a single lucky ray). The ramp must also no longer be makeable only within +-2 mm of one axis: report the made/missed width at the entry.
4. After the fixes, soak: build `pin_sim` release and the grey-box bundle, then run `pin_sim soak <bundle> --games 100 --seed 1 --skill <novice|standard|expert> --out <scratch dir>` for each skill (use `pin_sim soak --help`-style usage in crates/pin_sim/src/cli/mod.rs line 348 for the flags). Required: zero StuckBall, zero EscapedBall, zero NonFinite; `ramp_made` hit at least once; the ramp ShotCoverage finding (if the report has one) not present at standard and expert. Report the number of ramp shots and the median ball time (ms) per skill; do not tune anything to the median here.
5. Novice StuckBall at trough_2 (318,14,14): after stage 1 the novice 100-game run has zero StuckBall. If it still occurs, trace it (the scratch a1drain.rs shows how), and fix it in data (extend or move the right drain funnel wall near (357,150)-(299,10) so a ball at (318,14) cannot be outside it, or fix whatever lets the ball past it) and add a regression test in `a1_inlane.rs` that spawns a ball at rest at (318,14) at ball height and shows it reaches a trough switch within 4 s on the fixed table. If the novice run is clean with the inlane fix alone, still add that test, as a check, and say so.
6. Run the pre-existing pin_app headless suites listed in TESTS REQUIRED unchanged; they must pass.
7. Update tables/greybox/README.md only with the new geometry notes (one short paragraph), and append a `## 6.0 fix — Gate A1 grey-box geometry` section to docs/DECISIONS.md (Decision / Rationale / Alternatives) recording the inlane end points, the ramp zones and the Fix B design chosen; one HUMAN_QUEUE line.
8. Attempt limit (D29): if after three honest design iterations of Fix B no geometry makes the ramp makeable from the left flipper line with escapes 0, stop and report status: blocked with the sweep data and the traces, so the Orchestrator can route an investigation.

TESTS REQUIRED
 New: crates/pin_app/tests/a1_inlane.rs, crates/pin_app/tests/a1_ramp.rs (use the `support` module's `greybox_bundle()`/`on_big_stack`, as crates/pin_app/tests/greybox_game.rs does). They run in simulated time only (rule 5), loop fixed tick counts, and use the existing `Simulation` API (`spawn_ball`, `set_ball_state`, `advance`, switch counts). Keep each file's total runtime under about 60 s in debug on 1 core by choosing sweep sizes sensibly.
 Must still pass unmodified: `cargo test -p pin_app --test greybox_game --test spec_6_0_greybox --test greybox_load --test greybox_reload --test integration_table_sim --test spec_6_0_load` and `cargo test -p pin_tablegen` (the layout fixtures; you do not touch it but the grey-box is built by it).

COMMANDS (prefix every one with the cargo environment from COMMON)
 cd /home/user/wt/a1fix
 tables/greybox/build.sh <scratch>/bundle        (builds the bundle with pin_tablegen)
 cargo test -p pin_app --test a1_inlane --test a1_ramp
 cargo test -p pin_app --test greybox_game --test spec_6_0_greybox --test greybox_load --test greybox_reload --test integration_table_sim --test spec_6_0_load
 cargo build --release -p pin_sim --bin pin_sim
 target/sp_6_0/release/pin_sim soak <scratch>/bundle --games 100 --seed 1 --skill standard --out <scratch>/soak_standard   (likewise novice, expert)
 cargo fmt --check; cargo clippy -p pin_app --all-targets -- -D warnings

DONE WHEN
 All tests above pass; the three 100-game soaks show zero StuckBall/EscapedBall/NonFinite and `ramp_made` hit; the diff touches only OWNED PATHS; every number requested above is in the report (headings range, entry width, ramp shots, median ball time per skill, escapes before/after, the unfixed-layout result of the new tests).
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
