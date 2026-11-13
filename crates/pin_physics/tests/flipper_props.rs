//! Subphase 2.3 invariants: no tunnelling, no energy from the impulse,
//! stops, tilted mounting planes, EOS and coil behaviour.

#[path = "flipper_util.rs"]
mod util;

use glam::{Quat, Vec3};
use pin_core::FlipperConfig;
use pin_core::ids::SwitchId;
use pin_physics::ball::{BallHandle, MovingBodies};
use pin_physics::flipper::{EosEdge, FlipperSet};
use pin_physics::world::StaticWorldBuilder;
use proptest::prelude::*;
use util::*;

/// A flipper in empty space without gravity.
fn space_rig(config: FlipperConfig) -> Rig {
    Rig::with_world(&[config], StaticWorldBuilder::new().build(), Vec3::ZERO)
}

fn ball_energy(rig: &Rig, h: BallHandle) -> f64 {
    let b = rig.ball(h);
    let p = &profiles()[0];
    let (m, r) = (f64::from(p.mass_kg), f64::from(p.radius_m()));
    0.5 * m * f64::from(b.velocity.length_squared())
        + 0.5 * 0.4 * m * r * r * f64::from(b.angular_velocity.length_squared())
}

/// Bat-frame coordinates (along, across) of the ball, in the plane of a
/// z-normal flipper at the origin.
fn bat_frame(rig: &Rig, h: BallHandle) -> (f32, f32) {
    let dir = rig.flippers.bat_direction(LEFT).unwrap();
    let p = rig.ball(h).position - rig.flippers.pivot_m(LEFT).unwrap();
    (p.dot(dir), p.dot(Vec3::Z.cross(dir)))
}

#[test]
fn fast_balls_bounce_off_a_held_and_a_resting_bat() {
    for held in [false, true] {
        for speed in [1.0f32, 5.0, 15.0, 40.0, 80.0] {
            for fraction in [0.1f32, 0.5, 0.9] {
                let mut rig = space_rig(left_config(0, [0.0; 3]));
                if held {
                    rig.flippers.set_energised(LEFT, true);
                    for _ in 0..200 {
                        rig.flippers.advance(DT);
                    }
                }
                let dir = rig.flippers.bat_direction(LEFT).unwrap();
                let n = Vec3::Z.cross(dir);
                // From the side the stop backs: above a resting bat it is
                // pushed into the rest stop, below a held bat into the end
                // stop.
                let side = if held { -1.0 } else { 1.0 };
                let start = dir * (BAT_LEN * fraction) + n * (side * 0.1) + v(0.0, 0.0, BALL_R);
                let h = rig.add_ball(start, n * (-side * speed));
                for _ in 0..400 {
                    rig.step();
                    let (_, across) = bat_frame(&rig, h);
                    assert!(
                        across * side > 0.0,
                        "held {held}, {speed} m/s at {fraction}: went through"
                    );
                }
                let b = rig.ball(h);
                assert!(
                    b.velocity.dot(n) * side > 0.0,
                    "held {held}, {speed} m/s at {fraction}: did not come back"
                );
            }
        }
    }
}

#[test]
fn a_fast_bat_does_not_swing_through_a_ball() {
    // Tip speeds up to and beyond 15 m/s: strong coil, light bat.
    let mut fastest_tip = 0.0f32;
    for torque in [1.2f32, 4.0, 15.0] {
        for fraction in [0.3f32, 0.6, 0.95] {
            for gap_mm in [0.5f32, 5.0, 20.0, 40.0] {
                // Only gaps the stroke can close at this point of the bat.
                if gap_mm > 0.5 * fraction * 76.0 * 50.0f32.to_radians() {
                    continue;
                }
                let mut c = left_config(0, [0.0; 3]);
                c.power_torque_nm = torque;
                c.inertia_kg_m2 = 0.5e-4;
                let mut rig = space_rig(c);
                let dir = bat_dir(-30.0);
                let n = Vec3::Z.cross(dir);
                let radius = 0.011 + (0.0065 - 0.011) * fraction;
                let start = dir * (BAT_LEN * fraction)
                    + n * (radius + BALL_R + gap_mm * 1e-3)
                    + v(0.0, 0.0, BALL_R);
                let h = rig.add_ball(start, Vec3::ZERO);
                rig.flippers.set_energised(LEFT, true);
                let mut peak_tip = 0.0f32;
                for _ in 0..60 {
                    rig.step();
                    peak_tip = peak_tip.max(rig.flippers.angular_velocity(LEFT).abs() * 0.0825);
                    let cp = rig
                        .flippers
                        .closest_point(0, 0.0, rig.ball(h).position)
                        .unwrap();
                    assert!(
                        cp.distance > BALL_R - 1.0e-3,
                        "torque {torque}, {fraction}, gap {gap_mm} mm: ball {:.2} mm inside the bat",
                        (BALL_R - cp.distance) * 1e3
                    );
                }
                let b = rig.ball(h);
                assert!(
                    b.velocity.length() > 0.5,
                    "torque {torque}, {fraction}, gap {gap_mm}: the ball was hit, v = {}, pos {}, bat {}",
                    b.velocity,
                    b.position,
                    rig.flippers.angle_deg(LEFT)
                );
                // It left on the side it started on.
                assert!(b.position.dot(Vec3::Z.cross(bat_dir(20.0))) > 0.0);
                fastest_tip = fastest_tip.max(peak_tip);
            }
        }
    }
    println!("fastest bat tip in the sweep: {fastest_tip:.1} m/s");
    assert!(fastest_tip > 15.0);
}

