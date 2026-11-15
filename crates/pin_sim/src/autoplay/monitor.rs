//! Per-game health checks: the stuck-ball, escaped-ball and non-finite
//! detectors, shot and mode tracking, script faults, queue overflows, ball
//! times and switch hits.
//!
//! A [`GameMonitor`] watches one game through the public [`Simulation`]
//! API: call [`observe`](GameMonitor::observe) after every tick (and once
//! before the first, so injected states are checked too), then
//! [`finish`](GameMonitor::finish). It only reads the simulation; it never
//! changes it, so the game plays exactly as without it.
//!
//! # Detectors
//!
//! * **Stuck:** a ball slower than [`STUCK_SPEED_M_S`] in more than
//!   `stuck_ticks` consecutive observed ticks (default 10 000 = 10 s) while
//!   no exemption holds. Exemptions come from table data: inside a closed
//!   trough switch's zone; within [`LOCK_RADIUS_M`] of a VUK (lock / scoop)
//!   mech whose switch is closed (or that has none); within
//!   [`PLUNGER_RADIUS_M`] of a plunger mech (the plunger lane at rest); in
//!   range of a powered magnet; on the bat of an energised (held) flipper.
//!   The subject is the nearest named feature and the position.
//! * **Stuck (displacement):** a ball that stays within one ball diameter
//!   (`2 *` [`BALL_RADIUS_M`]) of an anchor point for more than
//!   `stuck_ticks` consecutive observed ticks while no exemption holds, at
//!   any speed: a ball pinned against a refiring sling or rattling in a
//!   pocket. The anchor is the ball's position when it first appears and is
//!   moved to the ball's position whenever it leaves the circle. The same
//!   exemptions apply, and the speed test above still runs; a ball is
//!   reported stuck once, by whichever test trips first.
//! * **Endless:** a ball still on the table when its age exceeds
//!   `endless_ticks` (default [`DEFAULT_ENDLESS_BALL_TICKS`] = 300 s), or
//!   still on the table when the soak stops a game at `max_game_ticks`
//!   ([`GameMonitor::end_at_cap`]), whichever comes first; once per ball, and
//!   not for a ball already reported stuck. The subject is the switch that
//!   ball hit most often (ties to the lowest switch id), or the nearest
//!   feature and position when it hit none. The detail lists the switches
//!   it hit in the last 60 s with their counts (most hits first, ties to the
//!   lowest id). A switch closing is credited to the nearest ball when the
//!   switch has a position, else to every ball on the table.
//! * **Escaped:** a ball outside the simulation's bounds (the collider
//!   bounds plus [`BOUNDS_MARGIN_M`](crate::load::BOUNDS_MARGIN_M), which
//!   includes "below the floor"), seen directly, or removed by the
//!   simulation as escaped ([`SimCounters::balls_escaped`]); then the
//!   vanished ball whose last state extrapolates furthest outside is named,
//!   with its last position.
//! * **Non-finite:** NaN or infinity in a ball's position, orientation,
//!   velocity or angular velocity, or a flipper's angle, angular velocity or
//!   zipper offset. (Scores are integers.)
//!
//! # Allocation
//!
//! All per-ball state is fixed-size and allocated in [`GameMonitor::new`];
//! [`observe`](GameMonitor::observe) allocates only when it records a
//! finding or the time of a ball that left the table.
//!
//! [`SimCounters::balls_escaped`]: crate::SimCounters::balls_escaped

use arrayvec::ArrayVec;
use glam::Vec3;
use pin_core::ids::SwitchId;
use pin_core::schema::table::{MechKind, SwitchKind};
use pin_core::snapshot::{BallState, FlipperState, MAX_BALLS};

use super::bot::BALL_RADIUS_M;
use super::report::{CounterTotals, Finding, FindingKind};
use super::table::{TableInfo, fmt_mm};
use crate::{BallHandle, Simulation};

/// Speed below which a ball may be stuck, m/s (1 cm/s).
pub const STUCK_SPEED_M_S: f32 = 0.01;

/// Default stuck time, ticks (10 s of simulated time).
pub const DEFAULT_STUCK_TICKS: u64 = 10_000;

/// A ball this close to a VUK (lock / scoop) mech is held by it, m.
pub const LOCK_RADIUS_M: f32 = 0.04;

