//! Implementer tests for `pin_physics::world::CubicSpline` (Subphase 2.1).

use glam::Vec3;
use pin_physics::world::{CubicSpline, SplineError};
use proptest::prelude::*;

fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

/// A wiggly 3D test curve with ~50 mm point spacing.
fn wiggle() -> CubicSpline {
    let pts: Vec<Vec3> = (0..12)
        .map(|i| {
            let x = i as f32 * 0.05;
            v(
                x,
                0.04 * libm::sinf(7.0 * x),
                0.02 * libm::cosf(11.0 * x) + 0.01 * i as f32,
            )
        })
        .collect();
    CubicSpline::interpolate(&pts).unwrap()
}

/// Dense brute-force closest distance (f64), refined by ternary search.
fn brute(spline: &CubicSpline, p: Vec3) -> f64 {
    let n = spline.segment_count();
    let per = 400;
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
    let mut lo = (best_k as f64 - 1.0).max(0.0) / per as f64;
    let mut hi = ((best_k + 1) as f64).min((n * per) as f64) / per as f64;
    for _ in 0..40 {
        let a = lo + (hi - lo) / 3.0;
        let b = hi - (hi - lo) / 3.0;
        if dist(a) < dist(b) {
            hi = b;
        } else {
            lo = a;
        }
    }
    best.min(dist(0.5 * (lo + hi)))
}

fn rel_close(a: Vec3, b: Vec3, rel: f32, abs: f32) -> bool {
    (a - b).length() <= rel * a.length().max(b.length()) + abs
}

#[test]
fn errors() {
    assert_eq!(
        CubicSpline::interpolate(&[]).unwrap_err(),
        SplineError::TooFewPoints
    );
    assert_eq!(
        CubicSpline::interpolate(&[Vec3::ZERO]).unwrap_err(),
        SplineError::TooFewPoints
    );
    assert_eq!(
        CubicSpline::interpolate(&[Vec3::ZERO, v(f32::NAN, 0.0, 0.0)]).unwrap_err(),
        SplineError::NonFinite
    );
    assert_eq!(
        CubicSpline::interpolate(&[Vec3::ZERO, v(0.0, f32::INFINITY, 0.0)]).unwrap_err(),
        SplineError::NonFinite
    );
    assert_eq!(
        CubicSpline::interpolate(&[Vec3::ZERO, Vec3::X, Vec3::X, Vec3::Y]).unwrap_err(),
        SplineError::DuplicatePoints
    );
    // Coordinates whose differences overflow f32.
    assert_eq!(
        CubicSpline::interpolate(&[v(3.0e38, 0.0, 0.0), v(-3.0e38, 0.0, 0.0), Vec3::ZERO])
            .unwrap_err(),
        SplineError::NonFinite
    );
    // Non-consecutive repeats are fine (closed loops).
    assert!(CubicSpline::interpolate(&[Vec3::ZERO, Vec3::X, Vec3::ZERO]).is_ok());
    for e in [
        SplineError::TooFewPoints,
        SplineError::NonFinite,
        SplineError::DuplicatePoints,
        SplineError::RollCountMismatch,
        SplineError::InvalidGauge,
        SplineError::InvalidUpHint,
        SplineError::InvalidHeight,
    ] {
        assert!(!e.to_string().is_empty());
        let _: &dyn std::error::Error = &e;
    }
}

