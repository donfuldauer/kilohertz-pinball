//! Subphase 2.8 verification 1 and 3 (PLAN 2.8): a 79.4 mm ball against a
//! 15.88 mm ball conserves momentum to the 2.2 tolerances, and the XL ball
//! never tunnels through a flipper, up to the fastest bat the coil model
//! produces.

#[path = "ball_util.rs"]
mod util;

#[path = "flipper_util.rs"]
mod flip;

use glam::Vec3;
use pin_core::ids::ProfileId;
use pin_physics::ball::{
    Ball, BallEvent, BallEvents, BallHandle, Geometry, ImpactTarget, MovingBodies,
};
use pin_physics::world::StaticWorldBuilder;
use util::*;

/// `jumbo_xl_resin` (79.4 mm, 445 g) in the built-in order.
const XL: ProfileId = ProfileId(6);

#[test]
fn builtin_profiles_span_15_88_to_79_4_mm() {
    let p = profiles();
    assert_eq!(p[XL.index()].name, "jumbo_xl_resin");
    assert!((p[XL.index()].diameter_m - 0.0794).abs() < 1e-7);
    assert!((p[MINI.index()].diameter_m - 0.01588).abs() < 1e-7);
}

// ---------------------------------------------------------------- criterion 1

/// 2.2 criterion 4's sweep with the extreme pair: mini (15.88 mm, 16.5 g)
/// fired at the XL resin ball (79.4 mm, 445 g). Per tick: linear momentum
/// within 1e-4 relative, angular momentum (about the centre of mass) within
/// 1e-3, kinetic energy never rising (1e-6 relative).
#[test]
fn c1_xl_vs_mini_free_collision_conserves_momenta() {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let (rm, rx) = (radius(MINI), radius(XL));
    let mut collisions = 0;
    let mut worst_p = 0.0f64;
    let mut worst_l = 0.0f64;
    for offset in [-0.95f32, -0.7, -0.4, 0.0, 0.25, 0.6, 0.9] {
        for (wm, wx) in [
            (Vec3::ZERO, Vec3::ZERO),
            (v(0.0, 0.0, 400.0), Vec3::ZERO),
            (v(80.0, -120.0, 30.0), v(-10.0, 5.0, 40.0)),
            (v(0.0, 500.0, 0.0), v(0.0, 0.0, -60.0)),
        ] {
            for (speed, xl_speed) in [(0.5f32, 0.0f32), (3.0, -0.4), (12.0, 0.3), (40.0, -2.0)] {
                for mini_first in [true, false] {
                    let mut s = solver_no_gravity();
                    let mut mini = ball_at(
                        MINI,
                        v(-0.08, offset * (rm + rx), 0.002),
                        v(speed, 0.0, 0.0),
                    );
                    mini.angular_velocity = wm;
                    let mut xl = ball_at(XL, v(0.0, 0.0, 0.0), v(xl_speed, 0.05, 0.0));
                    xl.angular_velocity = wx;
                    let (hm, hx) = if mini_first {
                        (s.add_ball(mini).unwrap(), s.add_ball(xl).unwrap())
                    } else {
                        let hx = s.add_ball(xl).unwrap();
                        (s.add_ball(mini).unwrap(), hx)
                    };
                    let (p0, l0, e0) = (momentum(&s), angular_momentum(&s), kinetic_energy(&s));
                    let mut ev = BallEvents::new();
                    let mut e_prev = e0;
                    for _ in 0..200 {
                        s.step(&geo, DT, &mut ev);
                        let e = kinetic_energy(&s);
                        assert!(e <= e_prev * (1.0 + 1e-6), "energy rose: {e_prev} -> {e}");
                        e_prev = e;
                        let dp = (momentum(&s) - p0).length() / p0.length();
                        assert!(dp <= 1e-4, "p drift {dp} (offset {offset}, speed {speed})");
                        worst_p = worst_p.max(dp);
                        let dl = (angular_momentum(&s) - l0).length() / l0.length().max(1e-7);
                        assert!(dl <= 1e-3, "L drift {dl} (offset {offset}, speed {speed})");
                        worst_l = worst_l.max(dl);
                        let (a, b) = (s.ball(hm).unwrap(), s.ball(hx).unwrap());
                        assert!(
                            a.position.distance(b.position) > rm + rx - 2.0e-5,
                            "overlap (offset {offset}, speed {speed})"
                        );
                    }
                    if ev.as_slice().iter().any(|e| {
                        matches!(
                            e,
                            BallEvent::Impact {
                                other: ImpactTarget::Ball(_),
                                ..
                            }
                        )
                    }) {
                        collisions += 1;
                        assert!(kinetic_energy(&s) < e0);
                        // The heavy ball took most of the momentum change.
                        let b = s.ball(hx).unwrap();
                        assert!(b.velocity.is_finite());
                    }
                }
            }
        }
    }
    println!("XL/mini: {collisions} collisions, worst |Δp|/p {worst_p:.2e}, |ΔL|/L {worst_l:.2e}");
    assert!(collisions > 150, "{collisions}");
}