/// A ball this close to a plunger mech rests in the plunger lane, m.
pub const PLUNGER_RADIUS_M: f32 = 0.06;

/// Extra reach beyond the bat for the held-cradle exemption, m.
pub const CRADLE_MARGIN_M: f32 = 0.01;

/// Default ball age after which a ball still on the table is an
/// [`FindingKind::EndlessBall`], ticks (300 s of simulated time).
pub const DEFAULT_ENDLESS_BALL_TICKS: u64 = 300_000;

/// Length of one hit-count bucket of the endless-ball window, ticks (1 s).
pub const HIT_BUCKET_TICKS: u64 = 1_000;

/// Buckets in the endless-ball window: the current second and the 59 before
/// it, so the window covers the last 59 to 60 s.
pub const HIT_BUCKETS: usize = 60;

/// Whether every component of `b` is finite.
#[must_use]
pub fn ball_is_finite(b: &BallState) -> bool {
    b.position_m
        .iter()
        .chain(&b.orientation)
        .chain(&b.velocity_m_s)
        .chain(&b.angular_velocity_rad_s)
        .all(|v| v.is_finite())
}

/// Whether every component of `f` is finite.
#[must_use]
pub fn flipper_is_finite(f: &FlipperState) -> bool {
    f.angle_rad.is_finite() && f.angular_velocity_rad_s.is_finite() && f.zipper_offset_m.is_finite()
}

/// How far `p` lies outside the box `bounds` (0 inside; infinity for a
/// non-finite point).
#[must_use]
pub fn outside_distance(bounds: (Vec3, Vec3), p: Vec3) -> f32 {
    if !p.is_finite() {
        return f32::INFINITY;
    }
    let below = (bounds.0 - p).max(Vec3::ZERO);
    let above = (p - bounds.1).max(Vec3::ZERO);
    below.max(above).max_element()
}

#[derive(Copy, Clone, Debug)]
struct Track {
    handle: BallHandle,
    first_tick: u64,
    slow_ticks: u64,
    last: BallState,
    /// Displacement anchor (see the module docs).
    anchor: Vec3,
    /// Consecutive non-exempt ticks within one diameter of `anchor`.
    near_ticks: u64,
    stuck: bool,
    endless: bool,
    reported_escape: bool,
    reported_nonfinite: bool,
}

#[derive(Copy, Clone, Debug, Default)]
struct ShotProgress {
    next: usize,
    start: u64,
}

/// What one game produced.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GameOutcome {
    /// Findings, in detection order.
    pub findings: Vec<Finding>,
    /// Hits per coverage switch ([`TableInfo::coverage_switches`] order).
    pub switch_hits: Vec<u64>,
    /// Completions per shot.
    pub shot_counts: Vec<u64>,
    /// Starts per mode.
    pub mode_starts: Vec<u64>,
    /// Time on the table of every ball that left it, ms.
    pub ball_times_ms: Vec<i64>,
    /// Observed time on the table of every ball still on it when the game
    /// ended (censored: the real time is at least this), ms.
    pub censored_ball_times_ms: Vec<i64>,
    /// Counters at the end.
    pub counters: CounterTotals,
}

/// Watches one game (see the [module docs](self)).
#[derive(Clone, Debug)]
pub struct GameMonitor<'t> {
    info: &'t TableInfo,
    game: u32,
    stuck_ticks: u64,
    tracks: [Option<Track>; MAX_BALLS],
    prev_bits: [u64; 4],
    escaped_seen: u32,
    faults_seen: u32,
    shots: Vec<ShotProgress>,
    shot_counts: Vec<u64>,
    /// Union of the shot switches, ascending.
    shot_switches: Vec<SwitchId>,
    mode_last: Vec<i64>,
    mode_starts: Vec<u64>,
    last_mode: Option<usize>,
    reported_flipper: Vec<bool>,
    findings: Vec<Finding>,
    ball_times: Vec<i64>,
    endless_ticks: u64,
    /// Coverage switches by ascending id: (bit index, plane position).
    hit_switches: Vec<(usize, Option<[f32; 2]>)>,
    /// Hit-count row length: the largest coverage switch index + 1.
    hit_row: usize,
    /// Hits per ball slot and switch over the ball's life.
    hits: Vec<u32>,
    /// Hits per ball slot, window bucket and switch.
    window: Vec<u16>,
    /// The second (tick / [`HIT_BUCKET_TICKS`]) each slot's window was last
    /// advanced to.
    window_sec: [u64; MAX_BALLS],
}