#[test]
fn closed_spline_is_periodic() {
    let mut pts: Vec<Vec3> = (0..9)
        .map(|i| {
            let a = i as f32 * std::f32::consts::TAU / 9.0;
            v(
                0.2 * libm::cosf(a),
                0.1 * libm::sinf(a) + 0.03 * libm::sinf(3.0 * a),
                0.02 * libm::cosf(2.0 * a),
            )
        })
        .collect();
    assert_eq!(
        CubicSpline::interpolate_closed(&pts).unwrap_err(),
        SplineError::NotClosed
    );
    pts.push(pts[0]);
    assert_eq!(
        CubicSpline::interpolate_closed(&pts[..1]).unwrap_err(),
        SplineError::TooFewPoints
    );
    assert_eq!(
        CubicSpline::interpolate_closed(&[pts[0], pts[1], pts[0]]).unwrap_err(),
        SplineError::TooFewPoints
    );
    let mut bad = pts.clone();
    bad[3].x = f32::NAN;
    assert_eq!(
        CubicSpline::interpolate_closed(&bad).unwrap_err(),
        SplineError::NonFinite
    );
    let mut dup = pts.clone();
    dup[4] = dup[5];
    assert_eq!(
        CubicSpline::interpolate_closed(&dup).unwrap_err(),
        SplineError::DuplicatePoints
    );
    assert!(!SplineError::NotClosed.to_string().is_empty());
    assert!(!SplineError::ClosedGuideNotPeriodic.to_string().is_empty());

    let s = CubicSpline::interpolate_closed(&pts).unwrap();
    assert!(s.is_closed());
    assert!(!CubicSpline::interpolate(&pts).unwrap().is_closed());
    assert_eq!(s.segment_count(), 9);
    for (i, p) in pts.iter().enumerate() {
        assert_eq!(s.point(i as f32), *p);
    }
    // C² at every knot, and across the seam (t = 0 and t = n).
    let segs = s.segments();
    for i in 0..9 {
        let (l, r) = (segs[(i + 8) % 9], segs[i]);
        assert_eq!(l.p[3], r.p[0]);
        let dl = (l.p[3] - l.p[2]) * 3.0;
        let dr = (r.p[1] - r.p[0]) * 3.0;
        assert!(rel_close(dl, dr, 1e-4, 1e-6), "C1 at knot {i}");
        let ddl = (l.p[3] - l.p[2] * 2.0 + l.p[1]) * 6.0;
        let ddr = (r.p[2] - r.p[1] * 2.0 + r.p[0]) * 6.0;
        assert!(rel_close(ddl, ddr, 1e-3, 5e-6), "C2 at knot {i}");
    }
    assert!(rel_close(s.tangent(0.0), s.tangent(9.0), 1e-4, 1e-6));
    assert_eq!(s.second_derivative(0.0), s.second_derivative(9.0));
    assert!(s.second_derivative(0.0).length() > 1e-3);
    // The cyclic system is solved: M[i-1] + 4 M[i] + M[i+1] = 6 Δ²P[i].
    for i in 0..9 {
        let m = |j: usize| s.second_derivative((j % 9) as f32);
        let lhs = m(i + 8) + m(i) * 4.0 + m(i + 1);
        let rhs = (pts[(i + 8) % 9] - pts[i] * 2.0 + pts[(i + 1) % 9]) * 6.0;
        assert!((lhs - rhs).length() < 1e-5, "row {i}: {lhs} vs {rhs}");
    }
    // Closest point is global across the seam.
    let p = s.point(0.0) + v(0.01, 0.002, 0.003);
    for seed in [None, Some(0.2), Some(8.8), Some(4.0)] {
        let cp = s.closest_point(p, seed);
        assert!((f64::from(cp.distance) - brute(&s, p)).abs() < 1e-5);
    }
}

#[test]
fn two_points_is_a_straight_line() {
    let s = CubicSpline::interpolate(&[v(0.0, 0.0, 0.0), v(0.3, 0.6, -0.3)]).unwrap();
    assert_eq!(s.segment_count(), 1);
    assert_eq!(s.max_t(), 1.0);
    for k in 0..=10 {
        let t = k as f32 / 10.0;
        assert!((s.point(t) - v(0.3, 0.6, -0.3) * t).length() < 1e-6);
        assert!((s.tangent(t) - v(0.3, 0.6, -0.3)).length() < 1e-5);
        assert_eq!(s.second_derivative(t), Vec3::ZERO);
    }
}

#[test]
fn passes_through_points_and_clamps() {
    let pts = [
        v(0.0, 0.0, 0.0),
        v(0.1, 0.05, 0.0),
        v(0.2, -0.02, 0.03),
        v(0.25, 0.1, 0.05),
        v(0.4, 0.1, 0.0),
    ];
    let s = CubicSpline::interpolate(&pts).unwrap();
    assert_eq!(s.segment_count(), 4);
    for (i, p) in pts.iter().enumerate() {
        assert_eq!(s.point(i as f32), *p, "exact at knot {i}");
    }
    assert_eq!(s.point(-3.0), pts[0]);
    assert_eq!(s.point(99.0), pts[4]);
    assert_eq!(s.point(f32::NAN), pts[0]);
    assert_eq!(s.tangent(-1.0), s.tangent(0.0));
    assert_eq!(s.tangent(7.0), s.tangent(4.0));
    assert_eq!(s.second_derivative(7.0), s.second_derivative(4.0));
    // Natural end conditions.
    assert_eq!(s.second_derivative(0.0), Vec3::ZERO);
    assert_eq!(s.second_derivative(4.0), Vec3::ZERO);
}

