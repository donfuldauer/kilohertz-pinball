//! Subphase 3.2 round-5 tests: release-speed accuracy on curved return
//! strokes (review B4), quantised readings, pull-tick counting (N-k) and the
//! never-stuck invariant on offset devices (N-l). Deterministic.

use pin_input::calibration::{PlungerConfig, PlungerFilter};
use proptest::prelude::*;
use std::f32::consts::PI;

/// Return-stroke shapes: the fraction of the depth left at `u` = 0..1.
#[derive(Copy, Clone, Debug)]
enum Shape {
    Linear,
    /// Spring from rest: slow start, peak speed at the end.
    QuarterSine,
    /// Spring with a preload (in depths): `(1+p)·cos(ωt) − p`.
    Preloaded(f32),
    ConstAccel,
    /// Smooth start and end, peak in the middle.
    HalfCosine,
    /// Peak at the start.
    Decelerating,
}

impl Shape {
    fn at(self, u: f32, depth: f32) -> f32 {
        match self {
            Shape::Linear => 1.0 - u,
            Shape::QuarterSine => (PI / 2.0 * u).cos(),
            Shape::Preloaded(p) => {
                let p = p / depth;
                let w = (p / (1.0 + p)).acos();
                (1.0 + p) * (w * u).cos() - p
            }
            Shape::ConstAccel => 1.0 - u * u,
            Shape::HalfCosine => 0.5 * (1.0 + (PI * u).cos()),
            Shape::Decelerating => (1.0 - u) * (1.0 - u),
        }
    }

    /// The stroke samples after the hold, ending at exactly 0.
    fn stroke(self, depth: f32, ms: u32) -> Vec<f32> {
        (1..=ms)
            .map(|k| {
                if k == ms {
                    0.0
                } else {
                    depth * self.at(k as f32 / ms as f32, depth)
                }
            })
            .collect()
    }
}

/// Largest 1 ms forward difference (full-scale/s) of `hold` then `stroke`,
/// up to and including the first sample at or below `until`. The release is
/// reported on the tick the plunger is back within the deadzone (fixed API),
/// so `until` = 0.02 gives what the filter can have seen; `until` = 0 gives
/// the peak of the whole stroke.
fn true_peak(hold: f32, stroke: &[f32], until: f32) -> f32 {
    let mut prev = hold;
    let mut peak = 0.0f32;
    for &x in stroke {
        peak = peak.max((prev - x) * 1000.0);
        prev = x;
        if x <= until {
            break;
        }
    }
    peak
}

const DZ: f32 = 0.02;

/// 2 s rest, 300 ms pull, 100 ms hold, the stroke, 300 ms rest, through `q`
/// (a per-sample distortion). Returns the releases.
fn launch(depth: f32, stroke: &[f32], mut q: impl FnMut(f32) -> f32) -> Vec<f32> {
    let mut f = PlungerFilter::new(PlungerConfig::default());
    let mut t = 0u64;
    let mut out = Vec::new();
    let mut feed = |x: f32| {
        out.extend(f.update(q(x), t).release);
        t += 1;
    };
    (0..2000).for_each(|_| feed(0.0));
    (1..=300).for_each(|k| feed(depth * k as f32 / 300.0));
    (0..100).for_each(|_| feed(depth));
    stroke.iter().for_each(|&x| feed(x));
    (0..300).for_each(|_| feed(0.0));
    out
}

/// Errors against the peak up to the release tick and of the whole stroke.
fn error(depth: f32, shape: Shape, ms: u32) -> (f32, f32) {
    let stroke = shape.stroke(depth, ms);
    let rel = launch(depth, &stroke, |x| x);
    assert_eq!(rel.len(), 1, "{shape:?} {ms} ms depth {depth}: {rel:?}");
    let seen = true_peak(depth, &stroke, DZ);
    let whole = true_peak(depth, &stroke, 0.0);
    ((rel[0] - seen) / seen, (rel[0] - whole) / whole)
}

