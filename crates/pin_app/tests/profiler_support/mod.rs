//! Shared harness of the 7.1 determinism soak (`profiler_soak.rs`) and the
//! profiler integration tests: the three shipped tables' bundles (built by
//! the existing 6.x support modules, included by path), a closed-loop
//! scripted player that produces a recorded input stream, a scheduled
//! extra-ball injection for the single-ball Apex Circuit, and the replay /
//! profiled-replay runners.
#![allow(dead_code)]

#[path = "../apex_support/mod.rs"]
pub mod apex;
#[path = "../feature_lab_support/mod.rs"]
pub mod feature_lab;
#[path = "../sky_deck_support/mod.rs"]
pub mod sky_deck;

use pin_app::profiler::{ProfiledSim, Profiler, ProfilerSnapshot};
use pin_core::{CabinetInput, FrameStateSnapshot, InputEvent, SimStep, TICK_NS};
use pin_sim::autoplay::{FlipperModel, TableInfo};
use pin_sim::replay::{Recorder, Recording};
use pin_sim::{SimConfig, Simulation};
use std::path::Path;

/// A shipped, non-greybox table.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Table {
    /// `tables/apex_circuit` (6.1).
    Apex,
    /// `tables/sky_deck` (6.2).
    SkyDeck,
    /// `tables/feature_lab` (6.4).
    FeatureLab,
}

impl Table {
    /// Every table in scope, in report order.
    pub const ALL: [Table; 3] = [Table::Apex, Table::SkyDeck, Table::FeatureLab];

    /// The table's directory name (the `table=` field of `SOAK_HASH`).
    pub fn name(self) -> &'static str {
        match self {
            Table::Apex => "apex_circuit",
            Table::SkyDeck => "sky_deck",
            Table::FeatureLab => "feature_lab",
        }
    }

    /// The built bundle (once per test binary).
    pub fn bundle(self) -> &'static Path {
        match self {
            Table::Apex => apex::apex_bundle(),
            Table::SkyDeck => sky_deck::sky_deck_bundle(),
            Table::FeatureLab => feature_lab::bundle(),
        }
    }

    /// Whether the table's rules never put a second ball in play (Apex
    /// Circuit serves one ball at a time and has no multiball), so the
    /// harness injects one (decision `7.1 — Apex multiball`).
    pub fn single_ball_rules(self) -> bool {
        self == Table::Apex
    }
}

/// Runs `f` on a thread with an 8 MiB stack.
pub fn on_big_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    apex::on_big_stack(f)
}

/// `sin` and `cos` of `x` (|x| ≤ π) by Taylor series: plain `+` and `*`
/// (no platform libm), so the player's decisions are the same on every OS.
fn sin_cos(x: f32) -> (f32, f32) {
    let x2 = x * x;
    let mut s = x;
    let mut c = 1.0f32;
    let mut ts = x;
    let mut tc = 1.0f32;
    for k in 1..10 {
        let k = k as f32;
        ts = -ts * x2 / ((2.0 * k) * (2.0 * k + 1.0));
        tc = -tc * x2 / ((2.0 * k - 1.0) * (2.0 * k));
        s += ts;
        c += tc;
    }
    (s, c)
}

/// One flipper the player watches: pivot-relative sector test without
/// transcendental calls.
#[derive(Copy, Clone, Debug)]
struct Watched {
    model: FlipperModel,
    button: CabinetInput,
    /// Bat direction at rest and at the end of the sweep (widened by a
    /// margin), table plane.
    rest: [f32; 2],
    end: [f32; 2],
    reach: f32,
}

fn cross(a: [f32; 2], b: [f32; 2]) -> f32 {
    a[0] * b[1] - a[1] * b[0]
}

impl Watched {
    fn new(model: FlipperModel, button: CabinetInput) -> Self {
        let margin = 0.12f32;
        let sweep = model.sweep_rad.abs() + margin;
        let sgn = if model.sweep_rad >= 0.0 { 1.0 } else { -1.0 };
        // Rest direction turned back by the margin, end turned forward.
        let r0 = [model.rest_dir.x, model.rest_dir.y];
        let (s, c) = sin_cos(-sgn * margin);
        let rest = [c * r0[0] - s * r0[1], s * r0[0] + c * r0[1]];
        let (s, c) = sin_cos(sgn * sweep);
        let end = [c * r0[0] - s * r0[1], s * r0[0] + c * r0[1]];
        Self {
            model,
            button,
            rest,
            end,
            reach: model.reach(),
        }
    }