fn tilt(axis_angle: f32, tilt_deg: f32) -> Quat {
    let axis = v(libm::cosf(axis_angle), libm::sinf(axis_angle), 0.0);
    Quat::from_axis_angle(axis, tilt_deg.to_radians())
}

/// A left flipper mounted in the plane `frame` carries the flat one to.
fn mounted(frame: Quat) -> FlipperConfig {
    let mut c = left_config(0, [0.0; 3]);
    c.base_position_mm = (frame * v(40.0, -25.0, 10.0)).to_array();
    c.surface_normal = (frame * Vec3::Z).to_array();
    c
}

/// The shot of a ball lying against the resting bat, in the flipper's own
/// frame, after `ticks`.
fn shot_in_frame(frame: Quat, ticks: u32) -> (Vec3, Vec3, f32) {
    let mut rig = Rig::with_world(
        &[mounted(frame)],
        StaticWorldBuilder::new().build(),
        frame * v(0.0, -1.1, -9.7),
    );
    let dir = bat_dir(-30.0);
    let n = Vec3::Z.cross(dir);
    let local =
        v(0.04, -0.025, 0.01) + dir * 0.045 + n * (0.0085 + BALL_R + 1e-4) + v(0.0, 0.0, BALL_R);
    let h = rig.add_ball(frame * local, Vec3::ZERO);
    rig.flippers.set_energised(LEFT, true);
    rig.run(ticks);
    let b = rig.ball(h);
    let back = frame.inverse();
    (
        back * b.position,
        back * b.velocity,
        rig.flippers.angle_deg(LEFT),
    )
}

#[test]
fn a_tilted_flipper_is_the_flat_one_rotated() {
    let (p0, v0, a0) = shot_in_frame(Quat::IDENTITY, 30);
    assert!(
        v0.length() > 2.0,
        "the reference shot launches the ball: {v0}"
    );
    for (axis, deg) in [
        (0.0f32, 6.5f32),
        (1.0, 30.0),
        (2.5, 75.0),
        (4.0, 120.0),
        (0.3, 179.0),
    ] {
        let (p, vel, a) = shot_in_frame(tilt(axis, deg), 30);
        assert!(
            (p - p0).length() < 2.0e-4,
            "tilt {deg}°: position {p} vs {p0}"
        );
        assert!(
            (vel - v0).length() < 0.02,
            "tilt {deg}°: velocity {vel} vs {v0}"
        );
        assert!((a - a0).abs() < 0.01);
    }
    // Upside down: the half turn about +X.
    let flip = Quat::from_axis_angle(Vec3::X, std::f32::consts::PI);
    let (p, vel, _) = shot_in_frame(flip, 30);
    assert!((p - p0).length() < 2.0e-4 && (vel - v0).length() < 0.02);
}

#[test]
fn plane_axes_follow_the_shortest_rotation() {
    use pin_physics::flipper::plane_axes;
    assert_eq!(plane_axes(Vec3::Z), (Vec3::X, Vec3::Y));
    assert_eq!(plane_axes(Vec3::NEG_Z), (Vec3::X, Vec3::NEG_Y));
    let (zero, quarter) = plane_axes(Vec3::Y);
    assert!((zero - Vec3::X).length() < 1e-6 && (quarter - Vec3::NEG_Z).length() < 1e-6);
}