fn bit(bits: &[u64; 4], n: usize) -> bool {
    n < 256 && bits[n / 64] & (1u64 << (n % 64)) != 0
}

impl<'t> GameMonitor<'t> {
    /// A monitor for game `game` of `sim`, reporting a ball stuck after
    /// `stuck_ticks` slow ticks.
    #[must_use]
    pub fn new(info: &'t TableInfo, sim: &Simulation, game: u32, stuck_ticks: u64) -> Self {
        let mut shot_switches: Vec<SwitchId> = info
            .shots
            .iter()
            .flat_map(|s| s.sequence.iter().copied())
            .collect();
        shot_switches.sort_unstable();
        shot_switches.dedup();
        let mode_last = info
            .modes
            .iter()
            .map(|m| m.persist_key.map_or(0, |k| sim.persist(k)))
            .collect();
        let mut hit_switches: Vec<(usize, Option<[f32; 2]>)> = info
            .coverage_switches()
            .filter(|s| s.id.index() < 256)
            .map(|s| (s.id.index(), s.position_m.map(|p| [p[0], p[1]])))
            .collect();
        hit_switches.sort_unstable_by_key(|h| h.0);
        hit_switches.dedup_by_key(|h| h.0);
        let hit_row = hit_switches.last().map_or(0, |h| h.0 + 1);
        Self {
            info,
            game,
            stuck_ticks,
            tracks: [None; MAX_BALLS],
            prev_bits: sim.switch_bits(),
            escaped_seen: sim.counters().balls_escaped,
            faults_seen: sim.script_fault_count(),
            shots: vec![ShotProgress::default(); info.shots.len()],
            shot_counts: vec![0; info.shots.len()],
            shot_switches,
            mode_last,
            mode_starts: vec![0; info.modes.len()],
            last_mode: None,
            reported_flipper: vec![false; 16],
            findings: Vec::new(),
            ball_times: Vec::new(),
            endless_ticks: DEFAULT_ENDLESS_BALL_TICKS,
            hit_switches,
            hit_row,
            hits: vec![0; MAX_BALLS * hit_row],
            window: vec![0; MAX_BALLS * HIT_BUCKETS * hit_row],
            window_sec: [0; MAX_BALLS],
        }
    }

    /// Sets the ball age after which a ball still on the table is an
    /// [`FindingKind::EndlessBall`], ticks (default
    /// [`DEFAULT_ENDLESS_BALL_TICKS`]).
    #[must_use]
    pub fn with_endless_ticks(mut self, ticks: u64) -> Self {
        self.endless_ticks = ticks;
        self
    }

    /// Findings so far.
    #[must_use]
    pub fn findings(&self) -> &[Finding] {
        &self.findings
    }

    /// Whether at least one ball is on the table and every one of them has
    /// been reported stuck.
    #[must_use]
    pub fn all_balls_stuck(&self) -> bool {
        let mut any = false;
        for t in self.tracks.iter().flatten() {
            if !t.stuck {
                return false;
            }
            any = true;
        }
        any
    }

    fn push(&mut self, kind: FindingKind, subject: String, tick: u64, detail: String) {
        self.findings.push(Finding {
            kind,
            subject,
            game: self.game,
            tick,
            detail,
        });
    }

    /// Checks the state after the last tick.
    pub fn observe(&mut self, sim: &Simulation) {
        let tick = sim.tick();
        self.observe_balls(sim, tick);
        self.observe_flippers(sim, tick);
        self.observe_switches(sim, tick);
        self.observe_endless(tick);
        self.observe_modes(sim);
        self.observe_script(sim, tick);
    }

