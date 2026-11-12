//! Subphase 2.2 verification criteria (PLAN.md §4, 2.2) and behaviour tests
//! of the ball solver against real `pin_physics::world` geometry.

#[path = "ball_util.rs"]
mod util;

use glam::Vec3;
use pin_core::ids::{ProfileId, SwitchId};
use pin_core::{BallProfile, Material};
use pin_physics::ball::{
    Ball, BallEvent, BallEvents, BallHandle, BallSolver, Dispenser, ExternalForce, Geometry,
    ImpactTarget, MAX_BALLS, Magnet, NewtonChain, Sensor, SensorKind, SurfaceVelocity,
};
use pin_physics::world::{
    CubicSpline, GuideCurve, StaticWorldBuilder, Wireform, layers,
};
use util::*;

fn run(s: &mut BallSolver, geo: &Geometry<'_>, ticks: usize) -> BallEvents {
    let mut ev = BallEvents::new();
    for _ in 0..ticks {
        s.step(geo, DT, &mut ev);
    }
    ev
}

// ---------------------------------------------------------------- criterion 1

#[test]
fn c1_identical_impulse_launch_speed_ratio_is_mass_ratio() {
    let world = floor_world();
    let geo = Geometry::new(&world);
    for impulse in [
        v(0.05, 0.0, 0.0),
        v(0.0, 0.12, 0.0),
        v(0.1, 0.1, 0.0),
        v(0.0, 0.0, 0.2),
        v(0.06, 0.0, 0.08),
    ] {
        let mut speeds = [0.0f32; 2];
        for (k, profile) in [POWERBALL, STEEL].into_iter().enumerate() {
            let mut s = solver();
            let h = s
                .add_ball(Ball::new(profile, v(0.0, 0.0, radius(profile))))
                .unwrap();
            // Let it settle, then kick.
            run(&mut s, &geo, 50);
            assert_eq!(s.ball(h).unwrap().velocity, Vec3::ZERO);
            s.apply_impulse(h, impulse);
            run(&mut s, &geo, 1);
            speeds[k] = s.ball(h).unwrap().velocity.length();
        }
        let ratio = speeds[0] / speeds[1];
        assert!(
            (ratio - 80.0 / 65.0).abs() < 0.01,
            "impulse {impulse:?}: ratio {ratio}"
        );
    }
}

// ---------------------------------------------------------------- criterion 2

fn magnet_run(profile: ProfileId, power: Option<f32>, with_steel: bool) -> Vec<Ball> {
    let world = floor_world();
    let geo = Geometry::new(&world);
    let mut s = solver();
    let r = radius(profile);
    let mut b = ball_at(profile, v(-0.12, 0.004, r), v(0.9, 0.05, 0.0));
    b.angular_velocity = v(3.0, -20.0, 7.0);
    let h = s.add_ball(b).unwrap();
    if with_steel {
        // A steel ball elsewhere on the table, pulled by the same magnet,
        // must not change the trajectory of the non-magnetic ball.
        s.add_ball(ball_at(STEEL, v(0.02, 0.09, radius(STEEL)), Vec3::ZERO))
            .unwrap();
    }
    if let Some(p) = power {
        let id = s.add_magnet(Magnet {
            position: v(0.0, 0.0, -0.005),
            k_coil: 2.0e-5,
            d_min: 0.01,
            range: 0.15,
            layer_mask: layers::ALL,
        });
        s.set_magnet_power(id, p);
    }
    let mut out = Vec::new();
    let mut ev = BallEvents::new();
    for _ in 0..400 {
        s.step(&geo, DT, &mut ev);
        out.push(*s.ball(h).unwrap());
    }
    out
}

#[test]
fn c2_non_magnetic_balls_ignore_an_active_magnet_bit_for_bit() {
    for profile in [POWERBALL, GLOBALL] {
        let off = magnet_run(profile, None, false);
        let zero = magnet_run(profile, Some(0.0), false);
        let on = magnet_run(profile, Some(1.0), false);
        // `Ball: PartialEq` compares every float with `==`.
        assert_eq!(off, on, "{profile:?}");
        assert_eq!(off, zero, "{profile:?}");
        for (a, b) in off.iter().zip(&on) {
            assert_eq!(a.position.to_array().map(f32::to_bits), b.position.to_array().map(f32::to_bits));
            assert_eq!(a.velocity.to_array().map(f32::to_bits), b.velocity.to_array().map(f32::to_bits));
        }
        // Also with a magnetic ball on the table (which does get pulled).
        let off = magnet_run(profile, None, true);
        let on = magnet_run(profile, Some(1.0), true);
        assert_eq!(off, on, "{profile:?} with a steel ball present");
    }
    // The magnet is really on: a steel ball is deflected by it.
    let off = magnet_run(STEEL, None, false);
    let on = magnet_run(STEEL, Some(1.0), false);
    assert!(off.last().unwrap().position.distance(on.last().unwrap().position) > 1.0e-3);
}

