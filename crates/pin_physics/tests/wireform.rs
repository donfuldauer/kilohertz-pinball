//! Implementer tests for `Wireform`, `Frame` and `GuideCurve` (Subphase 2.1).

use glam::Vec3;
use pin_physics::world::{CubicSpline, Frame, GuideCurve, SplineError, Wireform};
use proptest::prelude::*;

fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

fn helix(turns: f32, points: usize) -> CubicSpline {
    // Radius 100 mm, pitch 60 mm per turn.
    let pts: Vec<Vec3> = (0..points)
        .map(|i| {
            let a = turns * std::f32::consts::TAU * i as f32 / (points - 1) as f32;
            v(0.1 * libm::cosf(a), 0.1 * libm::sinf(a), 0.06 * a / std::f32::consts::TAU)
        })
        .collect();
    CubicSpline::interpolate(&pts).unwrap()
}

fn planar_s_curve() -> CubicSpline {
    let pts: Vec<Vec3> = (0..=16)
        .map(|i| {
            let x = i as f32 * 0.04;
            v(x, 0.08 * libm::sinf(9.0 * x), 0.0)
        })
        .collect();
    CubicSpline::interpolate(&pts).unwrap()
}

fn assert_orthonormal(f: &Frame) {
    for a in [f.tangent, f.normal, f.binormal] {
        assert!((a.length() - 1.0).abs() < 1e-5, "{f:?}");
    }
    assert!(f.tangent.dot(f.normal).abs() < 1e-5, "{f:?}");
    assert!(f.tangent.dot(f.binormal).abs() < 1e-5, "{f:?}");
    assert!(f.normal.dot(f.binormal).abs() < 1e-5, "{f:?}");
    assert!((f.tangent.cross(f.normal) - f.binormal).length() < 1e-5, "{f:?}");
}

#[test]
fn construction_errors() {
    let s = planar_s_curve();
    let n = s.segment_count() + 1;
    let ok = vec![0.0; n];
    assert_eq!(
        Wireform::new(s.clone(), &ok[1..], 0.02, Vec3::Z).unwrap_err(),
        SplineError::RollCountMismatch
    );
    let mut bad = ok.clone();
    bad[3] = f32::NAN;
    assert_eq!(
        Wireform::new(s.clone(), &bad, 0.02, Vec3::Z).unwrap_err(),
        SplineError::NonFinite
    );
    assert_eq!(
        Wireform::new(s.clone(), &ok, f32::INFINITY, Vec3::Z).unwrap_err(),
        SplineError::NonFinite
    );
    assert_eq!(
        Wireform::new(s.clone(), &ok, 0.02, v(0.0, f32::NAN, 1.0)).unwrap_err(),
        SplineError::NonFinite
    );
    for gauge in [0.0, -0.02] {
        assert_eq!(
            Wireform::new(s.clone(), &ok, gauge, Vec3::Z).unwrap_err(),
            SplineError::InvalidGauge
        );
    }
    assert_eq!(
        Wireform::new(s.clone(), &ok, 0.02, Vec3::ZERO).unwrap_err(),
        SplineError::InvalidUpHint
    );
    let w = Wireform::new(s.clone(), &ok, 0.02, Vec3::Z).unwrap();
    assert_eq!(w.gauge(), 0.02);
    assert_eq!(w.centerline().segment_count(), s.segment_count());

    assert_eq!(
        GuideCurve::new(s.clone(), -0.01).unwrap_err(),
        SplineError::InvalidHeight
    );
    assert_eq!(
        GuideCurve::new(s.clone(), f32::NAN).unwrap_err(),
        SplineError::NonFinite
    );
    let g = GuideCurve::new(s.clone(), 0.03).unwrap();
    assert_eq!(g.height(), 0.03);
    assert_eq!(g.spline().segment_count(), s.segment_count());
}

#[test]
fn planar_curve_zero_roll_keeps_rails_in_plane() {
    let s = planar_s_curve();
    let w = Wireform::new(s.clone(), &vec![0.0; 17], 0.024, Vec3::Z).unwrap();
    for k in 0..=320 {
        let t = k as f32 * 0.05;
        let f = w.frame_at(t);
        assert_orthonormal(&f);
        assert!((f.normal - Vec3::Z).length() < 1e-5, "normal at {t}: {:?}", f.normal);
        assert!((f.tangent - s.tangent(t).normalize()).length() < 1e-5);
        let (a, b) = w.rail_points(t);
        assert!(a.z.abs() < 1e-6 && b.z.abs() < 1e-6);
        assert!((a.distance(b) - 0.024).abs() < 1e-6);
        assert!(((a + b) * 0.5 - s.point(t)).length() < 1e-6);
        assert!((a - b).dot(f.tangent).abs() < 1e-6);
        assert!(((a - b).normalize() - f.binormal).length() < 1e-4);
    }
    // Clamped outside the range.
    assert_eq!(w.frame_at(-5.0), w.frame_at(0.0));
    assert_eq!(w.rail_points(99.0), w.rail_points(16.0));
}

