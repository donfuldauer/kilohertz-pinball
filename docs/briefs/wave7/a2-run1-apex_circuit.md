WORK ITEM A2-run1-apex_circuit: Gate A2 failures on tables/apex_circuit (owning subphase 6.1)   AGENT: issue-investigator
ROLE: Investigator
MODEL: Opus 5.5   TIER: T2 (every investigation is T2; no Fable: no D34 evidence yet, blast radius unknown until root cause)
BRANCH: none (read-only; probe worktrees only)   BASE: main (f3648a0 + Gate A2 runner = a8c0974)   PR: not yet open
CARGO_TARGET_DIR for you: /home/user/kilohertz-pinball/target/inv-apex

OWNED PATHS: none (read-only investigation). READ: /home/user/kp-gate-a2-run/{tables/apex_circuit,crates/pin_sim,crates/pin_physics,crates/pin_script,docs/decisions,docs/queue,docs/briefs}.

GOAL
 Find why bot games on apex_circuit leave a ball on the table for more than 300,000 ms (EndlessBall finding), and decide whether it is the table (geometry/rules.lua), the bot, the engine, or the test/monitor threshold, with a fix plan.

OBSERVED (Gate A2 run 1, SEED=1, 1,000 games per skill)
 * novice: clean. standard: EndlessBall game 156 (slot 0, last switch sling_right_sw). expert: EndlessBall game 450 (last switch newton_sw; in the last 60 s: newton_sw x41, sling_right_sw x23, flipper EOS, spinner, bumpers ...) and game 818 (sling_right_sw). Soak exits 1 for standard and expert.
 * Everything else passed on this table: switch/shot/mode coverage, standard median game length 121.9 s, score order 168750 < 263980 < 279380, replay re-run and pattern checks, zero script errors and queue overflows.
 * Three findings in 2,000 games: a rare loop. Hypotheses to test, not to assume: ball trapped in a sling/newton bumper loop where rebounds repeat without draining (a stable orbit between sling_right and newton, with the flippers unable to reach it); a ball resting or oscillating in a pocket the bot never lets drain; a rules.lua behaviour (ball saver, kickback, hold/relaunch) keeping a ball alive indefinitely; an engine solver artefact (energy gain in repeated sling or bumper kicks); the monitor's 300 s threshold being tripped by a legitimate long-lived ball (compare to ball_time_ms in the reports).

REQUIREMENTS
 1. Reproduce games 156 (standard), 450 and 818 (expert) singly with SEED=1 and confirm the EndlessBall finding (play_soak_game driver in a probe worktree; --threads 1).
 2. For each, capture the ball's trajectory over the last ~20 s before the finding (position, speed, active layer, flipper states, the last 100 switch events with times) and classify the loop (geometric orbit, standing contact, kick-driven energy gain, bot idle, rules holding the ball).
 3. Determine whether the three games share one mechanism. If the loop is an orbit, say which two or three table elements form it and give coordinates in mm (table.toml / layout.toml names).
 4. Check energy: does ball speed or total kinetic energy grow without bound or stay stationary (solver artefact) versus a closed trajectory under legitimate kicks?
 5. State whether the bot could have broken the loop (nudge, flipper hold/release) and whether a human could; if only a bot weakness, say so.
 6. Give the verdict and a fix plan with an estimate of how many such loops remain at 1,000 games after the fix (e.g. re-run the games adjacent in index only if cheap; do not run full soaks).

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
