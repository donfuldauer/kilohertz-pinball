# Decisions — gate-a2-cr1 (pin_sim side): recorded bot games

Context: CodeRabbit round 1 on PR #61, thread 4625148047. PLAN.md requires
"deterministic replay of 20 sampled games per table" (Gate A2), and PLAN.md
defines replay as record/replay of input (Subphase 2.5). The Orchestrator
did not amend PLAN.md, so pin_sim now records the effective input of
selected soak games and replays them without the bot. Brief B gates the
runner on it.

## gate-a2-cr1: bot-game recording format and hook point

**Decision.** A new file format and type, `pin_sim::replay::GameRecording`
(`crates/pin_sim/src/replay/game.rs`), holds what the bot *applied*, not
input events: a stream of `(tick, GameOp)` where `GameOp` is
`SetSwitch { input: CabinetInput, closed }`, `ReleaseSwitch { input }` or
`Nudge { dv: [f32; 3] }`, with the tick it was applied before
(`sim.tick() + 1`). Replay (`replay_game`, `GameRecording::play`) checks the
table hash first, loads the bundle with the recorded seed and
`SimConfig::default()`, and for each tick applies that tick's operations in
order through the same calls the bot uses (`Simulation::set_switch`,
`Simulation::release_switch`, `sim.mechs.nudge(dv)`), then `advance(1)`,
with no bot. File: magic `KHZGAME\0` (distinct from the input recording's
`KHZREPL\0`), version 1, seed, table hash, ticks, op count, 24-byte ops, an
FNV-1a 64 checksum trailer, the existing `MAX_TICKS` bound; it reuses
`ReplayError` and the private `read_capped` / `u64_at` / `u32_at` helpers,
and parsing is bounded and never panics. The existing `Recording` format,
`--record-inputs` and `--replay-inputs` are unchanged.

The hook point is `Bot::tick_recorded<S: GameOpSink>(&mut self, sim, sink)`;
`Bot::tick` is `tick_recorded(sim, &mut ())`. The bot reports an operation
right after the call succeeds (nudge in `update_nudge`, switch changes in
`apply`), in the order it applies them (nudge first, then switches in
`CabinetInput::ALL` order). The sink never feeds back into a decision. The
soak's `play_game` is generic over the sink: an unselected game runs the
`()` instance, which has no extra branch, allocation or behaviour (the
`Instant` timing stays around `advance(1)` only); a selected game (a
binary search per game, not per tick) runs the `GameRecording` instance,
whose `Vec` push happens between ticks, never inside `advance`/`step_1ms`.
`run_soak_recording(bundle, cfg, &[games])` returns the unchanged report plus
`(game, GameRecording)` pairs; `run_soak` is that with no selection.
`check_record_selection` refuses (before anything runs) an index not below
`games`, unsorted or repeated indices, non-default script limits in
`cfg.sim` (a recording stores only the seed) and `max_game_ticks` above
`MAX_TICKS`. The CLI (`--record-games SPEC [--record-dir DIR]` on `soak`,
`<bundle> --replay-game FILE`) follows the PIN_SIM CLI CONTRACT of the
brief. `--record-dir` without `--record-games` is a usage error (the
contract does not define it; refusing is the conservative reading).
Recordings and `recorded_games.tsv` are written after the reports, before
the pass/fail and `--gate a1` verdicts, so they exist when the soak fails.

**Rationale.** The fact the brief flags holds: the bot does not feed
`InputEvent`s. `set_switch` sets `forced_mask`/`forced_val` and calls
`update_switches(self.tick)` immediately, between ticks; the override
persists until `release_switch` and takes precedence over the cabinet
input bits in the tick (`tick/mod.rs`, forced switches). A button
`InputEvent` instead goes through `pin_input` ingestion in tick step 1 of
the *next* tick, sets the unforced cabinet bits, and leaves `forced_mask`
untouched, so both the edge timing and the stored state differ. Nudges are
`sim.mechs.nudge(dv)` between ticks, while a `NudgeX` axis event is ingested
as an axis value in step 1. The bot's game therefore cannot be expressed as
an equal-hash `Recording` in general, and no test was attempted to prove
otherwise (the brief prefers exactness over reuse). Recording the applied
operations and re-applying them through the same calls at the same tick
boundaries is exact by construction: the only inputs to the simulation in
a soak game are the load seed, these calls and `advance(1)`; everything else
the bot does is reading. Tests (`tests/autoplay_record_game.rs`) prove
equality of state hash, score and ticks for all three skills, two seeds
and 1/2 threads, through the library and the CLI, and that dropping or
shifting one operation changes the hash.

**Alternatives.** (1) Reuse `Recording` with button/axis events: not exact
(above). (2) Make the bot feed `InputEvent`s: changes bot behaviour, every
soak hash and Gate A1 evidence; out of scope. (3) A `record: Option<Vec<_>>`
field inside `Bot`: a per-tick branch in every soak and a `Bot` that owns
tooling state; the generic sink keeps the unrecorded path byte-for-byte the
old code. (4) Store the expected end state in the file: rejected, the
`recorded_games.tsv` written from the soak's `GameSummary` is the reference,
so the replay cannot check itself against its own claim. (5) A new
`ReplayError` variant set for game files: unneeded; the existing variants
describe every failure, and the enum is unchanged.