/// Review B4: every row of the reviewer's table, noise-free: exact (up to
/// rounding) against the largest 1 ms difference up to the release tick, and
/// within 5 % of the largest 1 ms difference of the whole stroke.
#[test]
fn b4_table_noise_free_shapes() {
    let rows = [
        ("linear 20 ms d0.6", Shape::Linear, 20, 0.6),
        ("quarter-sine 20 ms d0.6", Shape::QuarterSine, 20, 0.6),
        (
            "preloaded spring (1.0) 20 ms d0.6",
            Shape::Preloaded(1.0),
            20,
            0.6,
        ),
        (
            "preloaded spring (3.0) 25 ms d1.0",
            Shape::Preloaded(3.0),
            25,
            1.0,
        ),
        ("const. accel 20 ms d0.6", Shape::ConstAccel, 20, 0.6),
        ("const. accel 35 ms d0.3", Shape::ConstAccel, 35, 0.3),
        ("half-cosine 12 ms d0.6", Shape::HalfCosine, 12, 0.6),
        ("half-cosine 8 ms d1.0", Shape::HalfCosine, 8, 1.0),
        ("quarter-sine tap 5 ms d0.6", Shape::QuarterSine, 5, 0.6),
        ("const.-accel tap 5 ms d0.6", Shape::ConstAccel, 5, 0.6),
        ("decelerating tap 5 ms d1.0", Shape::Decelerating, 5, 1.0),
    ];
    for (name, shape, ms, depth) in rows {
        let (seen, whole) = error(depth, shape, ms);
        println!(
            "{name}: {:+.3} % of the peak up to the release tick, {:+.3} % of the whole stroke",
            seen * 100.0,
            whole * 100.0
        );
        assert!(seen.abs() <= 1e-3, "{name}: {:+.2} %", seen * 100.0);
        assert!(whole.abs() <= 0.05, "{name}: {:+.2} %", whole * 100.0);
    }
}

/// The same over a sweep of shapes, durations and depths.
#[test]
fn noise_free_shape_sweep_is_exact() {
    let shapes = [
        Shape::Linear,
        Shape::QuarterSine,
        Shape::Preloaded(0.3),
        Shape::Preloaded(1.0),
        Shape::Preloaded(3.0),
        Shape::ConstAccel,
        Shape::HalfCosine,
        Shape::Decelerating,
    ];
    let mut worst = 0.0f32;
    for shape in shapes {
        for ms in (3..=60).chain([70, 80, 90]) {
            for depth in [0.1f32, 0.15, 0.3, 0.37, 0.5, 0.6, 0.8, 1.0] {
                let stroke = shape.stroke(depth, ms);
                let want = true_peak(depth, &stroke, DZ);
                if want < 4.0 {
                    continue; // near release_min_speed: not a launch
                }
                let rel = launch(depth, &stroke, |x| x);
                assert_eq!(rel.len(), 1, "{shape:?} {ms} ms depth {depth}: {rel:?}");
                let e = (rel[0] - want).abs() / want;
                worst = worst.max(e);
                assert!(
                    e <= 0.05,
                    "{shape:?} {ms} ms depth {depth}: {} vs {want}",
                    rel[0]
                );
            }
        }
    }
    println!("worst noise-free error {:.3} %", worst * 100.0);
    assert!(worst <= 0.01, "worst {worst}");
}

/// xorshift64.
struct Rng(u64);
impl Rng {
    fn sym(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 40) as f32 / (1u64 << 23) as f32 - 1.0
    }
}

/// Worst error over seeds of a shape with uniform ±`a` noise.
fn noisy_worst(shape: Shape, ms: u32, depth: f32, a: f32, seeds: u64) -> f32 {
    let stroke = shape.stroke(depth, ms);
    let want = true_peak(depth, &stroke, DZ);
    let mut worst = 0.0f32;
    for seed in 0..seeds {
        let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
        let rel = launch(depth, &stroke, |x| x + a * r.sym());
        assert_eq!(
            rel.len(),
            1,
            "{shape:?} {ms} {depth} ±{a} seed {seed}: {rel:?}"
        );
        worst = worst.max((rel[0] - want).abs() / want);
    }
    worst
}

/// Curved strokes with noise: one release each; the error is bounded by the
/// measured values (≤ 3.5 % up to ±depth/1000, ≤ 9.7 % up to ±depth/200).
#[test]
fn noisy_curved_strokes() {
    let cases = [
        (Shape::QuarterSine, 20, 0.6),
        (Shape::Preloaded(1.0), 20, 0.6),
        (Shape::Preloaded(3.0), 25, 1.0),
        (Shape::ConstAccel, 20, 0.6),
        (Shape::HalfCosine, 12, 0.6),
        (Shape::HalfCosine, 30, 0.8),
        (Shape::Linear, 15, 0.6),
    ];
    for (shape, ms, depth) in cases {
        for div in [5000.0f32, 1000.0, 400.0, 200.0] {
            let w = noisy_worst(shape, ms, depth, depth / div, 100);
            println!(
                "{shape:?} {ms} ms d{depth} ±d/{div}: worst {:.1} %",
                w * 100.0
            );
            let allowed = if div >= 1000.0 { 0.05 } else { 0.12 };
            assert!(w <= allowed, "{shape:?} {ms} ms d{depth} ±d/{div}: {w}");
        }
    }
}

