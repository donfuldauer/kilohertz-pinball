//! Independent spec tests for Subphase 3.2 `pin_input::calibration` nudge
//! conditioning (verification criterion 4), written from the brief without
//! sight of the implementation.
//!
//! Every filter is primed with a few zero samples before a stimulus so the
//! tests do not depend on how the first sample initialises the filter state.

use pin_input::calibration::{DigitalNudge, NudgeConfig, NudgeFilter, thumbstick_nudge};
use pin_input::profiles::NudgeDirection;
use proptest::prelude::*;
use std::f32::consts::PI;

const DT: f32 = 0.001;

/// High-pass only: unity gain, linear curve, no deadzone.
fn linear(high_pass_hz: f32) -> NudgeConfig {
    NudgeConfig {
        high_pass_hz,
        gain: 1.0,
        curve_exponent: 1.0,
        deadzone: 0.0,
        max: 1.0,
        ..NudgeConfig::default()
    }
}

/// A cut-off so low (tau ~ 160 s) that one tick passes a step unchanged.
const NO_HP: f32 = 0.001;

fn primed(cfg: NudgeConfig, dt: f32) -> NudgeFilter {
    let mut f = NudgeFilter::new(cfg);
    for _ in 0..20 {
        let o = f.update([0.0, 0.0], dt);
        assert_eq!(o, [0.0, 0.0], "zero input gives zero output");
    }
    f
}

fn tau(hz: f32) -> f32 {
    1.0 / (2.0 * PI * hz)
}

fn dir(i: usize) -> NudgeDirection {
    match i % 3 {
        0 => NudgeDirection::Left,
        1 => NudgeDirection::Right,
        _ => NudgeDirection::Forward,
    }
}

fn close(a: [f32; 2], b: [f32; 2], tol: f32) -> bool {
    (a[0] - b[0]).abs() <= tol && (a[1] - b[1]).abs() <= tol
}

// ---------------------------------------------------------------------------
// NudgeFilter
// ---------------------------------------------------------------------------

#[test]
fn default_config_is_sane() {
    let c = NudgeConfig::default();
    assert_eq!(c.high_pass_hz, 2.0);
    assert_eq!(c.max, 1.0);
    assert!(c.gain.is_finite() && c.gain > 0.0);
    assert!(c.curve_exponent.is_finite() && c.curve_exponent > 0.0);
    assert!(c.deadzone.is_finite() && c.deadzone >= 0.0);
}

#[test]
fn step_response_decays_with_the_configured_cut_off() {
    let hz = 2.0;
    let t = tau(hz);
    let step = 0.5f32;
    let mut f = primed(linear(hz), DT);
    let n_tau = (t / DT).round() as usize;
    let n_5tau = (5.0 * t / DT).ceil() as usize + 1;
    let mut out = Vec::new();
    for _ in 0..(2 * n_5tau) {
        out.push(f.update([step, 0.0], DT));
    }
    // The step passes at first...
    assert!(
        (out[0][0] - step).abs() < 0.03 * step,
        "first sample {}",
        out[0][0]
    );
    // ...then decays monotonically, staying positive...
    for w in out.windows(2) {
        assert!(
            w[1][0] <= w[0][0] + 1e-7,
            "not decaying: {} then {}",
            w[0][0],
            w[1][0]
        );
        assert!(w[1][0] >= -1e-6);
    }
    // ...to 1/e after one time constant...
    let at_tau = out[n_tau][0] / step;
    assert!(
        (0.34..=0.40).contains(&at_tau),
        "at tau: {at_tau}, expected ~0.368"
    );
    let at_2tau = out[2 * n_tau][0] / step;
    assert!(
        (0.115..=0.155).contains(&at_2tau),
        "at 2 tau: {at_2tau}, expected ~0.135"
    );
    // ...and below 1 % of the initial step after five.
    for o in &out[n_5tau..] {
        assert!(o[0].abs() < 0.01 * step, "after 5 tau: {}", o[0]);
    }
    // But not absurdly faster than first order: still above 0.2 % at 5 tau.
    assert!(out[n_5tau][0] > 0.002 * step);
    // The other axis heard nothing.
    assert!(out.iter().all(|o| o[1] == 0.0));
}