#[test]
fn up_hint_is_projected_and_has_a_fallback() {
    let s = CubicSpline::interpolate(&[v(0.0, 0.0, 0.0), v(0.1, 0.0, 0.0), v(0.2, 0.0, 0.02)]).unwrap();
    // Tilted hint: projected perpendicular to the tangent.
    let w = Wireform::new(s.clone(), &[0.0; 3], 0.02, v(5.0, 0.0, 5.0)).unwrap();
    let f = w.frame_at(0.0);
    assert_orthonormal(&f);
    assert!(f.normal.z > 0.9 && f.normal.y.abs() < 1e-5);
    // Hint parallel to the tangent: deterministic fallback, still orthonormal.
    let hint = s.tangent(0.0);
    let w = Wireform::new(s.clone(), &[0.0; 3], 0.02, hint).unwrap();
    let w2 = Wireform::new(s, &[0.0; 3], 0.02, -hint * 3.0).unwrap();
    for k in 0..=20 {
        let t = k as f32 * 0.1;
        assert_orthonormal(&w.frame_at(t));
        assert_eq!(w.frame_at(t), w2.frame_at(t));
    }
}

#[test]
fn rotation_minimizing_frame_has_low_twist() {
    let s = helix(2.0, 49);
    let w = Wireform::new(s.clone(), &vec![0.0; 49], 0.02, Vec3::Z).unwrap();
    // Twist rate = (dn/dt · b): zero for a rotation-minimizing frame. Compare
    // it with the Frenet frame's twist (the torsion), which is large here.
    let h = 1.0e-2f32;
    let mut max_twist = 0.0f32;
    let mut max_turn = 0.0f32;
    for k in 2..470 {
        let t = k as f32 * 0.1 + 0.05;
        let f = w.frame_at(t);
        assert_orthonormal(&f);
        let dn = (w.frame_at(t + h).normal - w.frame_at(t - h).normal) / (2.0 * h);
        max_twist = max_twist.max(dn.dot(f.binormal).abs());
        max_turn = max_turn.max(dn.length());
    }
    assert!(max_turn > 0.05, "the frame does turn: {max_turn}");
    assert!(max_twist < 2e-3 * max_turn.max(1.0), "twist {max_twist} vs turn {max_turn}");

    // Total rotation relative to the Frenet frame equals the integrated
    // torsion: for this helix, -2π·turns·sin(pitch angle).
    let frenet_angle = |t: f32| {
        let f = w.frame_at(t);
        let d1 = s.tangent(t);
        let d2 = s.second_derivative(t);
        // Principal normal.
        let n = (d2 - d1 * (d2.dot(d1) / d1.length_squared())).normalize();
        let b = f.tangent.cross(n);
        libm::atan2f(f.normal.dot(b), f.normal.dot(n))
    };
    // Measure between t = 6 and t = 42 (1.5 turns), unwrapping the angle.
    let mut total = 0.0f32;
    let mut prev = frenet_angle(6.0);
    for k in 1..=360 {
        let a = frenet_angle(6.0 + k as f32 * 0.1);
        let mut d = a - prev;
        if d > std::f32::consts::PI {
            d -= std::f32::consts::TAU;
        } else if d < -std::f32::consts::PI {
            d += std::f32::consts::TAU;
        }
        total += d;
        prev = a;
    }
    let circumference = std::f32::consts::TAU * 0.1;
    let sin_pitch = 0.06 / (circumference * circumference + 0.06 * 0.06).sqrt();
    let expected = -1.5 * std::f32::consts::TAU * sin_pitch;
    assert!((total - expected).abs() < 0.01, "{total} vs {expected}");
}

