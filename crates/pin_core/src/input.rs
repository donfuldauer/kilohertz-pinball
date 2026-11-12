//! Input and haptics contracts (PLAN.md §2.2, §3.4).
//!
//! The main thread converts SDL events into [`InputEvent`]s, keeping SDL's
//! nanosecond timestamp, and forwards them to the sim thread through an SPSC
//! ring. The sim drains every event with `timestamp_ns` ≤ the tick time.

use crate::telemetry::{TelemetryEntry, TelemetryKind};
use serde::{Deserialize, Serialize};

/// The eight reserved cabinet switches (§3.4). Their switch numbers are declared
/// in `[cabinet_switches]` of `table.toml`; scripts see them as ordinary switches.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CabinetInput {
    /// Lower-left flipper button.
    LeftFlipper,
    /// Lower-right flipper button.
    RightFlipper,
    /// Upper-left flipper button (or second stage).
    LeftUpper,
    /// Upper-right flipper button (or second stage).
    RightUpper,
    /// Action / fire button.
    Action,
    /// Start button.
    Start,
    /// Launch button (autoplunger).
    Launch,
    /// Tilt bob.
    Tilt,
}

impl CabinetInput {
    /// Number of cabinet inputs.
    pub const COUNT: usize = 8;

    /// Every cabinet input, in `[cabinet_switches]` order.
    pub const ALL: [CabinetInput; 8] = [
        CabinetInput::LeftFlipper,
        CabinetInput::RightFlipper,
        CabinetInput::LeftUpper,
        CabinetInput::RightUpper,
        CabinetInput::Action,
        CabinetInput::Start,
        CabinetInput::Launch,
        CabinetInput::Tilt,
    ];

    /// Position in [`CabinetInput::ALL`].
    pub const fn index(self) -> usize {
        self as usize
    }

    /// The `[cabinet_switches]` key, e.g. `left_flipper`.
    pub const fn name(self) -> &'static str {
        match self {
            CabinetInput::LeftFlipper => "left_flipper",
            CabinetInput::RightFlipper => "right_flipper",
            CabinetInput::LeftUpper => "left_upper",
            CabinetInput::RightUpper => "right_upper",
            CabinetInput::Action => "action",
            CabinetInput::Start => "start",
            CabinetInput::Launch => "launch",
            CabinetInput::Tilt => "tilt",
        }
    }

    /// Parses a `[cabinet_switches]` key.
    pub fn from_name(name: &str) -> Option<CabinetInput> {
        CabinetInput::ALL.into_iter().find(|c| c.name() == name)
    }
}

/// Analog cabinet axes.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CabinetAxis {
    /// Plunger position, 0 = rest, 1 = fully pulled.
    Plunger,
    /// Nudge acceleration, left/right.
    NudgeX,
    /// Nudge acceleration, front/back.
    NudgeY,
    /// Analog (dual-stage) left flipper button travel, 0..1.
    LeftFlipper,
    /// Analog (dual-stage) right flipper button travel, 0..1.
    RightFlipper,
}

impl CabinetAxis {
    /// Every axis.
    pub const ALL: [CabinetAxis; 5] = [
        CabinetAxis::Plunger,
        CabinetAxis::NudgeX,
        CabinetAxis::NudgeY,
        CabinetAxis::LeftFlipper,
        CabinetAxis::RightFlipper,
    ];
}

/// What happened.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum InputEventKind {
    /// A cabinet button changed state.
    Button {
        /// Which button.
        input: CabinetInput,
        /// `true` = pressed (switch closed).
        pressed: bool,
    },
    /// An analog axis moved. Values are normalised: `0..=1` for plunger and
    /// flipper travel, `-1..=1` for nudge.
    Axis {
        /// Which axis.
        axis: CabinetAxis,
        /// New value.
        value: f32,
    },
}

/// One input event, timestamped with SDL's nanosecond clock (`SDL_GetTicksNS`).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct InputEvent {
    /// SDL event timestamp, nanoseconds.
    pub timestamp_ns: u64,
    /// The event.
    pub kind: InputEventKind,
}

impl InputEvent {
    /// A button event.
    pub const fn button(timestamp_ns: u64, input: CabinetInput, pressed: bool) -> Self {
        Self {
            timestamp_ns,
            kind: InputEventKind::Button { input, pressed },
        }
    }

    /// An axis event.
    pub const fn axis(timestamp_ns: u64, axis: CabinetAxis, value: f32) -> Self {
        Self {
            timestamp_ns,
            kind: InputEventKind::Axis { axis, value },
        }
    }
}

/// A haptic impulse produced by the sim (flipper fire, bumper, knocker, ...),
/// routed to rumble/SSF on the audio side and to UDP telemetry (kind 5).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct HapticsTelemetryEvent {
    /// Sim tick of the impulse.
    pub tick: u64,
    /// Haptic source id (e.g. the coil or mech that fired).
    pub source: u16,
    /// Strength `0..=1`.
    pub strength: f32,
}

impl HapticsTelemetryEvent {
    /// The telemetry entry for this impulse (Appendix B kind 5). Strength is
    /// clamped to `0..=1`; NaN becomes 0.
    pub fn to_telemetry_entry(&self) -> TelemetryEntry {
        let s = if self.strength.is_nan() {
            0.0
        } else {
            self.strength.clamp(0.0, 1.0)
        };
        TelemetryEntry {
            kind: TelemetryKind::HapticImpulse,
            id: self.source,
            value: s,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cabinet_names_round_trip_and_index() {
        for (i, c) in CabinetInput::ALL.into_iter().enumerate() {
            assert_eq!(c.index(), i);
            assert_eq!(CabinetInput::from_name(c.name()), Some(c));
        }
        assert_eq!(CabinetInput::from_name("coin"), None);
        assert_eq!(CabinetAxis::ALL.len(), 5);
    }

    #[test]
    fn constructors() {
        let e = InputEvent::button(5, CabinetInput::Start, true);
        assert_eq!(e.timestamp_ns, 5);
        assert_eq!(
            e.kind,
            InputEventKind::Button {
                input: CabinetInput::Start,
                pressed: true
            }
        );
        let a = InputEvent::axis(9, CabinetAxis::Plunger, 0.5);
        assert_eq!(
            a.kind,
            InputEventKind::Axis {
                axis: CabinetAxis::Plunger,
                value: 0.5
            }
        );
    }

    #[test]
    fn haptic_entry_clamps() {
        let mut h = HapticsTelemetryEvent {
            tick: 1,
            source: 3,
            strength: 2.0,
        };
        let e = h.to_telemetry_entry();
        assert_eq!((e.kind, e.id, e.value), (TelemetryKind::HapticImpulse, 3, 1.0));
        h.strength = f32::NAN;
        assert_eq!(h.to_telemetry_entry().value, 0.0);
        h.strength = 0.25;
        assert_eq!(h.to_telemetry_entry().value, 0.25);
    }
}