    /// Whether a ball centre at `q` (table plane) is inside the widened
    /// sweep sector of the bat pivoting at `pivot`.
    fn in_sector(&self, pivot: [f32; 2], q: [f32; 2]) -> bool {
        let d = [q[0] - pivot[0], q[1] - pivot[1]];
        let r2 = d[0] * d[0] + d[1] * d[1];
        if !(r2.is_finite() && r2 <= self.reach * self.reach) {
            return false;
        }
        let sgn = if self.model.sweep_rad >= 0.0 {
            1.0
        } else {
            -1.0
        };
        sgn * cross(self.rest, d) >= 0.0 && sgn * cross(d, self.end) >= 0.0
    }
}

/// Button hold and cooldown, ticks.
const FLIP_HOLD: u64 = 110;
const FLIP_COOLDOWN: u64 = 140;
/// Look-ahead of the flip decision, s, in 2 ms steps.
const LOOKAHEAD_S: f32 = 0.024;

#[derive(Copy, Clone, Debug, Default)]
struct Button {
    pressed: bool,
    release_at: u64,
    ready_at: u64,
}

/// A closed-loop scripted player: it reads the public sim state before each
/// tick and returns that tick's input events (start when no game runs,
/// a launch every 2.5 s, flips when a ball is in or about to enter a bat's
/// sweep, ACTION every 7 s and as soon as a persist key `overdrive_ready`
/// reads 1). Its output is what gets recorded; replays never run it.
pub struct Player {
    watched: Vec<Watched>,
    buttons: [Button; CabinetInput::COUNT],
    game_state: Option<u16>,
    overdrive_ready: Option<u16>,
    gravity: [f32; 2],
}

impl Player {
    /// A player for `sim` (just loaded) on the table described by `info`.
    pub fn new(sim: &Simulation, info: &TableInfo) -> Self {
        let watched = info
            .flippers
            .iter()
            .filter_map(|f| {
                let button = f.button?;
                let m = FlipperModel::infer(sim, f.id, Some(info))?;
                Some(Watched::new(m, button))
            })
            .collect();
        let key = |n: &str| sim.ids().get(pin_assets::IdKind::PersistKey, n);
        let g = sim.solver().config().gravity;
        Self {
            watched,
            buttons: [Button::default(); CabinetInput::COUNT],
            game_state: key("game_state"),
            overdrive_ready: key("overdrive_ready"),
            gravity: [g.x, g.y],
        }
    }

    fn set(&mut self, out: &mut Vec<InputEvent>, ts: u64, i: CabinetInput, pressed: bool) {
        let b = &mut self.buttons[i.index()];
        if b.pressed != pressed {
            b.pressed = pressed;
            out.push(InputEvent::button(ts, i, pressed));
        }
    }

    /// Presses `i` for `hold` ticks from `t` (when not already pressed and
    /// past its cooldown).
    fn tap(&mut self, out: &mut Vec<InputEvent>, t: u64, i: CabinetInput, hold: u64, cool: u64) {
        let b = self.buttons[i.index()];
        if !b.pressed && t >= b.ready_at {
            self.buttons[i.index()].release_at = t + hold;
            self.buttons[i.index()].ready_at = t + hold + cool;
            self.set(out, t * TICK_NS, i, true);
        }
    }

    /// The input events of the next tick (`sim.tick() + 1`), into `out`.
    pub fn inputs(&mut self, sim: &Simulation, out: &mut Vec<InputEvent>) {
        out.clear();
        let t = sim.tick() + 1;
        let ts = t * TICK_NS;
        // Releases due this tick.
        for i in CabinetInput::ALL {
            let b = self.buttons[i.index()];
            if b.pressed && t >= b.release_at {
                self.set(out, ts, i, false);
            }
        }
        let in_game = self.game_state.is_some_and(|k| sim.persist(k) == 1);
        if !in_game && t % 3000 == 100 {
            self.tap(out, t, CabinetInput::Start, 40, 0);
        }
        if t % 2500 == 1600 {
            self.tap(out, t, CabinetInput::Launch, 40, 0);
        }
        let armed = self.overdrive_ready.is_some_and(|k| sim.persist(k) == 1);
        if t % 7000 == 3000 || armed {
            self.tap(out, t, CabinetInput::Action, 40, 500);
        }
        let mut want = [false; CabinetInput::COUNT];
        for h in sim.ball_handles() {
            let Some(b) = sim.ball(h) else { continue };
            let (p, v) = (b.position_m, b.velocity_m_s);
            for w in &self.watched {
                if w.model.layer != b.layer || want[w.button.index()] {
                    continue;
                }
                let Some(pv) = sim.flipper_set().pivot_m(w.model.id) else {
                    continue;
                };
                let pivot = [pv.x, pv.y];
                let hit = (0..=12).any(|k| {
                    let s = k as f32 * 0.002;
                    let q = [
                        p[0] + v[0] * s + 0.5 * self.gravity[0] * s * s,
                        p[1] + v[1] * s + 0.5 * self.gravity[1] * s * s,
                    ];
                    s <= LOOKAHEAD_S && w.in_sector(pivot, q)
                });
                if hit {
                    want[w.button.index()] = true;
                }
            }
        }
        for i in CabinetInput::ALL {
            if want[i.index()] {
                self.tap(out, t, i, FLIP_HOLD, FLIP_COOLDOWN);
            }
        }
    }
}

