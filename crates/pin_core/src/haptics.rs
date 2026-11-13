//! Rich haptic events (CR-003, Subphase 3.4).
//!
//! The sim thread emits one [`HapticEvent`] per physical happening that the
//! cabinet can feel: a coil firing, a flipper reaching its end-of-stroke stop,
//! a pop bumper, a ball dropping onto the playfield and, every tick a ball
//! rolls, its position, mass and speed. The events travel through lock-free
//! rings (`pin_input::haptics`) to gamepad rumble, the SSF exciter mixer and
//! the UDP telemetry stream.
//!
//! The older [`HapticsTelemetryEvent`] (tick, source, strength) keeps working:
//! it converts into [`HapticKind::Impulse`], and every impulse kind converts
//! back with [`HapticEvent::to_telemetry_event`].
//!
//! # Source ids
//! Telemetry (Appendix B kind 5) and per-source settings identify an event by
//! a `u16` source id, [`HapticEvent::source`]: the top four bits name the
//! class ([`SOURCE_CLASS_COIL`] = 0, [`SOURCE_CLASS_EOS`] = 1,
//! [`SOURCE_CLASS_BUMPER`] = 2, [`SOURCE_CLASS_BALL_DROP`] = 3,
//! [`SOURCE_CLASS_BALL_ROLL`] = 4) and the low twelve bits the coil, flipper,
//! mech or ball number (masked with [`SOURCE_INDEX_MASK`], so numbers above
//! 4095 wrap). [`HapticKind::Impulse`] passes its source through unchanged.

use crate::ids::{CoilId, FlipperId};
use crate::input::HapticsTelemetryEvent;

/// Bit position of the class in a source id.
pub const SOURCE_CLASS_SHIFT: u16 = 12;
/// Mask of the element number in a source id.
pub const SOURCE_INDEX_MASK: u16 = 0x0FFF;
/// Source class of [`HapticKind::CoilFire`].
pub const SOURCE_CLASS_COIL: u16 = 0;
/// Source class of [`HapticKind::EosHit`].
pub const SOURCE_CLASS_EOS: u16 = 1;
/// Source class of [`HapticKind::Bumper`].
pub const SOURCE_CLASS_BUMPER: u16 = 2;
/// Source class of [`HapticKind::BallDrop`].
pub const SOURCE_CLASS_BALL_DROP: u16 = 3;
/// Source class of [`HapticKind::BallRoll`].
pub const SOURCE_CLASS_BALL_ROLL: u16 = 4;

/// The source id of element `index` in class `class` (see the module docs).
#[inline]
pub const fn class_source(class: u16, index: u16) -> u16 {
    ((class & 0xF) << SOURCE_CLASS_SHIFT) | (index & SOURCE_INDEX_MASK)
}

/// What happened.
#[non_exhaustive]
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum HapticKind {
    /// A coil fired (flipper, sling, kicker, ...).
    CoilFire {
        /// The coil.
        coil: CoilId,
    },
    /// A flipper hit its end-of-stroke stop.
    EosHit {
        /// The flipper.
        flipper: FlipperId,
    },
    /// A pop bumper fired.
    Bumper {
        /// The bumper's mech index.
        mech: u16,
    },
    /// A ball landed on the playfield after being airborne.
    BallDrop {
        /// Ball slot.
        ball: u16,
        /// Landing position, mm (playfield frame).
        position_mm: [f32; 3],
    },
    /// A ball is rolling (sent every tick it rolls).
    BallRoll {
        /// Ball slot.
        ball: u16,
        /// Position, mm (playfield frame).
        position_mm: [f32; 3],
        /// Ball mass, kg.
        mass_kg: f32,
        /// Ball speed, m/s.
        speed_m_s: f32,
    },
    /// A generic impulse from a [`HapticsTelemetryEvent`].
    Impulse {
        /// Raw source id.
        source: u16,
    },
}

/// One haptic event from the sim thread.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct HapticEvent {
    /// Sim tick.
    pub tick: u64,
    /// What happened.
    pub kind: HapticKind,
    /// Strength `0..=1` (consumers clamp; NaN counts as 0).
    pub strength: f32,
}

