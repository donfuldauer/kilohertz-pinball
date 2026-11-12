//! Implementer tests for `Wireform`, `Frame` and `GuideCurve` (Subphase 2.1).

use glam::Vec3;
use pin_physics::world::{
    CubicSpline, Frame, GuideCast, GuideCurve, GuideHit, SplineError, Wireform,
};
use proptest::prelude::*;

fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

fn helix(turns: f32, points: usize) -> CubicSpline {
    // Radius 100 mm, pitch 60 mm per turn.
    let pts: Vec<Vec3> = (0..points)
        .map(|i| {
            let a = turns * std::f32::consts::TAU * i as f32 / (points - 1) as f32;
            v(
                0.1 * libm::cosf(a),
                0.1 * libm::sinf(a),
                0.06 * a / std::f32::consts::TAU,
            )
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
    assert!(
        (f.tangent.cross(f.normal) - f.binormal).length() < 1e-5,
        "{f:?}"
    );
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
    let w = Wireform::new(s.clone(), &[0.0; 17], 0.024, Vec3::Z).unwrap();
    for k in 0..=320 {
        let t = k as f32 * 0.05;
        let f = w.frame_at(t);
        assert_orthonormal(&f);
        assert!(
            (f.normal - Vec3::Z).length() < 1e-5,
            "normal at {t}: {:?}",
            f.normal
        );
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
    let s =
        CubicSpline::interpolate(&[v(0.0, 0.0, 0.0), v(0.1, 0.0, 0.0), v(0.2, 0.0, 0.02)]).unwrap();
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
    let w = Wireform::new(s.clone(), &[0.0; 49], 0.02, Vec3::Z).unwrap();
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
    assert!(
        max_twist < 2e-3 * max_turn.max(1.0),
        "twist {max_twist} vs turn {max_turn}"
    );

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
            assert!(
                (vel - p).length() < 0.02 * speed.max(0.01),
                "kink at t = {t}"
            );
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
        assert!(
            (f.normal - v(0.0, -sin, cos)).length() < 1e-5,
            "{:?}",
            f.normal
        );
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
                assert!(
                    (cp.distance - best).abs() < 2e-6,
                    "{} vs {best}",
                    cp.distance
                );
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
            assert!(
                (cp.distance - best).abs() < 5e-6,
                "{} vs {best}",
                cp.distance
            );
        }
    }
}

const SKIN: f32 = 1.0e-5;
const BALL: f32 = 0.0135;

/// Outcome of following a guide cast to its end, re-casting after every
/// `Advance` exactly as a solver would.
struct Resolved {
    hit: Option<GuideHit>,
    casts: u32,
    steps: u32,
}

fn resolve(g: &GuideCurve, start: Vec3, r: f32, vel: Vec3, max_time: f32) -> Resolved {
    let mut out = Resolved {
        hit: None,
        casts: 0,
        steps: 0,
    };
    let mut elapsed = 0.0f32;
    let seed = None;
    while out.casts < 1000 {
        let from = start + vel * elapsed;
        let (cast, steps) = g.cast_sphere_counted(from, r, vel, max_time - elapsed, seed);
        assert_eq!(cast, g.cast_sphere(from, r, vel, max_time - elapsed, seed));
        assert!(steps <= 32, "{steps} steps in one cast");
        out.casts += 1;
        out.steps += steps;
        match cast {
            None => return out,
            Some(GuideCast::Contact(hit)) => {
                assert!(hit.time >= 0.0 && hit.time <= max_time - elapsed);
                assert!((hit.normal.length() - 1.0).abs() < 1e-5);
                // A contact always closes in.
                assert!(
                    hit.normal.dot(vel) < 0.0,
                    "contact while not approaching: {hit:?}"
                );
                out.hit = Some(GuideHit {
                    time: elapsed + hit.time,
                    ..hit
                });
                return out;
            }
            Some(GuideCast::Advance { time }) => {
                assert!(
                    time > 0.0 && time < max_time - elapsed,
                    "advance made no progress"
                );
                elapsed += time;
            }
        }
    }
    panic!("cast did not finish");
}

fn straight_wall_x() -> GuideCurve {
    // Straight wall along +X at y = 0, 30 mm high.
    let pts: Vec<Vec3> = (0..=20).map(|i| v(0.05 * i as f32, 0.0, 0.0)).collect();
    GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), 0.03).unwrap()
}

