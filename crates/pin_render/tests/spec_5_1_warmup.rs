//! Independent spec tests for Subphase 5.1, verification criterion 4: lamp warm-up curves.
//! GPU-free, simulated time only.
//!
//! From the brief: `LampWarmup::new(kind)`, `update(&mut self, commanded, dt_s) -> f32`
//! (emitted, `0..=1`) is a first-order response of emitted intensity to the commanded
//! level. Incandescent: a 0→1 step reaches ≥ 0.95 at 15 ms and is < 0.95 at 10 ms.
//! LED: ≥ 0.95 at 1 ms. Deterministic.
//!
//! Derived properties asserted here (all follow from "first-order response"): the output
//! starts dark, moves monotonically toward the commanded level without overshoot, is
//! linear in the commanded level, falls as fast as it rises, and converges.

use pin_render::pbr::{LampKind, LampWarmup};
use proptest::prelude::*;

/// Sim tick: 1 ms of simulated time.
const TICK_S: f32 = 1.0e-3;

/// Drives a fresh lamp with a constant command for `steps` updates of `dt_s`; returns the
/// last emitted value.
fn step_response(kind: LampKind, commanded: f32, dt_s: f32, steps: u32) -> f32 {
    let mut lamp = LampWarmup::new(kind);
    let mut out = 0.0;
    for _ in 0..steps {
        out = lamp.update(commanded, dt_s);
    }
    out
}

/// A lamp driven at full command until it has settled at 1.
fn lit(kind: LampKind) -> LampWarmup {
    let mut lamp = LampWarmup::new(kind);
    for _ in 0..1000 {
        lamp.update(1.0, TICK_S);
    }
    lamp
}

#[test]
fn a_new_lamp_is_dark() {
    for kind in [LampKind::Incandescent, LampKind::Led] {
        let mut lamp = LampWarmup::new(kind);
        assert_eq!(lamp.update(0.0, TICK_S), 0.0);
        assert_eq!(lamp.update(0.0, 0.0), 0.0);
    }
}

#[test]
fn incandescent_step_reaches_95_percent_at_15_ms_in_sim_ticks() {
    let v = step_response(LampKind::Incandescent, 1.0, TICK_S, 15);
    assert!(v >= 0.95, "incandescent at 15 ms (1 ms ticks) = {v}, want >= 0.95");
    assert!(v <= 1.0, "incandescent at 15 ms = {v} exceeds 1");
}

#[test]
fn incandescent_step_is_below_95_percent_at_10_ms_in_sim_ticks() {
    let v = step_response(LampKind::Incandescent, 1.0, TICK_S, 10);
    assert!(v < 0.95, "incandescent at 10 ms (1 ms ticks) = {v}, want < 0.95");
    assert!(v > 0.0, "incandescent has not started to warm at 10 ms");
}

#[test]
fn led_step_reaches_95_percent_at_1_ms() {
    let v = step_response(LampKind::Led, 1.0, TICK_S, 1);
    assert!(v >= 0.95, "LED at 1 ms (one tick) = {v}, want >= 0.95");
    assert!(v <= 1.0);
    let v = step_response(LampKind::Led, 1.0, 0.25e-3, 4);
    assert!(v >= 0.95, "LED at 1 ms (four 0.25 ms steps) = {v}, want >= 0.95");
    assert!(v <= 1.0);
}

/// The timings are stated in time, not in ticks: they hold however the 15 ms / 10 ms are
/// sliced into updates (a render thread evaluates the curve at frame intervals).
#[test]
fn incandescent_timings_hold_for_other_step_sizes() {
    // (dt, steps to 15 ms, steps to 10 ms)
    for (dt, n15, n10) in [
        (0.25e-3_f32, 60, 40),
        (0.5e-3, 30, 20),
        (2.5e-3, 6, 4),
        (5.0e-3, 3, 2),
    ] {
        let at15 = step_response(LampKind::Incandescent, 1.0, dt, n15);
        let at10 = step_response(LampKind::Incandescent, 1.0, dt, n10);
        assert!((0.95..=1.0).contains(&at15), "dt={dt}: at 15 ms = {at15}, want >= 0.95");
        assert!(at10 < 0.95, "dt={dt}: at 10 ms = {at10}, want < 0.95");
    }
}

