//! Lamp warm-up curves: the emitted intensity of a lamp follows its
//! commanded level with a first-order response (an exponential approach),
//! evaluated from simulated time so it is deterministic.
//!
//! For a step of the commanded level from 0 to 1 at `t = 0` the emitted level
//! is `1 − exp(−t / τ)`:
//!
//! | kind | τ | at 1 ms | at 10 ms | at 15 ms |
//! |---|---|---|---|---|
//! | [`LampKind::Incandescent`] | 4.5 ms | 0.199 | 0.892 | 0.964 |
//! | [`LampKind::Led`] | 0.25 ms | 0.982 | 1.000 | 1.000 |
//!
//! so an incandescent bulb reaches 95 % in ~13.5 ms (`3τ`), inside the 15 ms
//! budget and not before 10 ms; an LED reaches it in 0.75 ms. Cooling uses
//! the same time constant. The update is the exact discretisation of the
//! first-order system, so the result after a given span of time does not
//! depend on how it is split into steps (up to rounding).

use pin_core::FrameStateSnapshot;
use pin_core::snapshot::MAX_LAMPS;

/// The light source behind a lamp.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum LampKind {
    /// A #44/#47/#555-style filament bulb (time constant 4.5 ms).
    #[default]
    Incandescent,
    /// An LED (time constant 0.25 ms).
    Led,
}

impl LampKind {
    /// Time constant τ of the first-order response, in seconds.
    pub const fn time_constant_s(self) -> f32 {
        match self {
            LampKind::Incandescent => 0.0045,
            LampKind::Led => 0.000_25,
        }
    }
}

/// The emitted level of one lamp (first-order response to its commanded
/// level).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LampWarmup {
    kind: LampKind,
    level: f32,
}

impl Default for LampWarmup {
    fn default() -> Self {
        Self::new(LampKind::default())
    }
}

/// `commanded` clamped to `0..=1` (NaN → 0).
fn sanitize_level(x: f32) -> f32 {
    if x.is_nan() { 0.0 } else { x.clamp(0.0, 1.0) }
}

impl LampWarmup {
    /// A dark lamp of `kind`.
    pub const fn new(kind: LampKind) -> Self {
        Self { kind, level: 0.0 }
    }

    /// The lamp kind.
    pub const fn kind(&self) -> LampKind {
        self.kind
    }

    /// The current emitted level (`0..=1`).
    pub const fn level(&self) -> f32 {
        self.level
    }

    /// Jumps straight to `level` (clamped to `0..=1`), e.g. after a replay
    /// seek.
    pub fn reset(&mut self, level: f32) {
        self.level = sanitize_level(level);
    }

    /// Advances by `dt_s` seconds of simulated time towards `commanded`
    /// (clamped to `0..=1`; NaN counts as 0) and returns the emitted level
    /// (`0..=1`). A `dt_s` that is not finite and positive leaves the level
    /// unchanged.
    pub fn update(&mut self, commanded: f32, dt_s: f32) -> f32 {
        let target = sanitize_level(commanded);
        if dt_s.is_finite() && dt_s > 0.0 {
            let decay = libm::expf(-dt_s / self.kind.time_constant_s());
            // level' = target + (level − target)·e^(−dt/τ)
            self.level = sanitize_level(target + (self.level - target) * decay);
        }
        self.level
    }
}

/// Warm-up state of every lamp of a snapshot (`MAX_LAMPS`, indexed by
/// `LampId`), advanced from snapshot ticks (1 tick = 1 ms of simulated time).
///
/// Allocation-free after construction.
#[derive(Clone, Debug)]
pub struct LampBank {
    lamps: [LampWarmup; MAX_LAMPS],
    last_tick: Option<u64>,
}

impl Default for LampBank {
    fn default() -> Self {
        Self::new()
    }
}

impl LampBank {
    /// Every lamp dark and incandescent.
    pub fn new() -> Self {
        Self {
            lamps: [LampWarmup::default(); MAX_LAMPS],
            last_tick: None,
        }
    }