fn arc_wall(radius: f32, degrees: usize) -> GuideCurve {
    // 5° spacing around the origin.
    let pts: Vec<Vec3> = (0..=degrees / 5)
        .map(|i| {
            let a = (i as f32 * 5.0).to_radians();
            v(radius * libm::cosf(a), radius * libm::sinf(a), 0.0)
        })
        .collect();
    GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), 0.03).unwrap()
}

#[test]
fn guide_cast_hits_without_tunnelling() {
    let g = straight_wall_x();
    assert!(g.is_level());
    let r = BALL;
    let start = v(0.5, 0.2, 0.0135);
    for speed in [0.5f32, 8.0, 100.0, 5000.0] {
        let vel = v(0.0, -speed, 0.0);
        let res = resolve(&g, start, r, vel, 1.0);
        let hit = res.hit.unwrap();
        assert!(res.steps <= 3, "{} steps", res.steps);
        let travelled = hit.time * speed;
        // Stops within the 10 µm skin before the wall, never beyond.
        assert!(
            travelled <= 0.2 - r + 1e-6 && travelled > 0.2 - r - 2e-5,
            "speed {speed}: {travelled}"
        );
        assert!((hit.normal - Vec3::Y).length() < 1e-4);
        assert!((hit.point - v(0.5, 0.0, 0.0135)).length() < 1e-4);
        assert!((hit.t - 10.0).abs() < 1e-3);
        // Oblique approach from the other side.
        let vel = v(0.6, 0.8, 0.0) * speed;
        let res = resolve(&g, v(0.2, -0.2, 0.0135), r, vel, 1.0);
        let hit = res.hit.unwrap();
        assert!(res.steps <= 4, "{} steps", res.steps);
        let expected = (0.2 - r) / (0.8 * speed);
        assert!(hit.time <= expected * (1.0 + 1e-5) && hit.time > expected - 3e-5 / speed);
        assert!((hit.normal + Vec3::Y).length() < 1e-3);
        // Out of time.
        assert!(
            resolve(&g, start, r, v(0.0, -speed, 0.0), 0.1 / speed)
                .hit
                .is_none()
        );
    }
    // Moving away, parallel, or not moving: no contact, at negligible cost.
    for vel in [v(0.0, 1.0, 0.0), v(1.0, 0.0, 0.0), v(8.0, 0.02, 0.0)] {
        let res = resolve(&g, start, r, vel, 0.03);
        assert!(res.hit.is_none());
        assert!(res.steps <= 3, "{} steps for {vel}", res.steps);
    }
    assert!(g.cast_sphere(start, r, Vec3::ZERO, 1.0, None).is_none());
    assert!(
        g.cast_sphere(start, r, v(0.0, -1.0, 0.0), f32::NAN, None)
            .is_none()
    );
    assert!(
        g.cast_sphere(v(f32::NAN, 0.0, 0.0), r, v(0.0, -1.0, 0.0), 1.0, None)
            .is_none()
    );
    // Passing over the top of the wall.
    let over = v(0.5, 0.2, 0.03 + r + 0.001);
    assert!(resolve(&g, over, r, v(0.0, -1.0, 0.0), 0.4).hit.is_none());
    // Touching: contact at time 0 only when pressing in.
    let touching = v(0.5, r, 0.0135);
    let hit = resolve(&g, touching, r, v(1.0, -0.1, 0.0), 0.3)
        .hit
        .unwrap();
    assert_eq!(hit.time, 0.0);
    assert!(
        resolve(&g, touching, r, v(1.0, 0.0, 0.0), 0.1)
            .hit
            .is_none()
    );
    assert!(
        resolve(&g, touching, r, v(1.0, 0.1, 0.0), 0.3)
            .hit
            .is_none()
    );
    // Overlapping by 1 mm: leaving is free, pressing in is a contact.
    let sunk = v(0.5, r - 0.001, 0.0135);
    assert!(resolve(&g, sunk, r, v(1.0, 0.5, 0.0), 0.1).hit.is_none());
    assert_eq!(
        resolve(&g, sunk, r, v(1.0, -0.5, 0.0), 0.1)
            .hit
            .unwrap()
            .time,
        0.0
    );
    // Centre exactly on the wall: the fallback normal is the curve normal.
    let on_wall = v(0.5, 0.0, 0.01);
    for vel in [v(0.0, -1.0, 0.0), v(0.0, 1.0, 0.0)] {
        if let Some(hit) = resolve(&g, on_wall, r, vel, 0.01).hit {
            assert_eq!(hit.time, 0.0);
            assert!((hit.normal.length() - 1.0).abs() < 1e-6 && hit.normal.z.abs() < 1e-6);
        }
    }
}

