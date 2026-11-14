//! Fix task 2.1-W: precision of the wireform's rotation-minimizing frame.
//!
//! A planar centerline with zero roll must keep both rails in its plane. The
//! frame used to be built from the f32 sample positions; over the short chord
//! of a near-cusp (a natural spline that overshoots backwards, then turns
//! round within one sample step) their rounding tilted the frame by ~1e-3
//! rad, which the double reflection could not cancel. These tests pin the
//! regression and hunt such inputs.

use glam::Vec3;
use pin_physics::world::{CubicSpline, Wireform};
use proptest::prelude::*;

/// Gauge used throughout (metres).
const GAUGE: f32 = 0.03;

/// Polyline from `start` by planar steps `(along_a, along_b)` in the plane
/// spanned by `a` and `b`.
fn walk(start: Vec3, a: Vec3, b: Vec3, steps: &[(f32, f32)]) -> Vec<Vec3> {
    let mut pts = vec![start];
    for &(da, db) in steps {
        let last = *pts.last().unwrap();
        pts.push(last + a * da + b * db);
    }
    pts
}

/// Largest distance of either rail from the plane `axis = value`, sampled
/// densely (every 1/64 of a segment) and at the given extra fractions.
fn max_out_of_plane(w: &Wireform, axis: usize, value: f32, extra: &[f32]) -> f32 {
    let n = w.centerline().segment_count();
    let mut worst = 0.0f32;
    let mut check = |t: f32| {
        let (a, b) = w.rail_points(t);
        let f = w.frame_at(t);
        assert!(f.normal.is_finite() && f.binormal.is_finite(), "t = {t}");
        worst = worst
            .max((a[axis] - value).abs())
            .max((b[axis] - value).abs());
    };
    for k in 0..=n * 64 {
        check(k as f32 / 64.0);
    }
    for &u in extra {
        check(u * n as f32);
    }
    worst
}

/// The minimal input found by the Wave 4 gate CI (seed `cc ab9f17fb…`).
#[test]
fn regression_ci_near_cusp_planar_path_keeps_rails_in_plane() {
    let steps = [
        (0.062_586_926, -0.024_000_647),
        (0.294_962_8, -0.082_802_04),
        (0.233_436_8, 0.050_341_614),
        (0.284_535_17, 0.005_840_165_5),
    ];
    let z0 = -0.036_043_34_f32;
    let pts = walk(Vec3::new(0.0, 0.0, z0), Vec3::X, Vec3::Y, &steps);
    let s = CubicSpline::interpolate(&pts).unwrap();
    // The path really has the near-cusp: it starts out backwards.
    assert!(s.tangent(0.0).x < 0.0 && s.tangent(0.25).x > 0.0);
    let w = Wireform::new(s, &[0.0; 5], GAUGE, Vec3::Z).unwrap();
    // Before the fix: 1.3657e-5 m at every t > 0.
    let worst = max_out_of_plane(&w, 2, z0, &[0.282_189_04]);
    assert!(worst < 1.0e-7, "rails leave the plane by {worst} m");
    // The frame itself stays exactly vertical.
    for k in 0..=64 {
        let f = w.frame_at(k as f32 / 16.0);
        assert!(f.binormal.z.abs() < 1.0e-7, "binormal {:?}", f.binormal);
    }
}

/// An exact cusp: the centerline runs out and straight back, so its speed
/// is zero at the turning knot and the tangent reverses (the second mirror
/// of the double reflection degenerates to `v2 = 0`).
#[test]
fn exact_cusp_keeps_frame_continuous_and_in_plane() {
    let z0 = 0.05;
    let pts = [
        Vec3::new(0.0, 0.0, z0),
        Vec3::new(0.2, 0.0, z0),
        Vec3::new(0.0, 0.0, z0),
    ];
    let w = Wireform::new(
        CubicSpline::interpolate(&pts).unwrap(),
        &[0.0; 3],
        GAUGE,
        Vec3::Z,
    )
    .unwrap();
    assert!(max_out_of_plane(&w, 2, z0, &[]) < 1.0e-7);
    for k in 0..=32 {
        let f = w.frame_at(k as f32 / 16.0);
        assert!(
            (f.normal - Vec3::Z).length() < 1.0e-6,
            "normal {:?}",
            f.normal
        );
    }
}

/// Paths in a vertical plane (`y = y0`) with the up hint in that plane: the
/// normal stays in the plane and the rails sit at `y0 ± gauge/2`.
#[test]
fn vertical_plane_path_rails_straddle_plane() {
    let y0 = -0.4;
    let steps = [(0.06, 0.03), (0.3, -0.05), (0.2, 0.08), (0.25, 0.0)];
    let pts = walk(Vec3::new(0.1, y0, 0.0), Vec3::X, Vec3::Z, &steps);
    let w = Wireform::new(
        CubicSpline::interpolate(&pts).unwrap(),
        &[0.0; 5],
        GAUGE,
        Vec3::Z,
    )
    .unwrap();
    let n = w.centerline().segment_count();
    for k in 0..=n * 64 {
        let t = k as f32 / 64.0;
        let f = w.frame_at(t);
        assert!(f.normal.y.abs() < 1.0e-6, "t = {t}: normal {:?}", f.normal);
        let (a, b) = w.rail_points(t);
        let spread = (a.y - y0).abs().min((b.y - y0).abs());
        assert!(
            (spread - 0.5 * GAUGE).abs() < 1.0e-6,
            "t = {t}: {a:?} {b:?}"
        );
    }
}

