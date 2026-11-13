//! Shared helpers of the Subphase 2.4 implementer tests (`mech_*.rs`);
//! included with `#[path]`, and an empty test crate on its own.
#![allow(dead_code)]

use glam::{Quat, Vec3};
use pin_core::ids::ProfileId;
use pin_core::profiles::builtin_profiles;
use pin_core::{BallProfile, PhysicsDefaults};
use pin_physics::ball::{Ball, BallEvents, BallHandle, BallSolver, BallSolverConfig, Geometry};
use pin_physics::mech::{MechSet, MechSpec};
use pin_physics::world::{StaticShape, StaticWorld, StaticWorldBuilder, layers};

pub const DT: f32 = 1.0e-3;
pub const STEEL: ProfileId = ProfileId(0);

pub fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

pub fn profiles() -> Vec<BallProfile> {
    builtin_profiles().collect()
}

pub fn radius(p: ProfileId) -> f32 {
    profiles()[p.index()].radius_m()
}

pub fn cuboid(center: Vec3, half_extents: Vec3) -> StaticShape {
    StaticShape::Cuboid {
        center,
        half_extents,
        rotation: Quat::IDENTITY,
    }
}

/// A playfield: wood floor (top at z = 0) of ±0.5 m on every layer, plus
/// `extra` boxes `(centre, half extents, layer mask)`.
pub fn world(extra: &[(Vec3, Vec3, u8)]) -> StaticWorld {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        cuboid(v(0.0, 0.0, -0.05), v(0.5, 0.5, 0.05)),
        layers::MAIN_DECK,
        true,
    );
    for &(c, h, m) in extra {
        b.add_collider(cuboid(c, h), m, true);
    }
    b.build()
}

/// A solver with gravity tilted by `pitch_deg` down `−y` (0 = flat).
pub fn solver(pitch_deg: f32) -> BallSolver {
    let mut cfg = BallSolverConfig::from_defaults(&PhysicsDefaults::builtin());
    let p = pitch_deg.to_radians();
    cfg.gravity = v(0.0, -9.81 * libm::sinf(p), -9.81 * libm::cosf(p));
    BallSolver::new(cfg, &profiles())
}

pub struct Table {
    pub world: StaticWorld,
    pub solver: BallSolver,
    pub mechs: MechSet,
    pub events: BallEvents,
    pub ticks: u64,
}

impl Table {
    pub fn new(specs: &[MechSpec], world: StaticWorld, pitch_deg: f32) -> Self {
        let mechs = MechSet::new(specs, &PhysicsDefaults::builtin()).expect("valid specs");
        Self {
            world,
            solver: solver(pitch_deg),
            mechs,
            events: BallEvents::new(),
            ticks: 0,
        }
    }

    /// Adds a steel ball resting on the floor at `(x, y)` with `velocity`.
    pub fn ball(&mut self, x: f32, y: f32, velocity: Vec3) -> BallHandle {
        let mut b = Ball::new(STEEL, v(x, y, radius(STEEL)));
        b.velocity = velocity;
        self.solver.add_ball(b).expect("slot")
    }

    pub fn tick(&mut self) {
        self.events.clear();
        let geo = Geometry::new(&self.world);
        self.mechs
            .step(&mut self.solver, &geo, None, DT, &mut self.events);
        self.ticks += 1;
    }

    pub fn get(&self, h: BallHandle) -> Ball {
        *self.solver.ball(h).expect("ball")
    }

    /// Whether `switch` is closed after the last tick.
    pub fn closed(&self, switch: u16) -> bool {
        let mut s = [0u64; 4];
        self.mechs.switch_states(&mut s);
        s[usize::from(switch) / 64] & (1 << (switch % 64)) != 0
    }
}