/// Reviewer's reproductions (2.1 fix 1): shallow casts next to a straight
/// wall used to report contacts while moving away or parallel.
#[test]
fn guide_cast_reports_no_contact_when_not_approaching() {
    // Straight wall along +Y at x = 0, through-points 50 mm apart.
    let pts: Vec<Vec3> = (0..20).map(|i| v(0.0, 0.05 * i as f32, 0.0)).collect();
    let g = GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), 0.03).unwrap();
    for (gap, vx, vy, dt) in [
        (5.0e-5f32, 0.04f32, 8.0f32, 1.0e-3f32),
        (3.2e-4, 0.75, 39.56, 1.0e-3),
        (1.1e-5, 0.0, 8.0, 1.25e-4),
        (1.1e-5, 0.0, 8.0, 1.0e-3),
        (2.0e-4, 0.02, 8.0, 1.0e-3),
        (5.0e-6, 0.0, 8.0, 1.0e-3),
        (0.0, 0.0, 8.0, 1.0e-3),
    ] {
        let start = v(BALL + gap, 0.2, 0.0135);
        let res = resolve(&g, start, BALL, v(vx, vy, 0.0), dt);
        assert!(
            res.hit.is_none(),
            "gap {gap}, v ({vx}, {vy}): {:?}",
            res.hit
        );
        assert_eq!(res.casts, 1);
        assert!(res.steps <= 2, "{} steps", res.steps);
    }
    // Closing in at the same shallow angle is a contact, at the right time.
    let start = v(BALL + 5.0e-5, 0.2, 0.0135);
    let hit = resolve(&g, start, BALL, v(-0.04, 8.0, 0.0), 2.0e-3)
        .hit
        .unwrap();
    assert!(
        hit.time <= 5.0e-5 / 0.04 * 1.001 && hit.time >= 4.0e-5 / 0.04 * 0.999,
        "{}",
        hit.time
    );
}

#[test]
fn ball_leaving_the_outside_of_an_orbit_arc_is_free_and_cheap() {
    // 0.5 m orbit, 199 segments.
    let pts: Vec<Vec3> = (0..200)
        .map(|i| {
            let a = i as f32 * 0.03;
            v(0.5 * libm::cosf(a), 0.5 * libm::sinf(a), 0.0)
        })
        .collect();
    let g = GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), 0.03).unwrap();
    for k in 0..40 {
        let a = 0.5 + 0.12 * k as f32;
        let radial = v(libm::cosf(a), libm::sinf(a), 0.0);
        let tangent = v(-radial.y, radial.x, 0.0);
        for gap in [0.0f32, 5.0e-6, 5.0e-5, 1.0e-3] {
            // Outside, leaving or running along the wall: never a contact.
            let start = radial * (0.5 + BALL + gap) + Vec3::Z * 0.0135;
            for vel in [
                tangent * 8.0 + radial * 0.02,
                tangent * 8.0,
                -tangent * 3.0 + radial,
            ] {
                let res = resolve(&g, start, BALL, vel, 1.0e-3);
                assert!(
                    res.hit.is_none(),
                    "false contact {:?} (k {k}, gap {gap})",
                    res.hit
                );
                assert!(res.casts == 1 && res.steps <= 3, "{} steps", res.steps);
            }
            // Inside, running along the wall: the wall curves into the path,
            // which is a real contact, found in a few steps, never deeper
            // than the sliding allowance.
            let start = radial * (0.5 - BALL - gap) + Vec3::Z * 0.0135;
            let vel = tangent * 8.0;
            let res = resolve(&g, start, BALL, vel, 4.0e-3);
            let hit = res.hit.expect("the arc must stop a ball sliding inside it");
            assert!(res.casts == 1 && res.steps <= 8, "{} steps", res.steps);
            let at = start + vel * hit.time;
            let found = g.closest_point(at, Some(hit.t)).distance - BALL;
            assert!(
                (-SKIN - 1e-6..=SKIN + 1e-6).contains(&found),
                "gap at contact {found}"
            );
        }
    }
}

