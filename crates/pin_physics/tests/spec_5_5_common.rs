//! Shared helpers of the Subphase 5.5 physics spec tests (Independent
//! Tester), included with `#[path]` (an empty test crate on its own).
//!
//! Written black-box from the 5.5 brief against the merged public
//! `pin_physics` API: `FlipperSet` (2.3), `BallSolver::step_with_bodies`
//! (2.2-S), `StaticWorldBuilder` layer masks and `TransitionSet` (2.1) and
//! `world::layers` (bit 4 = backbox). The flipper rig and the shot
//! measurement come from the 2.3 spec helpers (`spec_2_3_common.rs`).
//!
//! # Frames
//!
//! Every scene is first described in the *table frame* of the flat
//! playfield (x across, y up the table, z the playfield normal). The
//! horizontal case uses it as is, with the 6.5° pitch gravity. The vertical
//! (backbox) case rotates the whole rig by [`vertical_frame`], a +90° turn
//! about world +X: table +y becomes world up (+Z) and the playfield normal
//! becomes world −Y, so the mounting plane is a 90° (vertical) wall and
//! world gravity `(0, 0, −g)` lies entirely in the plane, pulling "down the
//! table" with the full `g`.
#![allow(dead_code)]

#[path = "spec_2_3_common.rs"]
pub mod c23;

use c23::{BatFrame, DT, R, Scene, Shot, config, measure_frame};
use glam::{Quat, Vec3};
use pin_core::ids::FlipperId;
use pin_core::{CabinetAxis, CabinetInput, FlipperConfig, FlipperStagingMode};
use pin_physics::ball::{BallHandle, MovingBodies};
use pin_physics::flipper::FlipperSet;
use pin_physics::world::{StaticShape, StaticWorld, StaticWorldBuilder, TransitionSet, layers};

/// Standard gravity used by the 2.3 rigs, m/s².
pub const G: f32 = 9.81;
/// Pitch of the horizontal playfield, degrees.
pub const PITCH_DEG: f32 = 6.5;
/// Layer index of the backbox (bit 4).
pub const BACKBOX_LAYER: u8 = 4;

/// The rotation that stands the flat table frame up as a vertical
/// (90°) backbox plane: +90° about world +X.
pub fn vertical_frame() -> Quat {
    Quat::from_rotation_x(90.0f32.to_radians())
}

/// Table-frame gravity of the flat playfield pitched by [`PITCH_DEG`].
pub fn gravity_flat_table() -> Vec3 {
    let p = PITCH_DEG.to_radians();
    Vec3::new(0.0, -G * libm::sinf(p), -G * libm::cosf(p))
}

/// World gravity of the vertical rig: straight down, `(0, 0, −g)`. In the
/// table frame (rotated back by [`vertical_frame`]) it is `(0, −g, 0)`:
/// all of it along the plane.
pub fn gravity_vertical_world() -> Vec3 {
    Vec3::new(0.0, 0.0, -G)
}

/// A large mounting slab (top face through the origin, normal `rot · z`),
/// colliding on `mask` only.
pub fn plane_world(rot: Quat, mask: u8) -> StaticWorld {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        StaticShape::Cuboid {
            center: rot * Vec3::new(0.0, 0.0, -0.05),
            half_extents: Vec3::new(0.6, 0.8, 0.05),
            rotation: rot,
        },
        mask,
        true,
    );
    b.build()
}

/// One of the two mountings compared by the shot-speed test.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Mount {
    /// The flat playfield (layer 0, 6.5° pitch).
    Horizontal,
    /// The 90° backbox plane (layer 4, all of gravity along the plane).
    Vertical,
}

impl Mount {
    /// World rotation of the table frame.
    pub fn rot(self) -> Quat {
        match self {
            Mount::Horizontal => Quat::IDENTITY,
            Mount::Vertical => vertical_frame(),
        }
    }

    /// Collision layer index of the mounting.
    pub fn layer(self) -> u8 {
        match self {
            Mount::Horizontal => 0,
            Mount::Vertical => BACKBOX_LAYER,
        }
    }

    /// World gravity.
    pub fn gravity(self) -> Vec3 {
        match self {
            Mount::Horizontal => gravity_flat_table(),
            Mount::Vertical => gravity_vertical_world(),
        }
    }

    /// Gravity in the table frame.
    pub fn gravity_table(self) -> Vec3 {
        self.rot().inverse() * self.gravity()
    }
}

/// A left flipper (Appendix C defaults, 76 mm capsule, rest −30°, sweep
/// +50°) with its pivot at the table-frame origin, mounted on `mount`, with
/// the coil's power torque scaled by `power_scale`.
pub fn left_flipper(id: u16, mount: Mount, power_scale: f32) -> FlipperConfig {
    left_flipper_at(id, mount, power_scale, Vec3::ZERO)
}

/// [`left_flipper`] with the pivot at `pivot_table_m` (table frame, metres).
pub fn left_flipper_at(
    id: u16,
    mount: Mount,
    power_scale: f32,
    pivot_table_m: Vec3,
) -> FlipperConfig {
    let rot = mount.rot();
    let n = rot * Vec3::Z;
    let base = rot * pivot_table_m * 1.0e3;
    let mut c = config(id, base.to_array(), n, -30.0, 50.0);
    c.power_torque_nm *= power_scale;
    c.collision_layer = mount.layer();
    c
}