impl HapticEvent {
    /// Builds an event.
    #[inline]
    pub const fn new(tick: u64, kind: HapticKind, strength: f32) -> Self {
        Self {
            tick,
            kind,
            strength,
        }
    }

    /// The `u16` source id (module docs, "Source ids").
    pub fn source(&self) -> u16 {
        match self.kind {
            HapticKind::CoilFire { coil } => class_source(SOURCE_CLASS_COIL, coil.0),
            HapticKind::EosHit { flipper } => class_source(SOURCE_CLASS_EOS, flipper.0),
            HapticKind::Bumper { mech } => class_source(SOURCE_CLASS_BUMPER, mech),
            HapticKind::BallDrop { ball, .. } => class_source(SOURCE_CLASS_BALL_DROP, ball),
            HapticKind::BallRoll { ball, .. } => class_source(SOURCE_CLASS_BALL_ROLL, ball),
            HapticKind::Impulse { source } => source,
        }
    }

    /// True for one-shot kinds (everything but [`HapticKind::BallRoll`]).
    pub fn is_impulse(&self) -> bool {
        !matches!(self.kind, HapticKind::BallRoll { .. })
    }

    /// The position carried by the event, if any (mm).
    pub fn position_mm(&self) -> Option<[f32; 3]> {
        match self.kind {
            HapticKind::BallDrop { position_mm, .. } | HapticKind::BallRoll { position_mm, .. } => {
                Some(position_mm)
            }
            _ => None,
        }
    }

    /// The strength clamped to `0..=1` (NaN → 0).
    pub fn clamped_strength(&self) -> f32 {
        if self.strength > 0.0 {
            self.strength.min(1.0)
        } else {
            0.0
        }
    }

    /// The telemetry impulse for this event (Appendix B kind 5), or `None`
    /// for a continuous [`HapticKind::BallRoll`].
    pub fn to_telemetry_event(&self) -> Option<HapticsTelemetryEvent> {
        self.is_impulse().then(|| HapticsTelemetryEvent {
            tick: self.tick,
            source: self.source(),
            strength: self.strength,
        })
    }
}

impl From<HapticsTelemetryEvent> for HapticEvent {
    fn from(e: HapticsTelemetryEvent) -> Self {
        Self {
            tick: e.tick,
            kind: HapticKind::Impulse { source: e.source },
            strength: e.strength,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_and_conversions() {
        let coil = HapticEvent::new(5, HapticKind::CoilFire { coil: CoilId(7) }, 0.5);
        assert_eq!(coil.source(), 7);
        let eos = HapticEvent::new(5, HapticKind::EosHit { flipper: FlipperId(1) }, 0.5);
        assert_eq!(eos.source(), 0x1001);
        let bump = HapticEvent::new(5, HapticKind::Bumper { mech: 0x1FFF }, 0.5);
        assert_eq!(bump.source(), 0x2FFF);
        let drop = HapticEvent::new(
            5,
            HapticKind::BallDrop {
                ball: 2,
                position_mm: [1.0, 2.0, 3.0],
            },
            2.0,
        );
        assert_eq!(drop.source(), 0x3002);
        assert_eq!(drop.position_mm(), Some([1.0, 2.0, 3.0]));
        assert_eq!(drop.clamped_strength(), 1.0);
        let roll = HapticEvent::new(
            5,
            HapticKind::BallRoll {
                ball: 3,
                position_mm: [0.0; 3],
                mass_kg: 0.08,
                speed_m_s: 1.0,
            },
            f32::NAN,
        );
        assert_eq!(roll.source(), 0x4003);
        assert!(!roll.is_impulse());
        assert_eq!(roll.to_telemetry_event(), None);
        assert_eq!(roll.clamped_strength(), 0.0);
        assert_eq!(coil.position_mm(), None);

        let old = HapticsTelemetryEvent {
            tick: 9,
            source: 0xABCD,
            strength: 0.25,
        };
        let new = HapticEvent::from(old);
        assert_eq!(new.source(), 0xABCD);
        assert_eq!(new.to_telemetry_event(), Some(old));
        assert_eq!(
            eos.to_telemetry_event().map(|e| e.source),
            Some(class_source(SOURCE_CLASS_EOS, 1))
        );
    }
}
