//! Subphase 2.6 invariants: the analytic closest point against brute force,
//! no tunnelling through either face or the tip at any ball or bat speed,
//! no energy out of nothing, mirror and tilt symmetry.

#[path = "banana_util.rs"]
mod butil;

use butil::*;
use glam::{Quat, Vec3};
use pin_core::{FlipperConfig, FlipperShape};
use pin_physics::ball::MovingBodies;
use pin_physics::flipper::BatShape;
use pin_physics::flipper_banana::{BananaFace, ConcaveSide, CurvedBanana};
use proptest::prelude::*;

/// Proptest cases: `PROPTEST_CASES` if set, else 48 (the 2.3 convention).
fn cases() -> u32 {
    std::env::var("PROPTEST_CASES")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(48)
}

/// A banana on the 2.3 left flipper; a negative `arc_sweep_deg` puts the
/// convex face in front.
fn bent(arc_radius_mm: f32, arc_sweep_deg: f32) -> FlipperConfig {
    let mut c = banana_config(0, [0.0; 3], arc_radius_mm);
    if let FlipperShape::CurvedBanana {
        arc_sweep_deg: a, ..
    } = &mut c.shape
    {
        *a = arc_sweep_deg;
    }
    c
}

fn shape_of(config: &FlipperConfig) -> CurvedBanana {
    CurvedBanana::from_config(config, 0.03).unwrap()
}

/// Distance from `p` to the bat by sampling: the nearest of `samples + 1`
/// centreline points gives the in-plane distance to the tube, combined with
/// the flat faces. Exact up to the sampling for points outside the bat.
fn brute_force_distance(bat: &CurvedBanana, p: Vec3, samples: u32) -> f32 {
    let mut nearest = f32::INFINITY;
    for i in 0..=samples {
        let c = bat.centerline_point(i as f32 / samples as f32);
        nearest = nearest.min((p - c).truncate().length());
    }
    let side = nearest - bat.tube_radius();
    let (bottom, top) = bat.height_range();
    let above = (p.z - top).max(bottom - p.z);
    match (side > 0.0, above > 0.0) {
        (true, true) => libm::sqrtf(side * side + above * above),
        (true, false) => side,
        (false, true) => above,
        (false, false) => side.max(above),
    }
}

#[test]
fn fast_balls_bounce_off_both_faces_and_the_tip() {
    for radius_mm in [50.0f32, RADIUS_MM, 300.0] {
        for held in [false, true] {
            for speed in [1.0f32, 5.0, 15.0, 40.0, 80.0] {
                for fraction in [0.1f32, 0.5, 0.9] {
                    let config = bent(radius_mm, 30.0);
                    let bat = shape_of(&config);
                    let mut rig = space_rig(config.clone());
                    let mut angle = -30.0;
                    if held {
                        rig.flippers.set_energised(LEFT, true);
                        for _ in 0..200 {
                            rig.flippers.advance(DT);
                        }
                        angle = 20.0;
                    }
                    // From the side the stop backs: the concave (leading)
                    // face of a resting bat, the convex face of a held one.
                    let (face, toward_concave) = if held {
                        (BananaFace::Convex, -1.0)
                    } else {
                        (BananaFace::Concave, 1.0)
                    };
                    let (p, n) = bat.face_point(fraction, face);
                    // Start within the pocket's reach of the face.
                    let away = 0.03 + BALL_R;
                    let start = bat_to_world(angle, p + n * away) + v(0.0, 0.0, BALL_R);
                    let normal = bat_to_world(angle, n);
                    let h = rig.add_ball(start, normal * -speed);
                    let label = format!("R {radius_mm}, held {held}, {speed} m/s at {fraction}");
                    let before = ball_energy(&rig, h);
                    let mut hit = false;
                    for _ in 0..400 {
                        rig.step();
                        hit |= !rig.hits.is_empty();
                        let cp = rig
                            .flippers
                            .closest_point(0, 0.0, rig.ball(h).position)
                            .unwrap();
                        assert!(cp.distance > BALL_R - 1.0e-3, "{label}: inside the bat");
                        let (along, side) = arc_coords(&bat, in_bat_frame(&rig, LEFT, h));
                        if (0.0..=1.0).contains(&along) && side.abs() < 0.04 {
                            assert!(side * toward_concave > 0.0, "{label}: went through");
                        }
                        if !held {
                            // A bat on its rest stop, coil off: no energy
                            // can come out of the impact.
                            let now = ball_energy(&rig, h) + rig.flippers.kinetic_energy_j(LEFT);
                            assert!(now <= before * 1.000_01 + 1e-9, "{label}: {now} > {before}");
                        }
                    }
                    assert!(hit, "{label}: no impact");
                    assert!(
                        rig.ball(h).velocity.dot(normal) > 0.0,
                        "{label}: did not come back"
                    );
                }
                // Onto the tip cap, along the tip's tangent (pushes a
                // resting bat into its rest stop).
                if !held {
                    let config = bent(radius_mm, 30.0);
                    let bat = shape_of(&config);
                    let mut rig = space_rig(config);
                    let t = bat_to_world(-30.0, bat.centerline_tangent(1.0));
                    let tip = bat_to_world(-30.0, bat.tip_center());
                    let h = rig.add_ball(tip + t * 0.08 + v(0.0, 0.0, BALL_R), t * -speed);
                    for _ in 0..400 {
                        rig.step();
                        let b = rig.ball(h);
                        let cp = rig.flippers.closest_point(0, 0.0, b.position).unwrap();
                        assert!(cp.distance > BALL_R - 1.0e-3);
                        assert!((b.position - tip).dot(t) > 0.0, "through the tip");
                    }
                    assert!(rig.ball(h).velocity.dot(t) > 0.0, "tip, {speed} m/s");
                }
            }
        }
    }
}

