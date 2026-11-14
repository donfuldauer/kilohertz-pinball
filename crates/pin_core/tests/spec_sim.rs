//! Independent spec tests for `pin_core::sim` (CR-002, Subphase 1.4 FIXED API).
//!
//! Written from the brief alone: the `SimStep` trait is object safe and `Send`, ticks are
//! 1-based, and the two timing constants have the values the brief fixes.

use pin_core::sim::{MAX_CATCH_UP_TICKS, SimStep, TICK_NS};
use pin_core::{CabinetInput, FrameStateSnapshot, InputEvent};

#[test]
fn constants_match_the_brief() {
    let tick_ns: u64 = TICK_NS;
    let cap: u32 = MAX_CATCH_UP_TICKS;
    assert_eq!(tick_ns, 1_000_000, "1 kHz: one tick is 1 ms");
    assert_eq!(cap, 8, "catch-up cap is 8 ticks");
}

/// A toy sim: counts inputs into lamps[0] and records the tick.
struct Toy {
    calls: u64,
    inputs_seen: u64,
}

impl SimStep for Toy {
    fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
        self.calls += 1;
        self.inputs_seen += inputs.len() as u64;
        out.tick = tick;
        out.lamps[0] = self.inputs_seen as f32;
    }
}

#[test]
fn sim_step_is_object_safe_and_send() {
    fn assert_send<T: Send + ?Sized>() {}
    assert_send::<Box<dyn SimStep>>();
    assert_send::<dyn SimStep>();
    assert_send::<Toy>();

    let mut boxed: Box<dyn SimStep> = Box::new(Toy {
        calls: 0,
        inputs_seen: 0,
    });
    let mut out = FrameStateSnapshot::default();
    let ev = [
        InputEvent::button(10, CabinetInput::LeftFlipper, true),
        InputEvent::button(20, CabinetInput::LeftFlipper, false),
    ];
    boxed.step_1ms(1, &ev, &mut out);
    assert_eq!(out.tick, 1);
    assert_eq!(out.lamps[0], 2.0);
    boxed.step_1ms(2, &[], &mut out);
    assert_eq!(out.tick, 2);
    assert_eq!(out.lamps[0], 2.0);
}

#[test]
fn a_boxed_sim_can_move_to_another_thread() {
    let mut boxed: Box<dyn SimStep> = Box::new(Toy {
        calls: 0,
        inputs_seen: 0,
    });
    let last = std::thread::spawn(move || {
        let mut out = FrameStateSnapshot::default();
        for tick in 1..=100u64 {
            boxed.step_1ms(tick, &[], &mut out);
        }
        out.tick
    })
    .join()
    .expect("sim thread");
    assert_eq!(last, 100);
}

#[test]
fn generic_use_through_a_mutable_reference() {
    fn drive<S: SimStep + ?Sized>(s: &mut S, n: u64) -> u64 {
        let mut out = FrameStateSnapshot::default();
        for tick in 1..=n {
            s.step_1ms(tick, &[], &mut out);
        }
        out.tick
    }
    let mut toy = Toy {
        calls: 0,
        inputs_seen: 0,
    };
    assert_eq!(drive(&mut toy, 17), 17);
    assert_eq!(toy.calls, 17);
}
