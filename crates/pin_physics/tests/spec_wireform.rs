//! Independent spec tests (Subphase 2.1): wireforms (two rails framed by a
//! rotation-minimizing frame plus authored roll) and their frames.
//!
//! Black-box against `pin_physics::world::{Wireform, Frame, CubicSpline}`.

use glam::Vec3;
use pin_physics::world::{CubicSpline, Frame, Wireform};
use proptest::prelude::*;

// ---------------------------------------------------------------- helpers

fn mk(pts: &[Vec3]) -> CubicSpline {
    CubicSpline::interpolate(pts).expect("valid spline")
}

fn assert_frame_valid(f: &Frame, what: &str) {
    let tol = 1e-4;
    assert!(
        (f.tangent.length() - 1.0).abs() < tol,
        "{what}: |T| = {}",
        f.tangent.length()
    );
    assert!(
        (f.normal.length() - 1.0).abs() < tol,
        "{what}: |N| = {}",
        f.normal.length()
    );
    assert!(
        (f.binormal.length() - 1.0).abs() < tol,
        "{what}: |B| = {}",
        f.binormal.length()
    );
    assert!(
        f.tangent.dot(f.normal).abs() < tol,
        "{what}: T·N = {}",
        f.tangent.dot(f.normal)
    );
    assert!(
        f.tangent.dot(f.binormal).abs() < tol,
        "{what}: T·B = {}",
        f.tangent.dot(f.binormal)
    );
    assert!(
        f.normal.dot(f.binormal).abs() < tol,
        "{what}: N·B = {}",
        f.normal.dot(f.binormal)
    );
    let tn = f.tangent.cross(f.normal);
    assert!(
        (tn - f.binormal).length() < 1e-3,
        "{what}: not right-handed, T×N = {tn:?}, B = {:?}",
        f.binormal
    );
}

fn angle_between(a: Vec3, b: Vec3) -> f32 {
    a.normalize().dot(b.normalize()).clamp(-1.0, 1.0).acos()
}

/// Reference rotation-minimizing frame normal by fine-step double reflection
/// (Wang et al. 2008), starting from the up hint projected off the tangent.
fn reference_rmf_normals(s: &CubicSpline, up: Vec3, samples: &[f32]) -> Vec<Vec3> {
    let n = s.segment_count() as f32;
    let steps = (n * 4000.0) as usize;
    let t0 = s.tangent(0.0).normalize();
    let mut r = (up - t0 * up.dot(t0)).normalize();
    let mut x = s.point(0.0);
    let mut t_i = t0;
    let mut out = Vec::with_capacity(samples.len());
    let mut si = 0;
    while si < samples.len() && samples[si] <= 0.0 {
        out.push(r);
        si += 1;
    }
    for k in 1..=steps {
        let t = n * k as f32 / steps as f32;
        let x1 = s.point(t);
        let t1 = s.tangent(t).normalize();
        let v1 = x1 - x;
        let c1 = v1.dot(v1);
        if c1 > 0.0 {
            let rl = r - v1 * (2.0 / c1 * v1.dot(r));
            let tl = t_i - v1 * (2.0 / c1 * v1.dot(t_i));
            let v2 = t1 - tl;
            let c2 = v2.dot(v2);
            r = if c2 > 0.0 {
                rl - v2 * (2.0 / c2 * v2.dot(rl))
            } else {
                rl
            };
            r = (r - t1 * r.dot(t1)).normalize();
        }
        x = x1;
        t_i = t1;
        while si < samples.len() && samples[si] <= t + 1e-7 {
            out.push(r);
            si += 1;
        }
    }
    while out.len() < samples.len() {
        out.push(r);
    }
    out
}

fn planar_s_curve() -> Vec<Vec3> {
    vec![
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.15, 0.08, 0.0),
        Vec3::new(0.30, 0.0, 0.0),
        Vec3::new(0.45, -0.1, 0.0),
        Vec3::new(0.60, 0.0, 0.0),
        Vec3::new(0.75, 0.12, 0.0),
    ]
}

