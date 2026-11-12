//! Shared helpers for the Subphase 2.2 independent spec tests (`spec_ball*.rs`).
//!
//! Included by the other spec files with `#[path = "spec_ball_common.rs"] mod common;`.
//! Compiled on its own it is an empty test binary.
#![allow(dead_code)]

use glam::{Quat, Vec3};
use pin_core::profiles::{BUILTIN_PROFILE_NAMES, builtin_profiles};
use pin_core::{BallProfile, PhysicsDefaults, ProfileId};
use pin_physics::ball::{Ball, BallEvents, BallSolver, BallSolverConfig, Geometry};
use pin_physics::world::{GuideCurve, StaticShape, StaticWorld, StaticWorldBuilder, Wireform};

/// The engine tick.
pub const DT: f32 = 0.001;

/// All seven built-in profiles; `ProfileId(i)` is index `i`.
pub fn profiles() -> Vec<BallProfile> {
    builtin_profiles().collect()
}

/// `ProfileId` of a built-in profile name.
pub fn pid(name: &str) -> ProfileId {
    let i = BUILTIN_PROFILE_NAMES
        .iter()
        .position(|n| *n == name)
        .unwrap_or_else(|| panic!("unknown builtin profile {name}"));
    ProfileId(i as u16)
}

pub fn profile(name: &str) -> BallProfile {
    profiles()[pid(name).index()].clone()
}

pub fn defaults() -> PhysicsDefaults {
    PhysicsDefaults::builtin()
}

/// Solver configured from the built-in defaults, with `gravity`.
pub fn solver_with_gravity(gravity: Vec3) -> BallSolver {
    let mut cfg = BallSolverConfig::from_defaults(&defaults());
    cfg.gravity = gravity;
    BallSolver::new(cfg, &profiles())
}

pub fn solver() -> BallSolver {
    BallSolver::new(BallSolverConfig::from_defaults(&defaults()), &profiles())
}

pub fn cuboid(center: Vec3, half: Vec3) -> StaticShape {
    StaticShape::Cuboid {
        center,
        half_extents: half,
        rotation: Quat::IDENTITY,
    }
}

/// A 20 m × 20 m slab whose top face is the plane z = 0.
pub fn floor_shape() -> StaticShape {
    cuboid(Vec3::new(0.0, 0.0, -0.05), Vec3::new(10.0, 10.0, 0.05))
}

pub fn empty_world() -> StaticWorld {
    StaticWorldBuilder::new().build()
}

pub fn floor_world() -> StaticWorld {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(floor_shape(), pin_physics::world::layers::ALL, true);
    b.build()
}

pub fn geo<'a>(world: &'a StaticWorld) -> Geometry<'a> {
    Geometry {
        world,
        guides: &[],
        wireforms: &[],
    }
}

pub fn geo_full<'a>(
    world: &'a StaticWorld,
    guides: &'a [GuideCurve],
    wireforms: &'a [Wireform],
) -> Geometry<'a> {
    Geometry {
        world,
        guides,
        wireforms,
    }
}

pub fn events() -> BallEvents {
    BallEvents::default()
}

pub fn ball(name: &str, position: Vec3) -> Ball {
    Ball::new(pid(name), position)
}

/// Linear + rotational kinetic energy of one ball (J).
pub fn kinetic(b: &Ball, p: &BallProfile) -> f64 {
    let m = f64::from(p.mass_kg);
    let i = f64::from(p.moment_of_inertia());
    let v = b.velocity.as_dvec3();
    let w = b.angular_velocity.as_dvec3();
    0.5 * m * v.length_squared() + 0.5 * i * w.length_squared()
}

pub fn total_kinetic(s: &BallSolver, profs: &[BallProfile]) -> f64 {
    s.balls()
        .map(|(_, b)| kinetic(b, &profs[b.profile.index()]))
        .sum()
}

/// Contact-point slip speed `|v + ω × r_c|` of a ball resting on a surface with
/// upward normal `n` (contact point at `-r n` from the centre), surface at rest.
pub fn slip(b: &Ball, r: f32, n: Vec3) -> f32 {
    (b.velocity + b.angular_velocity.cross(-r * n)).length()
}

/// Every float of a ball's state as raw bits (for bit-exact comparisons).
pub fn state_bits(b: &Ball) -> [u32; 13] {
    let p = b.position;
    let v = b.velocity;
    let w = b.angular_velocity;
    let q = b.orientation;
    [
        p.x.to_bits(),
        p.y.to_bits(),
        p.z.to_bits(),
        v.x.to_bits(),
        v.y.to_bits(),
        v.z.to_bits(),
        w.x.to_bits(),
        w.y.to_bits(),
        w.z.to_bits(),
        q.x.to_bits(),
        q.y.to_bits(),
        q.z.to_bits(),
        q.w.to_bits(),
    ]
}

/// A busy 5 s scenario: a pitched playfield (6.5°, pitch from geometry) with
/// side walls, posts, a level guide arc, a wireform, a Newton chain, a magnet
/// pulsed by the test, two sensors and four balls of different profiles.
pub struct Scenario {
    pub world: StaticWorld,
    pub guides: Vec<GuideCurve>,
    pub wireforms: Vec<Wireform>,
    pub solver: BallSolver,
    pub magnet: pin_core::MagnetId,
}