/// Rebuilding gives bit-identical frames (determinism of the f64 build).
#[test]
fn frame_build_is_bit_identical() {
    let pts = walk(
        Vec3::new(0.0, 0.0, 0.01),
        Vec3::X,
        Vec3::Y,
        &[(0.05, 0.1), (0.3, -0.14), (0.07, 0.12), (0.29, 0.0)],
    );
    let build = || {
        Wireform::new(
            CubicSpline::interpolate(&pts).unwrap(),
            &[0.0, 10.0, -20.0, 5.0, 0.0],
            GAUGE,
            Vec3::new(0.1, 0.0, 1.0),
        )
        .unwrap()
    };
    let (w1, w2) = (build(), build());
    for k in 0..=256 {
        let t = k as f32 / 64.0;
        assert_eq!(w1.frame_at(t), w2.frame_at(t));
        assert_eq!(w1.rail_points(t), w2.rail_points(t));
    }
}

/// Steps that make the natural spline overshoot: a short step next to a
/// long, near-reversing one, and near-straight runs (tiny cross steps).
fn hunting_steps() -> impl Strategy<Value = Vec<(f32, f32)>> {
    let step = prop_oneof![
        // The gate's generator.
        (0.05f32..0.3, -0.15f32..0.15),
        // Near-straight: almost no sideways component.
        (0.05f32..0.3, -1.0e-4f32..1.0e-4),
        // Short and long steps (strong overshoot, near-cusps).
        (0.001f32..0.02, -0.01f32..0.01),
        (0.25f32..0.5, -0.02f32..0.02),
        // Backwards steps (the path doubles back).
        (-0.3f32..-0.05, -0.15f32..0.15),
    ];
    prop::collection::vec(step, 2..9)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3000))]

    /// Zero-roll planar paths in a horizontal plane keep their rails in it.
    #[test]
    fn prop_planar_rails_stay_in_horizontal_plane(
        steps in hunting_steps(),
        z0 in -0.2f32..0.2,
        extra in prop::collection::vec(0.0f32..=1.0, 8),
    ) {
        let pts = walk(Vec3::new(0.0, 0.0, z0), Vec3::X, Vec3::Y, &steps);
        prop_assume!(pts.windows(2).all(|p| p[0] != p[1]));
        let s = CubicSpline::interpolate(&pts).unwrap();
        let w = Wireform::new(s, &vec![0.0; pts.len()], GAUGE, Vec3::Z).unwrap();
        let worst = max_out_of_plane(&w, 2, z0, &extra);
        prop_assert!(worst < 1.0e-7, "rails leave the plane by {} m", worst);
    }

    /// The same in the other two axis planes, with a tilted up hint.
    #[test]
    fn prop_planar_rails_stay_in_axis_planes(
        steps in hunting_steps(),
        c0 in -0.5f32..0.5,
        tilt in -0.5f32..0.5,
        x_plane in any::<bool>(),
    ) {
        let (axis, a, b, start, up) = if x_plane {
            (0, Vec3::Y, Vec3::Z, Vec3::new(c0, 0.0, 0.0), Vec3::new(0.0, tilt, 1.0))
        } else {
            (1, Vec3::X, Vec3::Z, Vec3::new(0.0, c0, 0.0), Vec3::new(tilt, 0.0, 1.0))
        };
        let pts = walk(start, a, b, &steps);
        prop_assume!(pts.windows(2).all(|p| p[0] != p[1]));
        let s = CubicSpline::interpolate(&pts).unwrap();
        // The up hint lies in the plane, so the normal starts and stays in
        // it and the rails straddle the plane by half the gauge.
        let w = Wireform::new(s, &vec![0.0; pts.len()], GAUGE, up).unwrap();
        let n = w.centerline().segment_count();
        for k in 0..=n * 32 {
            let t = k as f32 / 32.0;
            let f = w.frame_at(t);
            prop_assert!(f.normal[axis].abs() < 1.0e-6, "t = {}: normal {:?}", t, f.normal);
            let (p, q) = w.rail_points(t);
            let near = (p[axis] - c0).abs().min((q[axis] - c0).abs());
            prop_assert!((near - 0.5 * GAUGE).abs() < 1.0e-6, "t = {}: {:?} {:?}", t, p, q);
        }
    }

    /// Non-planar paths through the same near-cusps: the frame stays
    /// orthonormal, right-handed and finite everywhere.
    #[test]
    fn prop_frame_valid_through_near_cusps(
        steps in hunting_steps(),
        dz in prop::collection::vec(-0.02f32..0.02, 8),
        roll in prop::collection::vec(-60.0f32..60.0, 9),
    ) {
        let mut pts = vec![Vec3::ZERO];
        for (i, &(dx, dy)) in steps.iter().enumerate() {
            let last = *pts.last().unwrap();
            pts.push(last + Vec3::new(dx, dy, dz[i]));
        }
        prop_assume!(pts.windows(2).all(|p| p[0] != p[1]));
        let s = CubicSpline::interpolate(&pts).unwrap();
        let w = Wireform::new(s, &roll[..pts.len()], GAUGE, Vec3::Z).unwrap();
        let n = w.centerline().segment_count();
        for k in 0..=n * 32 {
            let t = k as f32 / 32.0;
            let f = w.frame_at(t);
            for v in [f.tangent, f.normal, f.binormal] {
                prop_assert!((v.length() - 1.0).abs() < 1.0e-4, "t = {}: {:?}", t, f);
            }
            prop_assert!(f.tangent.dot(f.normal).abs() < 1.0e-4, "t = {}: {:?}", t, f);
            prop_assert!((f.tangent.cross(f.normal) - f.binormal).length() < 1.0e-4);
            let (a, b) = w.rail_points(t);
            prop_assert!(((a - b).length() - GAUGE).abs() < 1.0e-5);
        }
    }
}
