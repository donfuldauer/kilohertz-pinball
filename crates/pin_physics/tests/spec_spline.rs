//! Independent spec tests (Subphase 2.1): C² interpolating cubic splines.
//!
//! Black-box tests against the fixed public API of `pin_physics::world`:
//! interpolation at integer t, C⁰/C¹/C² continuity at knots, natural end
//! conditions, Bézier storage consistency, derivative consistency, clamping,
//! closest point vs brute force, seeded vs unseeded agreement and error cases.

use glam::Vec3;
use pin_physics::world::{BezierSegment, ClosestPoint, CubicSpline};
use proptest::prelude::*;

// ---------------------------------------------------------------- helpers

fn bezier_eval(seg: &BezierSegment, u: f32) -> Vec3 {
    // de Casteljau.
    let [p0, p1, p2, p3] = seg.p;
    let a = p0.lerp(p1, u);
    let b = p1.lerp(p2, u);
    let c = p2.lerp(p3, u);
    let d = a.lerp(b, u);
    let e = b.lerp(c, u);
    d.lerp(e, u)
}

fn bezier_d1(seg: &BezierSegment, u: f32) -> Vec3 {
    let [p0, p1, p2, p3] = seg.p;
    let v = 1.0 - u;
    (p1 - p0) * (3.0 * v * v) + (p2 - p1) * (6.0 * v * u) + (p3 - p2) * (3.0 * u * u)
}

fn bezier_d2(seg: &BezierSegment, u: f32) -> Vec3 {
    let [p0, p1, p2, p3] = seg.p;
    (p2 - p1 * 2.0 + p0) * (6.0 * (1.0 - u)) + (p3 - p2 * 2.0 + p1) * (6.0 * u)
}

/// Largest first-derivative magnitude over the stored segments: a scale for
/// relative tolerances.
fn d1_scale(s: &CubicSpline) -> f32 {
    let mut m = 1e-6f32;
    for seg in s.segments() {
        for k in 0..=8 {
            m = m.max(bezier_d1(seg, k as f32 / 8.0).length());
        }
    }
    m
}

fn assert_close(a: Vec3, b: Vec3, tol: f32, what: &str) {
    assert!(
        (a - b).length() <= tol,
        "{what}: {a:?} vs {b:?} (|diff| = {}, tol = {tol})",
        (a - b).length()
    );
}

/// Brute-force closest point: dense sampling followed by golden-section
/// refinement around the best sample.
fn brute_closest(s: &CubicSpline, p: Vec3) -> (f32, f32) {
    let n = s.segment_count();
    let per_seg = 1000usize;
    let total = n * per_seg;
    let tmax = n as f32;
    let mut best_t = 0.0f32;
    let mut best_d = f32::INFINITY;
    for k in 0..=total {
        let t = tmax * (k as f32 / total as f32);
        let d = (s.point(t) - p).length();
        if d < best_d {
            best_d = d;
            best_t = t;
        }
    }
    let h = 2.0 / per_seg as f32;
    let (mut a, mut b) = ((best_t - h).max(0.0), (best_t + h).min(tmax));
    let g = 0.618_034_f32;
    for _ in 0..60 {
        let c = b - g * (b - a);
        let d = a + g * (b - a);
        if (s.point(c) - p).length() < (s.point(d) - p).length() {
            b = d;
        } else {
            a = c;
        }
    }
    let t = 0.5 * (a + b);
    let d = (s.point(t) - p).length();
    if d < best_d { (t, d) } else { (best_t, best_d) }
}