#[test]
fn a_fast_bat_does_not_swing_through_a_ball() {
    // Far beyond what a flipper turns at: coils up to 33 times the default
    // on a bat of half the default inertia.
    let mut fastest = (0.0f32, 0.0f32);
    let mut cases = 0;
    for arc_sweep in [30.0f32, -30.0] {
        for radius_mm in [50.0f32, RADIUS_MM, 300.0] {
            for torque in [1.2f32, 4.0, 15.0, 40.0] {
                for fraction in [0.3f32, 0.6, 0.95] {
                    for gap_mm in [0.5f32, 5.0, 20.0, 40.0] {
                        let mut config = bent(radius_mm, arc_sweep);
                        config.power_torque_nm = torque;
                        config.inertia_kg_m2 = 0.5e-4;
                        let bat = shape_of(&config);
                        // Only where the stroke carries the face into the
                        // ball: the face moves along its own normal there
                        // (a tip curled far back moves edge-on), and half
                        // the stroke closes the gap.
                        let (p, n) = leading_face(&config, fraction);
                        let advance = Vec3::Z.cross(p).dot(n);
                        if advance < 0.7 * p.length()
                            || gap_mm * 1e-3 > 0.5 * advance * 50.0f32.to_radians()
                        {
                            continue;
                        }
                        cases += 1;
                        let (start, _) = ball_on_face(&config, -30.0, fraction, gap_mm * 1e-3);
                        let mut rig = space_rig(config);
                        let h = rig.add_ball(start, Vec3::ZERO);
                        let (_, side0) = arc_coords(&bat, in_bat_frame(&rig, LEFT, h));
                        rig.flippers.set_energised(LEFT, true);
                        let label = format!(
                            "arc {arc_sweep}, R {radius_mm}, torque {torque}, at {fraction}, gap {gap_mm} mm"
                        );
                        let mut peak = 0.0f32;
                        for _ in 0..60 {
                            rig.step();
                            peak = peak.max(rig.flippers.angular_velocity(LEFT).abs());
                            let cp = rig
                                .flippers
                                .closest_point(0, 0.0, rig.ball(h).position)
                                .unwrap();
                            assert!(
                                cp.distance > BALL_R - 1.0e-3,
                                "{label}: ball {:.2} mm inside the bat",
                                (BALL_R - cp.distance) * 1e3
                            );
                            // Still on the face it started on.
                            let (along, side) = arc_coords(&bat, in_bat_frame(&rig, LEFT, h));
                            if (0.0..=1.0).contains(&along) && side.abs() < 0.04 {
                                assert!(side * side0 > 0.0, "{label}: went through");
                            }
                        }
                        assert!(
                            rig.ball(h).velocity.length() > 0.5,
                            "{label}: the ball was hit"
                        );
                        if peak > fastest.0 {
                            fastest = (peak, peak * bat.reach());
                        }
                    }
                }
            }
        }
    }
    println!(
        "fastest bat in the sweep: {:.0} rad/s, tip {:.1} m/s",
        fastest.0, fastest.1
    );
    assert!(fastest.1 > 15.0);
    assert!(cases > 100, "the sweep covers both bends: {cases}");
}