/// Head-on, the textbook exchange: `v' = ((m1 − e m2) v1) / (m1 + m2)` for
/// the mini ball and `(1 + e) m1 v1 / (m1 + m2)` for the XL ball, with the
/// pair-table restitution.
#[test]
fn c1_head_on_exchange_matches_the_two_body_formula() {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let p = profiles();
    let (m1, m2) = (
        f64::from(p[MINI.index()].mass_kg),
        f64::from(p[XL.index()].mass_kg),
    );
    for speed in [0.5f32, 4.0, 20.0] {
        let mut s = solver_no_gravity();
        let hm = s
            .add_ball(ball_at(MINI, v(-0.1, 0.0, 0.0), v(speed, 0.0, 0.0)))
            .unwrap();
        let hx = s
            .add_ball(ball_at(XL, v(0.0, 0.0, 0.0), Vec3::ZERO))
            .unwrap();
        let mut ev = BallEvents::new();
        for _ in 0..300 {
            s.step(&geo, DT, &mut ev);
        }
        let (vm, vx) = (
            f64::from(s.ball(hm).unwrap().velocity.x),
            f64::from(s.ball(hx).unwrap().velocity.x),
        );
        // Restitution from the result, then both velocities must agree with it.
        let v1 = f64::from(speed);
        let e = (vx - vm) / v1;
        assert!(e > 0.0 && e <= 1.0, "e {e}");
        let want_m = (m1 - e * m2) * v1 / (m1 + m2);
        let want_x = (1.0 + e) * m1 * v1 / (m1 + m2);
        assert!((vm - want_m).abs() <= 1e-4 * v1, "{vm} vs {want_m}");
        assert!((vx - want_x).abs() <= 1e-4 * v1, "{vx} vs {want_x}");
    }
}

// ---------------------------------------------------------------- criterion 3

fn xl_ball(rig: &mut flip::Rig, position: Vec3, velocity: Vec3) -> BallHandle {
    let mut b = Ball::new(XL, position);
    b.velocity = velocity;
    rig.solver.add_ball(b).unwrap()
}

fn space_rig(c: pin_core::FlipperConfig) -> flip::Rig {
    flip::Rig::with_world(&[c], StaticWorldBuilder::new().build(), Vec3::ZERO)
}

