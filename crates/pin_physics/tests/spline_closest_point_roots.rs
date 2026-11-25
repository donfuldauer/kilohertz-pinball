//! Issue #8 (PR #4 review r4573613071): `CubicSpline::closest_point` returns
//! the global minimum even where one sample interval holds several roots of
//! `g = (C - p) · C'`, so seeded and unseeded queries agree.
//!
//! Tolerances: seeded and unseeded distances agree within `DIST_TOL` =
//! 0.3 µm (f32 spacing is 0.12 µm at the 1.2 m coordinates of the widest
//! spline here), and the distance is within 1 µm of a dense f64 brute-force
//! minimum. The old code differed by up to 8.3 µm on the review's example.

use glam::Vec3;
use pin_core::fmath::{max_f32, max_f64, min_f32, min_f64};
use pin_physics::world::CubicSpline;
use std::time::Instant;

/// Seeded and unseeded distances agree within this (metres).
const DIST_TOL: f32 = 3.0e-7;
/// Distance within this of the brute-force minimum (metres).
const BRUTE_TOL: f64 = 1.0e-6;

/// Dense brute-force closest distance (f64 arithmetic on the f32 curve),
/// refined by ternary search around the best sample.
fn brute(spline: &CubicSpline, p: Vec3) -> f64 {
    let n = spline.segment_count();
    let per = 1000;
    let dist = |t: f64| (spline.point(t as f32).as_dvec3() - p.as_dvec3()).length();
    let mut best = f64::INFINITY;
    let mut best_k = 0;
    for k in 0..=n * per {
        let d = dist(k as f64 / per as f64);
        if d < best {
            best = d;
            best_k = k;
        }
    }
    let mut lo = max_f64(best_k as f64 - 1.0, 0.0) / per as f64;
    let mut hi = min_f64((best_k + 1) as f64, (n * per) as f64) / per as f64;
    for _ in 0..60 {
        let a = lo + (hi - lo) / 3.0;
        let b = hi - (hi - lo) / 3.0;
        if dist(a) < dist(b) {
            hi = b;
        } else {
            lo = a;
        }
    }
    min_f64(best, dist(0.5 * (lo + hi)))
}

/// The review's tight curve: the natural spline overshoots the last point
/// and turns back sharply (a near-cusp) in its second segment.
fn review_curve() -> CubicSpline {
    CubicSpline::interpolate(&[
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(1.199, 0.0001, 0.0),
    ])
    .unwrap()
}

/// The review's curve plus tightly curved splines at pinball scale: arcs of
/// 50, 20 and 10 mm radius, a hairpin and a rising zig-zag.
fn tight_splines() -> Vec<(&'static str, CubicSpline)> {
    let mut out = vec![("review", review_curve())];
    for (name, r, step) in [
        ("arc50", 0.05f32, 0.2f32),
        ("arc20", 0.02, 0.5),
        ("arc10", 0.01, 0.9),
    ] {
        let pts: Vec<Vec3> = (0..=10)
            .map(|i| {
                let a = i as f32 * step;
                Vec3::new(r * libm::cosf(a), r * libm::sinf(a), 0.0)
            })
            .collect();
        out.push((name, CubicSpline::interpolate(&pts).unwrap()));
    }
    let mut pts: Vec<Vec3> = (0..=5)
        .map(|i| Vec3::new(0.01 * i as f32, 0.0, 0.0))
        .collect();
    pts.push(Vec3::new(0.06, 0.01, 0.0));
    pts.extend((0..=5).map(|i| Vec3::new(0.05 - 0.01 * i as f32, 0.02, 0.0)));
    out.push(("hairpin", CubicSpline::interpolate(&pts).unwrap()));
    let pts: Vec<Vec3> = (0..=10)
        .map(|i| {
            let y = if i % 2 == 0 { 0.0 } else { 0.012 };
            Vec3::new(0.01 * i as f32, y, 0.002 * i as f32)
        })
        .collect();
    out.push(("zigzag", CubicSpline::interpolate(&pts).unwrap()));
    out
}