#[test]
fn frame_is_smooth_across_samples_and_knots() {
    let s = helix(1.5, 25);
    let roll: Vec<f32> = (0..25).map(|i| 3.0 * i as f32).collect();
    let w = Wireform::new(s, &roll, 0.02, Vec3::Z).unwrap();
    // Rail velocity (finite difference) must not jump anywhere.
    let h = 1.0 / 256.0f32;
    let mut prev: Option<Vec3> = None;
    let mut speed = 0.0f32;
    for k in 0..(24 * 256) {
        let t = k as f32 * h;
        let (a0, _) = w.rail_points(t);
        let (a1, _) = w.rail_points(t + h);
        let vel = (a1 - a0) / h;
        if let Some(p) = prev {
            assert!((vel - p).length() < 0.02 * speed.max(0.01), "kink at t = {t}");
        }
        speed = vel.length();
        prev = Some(vel);
    }
}

#[test]
fn roll_rotates_about_the_tangent() {
    let s = CubicSpline::interpolate(&[
        v(0.0, 0.0, 0.0),
        v(0.1, 0.0, 0.0),
        v(0.2, 0.0, 0.0),
        v(0.3, 0.0, 0.0),
    ])
    .unwrap();
    let w = Wireform::new(s, &[0.0, 30.0, 60.0, 90.0], 0.02, Vec3::Z).unwrap();
    for (i, deg) in [0.0f32, 30.0, 60.0, 90.0].iter().enumerate() {
        let t = i as f32;
        assert!((w.roll_rad(t) - deg.to_radians()).abs() < 1e-6);
        let f = w.frame_at(t);
        assert_orthonormal(&f);
        assert!((f.tangent - Vec3::X).length() < 1e-6);
        // Unrolled frame: normal +Z, binormal X × Z = -Y. Right-handed roll
        // about +X turns +Z toward -Y.
        let (sin, cos) = (libm::sinf(deg.to_radians()), libm::cosf(deg.to_radians()));
        assert!((f.normal - v(0.0, -sin, cos)).length() < 1e-5, "{:?}", f.normal);
        assert!((f.binormal - v(0.0, -cos, -sin)).length() < 1e-5);
    }
    // Linear data stays linear under the natural spline.
    assert!((w.roll_rad(1.5) - 45.0f32.to_radians()).abs() < 1e-5);
    // Full turn at 90°: rails are stacked vertically.
    let (a, b) = w.rail_points(3.0);
    assert!((a - b - v(0.0, 0.0, -0.02)).length() < 1e-6);
}

#[test]
fn closest_on_rails_matches_brute_force() {
    let s = helix(1.0, 25);
    let roll: Vec<f32> = (0..25).map(|i| 10.0 * libm::sinf(i as f32 * 0.4)).collect();
    let w = Wireform::new(s, &roll, 0.02, Vec3::Z).unwrap();
    let per = 2000;
    for k in 0..40 {
        let t = 1.0 + 22.0 * k as f32 / 40.0;
        let f = w.frame_at(t);
        // A ball sitting on the rails, slightly off-centre.
        let p = w.centerline().point(t)
            + f.normal * 0.009
            + f.binormal * (0.002 * libm::sinf(k as f32))
            + f.tangent * 0.001;
        let seeds = [None, Some(t), Some(t - 0.4)];
        for seed in seeds {
            let found = w.closest_on_rails(p, seed);
            for (side, cp) in found.iter().enumerate() {
                let rail = |u: f32| {
                    let (a, b) = w.rail_points(u);
                    if side == 0 { a } else { b }
                };
                let mut best = f32::INFINITY;
                for j in 0..=(2 * per) {
                    let u = t - 1.0 + j as f32 / per as f32;
                    best = best.min(rail(u).distance(p));
                }
                assert!((cp.distance - best).abs() < 2e-6, "{} vs {best}", cp.distance);
                assert!((rail(cp.t) - cp.point).length() < 1e-6);
                assert!((cp.point.distance(p) - cp.distance).abs() < 1e-6);
            }
            // The + rail comes first.
            assert!((found[0].point - found[1].point).dot(f.binormal) > 0.0);
        }
    }
    // Beyond the end: clamps.
    let end = w.centerline().max_t();
    let p = w.centerline().point(end) + w.frame_at(end).tangent * 0.02;
    let found = w.closest_on_rails(p, None);
    assert_eq!(found[0].t, end);
    assert_eq!(found[1].t, end);
}