fn check_closest(s: &CubicSpline, p: Vec3, cp: &ClosestPoint) {
    let n = s.segment_count() as f32;
    assert!(
        cp.t.is_finite() && cp.t >= 0.0 && cp.t <= n,
        "t out of range: {}",
        cp.t
    );
    assert_close(
        cp.point,
        s.point(cp.t),
        1e-4,
        "closest.point == point(closest.t)",
    );
    assert!(
        (cp.distance - (p - cp.point).length()).abs() <= 1e-5,
        "closest.distance {} inconsistent with |p - point| {}",
        cp.distance,
        (p - cp.point).length()
    );
    let (_bt, bd) = brute_closest(s, p);
    assert!(
        (cp.distance - bd).abs() <= 1e-4,
        "closest distance {} vs brute force {} (query {p:?}, t = {})",
        cp.distance,
        bd,
        cp.t
    );
}

/// A gently curving random path roughly along +X (tangent never vanishes).
fn path_strategy() -> impl Strategy<Value = Vec<Vec3>> {
    prop::collection::vec((0.05f32..0.3, -0.15f32..0.15, -0.08f32..0.08), 2..9).prop_map(|steps| {
        let mut pts = Vec::with_capacity(steps.len() + 1);
        let mut cur = Vec3::new(-0.5, 0.2, 0.0);
        pts.push(cur);
        for (dx, dy, dz) in steps {
            cur += Vec3::new(dx, dy, dz);
            pts.push(cur);
        }
        pts
    })
}

/// Arbitrary points in a 1 m box, consecutive points at least 2 cm apart.
fn wild_points_strategy() -> impl Strategy<Value = Vec<Vec3>> {
    prop::collection::vec((-1.0f32..1.0, -1.0f32..1.0, -0.2f32..0.2), 2..8)
        .prop_map(|v| {
            v.into_iter()
                .map(|(x, y, z)| Vec3::new(x, y, z))
                .collect::<Vec<_>>()
        })
        .prop_filter("consecutive points too close", |pts| {
            pts.windows(2).all(|w| (w[0] - w[1]).length() > 0.02)
        })
}

fn semicircle(radius: f32, n: usize) -> Vec<Vec3> {
    (0..n)
        .map(|i| {
            let a = std::f32::consts::PI * i as f32 / (n - 1) as f32;
            Vec3::new(radius * a.cos(), radius * a.sin(), 0.0)
        })
        .collect()
}

// ---------------------------------------------------------------- invariants as functions

fn check_interpolation(s: &CubicSpline, pts: &[Vec3]) {
    assert_eq!(s.segment_count(), pts.len() - 1);
    assert_eq!(s.segments().len(), pts.len() - 1);
    for (i, &q) in pts.iter().enumerate() {
        assert_close(
            s.point(i as f32),
            q,
            1e-5 * (1.0 + q.length()),
            "point(i) == through-point",
        );
    }
}

fn check_bezier_storage(s: &CubicSpline, pts: &[Vec3]) {
    for (i, seg) in s.segments().iter().enumerate() {
        assert_close(
            seg.p[0],
            pts[i],
            1e-5 * (1.0 + pts[i].length()),
            "segment start",
        );
        assert_close(
            seg.p[3],
            pts[i + 1],
            1e-5 * (1.0 + pts[i + 1].length()),
            "segment end",
        );
        for k in 0..=10 {
            let u = k as f32 / 10.0;
            let t = i as f32 + u;
            let tol = 1e-5 * (1.0 + seg.p.iter().map(|v| v.length()).fold(0.0, f32::max));
            assert_close(
                s.point(t),
                bezier_eval(seg, u),
                tol,
                "point(t) == Bézier eval",
            );
            let sc = d1_scale(s);
            assert_close(
                s.tangent(t),
                bezier_d1(seg, u),
                1e-4 * sc,
                "tangent(t) == Bézier d1",
            );
            assert_close(
                s.second_derivative(t),
                bezier_d2(seg, u),
                1e-3 * sc,
                "second_derivative(t) == Bézier d2",
            );
        }
    }
}

