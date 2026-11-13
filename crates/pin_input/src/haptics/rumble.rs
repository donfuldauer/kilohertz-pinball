//! SDL-free gamepad rumble mapper.
//!
//! Each [`HapticEvent`] becomes a rumble *envelope*: a peak on the
//! low-frequency (big, heavy) motor and the high-frequency (small, buzzy)
//! motor, scaled by the event strength, that decays linearly to zero over the
//! effect's duration.
//!
//! # Exact rules
//! * The [`RumbleEffect`] for an event is the per-source override
//!   ([`RumbleSettings::set_override`], keyed by [`HapticEvent::source`]) if
//!   there is one, otherwise the effect of the event's kind
//!   ([`RumbleSettings::coil_fire`], `eos_hit`, `bumper`, `ball_drop`,
//!   `ball_roll`, `impulse`).
//! * Peaks: `low₀ = c(effect.low) · c(strength) · c(master)` and likewise for
//!   `high`, where `c` clamps to `0..=1` and maps NaN to 0. A disabled
//!   mapper, a zero duration or a zero peak adds nothing.
//! * Envelope of an event at tick `t₀` with duration `d` ticks, at tick `t`:
//!   `peak · (1 − (t − t₀)/d)` for `t₀ ≤ t < t₀ + d`, else 0 (nothing before
//!   the event's tick, exactly 0 from `t₀ + d` on).
//! * [`RumbleMapper::output_at`] returns, per motor, the **maximum** over all
//!   active envelopes (overlapping events merge by max, never by sum), so
//!   the output is always in `0..=1`.
//! * At most [`MAX_ACTIVE_RUMBLES`] envelopes are kept. A new event takes a
//!   free or expired slot (expired: `t₀ + d ≤` the latest tick seen); with
//!   none, it replaces the slot whose `low + high` at the latest tick seen is
//!   smallest (lowest index on a tie) if its own peak sum is larger, and is
//!   dropped otherwise.
//!
//! The result depends only on the settings and the sequence of `feed` and
//! `output_at` calls, never on wall-clock time, so it is deterministic.
//! `feed` and `output_at` never allocate.

use arrayvec::ArrayVec;
use pin_core::haptics::{HapticEvent, HapticKind};
use pin_core::input::HapticsTelemetryEvent;

/// Most envelopes a [`RumbleMapper`] tracks at once.
pub const MAX_ACTIVE_RUMBLES: usize = 32;

/// Most per-source overrides in [`RumbleSettings`].
pub const MAX_RUMBLE_OVERRIDES: usize = 32;

/// Motor intensities, each `0..=1`.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Rumble {
    /// Low-frequency (large) motor.
    pub low: f32,
    /// High-frequency (small) motor.
    pub high: f32,
}

impl Rumble {
    /// Both motors off.
    pub const OFF: Rumble = Rumble {
        low: 0.0,
        high: 0.0,
    };

    /// True when both motors are off.
    pub fn is_off(&self) -> bool {
        self.low == 0.0 && self.high == 0.0
    }
}

/// One rumble effect: motor peaks at full strength and the decay length.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RumbleEffect {
    /// Low-frequency motor peak (`0..=1`).
    pub low: f32,
    /// High-frequency motor peak (`0..=1`).
    pub high: f32,
    /// Ticks (ms) from the peak to zero.
    pub duration_ticks: u32,
}

impl RumbleEffect {
    /// No rumble.
    pub const NONE: RumbleEffect = RumbleEffect {
        low: 0.0,
        high: 0.0,
        duration_ticks: 0,
    };

    /// Builds an effect.
    pub const fn new(low: f32, high: f32, duration_ticks: u32) -> Self {
        Self {
            low,
            high,
            duration_ticks,
        }
    }
}

