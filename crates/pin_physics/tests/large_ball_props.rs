//! Subphase 2.8 property sweeps over ball diameters 15.88–79.4 mm (PLAN
//! 2.8 deliverables 1 and 3): no tunnelling through thin walls (zero-
//! thickness meshes, 1 mm plates, guide walls) at up to 100 m/s, stable
//! resting contact, momentum conservation between any two sizes, and a
//! flipper strike on any size. The CCD and broad-phase margins are
//! radius-relative (sub-step travel `r/4`, `r/16` near rails, paths padded
//! by `2r`, BVH queries by `r` plus a 10 µm leaf margin), so nothing in the
//! solver is tuned to the 27 mm ball.

#[path = "ball_util.rs"]
mod util;

#[path = "flipper_util.rs"]
mod flip;

use glam::{Quat, Vec3};
use pin_core::BallProfile;
use pin_core::ids::ProfileId;
use pin_physics::ball::{
    Ball, BallEvents, BallSolver, BallSolverConfig, Geometry, MovingBodies,
};
use pin_physics::flipper::FlipperSet;
use pin_physics::world::{
    CubicSpline, GuideCurve, StaticShape, StaticWorld, StaticWorldBuilder, layers,
};
use proptest::prelude::*;
use util::*;

const D_MIN: f32 = 0.01588;
const D_MAX: f32 = 0.0794;
/// Fastest ball the sweeps fire (the body-contact speed limit).
const MAX_SPEED: f32 = 100.0;

/// A steel-like ball of diameter `d` (mass ∝ d³ from the 27 mm, 80 g ball).
fn profile(d: f32) -> BallProfile {
    let mut p = pin_core::profiles::builtin_profile("standard_steel").unwrap();
    p.name = format!("d{d}");
    p.diameter_m = d;
    p.mass_kg = 0.080 * (d / 0.027).powi(3);
    p
}

fn sized_solver(ds: &[f32], gravity: Vec3) -> BallSolver {
    let mut cfg: BallSolverConfig = config();
    cfg.gravity = gravity;
    let profiles: Vec<BallProfile> = ds.iter().map(|&d| profile(d)).collect();
    BallSolver::new(cfg, &profiles)
}

fn ball(i: u16, pos: Vec3, vel: Vec3) -> Ball {
    let mut b = Ball::new(ProfileId(i), pos);
    b.velocity = vel;
    b
}

/// The wall kinds of the tunnelling sweep, all with their face on x = 0.
#[derive(Copy, Clone, Debug)]
enum Wall {
    /// Two-triangle zero-thickness mesh.
    Mesh,
    /// 1 mm plate (x in [0, 1 mm]).
    Plate,
    /// Level guide wall along y, 0.2 m high.
    Guide,
}

fn wall_world(kind: Wall) -> (StaticWorld, Vec<GuideCurve>) {
    let mut b = StaticWorldBuilder::new();
    let e = 2.0;
    match kind {
        Wall::Mesh => {
            b.add_collider(
                StaticShape::TriMesh {
                    vertices: vec![v(0.0, -e, -e), v(0.0, e, -e), v(0.0, e, e), v(0.0, -e, e)],
                    indices: vec![[0, 1, 2], [0, 2, 3]],
                },
                layers::ALL,
                true,
            );
        }
        Wall::Plate => {
            b.add_collider(cuboid(v(0.0005, 0.0, 0.0), v(0.0005, e, e)), layers::ALL, true);
        }
        Wall::Guide => {}
    }
    let guides = match kind {
        Wall::Guide => {
            let pts: Vec<Vec3> = (0..=8).map(|k| v(0.0, -e + 0.5 * k as f32, -0.1)).collect();
            vec![GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), 0.2).unwrap()]
        }
        _ => vec![],
    };
    (b.build(), guides)
}