fn check_continuity(s: &CubicSpline) {
    let segs = s.segments();
    let sc = d1_scale(s);
    for i in 0..segs.len().saturating_sub(1) {
        let (a, b) = (&segs[i], &segs[i + 1]);
        // C⁰
        assert_close(a.p[3], b.p[0], 1e-6 * (1.0 + a.p[3].length()), "C0 at knot");
        // C¹
        let (l1, r1) = (bezier_d1(a, 1.0), bezier_d1(b, 0.0));
        let tol1 = 1e-3 * l1.length().max(r1.length()).max(sc);
        assert_close(l1, r1, tol1, "C1 at knot");
        // C²
        let (l2, r2) = (bezier_d2(a, 1.0), bezier_d2(b, 0.0));
        let tol2 = 1e-3 * l2.length().max(r2.length()).max(sc);
        assert_close(l2, r2, tol2, "C2 at knot");
    }
    // Natural end conditions: zero second derivative at both ends.
    let first = &segs[0];
    let last = &segs[segs.len() - 1];
    assert_close(
        bezier_d2(first, 0.0),
        Vec3::ZERO,
        1e-3 * sc,
        "natural start",
    );
    assert_close(bezier_d2(last, 1.0), Vec3::ZERO, 1e-3 * sc, "natural end");
    let n = segs.len() as f32;
    assert_close(
        s.second_derivative(0.0),
        Vec3::ZERO,
        1e-3 * sc,
        "second_derivative(0)",
    );
    assert_close(
        s.second_derivative(n),
        Vec3::ZERO,
        1e-3 * sc,
        "second_derivative(n-1)",
    );
}

// ---------------------------------------------------------------- deterministic tests

#[test]
fn two_points_is_a_straight_line() {
    let a = Vec3::new(0.1, 0.2, 0.0);
    let b = Vec3::new(0.5, -0.1, 0.03);
    let s = CubicSpline::interpolate(&[a, b]).expect("two points are valid");
    assert_eq!(s.segment_count(), 1);
    for k in 0..=10 {
        let t = k as f32 / 10.0;
        assert_close(s.point(t), a.lerp(b, t), 1e-6, "line point");
        assert_close(s.tangent(t), b - a, 1e-5, "line tangent");
        assert_close(s.second_derivative(t), Vec3::ZERO, 1e-5, "line curvature");
    }
}

#[test]
fn collinear_evenly_spaced_points_give_constant_speed() {
    let pts: Vec<Vec3> = (0..6)
        .map(|i| Vec3::new(0.1 * i as f32, 0.0, 0.0))
        .collect();
    let s = CubicSpline::interpolate(&pts).unwrap();
    for k in 0..=50 {
        let t = 5.0 * k as f32 / 50.0;
        assert_close(
            s.point(t),
            Vec3::new(0.1 * t, 0.0, 0.0),
            1e-5,
            "uniform line",
        );
        assert_close(
            s.tangent(t),
            Vec3::new(0.1, 0.0, 0.0),
            1e-4,
            "uniform speed",
        );
    }
}

#[test]
fn semicircle_guide_invariants() {
    // The 180° guide of radius 150 mm used by the verification scenario.
    let pts = semicircle(0.150, 9);
    let s = CubicSpline::interpolate(&pts).unwrap();
    check_interpolation(&s, &pts);
    check_bezier_storage(&s, &pts);
    check_continuity(&s);
    // The interpolant stays close to the true circle between knots.
    for k in 0..=400 {
        let t = 8.0 * k as f32 / 400.0;
        let r = s.point(t).truncate().length();
        assert!((r - 0.150).abs() < 2e-3, "radius {r} at t = {t}");
    }
}

#[test]
fn parameter_is_clamped() {
    let pts = [
        Vec3::ZERO,
        Vec3::new(0.2, 0.1, 0.0),
        Vec3::new(0.4, 0.0, 0.05),
    ];
    let s = CubicSpline::interpolate(&pts).unwrap();
    assert_close(s.point(-3.0), pts[0], 1e-6, "clamp below");
    assert_close(s.point(7.5), pts[2], 1e-6, "clamp above");
    assert_close(s.tangent(-1.0), s.tangent(0.0), 1e-6, "tangent clamp below");
    assert_close(s.tangent(9.0), s.tangent(2.0), 1e-6, "tangent clamp above");
    assert_close(
        s.second_derivative(-1.0),
        s.second_derivative(0.0),
        1e-6,
        "d2 clamp below",
    );
    assert_close(
        s.second_derivative(9.0),
        s.second_derivative(2.0),
        1e-6,
        "d2 clamp above",
    );
}