/// Rumble settings: one effect per event kind, per-source overrides and a
/// master level.
#[derive(Clone, Debug, PartialEq)]
pub struct RumbleSettings {
    /// Rumble on/off.
    pub enabled: bool,
    /// Master level applied to every peak (`0..=1`).
    pub master: f32,
    /// [`HapticKind::CoilFire`] (flippers, slings, kickers).
    pub coil_fire: RumbleEffect,
    /// [`HapticKind::EosHit`] (flipper end-of-stroke).
    pub eos_hit: RumbleEffect,
    /// [`HapticKind::Bumper`].
    pub bumper: RumbleEffect,
    /// [`HapticKind::BallDrop`].
    pub ball_drop: RumbleEffect,
    /// [`HapticKind::BallRoll`] (refreshed every tick the ball rolls).
    pub ball_roll: RumbleEffect,
    /// [`HapticKind::Impulse`] and kinds added later.
    pub impulse: RumbleEffect,
    /// Per-source overrides `(source id, effect)`, first match wins (see
    /// [`set_override`](Self::set_override)).
    pub overrides: ArrayVec<(u16, RumbleEffect), MAX_RUMBLE_OVERRIDES>,
}

impl Default for RumbleSettings {
    /// Planned defaults: coil 0.55/0.35 over 60 ms, EOS 0.25/0.6 over 30 ms,
    /// bumper 0.8/0.45 over 80 ms, ball drop 0.7/0.2 over 70 ms, ball roll
    /// 0.2/0.05 over 20 ms, generic impulse 0.5/0.5 over 50 ms; master 1.
    fn default() -> Self {
        Self {
            enabled: true,
            master: 1.0,
            coil_fire: RumbleEffect::new(0.55, 0.35, 60),
            eos_hit: RumbleEffect::new(0.25, 0.6, 30),
            bumper: RumbleEffect::new(0.8, 0.45, 80),
            ball_drop: RumbleEffect::new(0.7, 0.2, 70),
            ball_roll: RumbleEffect::new(0.2, 0.05, 20),
            impulse: RumbleEffect::new(0.5, 0.5, 50),
            overrides: ArrayVec::new(),
        }
    }
}

impl RumbleSettings {
    /// Sets (or replaces) the effect for one source id
    /// ([`HapticEvent::source`]). Returns `false` when
    /// [`MAX_RUMBLE_OVERRIDES`] other sources already have one.
    pub fn set_override(&mut self, source: u16, effect: RumbleEffect) -> bool {
        if let Some(o) = self.overrides.iter_mut().find(|o| o.0 == source) {
            o.1 = effect;
            return true;
        }
        self.overrides.try_push((source, effect)).is_ok()
    }

    /// Builder form of [`set_override`](Self::set_override) (a full table
    /// ignores the override).
    pub fn with_override(mut self, source: u16, effect: RumbleEffect) -> Self {
        self.set_override(source, effect);
        self
    }

    /// Removes a source's override; returns whether there was one.
    pub fn clear_override(&mut self, source: u16) -> bool {
        if let Some(i) = self.overrides.iter().position(|o| o.0 == source) {
            self.overrides.remove(i);
            true
        } else {
            false
        }
    }

    /// The override for `source`, if any.
    pub fn override_for(&self, source: u16) -> Option<RumbleEffect> {
        self.overrides.iter().find(|o| o.0 == source).map(|o| o.1)
    }

    /// The effect used for `e` (override first, then its kind).
    pub fn effect_for(&self, e: &HapticEvent) -> RumbleEffect {
        if let Some(o) = self.override_for(e.source()) {
            return o;
        }
        match e.kind {
            HapticKind::CoilFire { .. } => self.coil_fire,
            HapticKind::EosHit { .. } => self.eos_hit,
            HapticKind::Bumper { .. } => self.bumper,
            HapticKind::BallDrop { .. } => self.ball_drop,
            HapticKind::BallRoll { .. } => self.ball_roll,
            _ => self.impulse,
        }
    }
}

/// Anything [`RumbleMapper::feed`] accepts: a [`HapticEvent`] or a
/// [`HapticsTelemetryEvent`], by value or by reference.
pub trait IntoHapticEvent {
    /// The event as a [`HapticEvent`].
    fn into_haptic_event(self) -> HapticEvent;
}

impl IntoHapticEvent for HapticEvent {
    fn into_haptic_event(self) -> HapticEvent {
        self
    }
}

impl IntoHapticEvent for &HapticEvent {
    fn into_haptic_event(self) -> HapticEvent {
        *self
    }
}

impl IntoHapticEvent for HapticsTelemetryEvent {
    fn into_haptic_event(self) -> HapticEvent {
        HapticEvent::from(self)
    }
}

