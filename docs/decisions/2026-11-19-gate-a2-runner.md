# Decisions: gate-a2-runner (Wave 7, Gate A2 table-soak runner)

## gate-a2-runner — where thresholds are judged

**Decision:** Gate A2 thresholds are evaluated in the runner, from the `soak_report.json` files
(`scripts/gate_a2_verdict.py`, called by `scripts/gate_a2.sh`), not by a new `pin_sim soak --gate a2`.
`pin_sim soak` runs without `--gate a1` (the grey-box timing gate) and without `--no-tick-budget`.
Its exit status is recorded as its own criterion (`A2.0 soak exit status [<skill>]`).

**Rationale:** An `a2` gate in pin_sim is an engine change outside this unit's owned paths. Rules
freeze after A2, and some criteria span three soaks (median score novice < standard < expert),
which a per-run `--gate` cannot judge anyway. The field names come from `SoakReport::to_json`
(crates/pin_sim/src/autoplay/report.rs, schema 1): `findings[].kind` (`FindingKind::name`),
`coverage.switches[].hits`, `coverage.shots[].count`, `coverage.modes[].starts/observable`,
`distributions.game_score.median`, `counters.*`, `games[]` (`game, seed, ticks, end, state_hash`).
The game-length median uses the nearest-rank rule of `report.rs` `nearest_rank` (rank ceil(q*n),
at least 1).

**Alternatives:** `pin_sim soak --gate a2` for the per-run checks. Rejected for now; queued as
a follow-up (docs/queue/gate-a2-runner.md).

## gate-a2-runner — verdict needs python3; jq only for the binary lookup

**Decision:** The verdict is computed by `python3 -I scripts/gate_a2_verdict.py OUT`. Cargo's
JSON is read with jq, or python3 when jq is missing, as in gate_a1.sh. When python3 is missing,
the script stops with a clear message and exits 1. It does not fall back to jq for the verdict.

**Rationale:** The brief asks that verdict logic be unit-testable on hand-made JSON (requirement 8).
It also asks that the verdict be rebuildable from artifacts alone (`--verdict-only`). A single
Python implementation serves the live run, the rebuild and the tests, so the three cannot drift.
A second jq implementation of the same thresholds would be a second source of truth.

**Alternatives:** The whole verdict in jq, with python3 as a fallback. Rejected because it
duplicates the logic.

## gate-a2-runner — the run's state lives in files under OUT

**Decision:** The live run writes everything the verdict reads into OUT: `gate_a2_run.txt`
(parameters), `<t>/build.rc`, `<t>/soak_<skill>.rc`, `<t>/replay_soak.rc`, `<t>/replay_pattern.tsv`
and `readiness.tsv`, next to the soak reports and the logs. It then runs the same verdict command
that `--verdict-only OUT` runs. A run first removes these status files and reports for its own
tables (files only, only under OUT), so stale artifacts are never judged. When `gate_a2_run.txt`
is missing, `--verdict-only` judges the table directories it finds as a full (non-dry) run.

**Rationale:** Gate A1 lost its final line in a long log. With the state in files, the verdict can
be rebuilt byte for byte without re-soaking.

**Alternatives:** Keeping results only in shell arrays, as gate_a1.sh does. Rejected because
that makes `--verdict-only` impossible.

## gate-a2-runner — verdict files

**Decision:** `OUT/gate_a2_verdict.txt` holds one `STATUS<TAB>table<TAB>criterion<TAB>value`
line per criterion, then the final line. The copy of the human table is in a separate file,
`OUT/gate_a2_table.txt`, which also ends with the final line. Both files are written, flushed
and fsynced before the final line is printed. Global rows use the table names `run` and
`readiness`.

**Rationale:** Keeping human table lines out of the TSV file keeps it machine-readable, and
both files still end with the final line.

**Alternatives:** One file with both formats. Rejected because a TSV parser would then have to
skip the human lines.

## gate-a2-runner — tables, INCOMPLETE and precedence

