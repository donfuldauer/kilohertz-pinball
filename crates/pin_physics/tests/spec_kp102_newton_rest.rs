//! KP-102 / SPEC §3 (physics) and Subphase 2.2 deliverable 4 ("Captive Newton
//! balls constrained to a 1D spline"); REQ-VIS-002 shows the effect, because
//! the cradle slot reports the drift.
//!
//! A captive Newton ball at rest on a pitched table must stay at rest, or
//! settle to the low end of its path. Black-box tests of the public
//! `pin_physics::ball` API.

#[path = "ball_util.rs"]
mod util;

use glam::Vec3;
use pin_physics::ball::{BallEvents, BallSolver, Geometry, NewtonChain};
use pin_physics::world::CubicSpline;
use proptest::prelude::*;
use util::*;

const PITCH_DEG: f32 = 6.5;
const REST_TOL: f32 = 1.0e-6;

fn gravity(pitch_deg: f32) -> Vec3 {
    let (s, c) = libm::sincosf(pitch_deg.to_radians());
    v(0.0, -9.81 * s, -9.81 * c)
}

fn path(angle_deg: f32) -> CubicSpline {
    let (s, c) = libm::sincosf(angle_deg.to_radians());
    let start = v(0.3, 0.4, 0.0135);
    CubicSpline::interpolate(&[start, start + v(c, s, 0.0) * 0.05]).unwrap()
}

fn max_t(p: &CubicSpline) -> f32 {
    p.max_t()
}

/// Per-tick record of one captive ball.
struct Trace {
    t: Vec<f32>,
    speed: Vec<f32>,
    pos: Vec<Vec3>,
    energy: Vec<f64>,
}

/// Mechanical energy per unit mass. The captive is a rolling ball (gravity
/// along the path accelerates it at 5/7 of the along-path component, SPEC
/// Subphase 2.2), so its kinetic energy is (1/2)(1 + 2/5) v^2 = 0.7 v^2.
fn energy(g: Vec3, speed: f32, pos: Vec3) -> f64 {
    0.7 * f64::from(speed) * f64::from(speed) - f64::from(g.dot(pos))
}

/// Runs one captive ball for `ticks` steps; `strike` is applied before tick 1.
/// Index 0 of each vector is the initial state, index k the state after tick k.
fn run(pitch: f32, angle: f32, t0_frac: f32, strike: Option<f32>, ticks: usize) -> Trace {
    let g = gravity(pitch);
    let mut cfg = config();
    cfg.gravity = g;
    let mut s = BallSolver::new(cfg, &profiles());
    let p = path(angle);
    let mt = max_t(&p);
    let c = s.add_newton_chain(NewtonChain::new(p, STEEL, &[t0_frac * mt]).unwrap());
    if let Some(u) = strike {
        s.newton_chain_mut(c).strike(0, u);
    }
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut tr = Trace {
        t: vec![],
        speed: vec![],
        pos: vec![],
        energy: vec![],
    };
    let rec = |s: &BallSolver, tr: &mut Trace| {
        let ch = s.newton_chain(c);
        let (t, u, q) = (ch.ball_t(0), ch.ball_speed(0), ch.ball_position(0));
        tr.t.push(t);
        tr.speed.push(u);
        tr.pos.push(q);
        tr.energy.push(energy(g, u, q));
    };
    rec(&s, &mut tr);
    for _ in 0..ticks {
        s.step(&geo, DT, &mut BallEvents::new());
        rec(&s, &mut tr);
    }
    tr
}

fn assert_idle_at_low_end(angle: f32, low_frac: f32) {
    let tr = run(PITCH_DEG, angle, low_frac, None, 10_000);
    for k in 1..tr.pos.len() {
        let d = tr.pos[k].distance(tr.pos[0]);
        assert!(d <= REST_TOL, "tick {k}: moved {d:e} m");
        if k > 1 {
            assert!(
                tr.speed[k].abs() <= 1.0e-6,
                "tick {k}: speed {:e}",
                tr.speed[k]
            );
        }
    }
}

/// KP-102 req 1: idle captive at the low end (path +y, t0 = 0).
#[test]
fn kp102_idle_at_low_end_stays_put() {
    assert_idle_at_low_end(90.0, 0.0);
}

/// KP-102 req 2: idle captive at the low end of the reversed path.
#[test]
fn kp102_idle_at_low_end_reversed_path_stays_put() {
    assert_idle_at_low_end(270.0, 1.0);
}

/// KP-102 req 3: level table, mid path and low end.
#[test]
fn kp102_level_table_stays_put() {
    for frac in [0.5f32, 0.0] {
        let tr = run(0.0, 90.0, frac, None, 10_000);
        for k in 1..tr.pos.len() {
            let d = tr.pos[k].distance(tr.pos[0]);
            assert!(d <= REST_TOL, "frac {frac} tick {k}: moved {d:e} m");
        }
    }
}

/// KP-102 req 4: from mid path at rest the ball settles to the low end.
#[test]
fn kp102_settles_to_low_end() {
    let tr = run(PITCH_DEG, 90.0, 0.5, None, 10_000);
    let mt = max_t(&path(90.0));
    for (k, &t) in tr.t.iter().enumerate() {
        assert!((0.0..=mt).contains(&t), "tick {k}: t {t} outside [0, {mt}]");
    }
    let low = v(0.3, 0.4, 0.0135);
    let last = tr.pos.len() - 1;
    assert!(
        tr.pos[last].distance(low) <= REST_TOL,
        "end offset {:e}",
        tr.pos[last].distance(low)
    );
    assert!(tr.speed[last].abs() <= 1.0e-3, "speed {:e}", tr.speed[last]);
}

