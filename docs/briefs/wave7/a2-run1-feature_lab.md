WORK ITEM A2-run1-feature_lab: Gate A2 failures on tables/feature_lab (owning subphase 6.4)   AGENT: issue-investigator
ROLE: Investigator
MODEL: Opus 5.5   TIER: T2 (every investigation is T2; no Fable: no D34 evidence yet, blast radius unknown until root cause)
BRANCH: none (read-only; probe worktrees only)   BASE: main (f3648a0 + Gate A2 runner = a8c0974)   PR: not yet open
CARGO_TARGET_DIR for you: /home/user/kilohertz-pinball/target/inv-flab

OWNED PATHS: none (read-only investigation). READ: /home/user/kp-gate-a2-run/{tables/feature_lab,crates/pin_sim,crates/pin_physics,crates/pin_script,docs/decisions,docs/queue,docs/briefs}. Prior history: docs/decisions/2026-11-19-6.4-cr1.md, 2026-11-19-6.4-cr1-tests.md, and docs/queue/6.4*.md in the same worktree (before a bot fix, this table had similar findings; learn what was already tried so you do not repeat it).

GOAL
 Explain, per distinct class, every finding on feature_lab and say for each whether table geometry/rules, bot, engine or test is responsible, with a fix plan. This table has FOUR finding classes; do not assume one cause.

OBSERVED (Gate A2 run 1, SEED=1, 1,000 games per skill)
 * standard: EndlessBall games 283 (slot 0) and 360 (slot 1); EscapedBall games 424 and 426 at walls[0] near (5..21, 903, -48) mm (ball left the table bounds, z = -48 mm, velocity mostly -z).
 * expert: StuckBall game 36 (3 balls stuck near walls[3] / walls[12] / walls[0] at about (13..67, 321..371, 14) mm, 0 m/s, for 10 s); EndlessBall games 23, 110, 124, 369, 417 (one at z = 291 mm), 695, 793, 967, 984, mostly slot 1 or 2 (extra balls in multiball / XL modes, plunger_tip listed as subject); EscapedBall game 424 at walls[0] (5, 903, -48).
 * Everything else passed: coverage, standard median game length 131.8 s, score order 131630 < 239380 < 285520, replay checks, zero script errors and queue overflows. Novice: check the report for its status.
 * Classes to separate:
   A. EscapedBall (standard 424, 426; expert 424): the same wall segment walls[0] near y = 903 mm, z = -48 mm: a hole in the collision geometry at a wall end, or tunnelling at speed, or a ball spawned/ejected outside? Find the first tick the ball is outside, its speed and the closest wall feature.
   B. StuckBall (expert 36): three balls at rest in a pocket formed by walls[3]/[12]/[0] near y = 321..371 mm, z = 14 mm; is this a multiball lock/saucer/trough pile-up where rules never release them, or a geometry trap?
   C. EndlessBall on extra balls (slots 1 and 2; multiball/XL): balls listed with plunger_tip as subject: the extra ball is waiting at the plunger and the bot never plunges it, or the rules never release the ball, or the ball is in a lane the bot ignores. Likely a bot/rules interplay; confirm with the switch history.
   D. EndlessBall on slot 0 (standard 283) and game 417 (z = 291 mm, which is high: ball above the playfield on a ramp/layer?): ball loops or sits somewhere reachable.

REQUIREMENTS
 1. Reproduce each listed game singly with SEED=1 (play_soak_game driver in a probe worktree; --threads 1) and confirm the same finding kind and subject. Report any that do not reproduce.
 2. Fully root-cause one representative of each class A-D (more if the mechanism differs), with trajectory/state evidence (position, speed, layer, last 100 switch events with times, mode/multiball state from the script).
 3. Cluster all 17 reported findings into classes by mechanism, citing evidence (cheap checks: ball-state at the finding tick) for the non-representative ones.
 4. For A, give the exact geometry (wall coordinates from layout.toml/table.toml, mm) and whether the fix is table geometry, or an engine tunnelling/CCD limit; if the engine is at fault say which crate/module and whether the fix changes replay digests of other tables.
 5. For B and C, decide between table rules (rules.lua: release logic, timeouts), bot behaviour (skill model never plunges the ball, ignores a ball in a pocket), and the monitor definition. Check whether the earlier "bot fix" left an uncovered path.
 6. Provide a fix plan per class, with expected remaining findings, suggested tier per fix unit and ordering (fixes touching shared engine code or rules after Gate A2 freezes are higher risk: flag them).

RULES (PLAN §2.3, binding; you change no repository files, so rules 1-2 and 4 matter only if your fix plan proposes changes)
 1. Crate and module boundaries: an agent edits only its target crate and module directory.
 2. Contract-change protocol (D4): pin_core changes go through contracts/CHANGE_REQUESTS.md as a separate pin_core-only PR.
 3. GPU-free core tests: pin_core, pin_physics, pin_input, pin_script, pin_assets, pin_tablegen, pin_sim pass cargo test with no display or GPU.
 4. No blocking on the sim thread (D5): no heap allocation, mutex or file I/O in step_1ms() in Rust engine code.
 5. Simulated time in tests (D8): headless tests never sleep and never assert on wall-clock time.
 6. No test-shaped special cases: solver code must not branch on named behaviours; tests assert emergent results across parameter sweeps.
 7. Agent roles (D34, D35): the model and role are whatever the classifier picked; investigators diagnose, executors fix.
 8. Never wait on a person (D28): when ambiguous choose the option most consistent with the decision log, record it in a new docs/decisions/<date>-<unit-id>.md and add a line in docs/queue/<unit-id>.md. (As an Investigator you change no repository files: put any such decision text and queue line in your report instead.)
 9. No self-review (D36, D37): no style or design review of code. Resolving CodeRabbit threads is the Orchestrator's job.