#[test]
fn derivatives_match_finite_differences() {
    let pts = [
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.2, 0.15, 0.01),
        Vec3::new(0.35, 0.05, 0.03),
        Vec3::new(0.5, 0.3, 0.065),
        Vec3::new(0.6, 0.45, 0.065),
    ];
    let s = CubicSpline::interpolate(&pts).unwrap();
    let h = 1e-3f32;
    for k in 1..80 {
        let t = 4.0 * k as f32 / 80.0;
        if (t - t.round()).abs() < 2.0 * h {
            continue; // skip knots for the second-derivative FD (only C², not C³)
        }
        let fd1 = (s.point(t + h) - s.point(t - h)) / (2.0 * h);
        assert_close(s.tangent(t), fd1, 2e-3, "tangent vs FD");
        let fd2 = (s.tangent(t + h) - s.tangent(t - h)) / (2.0 * h);
        assert_close(s.second_derivative(t), fd2, 2e-2, "d2 vs FD");
    }
}

#[test]
fn non_consecutive_duplicates_are_allowed() {
    // A closed-ish loop returning to its start point is legal.
    let pts = [
        Vec3::ZERO,
        Vec3::new(0.2, 0.0, 0.0),
        Vec3::new(0.2, 0.2, 0.0),
        Vec3::ZERO,
    ];
    let s = CubicSpline::interpolate(&pts).expect("only consecutive duplicates are invalid");
    check_interpolation(&s, &pts);
}

#[test]
fn error_cases() {
    assert!(CubicSpline::interpolate(&[]).is_err(), "zero points");
    assert!(CubicSpline::interpolate(&[Vec3::ONE]).is_err(), "one point");
    assert!(
        CubicSpline::interpolate(&[Vec3::ZERO, Vec3::new(f32::NAN, 0.0, 0.0)]).is_err(),
        "NaN"
    );
    assert!(
        CubicSpline::interpolate(&[Vec3::ZERO, Vec3::new(0.0, f32::INFINITY, 0.0)]).is_err(),
        "+inf"
    );
    assert!(
        CubicSpline::interpolate(&[Vec3::new(0.0, 0.0, f32::NEG_INFINITY), Vec3::ONE]).is_err(),
        "-inf"
    );
    assert!(
        CubicSpline::interpolate(&[Vec3::ZERO, Vec3::ONE, Vec3::ONE, Vec3::X]).is_err(),
        "duplicate consecutive points"
    );
    assert!(
        CubicSpline::interpolate(&[Vec3::ONE, Vec3::ONE]).is_err(),
        "two identical points"
    );
}

#[test]
fn closest_point_on_semicircle_matches_brute_force() {
    let s = CubicSpline::interpolate(&semicircle(0.150, 9)).unwrap();
    for i in 0..60 {
        let a = -0.3 + 3.6 * i as f32 / 59.0; // includes points beyond both ends
        for &r in &[0.0f32, 0.05, 0.12, 0.149, 0.151, 0.2, 0.3] {
            for &z in &[-0.05f32, 0.0, 0.03] {
                let p = Vec3::new(r * a.cos(), r * a.sin(), z);
                // Every query is checked (the brute-force reference in
                // `check_closest` is independent of the value under test).
                let cp = s.closest_point(p, None);
                check_closest(&s, p, &cp);
            }
        }
    }
}