#[test]
fn guide_closest_point_on_the_wall_ribbon() {
    let g = GuideCurve::new(planar_s_curve(), 0.03).unwrap();
    let s = g.spline().clone();
    for k in 0..=64 {
        let t = k as f32 * 0.25;
        let c = s.point(t);
        let tan = s.tangent(t).normalize();
        let side = v(-tan.y, tan.x, 0.0);
        // Beside the wall at mid height: closest point is level with the query.
        let p = c + side * 0.01 + Vec3::Z * 0.012;
        let cp = g.closest_point(p, Some(t));
        assert!((cp.distance - 0.01).abs() < 2e-6, "{}", cp.distance);
        assert!((cp.point.z - 0.012).abs() < 1e-6);
        assert!((cp.t - t).abs() < 1e-3);
        // Above the wall: closest point is on the top edge.
        let p = c + side * 0.003 + Vec3::Z * 0.034;
        let cp = g.closest_point(p, None);
        assert!((cp.distance - 0.005).abs() < 2e-6);
        assert!((cp.point.z - 0.03).abs() < 1e-6);
        // Below the wall: bottom edge.
        let p = c + side * 0.003 - Vec3::Z * 0.004;
        let cp = g.closest_point(p, None);
        assert!((cp.distance - 0.005).abs() < 2e-6);
        assert!(cp.point.z.abs() < 1e-6);
    }
    let cp = g.closest_point(v(f32::NAN, 0.0, 0.0), None);
    assert!(cp.distance.is_infinite());
}

#[test]
fn guide_closest_point_on_a_rising_curve_matches_brute_force() {
    let g = GuideCurve::new(helix(0.5, 13), 0.03).unwrap();
    let s = g.spline().clone();
    for k in 0..30 {
        let t = 0.5 + 11.0 * k as f32 / 30.0;
        let c = s.point(t);
        let tan = s.tangent(t).normalize();
        let side = v(-tan.y, tan.x, 0.0).normalize();
        for p in [
            c + side * 0.012 + Vec3::Z * 0.015,
            c - side * 0.007 + Vec3::Z * 0.04,
            c + side * 0.007 - Vec3::Z * 0.01,
        ] {
            let cp = g.closest_point(p, None);
            // Brute force over (t, u).
            let mut best = f32::INFINITY;
            for j in 0..=4000 {
                let q = s.point(t - 1.0 + j as f32 / 2000.0);
                let u = (p.z - q.z).clamp(0.0, 0.03);
                best = best.min((q + Vec3::Z * u).distance(p));
            }
            assert!((cp.distance - best).abs() < 5e-6, "{} vs {best}", cp.distance);
        }
    }
}

#[test]
fn guide_cast_hits_without_tunnelling() {
    // Straight wall along +X at y = 0, 30 mm high.
    let s = CubicSpline::interpolate(&[v(0.0, 0.0, 0.0), v(0.5, 0.0, 0.0), v(1.0, 0.0, 0.0)]).unwrap();
    let g = GuideCurve::new(s, 0.03).unwrap();
    let r = 0.0135;
    let start = v(0.5, 0.2, 0.0135);
    for speed in [0.5f32, 8.0, 100.0, 5000.0] {
        let vel = v(0.0, -speed, 0.0);
        let hit = g.cast_sphere(start, r, vel, 1.0, None).unwrap();
        let travelled = hit.time * speed;
        // Stops within the 10 µm skin before the wall, never beyond.
        assert!(travelled <= 0.2 - r + 1e-6 && travelled > 0.2 - r - 2e-5, "speed {speed}: {travelled}");
        assert!((hit.normal - Vec3::Y).length() < 1e-4);
        assert!((hit.point - v(0.5, 0.0, 0.0135)).length() < 1e-4);
        assert!((hit.t - 1.0).abs() < 1e-3);
        // Oblique approach from the other side.
        let vel = v(0.6, 0.8, 0.0) * speed;
        let hit = g.cast_sphere(v(0.2, -0.2, 0.0135), r, vel, 1.0, Some(0.3)).unwrap();
        let expected = (0.2 - r) / (0.8 * speed);
        assert!(hit.time <= expected * (1.0 + 1e-5) && hit.time > expected - 3e-5 / speed);
        assert!((hit.normal + Vec3::Y).length() < 1e-3);
        // Out of time.
        assert!(g.cast_sphere(start, r, v(0.0, -speed, 0.0), 0.1 / speed, None).is_none());
    }
    // Moving away, parallel, or not moving: no hit.
    assert!(g.cast_sphere(start, r, v(0.0, 1.0, 0.0), 1.0, None).is_none());
    assert!(g.cast_sphere(start, r, v(1.0, 0.0, 0.0), 0.3, None).is_none());
    assert!(g.cast_sphere(start, r, Vec3::ZERO, 1.0, None).is_none());
    assert!(g.cast_sphere(start, r, v(0.0, -1.0, 0.0), f32::NAN, None).is_none());
    assert!(g.cast_sphere(v(f32::NAN, 0.0, 0.0), r, v(0.0, -1.0, 0.0), 1.0, None).is_none());
    // Passing over the top of the wall.
    assert!(g.cast_sphere(v(0.5, 0.2, 0.03 + r + 0.001), r, v(0.0, -1.0, 0.0), 0.4, None).is_none());
    // Touching: hit at time 0 only when pressing in.
    let touching = v(0.5, r, 0.0135);
    let hit = g.cast_sphere(touching, r, v(1.0, -0.1, 0.0), 1.0, None).unwrap();
    assert_eq!(hit.time, 0.0);
    assert!(g.cast_sphere(touching, r, v(1.0, 0.0, 0.0), 0.1, None).is_none());
    assert!(g.cast_sphere(touching, r, v(1.0, 0.1, 0.0), 1.0, None).is_none());
    // Centre exactly on the wall: the fallback normal is the curve normal.
    let hit = g.cast_sphere(v(0.5, 0.0, 0.01), r, v(0.0, -1.0, 0.0), 1.0, None).unwrap();
    assert_eq!(hit.time, 0.0);
    assert!((hit.normal.length() - 1.0).abs() < 1e-6 && hit.normal.z.abs() < 1e-6);
}

