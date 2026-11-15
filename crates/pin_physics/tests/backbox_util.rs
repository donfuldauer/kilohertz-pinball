//! Shared rig of the Subphase 5.5 backbox tests (`backbox_*.rs`): a vertical
//! mini-playfield on collision layer 4 next to a main deck and decoys on
//! layers 0–3. Included with `#[path]`; an empty test crate on its own.
//!
//! # Frames (DECISIONS.md "5.5 — Backbox vertical-plane convention")
//! The tests work in a **world** frame: `z` up, gravity `(0, 0, −g)`. The
//! horizontal reference table is the usual table frame rotated into it by
//! the 6.5° pitch; the backbox mini-playfield is the same flipper rig
//! rotated by [`vertical_frame`] (+90° about `x`): its playfield plane is the
//! world plane `y = 0`, its normal `−y` (facing the player), its "up the
//! playfield" direction world `+z`. Gravity along that plane is the full
//! `g` (the horizontal table has `g·sin 6.5°`); the normal component is 0.
#![allow(dead_code)]

use glam::{Quat, Vec3};
use pin_core::ids::{CoilId, FlipperId, ProfileId, SwitchId};
use pin_core::{
    BallProfile, CabinetInput, FlipperConfig, FlipperShape, FlipperStagingMode, PhysicsDefaults,
};
use pin_physics::ball::{
    Ball, BallEvent, BallEvents, BallHandle, BallSolver, BallSolverConfig, Geometry, ImpactTarget,
};
use pin_physics::flipper::FlipperSet;
use pin_physics::world::{StaticShape, StaticWorld, StaticWorldBuilder, layers};

/// Tick length, s.
pub const DT: f32 = 1.0e-3;
/// Standard steel ball profile.
pub const STEEL: ProfileId = ProfileId(0);
/// Standard ball radius, m.
pub const BALL_R: f32 = 0.0135;
/// Bat length of the Appendix C flipper, m.
pub const BAT_LEN: f32 = 0.076;
/// Bat base radius, m.
pub const BASE_R: f32 = 0.011;
/// Bat tip radius, m.
pub const TIP_R: f32 = 0.0065;
/// Standard gravity, m/s².
pub const G: f32 = 9.81;
/// Pitch of the horizontal reference table, degrees.
pub const PITCH_DEG: f32 = 6.5;
/// Backbox layer index (mask bit 4).
pub const BACKBOX_LAYER: u8 = 4;
/// The backbox flipper.
pub const BB_FLIPPER: FlipperId = FlipperId(0);

pub fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

/// Table frame → world for the horizontal reference: a rotation of the
/// pitch about `x` (the table's "up" `y` rises by the pitch).
pub fn pitched_frame(pitch_deg: f32) -> Quat {
    Quat::from_rotation_x(pitch_deg.to_radians())
}

/// Table frame → world for the vertical backbox playfield: +90° about `x`
/// (table `y` → world `z`, table normal `z` → world `−y`).
pub fn vertical_frame() -> Quat {
    Quat::from_rotation_x(core::f32::consts::FRAC_PI_2)
}

/// World gravity.
pub fn gravity() -> Vec3 {
    v(0.0, 0.0, -G)
}

/// Gravity component along a playfield plane whose table frame is `frame`
/// (its "down the playfield" direction is `frame · (−y)`), m/s².
pub fn gravity_along(frame: Quat) -> f32 {
    gravity().dot(frame * v(0.0, -1.0, 0.0))
}

/// A left flipper (Appendix C defaults, 76 mm bat, rest −30°, sweep +50°)
/// with its pivot at `base` (table frame, m) carried into the world by
/// `frame`, on layer `layer`.
pub fn flipper_config(
    frame: Quat,
    base: Vec3,
    layer: u8,
    staging: FlipperStagingMode,
) -> FlipperConfig {
    let p = PhysicsDefaults::builtin().resolve(None);
    let pivot = frame * base * 1000.0;
    let n = frame * Vec3::Z;
    FlipperConfig {
        id: BB_FLIPPER,
        coil: CoilId(0),
        eos_switch: SwitchId(100),
        collision_layer: layer,
        base_position_mm: pivot.to_array(),
        surface_normal: n.to_array(),
        rest_angle_deg: -30.0,
        sweep_angle_deg: 50.0,
        eos_angle_fraction: 0.92,
        shape: FlipperShape::StraightCapsule {
            length_mm: BAT_LEN * 1000.0,
            base_radius_mm: BASE_R * 1000.0,
            tip_radius_mm: TIP_R * 1000.0,
        },
        inertia_kg_m2: p.flipper_inertia,
        power_torque_nm: p.flipper_power_torque,
        hold_torque_nm: p.flipper_hold_torque,
        return_spring_torque_nm: p.flipper_return_spring_torque,
        stop_restitution: p.flipper_stop_restitution,
        rubber_restitution: p.flipper_rubber_restitution,
        rubber_damping: p.flipper_rubber_damping,
        staging,
        zipper_slide_axis_mm: None,
        zipper_slide_speed_mm_s: 0.0,
    }
}