    /// Sets the kind of lamp `lamp` (ignored when out of range). The current
    /// level is kept.
    pub fn set_kind(&mut self, lamp: usize, kind: LampKind) {
        if let Some(l) = self.lamps.get_mut(lamp) {
            l.kind = kind;
        }
    }

    /// The emitted level of lamp `lamp` (0 when out of range).
    pub fn level(&self, lamp: usize) -> f32 {
        self.lamps.get(lamp).map_or(0.0, LampWarmup::level)
    }

    /// The warm-up state of lamp `lamp`.
    pub fn lamp(&self, lamp: usize) -> Option<&LampWarmup> {
        self.lamps.get(lamp)
    }

    /// Advances every lamp to `snapshot.tick` towards `snapshot.lamps`.
    ///
    /// The first call only records the tick (lamps start dark and warm up
    /// from there). A tick that goes backwards (a replay seek or restart)
    /// snaps every lamp to its commanded level. The same tick again changes
    /// nothing.
    pub fn update(&mut self, snapshot: &FrameStateSnapshot) {
        self.update_levels(snapshot.tick, &snapshot.lamps);
    }

    /// [`update`](Self::update) from a tick and commanded levels.
    pub fn update_levels(&mut self, tick: u64, commanded: &[f32; MAX_LAMPS]) {
        match self.last_tick {
            None => {}
            Some(last) if tick < last => {
                for (l, &c) in self.lamps.iter_mut().zip(commanded) {
                    l.reset(c);
                }
            }
            Some(last) => {
                let dt = (tick - last) as f32 * 0.001;
                for (l, &c) in self.lamps.iter_mut().zip(commanded) {
                    l.update(c, dt);
                }
            }
        }
        self.last_tick = Some(tick);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_response_timings() {
        let at = |kind, ms: f32| {
            let mut w = LampWarmup::new(kind);
            w.update(1.0, ms * 1e-3)
        };
        assert!(at(LampKind::Incandescent, 15.0) >= 0.95);
        assert!(at(LampKind::Incandescent, 10.0) < 0.95);
        assert!(at(LampKind::Led, 1.0) >= 0.95);
        // Split into 1 ms steps: same answer.
        let mut w = LampWarmup::new(LampKind::Incandescent);
        let mut v = 0.0;
        for _ in 0..15 {
            v = w.update(1.0, 1e-3);
        }
        assert!((v - at(LampKind::Incandescent, 15.0)).abs() < 1e-5);
    }

    #[test]
    fn bad_inputs_are_sanitised() {
        let mut w = LampWarmup::new(LampKind::Led);
        assert_eq!(w.update(5.0, f32::NAN), 0.0);
        assert_eq!(w.update(5.0, -1.0), 0.0);
        assert!(w.update(5.0, 1.0) <= 1.0);
        assert_eq!(w.update(f32::NAN, 1.0), 0.0);
        w.reset(2.0);
        assert_eq!(w.level(), 1.0);
        assert_eq!(w.kind(), LampKind::Led);
    }

    #[test]
    fn bank_follows_ticks() {
        let mut bank = LampBank::new();
        bank.set_kind(1, LampKind::Led);
        bank.set_kind(9999, LampKind::Led);
        let mut s = FrameStateSnapshot::default();
        s.lamps[0] = 1.0;
        s.lamps[1] = 1.0;
        bank.update(&s);
        assert_eq!(bank.level(0), 0.0);
        s.tick = 2;
        bank.update(&s);
        assert!(bank.level(0) > 0.3 && bank.level(0) < 0.5);
        assert!(bank.level(1) > 0.99);
        assert_eq!(bank.level(1000), 0.0);
        assert_eq!(bank.lamp(1).map(LampWarmup::kind), Some(LampKind::Led));
        // Backwards: snap.
        s.tick = 1;
        s.lamps[0] = 0.25;
        bank.update(&s);
        assert_eq!(bank.level(0), 0.25);
        let _ = LampBank::default();
    }
}