#[test]
fn guide_cast_grazing_is_conservative() {
    // Sliding along the inside of an arc just outside the skin: the cast
    // reports a safe (non-penetrating) time.
    let r_wall = 0.15f32;
    let pts: Vec<Vec3> = (0..=36)
        .map(|i| {
            let a = (i as f32 * 5.0).to_radians();
            v(r_wall * libm::cosf(a), r_wall * libm::sinf(a), 0.0)
        })
        .collect();
    let g = GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), 0.03).unwrap();
    let r = 0.0135;
    let a = 60.0f32.to_radians();
    let radial = v(libm::cosf(a), libm::sinf(a), 0.0);
    let start = radial * (r_wall - r - 2e-5) + Vec3::Z * 0.0135;
    let vel = v(-radial.y, radial.x, 0.0) * 8.0;
    let hit = g.cast_sphere(start, r, vel, 1.0e-3, None).unwrap();
    let at = start + vel * hit.time;
    let gap = g.closest_point(at, Some(hit.t)).distance - r;
    assert!(gap >= -1e-6 && gap <= 2e-5, "gap {gap}");
    assert!(hit.time > 0.0 && hit.time < 1.0e-3);
    assert!(hit.normal.dot(radial) < -0.99);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn prop_frames_are_orthonormal_and_rails_keep_gauge(
        steps in prop::collection::vec((0.03f32..0.08, -1.0f32..1.0, -1.0f32..1.0, -0.5f32..0.5), 2..10),
        rolls in prop::collection::vec(-60.0f32..60.0, 11),
        gauge in 0.005f32..0.04,
        up in (-1.0f32..1.0, -1.0f32..1.0, 0.2f32..1.0),
        ts in prop::collection::vec(0.0f32..1.0, 16),
    ) {
        let mut p = Vec3::ZERO;
        let mut pts = vec![p];
        let mut dir = Vec3::X;
        for (len, dx, dy, dz) in steps {
            // Bounded turning, so the curve has no cusps.
            dir = (dir + v(dx, dy, dz) * 0.6).normalize();
            p += dir * len;
            pts.push(p);
        }
        let s = CubicSpline::interpolate(&pts).unwrap();
        let w = Wireform::new(s.clone(), &rolls[..pts.len()], gauge, v(up.0, up.1, up.2)).unwrap();
        for u in ts {
            let t = u * s.max_t();
            let f = w.frame_at(t);
            for a in [f.tangent, f.normal, f.binormal] {
                prop_assert!((a.length() - 1.0).abs() < 1e-4);
            }
            prop_assert!(f.tangent.dot(f.normal).abs() < 1e-4);
            prop_assert!(f.normal.dot(f.binormal).abs() < 1e-4);
            prop_assert!((f.tangent.cross(f.normal) - f.binormal).length() < 1e-4);
            prop_assert!((f.tangent - s.tangent(t).normalize()).length() < 1e-4);
            let (a, b) = w.rail_points(t);
            prop_assert!((a.distance(b) - gauge).abs() < 1e-5);
            prop_assert!(((a + b) * 0.5 - s.point(t)).length() < 1e-5);
            prop_assert!(((a - b) / gauge - f.binormal).length() < 1e-3);
        }
        // Through-point roll angles are honoured.
        for (i, deg) in rolls[..pts.len()].iter().enumerate() {
            prop_assert!((w.roll_rad(i as f32) - deg.to_radians()).abs() < 1e-5);
        }
    }
}
