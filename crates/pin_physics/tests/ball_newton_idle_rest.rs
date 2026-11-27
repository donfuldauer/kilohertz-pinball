//! KP-102 (issue #211): a Newton captive ball nobody touches stays at rest on
//! a pitched table (SPEC §3, Subphase 2.2 deliverable 4: "Captive Newton
//! balls constrained to a 1D spline"; the `newton_ball` cradle slot of
//! REQ-VIS-002 reports any drift). SPEC basis: a ball at rest stays at rest.
//!
//! Every case runs through [`BallSolver`] with the pitched gravity
//! `pin_sim` sets, `(0, -g sin p, -g cos p)`, the default rolling resistance
//! and 1 ms steps, on the path `pin_sim` builds for a `newton_ball` mech: two
//! knots 50 mm apart along `(cos angle, sin angle, 0)`, one radius above the
//! playfield, the captive at `t = 0` (one segment, `max_t = 1`). With the
//! default angle 90° the path runs uphill, so `t = 0` is the low end.
//!
//! Before the fix, gravity pressing the ball on the stop was reflected with
//! the chain restitution each step: the ball hopped off at about 2.9e-4 m/s
//! and crept to the touch distance (2e-5 m) by tick ~70.

#[path = "ball_util.rs"]
mod util;

use glam::Vec3;
use pin_core::fmath::max_f32;
use pin_physics::ball::{BallEvents, BallSolver, Geometry, NewtonChain};
use pin_physics::world::CubicSpline;
use proptest::prelude::*;
use util::*;

/// One second of 1 ms steps.
const TICKS_PER_S: usize = 1000;

fn gravity(pitch_deg: f32) -> Vec3 {
    let p = pitch_deg.to_radians();
    Vec3::new(0.0, -9.81 * libm::sinf(p), -9.81 * libm::cosf(p))
}

fn path(angle_deg: f32) -> CubicSpline {
    let a = angle_deg.to_radians();
    let start = Vec3::new(0.25, 0.8, radius(STEEL));
    let dir = Vec3::new(libm::cosf(a), libm::sinf(a), 0.0);
    CubicSpline::interpolate(&[start, start + dir * 0.05]).unwrap()
}

/// A solver on a table pitched by `pitch_deg` with one captive ball at path
/// parameter `t` of [`path`]`(angle_deg)`; returns the solver and the chain.
fn table(pitch_deg: f32, angle_deg: f32, t: f32) -> (BallSolver, usize) {
    let mut cfg = config();
    cfg.gravity = gravity(pitch_deg);
    let mut s = BallSolver::new(cfg, &profiles());
    let c = s.add_newton_chain(NewtonChain::new(path(angle_deg), STEEL, &[t]).unwrap());
    (s, c)
}

/// The chain restitution the solver binds for the standard steel profile:
/// the steel–steel material pair if the defaults have one, else the
/// profile's own restitution.
fn steel_restitution() -> f32 {
    let p = &profiles()[STEEL.index()];
    config()
        .pairs
        .iter()
        .find(|q| q.materials == [p.material, p.material])
        .map_or(p.restitution, |q| q.restitution)
}

/// Runs `ticks` steps, calling `each(tick, solver)` after every step.
fn run(s: &mut BallSolver, ticks: usize, mut each: impl FnMut(usize, &BallSolver)) {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut ev = BallEvents::new();
    for tick in 1..=ticks {
        ev.clear();
        s.step(&geo, DT, &mut ev);
        each(tick, s);
    }
}

/// SPEC §3 / Subphase 2.2, KP-102, requirement 3a: idle captive at the low
/// end of a 6.5° table. For 10 s it stays within 1e-6 m of its start at
/// every tick, and its path speed is at most 1e-6 m/s after tick 1.
#[test]
fn idle_captive_at_the_low_end_stays_at_rest() {
    let (mut s, c) = table(6.5, 90.0, 0.0);
    let p0 = s.newton_chain(c).ball_position(0);
    run(&mut s, 10 * TICKS_PER_S, |tick, s| {
        let ch = s.newton_chain(c);
        let off = ch.ball_position(0).distance(p0);
        assert!(off <= 1.0e-6, "tick {tick}: {off} m from rest");
        if tick > 1 {
            assert!(
                ch.ball_speed(0).abs() <= 1.0e-6,
                "tick {tick}: speed {}",
                ch.ball_speed(0)
            );
        }
    });
}

