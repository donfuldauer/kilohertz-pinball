WORK ITEM A2-run1-sky_deck: Gate A2 / readiness score-order failure on tables/sky_deck (owning subphase 6.2)   AGENT: issue-investigator
ROLE: Investigator
MODEL: Opus 5.5   TIER: T2 (every investigation is T2; no Fable: no D34 evidence yet, blast radius unknown until root cause)
BRANCH: none (read-only; probe worktrees only)   BASE: main (f3648a0 + Gate A2 runner = a8c0974)   PR: not yet open
CARGO_TARGET_DIR for you: /home/user/kilohertz-pinball/target/inv-sky

OWNED PATHS: none (read-only investigation). READ: /home/user/kp-gate-a2-run/{tables/sky_deck,crates/pin_sim,crates/pin_app/tests/spec_6_2_soak.rs,docs/decisions/2026-11-18-6.2-tester.md,docs/queue/6.2*.md,docs/briefs}.

GOAL
 Decide how the readiness test crates/pin_app/tests/spec_6_2_soak.rs::c6a_soak_readiness_40_games_per_skill (ignored, 120-game soak: 40 per skill) fails its score-order assertion while the 1,000-game Gate A2 soaks pass, and whether the right fix is (a) table tuning, (b) bot tuning, or (c) a test sample-size/statistics fix. Do not assume a verdict; compute it.

OBSERVED
 * Gate A2 run 1 (SEED=1, 1,000 games per skill, tables/sky_deck): zero findings at every skill; standard median game length 96.7 s; median scores novice 64400 < standard 78900 < expert 79200 (the standard/expert margin is only 0.4%); coverage, shots, modes, replay checks all pass.
 * Readiness test (40 games per skill) fails ONLY on the final assertion `median scores do not ascend with skill`: medians [69000, 90000, 57700] (novice, standard, expert: expert lowest). All earlier assertions in the test passed.

REQUIREMENTS
 1. Read the test and find its seed(s) and game count (cfg(skill, GAMES)); run it, or an equivalent probe, with the same configuration only for the needed skills to reproduce the medians [69000, 90000, 57700] (release profile, --threads 1; 120 games is acceptable, 1,000 is not). Record wall time.
 2. From target/gate_a2/sky_deck/soak_<skill>/soak_report.json (read-only) and fresh probe runs, estimate the score distribution per skill: median, mean, IQR, standard deviation, bootstrap 95% CI of each median and of the standard-vs-expert median difference at n = 40 and n = 1,000. State the probability that a 40-game sample puts expert below standard given the true distributions. Also try 2-3 other seeds at n = 40 (cheap) to show how often the order flips.
 3. Explain why expert barely outscores standard on this table: compare per-skill game length, balls per game, switch/shot/mode counts, and how the bot's skill levels differ (crates/pin_sim/src/autoplay/bot.rs: accuracy, flipper timing, nudges, risk). Is expert's accuracy advantage capped by table geometry/rules (scoring dominated by a mode or bonus the bot reaches equally at both skills, or by luck), or are the bot's skill levels barely separated on this table?
 4. Check whether the test's design is statistically sound: a 40-game median asserting strict ascending order of the standard and expert levels with a true margin of 0.4% cannot be reliable. Check also how the sibling tests/gates (6.1, 6.4 readiness, Gate A2 itself) treat score order (sample size, margin) and what PLAN says ("scores increase with skill level" at 1,000 games).
 5. Verdict and fix plan: choose among (a) table tuning with concrete data-only knobs (table TOML values, not rules.lua) that would widen the gap, (b) bot skill-level tuning (which owns the bot: subphase 2.9, shared by all three tables, so blast radius is every table's soak numbers), (c) change the test (e.g. assert novice < standard and expert >= standard within a tolerance, or compare means with a margin, or a bigger fixed sample) and say whether weakening it counts as test weakening against the brief (rule: a disagreement between a spec test and the implementation is resolved against the brief, never by deleting or weakening the test; so say what the 6.2 brief required). Give the smallest option that keeps Gate A2 meaningful. Do NOT recommend changing the bot if the data say it is a test sample issue only; do not recommend widening the gap in a way that changes the other two tables.
 6. Say whether a fix can be validated cheaply (name the exact commands and expected runtime) and the suggested tier.

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