#[test]
fn axes_are_filtered_independently() {
    let mut both = primed(linear(2.0), DT);
    let mut only_x = primed(linear(2.0), DT);
    let mut only_y = primed(linear(2.0), DT);
    for k in 0..500 {
        let x = 0.4 * (k as f32 * 0.05).sin();
        let y = -0.3 * (k as f32 * 0.021).cos();
        let b = both.update([x, y], DT);
        let ox = only_x.update([x, 0.0], DT);
        let oy = only_y.update([0.0, y], DT);
        assert_eq!(b[0], ox[0]);
        assert_eq!(b[1], oy[1]);
        assert_eq!(ox[1], 0.0);
        assert_eq!(oy[0], 0.0);
    }
}

#[test]
fn response_is_odd() {
    let cfg = NudgeConfig {
        high_pass_hz: 2.0,
        gain: 1.7,
        curve_exponent: 1.6,
        deadzone: 0.03,
        max: 1.0,
        ..NudgeConfig::default()
    };
    let mut a = primed(cfg, DT);
    let cfg = NudgeConfig {
        high_pass_hz: 2.0,
        gain: 1.7,
        curve_exponent: 1.6,
        deadzone: 0.03,
        max: 1.0,
        ..NudgeConfig::default()
    };
    let mut b = primed(cfg, DT);
    for k in 0..400 {
        let x = 0.8 * (k as f32 * 0.09).sin();
        let p = a.update([x, -x], DT);
        let n = b.update([-x, x], DT);
        assert!((p[0] + n[0]).abs() < 1e-6 && (p[1] + n[1]).abs() < 1e-6);
        assert!((p[0] + p[1]).abs() < 1e-6, "x and y use the same curve");
    }
}

/// Steady-state amplitude gain of the high-pass at `freq`, measured on x.
fn sine_gain(hz: f32, freq: f32, amp: f32) -> f32 {
    let mut f = primed(linear(hz), DT);
    let total = (4.0 / hz.min(freq) / DT) as usize + 2000;
    let mut peak = 0.0f32;
    for k in 0..total {
        let x = amp * (2.0 * PI * freq * k as f32 * DT).sin();
        let o = f.update([x, 0.0], DT);
        if k >= total / 2 {
            peak = peak.max(o[0].abs());
        }
    }
    peak / amp
}

#[test]
fn sinusoid_at_ten_times_the_cut_off_passes() {
    let g = sine_gain(2.0, 20.0, 0.5);
    assert!(g >= 0.95, "gain at 10x cut-off is {g}, expected >= 0.95");
    assert!(g <= 1.02, "a high-pass does not amplify: {g}");
}

#[test]
fn sinusoid_far_below_the_cut_off_is_rejected() {
    let g = sine_gain(2.0, 0.2, 0.5);
    assert!(g < 0.15, "gain at 0.1x cut-off is {g}, expected ~0.1");
    let g = sine_gain(2.0, 2.0, 0.5);
    assert!(
        (0.62..=0.78).contains(&g),
        "gain at the cut-off is {g}, expected ~0.707"
    );
}

#[test]
fn constant_tilt_is_removed() {
    // A cabinet that is not level: constant 0.3 g offset on both axes.
    let mut f = primed(linear(2.0), DT);
    let mut last = [1.0, 1.0];
    for _ in 0..3000 {
        last = f.update([0.3, -0.3], DT);
    }
    assert!(last[0].abs() < 1e-4 && last[1].abs() < 1e-4, "{last:?}");
    // A bump on top of the offset comes through at full size.
    let o = f.update([0.3 + 0.4, -0.3], DT);
    assert!((o[0] - 0.4).abs() < 0.02, "{o:?}");
    assert!(o[1].abs() < 1e-4);
}

#[test]
fn deadzone_follows_the_high_pass() {
    let cfg = || NudgeConfig {
        deadzone: 0.1,
        ..linear(NO_HP)
    };
    // A step below the deadzone is silence.
    for step in [0.02f32, 0.09, -0.09] {
        let mut f = primed(cfg(), DT);
        assert_eq!(f.update([step, step], DT), [0.0, 0.0], "step {step}");
    }
    // A step above it passes, no larger than the input.
    for step in [0.5f32, -0.5] {
        let mut f = primed(cfg(), DT);
        let o = f.update([step, 0.0], DT);
        assert_eq!(o[0].signum(), step.signum());
        assert!(
            (0.39..=0.5001).contains(&o[0].abs()),
            "step {step} -> {}",
            o[0]
        );
        assert_eq!(o[1], 0.0);
    }
    // The deadzone sees the high-passed signal: a large constant offset that
    // has decayed away is inside it, even though the raw input is far outside.
    let mut f = primed(
        NudgeConfig {
            deadzone: 0.1,
            ..linear(2.0)
        },
        DT,
    );
    let mut o = [1.0, 1.0];
    for _ in 0..1000 {
        o = f.update([0.9, 0.9], DT);
    }
    assert_eq!(o, [0.0, 0.0]);
}