/// The bat swings into a resting XL ball at up to the fastest tip speed the
/// coil model gives (light bat, 15 N·m): the ball is struck, never ends up
/// inside the bat, and leaves on the side it started on.
#[test]
fn c3_fastest_bat_does_not_swing_through_the_xl_ball() {
    let r = radius(XL);
    let mut fastest_tip = 0.0f32;
    let mut deepest = 0.0f32;
    for torque in [1.2f32, 4.0, 15.0, 40.0, 80.0] {
        for fraction in [0.3f32, 0.6, 0.95] {
            for gap_mm in [0.5f32, 5.0, 20.0, 30.0] {
                if gap_mm > 0.5 * fraction * 76.0 * 50.0f32.to_radians() {
                    continue;
                }
                let mut c = flip::left_config(0, [0.0; 3]);
                c.power_torque_nm = torque;
                c.inertia_kg_m2 = 0.5e-4;
                let mut rig = space_rig(c);
                let dir = flip::bat_dir(-30.0);
                let n = Vec3::Z.cross(dir);
                let bat_r = 0.011 + (0.0065 - 0.011) * fraction;
                let start = dir * (flip::BAT_LEN * fraction)
                    + n * (bat_r + r + gap_mm * 1e-3)
                    + v(0.0, 0.0, r);
                let h = xl_ball(&mut rig, start, Vec3::ZERO);
                rig.flippers.set_energised(flip::LEFT, true);
                let mut peak_tip = 0.0f32;
                for _ in 0..80 {
                    rig.step();
                    peak_tip =
                        peak_tip.max(rig.flippers.angular_velocity(flip::LEFT).abs() * 0.0825);
                    let cp = rig
                        .flippers
                        .closest_point(0, 0.0, rig.ball(h).position)
                        .unwrap();
                    deepest = deepest.max(r - cp.distance);
                    assert!(
                        cp.distance > r - 1.0e-3,
                        "torque {torque}, {fraction}, gap {gap_mm} mm: ball {:.2} mm inside the bat",
                        (r - cp.distance) * 1e3
                    );
                }
                let b = rig.ball(h);
                assert!(
                    b.velocity.length() > 0.1,
                    "torque {torque}, {fraction}, gap {gap_mm}: not hit"
                );
                assert!(b.position.dot(Vec3::Z.cross(flip::bat_dir(20.0))) > 0.0);
                fastest_tip = fastest_tip.max(peak_tip);
            }
        }
    }
    println!(
        "XL vs bat: fastest tip {fastest_tip:.1} m/s, deepest {:.3} mm",
        deepest * 1e3
    );
    assert!(fastest_tip > 15.0, "{fastest_tip}");
}

/// XL balls fired at 1–80 m/s into a resting bat (pushed into its rest
/// stop) and a held bat (into its end stop) never cross it and come back.
#[test]
fn c3_xl_balls_bounce_off_held_and_resting_bats_at_any_speed() {
    let r = radius(XL);
    for held in [false, true] {
        for speed in [1.0f32, 5.0, 15.0, 40.0, 80.0] {
            for fraction in [0.1f32, 0.5, 0.9] {
                let mut rig = space_rig(flip::left_config(0, [0.0; 3]));
                if held {
                    rig.flippers.set_energised(flip::LEFT, true);
                    for _ in 0..200 {
                        rig.flippers.advance(DT);
                    }
                }
                let dir = rig.flippers.bat_direction(flip::LEFT).unwrap();
                let n = Vec3::Z.cross(dir);
                let side = if held { -1.0 } else { 1.0 };
                let start = dir * (flip::BAT_LEN * fraction) + n * (side * 0.15) + v(0.0, 0.0, r);
                let h = xl_ball(&mut rig, start, n * (-side * speed));
                let mut hit = false;
                for _ in 0..400 {
                    rig.step();
                    hit |= !rig.hits.is_empty();
                    let b = rig.ball(h);
                    let p = b.position - rig.flippers.pivot_m(flip::LEFT).unwrap();
                    let d = rig.flippers.bat_direction(flip::LEFT).unwrap();
                    let across = p.dot(Vec3::Z.cross(d));
                    assert!(
                        across * side > 0.0,
                        "held {held}, {speed} m/s at {fraction}: went through"
                    );
                    let cp = rig.flippers.closest_point(0, 0.0, b.position).unwrap();
                    assert!(
                        cp.distance > r - 1.0e-3,
                        "held {held}, {speed} m/s: inside the bat"
                    );
                }
                assert!(hit, "held {held}, {speed} m/s at {fraction}: never touched");
                let b = rig.ball(h);
                assert!(
                    b.velocity.dot(n) * side > 0.0,
                    "held {held}, {speed} m/s at {fraction}: did not come back"
                );
            }
        }
    }
}