#[test]
fn eos_edges_hold_and_return() {
    let mut set = FlipperSet::new(&[left_config(0, [0.0; 3])]).unwrap();
    assert!(set.eos_closed(LEFT));
    assert_eq!(set.angle_deg(LEFT), -30.0);
    set.set_energised(LEFT, true);
    let mut edges = Vec::new();
    for t in 0..300 {
        set.advance(DT);
        for e in set.eos_edges() {
            edges.push((t, *e));
        }
        let a = set.angle_deg(LEFT);
        assert!((-30.0..=20.0).contains(&a), "angle {a} outside the stops");
    }
    assert_eq!(edges.len(), 1, "one opening edge, no chatter: {edges:?}");
    assert_eq!(
        edges[0].1,
        EosEdge {
            flipper: LEFT,
            switch: SwitchId(100),
            closed: false
        }
    );
    // Held: on the end stop, at rest, with the hold torque.
    assert_eq!(set.angle_deg(LEFT), 20.0);
    assert_eq!(set.angular_velocity(LEFT), 0.0);
    assert!(!set.eos_closed(LEFT));
    assert!((set.coil_torque_nm(LEFT) - 0.15).abs() < 1e-4);
    assert!((set.relative_angle_rad(LEFT) - 50.0f32.to_radians()).abs() < 1e-6);
    // Released: the spring brings it back, the EOS closes again.
    set.set_energised(LEFT, false);
    let mut back = None;
    for t in 0..400 {
        set.advance(DT);
        if back.is_none() && set.angle_deg(LEFT) == -30.0 && set.angular_velocity(LEFT) == 0.0 {
            back = Some(t);
        }
        if let Some(e) = set.eos_edges().first() {
            assert!(e.closed);
            edges.push((t, *e));
        }
    }
    assert_eq!(edges.len(), 2);
    let back = back.expect("returns to rest");
    println!("return to rest {back} ms after release");
    assert!(back > 30 && back < 250, "{back}");
    assert_eq!(set.coil_torque_nm(LEFT), 0.0);
    assert!(set.kinetic_energy_j(LEFT) == 0.0);
}

#[test]
fn a_short_tap_gives_a_weak_flip() {
    // The coil needs time to build up its pull: the peak bat speed grows
    // with the length of the tap.
    let mut peaks = Vec::new();
    for tap in [2u32, 5, 10, 15, 25] {
        let mut set = FlipperSet::new(&[left_config(0, [0.0; 3])]).unwrap();
        set.set_energised(LEFT, true);
        let mut peak = 0.0f32;
        for t in 0..80 {
            if t == tap {
                set.set_energised(LEFT, false);
            }
            set.advance(DT);
            peak = peak.max(set.angular_velocity(LEFT));
        }
        peaks.push(peak);
    }
    println!("peak bat speed for 2/5/10/15/25 ms taps: {peaks:.1?} rad/s");
    assert!(peaks.windows(2).all(|w| w[0] < w[1]), "{peaks:?}");
}

#[test]
fn a_cradled_ball_stays_put_for_seconds() {
    let (mut rig, h) = cradle_rig();
    let start = rig.ball(h).position;
    let mut fastest = 0.0f32;
    for _ in 0..3000 {
        rig.step();
        fastest = fastest.max(rig.ball(h).velocity.length());
    }
    let moved = (rig.ball(h).position - start).length();
    println!(
        "cradle: moved {:.3} mm in 3 s, fastest {fastest:.5} m/s",
        moved * 1e3
    );
    assert!(moved < 1.0e-3 && fastest < 0.02);
    assert_eq!(rig.flippers.angle_deg(LEFT), 20.0);
}

