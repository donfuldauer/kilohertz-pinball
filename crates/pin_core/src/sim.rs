//! The 1 kHz simulation step contract (CR-002, PLAN §2.2).
//!
//! The thread orchestrator (`pin_app`) owns the sim thread and its timing; the
//! simulation (`pin_sim`, Subphases 2.5/6.0) implements [`SimStep`]. Each call
//! computes exactly one tick of [`TICK_NS`] simulated nanoseconds and writes
//! the complete resulting [`FrameStateSnapshot`]. The orchestrator publishes
//! one snapshot per tick through a [`TripleBuffer`](crate::TripleBuffer).
//!
//! Implementations run on the sim thread: no heap allocation, no locks and no
//! file I/O inside [`SimStep::step_1ms`] (D5).

use crate::input::InputEvent;
use crate::snapshot::FrameStateSnapshot;

/// Length of one sim tick, nanoseconds (1 ms, PLAN §2.2).
pub const TICK_NS: u64 = 1_000_000;

/// Most ticks the sim thread runs back-to-back to catch up after a stall.
/// Whole ticks beyond the cap are dropped (simulated time slips against wall
/// time) instead of spiralling.
pub const MAX_CATCH_UP_TICKS: u32 = 8;

/// One simulation tick.
pub trait SimStep: Send {
    /// Computes tick `tick` (1-based: the first call gets 1). `inputs` holds the input
    /// events whose timestamp is ≤ the end of this tick, in arrival order. Writes the
    /// complete new state into `out` (out.tick must be set to `tick`).
    ///
    /// `out` holds the previous tick's snapshot on entry (the default snapshot
    /// before the first tick); implementations must not rely on that and must
    /// overwrite every field they own.
    fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot);
}

/// A boxed step is a step, so the orchestrator can drive a `Box<dyn SimStep>`.
impl<S: SimStep + ?Sized> SimStep for Box<S> {
    #[inline]
    fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
        (**self).step_1ms(tick, inputs, out);
    }
}

/// End of tick `tick` in simulated nanoseconds after `epoch_ns`
/// (`epoch_ns + tick · TICK_NS`), saturating at `u64::MAX`.
#[inline]
pub const fn tick_end_ns(epoch_ns: u64, tick: u64) -> u64 {
    epoch_ns.saturating_add(tick.saturating_mul(TICK_NS))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::CabinetInput;

    struct Counting {
        calls: u64,
        inputs_seen: usize,
    }

    impl SimStep for Counting {
        fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
            self.calls += 1;
            self.inputs_seen += inputs.len();
            *out = FrameStateSnapshot {
                tick,
                ..FrameStateSnapshot::default()
            };
        }
    }

    #[test]
    fn constants() {
        assert_eq!(TICK_NS, 1_000_000);
        assert_eq!(MAX_CATCH_UP_TICKS, 8);
    }

    #[test]
    fn boxed_trait_object_forwards() {
        let mut b: Box<dyn SimStep> = Box::new(Counting {
            calls: 0,
            inputs_seen: 0,
        });
        let mut out = FrameStateSnapshot::default();
        let ev = [InputEvent::button(5, CabinetInput::Start, true)];
        b.step_1ms(1, &ev, &mut out);
        assert_eq!(out.tick, 1);
        b.step_1ms(2, &[], &mut out);
        assert_eq!(out.tick, 2);
        // Box<Box<dyn SimStep>> also works through the blanket impl.
        let mut bb = Box::new(b);
        bb.step_1ms(3, &ev, &mut out);
        assert_eq!(out.tick, 3);
    }

    #[test]
    fn concrete_step_counts() {
        let mut c = Counting {
            calls: 0,
            inputs_seen: 0,
        };
        let mut out = FrameStateSnapshot::default();
        let ev = [
            InputEvent::button(1, CabinetInput::LeftFlipper, true),
            InputEvent::button(2, CabinetInput::LeftFlipper, false),
        ];
        c.step_1ms(1, &ev, &mut out);
        c.step_1ms(2, &[], &mut out);
        assert_eq!((c.calls, c.inputs_seen, out.tick), (2, 2, 2));
    }

    #[test]
    fn tick_end_saturates() {
        assert_eq!(tick_end_ns(0, 0), 0);
        assert_eq!(tick_end_ns(0, 1), TICK_NS);
        assert_eq!(tick_end_ns(500, 3), 500 + 3 * TICK_NS);
        assert_eq!(tick_end_ns(u64::MAX - 1, 1), u64::MAX);
        assert_eq!(tick_end_ns(0, u64::MAX), u64::MAX);
    }

    #[test]
    fn trait_objects_are_send() {
        fn assert_send<T: Send + ?Sized>() {}
        assert_send::<dyn SimStep>();
        assert_send::<Box<dyn SimStep>>();
    }
}
