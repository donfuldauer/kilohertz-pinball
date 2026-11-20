//! A free ball touching a Newton-chain captive ball that moves away from it
//! faster than the ball follows (fix a1fix-A, docs/DECISIONS.md
//! "a1fix-A-resolved — receding-captive gravity defect fixed").
//!
//! The solver advances the chain to the end of the tick first and casts the
//! free ball against the captive balls at those positions. A ball whose path
//! reaches a captive that recedes from it faster than it closes in has no
//! impact to apply there. It used to be frozen for the rest of its sub-step
//! while gravity was still integrated into its velocity, so its energy rose
//! by `m (g·v) t_frozen`. It must keep moving and gain no energy.
//!
//! Each case: a one-ball chain on a straight path along +x, pitched gravity
//! (a component along the path), the captive struck to path speed `u`. A
//! twin solver without the free ball gives the captive's position and speed
//! at the end of the tick; the free ball starts exactly touching that
//! position, closing in on it along the contact normal faster than the
//! resting-contact slop (1 mm/s) but slower than the captive recedes.
//!
//! A captive set moving by the ball's own push during the tick has not moved
//! on yet (the chain advanced before the push): the ball then waits for its
//! next sub-step, but without the gravity of the time it waits
//! ([`ball_pushing_a_captive_at_rest_gains_no_energy_while_it_waits`]).

#[path = "ball_util.rs"]
mod util;

use glam::{DVec3, Vec3};
use pin_physics::ball::{BallEvent, BallEvents, BallSolver, Geometry, ImpactTarget, NewtonChain};
use pin_physics::world::CubicSpline;
use util::*;

/// Closing speed below which touching balls rest (solver's `REST_SLOP_SPEED`).
const SLOP: f32 = 1.0e-3;

fn straight_path() -> CubicSpline {
    let d = 2.0 * radius(STEEL);
    let pts: Vec<Vec3> = (0..12).map(|i| v(d * i as f32, 0.0, 0.0)).collect();
    CubicSpline::interpolate(&pts).unwrap()
}

fn solver_with(gravity: Vec3) -> BallSolver {
    let mut cfg = config();
    cfg.gravity = gravity;
    BallSolver::new(cfg, &profiles())
}

/// A solver with one captive ball at path parameter 5, struck to `u`.
fn chain_solver(gravity: Vec3, u: f32) -> BallSolver {
    let mut s = solver_with(gravity);
    let c = s.add_newton_chain(NewtonChain::new(straight_path(), STEEL, &[5.0]).unwrap());
    s.newton_chain_mut(c).strike(0, u);
    s
}

/// Mechanical energy of the free ball (f64): `−m g·p + ½ m v² + ½ I ω²`.
fn energy(s: &BallSolver, g: Vec3) -> f64 {
    let p = &s.profiles()[STEEL.index()];
    let (m, i) = (f64::from(p.mass_kg), f64::from(p.moment_of_inertia()));
    s.balls()
        .map(|(_, b)| {
            -m * g.as_dvec3().dot(b.position.as_dvec3())
                + 0.5 * m * b.velocity.as_dvec3().length_squared()
                + 0.5 * i * b.angular_velocity.as_dvec3().length_squared()
        })
        .sum()
}

/// One case of the sweep, set up as in the module doc. `n` is the contact
/// normal (from the captive toward the free ball), `closing` the ball's
/// speed toward the captive along `-n`, `slide` a speed across `n`.
struct Case {
    pitch: f32,
    u: f32,
    n: Vec3,
    closing: f32,
    slide: Vec3,
}

/// What one step of a case did.
struct Outcome {
    gravity: Vec3,
    /// Captive path speed at the end of the tick and its unit tangent (+x).
    u_end: f32,
    /// Ball velocity at the start and after the step.
    v0: Vec3,
    v1: Vec3,
    travelled: f32,
    e0: f64,
    e1: f64,
    newton_impact: bool,
}