#[test]
fn magnet_force_follows_the_inverse_cube_law_and_range() {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let m = 0.080f32;
    let k = 1.0e-6f32;
    let accel = |d: f32, power: f32, d_min: f32, range: f32, layer: u8| {
        let mut cfg = config();
        cfg.gravity = Vec3::ZERO;
        let mut s = BallSolver::new(cfg, &profiles());
        let mut b = Ball::new(STEEL, v(d, 0.0, 0.0));
        b.layer = layer;
        let h = s.add_ball(b).unwrap();
        let id = s.add_magnet(Magnet {
            position: Vec3::ZERO,
            k_coil: k,
            d_min,
            range,
            layer_mask: layers::MAIN_DECK,
        });
        s.set_magnet_power(id, power);
        assert_eq!(s.magnet_power(id), power.clamp(0.0, 1.0));
        run(&mut s, &geo, 1);
        s.ball(h).unwrap().velocity / DT
    };
    // Toward the magnet, K / d³ / m.
    let a = accel(0.05, 1.0, 0.01, 0.2, 0);
    assert!((a.x + k / 0.05f32.powi(3) / m).abs() < 1e-3 * a.x.abs(), "{a:?}");
    assert_eq!((a.y, a.z), (0.0, 0.0));
    // Power scales linearly and is clamped.
    let half = accel(0.05, 0.5, 0.01, 0.2, 0);
    assert!((half.x / a.x - 0.5).abs() < 1e-4);
    assert_eq!(accel(0.05, 7.0, 0.01, 0.2, 0), a);
    assert_eq!(accel(0.05, -1.0, 0.01, 0.2, 0), Vec3::ZERO);
    // Inside d_min the force is capped.
    let capped = accel(0.005, 1.0, 0.01, 0.2, 0);
    assert!((capped.x + k / 0.01f32.powi(3) / m).abs() < 1e-3 * capped.x.abs());
    // Beyond the range, and on another layer: exactly nothing.
    assert_eq!(accel(0.21, 1.0, 0.01, 0.2, 0), Vec3::ZERO);
    assert_eq!(accel(0.05, 1.0, 0.01, 0.2, 1), Vec3::ZERO);
    // Unknown ids are ignored.
    let mut s = solver();
    s.set_magnet_power(pin_core::ids::MagnetId(9), 1.0);
    assert_eq!(s.magnet_power(pin_core::ids::MagnetId(9)), 0.0);
}

// ---------------------------------------------------------------- criterion 3

fn straight_path(z: f32) -> CubicSpline {
    let d = 2.0 * radius(STEEL);
    let pts: Vec<Vec3> = (0..12).map(|i| v(d * i as f32, 0.0, z)).collect();
    CubicSpline::interpolate(&pts).unwrap()
}

#[test]
fn c3_strike_reaches_the_far_newton_ball_within_two_ticks() {
    let world = empty_world();
    let geo = Geometry::new(&world);
    for n in 3..=6usize {
        let params: Vec<f32> = (0..n).map(|i| 3.0 + i as f32).collect();
        let mut s = solver();
        let c = s.add_newton_chain(
            NewtonChain::new(straight_path(0.0135), STEEL, &params).unwrap(),
        );
        assert_eq!(s.newton_chain_count(), 1);
        run(&mut s, &geo, 5);
        assert_eq!(s.newton_chain(c).ball_speed(n - 1), 0.0);
        s.newton_chain_mut(c).strike(0, 1.5);
        run(&mut s, &geo, 2);
        assert!(s.newton_chain(c).ball_speed(n - 1) > 0.1, "{n} balls");
    }
}

#[test]
fn c3_free_ball_impact_reaches_the_far_newton_ball_within_two_ticks() {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let r = radius(STEEL);
    let mut s = solver_no_gravity();
    let c = s.add_newton_chain(
        NewtonChain::new(straight_path(0.0), STEEL, &[4.0, 5.0, 6.0]).unwrap(),
    );
    // First captive ball at x = 8r; the free ball comes in along the path.
    let h = s
        .add_ball(ball_at(STEEL, v(8.0 * r - 2.0 * r - 0.004, 0.0, 0.0), v(2.0, 0.0, 0.0)))
        .unwrap();
    let mut ev = BallEvents::new();
    let mut hit_tick = None;
    for tick in 0..10 {
        s.step(&geo, DT, &mut ev);
        if hit_tick.is_none() && s.ball(h).unwrap().velocity.x < 1.99 {
            hit_tick = Some(tick);
            // Same tick: the impulse is already at the far ball.
            assert!(s.newton_chain(c).ball_speed(2) > 0.1);
        }
    }
    assert_eq!(hit_tick, Some(2));
    assert!(ev.as_slice().iter().any(|e| matches!(
        e,
        BallEvent::Impact { other: ImpactTarget::Newton(0), .. }
    )));
    // Momentum along the path is conserved between the free and captive balls.
    let m = 0.08f32;
    let chain = s.newton_chain(c);
    let p: f32 = (0..3).map(|i| chain.ball_speed(i)).sum::<f32>() * m
        + s.ball(h).unwrap().velocity.x * m;
    assert!((p - 2.0 * m).abs() < 1e-5);
    // The free ball never passes through the captive ball.
    assert!(s.ball(h).unwrap().position.x < chain.ball_position(0).x - 2.0 * r + 1e-4);
}