/// Undithered ADC readings (rest is one constant code, so the rest noise is
/// 0): linear strokes stay within 5 % from depth ≥ 100 steps.
#[test]
fn quantised_linear_strokes() {
    for bits in [8u32, 10, 12] {
        let m = ((1u32 << bits) - 1) as f32;
        let mut worst = 0.0f32;
        for depth in [0.1f32, 0.2, 0.4, 0.8, 1.0] {
            if depth * m < 100.0 {
                continue;
            }
            for ms in [8u32, 10, 13, 17, 20, 30, 45] {
                let stroke = Shape::Linear.stroke(depth, ms);
                let want = depth / ms as f32 * 1000.0;
                if want < 4.0 {
                    continue;
                }
                let rel = launch(depth, &stroke, |x| (x * m).round() / m);
                assert_eq!(rel.len(), 1, "{bits} bit {depth} {ms}: {rel:?}");
                let e = (rel[0] - want).abs() / want;
                worst = worst.max(e);
                assert!(
                    e <= 0.05,
                    "{bits} bit depth {depth} {ms} ms: {} vs {want}",
                    rel[0]
                );
            }
        }
        println!("{bits} bit linear: worst {:.2} %", worst * 100.0);
    }
}

/// Undithered ADC readings of curved strokes: within 5 % plus one step per
/// ms of the peak.
#[test]
fn quantised_curved_strokes() {
    for bits in [8u32, 10, 12] {
        let m = ((1u32 << bits) - 1) as f32;
        let mut worst = 0.0f32;
        for shape in [
            Shape::QuarterSine,
            Shape::ConstAccel,
            Shape::HalfCosine,
            Shape::Preloaded(1.0),
        ] {
            for depth in [0.4f32, 0.6, 1.0] {
                for ms in [8u32, 12, 20, 30] {
                    let stroke = shape.stroke(depth, ms);
                    let want = true_peak(depth, &stroke, DZ);
                    let rel = launch(depth, &stroke, |x| (x * m).round() / m);
                    assert_eq!(rel.len(), 1, "{bits} bit {shape:?} {depth} {ms}: {rel:?}");
                    let e = (rel[0] - want).abs() / want;
                    let rel_step = 1000.0 / m / want;
                    worst = worst.max(e);
                    assert!(
                        e <= 0.05 + rel_step,
                        "{bits} bit {shape:?} depth {depth} {ms} ms: {} vs {want}",
                        rel[0]
                    );
                }
            }
        }
        println!("{bits} bit curved: worst {:.2} %", worst * 100.0);
    }
}

fn hold(f: &mut PlungerFilter, t: &mut u64, level: f32, ms: u64) -> Vec<f32> {
    let mut r = Vec::new();
    for _ in 0..ms {
        r.extend(f.update(level, *t).release);
        *t += 1;
    }
    r
}

fn ramp(f: &mut PlungerFilter, t: &mut u64, a: f32, b: f32, ms: u64) -> Vec<f32> {
    let mut r = Vec::new();
    for k in 1..=ms {
        r.extend(f.update(a + (b - a) * k as f32 / ms as f32, *t).release);
        *t += 1;
    }
    r
}

/// N-k: one spike to qualifying depth inside a long shallow excursion is not
/// a launch; the pull must spend 3 updates at qualifying depth.
#[test]
fn shallow_hold_with_one_deep_sample_is_not_a_launch() {
    for spike in [vec![0.2f32], vec![0.2, 0.9]] {
        let mut f = PlungerFilter::new(PlungerConfig::default());
        let mut t = 0;
        let mut r = hold(&mut f, &mut t, 0.0, 2000);
        r.extend(hold(&mut f, &mut t, 0.03, 400));
        for &x in &spike {
            r.extend(f.update(x, t).release);
            t += 1;
        }
        r.extend(hold(&mut f, &mut t, 0.03, 400));
        r.extend(ramp(&mut f, &mut t, 0.03, 0.0, 3));
        r.extend(hold(&mut f, &mut t, 0.0, 300));
        assert!(r.is_empty(), "{spike:?}: {r:?}");
    }
}

