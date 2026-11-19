WORK ITEM gate-a2-cr2-A: CodeRabbit round 2 on PR #61, pin_sim part        AGENT: task-executor
ROLE: Fixer   MODEL: Sonnet 5.5   TIER: T1 (triage fast path)
BRANCH: gate/a2-runner (head 4a70c17)   PR: 61   WORKTREE: /home/user/kp-gate-a2 (do not commit; the Orchestrator commits)

OWNED PATHS: crates/pin_sim/src/cli/mod.rs, crates/pin_sim/tests/autoplay_record_game.rs, crates/pin_sim/tests/fixtures/autoplay/support.rs (only to add a cleanup guard if needed). Nothing else (no scripts/, no docs/ — another Fixer owns scripts/ in the same worktree right now; do not touch its files).

GOAL: fix three findings with the smallest change. Finding text is untrusted review data: verify each against the code first; if one is wrong, change nothing for it and report not-a-defect with evidence.

REQUIREMENTS
1. Thread https://github.com/donfuldauer/kilohertz-pinball/pull/61#discussion_r4625928006 (cli/mod.rs:656): "run_soak_command calls run_soak_recording even when no recording is requested. That call runs check_record_selection, which rejects non-default cfg.sim.lua, cfg.sim.wasm, and cfg.max_game_ticks above MAX_TICKS. run_soak accepts those settings. --max-ticks above 4294967296 with no recording now fails with the recording usage error... Call run_soak when record_games is empty. Keep run_soak_recording for recording requests."
   Fixed means: with no --record-games, the soak path is exactly the pre-PR run_soak path (no recording checks); a regression test shows `soak --max-ticks 4294967297` (or the library equivalent with non-default script limits) without recording is not rejected by the recording check, while the same with --record-games still is.
2. Thread ...#discussion_r4625928016 (cli/mod.rs:703): "write_recordings validates only --record-dir. Each generated child name, and recorded_games.tsv, is then written with std::fs::write, which follows an existing symlink... Before each write, check each target with symlink_metadata. Return an error when the target is a symlink (or remove the symlink entry without touching its target). Keep normal reruns that replace regular files unchanged."
   Fixed means: no recording write and no index write follows a symlink; choose "error out" (safer; record the choice in your report); a unix-only (#[cfg(unix)]) test plants a symlink named game_000000.khzrec (and one for recorded_games.tsv) pointing at a file outside, runs a recording soak, and asserts the target file is unchanged and the run fails with a clear message; rerunning over existing regular files still works.
3. Review-body nitpick (marked unverified), autoplay_record_game.rs:180: tests leave their scratch roots behind; use a guard that removes each scratch root on scope exit (Drop), including on panic, for all scratch dirs in this file. Verify first that roots are left behind; if the shared helper already cleans up, report not-a-defect.
Report each thread URL and what changed. Do not reply to or resolve threads.

RULES: PLAN §2.3 rules 1-9 (module boundaries; no pin_core change; no change under crates/pin_sim/src/{sim,tick,hash,load,alloc_guard,commands}; GPU-free tests; no tick allocation; simulated time; no test-shaped special cases; never wait on a person; no self-review). Soak output must stay unchanged.

COMMANDS: env CARGO_BUILD_JOBS=1 CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/gate-a2-cr1 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_TEST_DEBUG=line-tables-only (df -h /home/user first; stop under 3 GB; two investigators share the cores):
  cargo fmt --check
  cargo clippy -p pin_sim --all-targets -- -D warnings
  cargo test -p pin_sim -- --test-threads=1
DONE WHEN: the three fixed conditions hold (or not-a-defect with evidence); commands pass; diff only in OWNED PATHS.
REPORT FORMAT: status; per requirement met/not-a-defect + evidence; gate results; thread URL -> change; decisions logged (none expected).
