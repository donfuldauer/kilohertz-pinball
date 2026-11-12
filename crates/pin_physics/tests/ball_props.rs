//! Property tests of the ball solver invariants (gate step 5).

#[path = "ball_util.rs"]
mod util;

use glam::Vec3;
use pin_core::ids::ProfileId;
use pin_physics::ball::{Ball, BallEvents, BallHandle, Dispenser, Geometry, Magnet, NewtonChain};
use pin_physics::world::{CubicSpline, GuideCurve, StaticShape, StaticWorldBuilder, layers};
use proptest::prelude::*;
use util::*;

fn vec3(range: f32) -> impl Strategy<Value = Vec3> {
    (-range..range, -range..range, -range..range).prop_map(|(x, y, z)| Vec3::new(x, y, z))
}

/// One ball of a multi-ball scene: profile, velocity, spin.
fn ball_spec() -> impl Strategy<Value = (u16, Vec3, Vec3)> {
    (0u16..6, vec3(6.0), vec3(300.0))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Gravity off: every contact (walls, edges, other balls) is frictional
    /// and inelastic, so kinetic energy never increases, and nothing leaves
    /// the box.
    #[test]
    fn energy_never_increases_without_gravity(
        specs in prop::collection::vec(ball_spec(), 1..5),
    ) {
        let half = v(0.25, 0.15, 0.06);
        let mut b = StaticWorldBuilder::new();
        // The closed box plus a post and a slanted block inside it.
        let t = 0.05;
        for axis in 0..3 {
            for sign in [-1.0f32, 1.0] {
                let mut c = Vec3::ZERO;
                c[axis] = sign * (half[axis] + t);
                let mut he = half + Vec3::splat(2.0 * t);
                he[axis] = t;
                b.add_collider(cuboid(c, he), layers::ALL, true);
            }
        }
        b.add_collider(
            StaticShape::Cylinder { base: v(0.1, 0.05, -0.06), axis: v(0.0, 0.0, 0.12), radius: 0.01 },
            layers::ALL,
            true,
        );
        b.add_collider(
            StaticShape::Cuboid {
                center: v(-0.12, -0.05, -0.05),
                half_extents: v(0.03, 0.03, 0.03),
                rotation: glam::Quat::from_rotation_y(0.5) * glam::Quat::from_rotation_z(0.3),
            },
            layers::ALL,
            true,
        );
        let world = b.build();
        let geo = Geometry::new(&world);
        let mut s = solver_no_gravity();
        for (i, (profile, vel, spin)) in specs.iter().enumerate() {
            let mut ball = ball_at(ProfileId(*profile), v(-0.18 + 0.09 * i as f32, 0.07, 0.01), *vel);
            ball.angular_velocity = *spin;
            s.add_ball(ball).unwrap();
        }
        let mut ev = BallEvents::new();
        let mut e_prev = kinetic_energy(&s);
        for tick in 0..400 {
            ev.clear();
            s.step(&geo, DT, &mut ev);
            let e = kinetic_energy(&s);
            prop_assert!(e <= e_prev * (1.0 + 2e-6) + 1e-12, "tick {}: {} -> {}", tick, e_prev, e);
            e_prev = e;
            for (_, ball) in s.balls() {
                let r = radius(ball.profile);
                prop_assert!(ball.position.is_finite());
                prop_assert!(
                    (ball.position.abs() - (half - Vec3::splat(r))).max_element() < 1e-4,
                    "tick {}: left the box at {:?}", tick, ball.position
                );
            }
        }
    }

    /// No tunnelling through a 2 mm wall, a zero-thickness guide or a
    /// thin mesh at up to 30 m/s, with or without gravity.
    #[test]
    fn no_tunnelling_through_thin_walls(
        profile in 0u16..6,
        speed in 0.5f32..30.0,
        angle_y in -1.2f32..1.2,
        angle_z in -0.4f32..0.4,
        start in (-0.3f32..-0.05, -0.1f32..0.1, 0.03f32..0.12),
        spin in vec3(200.0),
        obstacle in 0usize..3,
        gravity: bool,
    ) {
        // The barrier is the plane x = 0 (front face), 1 m × 1 m.
        let mut b = StaticWorldBuilder::new();
        let mut guides = Vec::new();
        match obstacle {
            0 => {
                b.add_collider(cuboid(v(0.001, 0.0, 0.0), v(0.001, 1.0, 1.0)), layers::ALL, true);
            }
            1 => {
                b.add_collider(
                    StaticShape::TriMesh {
                        vertices: vec![v(0.0, -1.0, -1.0), v(0.0, 1.0, -1.0), v(0.0, 1.0, 1.0), v(0.0, -1.0, 1.0)],
                        indices: vec![[0, 1, 2], [0, 2, 3]],
                    },
                    layers::ALL,
                    true,
                );
            }
            _ => {
                let pts: Vec<Vec3> = (0..41).map(|i| v(0.0, -1.0 + 0.05 * i as f32, -1.0)).collect();
                guides.push(GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), 2.0).unwrap());
            }
        }
        let world = b.build();
        let geo = Geometry { world: &world, guides: &guides, wireforms: &[] };
        let mut s = if gravity { solver() } else { solver_no_gravity() };
        let dir = v(
            libm::cosf(angle_y) * libm::cosf(angle_z),
            libm::sinf(angle_y) * libm::cosf(angle_z),
            libm::sinf(angle_z),
        );
        let p = ProfileId(profile);
        let mut ball = ball_at(p, v(start.0, start.1, start.2), dir * speed);
        ball.angular_velocity = spin;
        let h = s.add_ball(ball).unwrap();
        let r = radius(p);
        let mut ev = BallEvents::new();
        for tick in 0..150 {
            ev.clear();
            s.step(&geo, DT, &mut ev);
            let x = s.ball(h).unwrap().position.x;
            prop_assert!(x < -r + 5.0e-5, "tick {}: x = {} (r = {})", tick, x, r);
        }
    }

    /// Two free balls in zero gravity: linear momentum, angular momentum
    /// about the centre of mass, and non-increasing energy, for any pair of
    /// profiles.
    #[test]
    fn free_collisions_conserve_momenta(
        pa in 0u16..7, pb in 0u16..7,
        offset in -0.95f32..0.95, lift in -0.5f32..0.5,
        speed in 0.3f32..10.0,
        vb in vec3(2.0), wa in vec3(300.0), wb in vec3(300.0),
    ) {
        let world = empty_world();
        let geo = Geometry::new(&world);
        let (ra, rb) = (radius(ProfileId(pa)), radius(ProfileId(pb)));
        let rsum = ra + rb;
        let mut s = solver_no_gravity();
        let mut a = ball_at(
            ProfileId(pa),
            v(-rsum - 0.02, offset * rsum * 0.9, lift * rsum * 0.4),
            v(speed, 0.0, 0.0),
        );
        a.angular_velocity = wa;
        let mut b = ball_at(ProfileId(pb), Vec3::ZERO, vb * 0.1);
        b.angular_velocity = wb;
        s.add_ball(a).unwrap();
        s.add_ball(b).unwrap();
        let (p0, l0) = (momentum(&s), angular_momentum(&s));
        let mut e_prev = kinetic_energy(&s);
        let mut ev = BallEvents::new();
        for _ in 0..120 {
            s.step(&geo, DT, &mut ev);
            let e = kinetic_energy(&s);
            prop_assert!(e <= e_prev * (1.0 + 1e-6));
            e_prev = e;
            prop_assert!((momentum(&s) - p0).length() <= 1e-4 * p0.length());
            let l = angular_momentum(&s);
            prop_assert!(
                (l - l0).length() <= 1e-3 * l0.length().max(1e-6),
                "L {:?} -> {:?}", l0, l
            );
        }
        let (a, b) = (s.ball(BallHandle(0)).unwrap(), s.ball(BallHandle(1)).unwrap());
        prop_assert!(a.position.distance(b.position) > rsum - 1e-5);
    }

    /// A ball with zero magnetic coupling is not perturbed by magnets, bit
    /// for bit, whatever the magnets and the other balls do.
    #[test]
    fn uncoupled_balls_ignore_magnets_exactly(
        profile in prop::sample::select(vec![2u16, 3, 6]),
        vel in vec3(2.0), spin in vec3(100.0),
        magnet_pos in vec3(0.1), k in 1.0e-7f32..1.0e-4, power in 0.0f32..1.0,
    ) {
        let world = floor_world();
        let geo = Geometry::new(&world);
        let run = |magnet: bool| {
            let mut s = solver();
            let p = ProfileId(profile);
            let mut b = ball_at(p, v(0.0, 0.0, radius(p) + 0.01), vel);
            b.angular_velocity = spin;
            let h = s.add_ball(b).unwrap();
            s.add_ball(Ball::new(STEEL, v(0.5, 0.5, radius(STEEL)))).unwrap();
            if magnet {
                let id = s.add_magnet(Magnet {
                    position: magnet_pos, k_coil: k, d_min: 0.005, range: 1.0, layer_mask: layers::ALL,
                });
                s.set_magnet_power(id, power);
            }
            let mut ev = BallEvents::new();
            let mut out = Vec::new();
            for _ in 0..150 {
                s.step(&geo, DT, &mut ev);
                out.push(*s.ball(h).unwrap());
            }
            out
        };
        prop_assert_eq!(run(false), run(true));
    }

    /// Adding balls that never come within contact range of a ball does not
    /// change one bit of that ball's trajectory, whatever they do (bounce,
    /// get pulled by a magnet, take many sub-steps, collide with each other).
    #[test]
    fn non_interacting_balls_do_not_perturb_each_other(
        profile in 0u16..6,
        vel in (-2.0f32..2.0, -0.4f32..0.4, -0.5f32..1.0),
        spin in vec3(150.0),
        drop in 0.0f32..0.05,
        others in prop::collection::vec((0u16..6, vec3(8.0), vec3(200.0), 0.0f32..0.1), 1..4),
        magnet_power in 0.0f32..1.0,
    ) {
        // The observed ball lives in the lane y < -0.2, the others in
        // y > 0.2, with a wall along y = 0 between them.
        let mut b = StaticWorldBuilder::new();
        b.add_collider(cuboid(v(0.0, 0.0, -0.05), v(10.0, 10.0, 0.05)), layers::ALL, true);
        b.add_collider(cuboid(v(0.0, 0.0, 0.5), v(10.0, 0.01, 0.5)), layers::ALL, true);
        b.add_collider(cuboid(v(0.0, 0.9, 0.5), v(10.0, 0.01, 0.5)), layers::ALL, true);
        b.add_collider(cuboid(v(0.0, -0.9, 0.5), v(10.0, 0.01, 0.5)), layers::ALL, true);
        let world = b.build();
        let geo = Geometry::new(&world);
        let run = |with_others: bool| {
            let mut s = solver();
            let p = ProfileId(profile);
            let mut ball = ball_at(p, v(0.0, -0.5, radius(p) + drop), v(vel.0, vel.1, vel.2));
            ball.angular_velocity = spin;
            let h = s.add_ball(ball).unwrap();
            if with_others {
                for (i, (op, ov, ow, od)) in others.iter().enumerate() {
                    let op = ProfileId(*op);
                    let mut o = ball_at(op, v(0.2 * i as f32 - 0.2, 0.5, radius(op) + od), *ov);
                    o.angular_velocity = *ow;
                    s.add_ball(o).unwrap();
                }
                let id = s.add_magnet(Magnet {
                    position: v(0.0, 0.5, -0.005), k_coil: 2.0e-5, d_min: 0.01, range: 0.2,
                    layer_mask: layers::ALL,
                });
                s.set_magnet_power(id, magnet_power);
            }
            let mut ev = BallEvents::new();
            let mut out = Vec::new();
            for _ in 0..300 {
                s.step(&geo, DT, &mut ev);
                out.push(*s.ball(h).unwrap());
            }
            out
        };
        let alone = run(false);
        let crowded = run(true);
        let first = alone.iter().zip(&crowded).position(|(a, b)| a != b);
        prop_assert_eq!(first, None);
    }

    /// A Newton chain in zero gravity conserves momentum along a straight
    /// path, keeps its order, and never lets neighbours overlap.
    #[test]
    fn newton_chain_invariants(
        n in 2usize..7,
        gaps in prop::collection::vec(prop::sample::select(vec![0.0f32, 0.0, 0.3, 1.0]), 6),
        strikes in prop::collection::vec(-2.0f32..2.0, 6),
    ) {
        let world = empty_world();
        let geo = Geometry::new(&world);
        let d = 2.0 * radius(STEEL);
        let pts: Vec<Vec3> = (0..60).map(|i| v(d * i as f32, 0.0, 0.0)).collect();
        let mut params = vec![25.0f32];
        for i in 1..n {
            params.push(params[i - 1] + 1.0 + gaps[i]);
        }
        let mut s = solver_no_gravity();
        let c = s.add_newton_chain(
            NewtonChain::new(CubicSpline::interpolate(&pts).unwrap(), STEEL, &params).unwrap(),
        );
        for (i, u) in strikes.iter().enumerate().take(n) {
            s.newton_chain_mut(c).strike(i, *u);
        }
        let p0: f32 = strikes[..n].iter().sum();
        let e0: f32 = strikes[..n].iter().map(|u| u * u).sum();
        let mut ev = BallEvents::new();
        for _ in 0..200 {
            s.step(&geo, DT, &mut ev);
            let chain = s.newton_chain(c);
            let p: f32 = (0..n).map(|i| chain.ball_speed(i)).sum();
            let e: f32 = (0..n).map(|i| chain.ball_speed(i).powi(2)).sum();
            prop_assert!((p - p0).abs() < 1e-4, "{} -> {}", p0, p);
            prop_assert!(e <= e0 * (1.0 + 1e-5) + 1e-9);
            for i in 1..n {
                prop_assert!(chain.ball_t(i) > chain.ball_t(i - 1));
                let gap = chain.ball_position(i).distance(chain.ball_position(i - 1)) - d;
                prop_assert!(gap > -5.0e-5, "gap {}", gap);
            }
        }
    }

    /// The dispenser is a FIFO with capacity 16.
    #[test]
    fn dispenser_is_a_bounded_fifo(ops in prop::collection::vec((any::<bool>(), 0u16..7), 0..80)) {
        let mut d = Dispenser::new(Vec3::ZERO, Vec3::ZERO);
        let mut model = std::collections::VecDeque::new();
        for (push, profile) in ops {
            if push {
                let ok = d.push(ProfileId(profile));
                prop_assert_eq!(ok, model.len() < 16);
                if ok {
                    model.push_back(ProfileId(profile));
                }
            } else {
                prop_assert_eq!(d.pop(), model.pop_front());
            }
            prop_assert_eq!(d.len(), model.len());
            prop_assert_eq!(d.is_empty(), model.is_empty());
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(6))]

    /// A ball at rest on a flat floor stays put: less than 1 µm of drift in
    /// 10 s, for every profile and anywhere on the floor.
    #[test]
    fn resting_ball_stays_put(profile in 0u16..7, x in -1.0f32..1.0, y in -1.0f32..1.0) {
        let world = floor_world();
        let geo = Geometry::new(&world);
        let mut s = solver();
        let p = ProfileId(profile);
        let start = v(x, y, radius(p));
        let h = s.add_ball(Ball::new(p, start)).unwrap();
        let mut ev = BallEvents::new();
        for _ in 0..10_000 {
            s.step(&geo, DT, &mut ev);
        }
        let b = s.ball(h).unwrap();
        prop_assert!(b.position.distance(start) < 1.0e-6, "drift {}", b.position.distance(start));
        prop_assert!(b.velocity.length() < 1.0e-6);
        prop_assert!(ev.as_slice().is_empty());
    }
}

