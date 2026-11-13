//! Seam task 2.2-S, fix round 1: regression tests for the review findings.
//!
//! * B1: the two-body contact solve on a finite-mass body with one-sided
//!   stops stays bounded, both for an owner that keeps the contract and for
//!   one that does not predict its stops (divergence guard).
//! * B2: a ball cradled on a held bat at the default `FlipperConfig`
//!   numbers (I = 1e-4 kg·m²) comes to rest for hold torques 0.15–5 N·m when
//!   the owner keeps obligation 3 (speculative stops).
//!
//! The first two tests are the Reviewer's repros, unchanged in substance.

#[path = "ball_util.rs"]
mod util;

#[path = "ball_seam_ref.rs"]
mod reference;

use glam::Vec3;
use pin_core::PhysicsDefaults;
use pin_physics::ball::{BallEvents, BallSolver, BallSolverConfig, Geometry, MovingBodies};
use pin_physics::world::{StaticWorldBuilder, layers};
use proptest::prelude::*;
use reference::*;
use util::*;

const L: f32 = 0.076;
const BR: f32 = 0.009;
const I_DEFAULT: f32 = 1.0e-4;

fn pitched_solver() -> BallSolver {
    let pitch = 6.5f32.to_radians();
    let mut cfg = BallSolverConfig::from_defaults(&PhysicsDefaults::builtin());
    cfg.gravity = v(0.0, -9.81 * libm::sinf(pitch), -9.81 * libm::cosf(pitch));
    BallSolver::new(cfg, &profiles())
}

/// A ball at `speed` lands at `x` on a bat lying on its lower stop; the
/// flip (`torque`) starts at tick `flip`. Returns the largest ball speed
/// seen and whether everything stayed finite.
fn land_on_stopped_bat(
    x: f32,
    speed: f32,
    flip: u32,
    torque: f32,
    inertia: f32,
    speculative: bool,
) -> f32 {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut s = solver_no_gravity();
    let r = radius(STEEL);
    let mut bat = RotorBody::new(v(0.0, 0.0, r), L, BR, inertia, -0.5);
    bat.limits = (-0.5, 0.5);
    bat.torque = -0.05;
    bat.speculative_stops = speculative;
    let mut set = Rotors::one(bat);
    let h = s
        .add_ball(ball_at(STEEL, v(x, 0.06, r), v(0.0, -speed, 0.0)))
        .unwrap();
    let mut ev = BallEvents::new();
    let mut top = 0.0f32;
    for tick in 0..60 {
        if tick == flip {
            set.bodies[0].torque = torque;
        }
        ev.clear();
        s.step_with_bodies(&geo, &mut [&mut set], DT, &mut ev);
        let b = s.ball(h).unwrap();
        assert!(
            b.velocity.is_finite() && b.position.is_finite(),
            "non-finite ball at tick {tick}: x {x} speed {speed} flip {flip} torque {torque} I {inertia}"
        );
        assert!(set.bodies[0].omega.is_finite());
        top = top.max(b.velocity.length());
    }
    top
}

/// B1 repro: a 7 m/s steel ball lands on a default-inertia bat lying on its
/// lower stop in the tick the 1.2 N·m flip starts.
#[test]
fn b1_ball_on_bat_at_its_stop_stays_finite() {
    for speculative in [true, false] {
        let top = land_on_stopped_bat(0.055, 7.0, 9, 1.2, I_DEFAULT, speculative);
        assert!(top < 30.0, "speculative {speculative}: {top}");
    }
}