#[test]
fn bezier_storage_matches_evaluation() {
    let s = wiggle();
    for (i, seg) in s.segments().iter().enumerate() {
        assert_eq!(seg.p[0], s.point(i as f32));
        assert_eq!(seg.p[3], s.point(i as f32 + 1.0));
        for k in 0..=8 {
            let u = k as f32 / 8.0;
            let a = 1.0 - u;
            // De Casteljau.
            let q0 = seg.p[0] * a + seg.p[1] * u;
            let q1 = seg.p[1] * a + seg.p[2] * u;
            let q2 = seg.p[2] * a + seg.p[3] * u;
            let r0 = q0 * a + q1 * u;
            let r1 = q1 * a + q2 * u;
            let c = r0 * a + r1 * u;
            let t = i as f32 + u;
            assert!((s.point(t) - c).length() < 1e-6);
            assert!((seg.point(u) - c).length() < 1e-6);
            assert!((s.tangent(t) - (r1 - r0) * 3.0).length() < 1e-5);
            assert!((seg.tangent(u) - (r1 - r0) * 3.0).length() < 1e-5);
            // Second derivative of the Bézier form agrees with the stored one.
            let dd = ((seg.p[2] - seg.p[1] * 2.0 + seg.p[0]) * a
                + (seg.p[3] - seg.p[2] * 2.0 + seg.p[1]) * u)
                * 6.0;
            assert!((s.second_derivative(t) - dd).length() < 2e-5);
        }
    }
}

#[test]
fn c2_at_knots() {
    let s = wiggle();
    let eps = 1.0e-4;
    for i in 1..s.segment_count() {
        let t = i as f32;
        assert!((s.point(t - eps) - s.point(t + eps)).length() < 1e-4);
        assert!(rel_close(
            s.tangent(t - eps),
            s.tangent(t + eps),
            1e-3,
            1e-6
        ));
        assert!(rel_close(
            s.second_derivative(t - eps),
            s.second_derivative(t + eps),
            1e-3,
            1e-6
        ));
        // One-sided limits taken from the Bézier control points themselves.
        let (l, r) = (s.segments()[i - 1], s.segments()[i]);
        let dl = (l.p[3] - l.p[2]) * 3.0;
        let dr = (r.p[1] - r.p[0]) * 3.0;
        assert!(rel_close(dl, dr, 1e-4, 1e-6), "C1 at knot {i}");
        let ddl = (l.p[3] - l.p[2] * 2.0 + l.p[1]) * 6.0;
        let ddr = (r.p[2] - r.p[1] * 2.0 + r.p[0]) * 6.0;
        assert!(rel_close(ddl, ddr, 1e-3, 5e-6), "C2 at knot {i}");
    }
}

#[test]
fn derivatives_match_finite_differences() {
    let s = wiggle();
    let h = 1.0e-2;
    for k in 1..(s.segment_count() * 4) {
        let t = k as f32 * 0.25 + 0.1;
        if t + h > s.max_t() {
            break;
        }
        let fd = (s.point(t + h) - s.point(t - h)) / (2.0 * h);
        assert!(rel_close(fd, s.tangent(t), 2e-3, 1e-5), "tangent at {t}");
        let fd2 = (s.tangent(t + h) - s.tangent(t - h)) / (2.0 * h);
        assert!(
            rel_close(fd2, s.second_derivative(t), 5e-3, 1e-4),
            "second derivative at {t}"
        );
    }
}

#[test]
fn approximates_a_circle() {
    // 10 degree spacing on a 150 mm circle, away from the natural ends.
    let r = 0.15f32;
    let pts: Vec<Vec3> = (0..=36)
        .map(|i| {
            let a = (i as f32 * 10.0).to_radians();
            v(r * libm::cosf(a), r * libm::sinf(a), 0.0)
        })
        .collect();
    let s = CubicSpline::interpolate(&pts).unwrap();
    for k in 0..=200 {
        let t = 8.0 + 20.0 * k as f32 / 200.0;
        let p = s.point(t);
        assert!((p.length() - r).abs() < 2e-6, "radius at {t}");
        let d1 = s.tangent(t);
        let d2 = s.second_derivative(t);
        let curvature = d1.cross(d2).length() / d1.length().powi(3);
        assert!((curvature * r - 1.0).abs() < 5e-3, "curvature at {t}");
    }
}