// ---------------------------------------------------------------- criterion 4

#[test]
fn c4_mini_vs_jumbo_free_collision_conserves_momenta() {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let (rm, rj) = (radius(MINI), radius(JUMBO));
    let mut collisions = 0;
    for offset in [-0.9f32, -0.6, -0.3, 0.0, 0.2, 0.5, 0.8, 0.95] {
        for (wm, wj) in [
            (Vec3::ZERO, Vec3::ZERO),
            (v(0.0, 0.0, 300.0), Vec3::ZERO),
            (v(50.0, -80.0, 20.0), v(-30.0, 10.0, 90.0)),
            (v(0.0, 400.0, 0.0), v(0.0, 0.0, -150.0)),
        ] {
            for speed in [0.5f32, 3.0, 12.0] {
                let mut s = solver_no_gravity();
                let mut mini = ball_at(
                    MINI,
                    v(-0.05, offset * (rm + rj), 0.001),
                    v(speed, 0.0, 0.0),
                );
                mini.angular_velocity = wm;
                let mut jumbo = ball_at(JUMBO, v(0.03, 0.0, 0.0), v(-0.3 * speed, 0.1, 0.0));
                jumbo.angular_velocity = wj;
                s.add_ball(mini).unwrap();
                s.add_ball(jumbo).unwrap();
                let (p0, l0, e0) = (momentum(&s), angular_momentum(&s), kinetic_energy(&s));
                let mut ev = BallEvents::new();
                let mut e_prev = e0;
                for _ in 0..200 {
                    s.step(&geo, DT, &mut ev);
                    let e = kinetic_energy(&s);
                    assert!(e <= e_prev * (1.0 + 1e-6), "energy rose: {e_prev} -> {e}");
                    e_prev = e;
                    let p = momentum(&s);
                    assert!((p - p0).length() <= 1e-4 * p0.length());
                    let l = angular_momentum(&s);
                    assert!(
                        (l - l0).length() <= 1e-3 * l0.length().max(1e-7),
                        "L {l0:?} -> {l:?} (offset {offset}, speed {speed})"
                    );
                }
                if !ev.as_slice().is_empty() {
                    collisions += 1;
                    assert!(matches!(
                        ev.as_slice()[0],
                        BallEvent::Impact {
                            ball: BallHandle(0),
                            other: ImpactTarget::Ball(BallHandle(1)),
                            ..
                        }
                    ));
                    assert!(kinetic_energy(&s) < e0);
                }
            }
        }
    }
    assert!(collisions > 60, "{collisions}");
}

// ---------------------------------------------------------------- criterion 5

#[test]
fn c5_spinning_ball_converges_to_rolling_and_stays() {
    let world = floor_world();
    let geo = Geometry::new(&world);
    for profile in [STEEL, POWERBALL, GLOBALL, MINI, JUMBO] {
        for (vel, spin, drop) in [
            (Vec3::ZERO, v(0.0, 150.0, 0.0), 0.02f32),
            (v(0.5, 0.0, 0.0), v(100.0, -200.0, 30.0), 0.05),
            (v(-0.3, 0.8, 0.0), v(0.0, 0.0, 250.0), 0.0),
            (v(1.0, 0.0, 0.0), Vec3::ZERO, 0.0),
        ] {
            let r = radius(profile);
            let mut s = solver();
            let mut b = ball_at(profile, v(0.0, 0.0, r + drop), vel);
            b.angular_velocity = spin;
            let h = s.add_ball(b).unwrap();
            let slip = |s: &BallSolver| {
                let b = s.ball(h).unwrap();
                (b.velocity + b.angular_velocity.cross(v(0.0, 0.0, -r))).length()
            };
            let mut ev = BallEvents::new();
            let mut converged = None;
            for tick in 0..3000 {
                s.step(&geo, DT, &mut ev);
                let b = s.ball(h).unwrap();
                let grounded = b.position.z - r < 1e-5;
                match converged {
                    None if grounded && slip(&s) < 1e-3 && tick > 300 => converged = Some(tick),
                    Some(_) => {
                        assert!(slip(&s) < 1e-3, "{profile:?} left rolling at tick {tick}");
                        assert!(b.position.z - r < 1e-4 && b.position.z - r > -1e-5);
                    }
                    _ => {}
                }
            }
            assert!(converged.is_some(), "{profile:?} {vel:?} {spin:?} never rolled");
        }
    }
}

// ------------------------------------------------------------------ behaviour