#[test]
fn closest_point_of_on_curve_point_is_itself() {
    let s = CubicSpline::interpolate(&semicircle(0.150, 7)).unwrap();
    for k in 0..=120 {
        let t = 6.0 * k as f32 / 120.0;
        let p = s.point(t);
        let cp = s.closest_point(p, None);
        assert!(cp.distance <= 1e-4, "distance {} at t = {t}", cp.distance);
        assert!((cp.t - t).abs() <= 1e-2, "t {} vs {t}", cp.t);
        let cps = s.closest_point(p, Some(t));
        assert!(cps.distance <= 1e-4);
    }
}

#[test]
fn seeded_tracking_follows_a_moving_ball() {
    // Ball sweeps along the guide 10 mm inside the curve at 8 m/s, 1 kHz; each
    // tick seeds from the previous parameter, like the solver does.
    let s = CubicSpline::interpolate(&semicircle(0.150, 9)).unwrap();
    let mut seed: Option<f32> = None;
    let steps = (std::f32::consts::PI * 0.140 / 0.008) as usize;
    for k in 0..=steps {
        let a = k as f32 * 0.008 / 0.140;
        let p = Vec3::new(0.140 * a.cos(), 0.140 * a.sin(), 0.0);
        let seeded = s.closest_point(p, seed);
        let fresh = s.closest_point(p, None);
        assert!(
            (seeded.distance - fresh.distance).abs() <= 1e-4,
            "seeded {} vs unseeded {} at tick {k}",
            seeded.distance,
            fresh.distance
        );
        assert!(
            (seeded.distance - 0.010).abs() < 2e-3,
            "distance {}",
            seeded.distance
        );
        if let Some(prev) = seed {
            assert!(
                seeded.t >= prev - 1e-4,
                "parameter went backwards: {prev} -> {}",
                seeded.t
            );
        }
        seed = Some(seeded.t);
    }
}

#[test]
fn bad_seed_still_finds_global_minimum() {
    // An S-curve where a seed on the wrong lobe has a nearby local minimum.
    let pts = [
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.1, 0.1, 0.0),
        Vec3::new(0.0, 0.2, 0.0),
        Vec3::new(-0.1, 0.3, 0.0),
        Vec3::new(0.0, 0.4, 0.0),
    ];
    let s = CubicSpline::interpolate(&pts).unwrap();
    let p = Vec3::new(0.12, 0.1, 0.0);
    for &seed in &[0.0f32, 1.0, 2.0, 3.0, 3.9, 4.0, -5.0, 100.0] {
        let cp = s.closest_point(p, Some(seed));
        check_closest(&s, p, &cp);
    }
}

#[test]
fn non_finite_seed_is_handled() {
    let s = CubicSpline::interpolate(&semicircle(0.150, 5)).unwrap();
    let p = Vec3::new(0.0, 0.16, 0.0);
    for seed in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let cp = s.closest_point(p, Some(seed));
        check_closest(&s, p, &cp);
    }
}