/// A ball settles and rests in a V-groove and in a corner (several
/// simultaneous contacts) without creeping or jittering.
#[test]
fn ball_rests_in_groove_and_corner() {
    let rot = |a: f32| glam::Quat::from_rotation_y(a);
    let mut b = StaticWorldBuilder::new();
    for a in [0.6f32, -0.6] {
        b.add_collider(
            StaticShape::Cuboid {
                center: rot(a) * v(0.0, 0.0, -0.05),
                half_extents: v(0.5, 0.5, 0.05),
                rotation: rot(a),
            },
            layers::ALL,
            true,
        );
    }
    // End wall of the groove.
    b.add_collider(
        cuboid(v(0.0, -0.3, 0.0), v(0.5, 0.01, 0.5)),
        layers::ALL,
        true,
    );
    let world = b.build();
    let geo = Geometry::new(&world);
    let mut s = solver();
    let h = s.add_ball(Ball::new(STEEL, v(0.004, 0.0, 0.05))).unwrap();
    let mut ev = BallEvents::new();
    for _ in 0..4000 {
        s.step(&geo, DT, &mut ev);
    }
    let rest = *s.ball(h).unwrap();
    assert!(rest.velocity.length() < 1e-4, "{:?}", rest.velocity);
    assert!(rest.position.x.abs() < 1e-4, "{:?}", rest.position);
    for _ in 0..4000 {
        s.step(&geo, DT, &mut ev);
    }
    let later = *s.ball(h).unwrap();
    assert!(later.position.distance(rest.position) < 2.0e-6);
    assert_eq!(ev.dropped(), 0);
    // Rolling along the groove into the end wall: stops there, in a corner.
    s.apply_impulse(h, v(0.0, -0.04, 0.0));
    for _ in 0..6000 {
        s.step(&geo, DT, &mut ev);
    }
    let end = *s.ball(h).unwrap();
    assert!((end.position.y - (-0.29 + radius(STEEL))).abs() < 0.2);
    assert!(
        end.position.y > -0.29 + radius(STEEL) - 1e-4,
        "{:?}",
        end.position
    );
}