**Decision:** The default TABLES is every `tables/*/` with a `build.sh` except `greybox`,
sorted (LC_ALL=C) and de-duplicated. PLAN's "the three tables" are apex_circuit, feature_lab
and sky_deck. greybox runs only when it is named. When any of the three is missing from TABLES,
a full run reports `SKIPPED (<t> not in TABLES)` and ends with `GATE A2 INCOMPLETE: ...` and
exit 1. That line takes precedence over `GATE A2: FAIL` and includes the failed-criteria count,
so a gate that is missing a table never reads as a plain FAIL or a PASS. A dry run always ends
with `GATE A2 (DRY RUN): ...`.

**Rationale:** The brief requires INCOMPLETE when a table is missing. Naming both the missing
tables and the failures keeps all the evidence on the last line.

**Alternatives:** FAIL taking precedence over INCOMPLETE. Rejected because it hides the fact
that the gate is incomplete.

## gate-a2-runner — A2.4 replay: what is compared (SUPERSEDED by A2.4c, see below)

*Superseded by "A2.4c sampled bot-game replay gates" below: pin_sim can now record and replay sampled bot games, so (c) gates and (a)/(b) are supplemental. The text is kept as written.*


**Decision:** (a) The standard soak is re-run alone for games 0..REPLAYS-1 with the same seed
and a different thread count (1, or 2 when THREADS=1). Each game's `seed` and `state_hash`
must equal the same game in the full run. `pin_sim soak` cannot select game indices, so the
first REPLAYS games are used rather than games spread over the run, and the verdict says so.
(b) For REPLAYS distinct seeds (SEED, SEED+1, ...), the built-in input pattern is recorded for
PATTERN_TICKS (default 60000) ticks with `--record-inputs` and replayed with `--replay-inputs`.
Both `state_hash=` values must be equal and non-empty. (c) True bot-input capture and playback
(docs/queue/6.2-cr1.md; spec_6_2_soak.rs:187 and :235) needs a pin_sim change
(`soak --record-game N`). It is reported as "REPORT: bot-input record/playback not available;
repeatability check used" and is not gating.

**Rationale:** These are the mechanisms that exist today. No engine hooks are added.

**Alternatives:** Wait for a `--record-game` option. Rejected because the change is outside
this unit, and it is queued.

## gate-a2-runner — readiness tests in a dry run

**Decision:** The Sky Deck readiness tests run only when sky_deck is in TABLES. Each runs as
`cargo test --release -p pin_app --test <target> -- [--ignored] --exact --nocapture <name>`.
`--ignored` is passed only for the two `#[ignore]` tests (r6c, c6a). The c6c tests are not
ignored, and with `--ignored` they would run zero tests. A test counts as PASS only when cargo
exits 0 and the log says `test result: ok. 1 passed`, so a name filter that matches nothing
cannot pass. The ignored tests have fixed game counts (200 and 40 per skill), so their
"minimum" is the whole test. In DRY mode they are SKIPPED with that reason, and the c6c tests
still run. A missing target is FAIL in a full run and REPORT (missing, dry run) in DRY mode.
READINESS=0 is refused unless DRY=1.

**Rationale:** This follows the brief's dry-run rule ("use the minimum their tests allow or are
skipped with a printed reason") and its presence rule.

**Alternatives:** Running the ignored soaks in dry runs as well. Rejected because they take
many minutes and defeat the purpose of a smoke run.

## gate-a2-runner — modes that are not observable; table-specific REPORT lines

**Decision:** A `[[modes]]` entry with `observable == false` (no `mode:<id>` persist key) is
listed as a REPORT row, `A2.2c modes not observable ... [<skill>]`, and its ModeCoverage finding
does not fail A2.2c. `pin_sim soak` still exits 1 for it, and that exit is judged by
`A2.0 soak exit status`. The only table-name special cases are the ones the brief names, all of
them REPORT: apex_circuit "not soak-covered ... Firewall Save, Apex Jackpot" [6.1-a2-modes] and
apex_circuit `drop_bank` "may be over-credited" [6.1-drop-bank-soak]. The thinnest-three-shots
line at expert ([6.1-expert-orbits]) is printed for every table.

**Rationale:** The brief, requirement 4c, and PLAN §2.3 rule 6.

**Alternatives:** None considered.