pub fn scenario(external: Option<Box<dyn pin_physics::ball::ExternalForce>>) -> Scenario {
    use pin_physics::ball::{Magnet, NewtonChain, Sensor, SensorKind};
    use pin_physics::world::{CubicSpline, layers};
    let pitch = 6.5f32.to_radians();
    let rot = Quat::from_rotation_x(pitch);
    let mut b = StaticWorldBuilder::new();
    let pf = b.add_collider(
        StaticShape::Cuboid {
            center: rot * Vec3::new(0.0, 0.0, -0.01),
            half_extents: Vec3::new(0.3, 0.6, 0.01),
            rotation: rot,
        },
        layers::ALL,
        true,
    );
    for x in [-0.31f32, 0.31] {
        b.add_collider(
            StaticShape::Cuboid {
                center: rot * Vec3::new(x, 0.0, 0.03),
                half_extents: Vec3::new(0.01, 0.6, 0.04),
                rotation: rot,
            },
            layers::ALL,
            true,
        );
    }
    // Bottom wall catches the balls.
    b.add_collider(
        StaticShape::Cuboid {
            center: rot * Vec3::new(0.0, -0.61, 0.03),
            half_extents: Vec3::new(0.3, 0.01, 0.04),
            rotation: rot,
        },
        layers::ALL,
        true,
    );
    let n = rot * Vec3::Z;
    for (i, (x, y)) in [(-0.1f32, 0.2f32), (0.1, 0.2), (0.0, 0.0), (-0.15, -0.2), (0.15, -0.25)]
        .into_iter()
        .enumerate()
    {
        let post = b.add_collider(
            StaticShape::Cylinder {
                base: rot * Vec3::new(x, y, -0.005),
                axis: n * 0.05,
                radius: 0.012 + 0.002 * i as f32,
            },
            layers::ALL,
            true,
        );
        let _ = post;
    }
    let world = b.build();
    let guide_pts: Vec<Vec3> = (0..9)
        .map(|i| {
            let a = std::f32::consts::PI * i as f32 / 8.0;
            Vec3::new(0.25 * libm::cosf(a), 0.4 + 0.15 * libm::sinf(a), 0.08)
        })
        .collect();
    let guides = vec![GuideCurve::new(CubicSpline::interpolate(&guide_pts).unwrap(), 0.05).unwrap()];
    let wf_pts: Vec<Vec3> = (0..6)
        .map(|i| Vec3::new(-0.25 + 0.1 * i as f32, 0.55, 0.3 - 0.03 * i as f32))
        .collect();
    let wireforms = vec![
        Wireform::new(CubicSpline::interpolate(&wf_pts).unwrap(), &[0.0; 6], 0.02, Vec3::Z)
            .unwrap(),
    ];
    let mut s = solver();
    if let Some(f) = external {
        s.add_external_force(f);
    }
    s.set_collider_material(pf, pin_core::Material::Wood);
    let magnet = s.add_magnet(Magnet {
        position: rot * Vec3::new(0.0, -0.1, -0.005),
        k_coil: 2.0e-5,
        d_min: 0.01,
        range: 0.08,
        layer_mask: layers::ALL,
    });
    s.add_sensor(Sensor {
        switch: pin_core::SwitchId(5),
        kind: SensorKind::Opto,
        center: rot * Vec3::new(0.0, -0.3, 0.0135),
        radius: 0.02,
        layer_mask: layers::ALL,
    });
    s.add_sensor(Sensor {
        switch: pin_core::SwitchId(130),
        kind: SensorKind::Eddy,
        center: rot * Vec3::new(-0.15, -0.4, 0.0135),
        radius: 0.03,
        layer_mask: layers::ALL,
    });
    let d = 0.027f32;
    let chain_pts: Vec<Vec3> = (0..8)
        .map(|i| Vec3::new(0.4 + i as f32 * d, 0.0, 0.2))
        .collect();
    let mut chain = NewtonChain::new(
        CubicSpline::interpolate(&chain_pts).unwrap(),
        pid("standard_steel"),
        &[2.0, 3.0, 4.0],
    )
    .unwrap();
    chain.strike(0, 0.7);
    s.add_newton_chain(chain);
    let starts = [
        ("standard_steel", Vec3::new(-0.2, 0.45, 0.0), Vec3::new(1.2, -0.5, 0.0)),
        ("ceramic_powerball", Vec3::new(0.05, 0.5, 0.0), Vec3::new(-0.8, -1.5, 0.0)),
        ("mini_steel", Vec3::new(0.2, 0.3, 0.0), Vec3::new(-1.5, 0.3, 0.0)),
        ("uv_globall", Vec3::new(-0.05, 0.1, 0.0), Vec3::new(0.4, 2.0, 0.0)),
    ];
    for (name, p, v) in starts {
        let r = profile(name).radius_m();
        let mut bl = Ball::new(pid(name), rot * (p + Vec3::new(0.0, 0.0, r + 0.001)));
        bl.velocity = rot * v;
        bl.angular_velocity = Vec3::new(3.0, -7.0, 11.0);
        s.add_ball(bl).unwrap();
    }
    Scenario {
        world,
        guides,
        wireforms,
        solver: s,
        magnet,
    }
}

/// Power command of the scenario magnet at `tick` (pulsed).
pub fn scenario_power(tick: u32) -> f32 {
    if (tick / 250) % 2 == 0 { 1.0 } else { 0.3 }
}