    fn observe_balls(&mut self, sim: &Simulation, tick: u64) {
        let handles = sim.ball_handles();
        let bounds = sim.bounds;
        // Balls that left the table since the last observation.
        let mut vanished: ArrayVec<(f32, Track), MAX_BALLS> = ArrayVec::new();
        for slot in 0..MAX_BALLS {
            if let Some(t) = self.tracks[slot]
                && !handles.contains(&t.handle)
            {
                self.ball_times
                    .push(i64::try_from(tick.saturating_sub(t.first_tick)).unwrap_or(i64::MAX));
                let p = Vec3::from_array(t.last.position_m);
                let v = Vec3::from_array(t.last.velocity_m_s);
                vanished.push((outside_distance(bounds, p + v * 1.0e-3), t));
                self.tracks[slot] = None;
            }
        }
        let escaped = sim.counters().balls_escaped;
        let mut new_escapes = escaped.saturating_sub(self.escaped_seen) as usize;
        self.escaped_seen = escaped;
        // Furthest outside first; ties by slot.
        vanished.sort_unstable_by(|a, b| {
            b.0.total_cmp(&a.0)
                .then(a.1.handle.slot().cmp(&b.1.handle.slot()))
        });
        for (_, t) in vanished {
            if new_escapes == 0 {
                break;
            }
            new_escapes -= 1;
            if t.reported_escape {
                continue;
            }
            let p = Vec3::from_array(t.last.position_m);
            let subject = self.info.subject_at(p);
            self.push(
                FindingKind::EscapedBall,
                subject,
                tick,
                format!(
                    "ball in slot {} left the table bounds (last seen at {}, velocity {:?} m/s) and was removed",
                    t.handle.slot(),
                    fmt_mm(p),
                    t.last.velocity_m_s
                ),
            );
        }
        // Escapes with no matching track: the ball spawned outside the
        // bounds or escaped on the tick it appeared, so no observation saw it.
        for _ in 0..new_escapes {
            self.push(
                FindingKind::EscapedBall,
                "untracked ball".to_string(),
                tick,
                "a ball escaped the table bounds but was never tracked (it left between monitor observations)".to_string(),
            );
        }

        for h in handles {
            let Some(b) = sim.ball(h) else { continue };
            let slot = usize::from(h.slot());
            let mut t = match self.tracks[slot] {
                Some(t) if t.handle == h => t,
                _ => {
                    self.reset_hits(slot, tick);
                    Track {
                        handle: h,
                        first_tick: tick,
                        slow_ticks: 0,
                        last: b,
                        anchor: Vec3::from_array(b.position_m),
                        near_ticks: 0,
                        stuck: false,
                        endless: false,
                        reported_escape: false,
                        reported_nonfinite: false,
                    }
                }
            };
            t.last = b;
            let p = Vec3::from_array(b.position_m);
            if !ball_is_finite(&b) {
                if !t.reported_nonfinite {
                    t.reported_nonfinite = true;
                    self.push(
                        FindingKind::NonFinite,
                        format!("ball slot {}", h.slot()),
                        tick,
                        format!("non-finite ball state {b:?}"),
                    );
                }
            } else if outside_distance(bounds, p) > 0.0 && !t.reported_escape {
                t.reported_escape = true;
                let subject = self.info.subject_at(p);
                self.push(
                    FindingKind::EscapedBall,
                    subject,
                    tick,
                    format!(
                        "ball in slot {} is {:.0} mm outside the table bounds",
                        h.slot(),
                        outside_distance(bounds, p) * 1.0e3
                    ),
                );
            }
            let speed = Vec3::from_array(b.velocity_m_s).length();
            let slow = speed < STUCK_SPEED_M_S;
            let finite = p.is_finite();
            if !finite || p.distance(t.anchor) > 2.0 * BALL_RADIUS_M {
                t.anchor = p;
                t.near_ticks = 0;
            }
            // A ball already reported stuck needs no exemption test.
            let exempt = !t.stuck && (slow || finite) && self.exempt(sim, &b);
            if slow && !exempt {
                t.slow_ticks += 1;
            } else {
                t.slow_ticks = 0;
            }
            if finite && !exempt {
                t.near_ticks = t.near_ticks.saturating_add(1);
            } else {
                t.near_ticks = 0;
            }
            if !t.stuck && (t.slow_ticks > self.stuck_ticks || t.near_ticks > self.stuck_ticks) {
                t.stuck = true;
                let subject = self.info.subject_at(p);
                let detail = if t.slow_ticks > self.stuck_ticks {
                    format!(
                        "ball in slot {} slower than 1 cm/s for {} ms outside any trough, lock, held cradle, magnet or plunger lane",
                        h.slot(),
                        t.slow_ticks
                    )
                } else {
                    format!(
                        "ball in slot {} stayed within one ball diameter ({:.0} mm) of {} for {} ms (now at {:.3} m/s) outside any trough, lock, held cradle, magnet or plunger lane",
                        h.slot(),
                        2.0e3 * BALL_RADIUS_M,
                        fmt_mm(t.anchor),
                        t.near_ticks,
                        speed
                    )
                };
                self.push(FindingKind::StuckBall, subject, tick, detail);
            }
            self.tracks[slot] = Some(t);
        }
    }