impl IntoHapticEvent for &HapticsTelemetryEvent {
    fn into_haptic_event(self) -> HapticEvent {
        HapticEvent::from(*self)
    }
}

/// `0..=1`, NaN → 0.
#[inline]
fn unit(v: f32) -> f32 {
    if v > 0.0 { v.min(1.0) } else { 0.0 }
}

#[derive(Copy, Clone, Debug, Default)]
struct Envelope {
    start: u64,
    /// 0 = free slot.
    duration: u32,
    low: f32,
    high: f32,
}

impl Envelope {
    #[inline]
    fn factor(&self, tick: u64) -> f32 {
        if self.duration == 0 || tick < self.start {
            return 0.0;
        }
        let dt = tick - self.start;
        let d = u64::from(self.duration);
        if dt >= d {
            0.0
        } else {
            // dt < d ≤ u32::MAX: exact in f64.
            (1.0 - dt as f64 / d as f64) as f32
        }
    }

    #[inline]
    fn expired(&self, now: u64) -> bool {
        self.duration == 0 || self.start.saturating_add(u64::from(self.duration)) <= now
    }

    #[inline]
    fn sum_at(&self, now: u64) -> f32 {
        if now < self.start {
            // Not started yet: it still holds its full peak ahead.
            self.low + self.high
        } else {
            (self.low + self.high) * self.factor(now)
        }
    }
}

/// Maps haptic events to gamepad rumble (module docs for the rules).
#[derive(Clone, Debug)]
pub struct RumbleMapper {
    settings: RumbleSettings,
    slots: [Envelope; MAX_ACTIVE_RUMBLES],
    /// Latest tick seen by `feed` or `output_at`.
    now: u64,
    dropped: u64,
}

impl RumbleMapper {
    /// A mapper with no active rumble.
    pub fn new(settings: RumbleSettings) -> Self {
        Self {
            settings,
            slots: [Envelope::default(); MAX_ACTIVE_RUMBLES],
            now: 0,
            dropped: 0,
        }
    }

    /// The settings in use.
    pub fn settings(&self) -> &RumbleSettings {
        &self.settings
    }

    /// Replaces the settings (active envelopes keep their peaks).
    pub fn set_settings(&mut self, settings: RumbleSettings) {
        self.settings = settings;
    }

    /// Stops all rumble.
    pub fn reset(&mut self) {
        self.slots = [Envelope::default(); MAX_ACTIVE_RUMBLES];
    }

    /// Adds one event. Never allocates.
    pub fn feed(&mut self, event: impl IntoHapticEvent) {
        let e = event.into_haptic_event();
        self.now = self.now.max(e.tick);
        if !self.settings.enabled {
            return;
        }
        let fx = self.settings.effect_for(&e);
        let scale = unit(e.strength) * unit(self.settings.master);
        let env = Envelope {
            start: e.tick,
            duration: fx.duration_ticks,
            low: unit(fx.low) * scale,
            high: unit(fx.high) * scale,
        };
        if env.duration == 0 || (env.low <= 0.0 && env.high <= 0.0) {
            return;
        }
        let now = self.now;
        if let Some(s) = self.slots.iter_mut().find(|s| s.expired(now)) {
            *s = env;
            return;
        }
        let mut weakest = 0;
        let mut weakest_sum = f32::INFINITY;
        for (i, s) in self.slots.iter().enumerate() {
            let v = s.sum_at(now);
            if v < weakest_sum {
                weakest = i;
                weakest_sum = v;
            }
        }
        if env.low + env.high > weakest_sum {
            self.slots[weakest] = env;
        } else {
            self.dropped += 1;
        }
    }

    /// Motor intensities at `tick` (max over active envelopes, `0..=1`).
    pub fn output_at(&mut self, tick: u64) -> Rumble {
        self.now = self.now.max(tick);
        let mut r = Rumble::OFF;
        for s in &self.slots {
            let f = s.factor(tick);
            if f > 0.0 {
                r.low = r.low.max(s.low * f);
                r.high = r.high.max(s.high * f);
            }
        }
        r
    }

    /// Envelopes still running at the latest tick seen.
    pub fn active(&self) -> usize {
        self.slots.iter().filter(|s| !s.expired(self.now)).count()
    }