/// The Reviewer's sweep: landing points, speeds, flip timing, torques and
/// inertias. A bat tip at 0.5 rad/ms is ~40 m/s, so no rebound can exceed
/// 60 m/s.
#[test]
fn b1_sweep_stays_bounded() {
    for speculative in [true, false] {
        for inertia in [I_DEFAULT, 2.5e-4] {
            for torque in [1.2f32, 3.0] {
                for x in [0.02f32, 0.035, 0.05, 0.065, 0.08] {
                    for speed in [1.0f32, 4.0, 7.0, 12.0] {
                        for flip in [5u32, 8, 9, 10, 12] {
                            let top = land_on_stopped_bat(x, speed, flip, torque, inertia, speculative);
                            assert!(
                                top < 60.0,
                                "x {x} speed {speed} flip {flip} torque {torque} I {inertia} spec {speculative}: {top}"
                            );
                        }
                    }
                }
            }
        }
    }
}

/// Two driven, stop-limited rotors squeezing one ball (and a rotor squeezing
/// it against a static wall): bounded, finite.
fn squeeze(torque: f32, inertia: f32, wall: bool, speculative: bool) -> f32 {
    let mut s = pitched_solver();
    let r = radius(STEEL);
    let mut wb = StaticWorldBuilder::new();
    wb.add_collider(
        cuboid(v(0.0, 0.0, -0.05), v(0.6, 0.6, 0.05)),
        layers::ALL,
        true,
    );
    if wall {
        wb.add_collider(
            cuboid(v(0.04, BR + 2.0 * r + 0.004 + 0.01, 0.03), v(0.06, 0.01, 0.03)),
            layers::ALL,
            true,
        );
    }
    let world = wb.build();
    let geo = Geometry::new(&world);
    let mut lower = RotorBody::new(v(0.0, 0.0, r), L, BR, inertia, 0.0);
    lower.torque = torque;
    lower.limits = (-0.3, 0.6);
    lower.speculative_stops = speculative;
    let mut set = Rotors::default();
    set.bodies.push(lower);
    if !wall {
        let top = 2.0 * BR + 2.0 * r + 0.004;
        let mut upper = RotorBody::new(v(0.1, top, r), L, BR, inertia, std::f32::consts::PI);
        upper.torque = torque;
        upper.limits = (std::f32::consts::PI - 0.3, std::f32::consts::PI + 0.6);
        upper.speculative_stops = speculative;
        set.bodies.push(upper);
    }
    let h = s
        .add_ball(ball_at(STEEL, v(0.03, BR + r + 0.002, r), Vec3::ZERO))
        .unwrap();
    let mut ev = BallEvents::new();
    let mut top = 0.0f32;
    for tick in 0..800 {
        ev.clear();
        s.step_with_bodies(&geo, &mut [&mut set], DT, &mut ev);
        let b = s.ball(h).unwrap();
        assert!(
            b.velocity.is_finite() && b.position.is_finite(),
            "tick {tick}: torque {torque} I {inertia} wall {wall} spec {speculative}"
        );
        for rb in &set.bodies {
            assert!(rb.omega.is_finite() && rb.angle.is_finite());
        }
        top = top.max(b.velocity.length());
    }
    top
}