/// Dedicated-button staging on the left flipper button.
pub fn dedicated() -> FlipperStagingMode {
    FlipperStagingMode::DedicatedButton(CabinetInput::LeftFlipper)
}

pub fn profiles() -> Vec<BallProfile> {
    pin_core::profiles::builtin_profiles().collect()
}

/// A thin slab whose face lies in the plane through `frame · (0, 0, 0)` with
/// normal `frame · z`, `half` (m) across and along the plane, 5 cm thick
/// behind the face.
pub fn plane_slab(frame: Quat, center: Vec3, half_u: f32, half_w: f32) -> StaticShape {
    StaticShape::Cuboid {
        center: center + frame * v(0.0, 0.0, -0.05),
        half_extents: v(half_u, half_w, 0.05),
        rotation: frame,
    }
}

/// The playfield of a flipper rig in `frame`: a 4 m square slab whose top
/// face is the plane through the origin, on `mask`.
pub fn playfield_world(frame: Quat, mask: u8) -> StaticWorld {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(plane_slab(frame, Vec3::ZERO, 2.0, 2.0), mask, true);
    b.build()
}

/// One flipper and a ball solver in a world.
pub struct Rig {
    pub world: StaticWorld,
    pub solver: BallSolver,
    pub flippers: FlipperSet,
    pub events: BallEvents,
    pub tick: u64,
}

impl Rig {
    pub fn new(configs: &[FlipperConfig], world: StaticWorld) -> Self {
        let mut cfg = BallSolverConfig::from_defaults(&PhysicsDefaults::builtin());
        cfg.gravity = gravity();
        Self {
            world,
            solver: BallSolver::new(cfg, &profiles()),
            flippers: FlipperSet::new(configs).expect("valid flippers"),
            events: BallEvents::new(),
            tick: 0,
        }
    }

    pub fn add_ball(&mut self, position: Vec3, velocity: Vec3, layer: u8) -> BallHandle {
        let mut ball = Ball::new(STEEL, position);
        ball.velocity = velocity;
        ball.layer = layer;
        self.solver.add_ball(ball).expect("room for a ball")
    }

    pub fn ball(&self, h: BallHandle) -> Ball {
        *self.solver.ball(h).expect("ball exists")
    }

    /// One tick: flipper schedule, then the ball step with the flippers as
    /// body set 0 (the engine's order).
    pub fn step(&mut self) {
        self.flippers.step_tick(self.tick);
        self.events.clear();
        let geo = Geometry::new(&self.world);
        self.solver
            .step_with_bodies(&geo, &mut [&mut self.flippers], DT, &mut self.events);
        self.tick += 1;
    }

    /// Static colliders hit in the last step, with the ball's layer.
    pub fn collider_hits(&self) -> Vec<(BallHandle, usize)> {
        self.events
            .as_slice()
            .iter()
            .filter_map(|e| match *e {
                BallEvent::Impact {
                    ball,
                    other: ImpactTarget::Collider(c),
                    ..
                } => Some((ball, c.index())),
                _ => None,
            })
            .collect()
    }
}

/// Gap (m) between the ball surface and the bat surface of flipper `id`
/// (a straight capsule from base radius to tip radius).
pub fn bat_gap(flippers: &FlipperSet, id: FlipperId, normal: Vec3, ball: Vec3) -> f32 {
    let pivot = flippers.pivot_m(id).expect("flipper");
    let dir = flippers.bat_direction(id).expect("flipper");
    let rel = ball - pivot;
    // The bat's contact shape is its outline in the sweep plane.
    let rel = rel - normal * rel.dot(normal);
    let s = rel.dot(dir).clamp(0.0, BAT_LEN);
    let radius = BASE_R + (TIP_R - BASE_R) * (s / BAT_LEN);
    (rel - dir * s).length() - radius - BALL_R
}

/// Unit vector along the bat at `angle_deg` in the table frame.
pub fn bat_dir(angle_deg: f32) -> Vec3 {
    let a = angle_deg.to_radians();
    v(libm::cosf(a), libm::sinf(a), 0.0)
}

/// A collider mask that is every layer except `layer`'s bit.
pub fn all_but(layer: u8) -> u8 {
    layers::ALL & !layers::layer_bit(layer)
}
