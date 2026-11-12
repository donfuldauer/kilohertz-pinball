//! Shared helpers of the Subphase 2.2 implementer tests (`ball_*.rs`);
//! included with `#[path]`, and an empty test crate on its own.
#![allow(dead_code)]

use glam::{DVec3, Quat, Vec3};
use pin_core::ids::ProfileId;
use pin_core::profiles::{BUILTIN_PROFILE_NAMES, builtin_profiles};
use pin_core::{BallProfile, PhysicsDefaults};
use pin_physics::ball::{Ball, BallSolver, BallSolverConfig};
use pin_physics::world::{StaticShape, StaticWorld, StaticWorldBuilder, layers};

pub const DT: f32 = 1.0e-3;

pub const STEEL: ProfileId = ProfileId(0);
pub const POWERBALL: ProfileId = ProfileId(2);
pub const GLOBALL: ProfileId = ProfileId(3);
pub const MINI: ProfileId = ProfileId(4);
pub const JUMBO: ProfileId = ProfileId(5);

pub fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

pub fn profiles() -> Vec<BallProfile> {
    let p: Vec<BallProfile> = builtin_profiles().collect();
    assert_eq!(p.len(), BUILTIN_PROFILE_NAMES.len());
    assert_eq!(p[2].name, "ceramic_powerball");
    assert_eq!(p[4].name, "mini_steel");
    p
}

pub fn config() -> BallSolverConfig {
    BallSolverConfig::from_defaults(&PhysicsDefaults::builtin())
}

pub fn solver() -> BallSolver {
    BallSolver::new(config(), &profiles())
}

pub fn solver_no_gravity() -> BallSolver {
    let mut cfg = config();
    cfg.gravity = Vec3::ZERO;
    BallSolver::new(cfg, &profiles())
}

pub fn radius(p: ProfileId) -> f32 {
    profiles()[p.index()].radius_m()
}

pub fn empty_world() -> StaticWorld {
    StaticWorldBuilder::new().build()
}

pub fn cuboid(center: Vec3, half_extents: Vec3) -> StaticShape {
    StaticShape::Cuboid {
        center,
        half_extents,
        rotation: Quat::IDENTITY,
    }
}

/// A 4 m × 4 m floor whose top face is the plane z = 0.
pub fn floor_world() -> StaticWorld {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(cuboid(v(0.0, 0.0, -0.05), v(2.0, 2.0, 0.05)), layers::ALL, true);
    b.build()
}

/// A closed box with inner half extents `h`, centred at the origin.
pub fn box_world(h: Vec3) -> StaticWorld {
    let t = 0.05;
    let mut b = StaticWorldBuilder::new();
    for axis in 0..3 {
        for sign in [-1.0f32, 1.0] {
            let mut c = Vec3::ZERO;
            c[axis] = sign * (h[axis] + t);
            let mut he = h + Vec3::splat(2.0 * t);
            he[axis] = t;
            b.add_collider(cuboid(c, he), layers::ALL, true);
        }
    }
    b.build()
}

pub fn ball_at(profile: ProfileId, pos: Vec3, vel: Vec3) -> Ball {
    let mut b = Ball::new(profile, pos);
    b.velocity = vel;
    b
}

/// Kinetic energy (linear + rotational) of all balls, in f64.
pub fn kinetic_energy(s: &BallSolver) -> f64 {
    let p = s.profiles();
    s.balls()
        .map(|(_, b)| {
            let pr = &p[b.profile.index()];
            0.5 * f64::from(pr.mass_kg) * b.velocity.as_dvec3().length_squared()
                + 0.5
                    * f64::from(pr.moment_of_inertia())
                    * b.angular_velocity.as_dvec3().length_squared()
        })
        .sum()
}

pub fn momentum(s: &BallSolver) -> DVec3 {
    let p = s.profiles();
    s.balls()
        .map(|(_, b)| b.velocity.as_dvec3() * f64::from(p[b.profile.index()].mass_kg))
        .sum()
}

/// Total angular momentum about the system's centre of mass.
pub fn angular_momentum(s: &BallSolver) -> DVec3 {
    let p = s.profiles();
    let mass: f64 = s
        .balls()
        .map(|(_, b)| f64::from(p[b.profile.index()].mass_kg))
        .sum();
    let com: DVec3 = s
        .balls()
        .map(|(_, b)| b.position.as_dvec3() * f64::from(p[b.profile.index()].mass_kg))
        .sum::<DVec3>()
        / mass;
    s.balls()
        .map(|(_, b)| {
            let pr = &p[b.profile.index()];
            (b.position.as_dvec3() - com).cross(b.velocity.as_dvec3()) * f64::from(pr.mass_kg)
                + b.angular_velocity.as_dvec3() * f64::from(pr.moment_of_inertia())
        })
        .sum()
}

/// Bit pattern of the whole ball state, for determinism checks.
pub fn state_bits(s: &BallSolver) -> Vec<u32> {
    let mut out = Vec::new();
    for (h, b) in s.balls() {
        out.push(u32::from(h.0));
        for x in b
            .position
            .to_array()
            .into_iter()
            .chain(b.velocity.to_array())
            .chain(b.angular_velocity.to_array())
            .chain(b.orientation.to_array())
        {
            out.push(x.to_bits());
        }
    }
    out
}
