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
//! [`SimCounters::balls_escaped`]: crate::SimCounters::balls_escaped

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
    stuck: bool,
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
        }
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
        self.observe_modes(sim);
        self.observe_script(sim, tick);
    }

    fn observe_balls(&mut self, sim: &Simulation, tick: u64) {
        let handles = sim.ball_handles();
        let bounds = sim.bounds;
        // Balls that left the table since the last observation.
        let mut vanished: Vec<(f32, Track)> = Vec::new();
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
        vanished.sort_by(|a, b| {
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

        for h in handles {
            let Some(b) = sim.ball(h) else { continue };
            let slot = usize::from(h.slot());
            let mut t = match self.tracks[slot] {
                Some(t) if t.handle == h => t,
                _ => Track {
                    handle: h,
                    first_tick: tick,
                    slow_ticks: 0,
                    last: b,
                    stuck: false,
                    reported_escape: false,
                    reported_nonfinite: false,
                },
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
            if speed < STUCK_SPEED_M_S && !self.exempt(sim, &b) {
                t.slow_ticks += 1;
            } else {
                t.slow_ticks = 0;
            }
            if t.slow_ticks > self.stuck_ticks && !t.stuck {
                t.stuck = true;
                let subject = self.info.subject_at(p);
                self.push(
                    FindingKind::StuckBall,
                    subject,
                    tick,
                    format!(
                        "ball in slot {} slower than 1 cm/s for {} ms outside any trough, lock, held cradle, magnet or plunger lane",
                        h.slot(),
                        t.slow_ticks
                    ),
                );
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

    /// Ends the game: queue-overflow findings, switch hits and counters.
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
        GameOutcome {
            findings: self.findings,
            switch_hits,
            shot_counts: self.shot_counts,
            mode_starts: self.mode_starts,
            ball_times_ms: self.ball_times,
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