    /// Whether a slow ball is legitimately at rest (see the module docs).
    #[must_use]
    pub fn exempt(&self, sim: &Simulation, b: &BallState) -> bool {
        let p = Vec3::from_array(b.position_m);
        let bits = sim.switch_bits();
        let near = |q: [f32; 3], r: f32| {
            let d = glam::Vec2::new(p.x - q[0], p.y - q[1]);
            d.length() <= r
        };
        for s in &self.info.switches {
            if s.kind == SwitchKind::Trough
                && bit(&bits, s.id.index())
                && let (Some(q), Some(r)) = (s.position_m, s.radius_m)
                && near(q, r + BALL_RADIUS_M)
            {
                return true;
            }
        }
        for m in &self.info.mechs {
            let Some(q) = m.position_m else { continue };
            match m.kind {
                MechKind::Vuk
                    if near(q, LOCK_RADIUS_M) && m.switch.is_none_or(|s| bit(&bits, s.index())) =>
                {
                    return true;
                }
                MechKind::Plunger if near(q, PLUNGER_RADIUS_M) => return true,
                _ => {}
            }
        }
        if sim
            .solver()
            .magnets()
            .iter()
            .any(|m| m.power > 0.0 && m.in_range(p, b.layer))
        {
            return true;
        }
        let fs = sim.flipper_set();
        self.info.flippers.iter().any(|f| {
            fs.is_energised(f.id)
                && f.layer == b.layer
                && fs.pivot_m(f.id).is_some_and(|pv| {
                    glam::Vec2::new(p.x - pv.x, p.y - pv.y).length()
                        <= f.length_m + f.radius_m + BALL_RADIUS_M + CRADLE_MARGIN_M
                })
        })
    }

    fn observe_flippers(&mut self, sim: &Simulation, tick: u64) {
        for f in sim.snapshot().flippers() {
            let i = usize::from(f.id.0);
            if !flipper_is_finite(f) && !self.reported_flipper.get(i).copied().unwrap_or(true) {
                self.reported_flipper[i] = true;
                let name = sim
                    .ids()
                    .name(pin_assets::IdKind::Flipper, f.id.0)
                    .unwrap_or("?")
                    .to_owned();
                self.push(
                    FindingKind::NonFinite,
                    format!("flipper {name}"),
                    tick,
                    format!("non-finite flipper state {f:?}"),
                );
            }
        }
    }

    fn observe_switches(&mut self, sim: &Simulation, tick: u64) {
        let bits = sim.switch_bits();
        let mut rising = [0u64; 4];
        for (r, (n, p)) in rising.iter_mut().zip(bits.iter().zip(self.prev_bits)) {
            *r = n & !p;
        }
        self.prev_bits = bits;
        if rising == [0; 4] {
            return;
        }
        self.credit_hits(&rising, tick);
        for k in 0..self.shot_switches.len() {
            let sw = self.shot_switches[k];
            if !bit(&rising, sw.index()) {
                continue;
            }
            for (i, shot) in self.info.shots.iter().enumerate() {
                let prog = &mut self.shots[i];
                if prog.next > 0 && tick.saturating_sub(prog.start) > shot.window_ticks {
                    prog.next = 0;
                }
                if shot.sequence.get(prog.next) == Some(&sw) {
                    if prog.next == 0 {
                        prog.start = tick;
                    }
                    prog.next += 1;
                } else if shot.sequence.first() == Some(&sw) {
                    prog.next = 1;
                    prog.start = tick;
                }
                if prog.next == shot.sequence.len() && !shot.sequence.is_empty() {
                    self.shot_counts[i] += 1;
                    prog.next = 0;
                }
            }
        }
    }