    /// Events dropped because every slot held a stronger envelope.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pin_core::{CoilId, FlipperId};

    fn coil(tick: u64, strength: f32) -> HapticEvent {
        HapticEvent::new(tick, HapticKind::CoilFire { coil: CoilId(3) }, strength)
    }

    #[test]
    fn envelope_decays_linearly_to_zero() {
        let mut m = RumbleMapper::new(RumbleSettings::default());
        m.feed(coil(100, 1.0));
        assert_eq!(m.output_at(99), Rumble::OFF);
        let r = m.output_at(100);
        assert_eq!((r.low, r.high), (0.55, 0.35));
        let r = m.output_at(130);
        assert!((r.low - 0.275).abs() < 1e-6);
        assert!(m.output_at(159).low > 0.0);
        assert_eq!(m.output_at(160), Rumble::OFF);
        assert_eq!(m.active(), 0);
        assert!(m.output_at(1000).is_off());
    }

    #[test]
    fn overlap_merges_by_max() {
        let mut m = RumbleMapper::new(RumbleSettings::default());
        m.feed(coil(0, 1.0));
        m.feed(coil(0, 1.0));
        m.feed(HapticEvent::new(
            0,
            HapticKind::EosHit {
                flipper: FlipperId(0),
            },
            1.0,
        ));
        let r = m.output_at(0);
        assert_eq!((r.low, r.high), (0.55, 0.6));
        assert_eq!(m.active(), 3);
    }

    #[test]
    // Exercises the by-reference `IntoHapticEvent` impls on purpose.
    #[allow(clippy::needless_borrows_for_generic_args)]
    fn overrides_master_and_disable() {
        let s = RumbleSettings::default().with_override(3, RumbleEffect::new(1.0, 0.0, 10));
        assert_eq!(s.override_for(3), Some(RumbleEffect::new(1.0, 0.0, 10)));
        let mut s2 = s.clone();
        assert!(s2.set_override(3, RumbleEffect::NONE));
        assert!(s2.clear_override(3));
        assert!(!s2.clear_override(3));
        let mut m = RumbleMapper::new(s);
        let strong = coil(0, 2.0);
        m.feed(&strong);
        assert_eq!(strong.tick, 0);
        assert_eq!(
            m.output_at(0),
            Rumble {
                low: 1.0,
                high: 0.0
            }
        );
        assert_eq!(m.output_at(10), Rumble::OFF);

        let mut s = RumbleSettings {
            master: 0.5,
            ..RumbleSettings::default()
        };
        let mut m = RumbleMapper::new(s.clone());
        m.feed(HapticsTelemetryEvent {
            tick: 0,
            source: 9,
            strength: 1.0,
        });
        assert_eq!(
            m.output_at(0),
            Rumble {
                low: 0.25,
                high: 0.25
            }
        );
        s.enabled = false;
        m.set_settings(s);
        assert!(!m.settings().enabled);
        m.reset();
        let legacy = HapticsTelemetryEvent {
            tick: 1,
            source: 9,
            strength: 1.0,
        };
        m.feed(&legacy);
        assert_eq!(legacy.source, 9);
        assert!(m.output_at(1).is_off());
        let mut full = RumbleSettings::default();
        for i in 0..MAX_RUMBLE_OVERRIDES as u16 {
            assert!(full.set_override(i, RumbleEffect::NONE));
        }
        assert!(!full.set_override(999, RumbleEffect::NONE));
    }

    #[test]
    fn full_table_replaces_weakest_or_drops() {
        let mut m = RumbleMapper::new(RumbleSettings::default());
        for _ in 0..MAX_ACTIVE_RUMBLES {
            m.feed(coil(0, 0.5));
        }
        assert_eq!(m.active(), MAX_ACTIVE_RUMBLES);
        m.feed(coil(0, 0.25));
        assert_eq!(m.dropped(), 1);
        m.feed(coil(0, 1.0));
        assert_eq!(m.dropped(), 1);
        assert_eq!(m.output_at(0).low, 0.55);
        // Future event (not yet started) keeps its full weight.
        m.feed(coil(5, 0.9));
        assert!(m.output_at(5).low > 0.4);
    }
}