/// A climbing helix-like ramp (non-zero torsion): Frenet frames twist here,
/// a rotation-minimizing frame must not.
fn helix(turns: f32, radius: f32, pitch: f32, n: usize) -> Vec<Vec3> {
    (0..n)
        .map(|i| {
            let a = std::f32::consts::TAU * turns * i as f32 / (n - 1) as f32;
            Vec3::new(
                radius * a.cos(),
                radius * a.sin(),
                pitch * a / std::f32::consts::TAU,
            )
        })
        .collect()
}

const GAUGE: f32 = 0.032; // a typical wireform rail gauge (m)

// ---------------------------------------------------------------- tests

#[test]
fn frames_are_orthonormal_and_right_handed() {
    let pts = helix(1.0, 0.2, 0.15, 13);
    let s = CubicSpline::interpolate(&pts).unwrap();
    let rolls: Vec<f32> = (0..pts.len()).map(|i| -40.0 + 7.0 * i as f32).collect();
    let w = Wireform::new(mk(&pts), &rolls, GAUGE, Vec3::Z).unwrap();
    for k in 0..=600 {
        let t = 12.0 * k as f32 / 600.0;
        let f = w.frame_at(t);
        assert_frame_valid(&f, &format!("t = {t}"));
        let tan = s.tangent(t).normalize();
        assert!(
            f.tangent.dot(tan) > 0.9999,
            "frame tangent follows the curve at t = {t}"
        );
    }
}

#[test]
fn rails_are_gauge_apart_and_centred() {
    let pts = helix(0.75, 0.25, 0.1, 10);
    let s = CubicSpline::interpolate(&pts).unwrap();
    let rolls = vec![15.0f32; pts.len()];
    let w = Wireform::new(mk(&pts), &rolls, GAUGE, Vec3::Z).unwrap();
    for k in 0..=300 {
        let t = 9.0 * k as f32 / 300.0;
        let (a, b) = w.rail_points(t);
        assert!(
            ((a - b).length() - GAUGE).abs() < 1e-5,
            "separation {}",
            (a - b).length()
        );
        assert!(
            ((a + b) * 0.5 - s.point(t)).length() < 1e-5,
            "rails centred on centreline"
        );
        let f = w.frame_at(t);
        // The rail offset is along the (rolled) binormal, perpendicular to the tangent.
        assert!((a - b).normalize().dot(f.binormal).abs() > 0.9999);
        assert!((a - b).dot(f.tangent).abs() < 1e-5);
    }
}

#[test]
fn zero_roll_on_planar_curve_keeps_rails_in_plane() {
    let pts = planar_s_curve();
    let s = CubicSpline::interpolate(&pts).unwrap();
    let w = Wireform::new(s, &vec![0.0; pts.len()], GAUGE, Vec3::Z).unwrap();
    for k in 0..=500 {
        let t = 5.0 * k as f32 / 500.0;
        let (a, b) = w.rail_points(t);
        assert!(
            a.z.abs() < 1e-5 && b.z.abs() < 1e-5,
            "rails left the plane at t = {t}: {a:?} {b:?}"
        );
        let f = w.frame_at(t);
        // A planar curve's RMF normal stays the plane normal (the up hint).
        assert!(
            f.normal.dot(Vec3::Z) > 0.9999,
            "normal {:?} at t = {t}",
            f.normal
        );
        assert!(f.binormal.z.abs() < 1e-4);
    }
}

#[test]
fn up_hint_not_unit_or_not_perpendicular_is_projected() {
    // A curve that starts climbing: up hint is not perpendicular to the tangent.
    let pts = vec![
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(0.1, 0.0, 0.03),
        Vec3::new(0.2, 0.05, 0.065),
        Vec3::new(0.3, 0.1, 0.065),
    ];
    let s = CubicSpline::interpolate(&pts).unwrap();
    let w = Wireform::new(mk(&pts), &[0.0; 4], GAUGE, Vec3::new(0.0, 0.0, 5.0)).unwrap();
    let f = w.frame_at(0.0);
    assert_frame_valid(&f, "t = 0");
    let t0 = s.tangent(0.0).normalize();
    let expect = (Vec3::Z - t0 * t0.z).normalize();
    assert!(
        f.normal.dot(expect) > 0.9999,
        "initial normal {:?} vs {expect:?}",
        f.normal
    );
}