    /// Clears slot `slot`'s hit counts for a new ball seen at `tick`.
    fn reset_hits(&mut self, slot: usize, tick: u64) {
        let row = self.hit_row;
        self.hits[slot * row..(slot + 1) * row].fill(0);
        let w = HIT_BUCKETS * row;
        self.window[slot * w..(slot + 1) * w].fill(0);
        self.window_sec[slot] = tick / HIT_BUCKET_TICKS;
    }

    /// Moves slot `slot`'s window to second `sec`, clearing the buckets of
    /// the seconds it skips (at most the whole window).
    fn advance_window(&mut self, slot: usize, sec: u64) {
        let last = self.window_sec[slot];
        if sec <= last {
            return;
        }
        let row = self.hit_row;
        let n = (sec - last).min(HIT_BUCKETS as u64);
        for k in 0..n {
            let b = ((sec - k) % HIT_BUCKETS as u64) as usize;
            let at = (slot * HIT_BUCKETS + b) * row;
            self.window[at..at + row].fill(0);
        }
        self.window_sec[slot] = sec;
    }

    /// Credits one hit of switch index `sw` to slot `slot`.
    fn credit(&mut self, slot: usize, sw: usize, tick: u64) {
        let sec = tick / HIT_BUCKET_TICKS;
        self.advance_window(slot, sec);
        let row = self.hit_row;
        let total = &mut self.hits[slot * row + sw];
        *total = total.saturating_add(1);
        let b = (sec % HIT_BUCKETS as u64) as usize;
        let w = &mut self.window[(slot * HIT_BUCKETS + b) * row + sw];
        *w = w.saturating_add(1);
    }

    /// Credits the rising coverage switches in `rising` to the balls on
    /// the table: to the nearest ball (ties to the lowest slot) when the
    /// switch has a position, else to every ball.
    fn credit_hits(&mut self, rising: &[u64; 4], tick: u64) {
        for k in 0..self.hit_switches.len() {
            let (sw, pos) = self.hit_switches[k];
            if !bit(rising, sw) {
                continue;
            }
            if let Some(q) = pos {
                let mut best: Option<(f32, usize)> = None;
                for (slot, t) in self.tracks.iter().enumerate() {
                    let Some(t) = t else { continue };
                    let d =
                        glam::Vec2::new(t.last.position_m[0] - q[0], t.last.position_m[1] - q[1])
                            .length();
                    // A non-finite distance never wins over a finite one.
                    let d = if d.is_finite() { d } else { f32::INFINITY };
                    if best.is_none_or(|(bd, _)| d < bd) {
                        best = Some((d, slot));
                    }
                }
                if let Some((_, slot)) = best {
                    self.credit(slot, sw, tick);
                }
            } else {
                for slot in 0..MAX_BALLS {
                    if self.tracks[slot].is_some() {
                        self.credit(slot, sw, tick);
                    }
                }
            }
        }
    }

    /// Raises [`FindingKind::EndlessBall`] for every ball older than the
    /// endless limit.
    fn observe_endless(&mut self, tick: u64) {
        for slot in 0..MAX_BALLS {
            if let Some(t) = self.tracks[slot]
                && !t.endless
                && !t.stuck
                && tick.saturating_sub(t.first_tick) > self.endless_ticks
            {
                let why = format!(
                    "on the table longer than the {} ms limit",
                    self.endless_ticks
                );
                self.raise_endless(slot, tick, &why);
            }
        }
    }

    /// Raises [`FindingKind::EndlessBall`] for every ball still on the
    /// table that has none yet and is not reported stuck. The soak calls
    /// this when it stops a game at `max_game_ticks`.
    pub fn end_at_cap(&mut self, sim: &Simulation) {
        let tick = sim.tick();
        for slot in 0..MAX_BALLS {
            if let Some(t) = self.tracks[slot]
                && !t.endless
                && !t.stuck
            {
                self.raise_endless(
                    slot,
                    tick,
                    "still in play when the game reached max_game_ticks",
                );
            }
        }
    }

    /// The coverage switch indices with their counts in `counts` (one hit
    /// row), most hits first, ties to the lowest index; zero counts left
    /// out.
    fn ranked(&self, counts: &[u64]) -> Vec<(usize, u64)> {
        let mut v: Vec<(usize, u64)> = self
            .hit_switches
            .iter()
            .map(|&(sw, _)| (sw, counts[sw]))
            .filter(|&(_, n)| n > 0)
            .collect();
        v.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    }