/// KP-102 req 5: a real impact still reflects, and energy never increases.
#[test]
fn kp102_strike_still_bounces_and_loses_energy() {
    let tr = run(PITCH_DEG, 90.0, 0.0, Some(1.0), 10_000);
    let low = tr.pos[0];
    let max_off = tr
        .pos
        .iter()
        .map(|p| p.distance(low))
        .fold(0.0f32, pin_core::fmath::max_f32);
    assert!(max_off > 1.0e-2, "max offset {max_off}");
    let bounced = (1..tr.speed.len()).any(|k| tr.speed[k - 1] < 0.0 && tr.speed[k] > 0.0);
    assert!(bounced, "no negative-to-positive speed change");
    let last = tr.pos.len() - 1;
    assert!(
        tr.pos[last].distance(low) <= REST_TOL,
        "end offset {:e}",
        tr.pos[last].distance(low)
    );
    assert!(
        tr.speed[last].abs() <= 1.0e-3,
        "end speed {:e}",
        tr.speed[last]
    );
    for k in 2..tr.energy.len() {
        assert!(
            tr.energy[k] <= tr.energy[k - 1] + 1.0e-6,
            "tick {k}: energy {} -> {}",
            tr.energy[k - 1],
            tr.energy[k]
        );
    }
}

/// KP-102 req 6: two independent solvers give bit-identical sequences.
#[test]
fn kp102_deterministic() {
    for (frac, angle) in [(0.0f32, 90.0f32), (0.5, 90.0)] {
        let a = run(PITCH_DEG, angle, frac, None, 10_000);
        let b = run(PITCH_DEG, angle, frac, None, 10_000);
        for k in 0..a.t.len() {
            assert_eq!(a.t[k].to_bits(), b.t[k].to_bits(), "t tick {k}");
            assert_eq!(a.speed[k].to_bits(), b.speed[k].to_bits(), "speed tick {k}");
        }
    }
}

/// KP-102 req 8: two touching captives at the low end stay at rest and do not
/// interpenetrate.
#[test]
fn kp102_two_touching_balls_rest() {
    let d = 2.0 * radius(STEEL);
    let pts: Vec<Vec3> = (0..12).map(|i| v(d * i as f32, 0.0, 0.0)).collect();
    let p = CubicSpline::interpolate(&pts).unwrap();
    let mut cfg = config();
    cfg.gravity = gravity(PITCH_DEG);
    // Pitched gravity pulls toward -x on a path along +x? Use +y-uphill by
    // rotating the path instead: gravity's along-path component is -y, so lay
    // the knots along +y.
    let pts_y: Vec<Vec3> = (0..12)
        .map(|i| v(0.3, 0.4 + d * i as f32, 0.0135))
        .collect();
    let _ = p;
    let p = CubicSpline::interpolate(&pts_y).unwrap();
    let mut s = BallSolver::new(cfg, &profiles());
    let c = s.add_newton_chain(NewtonChain::new(p, STEEL, &[0.0, 1.0]).unwrap());
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut at5 = [Vec3::ZERO; 2];
    for k in 1..=10_000usize {
        s.step(&geo, DT, &mut BallEvents::new());
        let ch = s.newton_chain(c);
        let gap = ch.ball_position(0).distance(ch.ball_position(1)) - d;
        assert!(gap >= -2.0e-5, "tick {k}: gap {gap:e}");
        if k == 5_000 {
            at5 = [ch.ball_position(0), ch.ball_position(1)];
        }
        if k > 5_000 {
            #[allow(clippy::needless_range_loop)]
            for i in 0..2 {
                let m = ch.ball_position(i).distance(at5[i]);
                assert!(m <= REST_TOL, "tick {k} ball {i}: moved {m:e}");
                assert!(
                    ch.ball_speed(i).abs() <= 1.0e-4,
                    "tick {k} ball {i}: speed {:e}",
                    ch.ball_speed(i)
                );
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// KP-102 req 7a: the idle low-end ball never moves more than 1e-6 m.
    #[test]
    fn kp102_prop_idle_low_end(pitch in 0.0f32..15.0, rev in any::<bool>()) {
        let (angle, frac) = if rev { (270.0, 1.0) } else { (90.0, 0.0) };
        let tr = run(pitch, angle, frac, None, 2_000);
        for k in 1..tr.pos.len() {
            prop_assert!(tr.pos[k].distance(tr.pos[0]) <= REST_TOL, "tick {k}");
        }
    }

    /// KP-102 req 7b: anywhere on the path, t stays in range, speed bounded,
    /// energy never rises.
    #[test]
    fn kp102_prop_bounded_and_dissipative(
        pitch in 0.0f32..15.0,
        rev in any::<bool>(),
        frac in 0.0f32..=1.0,
    ) {
        let angle = if rev { 270.0 } else { 90.0 };
        let mt = max_t(&path(angle));
        let tr = run(pitch, angle, frac, None, 2_000);
        for k in 0..tr.t.len() {
            prop_assert!((0.0..=mt).contains(&tr.t[k]), "tick {k}: t {}", tr.t[k]);
            prop_assert!(tr.speed[k].is_finite() && tr.speed[k].abs() <= 5.0, "tick {k}");
        }
        for k in 1..tr.energy.len() {
            prop_assert!(tr.energy[k] <= tr.energy[0] + 1.0e-6, "tick {k}: {} vs {}", tr.energy[k], tr.energy[0]);
        }
    }
}