#[test]
fn resting_ball_does_not_drift() {
    let world = floor_world();
    let geo = Geometry::new(&world);
    let mut s = solver();
    let handles: Vec<_> = [STEEL, POWERBALL, GLOBALL, MINI, JUMBO, ProfileId(6)]
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            let pos = v(0.2 * i as f32 - 0.5, 0.1, radius(p));
            (s.add_ball(Ball::new(p, pos)).unwrap(), pos)
        })
        .collect();
    let ev = run(&mut s, &geo, 10_000);
    for (h, pos) in handles {
        let b = s.ball(h).unwrap();
        assert!(b.position.distance(pos) < 1e-6, "{h:?} drifted");
        assert_eq!(b.velocity, Vec3::ZERO);
    }
    assert!(ev.as_slice().is_empty());
}

#[test]
fn drop_bounce_follows_the_pair_table() {
    // Steel on wood: restitution 0.25 from the table, not 0.65 from the profile.
    let world = floor_world();
    let geo = Geometry::new(&world);
    let r = radius(STEEL);
    let mut s = solver();
    let h = s.add_ball(Ball::new(STEEL, v(0.0, 0.0, r + 0.2))).unwrap();
    let mut ev = BallEvents::new();
    let mut apex = 0.0f32;
    let mut bounced = false;
    for _ in 0..600 {
        s.step(&geo, DT, &mut ev);
        let b = s.ball(h).unwrap();
        assert!(b.position.z > r - 1e-5, "tunnelled into the floor");
        if !ev.as_slice().is_empty() {
            bounced = true;
        }
        if bounced && ev.as_slice().len() == 1 {
            apex = apex.max(b.position.z - r);
        }
    }
    // First impact at sqrt(2 g h) = 1.98 m/s.
    let BallEvent::Impact { normal_speed, other, .. } = ev.as_slice()[0] else {
        panic!("no impact");
    };
    assert!((normal_speed - 1.981).abs() < 0.02);
    assert!(matches!(other, ImpactTarget::Collider(_)));
    // Rebound height e² h.
    assert!((apex - 0.0625 * 0.2).abs() < 1.0e-3, "apex {apex}");
    // And it comes to rest.
    run(&mut s, &geo, 2000);
    assert_eq!(s.ball(h).unwrap().velocity, Vec3::ZERO);
    // A rubber floor bounces higher (0.65).
    let mut s = solver();
    let mut b = StaticWorldBuilder::new();
    let c = b.add_collider(cuboid(v(0.0, 0.0, -0.05), v(1.0, 1.0, 0.05)), layers::ALL, true);
    let rubber = b.build();
    s.set_collider_material(c, Material::Rubber);
    let h = s.add_ball(Ball::new(STEEL, v(0.0, 0.0, r + 0.2))).unwrap();
    let geo = Geometry::new(&rubber);
    let mut ev = BallEvents::new();
    let mut after = 0.0f32;
    for _ in 0..260 {
        s.step(&geo, DT, &mut ev);
        if ev.as_slice().len() == 1 {
            after = after.max(s.ball(h).unwrap().velocity.z);
        }
    }
    assert!((after - 0.65 * 1.981).abs() < 0.03, "{after}");
}

#[test]
fn missing_pair_falls_back_to_profile_restitution() {
    // Resin on acrylic is not in the table: profile restitution 0.80.
    let mut b = StaticWorldBuilder::new();
    let c = b.add_collider(cuboid(v(0.0, 0.0, -0.05), v(1.0, 1.0, 0.05)), layers::ALL, true);
    let world = b.build();
    let geo = Geometry::new(&world);
    let xl = ProfileId(6);
    let r = radius(xl);
    let mut s = solver();
    s.set_collider_material(c, Material::Acrylic);
    let h = s.add_ball(Ball::new(xl, v(0.0, 0.0, r + 0.1))).unwrap();
    let mut ev = BallEvents::new();
    let mut up = 0.0f32;
    for _ in 0..200 {
        s.step(&geo, DT, &mut ev);
        if ev.as_slice().len() == 1 {
            up = up.max(s.ball(h).unwrap().velocity.z);
        }
    }
    assert!((up - 0.8 * 1.4007).abs() < 0.03, "{up}");
}

#[test]
fn rolling_ball_decelerates_by_rolling_resistance_and_stops() {
    let world = floor_world();
    let geo = Geometry::new(&world);
    let r = radius(STEEL);
    let mut s = solver();
    let mut b = ball_at(STEEL, v(-1.0, 0.0, r), v(0.2, 0.0, 0.0));
    b.angular_velocity = v(0.0, 0.2 / r, 0.0);
    let h = s.add_ball(b).unwrap();
    run(&mut s, &geo, 1000);
    // a = 5/7 · c · g.
    let expected = 0.2 - (5.0 / 7.0) * 0.002 * 9.81;
    let vx = s.ball(h).unwrap().velocity.x;
    assert!((vx - expected).abs() < 2e-4, "{vx} vs {expected}");
    run(&mut s, &geo, 14_500);
    assert_eq!(s.ball(h).unwrap().velocity, Vec3::ZERO);
    assert_eq!(s.ball(h).unwrap().angular_velocity, Vec3::ZERO);
}