    fn switch_label(&self, sw: usize) -> String {
        self.info
            .switches
            .iter()
            .find(|s| s.id.index() == sw)
            .map_or_else(|| format!("switch {sw}"), |s| s.name.clone())
    }

    fn raise_endless(&mut self, slot: usize, tick: u64, why: &str) {
        let Some(mut t) = self.tracks[slot] else {
            return;
        };
        t.endless = true;
        self.tracks[slot] = Some(t);
        self.advance_window(slot, tick / HIT_BUCKET_TICKS);
        let row = self.hit_row;
        let total: Vec<u64> = self.hits[slot * row..(slot + 1) * row]
            .iter()
            .map(|&n| u64::from(n))
            .collect();
        let mut recent = vec![0u64; row];
        for b in 0..HIT_BUCKETS {
            let at = (slot * HIT_BUCKETS + b) * row;
            for (r, &n) in recent.iter_mut().zip(&self.window[at..at + row]) {
                *r += u64::from(n);
            }
        }
        let p = Vec3::from_array(t.last.position_m);
        let subject = self
            .ranked(&total)
            .first()
            .map_or_else(|| self.info.subject_at(p), |&(sw, _)| self.switch_label(sw));
        let recent = self.ranked(&recent);
        let list = if recent.is_empty() {
            "none".to_owned()
        } else {
            recent
                .iter()
                .map(|&(sw, n)| format!("{} x{n}", self.switch_label(sw)))
                .collect::<Vec<_>>()
                .join(", ")
        };
        let age = tick.saturating_sub(t.first_tick);
        self.push(
            FindingKind::EndlessBall,
            subject,
            tick,
            format!(
                "ball in slot {} {why}: {age} ms on the table, last at {}; switches hit in the last 60 s: {list}",
                t.handle.slot(),
                fmt_mm(p)
            ),
        );
    }

    fn observe_modes(&mut self, sim: &Simulation) {
        for (i, m) in self.info.modes.iter().enumerate() {
            if let Some(k) = m.persist_key {
                let v = sim.persist(k);
                if v > self.mode_last[i] {
                    self.mode_starts[i] += 1;
                    self.last_mode = Some(i);
                }
                self.mode_last[i] = v;
            }
        }
    }

    fn observe_script(&mut self, sim: &Simulation, tick: u64) {
        let faults = sim.script_fault_count();
        if faults <= self.faults_seen {
            return;
        }
        self.faults_seen = faults;
        let msg = sim
            .script_error()
            .map_or_else(|| "unknown script fault".to_owned(), ToString::to_string);
        let mode_names: Vec<&str> = self.info.modes.iter().map(|m| m.name.as_str()).collect();
        let subject = script_subject(&msg, &mode_names);
        let last = self
            .last_mode
            .map_or("none", |i| self.info.modes[i].name.as_str());
        self.push(
            FindingKind::ScriptError,
            subject,
            tick,
            format!("{msg} (fault {faults}; last mode started: {last})"),
        );
    }

    /// Ends the game: queue-overflow findings, switch hits, counters, and
    /// the observed ages of the balls still on the table (censored ball
    /// times).
    #[must_use]
    pub fn finish(mut self, sim: &Simulation) -> GameOutcome {
        let tick = sim.tick();
        let c = sim.counters();
        let overflows = sim.script_queue_overflows();
        for (subject, n) in [
            ("script command queue", overflows),
            ("switch edge queue", c.edges_dropped),
            ("input event queue", c.inputs_dropped),
            ("haptic event buffer", c.haptics_dropped),
            ("ball event list", c.ball_events_dropped),
        ] {
            if n > 0 {
                self.push(
                    FindingKind::QueueOverflow,
                    subject.to_owned(),
                    tick,
                    format!("{n} entries dropped"),
                );
            }
        }
        let switch_hits = self
            .info
            .coverage_switches()
            .map(|s| u64::from(sim.switch_hits(&s.name).unwrap_or(0)))
            .collect();
        let censored = self
            .tracks
            .iter()
            .flatten()
            .map(|t| i64::try_from(tick.saturating_sub(t.first_tick)).unwrap_or(i64::MAX))
            .collect();
        GameOutcome {
            findings: self.findings,
            switch_hits,
            shot_counts: self.shot_counts,
            mode_starts: self.mode_starts,
            ball_times_ms: self.ball_times,
            censored_ball_times_ms: censored,
            counters: CounterTotals {
                edges_dropped: u64::from(c.edges_dropped),
                haptics_dropped: u64::from(c.haptics_dropped),
                inputs_dropped: u64::from(c.inputs_dropped),
                spawns_failed: u64::from(c.spawns_failed),
                ball_events_dropped: u64::from(c.ball_events_dropped),
                balls_drained: u64::from(c.balls_drained),
                balls_escaped: u64::from(c.balls_escaped),
                script_queue_overflows: u64::from(overflows),
                script_faults: u64::from(sim.script_fault_count()),
            },
        }
    }
}