#[test]
fn b1_squeezes_stay_bounded() {
    for speculative in [true, false] {
        for (torque, inertia) in [(0.15f32, I_DEFAULT), (1.2, I_DEFAULT), (1.2, 2.5e-4), (3.0, I_DEFAULT)] {
            for wall in [false, true] {
                let top = squeeze(torque, inertia, wall, speculative);
                assert!(top < 60.0, "torque {torque} I {inertia} wall {wall} spec {speculative}: {top}");
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn ball_on_stop_engaged_rotor_is_bounded(
        x in 0.012f32..0.09, speed in 0.2f32..15.0, flip in 0u32..20,
        torque in 0.1f32..5.0, inertia in 5.0e-5f32..5.0e-4, speculative: bool,
    ) {
        let top = land_on_stopped_bat(x, speed, flip, torque, inertia, speculative);
        prop_assert!(top < 60.0, "{}", top);
    }
}

/// B2: default `FlipperConfig` numbers (I = 1e-4), bat held on its upper
/// stop with `hold` N·m; a ball rolls down it into the cradle. Returns, over
/// the last 2 s, the largest gap to the bat, the largest ball speed and the
/// number of events.
fn cradle(hold: f32) -> (f32, f32, usize) {
    let mut s = pitched_solver();
    let r = radius(STEEL);
    let mut wb = StaticWorldBuilder::new();
    wb.add_collider(
        cuboid(v(0.0, 0.0, -0.05), v(0.5, 0.5, 0.05)),
        layers::ALL,
        true,
    );
    wb.add_collider(
        cuboid(v(-0.02, 0.0, 0.02), v(0.005, 0.1, 0.02)),
        layers::ALL,
        true,
    );
    let world = wb.build();
    let geo = Geometry::new(&world);
    let mut bat = RotorBody::new(v(0.0, 0.0, r), L, BR, I_DEFAULT, 0.35);
    bat.limits = (-0.5, 0.35);
    bat.torque = hold;
    let mut set = Rotors::one(bat);
    let dir = RotorBody::direction(0.35);
    let nrm = Vec3::Z.cross(dir);
    let h = s
        .add_ball(ball_at(
            STEEL,
            v(0.0, 0.0, r) + dir * 0.06 + nrm * (BR + r + 0.0002),
            Vec3::ZERO,
        ))
        .unwrap();
    let mut ev = BallEvents::new();
    let (mut max_gap, mut max_speed, mut impacts) = (0.0f32, 0.0f32, 0);
    for tick in 0..4000 {
        ev.clear();
        s.step_with_bodies(&geo, &mut [&mut set], DT, &mut ev);
        if tick >= 2000 {
            let b = s.ball(h).unwrap();
            max_gap = max_gap.max(set.bodies[0].closest(0.0, b.position).distance - r);
            max_speed = max_speed.max(b.velocity.length());
            impacts += ev.as_slice().len();
        }
    }
    let b = s.ball(h).unwrap();
    let proj = (b.position - set.bodies[0].pivot).dot(dir);
    assert!(proj < 0.03, "hold {hold}: ball not in the cradle ({proj})");
    (max_gap, max_speed, impacts)
}

#[test]
fn b2_cradle_at_default_flipper_parameters() {
    let mut bad = Vec::new();
    for hold in [0.15f32, 0.5, 1.2, 5.0] {
        let (gap, speed, impacts) = cradle(hold);
        println!(
            "hold {hold}: last 2 s max gap {gap:.2e} m, max speed {speed:.2e} m/s, events {impacts}"
        );
        if gap > 2.0e-5 || speed > 1.0e-3 || impacts > 0 {
            bad.push(hold);
        }
    }
    assert!(bad.is_empty(), "not at rest for hold torques {bad:?}");
}

/// Obligation 3 itself: the reference rotor never predicts travel past a
/// stop within a tick, and arrives on it exactly at the end of the tick.
#[test]
fn speculative_stop_prediction() {
    let mut b = RotorBody::new(Vec3::ZERO, L, BR, I_DEFAULT, 0.345);
    b.limits = (-0.5, 0.35);
    b.torque = 1.2; // α·dt = 12 rad/s
    let mut set = Rotors::one(b);
    set.begin_tick(DT);
    let r = set.bodies[0];
    assert!((r.omega - 5.0).abs() < 1e-2, "{}", r.omega); // (0.35 − 0.345) / 1 ms
    assert!((r.angle_at(DT) - 0.35).abs() < 1e-6);
    set.end_tick(DT);
    set.begin_tick(DT);
    assert_eq!(set.bodies[0].omega, 0.0); // held
    // Off: the full α·dt is predicted and the stop clamps the position.
    let mut b = RotorBody::new(Vec3::ZERO, L, BR, I_DEFAULT, 0.345);
    b.limits = (-0.5, 0.35);
    b.torque = 1.2;
    b.speculative_stops = false;
    let mut set = Rotors::one(b);
    set.begin_tick(DT);
    assert!((set.bodies[0].omega - 12.0).abs() < 1e-3);
}