#[test]
fn gain_curve_matches_the_formula_at_landmarks() {
    let run = |gain: f32, exp: f32, max: f32, x: f32| {
        let mut f = primed(
            NudgeConfig {
                high_pass_hz: NO_HP,
                gain,
                curve_exponent: exp,
                deadzone: 0.0,
                max,
                ..NudgeConfig::default()
            },
            DT,
        );
        f.update([x, -x], DT)
    };
    let check = |gain: f32, exp: f32, max: f32, x: f32| {
        let want = x.signum() * (gain * x.abs().powf(exp)).min(max);
        let o = run(gain, exp, max, x);
        assert!(
            (o[0] - want).abs() < 2e-3 && (o[1] + want).abs() < 2e-3,
            "gain {gain} exp {exp} max {max} x {x}: {o:?}, expected {want}"
        );
    };
    check(1.0, 1.0, 1.0, 0.5);
    check(2.0, 1.0, 1.0, 0.25);
    check(2.0, 1.0, 1.0, 0.75); // clamps to max
    check(1.0, 2.0, 1.0, 0.5); // 0.25
    check(1.0, 2.0, 1.0, -0.5); // sign kept with an even exponent
    check(3.0, 0.5, 1.0, 0.04); // 0.6
    check(1.0, 1.0, 0.3, 0.9); // max below 1
    check(4.0, 3.0, 0.6, -0.5); // -0.5
    check(1.5, 1.5, 1.0, 0.0);
    assert_eq!(run(2.0, 1.0, 1.0, 0.75)[0], 1.0, "clamped exactly to max");
    assert_eq!(run(2.0, 1.0, 0.25, -0.75)[0], -0.25);
}

#[test]
fn reset_restores_the_initial_state() {
    let cfg = || NudgeConfig {
        gain: 1.3,
        curve_exponent: 1.2,
        deadzone: 0.01,
        ..linear(2.0)
    };
    let stimulus: Vec<[f32; 2]> = (0..300)
        .map(|k| {
            [
                0.6 * (k as f32 * 0.11).sin() + 0.2,
                0.5 * (k as f32 * 0.07).cos(),
            ]
        })
        .collect();
    let mut fresh = NudgeFilter::new(cfg());
    let want: Vec<[f32; 2]> = stimulus.iter().map(|s| fresh.update(*s, DT)).collect();

    let mut used = NudgeFilter::new(cfg());
    for k in 0..777 {
        used.update([0.9 * (k as f32 * 0.3).sin() - 0.4, 0.7], DT);
    }
    used.update([f32::NAN, f32::NAN], DT);
    used.reset();
    let got: Vec<[f32; 2]> = stimulus.iter().map(|s| used.update(*s, DT)).collect();
    assert_eq!(got, want);
}

#[test]
fn nan_input_never_propagates() {
    let mut f = primed(linear(2.0), DT);
    for k in 0..2000 {
        let x = 0.5 * (k as f32 * 0.13).sin();
        let input = match k % 53 {
            7 => [f32::NAN, x],
            19 => [x, f32::NAN],
            31 => [f32::NAN, f32::NAN],
            _ => [x, -x],
        };
        let o = f.update(input, DT);
        assert!(
            o[0].is_finite() && o[1].is_finite() && o[0].abs() <= 1.0 && o[1].abs() <= 1.0,
            "tick {k}: {o:?}"
        );
    }
    // A long run of NaN, then silence: the output returns to zero.
    for _ in 0..100 {
        let o = f.update([f32::NAN, f32::NAN], DT);
        assert!(o[0].is_finite() && o[1].is_finite());
    }
    let mut o = [1.0, 1.0];
    for _ in 0..3000 {
        o = f.update([0.0, 0.0], DT);
        assert!(o[0].is_finite() && o[1].is_finite());
    }
    assert!(o[0].abs() < 1e-3 && o[1].abs() < 1e-3, "{o:?}");
    // NaN as the very first sample of a fresh filter.
    let mut f = NudgeFilter::new(linear(2.0));
    let o = f.update([f32::NAN, f32::NAN], DT);
    assert!(o[0].is_finite() && o[1].is_finite());
    let o = f.update([0.1, 0.1], DT);
    assert!(o[0].is_finite() && o[1].is_finite());
}