fn run_case(c: &Case) -> Outcome {
    let gravity = 9.81 * v(libm::sinf(c.pitch), 0.0, -libm::cosf(c.pitch));
    let world = empty_world();
    let geo = Geometry::new(&world);
    // Twin without the free ball: where the captive is at the end of the
    // tick (the chain does not depend on the free balls before the tick's
    // ball timeline).
    let mut twin = chain_solver(gravity, c.u);
    twin.step(&geo, DT, &mut BallEvents::new());
    let q_end = twin.newton_chain(0).ball_position(0);
    let u_end = twin.newton_chain(0).ball_speed(0);

    let mut s = chain_solver(gravity, c.u);
    let rsum = 2.0 * radius(STEEL);
    let v0 = -c.n * c.closing + c.slide;
    let h = s.add_ball(ball_at(STEEL, q_end + c.n * rsum, v0)).unwrap();
    let p0 = s.ball(h).unwrap().position;
    let e0 = energy(&s, gravity);
    let mut ev = BallEvents::new();
    s.step(&geo, DT, &mut ev);
    assert_eq!(
        s.newton_chain(0).ball_position(0),
        q_end,
        "the twin predicts the captive"
    );
    let b = s.ball(h).unwrap();
    Outcome {
        gravity,
        u_end,
        v0,
        v1: b.velocity,
        travelled: b.position.distance(p0),
        e0,
        e1: energy(&s, gravity),
        newton_impact: ev.as_slice().iter().any(|e| {
            matches!(
                e,
                BallEvent::Impact {
                    other: ImpactTarget::Newton(0),
                    ..
                }
            )
        }),
    }
}

/// Contact normals: on the side of the captive the ball approaches from,
/// at several angles to the path and out of the plane.
fn normals(behind: bool) -> Vec<Vec3> {
    let sx = if behind { -1.0 } else { 1.0 };
    let mut out = Vec::new();
    for a in [0.0f32, 0.35, 0.7, 1.0] {
        for b in [0.0f32, 0.25] {
            out.push(v(
                sx * libm::cosf(a) * libm::cosf(b),
                libm::sinf(a) * libm::cosf(b),
                libm::sinf(b),
            ));
        }
    }
    out
}

/// A slide direction across `n` (zero, or a fixed perpendicular).
fn slides(n: Vec3) -> [Vec3; 2] {
    let perp = n.cross(Vec3::Z).normalize_or_zero();
    [Vec3::ZERO, perp * 0.25]
}

#[test]
fn ball_touching_a_receding_captive_keeps_moving_and_gains_no_energy() {
    let mut cases = 0;
    for pitch in [-0.12f32, -0.04, 0.0, 0.04, 0.12] {
        for u in [0.3f32, 1.0, 2.5] {
            // The ball is behind the captive (−x side), the captive moves +x.
            for n in normals(true) {
                for closing in [0.0025f32, 0.03, 0.2] {
                    for slide in slides(n) {
                        let c = Case {
                            pitch,
                            u,
                            n,
                            closing,
                            slide,
                        };
                        let o = run_case(&c);
                        let tangent = Vec3::X;
                        // Captive velocity along the normal (negative: away).
                        let recede = -(tangent * o.u_end).dot(n);
                        // The ball's normal speed toward the captive, at the
                        // start and after the boundary's gravity (the speed
                        // it moves with during the step).
                        let v_mid = o.v0 + o.gravity * DT;
                        let close0 = -o.v0.dot(n);
                        let close_mid = -v_mid.dot(n);
                        if !(close0.min(close_mid) > 1.05 * SLOP
                            && recede > 1.05 * close0.max(close_mid))
                        {
                            continue;
                        }
                        cases += 1;
                        let label = format!(
                            "pitch {pitch} u {u} (end {}) n {n:?} closing {closing} slide {slide:?}",
                            o.u_end
                        );
                        assert!(!o.newton_impact, "{label}: no impact on a receding captive");
                        assert!(
                            (o.v1 - v_mid).length() <= 1.0e-5,
                            "{label}: velocity {:?}, expected free flight {v_mid:?}",
                            o.v1
                        );
                        let need = 0.99 * o.v1.length() * DT;
                        assert!(
                            o.travelled >= need,
                            "{label}: travelled {} m, expected >= {need} m (frozen?)",
                            o.travelled
                        );
                        assert!(
                            o.e1 <= o.e0 + 1.0e-9,
                            "{label}: energy rose by {:e} J",
                            o.e1 - o.e0
                        );
                    }
                }
            }
        }
    }
    assert!(cases >= 100, "only {cases} receding cases were checked");
}