/// SPEC §3 / Subphase 2.2, KP-102, requirement 3c: the path reversed (angle
/// 270°): the low end is `t = max_t`, where the captive is placed; it stays
/// within 1e-6 m for 10 s.
#[test]
fn idle_captive_at_the_low_end_of_a_reversed_path_stays_at_rest() {
    let (mut s, c) = table(6.5, 270.0, 1.0);
    assert_eq!(s.newton_chain(c).path().max_t(), 1.0);
    let p0 = s.newton_chain(c).ball_position(0);
    assert!(
        s.newton_chain(c).path().point(0.0).y > p0.y + 0.049,
        "low end at max_t"
    );
    run(&mut s, 10 * TICKS_PER_S, |tick, s| {
        let ch = s.newton_chain(c);
        let off = ch.ball_position(0).distance(p0);
        assert!(off <= 1.0e-6, "tick {tick}: {off} m from rest");
        if tick > 1 {
            assert!(ch.ball_speed(0).abs() <= 1.0e-6, "tick {tick}");
        }
    });
}

/// SPEC §3 / Subphase 2.2, KP-102, requirement 3b: a captive placed halfway
/// up (t = 0.5) rolls down, bounces with the steel chain restitution and
/// after 10 s is within 1e-6 m of the low end with |speed| <= 1e-3 m/s (at
/// both orientations of the path).
#[test]
fn captive_placed_higher_settles_at_the_low_end() {
    let e = steel_restitution();
    assert!(e > 0.0 && e < 1.0, "steel restitution {e}");
    for &(angle, low) in &[(90.0f32, 0.0f32), (270.0, 1.0)] {
        let (mut s, c) = table(6.5, angle, 0.5);
        let stop = s.newton_chain(c).path().point(low);
        let mut bounces = 0;
        let mut prev = 0.0f32;
        run(&mut s, 10 * TICKS_PER_S, |_, s| {
            let u = s.newton_chain(c).ball_speed(0);
            // Toward the low end is `-sign`; a bounce turns it around.
            if prev != 0.0 && u != 0.0 && u.signum() != prev.signum() {
                bounces += 1;
            }
            prev = u;
        });
        let ch = s.newton_chain(c);
        let off = ch.ball_position(0).distance(stop);
        assert!(off <= 1.0e-6, "angle {angle}: {off} m from the low end");
        assert!(ch.ball_speed(0).abs() <= 1.0e-3, "angle {angle}");
        assert!(bounces >= 3, "angle {angle}: {bounces} bounces");
    }
}

/// SPEC §3 / Subphase 2.2, KP-102, requirement 3d: a real impact still
/// reflects. The captive at the low stop is struck outward at 1 m/s; each
/// time it comes back it bounces off the low stop with `e` times its
/// impact speed (to 1e-5 m/s), and it finishes at rest on the stop.
///
/// The impact speed of the step that bounces is the previous speed plus
/// the step's gravity gain along the path less the rolling hold (the chain
/// applies both at the start of the step).
#[test]
fn real_impact_at_the_low_stop_reflects_with_the_chain_restitution() {
    let e = steel_restitution();
    let g = gravity(6.5);
    let rr = config().rolling_resistance;
    let (mut s, c) = table(6.5, 90.0, 0.0);
    let tan = s.newton_chain(c).path().tangent(0.0).normalize();
    let along = g.dot(tan);
    let gain = 5.0 / 7.0 * along * DT;
    let hold = 5.0 / 7.0 * rr * (g - tan * along).length() * DT;
    s.newton_chain_mut(c).strike(0, 1.0);
    let mut prev = 1.0f32;
    let mut bounces = 0;
    run(&mut s, 10 * TICKS_PER_S, |tick, s| {
        let u = s.newton_chain(c).ball_speed(0);
        if prev < 0.0 && u > 0.0 {
            let impact = -(prev + gain) - hold;
            assert!(
                (u - e * impact).abs() <= 1.0e-5,
                "tick {tick}: {u} m/s after an impact at {impact} m/s (e {e})"
            );
            bounces += 1;
        }
        prev = u;
    });
    assert!(bounces >= 3, "{bounces} bounces");
    let ch = s.newton_chain(c);
    assert_eq!(ch.ball_t(0), 0.0, "at rest on the low stop");
    assert_eq!(ch.ball_speed(0), 0.0);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(32))]

    /// SPEC §3 / Subphase 2.2, KP-102: on any pitch from 0 to 15°, an idle
    /// captive at the low stop of either path orientation never leaves it
    /// by more than 1e-6 m (2 s through the solver). The restitution sweep
    /// is in the `newton.rs` unit proptest (the solver binds the profile's).
    #[test]
    fn idle_captive_never_leaves_the_low_stop(
        pitch in 0.0f32..=15.0,
        reversed in any::<bool>(),
    ) {
        let (angle, low) = if reversed { (270.0, 1.0) } else { (90.0, 0.0) };
        let (mut s, c) = table(pitch, angle, low);
        let p0 = s.newton_chain(c).ball_position(0);
        let mut worst = 0.0f32;
        run(&mut s, 2 * TICKS_PER_S, |_, s| {
            worst = max_f32(worst, s.newton_chain(c).ball_position(0).distance(p0));
        });
        prop_assert!(worst <= 1.0e-6, "pitch {}: {} m", pitch, worst);
    }
}