/// Whether `word` occurs in `text` as a whole identifier (not inside a
/// longer run of `[A-Za-z0-9_]`).
#[must_use]
pub fn mentions(text: &str, word: &str) -> bool {
    if word.is_empty() {
        return false;
    }
    let ident = |c: char| c.is_ascii_alphanumeric() || c == '_';
    let mut from = 0;
    while let Some(i) = text[from..].find(word) {
        let at = from + i;
        let end = at + word.len();
        let before = text[..at].chars().next_back().is_none_or(|c| !ident(c));
        let after = text[end..].chars().next().is_none_or(|c| !ident(c));
        if before && after {
            return true;
        }
        from = at + text[at..].chars().next().map_or(1, char::len_utf8);
    }
    false
}

/// The subject of a script fault: the longest declared mode id the error
/// text mentions; else the entry point it mentions (`on_init`, `on_tick`,
/// `on_switch`); else `script`.
#[must_use]
pub fn script_subject(msg: &str, modes: &[&str]) -> String {
    let mut best: Option<&str> = None;
    for m in modes {
        if mentions(msg, m) && best.is_none_or(|b| m.len() > b.len()) {
            best = Some(m);
        }
    }
    if let Some(m) = best {
        return m.to_owned();
    }
    for entry in ["on_init", "on_tick", "on_switch_edge", "on_switch"] {
        if mentions(msg, entry) {
            return entry.to_owned();
        }
    }
    "script".to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pin_core::FlipperId;

    #[test]
    fn mentions_whole_words_only() {
        assert!(mentions("mode frenzy failed", "frenzy"));
        assert!(mentions("frenzy", "frenzy"));
        assert!(!mentions("superfrenzy failed", "frenzy"));
        assert!(!mentions("frenzy_2 failed", "frenzy"));
        assert!(mentions("frenzy_2 and frenzy", "frenzy"));
        assert!(!mentions("abc", ""));
        assert!(mentions("é frenzy", "frenzy"));
    }

    #[test]
    fn script_subject_prefers_modes_then_entries() {
        let modes = ["multi", "multiball"];
        assert_eq!(
            script_subject("error in multiball start", &modes),
            "multiball"
        );
        assert_eq!(script_subject("in function 'on_tick'", &modes), "on_tick");
        assert_eq!(script_subject("boom", &modes), "script");
    }

    #[test]
    fn finite_checks() {
        let mut b = BallState::default();
        assert!(ball_is_finite(&b));
        b.angular_velocity_rad_s[2] = f32::INFINITY;
        assert!(!ball_is_finite(&b));
        let mut f = FlipperState {
            id: FlipperId(0),
            angle_rad: 0.0,
            angular_velocity_rad_s: 0.0,
            zipper_offset_m: 0.0,
        };
        assert!(flipper_is_finite(&f));
        f.zipper_offset_m = f32::NAN;
        assert!(!flipper_is_finite(&f));
    }

    #[test]
    fn outside_distance_cases() {
        let b = (Vec3::ZERO, Vec3::ONE);
        assert_eq!(outside_distance(b, Vec3::splat(0.5)), 0.0);
        assert!((outside_distance(b, Vec3::new(0.5, 0.5, -0.25)) - 0.25).abs() < 1e-6);
        assert!((outside_distance(b, Vec3::new(2.0, 0.5, 0.5)) - 1.0).abs() < 1e-6);
        assert_eq!(outside_distance(b, Vec3::NAN), f32::INFINITY);
    }
}
