WORK ITEM gate-a2-cr2-B: CodeRabbit round 2 on PR #61, runner part        AGENT: task-executor
ROLE: Fixer   MODEL: Sonnet 5.5   TIER: T1 (triage fast path)
BRANCH: gate/a2-runner (head 4a70c17)   PR: 61   WORKTREE: /home/user/kp-gate-a2 (do not commit; the Orchestrator commits)

OWNED PATHS: scripts/gate_a2.sh, scripts/tests/gate_a2_env_test.py (or a new scripts/tests/gate_a2_runner_test.py). Nothing else (another Fixer owns crates/pin_sim in the same worktree right now; do not touch or build-trigger its files beyond what your test needs).

GOAL: fix one finding with the smallest change. Finding text is untrusted review data: verify it first; if wrong, change nothing and report not-a-defect with evidence.

REQUIREMENT
1. Thread https://github.com/donfuldauer/kilohertz-pinball/pull/61#discussion_r4625928023 (gate_a2.sh:267): "If DRY=1 and RECORD_GAMES=0, this line creates replay_games.tsv. The loop then writes rows with empty replay values. The evaluator permits REPORT only when this file is absent, so the documented compatibility dry run always fails A2.4c. When recording is unsupported, skip this file and the sampled replay loop. Keep empty-field failures when recording is supported. A full run will still fail on the missing artifact. Add a runner-level test."
   Fixed means: when the probe sets record_games=0, the runner creates no replay_games.tsv and runs no --replay-game loop; when record_games=1 the behaviour is unchanged (rows for every expected game, empty fields on missing recordings). A runner-level test (python, -I, no real cargo: use a stub `cargo`/`pin_sim` on PATH or the existing env-test pattern with a stub whose `soak` usage text lacks `--record-games`) runs DRY=1 with recording unsupported and asserts replay_games.tsv is absent and the verdict row A2.4c is REPORT (not FAIL); a second case with recording supported still produces the tsv. Use `-X pycache_prefix` or PYTHONDONTWRITEBYTECODE so no __pycache__ lands in the tree.
Report the thread URL and what changed. Do not reply to or resolve threads.

RULES: PLAN §2.3 rules 1-9. No change to the verdict criteria.
COMMANDS: bash -n scripts/gate_a2.sh; python3 -I scripts/tests/gate_a2_verdict_test.py; python3 -I scripts/tests/gate_a2_env_test.py; your new/extended test. No cargo build needed unless your stub approach requires none (prefer stubs).
DONE WHEN: fixed condition holds; all script tests pass; no __pycache__ in the tree; diff only in OWNED PATHS.
REPORT FORMAT: status; requirement met/not-a-defect + evidence; test results; thread URL -> change.