## gate-a2-runner: A2.4c sampled bot-game replay gates (supersedes the A2.4 section)

**Decision:** PLAN.md's "deterministic replay of 20 sampled games per table" is met by real
input replay, and it gates. The STANDARD-skill soak (novice and expert are unchanged) runs with
`--record-games spread:$REPLAYS`, so the bot's effective input is recorded for games
floor(i*GAMES/REPLAYS), i = 0..REPLAYS-1 (0, 50, ..., 950 for 20 of 1,000), into the default
`<soak out>/recordings`. Each recording is then replayed with
`pin_sim <bundle> --replay-game FILE`, with no bot. The runner writes `<t>/replay_games.tsv`, one
row per expected spread game (game, soak state_hash, replay state_hash, soak score, replay score,
ticks). A game that has no recording or no parseable replay output keeps empty fields. The
verdict row `A2.4c sampled bot games replayed from recorded input without the bot: N of REPLAYS
identical state_hash and score` FAILs unless all of the following hold:
- the rows are exactly the spread indices, using GAMES from the standard report's `config.games`;
- each row's soak hash and score equal that game's `state_hash` and `score` in the standard
  soak_report.json `games` list, where a `null` score is written `none`;
- each replay hash and score is well-formed, non-empty and equal to the soak value;
- REPLAYS >= 20 in a full run.

In a dry run, REPLAYS < 20 does not fail A2.4c, but a mismatch does. A dry run with no
replay file is a REPORT only when the runner recorded `record_games=0` in `gate_a2_run.txt`,
meaning the binary's `soak` usage text has no `--record-games`. Otherwise it FAILs. A2.4a and
A2.4b are renamed `A2.4a (supplemental)` and `A2.4b (supplemental)`. They keep their PASS/FAIL
semantics, so a FAIL in either still blocks PASS. In A2.4a, a seed or state_hash that is missing,
null or empty in either report is now a mismatch (CodeRabbit finding 2). The old non-gating
"A2.4c bot-game replay ... repeatability check used" REPORT row is removed.

The numeric env vars GAMES, REPLAYS, SEED, THREADS and PATTERN_TICKS are normalised to decimal
with `$((10#v))` after digit validation, and values longer than 18 digits are refused with
exit 2. The normalised values are used everywhere: in the header, in gate_a2_run.txt, in the
soak flags and in the seed arithmetic. `SEED=08` is 8 and `SEED=010` is 10 (finding 3). The
verdict fsyncs the OUT directory after both verdict files are closed and before the final line
is printed, and it tolerates platforms that cannot fsync a directory (finding 4).

**Rationale:** Gate criteria are the owner's (PLAN.md:530, :767), so PLAN.md is not amended to
accept a repeatability check in place of replay. The pin_sim side exists now
(docs/decisions/2026-11-19-gate-a2-cr1-pin-sim.md). The spread formula is the one pin_sim
uses for `spread:N`, so the runner and the verdict compute the same indices independently, and a
recording of the wrong games fails. The standard skill is enough because PLAN asks for "20
sampled games per table", not per skill, and the A2.3a median is judged on the standard soak.
Cross-checking the tsv against soak_report.json catches a recording whose listed soak values are
not the soak's own. The supplemental checks stay because they cover other risks (thread-count
independence, the `--record-inputs` path) at little cost. The runner writes one row per
*expected* game, so a recording that is missing shows up as an empty-field row, and it is named
in the verdict, instead of the row count silently shrinking.

**Alternatives:**
- Amend PLAN.md to accept the repeatability check. Rejected by the Orchestrator's decision: the
  criterion belongs to the owner.
- Record at every skill, or 20 games per skill. Rejected: this triples the recording I/O, and
  PLAN asks for 20 per table.
- Gate A2.4c in DRY on REPLAYS >= 20. Rejected because it conflicts with the dry-run rule that
  size criteria are REPORTed.
- Reject leading zeros instead of normalising them. Rejected because `SEED=08` is a reasonable
  thing to type and the brief asks for the decimal reading.
- Write replay_games.tsv rows only for the games that were actually recorded. Rejected because a
  missing recording would then show up only as a row count.