/// The steel ball the harness injects on single-ball tables: position
/// (m) and velocity (m/s) of an open upper-playfield spot of the Apex
/// Circuit (the spot `apex_mechanisms.rs` uses).
pub const INJECT_POSITION_M: [f32; 3] = [0.257, 0.560, 0.0135];
/// Initial velocity of the injected ball, m/s.
pub const INJECT_VELOCITY_M_S: [f32; 3] = [0.0, 0.6, 0.0];
/// Profile of the injected ball.
pub const INJECT_PROFILE: &str = "standard_steel";
/// How often the harness considers an injection, ticks (20 s), at phase
/// [`INJECT_PHASE`].
pub const INJECT_EVERY: u64 = 20_000;
/// Tick phase of the injection checks (8 s: the first one falls inside
/// the bounded 10 s soak).
pub const INJECT_PHASE: u64 = 8_000;

fn inject(sim: &mut Simulation) {
    sim.spawn_ball(INJECT_PROFILE, INJECT_POSITION_M, INJECT_VELOCITY_M_S)
        .expect("inject a ball");
}

/// A recorded soak stream: the input recording plus the ticks before which
/// the harness injected a ball (empty on tables with multiball rules).
#[derive(Clone, Debug)]
pub struct Stream {
    /// The input events (pin_sim's recording format).
    pub recording: Recording,
    /// Ticks before which one ball was injected (non-decreasing).
    pub injections: Vec<u64>,
}

/// What a run ends with: the values compared across runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunResult {
    /// `Simulation::state_hash`.
    pub hash: u64,
    /// Player 1's score.
    pub score: Option<i64>,
    /// Hit count of every switch of the table, in `TableInfo` order.
    pub switch_hits: Vec<(String, u32)>,
}

/// The run summary of a recording run.
#[derive(Clone, Debug)]
pub struct Recorded {
    /// The stream.
    pub stream: Stream,
    /// The recording run's final values.
    pub result: RunResult,
    /// Most balls in play at once.
    pub max_balls: usize,
    /// First tick with at least two balls in play.
    pub first_multiball_tick: Option<u64>,
    /// Ticks with at least two balls in play.
    pub multiball_ticks: u64,
    /// Games started (game_state went to 1).
    pub games: u32,
}

fn result_of(sim: &Simulation, info: &TableInfo) -> RunResult {
    RunResult {
        hash: sim.state_hash(),
        score: sim.score(0),
        switch_hits: info
            .switches
            .iter()
            .map(|s| (s.name.clone(), sim.switch_hits(&s.name).unwrap_or(0)))
            .collect(),
    }
}

/// Loads `table` with the default config (the recording's).
pub fn load(table: Table) -> Simulation {
    Simulation::load(table.bundle(), SimConfig::default()).unwrap_or_else(|e| panic!("{e}"))
}