#[test]
fn constant_roll_tilts_rails_out_of_plane() {
    let pts = planar_s_curve();
    let s = CubicSpline::interpolate(&pts).unwrap();
    for &deg in &[30.0f32, -30.0, 90.0, -90.0] {
        let w = Wireform::new(mk(&pts), &vec![deg; pts.len()], GAUGE, Vec3::Z).unwrap();
        let expect = 0.5 * GAUGE * deg.to_radians().sin().abs();
        for k in 0..=100 {
            let t = 5.0 * k as f32 / 100.0;
            let (a, b) = w.rail_points(t);
            assert!(
                (a.z.abs() - expect).abs() < 1e-5 && (b.z.abs() - expect).abs() < 1e-5,
                "roll {deg}° at t = {t}: rail z {} / {} vs ±{expect}",
                a.z,
                b.z
            );
            assert!((a.z + b.z).abs() < 1e-5, "rails tilt symmetrically");
        }
    }
}

#[test]
fn authored_roll_is_hit_at_through_points() {
    let pts = helix(0.5, 0.2, 0.08, 7);
    let s = CubicSpline::interpolate(&pts).unwrap();
    let rolls = [0.0f32, 10.0, -25.0, 45.0, 90.0, -60.0, 5.0];
    let base = Wireform::new(mk(&pts), &[0.0; 7], GAUGE, Vec3::Z).unwrap();
    let rolled = Wireform::new(s, &rolls, GAUGE, Vec3::Z).unwrap();
    for (i, &deg) in rolls.iter().enumerate() {
        let t = i as f32;
        let (f0, f1) = (base.frame_at(t), rolled.frame_at(t));
        assert!(
            f0.tangent.dot(f1.tangent) > 0.99999,
            "roll is about the tangent"
        );
        let ang = angle_between(f0.normal, f1.normal).to_degrees();
        assert!(
            (ang - deg.abs()).abs() < 0.05,
            "knot {i}: rolled by {ang}°, authored {deg}°"
        );
    }
    // Same signed direction for equal-signed angles: +10° and +45° roll the
    // normal the same way about the tangent.
    let sign_at = |i: usize| {
        let t = i as f32;
        let (f0, f1) = (base.frame_at(t), rolled.frame_at(t));
        f0.normal.cross(f1.normal).dot(f0.tangent).signum()
    };
    assert_eq!(sign_at(1), sign_at(3));
    assert_eq!(sign_at(1), sign_at(4));
    assert_eq!(sign_at(2), sign_at(5));
    assert_ne!(sign_at(1), sign_at(2));
}

#[test]
fn roll_is_continuous_between_knots() {
    let pts = planar_s_curve();
    let s = CubicSpline::interpolate(&pts).unwrap();
    let w = Wireform::new(s, &[0.0, 20.0, 60.0, -30.0, 0.0, 10.0], GAUGE, Vec3::Z).unwrap();
    let mut prev = w.frame_at(0.0);
    for k in 1..=1000 {
        let f = w.frame_at(5.0 * k as f32 / 1000.0);
        assert!(
            angle_between(prev.normal, f.normal).to_degrees() < 2.0,
            "frame jumped at step {k}"
        );
        prev = f;
    }
}

#[test]
fn rmf_has_low_twist_on_a_helix() {
    let pts = helix(1.0, 0.2, 0.2, 17);
    let s = CubicSpline::interpolate(&pts).unwrap();
    let w = Wireform::new(mk(&pts), &vec![0.0; pts.len()], GAUGE, Vec3::Z).unwrap();
    let samples: Vec<f32> = (0..=320).map(|k| 16.0 * k as f32 / 320.0).collect();
    let reference = reference_rmf_normals(&s, Vec3::Z, &samples);
    for (&t, &r) in samples.iter().zip(&reference) {
        let n = w.frame_at(t).normal;
        let ang = angle_between(n, r).to_degrees();
        assert!(
            ang < 1.0,
            "RMF normal deviates {ang}° from reference at t = {t}"
        );
    }
    // Local twist rate: the normal's rotation about the tangent stays ~0.
    let h = 1e-3f32;
    for &t in &samples[1..samples.len() - 1] {
        let f = w.frame_at(t);
        let dn = (w.frame_at(t + h).normal - w.frame_at(t - h).normal) / (2.0 * h);
        let twist = dn.dot(f.binormal);
        assert!(
            twist.abs() < 0.02,
            "twist rate {twist} rad per unit t at t = {t}"
        );
    }
}