DETERMINISM (D9): same binary + same machine => identical replay. UNITS (D17): SI internally, TOML uses mm/g/deg.

ENVIRONMENT AND CONSTRAINTS
 * 4 cores shared; another executor (pin_sim record/replay, PR #61) builds with CARGO_BUILD_JOBS=1. Use CARGO_BUILD_JOBS=1 for builds, and run soaks with --threads 1 (at most 2). Never run a full 1,000-game soak; reproduce the exact failing game indices.
 * Any cargo command is prefixed: CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_TEST_DEBUG=line-tables-only CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/<YOUR-DIR>   (use a release profile for soak probes; check `df -h /home/user` first, ~24 GB free; one target dir only).
 * READ-ONLY, never write or run builds in: /home/user/kp-gate-a2 (PR #61 work), /home/user/kp-gate-a2-run (pinned worktree, detached a8c0974 = main f3648a0 + Gate A2 runner; read its source freely), /home/user/kilohertz-pinball/target/gate_a2, /home/user/kilohertz-pinball/target/gate-a2-runner.
 * The release binary /home/user/kilohertz-pinball/target/gate-a2-runner/release/pin_sim (built from a8c0974) may be executed read-only. Write soak output only under your own --out dir inside your scratch area (e.g. /home/user/kilohertz-pinball/target/<YOUR-DIR>/out).
 * If you need code changes for probes (extra logging, a single-game driver), make them in a scratch copy: `git -C /home/user/kilohertz-pinball worktree add --detach /home/user/kp-probe-<name> a8c0974` (or `cp -r` of the source), build with your own CARGO_TARGET_DIR, and DO NOT commit, push, stash, reset or check out in any existing worktree. Remove your probe worktree when done (`git worktree remove --force`). Probe changes are never part of your deliverable except as a described patch in your fix plan.
 * Soak CLI: pin_sim soak <bundle> --games N --seed S --skill novice|standard|expert --threads T [--max-ticks N] [--no-tick-budget] --out DIR. There is no CLI option for a single game index. Game i has game_seed(seed, i) = splitmix64(seed ^ splitmix64(i+1)) and a bot seed derived from it, independent of N and of threads, so `--games i+1` replays games 0..i (slow) and the library function pin_sim::autoplay::soak::play_soak_game(bundle, &SoakConfig, game) plays exactly one game identically to run_soak (a tiny example/test driver in a probe worktree is the cheap way). The run used SEED=1, THREADS=3, GAMES=1000. The bundle dirs are target/gate_a2/<table>/bundle/. Soak reports: target/gate_a2/<table>/soak_<skill>/soak_report.json; logs under target/gate_a2/logs/ and the verdict at /tmp/claude-0/-home-user-kilohertz-pinball/fd4fd628-734d-5bec-bde4-1a9125eb3542/scratchpad/gate_a2_run1_verdict.txt. Check that the failure reproduces on a fresh single-game run before theorising; if it does not reproduce, say so and check thread-count/state dependence.
 * Stale-build hazard: when a result contradicts a report, touch the changed sources and rebuild before calling it a defect.

GATE A2 (the gate that failed; PLAN Phase 6): for each of the three tables, 1,000 autoplay games at each skill level: (1) zero stuck, escaped or non-finite balls; zero script errors; zero command-queue overflows. (2) 100% switch coverage; every declared shot made; every rule mode started. (3) median game length at the middle skill between 1 and 6 minutes; scores increase with skill level. (4) deterministic replay of 20 sampled games per table. Failure policy: a failing gate opens fix tasks against the owning subphases; it never pauses for a person. Rules are frozen after Gate A2; later tuning is data-only (physics_defaults.toml and table TOML), and the Lua/Wasm parity test stays in CI. So a fix that changes rules.lua is allowed now, but flag it, since it is the last chance.

WHEN SOMETHING IS UNCLEAR: choose the option most consistent with the decision log (PLAN D1-D42, docs/decisions/, frozen docs/DECISIONS.md), say what you chose in the report, and continue. Do not wait for a person.

ROLE-SPECIFIC (Investigator): reproduce, root-cause, verdict, fix plan; change no repository files.

REPORT FORMAT (final message; make it complete, it is your only output)
  status: root-caused | not-found | blocked
  reproduction: exact commands/driver and results (game indices, seeds, ticks)
  root cause: mechanism, with evidence (state traces, positions, switch sequences, numbers)
  verdict: one of table geometry/rules | bot | engine | test | mixed (state the share of each finding class)
  fix plan: owning subphase, files, smallest change, regression test, expected effect, blast radius (does it change replay digests / simulation output for other tables?), and the tier you suggest for the fix unit
  per finding class (list each distinct class separately; do not lump)
  decisions to log / queue lines proposed (text only)
  residual risk: what the fix may not cover; how many of the originally reported findings you expect it to clear