/// Records `ticks` ticks of the scripted player on `table`. On single-ball
/// tables, every [`INJECT_EVERY`] ticks while a game runs with exactly one
/// ball in play, one ball is injected before the tick (and the tick
/// noted).
pub fn record(table: Table, ticks: u64) -> Recorded {
    let info = TableInfo::load(table.bundle()).unwrap_or_else(|e| panic!("{e}"));
    let mut sim = load(table);
    let mut player = Player::new(&sim, &info);
    let state_key = sim.ids().get(pin_assets::IdKind::PersistKey, "game_state");
    let mut rec = Recording::new(sim.config().seed, sim.table_hash());
    let mut injections = Vec::new();
    let mut buf = Vec::new();
    let mut out = *sim.snapshot();
    let (mut max_balls, mut first_mb, mut mb_ticks, mut games) = (0, None, 0u64, 0u32);
    let mut last_state = 0i64;
    for t in 1..=ticks {
        player.inputs(&sim, &mut buf);
        let state = state_key.map_or(0, |k| sim.persist(k));
        if table.single_ball_rules()
            && t % INJECT_EVERY == INJECT_PHASE
            && state == 1
            && sim.balls_in_play() == 1
        {
            inject(&mut sim);
            injections.push(t);
        }
        for e in &buf {
            rec.events.push((t, *e));
        }
        rec.ticks = t;
        sim.step_1ms(t, &buf, &mut out);
        let n = sim.balls_in_play();
        max_balls = max_balls.max(n);
        if n >= 2 {
            mb_ticks += 1;
            first_mb.get_or_insert(t);
        }
        let st = state_key.map_or(0, |k| sim.persist(k));
        if st == 1 && last_state != 1 {
            games += 1;
        }
        last_state = st;
    }
    Recorded {
        result: result_of(&sim, &info),
        stream: Stream {
            recording: rec,
            injections,
        },
        max_balls,
        first_multiball_tick: first_mb,
        multiball_ticks: mb_ticks,
        games,
    }
}

/// Replays `stream` on a fresh load of `table` through `step` (the plain
/// simulation or a profiled wrapper), applying the scheduled injections
/// before their ticks.
fn play_with<S: SimStep>(
    table: Table,
    stream: &Stream,
    wrap: impl FnOnce(Simulation) -> S,
    unwrap: impl Fn(&mut S) -> &mut Simulation,
) -> S {
    let rec = &stream.recording;
    let sim = Simulation::load(
        table.bundle(),
        SimConfig {
            seed: rec.seed,
            ..SimConfig::default()
        },
    )
    .unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(sim.table_hash(), rec.table_hash, "recorded table");
    let mut step = wrap(sim);
    let mut out = FrameStateSnapshot::default();
    let mut scratch: Vec<InputEvent> = Vec::new();
    let (mut next, mut inj) = (0usize, 0usize);
    for t in 1..=rec.ticks {
        scratch.clear();
        while next < rec.events.len() && rec.events[next].0 <= t {
            if rec.events[next].0 == t {
                scratch.push(rec.events[next].1);
            }
            next += 1;
        }
        while inj < stream.injections.len() && stream.injections[inj] == t {
            inject(unwrap(&mut step));
            inj += 1;
        }
        step.step_1ms(t, &scratch, &mut out);
    }
    step
}

/// Replays `stream` on `table` without the profiler.
pub fn replay_plain(table: Table, stream: &Stream) -> RunResult {
    let info = TableInfo::load(table.bundle()).unwrap();
    let sim = play_with(table, stream, |s| s, |s| s);
    result_of(&sim, &info)
}

/// Replays `stream` through `pin_sim::replay::replay` (tables without
/// injections only).
pub fn replay_via_pin_sim(table: Table, stream: &Stream) -> u64 {
    assert!(stream.injections.is_empty());
    pin_sim::replay::replay(table.bundle(), &stream.recording).unwrap()
}

/// Replays `stream` through `Recorder::play` from a fresh load (tables
/// without injections only), returning the final values.
pub fn replay_recorder_play(table: Table, stream: &Stream) -> RunResult {
    assert!(stream.injections.is_empty());
    let info = TableInfo::load(table.bundle()).unwrap();
    let mut sim = load(table);
    Recorder::play(&mut sim, &stream.recording);
    result_of(&sim, &info)
}

/// Replays `stream` with every tick wrapped in [`ProfiledSim`] timed by
/// `clock`; returns the final values and the profiler snapshot.
pub fn replay_profiled<C>(table: Table, stream: &Stream, clock: C) -> (RunResult, ProfilerSnapshot)
where
    C: FnMut() -> u64 + Send,
{
    let info = TableInfo::load(table.bundle()).unwrap();
    let p = Profiler::new();
    let reader = p.reader;
    let sim_writer = p.sim;
    let step = play_with(
        table,
        stream,
        move |s| ProfiledSim::new(s, sim_writer, clock),
        |s| s.inner_mut(),
    );
    (result_of(step.inner(), &info), reader.snapshot())
}

/// A wall clock for report-only timing (never asserted, D8): ns since the
/// first call.
pub fn wall_ns() -> u64 {
    pin_app::orchestrator::monotonic_now_ns()
}

/// The `SOAK_HASH` line CI collects.
pub fn soak_hash_line(table: Table, ticks: u64, hash: u64) -> String {
    format!(
        "SOAK_HASH table={} os={} arch={} ticks={} hash={:016x}",
        table.name(),
        std::env::consts::OS,
        std::env::consts::ARCH,
        ticks,
        hash
    )
}