/// Fast taps (pull ms / return ms) still launch, with the exact speed.
#[test]
fn fast_taps_still_launch() {
    for (pull, ret) in [
        (2u64, 2u64),
        (2, 3),
        (3, 2),
        (3, 3),
        (5, 5),
        (10, 4),
        (30, 10),
    ] {
        for depth in [0.2f32, 0.5, 1.0] {
            let mut f = PlungerFilter::new(PlungerConfig::default());
            let mut t = 0;
            hold(&mut f, &mut t, 0.0, 500);
            let mut r = ramp(&mut f, &mut t, 0.0, depth, pull);
            r.extend(ramp(&mut f, &mut t, depth, 0.0, ret));
            r.extend(hold(&mut f, &mut t, 0.0, 300));
            assert_eq!(r.len(), 1, "tap {pull}/{ret} depth {depth}: {r:?}");
            let want = depth / ret as f32 * 1000.0;
            assert!(
                (r[0] - want).abs() <= 0.05 * want,
                "{pull}/{ret} {depth}: {}",
                r[0]
            );
        }
    }
}

/// N-l (a) and (b): the reviewer's two offset-device histories.
#[test]
fn offset_device_reviewer_cases() {
    // (a) rest 0.1, pushed to 0.0 for 1.2 s, eased to 0.045 for 1.2 s, let go.
    let mut f = PlungerFilter::new(PlungerConfig::default());
    let mut t = 0;
    hold(&mut f, &mut t, 0.1, 2000);
    hold(&mut f, &mut t, 0.0, 1200);
    hold(&mut f, &mut t, 0.045, 1200);
    hold(&mut f, &mut t, 0.1, 2000);
    assert!(
        !f.is_pulled() && (f.rest() - 0.1).abs() < 0.005,
        "rest {}",
        f.rest()
    );
    launch_from(&mut f, &mut t, 0.1, 0.5, 20, "(a)");
    // (b) rest −0.1, held still at reading 0 for 1.2 s, returned.
    let mut f = PlungerFilter::new(PlungerConfig::default());
    let mut t = 0;
    hold(&mut f, &mut t, -0.1, 2000);
    ramp(&mut f, &mut t, -0.1, 0.0, 100);
    hold(&mut f, &mut t, 0.0, 1200);
    ramp(&mut f, &mut t, 0.0, -0.1, 100);
    hold(&mut f, &mut t, -0.1, 2000);
    launch_from(&mut f, &mut t, -0.1, 0.5, 20, "(b) at 2 s");
    hold(&mut f, &mut t, -0.1, 1300);
    launch_from(&mut f, &mut t, -0.1, 0.5, 20, "(b) at 4 s");
}

fn launch_from(
    f: &mut PlungerFilter,
    t: &mut u64,
    rest: f32,
    depth: f32,
    snap_ms: u64,
    what: &str,
) {
    let mut r = ramp(f, t, rest, rest + depth, 300);
    r.extend(hold(f, t, rest + depth, 100));
    r.extend(ramp(f, t, rest + depth, rest, snap_ms));
    r.extend(hold(f, t, rest, 300));
    assert_eq!(r.len(), 1, "{what}: releases {r:?}, rest {}", f.rest());
    let want = depth / snap_ms as f32 * 1000.0;
    assert!(
        (r[0] - want).abs() <= 0.05 * want,
        "{what}: {} vs {want}",
        r[0]
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// N-l: on a device whose rest (acquired at power-on: 2 s still) is
    /// offset by −0.25..0.25, from any later history, after ≤ 2 s still at
    /// that rest the next normal launch reports exactly one release.
    #[test]
    fn offset_device_never_stuck(
        offset in -0.25f32..0.25,
        segs in prop::collection::vec((-0.35f32..1.0, 1u64..4000, any::<bool>()), 0..8),
        depth in 0.1f32..0.7,
        snap_ms in 5u64..20,
    ) {
        let mut f = PlungerFilter::new(PlungerConfig::default());
        let mut t = 0u64;
        hold(&mut f, &mut t, offset, 2000);
        let mut last = offset;
        for (level, ms, ramped) in segs {
            if ramped {
                ramp(&mut f, &mut t, last, level, ms.min(500));
            }
            hold(&mut f, &mut t, level, ms);
            last = level;
        }
        ramp(&mut f, &mut t, last, offset, 300);
        hold(&mut f, &mut t, offset, 2000);
        let mut r = ramp(&mut f, &mut t, offset, offset + depth, 300);
        r.extend(hold(&mut f, &mut t, offset + depth, 100));
        r.extend(ramp(&mut f, &mut t, offset + depth, offset, snap_ms));
        r.extend(hold(&mut f, &mut t, offset, 300));
        prop_assert_eq!(r.len(), 1, "offset {} rest {} releases {:?}", offset, f.rest(), r);
        let want = depth / snap_ms as f32 * 1000.0;
        prop_assert!((r[0] - want).abs() <= 0.05 * want, "{} vs {}", r[0], want);
    }
}