fn wall_kind() -> impl Strategy<Value = Wall> {
    prop_oneof![Just(Wall::Mesh), Just(Wall::Plate), Just(Wall::Guide)]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// A ball of any size fired at a thin wall at up to 100 m/s, from any
    /// starting gap and angle, never gets through and never sinks into it.
    #[test]
    fn no_tunnelling_through_thin_walls(
        d in D_MIN..=D_MAX,
        kind in wall_kind(),
        speed in 0.5f32..=MAX_SPEED,
        gap in 1.0e-5f32..0.05,
        along in -0.9f32..0.9,
        up in -0.3f32..0.3,
        spin in -500.0f32..500.0,
    ) {
        let r = 0.5 * d;
        let (world, guides) = wall_world(kind);
        let geo = Geometry { world: &world, guides: &guides, wireforms: &[] };
        let mut s = sized_solver(&[d], Vec3::ZERO);
        let dir = v(1.0, along, up).normalize();
        let mut b = ball(0, v(-(r + gap), 0.0, 0.0), dir * speed);
        b.angular_velocity = v(0.0, 0.0, spin);
        let h = s.add_ball(b).unwrap();
        let mut ev = BallEvents::new();
        for tick in 0..40 {
            s.step(&geo, DT, &mut ev);
            let b = s.ball(h).unwrap();
            prop_assert!(b.position.is_finite() && b.velocity.is_finite());
            // Only the face at x = 0 is in reach before the bounce.
            if b.position.y.abs() < 1.5 && b.position.z.abs() < 0.09 {
                prop_assert!(
                    b.position.x < -r + 1.0e-4,
                    "{kind:?} d {d} v {speed}: x {} at tick {tick} (r {r})", b.position.x
                );
            }
        }
        let b = s.ball(h).unwrap();
        prop_assert!(b.velocity.x < 0.0, "{kind:?} d {d} v {speed}: did not bounce: {}", b.velocity);
    }

    /// A ball of any size set down on a flat floor stays put; on a pitched
    /// floor against a wall it settles and then stays.
    #[test]
    fn resting_contact_is_stable(d in D_MIN..=D_MAX, x in -0.5f32..0.5, pitch_deg in 0.0f32..10.0) {
        let r = 0.5 * d;
        // Flat floor (top face z = 0), gravity straight down.
        let world = floor_world();
        let geo = Geometry::new(&world);
        let mut s = sized_solver(&[d], v(0.0, 0.0, -9.81));
        let start = v(x, 0.1, r);
        let h = s.add_ball(ball(0, start, Vec3::ZERO)).unwrap();
        let mut ev = BallEvents::new();
        for _ in 0..1000 {
            s.step(&geo, DT, &mut ev);
        }
        let b = s.ball(h).unwrap();
        prop_assert!(b.position.distance(start) < 1.0e-5, "d {d}: drifted to {}", b.position);
        prop_assert!(b.velocity.length() < 1.0e-4, "d {d}: {}", b.velocity);
        // Pitched: the ball rolls down -y into a wall at y = 0 and rests.
        let mut wb = StaticWorldBuilder::new();
        wb.add_collider(cuboid(v(0.0, 0.0, -0.05), v(2.0, 2.0, 0.05)), layers::ALL, true);
        wb.add_collider(cuboid(v(0.0, -0.05, 0.1), v(2.0, 0.05, 0.1)), layers::ALL, true);
        let world = wb.build();
        let geo = Geometry::new(&world);
        let p = pitch_deg.to_radians();
        let mut s = sized_solver(&[d], v(0.0, -9.81 * p.sin(), -9.81 * p.cos()));
        let h = s.add_ball(ball(0, v(x, r + 0.02, r), Vec3::ZERO)).unwrap();
        for _ in 0..2500 {
            s.step(&geo, DT, &mut ev);
        }
        let settled = *s.ball(h).unwrap();
        for _ in 0..500 {
            s.step(&geo, DT, &mut ev);
        }
        let b = s.ball(h).unwrap();
        prop_assert!(b.position.distance(settled.position) < 2.0e-5, "d {d} pitch {pitch_deg}: {} -> {}", settled.position, b.position);
        prop_assert!(b.position.z > r - 2.0e-5 && b.position.y > r - 2.0e-5, "d {d}: sank: {}", b.position);
        prop_assert!(b.velocity.length() < 1.0e-3, "d {d}: {}", b.velocity);
    }

    /// Any two sizes in the range: momentum within 1e-4 and kinetic energy
    /// never rising through a collision.
    #[test]
    fn any_two_sizes_conserve_momentum(
        d1 in D_MIN..=D_MAX, d2 in D_MIN..=D_MAX,
        speed in 0.2f32..40.0, offset in -0.95f32..0.95,
    ) {
        let world = empty_world();
        let geo = Geometry::new(&world);
        let mut s = sized_solver(&[d1, d2], Vec3::ZERO);
        let (r1, r2) = (0.5 * d1, 0.5 * d2);
        s.add_ball(ball(0, v(-(r1 + r2) - 0.02, offset * (r1 + r2), 0.0), v(speed, 0.0, 0.0))).unwrap();
        s.add_ball(ball(1, Vec3::ZERO, Vec3::ZERO)).unwrap();
        let (p0, e0) = (momentum(&s), kinetic_energy(&s));
        let mut ev = BallEvents::new();
        let mut e_prev = e0;
        for _ in 0..100 {
            s.step(&geo, DT, &mut ev);
            let e = kinetic_energy(&s);
            prop_assert!(e <= e_prev * (1.0 + 1e-6));
            e_prev = e;
            prop_assert!((momentum(&s) - p0).length() <= 1e-4 * p0.length());
        }
        prop_assert!(!ev.as_slice().is_empty(), "no collision");
    }
}