#[test]
fn closest_point_basics() {
    let s = wiggle();
    // On the curve.
    for k in 0..=44 {
        let t = k as f32 * 0.25;
        let cp = s.closest_point(s.point(t), None);
        assert!(cp.distance < 1e-6);
        assert!((cp.point - s.point(t)).length() < 1e-6);
    }
    // Beyond the ends: clamps to the end points.
    let start = s.point(0.0) - s.tangent(0.0).normalize() * 0.1;
    let cp = s.closest_point(start, None);
    assert_eq!(cp.t, 0.0);
    assert!((cp.distance - 0.1).abs() < 1e-6);
    let end = s.point(s.max_t()) + s.tangent(s.max_t()).normalize() * 0.1;
    let cp = s.closest_point(end, Some(3.0));
    assert_eq!(cp.t, s.max_t());
    assert!((cp.distance - 0.1).abs() < 1e-6);
    // Degenerate queries do not panic.
    let cp = s.closest_point(v(f32::NAN, 0.0, 0.0), Some(1.0));
    assert_eq!(cp.t, 0.0);
    assert!(cp.distance.is_infinite());
    let a = s.closest_point(v(0.2, 0.1, 0.0), Some(f32::NAN));
    let b = s.closest_point(v(0.2, 0.1, 0.0), None);
    assert_eq!(a, b);
}

#[test]
fn closest_point_is_global_whatever_the_seed() {
    // A hairpin: two nearly parallel legs 40 mm apart.
    let pts = [
        v(0.0, 0.0, 0.0),
        v(0.1, 0.0, 0.0),
        v(0.2, 0.0, 0.0),
        v(0.24, 0.02, 0.0),
        v(0.2, 0.04, 0.0),
        v(0.1, 0.04, 0.0),
        v(0.0, 0.04, 0.0),
    ];
    let s = CubicSpline::interpolate(&pts).unwrap();
    let p = v(0.1, 0.03, 0.0); // closest to the upper leg (t = 5)
    let expected = brute(&s, p);
    for seed in [
        None,
        Some(0.0),
        Some(1.0),
        Some(3.0),
        Some(5.0),
        Some(6.0),
        Some(-4.0),
        Some(1e9),
    ] {
        let cp = s.closest_point(p, seed);
        assert!(
            (f64::from(cp.distance) - expected).abs() < 1e-5,
            "seed {seed:?}"
        );
        assert!(cp.t > 4.0 && cp.t < 6.0, "seed {seed:?} gave t = {}", cp.t);
        assert!((s.point(cp.t) - cp.point).length() < 1e-6);
    }
}

#[test]
fn closest_point_from_the_centre_of_curvature() {
    // Nearly equidistant from a long stretch of the curve: the flat case.
    let r = 0.15f32;
    let pts: Vec<Vec3> = (0..=18)
        .map(|i| {
            let a = (i as f32 * 10.0).to_radians();
            v(r * libm::cosf(a), r * libm::sinf(a), 0.0)
        })
        .collect();
    let s = CubicSpline::interpolate(&pts).unwrap();
    for p in [Vec3::ZERO, v(1e-4, 2e-4, 0.0), v(0.0, 0.0, 0.05)] {
        let cp = s.closest_point(p, None);
        assert!((f64::from(cp.distance) - brute(&s, p)).abs() < 1e-5);
    }
}

