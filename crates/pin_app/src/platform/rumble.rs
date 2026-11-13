//! Gamepad rumble output (Subphase 3.4).
//!
//! The main thread owns a [`RumbleDriver`]: each loop iteration it calls
//! [`RumbleDriver::poll`] with the current sim tick, which drains the haptics
//! ring into the SDL-free [`RumbleMapper`] (`pin_input::haptics`) and says
//! whether the motors need a new command; [`Platform::set_gamepad_rumble`]
//! then hands that command to every open gamepad (`SDL_RumbleGamepad`). All
//! decisions live in the pure `poll`; the SDL part is one call per pad.
//!
//! # Command rules
//! * Motor levels are the mapper's `0..=1` outputs scaled to `0..=65535`
//!   and rounded ([`motor_level`]).
//! * A command is issued when the levels change, and again every
//!   [`RUMBLE_RESEND_TICKS`] while they are non-zero. Non-zero commands last
//!   [`RUMBLE_COMMAND_MS`], so a stalled main thread lets the pad fall silent
//!   by itself; the "off" command is `(0, 0, 0)`.

use super::Platform;
use pin_input::haptics::{HapticEvent, HapticsConsumer, Rumble, RumbleMapper, RumbleSettings};

/// Duration attached to a non-zero rumble command, ms.
pub const RUMBLE_COMMAND_MS: u32 = 100;

/// Ticks after which an unchanged non-zero command is sent again (well
/// within [`RUMBLE_COMMAND_MS`]).
pub const RUMBLE_RESEND_TICKS: u64 = 50;

/// One rumble command for SDL.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct RumbleCommand {
    /// Low-frequency motor, `0..=65535`.
    pub low: u16,
    /// High-frequency motor, `0..=65535`.
    pub high: u16,
    /// How long SDL keeps the motors at these levels, ms (0 = stop).
    pub duration_ms: u32,
}

impl RumbleCommand {
    /// Motors off.
    pub const OFF: RumbleCommand = RumbleCommand {
        low: 0,
        high: 0,
        duration_ms: 0,
    };

    /// The command for mapper output `r`.
    pub fn from_rumble(r: Rumble) -> Self {
        let (low, high) = (motor_level(r.low), motor_level(r.high));
        if low == 0 && high == 0 {
            Self::OFF
        } else {
            Self {
                low,
                high,
                duration_ms: RUMBLE_COMMAND_MS,
            }
        }
    }
}

/// `0..=1` → `0..=65535`, rounded (NaN and negatives → 0, above 1 → 65535).
pub fn motor_level(v: f32) -> u16 {
    if v > 0.0 {
        // In 0.0..=65535.0: the cast is exact after rounding.
        (f64::from(v.min(1.0)) * 65535.0).round() as u16
    } else {
        0
    }
}

/// Main-thread rumble state: the haptics ring, the mapper and the last
/// command sent.
pub struct RumbleDriver {
    mapper: RumbleMapper,
    rx: HapticsConsumer<HapticEvent>,
    last: RumbleCommand,
    last_sent: u64,
}

impl RumbleDriver {
    /// A driver reading `rx` with `settings`.
    pub fn new(settings: RumbleSettings, rx: HapticsConsumer<HapticEvent>) -> Self {
        Self {
            mapper: RumbleMapper::new(settings),
            rx,
            last: RumbleCommand::OFF,
            last_sent: 0,
        }
    }

    /// The mapper (for settings changes).
    pub fn mapper_mut(&mut self) -> &mut RumbleMapper {
        &mut self.mapper
    }

    /// Drains the ring and returns the command to send at `tick`, if any
    /// (module docs, "Command rules"). Never allocates.
    pub fn poll(&mut self, tick: u64) -> Option<RumbleCommand> {
        let mapper = &mut self.mapper;
        self.rx.drain(|e| mapper.feed(e));
        let cmd = RumbleCommand::from_rumble(self.mapper.output_at(tick));
        let changed = (cmd.low, cmd.high) != (self.last.low, self.last.high);
        let refresh =
            cmd != RumbleCommand::OFF && tick.saturating_sub(self.last_sent) >= RUMBLE_RESEND_TICKS;
        if changed || refresh {
            self.last = cmd;
            self.last_sent = tick;
            Some(cmd)
        } else {
            None
        }
    }
}

impl Platform {
    /// Sends `cmd` to every open gamepad; returns how many accepted it
    /// (pads without rumble refuse). Main thread only.
    pub fn set_gamepad_rumble(&mut self, cmd: RumbleCommand) -> usize {
        let mut accepted = 0;
        for g in &mut self.gamepads {
            if g.set_rumble(cmd.low, cmd.high, cmd.duration_ms).is_ok() {
                accepted += 1;
            }
        }
        accepted
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pin_core::CoilId;
    use pin_core::haptics::HapticKind;
    use pin_input::haptics::haptics_ring;

    #[test]
    fn levels() {
        assert_eq!(motor_level(f32::NAN), 0);
        assert_eq!(motor_level(-1.0), 0);
        assert_eq!(motor_level(0.5), 32768);
        assert_eq!(motor_level(2.0), 65535);
        assert_eq!(RumbleCommand::from_rumble(Rumble::OFF), RumbleCommand::OFF);
    }

    #[test]
    fn driver_sends_changes_refreshes_and_stops() {
        let (mut tx, rx) = haptics_ring(8);
        let mut d = RumbleDriver::new(RumbleSettings::default(), rx);
        assert_eq!(d.poll(0), None);
        tx.push(HapticEvent::new(
            10,
            HapticKind::CoilFire { coil: CoilId(0) },
            1.0,
        ));
        let c = d.poll(10).unwrap();
        assert_eq!(c.duration_ms, RUMBLE_COMMAND_MS);
        assert_eq!(c.low, motor_level(0.55));
        // Decaying: changes every tick.
        assert!(d.poll(11).is_some());
        // Long event: a hold would be re-sent after RUMBLE_RESEND_TICKS.
        d.mapper_mut().reset();
        assert_eq!(d.poll(12), Some(RumbleCommand::OFF));
        assert_eq!(d.poll(13), None);
        let mut s = RumbleSettings::default();
        s.coil_fire.duration_ticks = u32::MAX;
        d.mapper_mut().set_settings(s);
        tx.push(HapticEvent::new(
            100,
            HapticKind::CoilFire { coil: CoilId(0) },
            1.0,
        ));
        assert!(d.poll(100).is_some());
        assert_eq!(d.poll(101), None);
        assert!(d.poll(150).is_some());
    }
}