/// Stricter than the letter of the brief (which names NaN only): infinite
/// samples and degenerate time steps must not manufacture a NaN either.
#[test]
fn infinite_input_and_degenerate_dt_never_yield_nan() {
    let mut f = primed(linear(2.0), DT);
    let inputs = [
        ([f32::INFINITY, 0.1], DT),
        ([f32::INFINITY, 0.1], DT),
        ([f32::NEG_INFINITY, f32::INFINITY], DT),
        ([0.2, 0.2], DT),
        ([0.2, 0.2], 0.0),
        ([0.3, 0.1], f32::NAN),
        ([0.3, 0.1], -DT),
        ([0.3, 0.1], f32::INFINITY),
        ([1e30, -1e30], DT),
        ([0.0, 0.0], DT),
    ];
    for (i, (x, dt)) in inputs.into_iter().enumerate() {
        let o = f.update(x, dt);
        assert!(
            o[0].is_finite() && o[1].is_finite() && o[0].abs() <= 1.0 && o[1].abs() <= 1.0,
            "input {i} ({x:?}, dt {dt}): {o:?}"
        );
    }
    for _ in 0..100 {
        let o = f.update([0.0, 0.0], DT);
        assert!(o[0].is_finite() && o[1].is_finite());
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// The decay follows whatever cut-off and time step are configured.
    #[test]
    fn step_response_tracks_cut_off_and_dt(
        hz in 0.5f32..8.0,
        dt in prop_oneof![Just(0.0005f32), Just(0.001f32), Just(0.002f32)],
        step in prop_oneof![0.1f32..1.0, -1.0f32..-0.1],
    ) {
        let t = tau(hz);
        prop_assume!(dt / t <= 0.07);
        let mut f = primed(linear(hz), dt);
        let n_tau = (t / dt).round() as usize;
        let n_5tau = (5.0 * t / dt).ceil() as usize + 1;
        let mut at_tau = 0.0;
        let mut at_5tau = 0.0;
        for k in 0..=n_5tau {
            let o = f.update([0.0, step], dt);
            prop_assert_eq!(o[0], 0.0);
            if k == n_tau { at_tau = o[1] / step; }
            if k == n_5tau { at_5tau = o[1] / step; }
        }
        prop_assert!((0.31..=0.43).contains(&at_tau), "at tau: {}", at_tau);
        prop_assert!((0.001..0.01).contains(&at_5tau), "at 5 tau: {}", at_5tau);
    }

    /// 10x the cut-off passes with gain >= 0.95, for any cut-off.
    #[test]
    fn ten_times_cut_off_passes(hz in 0.5f32..3.0, amp in 0.1f32..0.9) {
        let g = sine_gain(hz, 10.0 * hz, amp);
        prop_assert!((0.95..=1.02).contains(&g), "gain {} at 10x {} Hz", g, hz);
    }

    /// out = sign(x) * min(max, gain * |x|^exponent) on the high-passed signal.
    #[test]
    fn gain_curve_matches_the_formula(
        x in -2.0f32..2.0,
        y in -2.0f32..2.0,
        gain in 0.1f32..4.0,
        exp in 0.5f32..3.0,
        max in 0.2f32..1.0,
    ) {
        let mut f = primed(
            NudgeConfig {
                high_pass_hz: NO_HP,
                gain,
                curve_exponent: exp,
                deadzone: 0.0,
                max,
                ..NudgeConfig::default()
            },
            DT,
        );
        let o = f.update([x, y], DT);
        for (got, input) in [(o[0], x), (o[1], y)] {
            let want = input.signum() * (gain * input.abs().powf(exp)).min(max);
            prop_assert!(
                (got - want).abs() <= 2e-3 + 2e-3 * want.abs(),
                "input {} -> {}, expected {}", input, got, want
            );
            prop_assert!(got.abs() <= max);
        }
    }

    /// Whatever arrives, the output is finite and within +/-max.
    #[test]
    fn output_is_always_within_max(
        samples in prop::collection::vec(
            (
                prop_oneof![8 => -4.0f32..4.0, 1 => any::<f32>(), 1 => Just(f32::NAN)],
                prop_oneof![8 => -4.0f32..4.0, 1 => any::<f32>(), 1 => Just(f32::NAN)],
            ),
            1..300,
        ),
        hz in 0.1f32..20.0,
        gain in 0.0f32..10.0,
        exp in 0.25f32..4.0,
        deadzone in 0.0f32..0.5,
        max in 0.05f32..1.0,
    ) {
        let mut f = NudgeFilter::new(NudgeConfig {
            high_pass_hz: hz,
            gain,
            curve_exponent: exp,
            deadzone,
            max,
            ..NudgeConfig::default()
        });
        for (i, (x, y)) in samples.iter().enumerate() {
            // Finite or NaN input only: the brief's guarantee.
            let x = if x.is_infinite() { 0.0 } else { *x };
            let y = if y.is_infinite() { 0.0 } else { *y };
            let o = f.update([x, y], DT);
            prop_assert!(
                o[0].is_finite() && o[1].is_finite() && o[0].abs() <= max && o[1].abs() <= max,
                "tick {} input ({}, {}) -> {:?} (max {})", i, x, y, o, max
            );
        }
    }
}

// ---------------------------------------------------------------------------
// DigitalNudge
// ---------------------------------------------------------------------------

#[test]
fn digital_press_is_a_constant_vector_for_duration_ticks() {
    let s = 0.6;
    for (d, want) in [
        (NudgeDirection::Left, [-s, 0.0]),
        (NudgeDirection::Right, [s, 0.0]),
        (NudgeDirection::Forward, [0.0, s]),
    ] {
        let mut n = DigitalNudge::new(s, 40);
        assert_eq!(n.sample(0), [0.0, 0.0], "nothing before any press");
        assert_eq!(n.sample(1000), [0.0, 0.0]);
        n.press(d, 1000);
        for t in 1000..1040 {
            assert!(
                close(n.sample(t), want, 1e-6),
                "tick {t}: {:?}",
                n.sample(t)
            );
        }
        for t in [1040u64, 1041, 1100, 5000, u64::MAX] {
            assert_eq!(n.sample(t), [0.0, 0.0], "tick {t} is past the 40 ms pulse");
        }
        // Sampling is a pure read.
        assert!(close(n.sample(1039), want, 1e-6));
    }
}

#[test]
fn digital_overlapping_presses_add_and_clamp() {
    let mut n = DigitalNudge::new(0.7, 20);
    n.press(NudgeDirection::Right, 0);
    n.press(NudgeDirection::Right, 5);
    assert!(close(n.sample(4), [0.7, 0.0], 1e-6));
    assert_eq!(n.sample(5), [1.0, 0.0], "0.7 + 0.7 clamps to 1");
    assert_eq!(n.sample(19), [1.0, 0.0]);
    assert!(
        close(n.sample(20), [0.7, 0.0], 1e-6),
        "first pulse over, second still on"
    );
    assert!(close(n.sample(24), [0.7, 0.0], 1e-6));
    assert_eq!(n.sample(25), [0.0, 0.0]);

    let mut n = DigitalNudge::new(0.7, 20);
    n.press(NudgeDirection::Left, 100);
    n.press(NudgeDirection::Left, 101);
    assert_eq!(n.sample(110), [-1.0, 0.0], "clamps at -1 too");

    let mut n = DigitalNudge::new(0.4, 20);
    n.press(NudgeDirection::Right, 0);
    n.press(NudgeDirection::Right, 3);
    assert!(
        close(n.sample(10), [0.8, 0.0], 1e-6),
        "0.4 + 0.4 adds without clamping"
    );
}

#[test]
fn digital_presses_in_different_directions_combine() {
    let s = 0.5;
    let mut n = DigitalNudge::new(s, 30);
    n.press(NudgeDirection::Right, 0);
    n.press(NudgeDirection::Forward, 10);
    assert!(close(n.sample(5), [s, 0.0], 1e-6));
    assert!(close(n.sample(15), [s, s], 1e-6));
    assert!(close(n.sample(35), [0.0, s], 1e-6));
    assert_eq!(n.sample(40), [0.0, 0.0]);

    let mut n = DigitalNudge::new(s, 30);
    n.press(NudgeDirection::Left, 0);
    n.press(NudgeDirection::Right, 0);
    assert!(
        close(n.sample(10), [0.0, 0.0], 1e-6),
        "left and right cancel"
    );
    n.press(NudgeDirection::Forward, 12);
    assert!(close(n.sample(20), [0.0, s], 1e-6));
}

#[test]
fn digital_nudge_can_be_pressed_again_after_it_expires() {
    let mut n = DigitalNudge::new(0.9, 10);
    for round in 0..200u64 {
        let t = round * 25;
        let d = dir(round as usize);
        n.press(d, t);
        let want = match round % 3 {
            0 => [-0.9, 0.0],
            1 => [0.9, 0.0],
            _ => [0.0, 0.9],
        };
        assert!(close(n.sample(t), want, 1e-6), "round {round}");
        assert!(close(n.sample(t + 9), want, 1e-6), "round {round}");
        assert_eq!(n.sample(t + 10), [0.0, 0.0], "round {round}");
    }
}

#[test]
fn digital_zero_duration_or_strength_is_silent() {
    let mut n = DigitalNudge::new(0.8, 0);
    n.press(NudgeDirection::Right, 5);
    assert_eq!(n.sample(5), [0.0, 0.0]);
    assert_eq!(n.sample(6), [0.0, 0.0]);
    let mut n = DigitalNudge::new(0.0, 50);
    n.press(NudgeDirection::Left, 5);
    assert!(close(n.sample(6), [0.0, 0.0], 0.0));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Against the formula, with at most three pulses alive at once.
    #[test]
    fn digital_nudge_matches_the_model(
        strength in 0.05f32..1.0,
        duration in 1u32..80,
        presses in prop::collection::vec((0u64..40, 0usize..3), 1..40),
    ) {
        let mut n = DigitalNudge::new(strength, duration);
        let mut t = 0u64;
        let mut model: Vec<(u64, usize)> = Vec::new();
        let expect = |model: &[(u64, usize)], at: u64| -> [f32; 2] {
            let mut v = [0.0f32; 2];
            for (start, d) in model {
                if at >= *start && at < *start + u64::from(duration) {
                    match d {
                        0 => v[0] -= strength,
                        1 => v[0] += strength,
                        _ => v[1] += strength,
                    }
                }
            }
            [v[0].clamp(-1.0, 1.0), v[1].clamp(-1.0, 1.0)]
        };
        for (gap, d) in presses {
            // Sample every tick up to the next press.
            for at in t..t + gap {
                let got = n.sample(at);
                prop_assert!(close(got, expect(&model, at), 1e-5), "tick {}: {:?}", at, got);
            }
            t += gap;
            model.retain(|(start, _)| t < *start + u64::from(duration));
            if model.len() < 3 {
                n.press(dir(d), t);
                model.push((t, d));
            }
            let got = n.sample(t);
            prop_assert!(close(got, expect(&model, t), 1e-5), "tick {}: {:?}", t, got);
        }
        for at in t..t + u64::from(duration) + 2 {
            let got = n.sample(at);
            prop_assert!(close(got, expect(&model, at), 1e-5), "tick {}: {:?}", at, got);
        }
        prop_assert_eq!(n.sample(t + u64::from(duration)), [0.0, 0.0]);
    }
}

// ---------------------------------------------------------------------------
// thumbstick_nudge
// ---------------------------------------------------------------------------

#[test]
fn thumbstick_inside_the_radial_deadzone_is_zero() {
    for (x, y) in [
        (0.0, 0.0),
        (0.1, 0.0),
        (0.0, -0.19),
        (0.14, 0.14),
        (-0.1, 0.17),
    ] {
        assert_eq!(thumbstick_nudge(x, y, 0.2), [0.0, 0.0], "({x}, {y})");
    }
    // Radial, not per axis: (0.15, 0.15) has radius 0.212 > 0.2.
    let o = thumbstick_nudge(0.15, 0.15, 0.2);
    assert!(o[0] > 0.0 && o[1] > 0.0, "{o:?}");
    // Not a square deadzone the other way either: x alone beyond it still counts.
    let o = thumbstick_nudge(0.5, 0.01, 0.2);
    assert!(
        o[0] > 0.0 && o[1] > 0.0,
        "y keeps its share of the vector: {o:?}"
    );
}

#[test]
fn thumbstick_is_rescaled_to_full_range() {
    for dz in [0.0f32, 0.1, 0.25, 0.5] {
        assert!(close(thumbstick_nudge(1.0, 0.0, dz), [1.0, 0.0], 1e-5));
        assert!(close(thumbstick_nudge(-1.0, 0.0, dz), [-1.0, 0.0], 1e-5));
        assert!(close(thumbstick_nudge(0.0, 1.0, dz), [0.0, 1.0], 1e-5));
        assert!(close(thumbstick_nudge(0.0, -1.0, dz), [0.0, -1.0], 1e-5));
        // Half-way between the deadzone edge and full deflection reads 0.5.
        let mid = dz + (1.0 - dz) / 2.0;
        assert!(
            close(thumbstick_nudge(mid, 0.0, dz), [0.5, 0.0], 1e-5),
            "dz {dz}"
        );
        // Just outside the edge starts from ~0: no jump.
        let o = thumbstick_nudge(0.0, dz + 0.001, dz);
        assert!(o[1] > 0.0 && o[1] < 0.01, "dz {dz}: {o:?}");
    }
    // Diagonal: magnitude rescaled, direction kept.
    let o = thumbstick_nudge(0.36, 0.48, 0.2); // radius 0.6 -> 0.5
    assert!(close(o, [0.3, 0.4], 1e-5), "{o:?}");
    let o = thumbstick_nudge(-0.48, 0.36, 0.2);
    assert!(close(o, [-0.4, 0.3], 1e-5), "{o:?}");
    assert!(close(thumbstick_nudge(0.3, -0.4, 0.0), [0.3, -0.4], 1e-6));
}

#[test]
fn thumbstick_handles_non_finite_input() {
    for (x, y) in [
        (f32::NAN, 0.0),
        (0.0, f32::NAN),
        (f32::NAN, f32::NAN),
        (f32::NAN, 0.9),
        (0.9, f32::NAN),
    ] {
        let o = thumbstick_nudge(x, y, 0.2);
        assert!(
            o[0].is_finite() && o[1].is_finite() && o[0].abs() <= 1.0 && o[1].abs() <= 1.0,
            "({x}, {y}) -> {o:?}"
        );
    }
    let o = thumbstick_nudge(f32::NAN, f32::NAN, 0.2);
    assert_eq!(o, [0.0, 0.0]);
    let o = thumbstick_nudge(0.5, 0.5, f32::NAN);
    assert!(o[0].is_finite() && o[1].is_finite());
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn thumbstick_matches_the_radial_formula(
        x in -1.0f32..=1.0,
        y in -1.0f32..=1.0,
        dz in 0.0f32..0.6,
    ) {
        let r = (x * x + y * y).sqrt();
        let o = thumbstick_nudge(x, y, dz);
        prop_assert!(o[0].is_finite() && o[1].is_finite());
        prop_assert!(o[0].abs() <= 1.0 + 1e-6 && o[1].abs() <= 1.0 + 1e-6);
        if r < dz - 1e-5 {
            prop_assert_eq!(o, [0.0, 0.0]);
        } else if r > dz + 1e-5 {
            // Direction is preserved.
            prop_assert!((o[0] * y - o[1] * x).abs() < 1e-4, "direction changed: {:?}", o);
            prop_assert!(o[0] * x >= 0.0 && o[1] * y >= 0.0);
            if r <= 1.0 {
                let want = (r - dz) / (1.0 - dz);
                let got = (o[0] * o[0] + o[1] * o[1]).sqrt();
                prop_assert!((got - want).abs() < 1e-4, "magnitude {} vs {}", got, want);
            }
        }
    }

    #[test]
    fn thumbstick_never_panics_or_leaks_non_finite(x in any::<f32>(), y in any::<f32>(), dz in any::<f32>()) {
        let o = thumbstick_nudge(x, y, dz);
        prop_assert!(!o[0].is_nan() && !o[1].is_nan(), "({}, {}, {}) -> {:?}", x, y, dz, o);
    }
}