fn tilt(axis_angle: f32, tilt_deg: f32) -> Quat {
    let axis = v(libm::cosf(axis_angle), libm::sinf(axis_angle), 0.0);
    Quat::from_axis_angle(axis, tilt_deg.to_radians())
}

/// The shot of a ball lying against the resting banana mounted in the plane
/// `frame` carries the flat one to, in the flipper's own frame.
fn shot_in_frame(frame: Quat, config: &FlipperConfig) -> (Vec3, Vec3) {
    let mut c = config.clone();
    let base = v(40.0, -25.0, 10.0);
    c.base_position_mm = (frame * base).to_array();
    c.surface_normal = (frame * Vec3::Z).to_array();
    let mut rig = Rig::with_world(
        std::slice::from_ref(&c),
        pin_physics::world::StaticWorldBuilder::new().build(),
        frame * v(0.0, -1.1, -9.7),
    );
    let flat = FlipperConfig {
        base_position_mm: base.to_array(),
        ..config.clone()
    };
    let (local, _) = ball_on_face(&flat, config.rest_angle_deg, 0.6, 1.0e-4);
    let h = rig.add_ball(frame * local, Vec3::ZERO);
    let id = c.id;
    rig.flippers.set_energised(id, true);
    rig.run(30);
    let b = rig.ball(h);
    let back = frame.inverse();
    (back * b.position, back * b.velocity)
}

#[test]
fn a_tilted_banana_is_the_flat_one_rotated() {
    let config = bent(RADIUS_MM, 30.0);
    let (p0, v0) = shot_in_frame(Quat::IDENTITY, &config);
    assert!(v0.length() > 2.0, "the reference shot launches the ball");
    for (axis, deg) in [(0.0f32, 6.5f32), (1.0, 30.0), (2.5, 75.0), (0.3, 179.0)] {
        let (p, vel) = shot_in_frame(tilt(axis, deg), &config);
        assert!((p - p0).length() < 2.0e-4, "tilt {deg}°: {p} vs {p0}");
        assert!((vel - v0).length() < 0.02, "tilt {deg}°: {vel} vs {v0}");
    }
}