/// A flipper rig whose solver knows one ball profile of diameter `d`.
fn sized_flipper_rig(d: f32, torque: f32) -> flip::Rig {
    let mut c = flip::left_config(0, [0.0; 3]);
    c.power_torque_nm = torque;
    c.inertia_kg_m2 = 0.5e-4;
    let mut rig = flip::Rig::with_world(&[c.clone()], StaticWorldBuilder::new().build(), Vec3::ZERO);
    rig.solver = sized_solver(&[d], Vec3::ZERO);
    rig.flippers = FlipperSet::new(&[c]).unwrap();
    rig
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// A strong flip into a resting ball of any size: never inside the bat,
    /// struck, and leaving on its own side.
    #[test]
    fn flipper_strike_never_tunnels_any_size(
        d in D_MIN..=D_MAX,
        torque in 1.0f32..40.0,
        fraction in 0.3f32..0.95,
        gap in 0.0005f32..0.01,
    ) {
        let r = 0.5 * d;
        let mut rig = sized_flipper_rig(d, torque);
        let dir = flip::bat_dir(-30.0);
        let n = Vec3::Z.cross(dir);
        let bat_r = 0.011 + (0.0065 - 0.011) * fraction;
        let start = dir * (flip::BAT_LEN * fraction) + n * (bat_r + r + gap) + v(0.0, 0.0, r);
        let h = rig.solver.add_ball(ball(0, start, Vec3::ZERO)).unwrap();
        rig.flippers.set_energised(flip::LEFT, true);
        for _ in 0..80 {
            rig.step();
            let cp = rig.flippers.closest_point(0, 0.0, rig.ball(h).position).unwrap();
            prop_assert!(cp.distance > r - 1.0e-3, "d {d}: {} mm inside", (r - cp.distance) * 1e3);
        }
        let b = rig.ball(h);
        prop_assert!(b.velocity.length() > 0.1, "d {d}: not struck");
        prop_assert!(b.position.dot(Vec3::Z.cross(flip::bat_dir(20.0))) > 0.0);
    }
}

#[test]
fn tilted_thin_wall_holds_the_xl_ball_at_full_speed() {
    // The same wall rotated off-axis, XL ball at 100 m/s, many angles.
    let d = D_MAX;
    let r = 0.5 * d;
    for k in 0..24 {
        let q = Quat::from_rotation_z(k as f32 * 0.26) * Quat::from_rotation_y(0.1 * k as f32);
        let mut b = StaticWorldBuilder::new();
        let e = 2.0;
        let verts = [v(0.0, -e, -e), v(0.0, e, -e), v(0.0, e, e), v(0.0, -e, e)].map(|p| q * p);
        b.add_collider(
            StaticShape::TriMesh {
                vertices: verts.to_vec(),
                indices: vec![[0, 1, 2], [0, 2, 3]],
            },
            layers::ALL,
            true,
        );
        let world = b.build();
        let geo = Geometry::new(&world);
        let normal = q * Vec3::X;
        let mut s = sized_solver(&[d], Vec3::ZERO);
        let h = s
            .add_ball(ball(0, -normal * (r + 0.001), normal * MAX_SPEED))
            .unwrap();
        let mut ev = BallEvents::new();
        for _ in 0..20 {
            s.step(&geo, DT, &mut ev);
            let p = s.ball(h).unwrap().position;
            if p.length() < 1.5 {
                assert!(p.dot(normal) < -r + 1.0e-4, "angle {k}: {}", p.dot(normal));
            }
        }
        assert!(s.ball(h).unwrap().velocity.dot(normal) < 0.0);
    }
}