// ---------------------------------------------------------------- property tests

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, .. ProptestConfig::default() })]

    #[test]
    fn prop_interpolates_through_points(pts in wild_points_strategy()) {
        let s = CubicSpline::interpolate(&pts).unwrap();
        check_interpolation(&s, &pts);
    }

    #[test]
    fn prop_bezier_storage_consistent(pts in wild_points_strategy()) {
        let s = CubicSpline::interpolate(&pts).unwrap();
        check_bezier_storage(&s, &pts);
    }

    #[test]
    fn prop_c2_at_knots_and_natural_ends(pts in wild_points_strategy()) {
        let s = CubicSpline::interpolate(&pts).unwrap();
        check_continuity(&s);
        // Also through the evaluation API, approaching each knot from both sides.
        let sc = d1_scale(&s);
        for i in 1..s.segment_count() {
            let t = i as f32;
            let e = 1e-4f32;
            assert_close(s.tangent(t - e), s.tangent(t + e), 1e-2 * sc, "C1 via tangent()");
            assert_close(
                s.second_derivative(t - e),
                s.second_derivative(t + e),
                1e-2 * sc,
                "C2 via second_derivative()",
            );
        }
    }

    #[test]
    fn prop_closest_point_matches_brute_force(
        pts in path_strategy(),
        t_frac in 0.0f32..1.0,
        dir in (-1.0f32..1.0, -1.0f32..1.0, -1.0f32..1.0),
        r in 0.0f32..0.2,
    ) {
        let s = CubicSpline::interpolate(&pts).unwrap();
        let n = s.segment_count() as f32;
        let t0 = t_frac * n;
        let d = Vec3::new(dir.0, dir.1, dir.2);
        let off = if d.length() > 1e-3 { d.normalize() * r } else { Vec3::ZERO };
        let p = s.point(t0) + off;
        let cp = s.closest_point(p, None);
        check_closest(&s, p, &cp);
    }

    #[test]
    fn prop_closest_point_wild_curves(
        pts in wild_points_strategy(),
        t_frac in 0.0f32..1.0,
        dir in (-1.0f32..1.0, -1.0f32..1.0, -1.0f32..1.0),
        r in 0.0f32..0.2,
    ) {
        let s = CubicSpline::interpolate(&pts).unwrap();
        let t0 = t_frac * s.segment_count() as f32;
        let d = Vec3::new(dir.0, dir.1, dir.2);
        let off = if d.length() > 1e-3 { d.normalize() * r } else { Vec3::ZERO };
        let p = s.point(t0) + off;
        let cp = s.closest_point(p, None);
        check_closest(&s, p, &cp);
    }

    #[test]
    fn prop_seeded_agrees_with_unseeded(
        pts in path_strategy(),
        t_frac in 0.0f32..1.0,
        seed_jitter in -0.05f32..0.05,
        dir in (-1.0f32..1.0, -1.0f32..1.0, -1.0f32..1.0),
        r in 0.0f32..0.2,
    ) {
        let s = CubicSpline::interpolate(&pts).unwrap();
        let n = s.segment_count() as f32;
        let d = Vec3::new(dir.0, dir.1, dir.2);
        let off = if d.length() > 1e-3 { d.normalize() * r } else { Vec3::ZERO };
        let p = s.point(t_frac * n) + off;
        let fresh = s.closest_point(p, None);
        let seeded = s.closest_point(p, Some(fresh.t + seed_jitter));
        prop_assert!(
            (seeded.distance - fresh.distance).abs() <= 1e-4,
            "seeded {} vs unseeded {}", seeded.distance, fresh.distance
        );
        // Any seed at all must still give a minimum that matches brute force.
        let any = s.closest_point(p, Some(t_frac * n * 0.5 + 0.3));
        check_closest(&s, p, &any);
    }

    #[test]
    fn prop_evaluation_is_finite_and_clamped(pts in wild_points_strategy(), t in -10.0f32..20.0) {
        let s = CubicSpline::interpolate(&pts).unwrap();
        let n = s.segment_count() as f32;
        let tc = t.clamp(0.0, n);
        prop_assert!(s.point(t).is_finite());
        prop_assert!(s.tangent(t).is_finite());
        prop_assert!(s.second_derivative(t).is_finite());
        prop_assert!((s.point(t) - s.point(tc)).length() <= 1e-6);
    }

    #[test]
    fn prop_rejects_non_finite(mut pts in wild_points_strategy(), idx in 0usize..8, comp in 0usize..3, which in 0usize..3) {
        let i = idx % pts.len();
        let bad = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY][which];
        pts[i][comp] = bad;
        prop_assert!(CubicSpline::interpolate(&pts).is_err());
    }

    #[test]
    fn prop_rejects_consecutive_duplicate(pts in wild_points_strategy(), idx in 0usize..8) {
        let i = idx % pts.len();
        let mut v = pts.clone();
        v.insert(i, pts[i]);
        prop_assert!(CubicSpline::interpolate(&v).is_err());
    }
}