#[test]
fn guide_cast_on_a_rising_curve_stays_safe() {
    // Not level: plain conservative advancement, with `Advance` hand-backs
    // when the ball runs alongside.
    let g = GuideCurve::new(helix(0.5, 13), 0.03).unwrap();
    assert!(!g.is_level());
    let s = g.spline().clone();
    let c = s.point(6.0);
    let tan = s.tangent(6.0).normalize();
    let side = v(-tan.y, tan.x, 0.0).normalize();
    // Head-on from 50 mm.
    let start = c + side * 0.05 + Vec3::Z * 0.015;
    let hit = resolve(&g, start, BALL, -side * 4.0, 0.1).hit.unwrap();
    let gap = g
        .closest_point(start - side * 4.0 * hit.time, None)
        .distance
        - BALL;
    assert!((-1e-6..=SKIN + 1e-6).contains(&gap), "gap {gap}");
    // Running alongside at 100 µm: no contact, but the casts hand back.
    let start = c - side * (BALL + 1.0e-4) + Vec3::Z * 0.015;
    let (first, steps) = g.cast_sphere_counted(start, BALL, tan * 8.0, 1.0e-3, None);
    assert_eq!(steps, 32);
    assert!(matches!(first, Some(GuideCast::Advance { time }) if time > 0.0 && time < 1.0e-3));
    let res = resolve(&g, start, BALL, tan * 8.0 - side * 0.05, 1.0e-3);
    assert!(res.hit.is_none());
    assert!(res.casts > 1);
}

#[test]
fn self_crossing_guides_fall_back_to_conservative_advancement() {
    // A figure-of-eight crosses itself: no reach, but still a correct cast.
    let pts: Vec<Vec3> = (0..=40)
        .map(|i| {
            let a = i as f32 * std::f32::consts::TAU / 40.0;
            v(0.2 * libm::sinf(a), 0.1 * libm::sinf(2.0 * a), 0.0)
        })
        .collect();
    let g = GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), 0.03).unwrap();
    let start = v(0.1, 0.3, 0.0135);
    let hit = resolve(&g, start, BALL, v(0.0, -5.0, 0.0), 1.0)
        .hit
        .unwrap();
    let gap = g
        .closest_point(start + v(0.0, -5.0, 0.0) * hit.time, None)
        .distance
        - BALL;
    assert!((-1e-6..=SKIN + 1e-6).contains(&gap), "gap {gap}");
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

