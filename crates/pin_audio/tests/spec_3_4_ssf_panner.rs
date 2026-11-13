//! Independent Tester spec tests, Subphase 3.4, verification criterion 1:
//! the four-corner equal-power SSF panner over the 7.1 exciter channels.
//! Written from the brief alone (black box).

use pin_audio::ssf::{CH_BL, CH_BR, CH_FC, CH_FL, CH_FR, CH_LFE, CH_SL, CH_SR, SsfPanner};
use proptest::prelude::*;

const X_MIN: f32 = -257.0;
const X_MAX: f32 = 257.0;
const Y_MIN: f32 = 0.0;
const Y_MAX: f32 = 1070.0;

fn playfield() -> SsfPanner {
    SsfPanner::new(X_MIN, X_MAX, Y_MIN, Y_MAX)
}

/// The brief's law, evaluated in f64: returns (SL, SR, BL, BR).
fn law(x: f32, y: f32, x_min: f32, x_max: f32, y_min: f32, y_max: f32) -> [f64; 4] {
    let u = ((f64::from(x) - f64::from(x_min)) / (f64::from(x_max) - f64::from(x_min)))
        .clamp(0.0, 1.0);
    let w = ((f64::from(y) - f64::from(y_min)) / (f64::from(y_max) - f64::from(y_min)))
        .clamp(0.0, 1.0);
    let h = std::f64::consts::FRAC_PI_2;
    [
        (u * h).cos() * (w * h).cos(),
        (u * h).sin() * (w * h).cos(),
        (u * h).cos() * (w * h).sin(),
        (u * h).sin() * (w * h).sin(),
    ]
}

fn path_point(i: usize, n: usize) -> (f32, f32) {
    let t = i as f32 / (n - 1) as f32;
    (-150.0 + 300.0 * t, 100.0 + 700.0 * t)
}

fn sum_sq(g: &[f32; 8]) -> f64 {
    g.iter().map(|&v| f64::from(v) * f64::from(v)).sum()
}

#[test]
fn channel_constants_are_smpte_7_1_order() {
    assert_eq!(
        [CH_FL, CH_FR, CH_FC, CH_LFE, CH_BL, CH_BR, CH_SL, CH_SR],
        [0, 1, 2, 3, 4, 5, 6, 7]
    );
}

#[test]
fn path_matches_law_within_1e_6() {
    let p = playfield();
    let n = 100;
    for i in 0..n {
        let (x, y) = path_point(i, n);
        let g = p.gains(x, y);
        let [sl, sr, bl, br] = law(x, y, X_MIN, X_MAX, Y_MIN, Y_MAX);
        for (name, got, want) in [
            ("SL", g[CH_SL], sl),
            ("SR", g[CH_SR], sr),
            ("BL", g[CH_BL], bl),
            ("BR", g[CH_BR], br),
        ] {
            assert!(
                (f64::from(got) - want).abs() <= 1e-6,
                "point {i} ({x}, {y}) {name}: got {got}, law {want}"
            );
        }
        for ch in [CH_FL, CH_FR, CH_FC, CH_LFE] {
            assert_eq!(g[ch], 0.0, "point {i}: channel {ch} must be silent");
        }
    }
}

#[test]
fn path_sum_of_squares_is_one() {
    let p = playfield();
    let n = 100;
    for i in 0..n {
        let (x, y) = path_point(i, n);
        let g = p.gains(x, y);
        assert!(
            (sum_sq(&g) - 1.0).abs() <= 1e-5,
            "point {i}: sum of squares {}",
            sum_sq(&g)
        );
    }
}

#[test]
fn sl_dominates_at_start_and_br_at_end() {
    let p = playfield();
    let start = p.gains(-150.0, 100.0);
    let end = p.gains(150.0, 800.0);
    for ch in 0..8 {
        if ch != CH_SL {
            assert!(
                start[CH_SL] > start[ch],
                "start: SL {} must exceed channel {ch} = {}",
                start[CH_SL],
                start[ch]
            );
        }
        if ch != CH_BR {
            assert!(
                end[CH_BR] > end[ch],
                "end: BR {} must exceed channel {ch} = {}",
                end[CH_BR],
                end[ch]
            );
        }
    }
}

#[test]
fn path_is_continuous_no_step_over_0_05() {
    let p = playfield();
    let n = 100;
    let mut prev = p.gains(-150.0, 100.0);
    for i in 1..n {
        let (x, y) = path_point(i, n);
        let g = p.gains(x, y);
        for ch in 0..8 {
            let step = (g[ch] - prev[ch]).abs();
            assert!(step <= 0.05, "step {step} on channel {ch} at point {i}");
        }
        prev = g;
    }
}

#[test]
fn corners_map_to_their_exciters() {
    let p = playfield();
    // Front (low y, near the flippers) -> side channels; back (high y) -> back channels.
    for (x, y, ch) in [
        (X_MIN, Y_MIN, CH_SL),
        (X_MAX, Y_MIN, CH_SR),
        (X_MIN, Y_MAX, CH_BL),
        (X_MAX, Y_MAX, CH_BR),
    ] {
        let g = p.gains(x, y);
        assert!((g[ch] - 1.0).abs() <= 1e-6, "corner ({x}, {y}): {g:?}");
        for other in 0..8 {
            if other != ch {
                assert!(g[other].abs() <= 1e-6, "corner ({x}, {y}): {g:?}");
            }
        }
    }
}

