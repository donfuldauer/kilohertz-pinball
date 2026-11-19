WORK ITEM: gate-a2-cr1-A (CodeRabbit round 1 on PR #61, finding 1, part A: pin_sim sampled-game input record and replay)
ROLE: task-executor (Fixer / Implementer)
MODEL: opus
BRANCH: gate/a2-runner (PR #61), worktree /home/user/kp-gate-a2, head a8c0974. Do not commit/push/checkout.
THREAD: https://github.com/donfuldauer/kilohertz-pinball/pull/61 review thread 4625148047 (MAJOR, docs/decisions/2026-11-19-gate-a2-runner.md:91-92). Quote: "Gate A2 can print PASS without the sampled-game replay that PLAN.md requires. PLAN.md requires 'deterministic replay of 20 sampled games per table.' A2.4a re-runs the first games with a different thread count and compares seed and state_hash. Bot-input record and playback is reported as REPORT and never fails. A2.4b exercises different games. Resolve: block PASS while sampled-game input replay is unmet, or amend PLAN.md to accept the repeatability check. Keep the existing checks as supplemental evidence."
ORCHESTRATOR DECISION: PLAN.md is NOT amended (gate criteria are the owner's). PLAN.md:530 defines replay as --record-inputs/--replay-inputs (Subphase 2.5) and :767 requires 20 sampled games per table. So real sampled-game input replay must exist. This brief (A) adds the pin_sim side. Brief B (separate, runs after you, edits only scripts/ and the runner docs) makes the runner gate on it.

OWNED PATHS (edit only these):
  crates/pin_sim/src/autoplay/**        (soak.rs, bot.rs hook points, mod.rs re-exports)
  crates/pin_sim/src/cli/mod.rs
  crates/pin_sim/src/replay/**          (ADDITIVE only; a new file such as replay/game.rs is fine; the existing Recording/Recorder/replay items, file format bytes and behaviour must not change)
  crates/pin_sim/src/lib.rs, crates/pin_sim/src/main.rs   (only for re-exports / usage text if needed)
  crates/pin_sim/tests/autoplay_record_game.rs   (new)
  crates/pin_sim/tests/*   existing files: only if an existing assertion on the USAGE strings must be extended; do not weaken anything
  docs/decisions/2026-11-19-gate-a2-cr1-pin-sim.md  (new)
  docs/queue/gate-a2-cr1.md                          (new)
DO NOT TOUCH: scripts/**, tables/**, .github/**, crates/pin_core, pin_physics and every other crate, docs/decisions/2026-11-19-gate-a2-runner.md and docs/queue/gate-a2-runner.md (brief B owns them), crates/pin_sim/src/{sim,tick,hash,load,alloc_guard,commands}/** (the simulation itself; if you think you need a change there, STOP and put it under "requests outside owned paths").

GOAL
Make it possible to record the effective cabinet input of selected bot games in a soak and to replay each recorded game later without the bot, reaching a bit-identical final state_hash and score. Recording is opt-in and must not change any existing soak output, replay digest, tick behaviour or bot decision. This lets Gate A2 check "deterministic replay of 20 sampled games per table" for real.

REQUIREMENTS
1. Implement the CLI exactly as in PIN_SIM CLI CONTRACT below (flags `--record-games`, `--record-dir` on `soak`; `--replay-game FILE` on the replay form). Update USAGE/SOAK_USAGE strings, parse functions and module docs; add rustdoc for every public item.
2. FACT you must handle (read crates/pin_sim/src/autoplay/bot.rs `Bot::tick`/`apply` around lines 685-960 and soak.rs `play_game`): the bot does NOT feed `InputEvent`s. It presses and releases the cabinet buttons with `Simulation::set_switch(name, true)` / `release_switch(name)` (forced-switch overrides, applied between ticks, which call update_switches immediately) and applies nudges with `sim.mechs.nudge(dv)` between ticks. A plain `Recording` of `InputEvent`s replayed through the ingest path is therefore NOT guaranteed to reach the same state. Record what the bot effectively did, at the point it is applied (the `apply` switch changes with their tick, and each nudge `dv`), as a time-stamped operation stream, and have replay apply the same operations through the same Simulation calls at the same tick boundaries, then `advance(1)`, with no Bot. Prefer exactness over reuse. Choose the file format yourself (a new sibling of Recording in replay/: magic distinct from the existing recording magic, version, seed, table_hash, ticks, ops, checksum trailer in the style of Recording::to_bytes/from_bytes, MAX_TICKS bound, bounded parsing that never panics on random bytes), but the existing Recording format bytes and `--replay-inputs` must remain unchanged. If you find the effective inputs CAN be expressed exactly as existing `InputEvent`s with equal hash (prove it with a test over several seeds and skills), you may reuse `Recording`; otherwise use the new format. Record the choice in the decision file.
3. The recorder is a no-op unless requested: `play_game` must make no extra allocation, branch cost or behavioural change when no game is selected (a `None`/disabled recorder). Recording may allocate (it is tooling outside `step_1ms`), but not inside `sim.advance`/`step_1ms`. The soak's tick timing measurement (the `Instant` around `advance(1)`) must stay around `advance(1)` only.
4. The replay must run on a thread with SIM_THREAD_STACK_BYTES stack like the other CLI paths and use `SimConfig::default()` (the soak CLI uses defaults; if `cfg.sim` can differ from default for a recorded game, store what is needed or refuse to record with a clear error).
5. `recorded_games.tsv` values come from the same `GameSummary` the soak reports (state_hash, ticks, score), written after the soak completes, including when the soak fails its findings; recording files are written for every selected game even if the soak fails. Selected index >= --games is a usage error. Reproducibility: the recorded files and tsv are identical across runs and across `--threads` values.
6. The soak's stdout/stderr summary, exit codes, soak_report.json and soak_report.md are byte-identical with and without `--record-games` (modulo nothing). Prove it with a test.
7. Docs: new file docs/decisions/2026-11-19-gate-a2-cr1-pin-sim.md with sections `## gate-a2-cr1: bot-game recording format and hook point` (Decision / Rationale / Alternatives, including the forced-switch/nudge fact), and docs/queue/gate-a2-cr1.md with one line per open follow-up you leave (e.g. [gate-a2-cr1-record-games-all-skills] if you only wire standard). Do not write the "resolved" marks for the old queue lines; brief B does.

TESTS REQUIRED (crates/pin_sim/tests/autoplay_record_game.rs, using the existing fixtures via `#[path = "fixtures/autoplay/support.rs"] mod support;` as autoplay_alloc.rs does; run the sim on `on_sim_thread`):
  T1. Record-and-replay equality: soak a fixture bundle with `--record-games` through the library API and through `main_with_args` (a few games, a few seeds, all three skills, `--threads 1` and 2), replay every recording with the new replay path, and assert replayed state_hash == the soak game's state_hash and replayed score == the soak score, and that ticks match.
  T2. Negative: (a) tamper at the operation level (decode, drop or shift one switch press or one nudge, re-encode with a valid checksum) and assert the replayed state_hash differs from the soak's (use a game where the press matters; assert the test is not vacuous by checking the unmodified file matches); (b) flip a byte of the file: replay returns a checksum/format error, never a panic; (c) a recording replayed against another bundle: table-mismatch error before building; (d) a bounded random-bytes run into the parser never panics.
  T3. Non-interference: soak_report.json, soak_report.md and the stdout summary are byte-identical with and without `--record-games` for the same args (timing fields: use `--no-tick-budget` plus whatever the existing tests do to compare reports, or compare with the timing section masked; see tests/autoplay_report.rs / autoplay_determinism.rs for the existing pattern); recordings and tsv identical for --threads 1 vs 2.
  T4. Spec parsing: `0,49,99`, `spread:20` over 1000 games gives 0,50,...,950, `spread:3` over 20 games, errors for index >= games, N > games, descending, duplicates, empty, garbage; --record-dir inside bundle refused.
  Existing suites that must still pass unchanged: autoplay_alloc, autoplay_determinism, autoplay_report, autoplay_detectors, autoplay_endless, autoplay_fixtures, replay_cli, replay_determinism, replay_format, spec_2_5_*, spec_2_9_*, a1_gate_verdict, sim_alloc (the allocation guard) and the pin_sim lib unit tests. Property test (proptest, already a dev-dependency of the crate if present; otherwise a fixed-seed random-bytes loop) for the format round trip and no-panic parsing.

COMMANDS (run from /home/user/kp-gate-a2; E= the env prefix below)
  E="CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_TEST_DEBUG=line-tables-only CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/gate-a2-cr1"
  env $E cargo fmt -p pin_sim --check
  env $E cargo clippy -p pin_sim --all-targets -- -D warnings
  env $E cargo test -p pin_sim --test autoplay_record_game -- --test-threads=1
  env $E cargo test -p pin_sim --test autoplay_alloc --test autoplay_determinism --test replay_cli --test replay_determinism --test replay_format --test sim_alloc -- --test-threads=1
  env $E cargo test -p pin_sim --lib -- --test-threads=1
  When done (time permitting, once): env $E cargo test -p pin_sim -- --test-threads=1
  Replay-hash stability: run the new replay on one recording 10 times and require identical output.
  Smoke on a real table (small, single-threaded): tables/apex_circuit/build.sh /home/user/kilohertz-pinball/target/gate-a2-cr1-smoke/bundle then
  env $E cargo run --release -q -p pin_sim -- soak <bundle> --games 6 --seed 1 --skill standard --threads 1 --record-games spread:3 --out <smoke>/soak, then `--replay-game` on each file; report the hashes and equality. Do not use /home/user/kilohertz-pinball/target/gate_a2 or target/gate-a2-runner.

PIN_SIM CLI CONTRACT (fixed; brief A implements it, brief B consumes it; both briefs carry the same text)
 Recording sampled bot games (soak):
   pin_sim soak <bundle> [existing flags] --record-games <SPEC> [--record-dir DIR] --out DIR
     SPEC = comma-separated ascending game indices, each < --games (e.g. `0,49,99`), or `spread:N` = the N indices floor(i*GAMES/N) for i = 0..N-1 (1 <= N <= GAMES; N = 20 over 1,000 games gives 0,50,...,950). Anything else is a usage error (exit 2, no soak run).
     --record-dir defaults to `<out>/recordings`. Both must not lie inside the bundle (same check as --out).
     For each selected game it writes `<record-dir>/game_<index zero-padded to 6 digits>.khzrec` and, after the soak completes (also when the soak itself fails its findings), `<record-dir>/recorded_games.tsv`: one line per recorded game, ascending by game, tab-separated: `game<TAB>file name (not path)<TAB>ticks<TAB>state_hash (16 lowercase hex)<TAB>score (decimal i64, or `none`)`. state_hash, ticks and score are the soak's own end-of-game values for that game (the same values as that game's game_list entry in soak_report.json).
     Without --record-games nothing changes: same stdout, same soak_report.json and soak_report.md bytes, same exit codes.
 Replaying one recorded bot game without the bot:
   pin_sim <bundle> --replay-game FILE
     Loads the bundle (default SimConfig), refuses a recording made on a different table hash (non-zero exit, message on stderr), replays the recorded effective cabinet input of the bot with no Bot running, and prints exactly two stdout lines: `state_hash=<16 lowercase hex>` then `score=<decimal i64 or none>`. Exit 0 on success; non-zero with a stderr message for a missing/corrupt/tampered file (checksum) or table mismatch. It takes no other flags.
 The existing `--ticks/--record-inputs/--replay-inputs` behaviour, file format bytes and replay digests are unchanged.

ENVIRONMENT / MACHINE LIMITS (important)
 * 4 cores. A FULL Gate A2 RUN IS EXECUTING NOW using 3 threads, from the separate pinned worktree /home/user/kp-gate-a2-run (detached at a8c0974), with CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/gate-a2-runner and OUT=/home/user/kilohertz-pinball/target/gate_a2. You MUST NOT touch that worktree, that target dir, or that OUT (no reads that lock, no builds into it, no deletes). Work only in /home/user/kp-gate-a2 (branch gate/a2-runner, head a8c0974).
 * Use your own target dir and the minimum CPU: every cargo command is prefixed with
     CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_TEST_DEBUG=line-tables-only CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/gate-a2-cr1
   Use `--threads 1` for every pin_sim soak you run and `cargo test ... -- --test-threads=1`. Only small smoke runs (about 10 to 20 games, a few thousand ticks); never a 1,000-game run. Check `df -h /home/user` before builds; under 4 GB free, `cargo clean -p pin_sim` in YOUR target dir only and say so. Build only `-p pin_sim` (and what tables/<t>/build.sh needs). Never `cargo build --workspace`.
 * Do NOT commit, push, stash, reset or check out; the Orchestrator does git. Report the list of changed files.

RULES (PLAN section 2.3, binding):
 1. Crate and module boundaries. An agent edits only its target crate and module directory. Edit only OWNED PATHS; anything else you need is a REQUEST in your report.
 2. Contract-change protocol (D4). Only Subphase 1.1 writes pin_core freely. Any later agent that needs a contract change appends a request to contracts/CHANGE_REQUESTS.md and submits it as a separate pin_core-only PR. (You need none; do not touch pin_core.)
 3. GPU-free core tests. pin_core, pin_physics, pin_input, pin_script, pin_assets, pin_tablegen and pin_sim pass cargo test with no display server and no GPU. Render tests live behind the gpu-tests feature (D20).
 4. No blocking on the sim thread (D5). No heap allocation, mutex, or file I/O in step_1ms() in Rust engine code. Tests wrap the tick in a panicking-allocator guard. Exemptions: the Lua VM's capped arena; a hot-reload swap tick.
 5. Simulated time in tests (D8). Headless tests never sleep and never assert on wall-clock time.
 6. No test-shaped special cases. Do not branch on named behaviours or table names (beyond the REPORT lines already in the runner). Tests assert emergent results.
 7. Agent roles (D34, D35). Work was classified by the work-classifier, is done by a task-executor, checked by the work-verifier, diagnosed by the issue-investigator when something fails for a non-obvious reason.
 8. Never wait on a person (D28). When the brief is ambiguous, choose the option most consistent with the decision log, record it in the decision file named in OWNED PATHS (a "## <id>: <topic>" section with Decision / Rationale / Alternatives), add a line to the queue file named there, and continue.
 9. No self-review (D36, D37). Do not review code style or design of yours or others'. The work-verifier checks objective facts only; CodeRabbit reviews the PR; the Orchestrator resolves threads (you report thread URLs and what changed).
DETERMINISM (D9): same binary + same machine => identical replay; fixed iteration order (no HashMap iteration), one seeded PCG, no fast-math. UNITS (D17): ticks are 1 ms.

DONE WHEN (the work-verifier re-runs these)
 1. fmt and clippy clean for pin_sim; all tests above pass; the allocation guard tests pass.
 2. Byte-identical soak outputs with/without --record-games (T3 passes); existing replay digests tests unchanged and passing; git diff shows no change to crates/pin_sim/src/{sim,tick,hash,load,alloc_guard,commands}/** and no change to the existing Recording format code paths other than additive items.
 3. Smoke: for every sampled game, `--replay-game` state_hash and score equal recorded_games.tsv.
 4. Path check: diff touches only OWNED PATHS.

REPORT FORMAT (final message)
 status: done | blocked
 gate results: <per step with numbers: tests run/passed, smoke hashes, timings>
 files changed: <list>
 decisions logged: <decision file path and section titles>
 contract-change requests: none
 requests outside owned paths: <list or none>
 findings: <anything the verifier should know, esp. whether recorded bot input could be replayed as plain InputEvents>
 thread follow-up: thread 4625148047 URL and what changed (the Orchestrator resolves it, D37; thread resolution waits for brief B).