fn spline_points() -> impl Strategy<Value = Vec<Vec3>> {
    // Random walk with 20..80 mm steps: no duplicates, table-sized curves.
    (
        prop::collection::vec(
            (0.02f32..0.08, -1.0f32..1.0, -1.0f32..1.0, -0.3f32..0.3),
            1..10,
        ),
        (-0.5f32..0.5, -0.5f32..0.5, -0.1f32..0.1),
    )
        .prop_map(|(steps, start)| {
            let mut p = v(start.0, start.1, start.2);
            let mut pts = vec![p];
            for (len, dx, dy, dz) in steps {
                let mut dir = v(dx, dy, dz);
                if dir.length() < 0.1 {
                    dir = Vec3::X;
                }
                p += dir.normalize() * len;
                pts.push(p);
            }
            pts
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn prop_interpolates_and_is_c2(pts in spline_points()) {
        let s = CubicSpline::interpolate(&pts).unwrap();
        prop_assert_eq!(s.segment_count(), pts.len() - 1);
        prop_assert_eq!(s.segments().len(), pts.len() - 1);
        for (i, p) in pts.iter().enumerate() {
            prop_assert_eq!(s.point(i as f32), *p);
        }
        prop_assert_eq!(s.second_derivative(0.0), Vec3::ZERO);
        prop_assert_eq!(s.second_derivative(s.max_t()), Vec3::ZERO);
        for i in 1..s.segment_count() {
            let (l, r) = (s.segments()[i - 1], s.segments()[i]);
            prop_assert_eq!(l.p[3], r.p[0]);
            let dl = (l.p[3] - l.p[2]) * 3.0;
            let dr = (r.p[1] - r.p[0]) * 3.0;
            prop_assert!(rel_close(dl, dr, 1e-4, 2e-6));
            let ddl = (l.p[3] - l.p[2] * 2.0 + l.p[1]) * 6.0;
            let ddr = (r.p[2] - r.p[1] * 2.0 + r.p[0]) * 6.0;
            prop_assert!(rel_close(ddl, ddr, 1e-3, 1e-5));
            let t = i as f32;
            // Evaluated on both sides of the knot (the curve itself moves a
            // little over the 2e-5 parameter gap).
            let e = 1.0e-5;
            let drift = 4.0 * e * s.second_derivative(t).length();
            prop_assert!(rel_close(s.tangent(t - e), s.tangent(t + e), 1e-3, 1e-6 + drift));
            let jerk = (s.second_derivative(t + 0.5) - s.second_derivative(t - 0.5)).length();
            prop_assert!(rel_close(
                s.second_derivative(t - e),
                s.second_derivative(t + e),
                1e-3,
                1e-6 + 4.0 * e * jerk
            ));
        }
    }

    #[test]
    fn prop_evaluation_is_finite_and_bounded(pts in spline_points(), t in -2.0f32..12.0) {
        let s = CubicSpline::interpolate(&pts).unwrap();
        let p = s.point(t);
        prop_assert!(p.is_finite() && s.tangent(t).is_finite() && s.second_derivative(t).is_finite());
        // Convex-hull property of the Bézier form.
        let (i, _) = (t.clamp(0.0, s.max_t()) as usize, 0);
        let seg = s.segments()[i.min(s.segment_count() - 1)];
        let lo = seg.p.iter().fold(Vec3::splat(f32::MAX), |a, b| a.min(*b));
        let hi = seg.p.iter().fold(Vec3::splat(f32::MIN), |a, b| a.max(*b));
        prop_assert!(p.cmpge(lo - 1e-5).all() && p.cmple(hi + 1e-5).all());
    }

    #[test]
    fn prop_closest_point_matches_brute_force(
        pts in spline_points(),
        at in 0.0f32..1.0,
        off in (-1.0f32..1.0, -1.0f32..1.0, -1.0f32..1.0),
        dist in 0.0f32..0.2,
        seed in prop::option::of(-1.0f32..11.0),
    ) {
        let s = CubicSpline::interpolate(&pts).unwrap();
        let mut dir = v(off.0, off.1, off.2);
        if dir.length() < 0.05 {
            dir = Vec3::Z;
        }
        let p = s.point(at * s.max_t()) + dir.normalize() * dist;
        let expected = brute(&s, p);
        let cp = s.closest_point(p, seed);
        prop_assert!(cp.t >= 0.0 && cp.t <= s.max_t());
        prop_assert!((f64::from(cp.distance) - expected).abs() < 2e-5,
            "distance {} vs brute force {}", cp.distance, expected);
        prop_assert!((cp.point - s.point(cp.t)).length() < 1e-6);
        prop_assert!((cp.point.distance(p) - cp.distance).abs() < 1e-6);
        // Seeded and unseeded agree.
        let unseeded = s.closest_point(p, None);
        prop_assert!((unseeded.distance - cp.distance).abs() < 2e-5);
    }
}