#[test]
fn wireform_errors() {
    let pts = planar_s_curve();
    let s = CubicSpline::interpolate(&pts).unwrap();
    assert!(
        Wireform::new(mk(&pts), &[0.0; 3], GAUGE, Vec3::Z).is_err(),
        "too few roll angles"
    );
    assert!(
        Wireform::new(mk(&pts), &[0.0; 9], GAUGE, Vec3::Z).is_err(),
        "too many roll angles"
    );
    let mut bad = vec![0.0f32; pts.len()];
    bad[2] = f32::NAN;
    assert!(
        Wireform::new(mk(&pts), &bad, GAUGE, Vec3::Z).is_err(),
        "NaN roll"
    );
    assert!(
        Wireform::new(mk(&pts), &vec![0.0; pts.len()], f32::NAN, Vec3::Z).is_err(),
        "NaN gauge"
    );
    assert!(
        Wireform::new(s, &vec![0.0; pts.len()], GAUGE, Vec3::ZERO).is_err(),
        "zero up hint"
    );
}

// ---------------------------------------------------------------- property tests

fn path_strategy() -> impl Strategy<Value = Vec<Vec3>> {
    prop::collection::vec((0.05f32..0.3, -0.15f32..0.15, -0.06f32..0.06), 2..9).prop_map(|steps| {
        let mut pts = Vec::with_capacity(steps.len() + 1);
        let mut cur = Vec3::ZERO;
        pts.push(cur);
        for (dx, dy, dz) in steps {
            cur += Vec3::new(dx, dy, dz);
            pts.push(cur);
        }
        pts
    })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, .. ProptestConfig::default() })]

    #[test]
    fn prop_frames_and_rails(
        pts in path_strategy(),
        roll_seed in prop::collection::vec(-90.0f32..90.0, 10),
        gauge in 0.01f32..0.05,
        t_frac in prop::collection::vec(0.0f32..=1.0, 16),
    ) {
        let s = CubicSpline::interpolate(&pts).unwrap();
        let rolls: Vec<f32> = roll_seed[..pts.len()].to_vec();
        let w = Wireform::new(mk(&pts), &rolls, gauge, Vec3::Z).unwrap();
        let n = s.segment_count() as f32;
        for &u in &t_frac {
            let t = u * n;
            let f = w.frame_at(t);
            assert_frame_valid(&f, &format!("t = {t}"));
            prop_assert!(f.tangent.dot(s.tangent(t).normalize()) > 0.9999);
            let (a, b) = w.rail_points(t);
            prop_assert!(((a - b).length() - gauge).abs() < 1e-5);
            prop_assert!(((a + b) * 0.5 - s.point(t)).length() < 1e-5);
        }
    }

    #[test]
    fn prop_zero_roll_planar_rails_stay_in_plane(
        steps in prop::collection::vec((0.05f32..0.3, -0.15f32..0.15), 2..9),
        z0 in -0.1f32..0.1,
        t_frac in prop::collection::vec(0.0f32..=1.0, 16),
    ) {
        let mut pts = vec![Vec3::new(0.0, 0.0, z0)];
        for (dx, dy) in steps {
            let last = *pts.last().unwrap();
            pts.push(last + Vec3::new(dx, dy, 0.0));
        }
        let s = CubicSpline::interpolate(&pts).unwrap();
        let w = Wireform::new(mk(&pts), &vec![0.0; pts.len()], 0.03, Vec3::Z).unwrap();
        for &u in &t_frac {
            let (a, b) = w.rail_points(u * s.segment_count() as f32);
            prop_assert!((a.z - z0).abs() < 1e-5 && (b.z - z0).abs() < 1e-5);
        }
    }
}