#[test]
fn the_concave_face_leads_the_swing_on_either_hand() {
    // Through the solver: the pocket of a left and of a right flipper both
    // face up the table, and a negative arc sweep turns the bat over.
    for (config, expect_up) in [
        (banana_config(0, [0.0; 3], RADIUS_MM), true),
        (banana_right_config(0, [0.0; 3], RADIUS_MM), true),
        (bent(RADIUS_MM, -30.0), false),
    ] {
        let rig = space_rig(config.clone());
        let bat = shape_of(&config);
        // The tip is off the pivot's tangent toward the concave side.
        let dir = rig.flippers.bat_direction(LEFT).unwrap();
        let tip_world = bat_to_world(config.rest_angle_deg, bat.tip_center());
        let off_axis = tip_world - dir * tip_world.dot(dir);
        // "Up the table" for a bat at rest is the side it swings to.
        let swing = Vec3::Z.cross(dir) * config.sweep_angle_deg.signum();
        assert_eq!(off_axis.dot(swing) > 0.0, expect_up, "{:?}", config.shape);
        // The solver's surface is where the shape says it is.
        let cp = rig
            .flippers
            .closest_point(0, 0.0, tip_world + v(0.0, 0.0, 0.015))
            .unwrap();
        assert!((cp.distance + bat.tube_radius()).abs() < 1.0e-6);
        assert!((tip_world.length() - bat.length()).abs() < 1.0e-6);
        let expected = if (config.sweep_angle_deg >= 0.0) == expect_up {
            ConcaveSide::PlusY
        } else {
            ConcaveSide::MinusY
        };
        assert_eq!(bat.concave_side(), expected);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(cases()))]

    /// The analytic closest point agrees with brute-force sampling of the
    /// centreline everywhere outside the bat (never farther; nearer by at
    /// most the sampling error), for any bend, thickness and side.
    #[test]
    fn closest_point_matches_brute_force_sampling(
        x in -0.06f32..0.16, y in -0.14f32..0.14, z in -0.03f32..0.06,
        length in 0.03f32..0.1, bend in 0.0f32..1.0, tube in 0.003f32..0.012,
        plus in any::<bool>(),
    ) {
        // bend 0: a half circle; bend 1: 3 m radius.
        let radius = (0.5 * length).max(tube * 1.01) + bend * bend * bend * 3.0;
        let side = if plus { ConcaveSide::PlusY } else { ConcaveSide::MinusY };
        let bat = CurvedBanana::new(length, radius, tube, side, 0.0, 0.03).unwrap();
        let p = Vec3::new(x, y, z);
        let got = bat.closest_point(p);
        let samples = 3000;
        let want = brute_force_distance(&bat, p, samples);
        let slack = 0.5 * bat.arc_length() / samples as f32 + 2.0e-6;
        if want > 0.0 || got.distance > 0.0 {
            prop_assert!(got.distance <= want + 2.0e-6, "{} > {}", got.distance, want);
            prop_assert!(got.distance >= want - slack, "{} < {}", got.distance, want);
            // The reported point is that far away, along the normal, and on
            // the surface.
            prop_assert!(((p - got.point).length() - got.distance).abs() < 2.0e-6);
            prop_assert!((got.point + got.normal * got.distance - p).length() < 2.0e-6);
            prop_assert!(brute_force_distance(&bat, got.point, samples).abs() < slack + 2.0e-6);
        } else {
            // Inside: both say so.
            prop_assert!(got.distance <= 0.0 && want <= 0.0);
        }
        // Surface speed never exceeds |ω|·reach.
        let speed = bat.surface_velocity(got.point, 300.0).length();
        if got.distance >= 0.0 {
            prop_assert!(speed <= 300.0 * bat.reach() * 1.000_1);
        }
    }

    /// No tunnelling and no energy out of nothing, for any bend, ball
    /// speed, direction, flip timing, coil strength and mounting plane: the
    /// ball never sits deep inside the bat, never changes faces through the
    /// middle of the arc, and the kinetic energy of ball and bat never
    /// exceeds the start plus what the drive put in (the 2.3 bound).
    #[test]
    fn no_tunnelling_no_free_energy(
        radius_mm in 40.0f32..400.0,
        concave_leads in any::<bool>(),
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
        let mut c = bent(radius_mm, if concave_leads { 30.0 } else { -30.0 });
        c.surface_normal = (frame * Vec3::Z).to_array();
        c.power_torque_nm = torque;
        c.inertia_kg_m2 = inertia;
        let bat = shape_of(&c);
        let mut rig = space_rig(c);
        // Aim at a point of the centreline in mid stroke from 12 cm away.
        let target = bat_to_world(-5.0, bat.centerline_point(aim));
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
            // Faces: in the flipper's own frame.
            let dir = rig.flippers.bat_direction(LEFT).unwrap();
            let p = b.position - rig.flippers.pivot_m(LEFT).unwrap();
            let local = v(p.dot(dir), p.dot((frame * Vec3::Z).cross(dir)), 0.0);
            let now = arc_coords(&bat, local);
            if let Some(before) = previous {
                let middle = |a: (f32, f32)| a.0 > 0.25 && a.0 < 0.75 && a.1.abs() < 0.035;
                if middle(before) && middle(now) {
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

    /// A left banana and its mirror-image right banana play mirror-image
    /// shots for any bend and contact point.
    #[test]
    fn mirror_flippers_play_mirror_shots(
        radius_mm in 45.0f32..500.0,
        fraction in 0.15f32..0.9,
        concave_leads in any::<bool>(),
    ) {
        let arc = if concave_leads { 30.0 } else { -30.0 };
        let left = bent(radius_mm, arc);
        let right = FlipperConfig { shape: left.shape, ..right_config(0, [0.0; 3]) };
        let shoot = |config: &FlipperConfig| {
            let mut rig = Rig::new(std::slice::from_ref(config));
            let (start, _) = ball_on_face(config, config.rest_angle_deg, fraction, 2.0e-5);
            let h = rig.add_ball(start, Vec3::ZERO);
            rig.flippers.set_energised(LEFT, true);
            rig.run(25);
            (rig.ball(h).position, rig.ball(h).velocity)
        };
        let ((pl, vl), (pr, vr)) = (shoot(&left), shoot(&right));
        // (The solver itself mirrors a straight bat's shot only to about
        // 0.03 mm and 0.06 m/s: 210° and −30° do not round alike.)
        prop_assert!((pl - v(-pr.x, pr.y, pr.z)).length() < 5.0e-4, "{} vs {}", pl, pr);
        prop_assert!((vl - v(-vr.x, vr.y, vr.z)).length() < 0.1, "{} vs {}", vl, vr);
    }
}