#[test]
fn ball_touching_an_approaching_captive_still_collides() {
    let mut cases = 0;
    for pitch in [-0.12f32, 0.0, 0.12] {
        // The captive moves −x, toward the ball on its −x side.
        for u in [-0.3f32, -1.0, -2.5] {
            for n in normals(true) {
                for closing in [0.0f32, 0.03] {
                    let c = Case {
                        pitch,
                        u,
                        n,
                        closing,
                        slide: Vec3::ZERO,
                    };
                    let o = run_case(&c);
                    let approach = (Vec3::X * o.u_end).dot(n);
                    let v_mid = o.v0 + o.gravity * DT;
                    // Relative approach speed along the normal.
                    let rel = approach - v_mid.dot(n);
                    if rel < 0.1 {
                        continue;
                    }
                    cases += 1;
                    let label = format!("pitch {pitch} u {u} n {n:?} closing {closing}");
                    assert!(o.newton_impact, "{label}: impact reported");
                    // The ball is pushed out along the normal and no longer
                    // closes in on the captive.
                    assert!(
                        o.v1.dot(n) >= v_mid.dot(n) + 0.5 * rel,
                        "{label}: v.n {} -> {}",
                        v_mid.dot(n),
                        o.v1.dot(n)
                    );
                }
            }
        }
    }
    assert!(cases >= 30, "only {cases} approaching cases were checked");
}

/// A ball slides along gravity past a captive at rest (path across the
/// slope, so the captive has no potential energy term) and touches it,
/// closing in slowly. The boundary solve pushes the captive away; the ball
/// then closes in on the captive's position, which the push has not moved
/// yet, with nothing to apply there. Before fix a1fix-A it waited for the
/// rest of its sub-step with that sub-step's full gravity in its velocity,
/// gaining `m (g·v) h`. The energy of the ball plus the captive's kinetic
/// energy must not rise, and the ball must not sink into the captive.
#[test]
fn ball_pushing_a_captive_at_rest_gains_no_energy_while_it_waits() {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let rsum = 2.0 * radius(STEEL);
    let mc = f64::from(s_mass());
    let mut cases = 0;
    for g_slope in [0.5f32, 1.1, 3.0] {
        // Gravity along +y (down the slope) and into −z; the path runs
        // along x, across the slope.
        let gravity = v(0.0, g_slope, -9.7);
        for speed in [0.3f32, 1.0, 2.0] {
            for closing in [0.004f32, 0.01, 0.03] {
                for a in [0.0f32, 0.3] {
                    let n = v(-libm::cosf(a), 0.0, libm::sinf(a));
                    let mut s = chain_solver(gravity, 0.0);
                    let q0 = s.newton_chain(0).ball_position(0);
                    let h = s
                        .add_ball(ball_at(
                            STEEL,
                            q0 + n * rsum,
                            -n * closing + v(0.0, speed, 0.0),
                        ))
                        .unwrap();
                    let e0 = energy(&s, gravity);
                    s.step(&geo, DT, &mut BallEvents::new());
                    let u = f64::from(s.newton_chain(0).ball_speed(0));
                    let e1 = energy(&s, gravity) + 0.5 * mc * u * u;
                    let label = format!("g {g_slope} speed {speed} closing {closing} a {a}");
                    assert!(u > 0.0, "{label}: the captive was pushed");
                    cases += 1;
                    assert!(
                        e1 <= e0 + 1.0e-9,
                        "{label}: ball + captive energy rose by {:e} J",
                        e1 - e0
                    );
                    let gap = s
                        .ball(h)
                        .unwrap()
                        .position
                        .distance(s.newton_chain(0).ball_position(0))
                        - rsum;
                    assert!(gap >= -2.0e-6, "{label}: sank {gap} m into the captive");
                }
            }
        }
    }
    assert_eq!(cases, 54);
}

fn s_mass() -> f32 {
    profiles()[STEEL.index()].mass_kg
}

/// The energy helper is the module doc's formula (a sanity check of the
/// harness, not of the solver).
#[test]
fn energy_helper_matches_formula() {
    let g = v(0.5, 0.0, -9.8);
    let mut s = solver_with(g);
    s.add_ball(ball_at(STEEL, v(0.1, 0.2, 0.3), v(1.0, 2.0, 0.0)))
        .unwrap();
    let p = &s.profiles()[STEEL.index()];
    let m = f64::from(p.mass_kg);
    let want = -m * DVec3::new(0.5, 0.0, -9.8).dot(DVec3::new(0.1, 0.2, 0.3)) + 0.5 * m * 5.0;
    let got = energy(&s, g);
    assert!((got - want).abs() <= 1.0e-6 * want.abs(), "{got} vs {want}");
}
