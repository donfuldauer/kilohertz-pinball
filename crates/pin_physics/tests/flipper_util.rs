//! Shared rig of the Subphase 2.3 implementer tests (`flipper_*.rs`);
//! included with `#[path]`, and an empty test crate on its own.
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

pub const DT: f32 = 1.0e-3;
pub const STEEL: ProfileId = ProfileId(0);
pub const BALL_R: f32 = 0.0135;
pub const BAT_LEN: f32 = 0.076;
pub const LEFT: FlipperId = FlipperId(0);
pub const RIGHT: FlipperId = FlipperId(1);

pub fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

/// A left flipper with the Appendix C defaults: 76 mm bat, pivot at
/// `base_mm`, rest −30°, sweep +50°.
pub fn left_config(id: u16, base_mm: [f32; 3]) -> FlipperConfig {
    let p = PhysicsDefaults::builtin().resolve(None);
    FlipperConfig {
        id: FlipperId(id),
        coil: CoilId(id),
        eos_switch: SwitchId(100 + id),
        collision_layer: 0,
        base_position_mm: base_mm,
        surface_normal: [0.0, 0.0, 1.0],
        rest_angle_deg: -30.0,
        sweep_angle_deg: 50.0,
        eos_angle_fraction: 0.92,
        shape: FlipperShape::StraightCapsule {
            length_mm: 76.0,
            base_radius_mm: 11.0,
            tip_radius_mm: 6.5,
        },
        inertia_kg_m2: p.flipper_inertia,
        power_torque_nm: p.flipper_power_torque,
        hold_torque_nm: p.flipper_hold_torque,
        return_spring_torque_nm: p.flipper_return_spring_torque,
        stop_restitution: p.flipper_stop_restitution,
        rubber_restitution: p.flipper_rubber_restitution,
        rubber_damping: p.flipper_rubber_damping,
        staging: FlipperStagingMode::DedicatedButton(CabinetInput::LeftFlipper),
        zipper_slide_axis_mm: None,
        zipper_slide_speed_mm_s: 0.0,
    }
}

/// The mirror image: rest 210°, sweep −50°.
pub fn right_config(id: u16, base_mm: [f32; 3]) -> FlipperConfig {
    FlipperConfig {
        rest_angle_deg: 210.0,
        sweep_angle_deg: -50.0,
        staging: FlipperStagingMode::DedicatedButton(CabinetInput::RightFlipper),
        ..left_config(id, base_mm)
    }
}

pub fn profiles() -> Vec<BallProfile> {
    pin_core::profiles::builtin_profiles().collect()
}

/// Gravity of a playfield pitched by `pitch_deg` (down the −y direction),
/// in a frame whose z axis is the playfield normal.
pub fn pitched_gravity(pitch_deg: f32) -> Vec3 {
    let p = pitch_deg.to_radians();
    v(0.0, -9.81 * libm::sinf(p), -9.81 * libm::cosf(p))
}

/// A large floor whose top face is the plane through the origin with
/// normal `rotation · z`.
pub fn floor(rotation: Quat) -> StaticWorld {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        StaticShape::Cuboid {
            center: rotation * v(0.0, 0.0, -0.05),
            half_extents: v(2.0, 2.0, 0.05),
            rotation,
        },
        layers::ALL,
        true,
    );
    b.build()
}

/// Flippers on a pitched floor with a ball solver. `frame` rotates the
/// whole rig (floor, gravity; the flipper configs must be given in the
/// rotated frame already).
pub struct Rig {
    pub world: StaticWorld,
    pub solver: BallSolver,
    pub flippers: FlipperSet,
    pub events: BallEvents,
    pub tick: u64,
    /// Body impacts of the last step: (ball, body, normal speed).
    pub hits: Vec<(BallHandle, usize, f32)>,
}

impl Rig {
    pub fn new(configs: &[FlipperConfig]) -> Self {
        Self::in_frame(configs, Quat::IDENTITY, 6.5)
    }

    pub fn in_frame(configs: &[FlipperConfig], frame: Quat, pitch_deg: f32) -> Self {
        Self::with_world(configs, floor(frame), frame * pitched_gravity(pitch_deg))
    }

    pub fn with_world(configs: &[FlipperConfig], world: StaticWorld, gravity: Vec3) -> Self {
        let mut cfg = BallSolverConfig::from_defaults(&PhysicsDefaults::builtin());
        cfg.gravity = gravity;
        Self {
            world,
            solver: BallSolver::new(cfg, &profiles()),
            flippers: FlipperSet::new(configs).expect("valid flippers"),
            events: BallEvents::new(),
            tick: 0,
            hits: Vec::with_capacity(64),
        }
    }

    pub fn add_ball(&mut self, position: Vec3, velocity: Vec3) -> BallHandle {
        let mut ball = Ball::new(STEEL, position);
        ball.velocity = velocity;
        self.solver.add_ball(ball).expect("room for a ball")
    }

    pub fn ball(&self, h: BallHandle) -> Ball {
        *self.solver.ball(h).expect("ball exists")
    }

    /// One tick in the documented order: Thing Flip schedule, then the ball
    /// step with the flippers as body set 0.
    pub fn step(&mut self) {
        self.flippers.step_tick(self.tick);
        self.events.clear();
        let geo = Geometry::new(&self.world);
        self.solver
            .step_with_bodies(&geo, &mut [&mut self.flippers], DT, &mut self.events);
        self.hits.clear();
        for e in self.events.as_slice() {
            if let BallEvent::Impact {
                ball,
                other: ImpactTarget::Body { set: 0, body },
                normal_speed,
            } = *e
            {
                self.hits.push((ball, body, normal_speed));
            }
        }
        self.tick += 1;
    }

    pub fn run(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.step();
        }
    }

    /// Bit pattern of every ball and flipper.
    pub fn state_bits(&self) -> Vec<u32> {
        let mut out = Vec::new();
        for (h, b) in self.solver.balls() {
            out.push(u32::from(h.0));
            for x in b
                .position
                .to_array()
                .into_iter()
                .chain(b.velocity.to_array())
                .chain(b.angular_velocity.to_array())
            {
                out.push(x.to_bits());
            }
        }
        for i in 0..self.flippers.len() {
            let id = self.flippers.id_of(i).unwrap();
            out.push(self.flippers.angle_deg(id).to_bits());
            out.push(self.flippers.angular_velocity(id).to_bits());
            out.push(self.flippers.base_offset_mm(id).to_bits());
            out.push(self.flippers.coil_torque_nm(id).to_bits());
            out.push(u32::from(self.flippers.eos_closed(id)));
        }
        out
    }
}

/// Unit vector along a bat at `angle_deg` in the flat (z-normal) frame.
pub fn bat_dir(angle_deg: f32) -> Vec3 {
    let a = angle_deg.to_radians();
    v(libm::cosf(a), libm::sinf(a), 0.0)
}