/// The mirror image: rest 210°, sweep −50°.
pub fn right_flipper_at(id: u16, mount: Mount, pivot_table_m: Vec3) -> FlipperConfig {
    let mut c = left_flipper_at(id, mount, 1.0, pivot_table_m);
    c.rest_angle_deg = 210.0;
    c.sweep_angle_deg = -50.0;
    c.staging = FlipperStagingMode::DedicatedButton(CabinetInput::RightFlipper);
    c
}

/// Signed distance from `p` to the surface of flipper body `body` at the
/// start of the tick.
pub fn bat_gap(set: &FlipperSet, body: usize, p: Vec3) -> f32 {
    set.closest_point(body, 0.0, p)
        .map_or(f32::INFINITY, |bp| bp.distance)
}

/// One cradled-ball shot rig: the flipper at rest on `mount`, a standard
/// ball at rest on the plane touching the upper (sweep) side of the bat at
/// `offset` metres from the pivot along the bat, `gap` metres off the
/// rubber.
pub struct ShotRig {
    pub mount: Mount,
    pub scene: Scene,
    pub frame: BatFrame,
    pub ball: BallHandle,
    pub cfg: FlipperConfig,
}

impl ShotRig {
    pub fn new(mount: Mount, power_scale: f32, offset: f32, gap: f32) -> Self {
        let cfg = left_flipper(0, mount, power_scale);
        let rot = mount.rot();
        let mut scene = Scene::new(
            plane_world(rot, c23_layer_bit(mount.layer())),
            mount.gravity(),
            std::slice::from_ref(&cfg),
        );
        // One empty tick so the bat pose is committed, then measure it.
        scene.set.advance(DT);
        let frame = measure_frame(&scene.set, 0, &cfg);
        // The ball rests on the plane (centre one radius above it) beside
        // the bat: start on the bat's sweep side at `offset` along it and
        // bisect the distance off the bat until the gap to the rubber is
        // `gap`.
        let n = rot * Vec3::Z;
        let e = frame.e_rest;
        let m = n.cross(e); // CCW side = the side the bat sweeps toward
        let at = |d: f32| e * offset + m * d + n * R;
        let (mut lo, mut hi) = (0.0f32, 0.08f32);
        for _ in 0..60 {
            let mid = 0.5 * (lo + hi);
            if bat_gap(&scene.set, 0, at(mid)) - R < gap {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let p = at(0.5 * (lo + hi));
        let mut b = c23::ball(p, Vec3::ZERO);
        b.layer = mount.layer();
        let ball = scene.solver.add_ball(b).expect("room for a ball");
        ShotRig {
            mount,
            scene,
            frame,
            ball,
            cfg,
        }
    }

    /// Energises the coil at the next tick and measures the shot (the 2.3
    /// rebound measurement: speed when the ball first clears the bat by
    /// 15 mm after its first impact).
    pub fn shoot(&mut self, ticks: u64) -> Shot {
        let id = FlipperId(0);
        c23::run_shot(
            &mut self.scene,
            self.ball,
            &self.frame,
            0,
            ticks,
            |_, set| set.set_energised(id, true),
        )
    }
}

fn c23_layer_bit(layer: u8) -> u8 {
    layers::layer_bit(layer)
}

/// A test-local staging router with the documented semantics of the
/// staging modes (pin_input `staging`, DECISIONS "3.1 dual-stage
/// semantics"): `DedicatedButton(i)` / `ParallelLinked(i)` are energised
/// iff cabinet input `i` is pressed; a `DualStageThreshold` flipper is
/// energised iff its axis is at or above its threshold (the lower-stage
/// flipper uses `lower_threshold`, the upper-stage one `upper_threshold`).
/// `pin_physics` does not depend on `pin_input`, so the physics tests route
/// the frame themselves and drive the solver through `set_energised`.
#[derive(Copy, Clone, Debug, Default)]
pub struct Frame {
    pub left: bool,
    pub right: bool,
    pub axis: f32,
}

/// Which stage of a dual-stage button a flipper sits on.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    Lower,
    Upper,
}

/// Whether `cfg` is energised for `frame` (`stage` used only by the
/// dual-stage mode).
pub fn route(cfg: &FlipperConfig, stage: Stage, frame: &Frame) -> bool {
    match cfg.staging {
        FlipperStagingMode::DedicatedButton(i) | FlipperStagingMode::ParallelLinked(i) => match i {
            CabinetInput::LeftFlipper => frame.left,
            CabinetInput::RightFlipper => frame.right,
            _ => false,
        },
        FlipperStagingMode::DualStageThreshold {
            lower_threshold,
            upper_threshold,
            ..
        } => match stage {
            Stage::Lower => frame.axis >= lower_threshold,
            Stage::Upper => frame.axis >= upper_threshold,
        },
        FlipperStagingMode::AutoSensorThingFlip { .. } => false,
    }
}

/// The dual-stage mode of the brief: lower stage at 30 %, upper at 85 %.
pub fn dual_stage() -> FlipperStagingMode {
    FlipperStagingMode::DualStageThreshold {
        axis: CabinetAxis::LeftFlipper,
        lower_threshold: 0.30,
        upper_threshold: 0.85,
    }
}

/// Applies the zone set to every ball after a step (the engine does this
/// per tick: `layer = transitions.update(layer, prev, pos)`).
pub fn apply_transitions(
    solver: &mut pin_physics::ball::BallSolver,
    set: &TransitionSet,
    prev: &[(BallHandle, Vec3)],
) {
    for &(h, p) in prev {
        if let Some(b) = solver.ball_mut(h) {
            b.layer = set.update(b.layer, p, b.position);
        }
    }
}

/// FNV-1a over 32-bit words (stable, platform independent).
pub fn fnv1a(words: &[u32]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for w in words {
        for b in w.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}