#[test]
fn centre_is_equal_on_all_four_exciters() {
    let g = playfield().gains(0.0, 535.0);
    for ch in [CH_SL, CH_SR, CH_BL, CH_BR] {
        assert!((g[ch] - 0.5).abs() <= 1e-6, "{g:?}");
    }
}

#[test]
fn positions_outside_the_rectangle_clamp_to_the_edge() {
    let p = playfield();
    for (outside, edge) in [
        ((-1000.0, 300.0), (X_MIN, 300.0)),
        ((1000.0, 300.0), (X_MAX, 300.0)),
        ((10.0, -500.0), (10.0, Y_MIN)),
        ((10.0, 5000.0), (10.0, Y_MAX)),
        ((-9e9, 9e9), (X_MIN, Y_MAX)),
        ((f32::INFINITY, f32::NEG_INFINITY), (X_MAX, Y_MIN)),
    ] {
        let a = p.gains(outside.0, outside.1);
        let b = p.gains(edge.0, edge.1);
        for ch in 0..8 {
            assert!(
                (a[ch] - b[ch]).abs() <= 1e-6,
                "{outside:?} vs {edge:?}: {a:?} / {b:?}"
            );
        }
    }
}

#[test]
fn gains_are_a_pure_function() {
    let p = playfield();
    let q = playfield();
    for i in 0..100 {
        let (x, y) = path_point(i, 100);
        let a = p.gains(x, y);
        assert_eq!(a.map(f32::to_bits), p.gains(x, y).map(f32::to_bits));
        assert_eq!(a.map(f32::to_bits), q.gains(x, y).map(f32::to_bits));
    }
}

#[test]
fn left_right_and_front_back_monotonic() {
    let p = playfield();
    let mut prev = p.gains(X_MIN, 300.0);
    for i in 1..=64 {
        let x = X_MIN + (X_MAX - X_MIN) * i as f32 / 64.0;
        let g = p.gains(x, 300.0);
        assert!(g[CH_SR] >= prev[CH_SR] - 1e-6 && g[CH_BR] >= prev[CH_BR] - 1e-6);
        assert!(g[CH_SL] <= prev[CH_SL] + 1e-6 && g[CH_BL] <= prev[CH_BL] + 1e-6);
        prev = g;
    }
    let mut prev = p.gains(40.0, Y_MIN);
    for i in 1..=64 {
        let y = Y_MIN + (Y_MAX - Y_MIN) * i as f32 / 64.0;
        let g = p.gains(40.0, y);
        assert!(g[CH_BL] >= prev[CH_BL] - 1e-6 && g[CH_BR] >= prev[CH_BR] - 1e-6);
        assert!(g[CH_SL] <= prev[CH_SL] + 1e-6 && g[CH_SR] <= prev[CH_SR] + 1e-6);
        prev = g;
    }
}

proptest! {
    /// Any finite position on any sane rectangle: law holds, unit power,
    /// gains in 0..=1, front/centre/LFE channels silent.
    #[test]
    fn law_holds_everywhere(
        x_min in -2000.0f32..2000.0, width in 1.0f32..4000.0,
        y_min in -2000.0f32..2000.0, height in 1.0f32..4000.0,
        fx in -0.5f32..1.5, fy in -0.5f32..1.5,
    ) {
        let (x_max, y_max) = (x_min + width, y_min + height);
        let p = SsfPanner::new(x_min, x_max, y_min, y_max);
        let (x, y) = (x_min + fx * width, y_min + fy * height);
        let g = p.gains(x, y);
        let want = law(x, y, x_min, x_max, y_min, y_max);
        // f32 evaluation of u and w on wide rectangles costs a few ulps more
        // than the playfield case in criterion 1, hence the looser bound here.
        for (got, want) in [g[CH_SL], g[CH_SR], g[CH_BL], g[CH_BR]].into_iter().zip(want) {
            prop_assert!((f64::from(got) - want).abs() <= 1e-5, "got {} want {}", got, want);
        }
        prop_assert!((sum_sq(&g) - 1.0).abs() <= 1e-5);
        for v in g {
            prop_assert!((0.0..=1.0 + 1e-6).contains(&v));
        }
        for ch in [CH_FL, CH_FR, CH_FC, CH_LFE] {
            prop_assert_eq!(g[ch], 0.0);
        }
    }

    /// Any f32 at all (NaN, infinities, subnormals) must not panic and must
    /// not produce non-finite gains on the standard playfield.
    #[test]
    fn arbitrary_floats_never_panic(x in any::<f32>(), y in any::<f32>()) {
        let g = playfield().gains(x, y);
        if x.is_finite() && y.is_finite() {
            prop_assert!((sum_sq(&g) - 1.0).abs() <= 1e-5);
        }
        for v in g {
            prop_assert!(v.is_finite(), "non-finite gain for ({}, {}): {:?}", x, y, g);
        }
    }
}