#[test]
fn ball_rolls_down_an_incline_at_five_sevenths_g_sin() {
    // Pitch comes from geometry: a floor tilted 6.5° about X.
    let pitch = 6.5f32.to_radians();
    let rot = glam::Quat::from_rotation_x(pitch);
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        pin_physics::world::StaticShape::Cuboid {
            center: rot * v(0.0, 0.0, -0.05),
            half_extents: v(1.0, 3.0, 0.05),
            rotation: rot,
        },
        layers::ALL,
        true,
    );
    let world = b.build();
    let geo = Geometry::new(&world);
    let r = radius(STEEL);
    let mut s = solver();
    let h = s.add_ball(Ball::new(STEEL, rot * v(0.0, 1.0, r))).unwrap();
    run(&mut s, &geo, 1000);
    let ball = s.ball(h).unwrap();
    let down = rot * v(0.0, -1.0, 0.0);
    let speed = ball.velocity.dot(down);
    let expected = (5.0 / 7.0) * 9.81 * (libm::sinf(pitch) - 0.002 * libm::cosf(pitch));
    assert!((speed - expected).abs() < 0.01 * expected, "{speed} vs {expected}");
    // Rolling without slipping, and still on the surface.
    let n = rot * Vec3::Z;
    let slip = ball.velocity + ball.angular_velocity.cross(n * -r);
    assert!(slip.length() < 1e-3);
    let height = (ball.position - rot * Vec3::ZERO).dot(n) - r;
    assert!(height.abs() < 1e-5, "{height}");
}

#[test]
fn ball_hugs_a_concave_guide_without_bouncing() {
    // 180° guide arc of radius 150 mm on a floor; ball enters along the wall.
    let world = floor_world();
    let radius_g = 0.150f32;
    let step = 2.0 * radius_g * libm::sinf(2.5f32.to_radians());
    let mut pts = Vec::new();
    for k in (1..=20).rev() {
        pts.push(v(radius_g, -step * k as f32, 0.0));
    }
    for k in 0..=36 {
        let a = (5.0 * k as f32).to_radians();
        pts.push(v(radius_g * libm::cosf(a), radius_g * libm::sinf(a), 0.0));
    }
    for k in 1..=20 {
        pts.push(v(-radius_g, -step * k as f32, 0.0));
    }
    let guides = [GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), 0.03).unwrap()];
    let geo = Geometry {
        world: &world,
        guides: &guides,
        wireforms: &[],
    };
    for speed in [1.0f32, 4.0, 8.0] {
        let r = radius(STEEL);
        let mut s = solver();
        let mut b = ball_at(STEEL, v(radius_g - r - 1e-4, -0.2, r), v(0.0, speed, 0.0));
        b.angular_velocity = v(-speed / r, 0.0, 0.0);
        let h = s.add_ball(b).unwrap();
        let mut ev = BallEvents::new();
        let mut max_gap = 0.0f32;
        let mut exited = false;
        for _ in 0..2000 {
            s.step(&geo, DT, &mut ev);
            let b = s.ball(h).unwrap();
            let cp = guides[0].closest_point(b.position, None);
            assert!(cp.distance - r > -3.0e-5, "sank into the guide");
            if b.position.y > 0.01 {
                // Beside the arc: inside it, hugging the wall.
                let rad = v(b.position.x, b.position.y, 0.0).length();
                assert!(rad < radius_g - r + 3.0e-5, "outside the guide: {rad}");
                max_gap = max_gap.max(radius_g - r - rad);
            }
            if b.position.x < 0.0 && b.position.y < -0.1 {
                exited = true;
                // Came out heading -Y having lost only friction energy.
                assert!(b.velocity.y < -0.5 * speed, "{:?}", b.velocity);
                break;
            }
        }
        assert!(exited, "speed {speed}: never left the arc");
        assert!(max_gap < 2.0e-4, "speed {speed}: bounced {max_gap} off the wall");
        // Sliding along the wall is not an impact.
        let wall_hits = ev
            .as_slice()
            .iter()
            .filter(|e| matches!(e, BallEvent::Impact { other: ImpactTarget::Guide(_), .. }))
            .count();
        assert!(wall_hits <= 2, "speed {speed}: {wall_hits} guide impacts");
        assert_eq!(ev.dropped(), 0);
    }
    // Head-on into a guide: bounces with steel–steel restitution 0.6.
    let r = radius(STEEL);
    let mut s = solver_no_gravity();
    let h = s
        .add_ball(ball_at(STEEL, v(0.0, 0.05, 0.015), v(0.0, 2.0, 0.0)))
        .unwrap();
    let ev = run(&mut s, &geo, 100);
    let b = s.ball(h).unwrap();
    assert!((b.velocity.y + 1.2).abs() < 0.01, "{:?}", b.velocity);
    assert!(b.position.y < radius_g - r);
    assert!(matches!(
        ev.as_slice()[0],
        BallEvent::Impact { other: ImpactTarget::Guide(0), .. }
    ));
    // A guide on another layer is ignored.
    let mut s = solver_no_gravity();
    s.set_guide_layers(0, layers::UPPER_DECK);
    s.set_guide_material(0, Material::Rubber);
    let h = s
        .add_ball(ball_at(STEEL, v(0.0, 0.05, 0.015), v(0.0, 2.0, 0.0)))
        .unwrap();
    run(&mut s, &geo, 100);
    assert!(s.ball(h).unwrap().position.y > radius_g);
}

