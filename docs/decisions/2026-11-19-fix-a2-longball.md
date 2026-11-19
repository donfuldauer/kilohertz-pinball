# Decisions: fix-a2-longball (Wave 7, Gate A2 run 1 follow-up)

## fix-a2-longball — LongBall reporting for drained long balls

**Decision:** This partly supersedes item 1 of DECISIONS "A1-endless-detector" (EndlessBall fails
the soak at 300 s or at the cap). The 300 s age limit (`SoakConfig::endless_ball_ticks`) stays. A
ball that passes it is not reported at once: the monitor holds the finding as it would be raised
today (same tick, subject and detail) for that ball slot, with a verdict on the 60 s switch window
at that tick. The window is broad when at least `LONG_BALL_MIN_SWITCHES` = 8 distinct non-cabinet
switches were credited and the two most-hit hold strictly under `LONG_BALL_TOP2_MAX_PERCENT` = 50 %
of the window's hits (integer comparison `100 * top_two < 50 * total`). If the ball later drains
naturally (its handle vanished and `balls_escaped` did not rise for it) after a broad window, the
finding becomes the new `FindingKind::LongBall`: reported (JSON name `LongBall`, Markdown section,
`findings_by_kind`, a `REPORT` row in `scripts/gate_a2_verdict.py`), never failing the soak, with
its original tick and the detail plus a drain suffix. Capped, still on the table at game end,
escaped, non-finite, stuck, and narrow-window cases remain EndlessBall, raised when that case is
seen, with the original tick and detail. `Report::passed()`, Gate A1 `no_endless_balls` (EndlessBall
only) and Gate A2 criterion A2.1a (`BALL_KINDS` unchanged) keep their meaning. `gate_a1` gains no
LongBall check (left alone so no check name or order that `a1_gate_verdict.rs` or
`scripts/gate_a1.sh` parse changes); LongBall shows in `findings_by_kind`. The plunger-switch
discriminator of the brief's requirement 8 was not added: measured on the Gate A2 run-1 games, the
monitor's own window data does not separate Feature Lab's shooter-lane re-entry balls from Apex
Circuit's long balls (see Rationale), so the rule is requirement 2's and the Orchestrator decides
about Feature Lab (docs/queue/fix-a2-longball.md).

**Rationale:** PLAN Gate A2 criterion 1 names stuck, escaped and non-finite balls only, and Gate A1
criterion 3 likewise; "endless" is a 2.9 detector. The 300 s rule is the A1 grey-box trap
heuristic (median ball about 15 s there), while A2.3 allows a median game up to 6 min. Evidence
from the three Apex Circuit balls of Gate A2 run 1 (seed 1; standard 156, expert 450 and 818):
21-24 distinct switches per 60 s window, top two 20-31 % of hits, 132-173 flips, flat energy, and
each drained naturally 7-50 s after the flag; a constant drain rate of about 0.021-0.026 /s
predicts about 1.8 / 4.7 balls over 300 s per 3,000 games, so they are the tail of an ordinary
ball-time distribution. Re-judged with this rule, all three are LongBall with unchanged end ticks
(435565 / 455673 / 351438). Feature Lab's 11 run-1 EndlessBalls (shooter-lane re-entry from the
top, relaunched by the bot) have 13-15 distinct switches, top two 24-43 % and plunger_tip 7-28 per
window; Apex's have plunger_tip 7-14 and shooter_lane 2-8 (the Apex left orbit returns to the
shooter lane by design, and plunger_tip has no position, so it is credited to every ball). Expert
450 (Apex) and expert 793 (Feature Lab) both show plunger_tip x7 / shooter_lane x2, so neither the
launch-switch count, the shooter-lane count nor their share separates the sets; distinct-switch
counts are 70-80 % of each table's coverage switches in both. Residual risk: a real trap that
breaks by chance after more than 300 s with a broad window is downgraded to LongBall. Mitigation:
the verdict script still reports the LongBall count per table and skill, and a nonzero LongBall
rate far above the tail estimate (about 0.1-0.2 % of games) deserves a look.

**Alternatives:** Make Apex Circuit harder (about 40 % shorter balls pushes the standard game
length toward the 60 s floor); retune the bot (no window satisfies all tables); raise the limit to
the cap with no drain test (a 10 min trap that drains would pass silently); a per-table limit (a
table-shaped special case); a launch-switch discriminator (3 or more plunger-switch hits in the
window make it narrow): implemented, measured and removed, because it also makes all three Apex
balls narrow (plunger_tip x7-x14).