#[test]
fn upper_flipper_only_touches_its_own_layer() {
    let mut c = left_config(0, [0.0; 3]);
    c.collision_layer = 1;
    let mut rig = space_rig(c);
    assert_eq!(rig.flippers.layer_mask(0), 0b10);
    let dir = bat_dir(-30.0);
    let n = Vec3::Z.cross(dir);
    let start = dir * 0.04 + n * 0.05 + v(0.0, 0.0, BALL_R);
    // Layer 0 ball flies through, layer 1 ball bounces.
    let through = rig.add_ball(start, n * -2.0);
    let mut other = pin_physics::ball::Ball::new(STEEL, start + dir * 0.02);
    other.velocity = n * -2.0;
    other.layer = 1;
    let bounced = rig.solver.add_ball(other).unwrap();
    rig.run(100);
    assert!(rig.ball(through).velocity.dot(n) < -1.9);
    assert!(rig.ball(bounced).velocity.dot(n) > 0.0);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// No tunnelling and no energy out of nothing, for any ball speed,
    /// direction, flip timing, coil strength and mounting plane: the ball
    /// never changes sides through the middle of the bat, never sits deep
    /// inside it, and the kinetic energy of ball and bat never exceeds the
    /// start plus what the drive put in.
    #[test]
    fn no_tunnelling_no_free_energy(
        speed in 0.3f32..20.0,
        from_angle in 0.0f32..std::f32::consts::TAU,
        aim in 0.3f32..0.7,
        flip_at in 0u32..50,
        release_after in 1u32..60,
        torque in 0.6f32..3.0,
        inertia in 0.5e-4f32..3.0e-4,
        axis in 0.0f32..std::f32::consts::TAU,
        tilt_deg in 0.0f32..179.0,
        spin in -200.0f32..200.0,
    ) {
        let frame = tilt(axis, tilt_deg);
        let mut c = left_config(0, [0.0; 3]);
        c.surface_normal = (frame * Vec3::Z).to_array();
        c.power_torque_nm = torque;
        c.inertia_kg_m2 = inertia;
        let mut rig = space_rig(c);
        // Aim at a point of the bat's mid stroke from 12 cm away.
        let target = bat_dir(-5.0) * (BAT_LEN * aim);
        let from = target + v(libm::cosf(from_angle), libm::sinf(from_angle), 0.0) * 0.12;
        let velocity = (target - from).normalize() * speed;
        let h = rig.add_ball(frame * (from + v(0.0, 0.0, BALL_R)), frame * velocity);
        rig.solver.ball_mut(h).unwrap().angular_velocity = frame * v(0.0, 0.0, spin);
        let start = ball_energy(&rig, h);
        let mut previous: Option<(f32, f32)> = None;
        for t in 0..150u32 {
            if t == flip_at {
                rig.flippers.set_energised(LEFT, true);
            }
            if t == flip_at + release_after {
                rig.flippers.set_energised(LEFT, false);
            }
            rig.step();
            let b = rig.ball(h);
            prop_assert!(b.position.is_finite() && b.velocity.is_finite());
            let cp = rig.flippers.closest_point(0, 0.0, b.position).unwrap();
            prop_assert!(cp.distance > BALL_R - 1.5e-3, "tick {}: {} mm inside", t, (BALL_R - cp.distance) * 1e3);
            // Sides: in the flipper's own frame.
            let dir = rig.flippers.bat_direction(LEFT).unwrap();
            let p = b.position - rig.flippers.pivot_m(LEFT).unwrap();
            let now = (p.dot(dir), p.dot((frame * Vec3::Z).cross(dir)));
            if let Some(before) = previous {
                let middle = |x: f32| x > 0.25 * BAT_LEN && x < 0.75 * BAT_LEN;
                if middle(before.0) && middle(now.0) {
                    prop_assert!(before.1 * now.1 > 0.0, "tick {}: through the bat, {:?} -> {:?}", t, before, now);
                }
            }
            previous = Some(now);
            let energy = ball_energy(&rig, h) + rig.flippers.kinetic_energy_j(LEFT);
            let budget = start + rig.flippers.drive_work_j(LEFT);
            prop_assert!(energy <= budget + 1.0e-4 + 1.0e-3 * budget.abs(),
                "tick {}: energy {} J exceeds {} J", t, energy, budget);
            let a = rig.flippers.angle_deg(LEFT);
            prop_assert!((-30.0..=20.0).contains(&a));
        }
    }

    /// The bat never leaves its stops and its state stays finite under any
    /// button sequence, for either sweep direction.
    #[test]
    fn stops_hold_under_any_button_sequence(
        presses in proptest::collection::vec((0u32..40, any::<bool>()), 1..20),
        sweep in prop_oneof![-170.0f32..-5.0, 5.0f32..170.0],
        rest in -360.0f32..360.0,
        fraction in 0.0f32..=1.0,
        restitution in 0.0f32..=1.0,
    ) {
        let mut c = left_config(0, [0.0; 3]);
        c.sweep_angle_deg = sweep;
        c.rest_angle_deg = rest;
        c.eos_angle_fraction = fraction;
        c.stop_restitution = restitution;
        let mut set = FlipperSet::new(&[c]).unwrap();
        let (lo, hi) = (rest.min(rest + sweep), rest.max(rest + sweep));
        for (ticks, on) in presses {
            set.set_energised(LEFT, on);
            for _ in 0..ticks {
                set.advance(DT);
                let a = set.angle_deg(LEFT);
                prop_assert!(a >= lo - 1e-3 && a <= hi + 1e-3, "angle {} outside [{}, {}]", a, lo, hi);
                prop_assert!(set.angular_velocity(LEFT).is_finite());
                let travelled = (a - rest) / sweep;
                // The EOS switch agrees with the angle.
                if travelled < fraction - 1e-3 {
                    prop_assert!(set.eos_closed(LEFT));
                }
                if travelled > fraction + 1e-3 {
                    prop_assert!(!set.eos_closed(LEFT));
                }
                prop_assert!(set.kinetic_energy_j(LEFT) <= set.drive_work_j(LEFT) + 1e-6);
            }
        }
    }
}