proptest! {
    #![proptest_config(ProptestConfig { cases: 400, failure_persistence: None, ..ProptestConfig::default() })]

    /// Straight wall: contact exactly when the ball closes in, at the
    /// analytic time; never while it moves away or along the wall.
    #[test]
    fn prop_guide_cast_straight_wall(
        gap in prop_oneof![0.0f32..2.0e-5, 0.0f32..1.0e-3, 0.0f32..0.1],
        angle in prop_oneof![-0.01f32..0.01, -3.2f32..3.2],
        speed in 0.05f32..40.0,
        along in 0.3f32..0.7,
        travel in 0.001f32..0.2,
    ) {
        let g = straight_wall_x();
        let start = v(along, BALL + gap, 0.0135);
        // `angle` is measured from the wall direction; negative closes in.
        let vel = v(libm::cosf(angle), libm::sinf(angle), 0.0) * speed;
        let max_time = travel / speed;
        let res = resolve(&g, start, BALL, vel, max_time);
        prop_assert!(res.casts <= 2 && res.steps <= 8, "{} casts, {} steps", res.casts, res.steps);
        let closing = -vel.y;
        match res.hit {
            Some(hit) => {
                prop_assert!(closing > 0.0, "contact while moving away: {:?}", hit);
                let gap_at = gap - closing * hit.time;
                prop_assert!(gap_at <= SKIN + 2e-6, "gap at contact {}", gap_at);
                prop_assert!(gap_at >= -2e-6, "contact after the wall: {}", gap_at);
                prop_assert!(hit.time <= gap / closing + 1e-6 / closing);
                prop_assert!((hit.normal - Vec3::Y).length() < 1e-3);
            }
            None => {
                // Free for the whole window: the ball must not have reached
                // the wall (beyond the sliding allowance).
                let gap_end = gap - closing * max_time;
                prop_assert!(gap_end >= -SKIN - 2e-6, "missed contact, end gap {}", gap_end);
                // And a real closing contact inside the window is not missed.
                if closing > 1e-4 * speed && gap > SKIN {
                    prop_assert!(gap_end > -2e-6, "missed contact, end gap {}", gap_end);
                }
            }
        }
    }

    /// Arc wall, ball inside or outside, any direction.
    #[test]
    fn prop_guide_cast_arc_wall(
        inside in any::<bool>(),
        gap in prop_oneof![0.0f32..2.0e-5, 0.0f32..1.0e-3, 0.0f32..0.03],
        at in 40.0f32..140.0,
        angle in prop_oneof![-0.02f32..0.02, -3.2f32..3.2],
        speed in 0.05f32..40.0,
        travel in 0.001f32..0.03,
    ) {
        let wall = 0.15f32;
        let g = arc_wall(wall, 180);
        let a = at.to_radians();
        let radial = v(libm::cosf(a), libm::sinf(a), 0.0);
        let tangent = v(-radial.y, radial.x, 0.0);
        let radius0 = if inside { wall - BALL - gap } else { wall + BALL + gap };
        let start = radial * radius0 + Vec3::Z * 0.0135;
        // `angle` from the tangent, positive = outward.
        let vel = (tangent * libm::cosf(angle) + radial * libm::sinf(angle)) * speed;
        let max_time = travel / speed;
        let res = resolve(&g, start, BALL, vel, max_time);
        prop_assert!(res.casts <= 2 && res.steps <= 12, "{} casts, {} steps", res.casts, res.steps);
        // Analytic gap to the ideal circle (the spline is within ~1 µm of it).
        let gap_at = |t: f32| {
            let p = start.as_dvec3() + vel.as_dvec3() * f64::from(t);
            let rho = (p.x * p.x + p.y * p.y).sqrt();
            let wall = f64::from(wall);
            (if inside { wall - rho } else { rho - wall }) - f64::from(BALL)
        };
        let tol = 3.0e-6;
        match res.hit {
            Some(hit) => {
                let found = gap_at(hit.time);
                prop_assert!(found <= f64::from(SKIN) + tol, "gap at contact {}", found);
                prop_assert!(found >= -f64::from(SKIN) - tol, "contact too deep: {}", found);
                // The gap is closing at the contact.
                prop_assert!(gap_at(hit.time + 1e-6 / speed) < found + 1e-9);
                let outward = if inside { -1.0 } else { 1.0 };
                prop_assert!((hit.normal - radial_at(start + vel * hit.time) * outward).length() < 2e-2);
            }
            None => {
                // No point of the path is deeper than the sliding allowance.
                for k in 0..=200 {
                    let found = gap_at(max_time * k as f32 / 200.0);
                    prop_assert!(found >= -f64::from(SKIN) - tol, "missed contact, gap {}", found);
                }
            }
        }
        // A ball outside the arc that is not closing in never gets a contact.
        if !inside && angle >= 0.0 && angle <= std::f32::consts::PI {
            prop_assert!(res.hit.is_none());
        }
    }
}

fn radial_at(p: Vec3) -> Vec3 {
    v(p.x, p.y, 0.0).normalize()
}
