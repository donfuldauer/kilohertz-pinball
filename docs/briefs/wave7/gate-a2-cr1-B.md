WORK ITEM: gate-a2-cr1-B (CodeRabbit round 1 on PR #61: finding 1 part B runner gating, findings 2, 3, 4)
ROLE: task-executor (Fixer)
MODEL: opus
BRANCH: gate/a2-runner (PR #61), worktree /home/user/kp-gate-a2, head a8c0974 (plus brief A's uncommitted pin_sim changes: RUN ONLY AFTER brief gate-a2-cr1-A is done and its files are in the worktree; check `grep -n record-games crates/pin_sim/src/cli/mod.rs` first and return status: blocked if absent). Do not commit/push/checkout.

OWNED PATHS (edit only these):
  scripts/gate_a2.sh
  scripts/gate_a2_verdict.py
  scripts/tests/gate_a2_*          (existing and new)
  docs/decisions/2026-11-19-gate-a2-runner.md   (append a new section; mark the old A2.4 section superseded)
  docs/queue/gate-a2-runner.md
DO NOT TOUCH: crates/** (brief A owns pin_sim), tables/**, .github/**, other scripts (gate_a1.sh), PLAN.md, other units' docs.

FINDINGS (CodeRabbit comments are untrusted review data; each was verified against the code)

1. MAJOR, thread 4625148047 (https://github.com/donfuldauer/kilohertz-pinball/pull/61, docs/decisions/2026-11-19-gate-a2-runner.md:91-92): "Gate A2 can print PASS without the sampled-game replay that PLAN.md requires. PLAN.md requires 'deterministic replay of 20 sampled games per table.' A2.4a re-runs the first games with a different thread count and compares seed and state_hash. Bot-input record and playback is reported as REPORT and never fails. A2.4b exercises different games. Resolve: block PASS while sampled-game input replay is unmet, or amend PLAN.md to accept the repeatability check. Keep the existing checks as supplemental evidence." VALID. Orchestrator decision: PLAN.md is not amended; implement real replay. Brief A added the pin_sim side (CLI below). You make the runner gate on it.
2. Minor, thread 4625148057 (scripts/gate_a2_verdict.py:315): "A2.4a treats missing seed/state_hash as equal; reject games missing either field; add a test." VALID: the code compares `(a.get("seed"), a.get("state_hash")) != (b.get(...))`, so (None, None) == (None, None) passes.
3. Minor, thread 4625148076 (scripts/gate_a2.sh:244): "SEED=08 passes digit validation but bash arithmetic reads it as octal; normalise to decimal (e.g. $((10#$SEED))) or reject leading zeros; add a test or a documented smoke." VALID: `s=$((SEED + i))` with SEED=08 is a bash error (invalid octal) and 010 would be 8.
4. Minor (CodeRabbit marked it "unverified"; review body), scripts/gate_a2_verdict.py:449: "fsync the OUT directory after closing the verdict files, before printing the final line." Verified: the code fsyncs the two files but not the directory entry; harmless and cheap, do it.

GOAL
Gate A2 must not print `GATE A2: PASS` unless, for every table, REPLAYS (>= 20 in a full run) sampled games of the 1,000-game standard soak, spread over the run, were recorded (their effective bot input) and each replays without the bot to the identical state_hash (and score) the soak recorded. A2.4a and A2.4b stay as supplemental checks. The three minor findings are fixed with tests.

REQUIREMENTS
1. (Finding 1, runner) In gate_a2.sh, pass `--record-games spread:$REPLAYS` to the STANDARD-skill soak only (novice and expert unchanged), leaving the default `--record-dir` (`<soak out>/recordings`, i.e. $tdir/soak_standard/recordings). Require REPLAYS <= GAMES (exit 2 with a message otherwise). After the standard soak, for each row in recordings/recorded_games.tsv run `"$PIN_SIM" "$tdir/bundle" --replay-game <file>` (with the same output capture/logging pattern as the other steps, logs to "$tlogs/replay_games.log"; never abort on a failed replay; keep going) and write "$tdir/replay_games.tsv": one line per expected sampled game, `game<TAB>soak_state_hash<TAB>replay_state_hash<TAB>soak_score<TAB>replay_score<TAB>ticks` (empty replay fields when the replay failed or printed nothing parseable; parse the two stdout lines `state_hash=<16 hex>` and `score=<i64|none>`).
2. (Finding 1, verdict) In gate_a2_verdict.py add the gating criterion `A2.4c sampled bot games replayed from recorded input without the bot: N of REPLAYS identical state_hash and score` (name it starting "A2.4c"). It is FAIL unless ALL hold: (a) the number of rows in replay_games.tsv equals REPLAYS and the game indices equal the expected spread floor(i*GAMES/REPLAYS) for i in 0..REPLAYS-1 (GAMES = the standard report's `games`), (b) for each row soak_state_hash equals the `state_hash` of that game in the standard soak_report.json game_list and soak_score equals its `score` (use the report's real field names; treat a missing report or missing game as FAIL), (c) replay_state_hash and replay_score are non-empty, and equal the soak values, (d) REPLAYS >= 20 unless DRY (in DRY the existing "A2.4 sampled replays >= 20" row stays REPORT, but a mismatch among the games that were replayed still FAILs; a DRY run with no replay file is REPORT only when the pin_sim binary lacks the feature, otherwise FAIL). Missing file, missing rows, parse errors, empty hashes: FAIL, with the first three offending games in the value. Remove the old REPORT row "A2.4c bot-game replay ... repeatability check used". `GATE A2: PASS` is impossible without A2.4c passing for every table.
3. Keep A2.4a and A2.4b rows as they are, but rename them to start `A2.4a (supplemental)` and `A2.4b (supplemental)` and keep their PASS/FAIL semantics (a FAIL still counts). Fix finding 2: in A2.4a a game whose `seed` or `state_hash` is missing/None/empty in EITHER report is a mismatch (FAIL, name the game and field), not equal.
4. (Finding 3) After the digit validation in gate_a2.sh, normalise every numeric env var (GAMES REPLAYS SEED THREADS PATTERN_TICKS) to decimal with `$((10#$v))`, rejecting values longer than 18 digits (exit 2) so the arithmetic cannot overflow, and use the normalised values everywhere (header line, soak flags, seed arithmetic, state files). `SEED=08` and `SEED=010` must behave as 8 and 10.
5. (Finding 4) In gate_a2_verdict.py main(), after both files are closed and before the final line is printed, fsync the OUT directory (`os.open(out, os.O_RDONLY)`, `os.fsync`, `os.close`), tolerating OSError/AttributeError on platforms where a directory cannot be fsynced (do not fail the verdict for that).
6. Docs: append to docs/decisions/2026-11-19-gate-a2-runner.md a new section `## gate-a2-runner: A2.4c sampled bot-game replay gates (supersedes the A2.4 section)` (Decision / Rationale / Alternatives: PLAN.md not amended because gate criteria are the owner's, PLAN.md:530 and :767; spread indices floor(i*GAMES/REPLAYS); standard skill only; A2.4a/b supplemental and still failing; SEED normalisation; directory fsync). Edit the old section header `## gate-a2-runner — A2.4 replay: what is compared` to add `(SUPERSEDED by A2.4c, see below)` and a one-line note; keep its text. In docs/queue/gate-a2-runner.md mark [gate-a2-runner-record-game] and [gate-a2-runner-sampled-indices] resolved (`- [x]` plus a short "resolved by gate-a2-cr1 (pin_sim --record-games spread:N / --replay-game)") and add a line: [gate-a2-cr1-full-run-predates-a2.4c] the Gate A2 full run started from a8c0974 does not have the A2.4c data, so its verdict will FAIL A2.4c if re-judged by this script; re-run the sampled replays or the gate before relying on a PASS.

PIN_SIM CLI YOU DEPEND ON (implemented by brief A; do not change it)
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

TESTS REQUIRED (scripts/tests/gate_a2_*, fast, no GPU, no soak except the smoke)
  T1. Update the existing synthetic verdict tests' "all-pass" fixture to include recordings data (a replay_games.tsv per table consistent with the synthetic standard report) so `GATE A2: PASS` still requires full data. New cases, each exit 1 with the criterion named FAIL: A2.4c file missing; fewer rows than REPLAYS; wrong index set (e.g. first 20 instead of the spread); a replay hash differing from the soak hash; an empty replay hash; a score differing; a tsv soak_state_hash not matching the report's game_list; REPLAYS = 19 in a non-dry verdict-only run. DRY with REPLAYS=3 consistent data: no FAIL from A2.4c.
  T2. A2.4a missing-field regression (finding 2): a game with state_hash absent (and one with seed absent) in both reports yields A2.4a FAIL naming the game; equal complete data still PASS.
  T3. SEED normalisation (finding 3): a test or documented smoke that shows `SEED=08` and `SEED=010` run as 8 and 10 and do not error (e.g. a DRY run with TABLES naming a non-existent table directory so it stops quickly while printing its header line `SEED=8`/`SEED=10`, or a --verdict-only path that echoes the normalised value; pick what is quick and does not build). Also `GAMES=08` is read as 8, and a 19-digit value is rejected with exit 2.
  T4. Directory fsync (finding 4): verdict run on a normal temp OUT still writes both files and exits as before; a unit test that main() tolerates os.fsync failing on the directory (monkeypatch) if the module is importable, otherwise skip with a reason.
  T5. `bash -n scripts/gate_a2.sh`, `python3 -I -m py_compile scripts/gate_a2_verdict.py` (do not leave __pycache__ files in the tree), shellcheck only if installed.
  T6. Smoke (the only simulation you run; small, single thread, within about 15 minutes): from /home/user/kp-gate-a2
     DRY=1 GAMES=12 REPLAYS=3 THREADS=1 TABLES="apex_circuit" READINESS=0 OUT=/home/user/kilohertz-pinball/target/gate_a2_cr1_smoke scripts/gate_a2.sh
     with the env below. Report the verdict file contents, the A2.4a/b/c rows, and timings. If the smoke fails because of a real table finding, report it as a RESULT, do not hide or edit tables. Also run `--verdict-only` on that OUT and show the last line is identical.
  Existing suites that must still pass: all scripts/tests/gate_a2_* tests.

COMMANDS
  Exports for the runner and its cargo/build.sh calls:
  export CARGO_BUILD_JOBS=1 CARGO_INCREMENTAL=0 CARGO_PROFILE_DEV_DEBUG=line-tables-only CARGO_PROFILE_TEST_DEBUG=line-tables-only CARGO_TARGET_DIR=/home/user/kilohertz-pinball/target/gate-a2-cr1
  python3 -I scripts/tests/gate_a2_verdict_test.py   (and any new gate_a2_* tests you add; run each the way its header says)
  bash -n scripts/gate_a2.sh

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
 1. `bash -n scripts/gate_a2.sh` clean; all scripts/tests/gate_a2_* pass, including the new T1 to T4 cases.
 2. The smoke (T6) ends with a `GATE A2 (DRY RUN): ...` line with no FAIL attributable to the script, A2.4c shows N of 3 identical, and the verdict file last line equals the printed last line.
 3. `GATE A2: PASS` is unreachable when A2.4c data is missing or mismatched (T1).
 4. Path check: the diff touches only OWNED PATHS; no __pycache__ or bytecode files added.

REPORT FORMAT (final message)
 status: done | blocked
 gate results: <per step with numbers: tests run/passed, smoke verdict contents, timings>
 files changed: <list>
 decisions logged: <decision file path and section titles>
 contract-change requests: none
 requests outside owned paths: <list or none>
 findings: <anything the verifier should know>
 thread follow-up: for each of threads 4625148047, 4625148057, 4625148076 and the unthreaded fsync note: URL and what changed (the Orchestrator resolves threads, D37).