/// Checks one query: unseeded vs brute force, and seeds spread over the
/// whole curve vs unseeded. Returns the largest seeded-vs-unseeded
/// distance difference.
fn check(s: &CubicSpline, p: Vec3, seeds: usize) -> f32 {
    let un = s.closest_point(p, None);
    let expected = brute(s, p);
    assert!(
        (f64::from(un.distance) - expected).abs() <= BRUTE_TOL,
        "p {p}: unseeded {} (t {}) vs brute force {expected}",
        un.distance,
        un.t
    );
    assert!((un.point - s.point(un.t)).length() < 1e-6);
    assert!((un.point.distance(p) - un.distance).abs() < 1e-6);
    let mut worst = 0.0f32;
    for j in 0..=seeds {
        let seed = s.max_t() * j as f32 / seeds as f32;
        let c = s.closest_point(p, Some(seed));
        let diff = (c.distance - un.distance).abs();
        worst = max_f32(worst, diff);
        assert!(
            diff <= DIST_TOL,
            "p {p} seed {seed}: seeded {} (t {}) vs unseeded {} (t {})",
            c.distance,
            c.t,
            un.distance,
            un.t
        );
        assert!((c.point - s.point(c.t)).length() < 1e-6);
        assert!((c.point.distance(p) - c.distance).abs() < 1e-6);
    }
    worst
}

/// The review's example: queries on the curve near t = 1.97 (and the worst
/// case found, t = 1.993) got a farther root of the same sample interval
/// unless the seed was close.
#[test]
fn review_example_seeded_and_unseeded_agree() {
    let s = review_curve();
    for t in [1.95f32, 1.96, 1.97, 1.98, 1.99, 1.993] {
        for off in [0.0f32, 1.0e-5, -1.0e-5, 1.0e-4, -1.0e-3] {
            let p = s.point(t) + Vec3::new(0.0, off, 0.0);
            check(&s, p, 40);
        }
    }
}

/// Sweep of query points around tightly curved splines, in and above the
/// curve plane, including points near centres of curvature.
#[test]
fn tight_curves_seeded_and_unseeded_agree() {
    let mut worst = 0.0f32;
    for (_, s) in tight_splines() {
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for k in 0..=200 {
            let q = s.point(k as f32 / 200.0 * s.max_t());
            lo = Vec3::new(min_f32(lo.x, q.x), min_f32(lo.y, q.y), min_f32(lo.z, q.z));
            hi = Vec3::new(max_f32(hi.x, q.x), max_f32(hi.y, q.y), max_f32(hi.z, q.z));
        }
        let span = hi - lo;
        let pad = 0.1 * max_f32(span.x, max_f32(span.y, span.z));
        let m = 13;
        let f = |i: usize| i as f32 / (m - 1) as f32;
        for a in 0..m {
            for b in 0..m {
                for z in [0.0f32, 0.003] {
                    let p = Vec3::new(
                        lo.x - pad + (hi.x - lo.x + 2.0 * pad) * f(a),
                        lo.y - pad + (hi.y - lo.y + 2.0 * pad) * f(b),
                        0.5 * (lo.z + hi.z) + z,
                    );
                    worst = max_f32(worst, check(&s, p, 10));
                }
            }
        }
        // Points on and just off the curve.
        for k in 0..=40 {
            let t = s.max_t() * k as f32 / 40.0;
            for off in [Vec3::ZERO, Vec3::new(2.0e-4, -1.0e-4, 0.0)] {
                worst = max_f32(worst, check(&s, s.point(t) + off, 10));
            }
        }
    }
    println!("largest seeded-vs-unseeded difference: {worst:e} m");
}

/// Timing report (release, `--ignored`): unseeded and seeded queries near
/// the tight curves, where sample intervals may hold several roots. Asserts
/// nothing on time (PLAN §2.3 rule 5).
#[test]
#[ignore = "timing report; run with --release -- --ignored --nocapture"]
fn report_tight_curve_query_time() {
    for (name, s) in tight_splines() {
        let mut pts = Vec::new();
        for k in 0..=400 {
            let t = s.max_t() * k as f32 / 400.0;
            let side = s.tangent(t).normalize_or_zero().cross(Vec3::Z);
            for off in [0.0f32, 0.002, -0.002, 0.0135, -0.0135, 0.05, -0.05] {
                pts.push((t, s.point(t) + side * off + Vec3::Z * 0.001));
            }
        }
        let mut best = [f64::INFINITY; 2];
        let mut sink = 0.0f32;
        for _ in 0..7 {
            for (slot, seeded) in [false, true].into_iter().enumerate() {
                let start = Instant::now();
                for &(t, p) in &pts {
                    sink += s.closest_point(p, seeded.then_some(t)).distance;
                }
                let per = start.elapsed().as_secs_f64() / pts.len() as f64;
                best[slot] = min_f64(best[slot], per);
            }
        }
        println!(
            "{name}: closest_point {:.0} ns (seeded {:.0} ns) over {} queries, fastest of 7 [{sink:.1}]",
            best[0] * 1e9,
            best[1] * 1e9,
            pts.len()
        );
    }
}