/// Same, with the whole interval in one update.
#[test]
fn incandescent_timings_hold_for_a_single_update() {
    let at15 = step_response(LampKind::Incandescent, 1.0, 15.0e-3, 1);
    assert!((0.95..=1.0).contains(&at15), "one 15 ms update = {at15}, want >= 0.95");
    let at10 = step_response(LampKind::Incandescent, 1.0, 10.0e-3, 1);
    assert!(at10 < 0.95, "one 10 ms update = {at10}, want < 0.95");
}

/// First-order response: the curve does not depend (beyond rounding and a small
/// discretisation allowance) on how sim time is sliced.
#[test]
fn response_is_step_size_independent() {
    for kind in [LampKind::Incandescent, LampKind::Led] {
        for total_ms in [1u32, 2, 5, 10, 15, 30] {
            let fine = step_response(kind, 1.0, 0.125e-3, total_ms * 8);
            let tick = step_response(kind, 1.0, TICK_S, total_ms);
            let one = step_response(kind, 1.0, total_ms as f32 * 1.0e-3, 1);
            assert!(
                (fine - tick).abs() <= 0.02 && (fine - one).abs() <= 0.02,
                "{total_ms} ms: fine={fine} tick={tick} single={one}"
            );
        }
    }
}

#[test]
fn zero_dt_does_not_advance_the_lamp() {
    for kind in [LampKind::Incandescent, LampKind::Led] {
        let mut lamp = LampWarmup::new(kind);
        assert_eq!(lamp.update(1.0, 0.0), 0.0, "a zero-length step lit the lamp");
        let a = lamp.update(1.0, 0.5e-3);
        let b = lamp.update(1.0, 0.0);
        assert_eq!(a, b, "a zero-length step changed the output");
        let c = lamp.update(0.0, 0.0);
        assert_eq!(a, c, "a zero-length step changed the output");
    }
}

#[test]
fn led_is_faster_than_incandescent_at_every_time() {
    let mut led = LampWarmup::new(LampKind::Led);
    let mut inc = LampWarmup::new(LampKind::Incandescent);
    for i in 1..=200 {
        let l = led.update(1.0, 0.1e-3);
        let n = inc.update(1.0, 0.1e-3);
        assert!(l >= n, "step {i}: LED {l} < incandescent {n}");
        if i <= 50 {
            assert!(l > n, "step {i}: LED {l} not ahead of incandescent {n}");
        }
    }
}

#[test]
fn step_converges_to_the_commanded_level() {
    for kind in [LampKind::Incandescent, LampKind::Led] {
        for cmd in [0.0_f32, 0.1, 0.25, 0.5, 0.75, 1.0] {
            let v = step_response(kind, cmd, TICK_S, 1000);
            assert!((v - cmd).abs() <= 1.0e-3, "after 1 s at {cmd}: {v}");
        }
    }
}

/// First-order means one time constant: the fall mirrors the rise.
#[test]
fn turn_off_mirrors_turn_on() {
    let mut lamp = lit(LampKind::Incandescent);
    let mut v = 1.0;
    for _ in 0..10 {
        v = lamp.update(0.0, TICK_S);
    }
    assert!(v > 0.05, "incandescent 10 ms after turn-off = {v}, want > 0.05");
    for _ in 0..5 {
        v = lamp.update(0.0, TICK_S);
    }
    assert!((0.0..=0.05).contains(&v), "incandescent 15 ms after turn-off = {v}, want <= 0.05");

    let mut lamp = lit(LampKind::Led);
    let v = lamp.update(0.0, TICK_S);
    assert!((0.0..=0.05).contains(&v), "LED 1 ms after turn-off = {v}, want <= 0.05");
}

/// The step response rises strictly while far from the target, is concave (largest gain
/// first) and never overshoots.
#[test]
fn incandescent_step_is_monotone_concave_and_bounded() {
    let mut lamp = LampWarmup::new(LampKind::Incandescent);
    let mut prev = 0.0_f32;
    let mut prev_gain = f32::INFINITY;
    for i in 1..=15 {
        let v = lamp.update(1.0, TICK_S);
        let gain = v - prev;
        assert!(gain > 0.0, "tick {i}: output did not rise ({prev} -> {v})");
        assert!(gain <= prev_gain + 1.0e-6, "tick {i}: gain {gain} grew from {prev_gain}");
        assert!(v <= 1.0, "tick {i}: overshoot {v}");
        prev = v;
        prev_gain = gain;
    }
}