#[test]
fn ball_rides_a_wireform_without_falling_through() {
    // Straight, gently rising wireform; the ball starts seated on the rails.
    let gauge = 0.020f32;
    let r = radius(STEEL);
    let seat = (r * r - 0.25 * gauge * gauge).sqrt();
    let pts: Vec<Vec3> = (0..=30)
        .map(|k| {
            let d = 0.05 * k as f32;
            v(0.0, d, 0.05 + 0.04 * d)
        })
        .collect();
    let rolls = vec![0.0; pts.len()];
    let wires = [Wireform::new(CubicSpline::interpolate(&pts).unwrap(), &rolls, gauge, Vec3::Z).unwrap()];
    let world = empty_world();
    let geo = Geometry {
        world: &world,
        guides: &[],
        wireforms: &wires,
    };
    for speed in [0.0f32, 1.5, 6.0] {
        let mut s = solver();
        s.set_wireform_material(0, Material::Steel);
        let start = wires[0].centerline().point(2.0) + v(0.0, 0.0, seat + 1e-4);
        let h = s.add_ball(ball_at(STEEL, start, v(0.0, speed, 0.04 * speed))).unwrap();
        let mut ev = BallEvents::new();
        for tick in 0..400 {
            s.step(&geo, DT, &mut ev);
            let b = s.ball(h).unwrap();
            if b.position.y > 1.45 || b.position.y < 0.02 {
                break;
            }
            let cp = wires[0].centerline().closest_point(b.position, None);
            let height = b.position.z - cp.point.z;
            assert!(
                height > seat - 1.0e-3 && height < seat + 5.0e-3,
                "speed {speed} tick {tick}: height {height} (seat {seat})"
            );
            assert!(b.position.x.abs() < 1.0e-3);
        }
        let b = s.ball(h).unwrap();
        if speed == 0.0 {
            // Released at rest it rolls back down the slope.
            assert!(b.velocity.y < -0.05);
        } else {
            assert!(b.position.y > 0.3);
        }
    }
    // On a layer the wireform is not on, the ball falls straight through.
    let mut s = solver();
    s.set_wireform_layers(0, layers::WIREFORM);
    let start = wires[0].centerline().point(2.0) + v(0.0, 0.0, seat + 1e-4);
    let h = s.add_ball(Ball::new(STEEL, start)).unwrap();
    run(&mut s, &geo, 300);
    assert!(s.ball(h).unwrap().position.z < -0.2);
}

#[test]
fn stacked_and_touching_balls_settle() {
    // Three balls side by side in a narrow box, a fourth dropped on top.
    let r = radius(STEEL);
    let world = box_world(v(3.0 * r + 0.0005, r + 0.0005, 0.2));
    let geo = Geometry::new(&world);
    let mut s = solver();
    for i in 0..3 {
        s.add_ball(Ball::new(STEEL, v((i as f32 - 1.0) * 2.0 * r, 0.0, -0.2 + r)))
            .unwrap();
    }
    let top = s.add_ball(Ball::new(STEEL, v(r, 0.0, -0.2 + 3.0 * r + 0.01))).unwrap();
    run(&mut s, &geo, 3000);
    for (_, b) in s.balls() {
        assert!(b.velocity.length() < 5e-3, "{:?}", b.velocity);
        assert!(b.position.x.abs() < 3.0 * r + 0.001);
        assert!(b.position.z > -0.2 + r - 1e-4);
    }
    // The top ball rests in the groove between two lower balls.
    let z = s.ball(top).unwrap().position.z;
    let expected = -0.2 + r + libm::sqrtf(3.0) * r;
    assert!((z - expected).abs() < 1.0e-3, "{z} vs {expected}");
    // No overlaps.
    let all: Vec<Ball> = s.balls().map(|(_, b)| *b).collect();
    for (i, a) in all.iter().enumerate() {
        for b in &all[i + 1..] {
            assert!(a.position.distance(b.position) > 2.0 * r - 2.0e-5);
        }
    }
}

#[test]
fn balls_on_different_layers_do_not_collide() {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut s = solver_no_gravity();
    let a = s.add_ball(ball_at(STEEL, v(-0.05, 0.0, 0.0), v(1.0, 0.0, 0.0))).unwrap();
    let mut other = ball_at(STEEL, v(0.05, 0.0, 0.0), v(-1.0, 0.0, 0.0));
    other.layer = 1;
    let b = s.add_ball(other).unwrap();
    let ev = run(&mut s, &geo, 100);
    assert!(ev.as_slice().is_empty());
    assert_eq!(s.ball(a).unwrap().velocity.x, 1.0);
    assert_eq!(s.ball(b).unwrap().velocity.x, -1.0);
    // The orientation follows the spin.
    s.ball_mut(a).unwrap().angular_velocity = v(0.0, 0.0, std::f32::consts::PI);
    run(&mut s, &geo, 500);
    let q = s.ball(a).unwrap().orientation;
    assert!(((q * Vec3::X) - Vec3::Y).length() < 1e-3);
}

