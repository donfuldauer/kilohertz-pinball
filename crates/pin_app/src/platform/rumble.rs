//! Gamepad rumble output (Subphase 3.4).
//!
//! The main thread owns a [`RumbleDriver`]: each loop iteration it calls
//! [`RumbleDriver::service`] with the latest sim tick, which drains the
//! haptics ring into the SDL-free [`RumbleMapper`] (`pin_input::haptics`)
//! ([`RumbleDriver::poll`]) and hands a new command, if any, to a
//! [`RumbleOutput`]: [`Platform::set_gamepad_rumble`] sends it to every open
//! gamepad (`SDL_RumbleGamepad`). All decisions live in the pure `poll`; the
//! SDL part is one call per pad.
//!
//! # Sessions
//! A table load, a table swap and a new sim session (the tick going
//! backwards) start from silence: [`RumbleDriver::reset`] drops every queued
//! impulse and every running envelope and stops the motors if they were
//! on, so no envelope of the previous session survives (KP-006), however
//! soon the new session starts.
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

/// Where rumble commands go: every open gamepad ([`Platform`]) or a test
/// double.
pub trait RumbleOutput {
    /// Applies `cmd` to the motors; returns how many devices accepted it.
    fn set_gamepad_rumble(&mut self, cmd: RumbleCommand) -> usize;
}

impl RumbleOutput for Platform {
    fn set_gamepad_rumble(&mut self, cmd: RumbleCommand) -> usize {
        Platform::set_gamepad_rumble(self, cmd)
    }
}

/// Main-thread rumble state: the haptics ring, the mapper and the last
/// command sent.
pub struct RumbleDriver {
    mapper: RumbleMapper,
    rx: HapticsConsumer<HapticEvent>,
    last: RumbleCommand,
    last_sent: u64,
    /// The tick of the previous [`poll`](Self::poll) (`None` after a reset).
    last_tick: Option<u64>,
}

impl RumbleDriver {
    /// A driver reading `rx` with `settings`.
    pub fn new(settings: RumbleSettings, rx: HapticsConsumer<HapticEvent>) -> Self {
        Self {
            mapper: RumbleMapper::new(settings),
            rx,
            last: RumbleCommand::OFF,
            last_sent: 0,
            last_tick: None,
        }
    }

    /// Starts from silence (a table load, swap or new session): drains and
    /// discards the ring, resets the mapper (no envelope survives) and
    /// clears the command bookkeeping. Returns [`RumbleCommand::OFF`] when
    /// the last command sent was not "off" (send it so the motors stop),
    /// else `None`. Never allocates.
    pub fn reset(&mut self) -> Option<RumbleCommand> {
        self.rx.drain(|_| {});
        self.mapper.reset();
        let was_on = self.last != RumbleCommand::OFF;
        self.last = RumbleCommand::OFF;
        self.last_sent = 0;
        self.last_tick = None;
        was_on.then_some(RumbleCommand::OFF)
    }

    /// One main-loop step at sim tick `tick`: [`poll`](Self::poll) and hand
    /// the resulting command, if any, to `out`. A `tick` lower than the
    /// previous one (a new session restarted at tick 0) first performs
    /// [`reset`](Self::reset), sending its "off" through `out` when needed.
    /// Returns the command handed to `out` last, if any. Never allocates.
    pub fn service(&mut self, tick: u64, out: &mut impl RumbleOutput) -> Option<RumbleCommand> {
        let mut sent = None;
        if self.last_tick.is_some_and(|t| tick < t)
            && let Some(off) = self.reset()
        {
            out.set_gamepad_rumble(off);
            sent = Some(off);
        }
        if let Some(cmd) = self.poll(tick) {
            out.set_gamepad_rumble(cmd);
            sent = Some(cmd);
        }
        sent
    }

    /// The mapper (for settings changes).
    pub fn mapper_mut(&mut self) -> &mut RumbleMapper {
        &mut self.mapper
    }

    /// Drains the ring and returns the command to send at `tick`, if any
    /// (module docs, "Command rules"). Never allocates.
    pub fn poll(&mut self, tick: u64) -> Option<RumbleCommand> {
        self.last_tick = Some(tick);
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

    /// Records every command it is given.
    #[derive(Default)]
    struct Pads(Vec<RumbleCommand>);

    impl RumbleOutput for Pads {
        fn set_gamepad_rumble(&mut self, cmd: RumbleCommand) -> usize {
            self.0.push(cmd);
            1
        }
    }

    fn fire(tick: u64) -> HapticEvent {
        HapticEvent::new(tick, HapticKind::CoilFire { coil: CoilId(0) }, 1.0)
    }

    /// KP-006 (SPEC §4 3.4): `reset` stops the motors only when they were
    /// on, and leaves no queued impulse or running envelope behind.
    #[test]
    fn kp006_reset_turns_off_only_running_motors() {
        let (mut tx, rx) = haptics_ring(8);
        let mut d = RumbleDriver::new(RumbleSettings::default(), rx);
        assert_eq!(d.reset(), None, "motors were never on");
        tx.push(fire(5));
        assert!(d.poll(5).is_some_and(|c| c != RumbleCommand::OFF));
        tx.push(fire(6)); // still queued at the reset: discarded
        assert_eq!(d.reset(), Some(RumbleCommand::OFF));
        assert_eq!(d.reset(), None, "already off");
        assert_eq!(d.mapper_mut().active(), 0);
        assert_eq!(d.poll(6), None, "the queued impulse was dropped");
        assert_eq!(d.poll(7), None);
    }

    /// KP-006 (SPEC §4 3.4): a tick going backwards in `service` is a new
    /// session: the old envelope is stopped at once and never resumes.
    #[test]
    fn kp006_service_resets_on_a_backwards_tick() {
        let (mut tx, rx) = haptics_ring(8);
        let mut s = RumbleSettings::default();
        s.coil_fire.duration_ticks = 2000;
        let mut d = RumbleDriver::new(s, rx);
        let mut pads = Pads::default();
        assert_eq!(d.service(0, &mut pads), None);
        tx.push(fire(300));
        let on = d.service(300, &mut pads).unwrap();
        assert_ne!(on, RumbleCommand::OFF);
        let _ = d.service(301, &mut pads);
        // New session at tick 0, well within the old envelope.
        assert_eq!(d.service(0, &mut pads), Some(RumbleCommand::OFF));
        let before = pads.0.len();
        for t in 1..2500 {
            assert_eq!(d.service(t, &mut pads), None, "tick {t}");
        }
        assert_eq!(pads.0.len(), before);
        // Monotonic ticks keep `poll`'s behaviour.
        tx.push(fire(2600));
        assert!(d.service(2600, &mut pads).is_some());
    }
}