#[test]
fn identical_inputs_give_bit_identical_outputs() {
    for kind in [LampKind::Incandescent, LampKind::Led] {
        let run = || {
            let mut lamp = LampWarmup::new(kind);
            let mut bits = Vec::new();
            for i in 0..2000u32 {
                // A fixed pseudo-random command pattern (LCG), no sim RNG needed.
                let x = i.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let cmd = ((x >> 8) & 0xff) as f32 / 255.0;
                bits.push(lamp.update(cmd, TICK_S).to_bits());
            }
            bits
        };
        let first = run();
        for _ in 0..9 {
            assert_eq!(first, run(), "warm-up output is not deterministic");
        }
    }
}

fn kinds() -> impl Strategy<Value = LampKind> {
    prop_oneof![Just(LampKind::Incandescent), Just(LampKind::Led)]
}

proptest! {
    /// Emitted intensity stays in 0..=1 and finite for any command sequence, including
    /// out-of-range commands and long or tiny steps.
    #[test]
    fn output_is_always_in_unit_range(
        kind in kinds(),
        seq in proptest::collection::vec((-2.0f32..3.0, 0.0f32..0.1), 1..200),
    ) {
        let mut lamp = LampWarmup::new(kind);
        for (cmd, dt) in seq {
            let v = lamp.update(cmd, dt);
            prop_assert!(v.is_finite());
            prop_assert!((0.0..=1.0).contains(&v), "emitted {} for cmd {} dt {}", v, cmd, dt);
        }
    }

    /// Each update moves the output toward the commanded level and never past it.
    #[test]
    fn output_moves_toward_the_command_without_overshoot(
        kind in kinds(),
        seq in proptest::collection::vec((0.0f32..=1.0, 0.0f32..0.05), 1..200),
    ) {
        let mut lamp = LampWarmup::new(kind);
        let mut prev = 0.0f32;
        for (cmd, dt) in seq {
            let v = lamp.update(cmd, dt);
            let (lo, hi) = if cmd >= prev { (prev, cmd) } else { (cmd, prev) };
            prop_assert!(
                v >= lo - 1.0e-6 && v <= hi + 1.0e-6,
                "prev {} cmd {} dt {} -> {}", prev, cmd, dt, v
            );
            prev = v;
        }
    }

    /// Holding the command equal to the current output changes nothing.
    #[test]
    fn steady_state_is_a_fixed_point(kind in kinds(), level in 0.0f32..=1.0, dt in 0.0f32..0.05) {
        let mut lamp = LampWarmup::new(kind);
        let mut v = 0.0;
        for _ in 0..2000 { v = lamp.update(level, TICK_S); }
        let w = lamp.update(level, dt);
        prop_assert!((w - v).abs() <= 1.0e-4, "{} -> {}", v, w);
        prop_assert!((w - level).abs() <= 1.0e-3);
    }

    /// Linearity of a first-order response: the step response to level `a` is `a` times
    /// the unit step response.
    #[test]
    fn step_response_is_linear_in_the_command(
        kind in kinds(), a in 0.0f32..=1.0, steps in 1u32..40, dt in 1.0e-4f32..2.0e-3,
    ) {
        let unit = step_response(kind, 1.0, dt, steps);
        let scaled = step_response(kind, a, dt, steps);
        prop_assert!((scaled - a * unit).abs() <= 1.0e-4, "a={} unit={} scaled={}", a, unit, scaled);
    }

    /// A longer step never leaves the lamp further from its target.
    #[test]
    fn longer_steps_get_closer(kind in kinds(), dt in 0.0f32..0.03, extra in 0.0f32..0.03) {
        let short = step_response(kind, 1.0, dt, 1);
        let long = step_response(kind, 1.0, dt + extra, 1);
        prop_assert!(long >= short - 1.0e-6, "dt {} -> {}, dt {} -> {}", dt, short, dt + extra, long);
    }

    /// The timing bounds hold for any uniform slicing of the interval.
    #[test]
    fn incandescent_timing_bounds_hold_for_any_uniform_slicing(n in 1u32..=120) {
        let at15 = step_response(LampKind::Incandescent, 1.0, 15.0e-3 / n as f32, n);
        prop_assert!(at15 >= 0.95 - 1.0e-4 && at15 <= 1.0, "n={} at 15 ms = {}", n, at15);
        let at10 = step_response(LampKind::Incandescent, 1.0, 10.0e-3 / n as f32, n);
        prop_assert!(at10 < 0.95, "n={} at 10 ms = {}", n, at10);
    }
}