#[test]
fn sensors_follow_conductivity_and_report_edges() {
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut s = solver_no_gravity();
    let sensor = |switch: u16, kind, center, layer_mask| Sensor {
        switch: SwitchId(switch),
        kind,
        center,
        radius: 0.02,
        layer_mask,
    };
    s.add_sensor(sensor(3, SensorKind::Eddy, Vec3::ZERO, layers::ALL));
    s.add_sensor(sensor(70, SensorKind::Opto, Vec3::ZERO, layers::ALL));
    s.add_sensor(sensor(200, SensorKind::Opto, Vec3::ZERO, layers::UPPER_DECK));
    s.add_sensor(sensor(300, SensorKind::Opto, Vec3::ZERO, layers::ALL));
    // Two sensors on one switch: either closes it.
    s.add_sensor(sensor(5, SensorKind::Opto, v(1.0, 0.0, 0.0), layers::ALL));
    s.add_sensor(sensor(5, SensorKind::Opto, Vec3::ZERO, layers::ALL));

    let bits = |s: &BallSolver| {
        let mut out = [u64::MAX; 4];
        // Foreign bits are left alone.
        s.sensor_switches(&mut out);
        out
    };
    let own = (1u64 << 3) | (1 << 5);
    assert_eq!(bits(&s), [!own, !(1 << 6), u64::MAX, !(1 << 8)]);

    // A Powerball passes: the opto closes, the eddy sensor does not.
    let h = s.add_ball(ball_at(POWERBALL, v(-0.05, 0.0, 0.0), v(1.0, 0.0, 0.0))).unwrap();
    let mut ev = BallEvents::new();
    let mut seen = Vec::new();
    for _ in 0..100 {
        ev.clear();
        s.step(&geo, DT, &mut ev);
        seen.extend_from_slice(ev.as_slice());
        if s.ball(h).unwrap().position.x.abs() < 0.01 {
            let b = bits(&s);
            assert_eq!(b[0] & own, 1 << 5);
            assert_ne!(b[1] & (1 << 6), 0);
            assert_eq!(b[3] & (1 << 8), 0);
        }
    }
    let edge = |switch: u16, closed| BallEvent::SensorEdge { switch: SwitchId(switch), closed };
    assert_eq!(
        seen,
        vec![edge(70, true), edge(300, true), edge(5, true), edge(70, false), edge(300, false), edge(5, false)]
    );
    assert_eq!(bits(&s)[0] & own, 0);

    // A steel ball closes the eddy sensor as well.
    s.remove_ball(h).unwrap();
    s.add_ball(Ball::new(STEEL, v(0.001, 0.0, 0.0))).unwrap();
    ev.clear();
    s.step(&geo, DT, &mut ev);
    assert!(ev.as_slice().contains(&edge(3, true)));
    assert_ne!(bits(&s)[0] & (1 << 3), 0);
}

#[test]
fn slots_handles_and_dispenser() {
    let mut s = solver();
    assert!(s.add_ball(Ball::new(ProfileId(99), Vec3::ZERO)).is_none());
    let mut d = Dispenser::new(v(0.1, 0.2, 0.3), v(0.0, 1.0, 0.0));
    d.exit_layer = 2;
    assert!(d.is_empty());
    assert!(d.pop().is_none());
    assert!(d.dispense(&mut s).is_none());
    for i in 0..16 {
        assert!(d.push(ProfileId(i % 7)));
    }
    assert!(!d.push(STEEL));
    assert_eq!(d.len(), 16);
    assert_eq!(d.pop(), Some(ProfileId(0)));
    let h = d.dispense(&mut s).unwrap();
    assert_eq!(h, BallHandle(0));
    let b = *s.ball(h).unwrap();
    assert_eq!((b.profile, b.position, b.velocity, b.layer), (ProfileId(1), v(0.1, 0.2, 0.3), v(0.0, 1.0, 0.0), 2));
    while d.dispense(&mut s).is_some() {}
    assert_eq!(d.len(), 0);
    assert_eq!(s.ball_count(), 15);
    // Fill the last slot; a full solver refuses and the dispenser keeps its ball.
    assert_eq!(s.add_ball(Ball::new(STEEL, Vec3::ZERO)), Some(BallHandle(15)));
    assert!(s.add_ball(Ball::new(STEEL, Vec3::ZERO)).is_none());
    assert_eq!(s.ball_count(), MAX_BALLS);
    d.push(MINI);
    assert!(d.dispense(&mut s).is_none());
    assert_eq!(d.len(), 1);
    // Freed slots are reused, lowest first; handles iterate in order.
    assert_eq!(s.remove_ball(BallHandle(4)).unwrap().profile, ProfileId(5));
    assert!(s.remove_ball(BallHandle(4)).is_none());
    assert!(s.remove_ball(BallHandle(200)).is_none());
    assert!(s.ball(BallHandle(4)).is_none() && s.ball_mut(BallHandle(77)).is_none());
    assert_eq!(d.dispense(&mut s), Some(BallHandle(4)));
    let order: Vec<u8> = s.balls().map(|(h, _)| h.0).collect();
    assert_eq!(order, (0..16).collect::<Vec<u8>>());
    // Impulses on missing balls or with NaN are ignored.
    s.apply_impulse(BallHandle(99), Vec3::X);
    s.apply_impulse(BallHandle(0), v(f32::NAN, 0.0, 0.0));
    assert_eq!(s.ball(BallHandle(0)).unwrap().velocity, v(0.0, 1.0, 0.0));
    assert_eq!(s.config().max_substeps, 64);
}

struct Wind(Vec3);
impl ExternalForce for Wind {
    fn force(&mut self, _h: BallHandle, _b: &Ball, profile: &BallProfile, dt: f32) -> Vec3 {
        assert!(dt > 0.0 && dt <= DT);
        self.0 * profile.mass_kg
    }
}

#[test]
fn external_force_hooks() {
    let world = floor_world();
    let geo = Geometry::new(&world);
    let scenario = |hook: Option<Vec3>| {
        let mut s = solver();
        if let Some(a) = hook {
            s.add_external_force(Box::new(Wind(a)));
        }
        let r = radius(GLOBALL);
        let mut b = ball_at(GLOBALL, v(0.0, 0.0, r + 0.03), v(0.4, 0.1, 0.0));
        b.angular_velocity = v(10.0, 20.0, 30.0);
        let h = s.add_ball(b).unwrap();
        run(&mut s, &geo, 500);
        *s.ball(h).unwrap()
    };
    // A hook returning zero changes nothing; a real one does.
    let core = scenario(None);
    assert_eq!(scenario(Some(Vec3::ZERO)), core);
    assert!(scenario(Some(v(0.0, 2.0, 0.0))).position.y > core.position.y + 0.05);
    // In free flight the hook is an exact extra acceleration.
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut s = solver_no_gravity();
    s.add_external_force(Box::new(Wind(v(0.0, 0.0, 3.0))));
    let h = s.add_ball(Ball::new(JUMBO, Vec3::ZERO)).unwrap();
    run(&mut s, &geo, 1000);
    assert!((s.ball(h).unwrap().velocity.z - 3.0).abs() < 1e-3);
}

struct Belt;
impl SurfaceVelocity for Belt {
    fn velocity_at(&self, target: ImpactTarget, _point: Vec3) -> Vec3 {
        assert!(matches!(target, ImpactTarget::Collider(_)));
        v(0.3, 0.0, 0.0)
    }
}

#[test]
fn moving_surface_carries_the_ball() {
    let world = floor_world();
    let geo = Geometry::new(&world);
    let mut s = solver();
    s.set_surface_velocity(Some(Box::new(Belt)));
    let h = s.add_ball(Ball::new(STEEL, v(0.0, 0.0, radius(STEEL)))).unwrap();
    run(&mut s, &geo, 2000);
    let b = s.ball(h).unwrap();
    assert!((b.velocity.x - 0.3).abs() < 0.01, "{:?}", b.velocity);
    s.set_surface_velocity(None);
    s.set_gravity(Vec3::ZERO);
    run(&mut s, &geo, 10);
    assert!((s.ball(h).unwrap().velocity.x - 0.3).abs() < 0.01);
}

#[test]
fn degenerate_input_is_ignored() {
    let world = floor_world();
    let geo = Geometry::new(&world);
    let mut s = solver();
    let good = s.add_ball(Ball::new(STEEL, v(0.0, 0.0, 0.1))).unwrap();
    let bad = s.add_ball(Ball::new(STEEL, v(f32::NAN, 0.0, 0.1))).unwrap();
    let mut ev = BallEvents::new();
    for dt in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        s.step(&geo, dt, &mut ev);
    }
    assert_eq!(s.ball(good).unwrap().position, v(0.0, 0.0, 0.1));
    s.ball_mut(good).unwrap().profile = ProfileId(50);
    run(&mut s, &geo, 10);
    assert_eq!(s.ball(good).unwrap().position, v(0.0, 0.0, 0.1));
    assert!(s.ball(bad).unwrap().position.x.is_nan());
    // A solver with a zero sub-step cap still steps.
    let mut cfg = config();
    cfg.max_substeps = 0;
    let mut s = BallSolver::new(cfg, &profiles());
    let h = s.add_ball(Ball::new(STEEL, v(0.0, 0.0, 0.1))).unwrap();
    run(&mut s, &geo, 300);
    assert!((s.ball(h).unwrap().position.z - radius(STEEL)).abs() < 1e-3);
}
