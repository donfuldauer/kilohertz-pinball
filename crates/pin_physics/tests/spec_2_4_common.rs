//! Shared helpers for the Subphase 2.4 independent spec tests (`spec_2_4_*.rs`).
//!
//! Included with `#[path = "spec_2_4_common.rs"] mod common;`. Compiled on its
//! own it is an empty test binary.
//!
//! Written from the 2.4 brief. Lookups of the implementation's public surface
//! (signatures and doc comments only): `MechSpec` (input type of
//! `MechSet::new`), `MechError`, `MechSet::{step, pre_step, post_step,
//! set_body_set, edges, switch_states, mech_switch_closed, slider_state,
//! spinner_state, set_spinner_speed, drop_target_down, vuk_ball, transitions,
//! set_tilt_switch, coil_on, kind, len}`, `TiltBob`/`TiltBobParams`, the module
//! table of what `position`/`angle_deg` mean per kind, and the CR-004 `pin_core`
//! types (`MechParams`, `MechDefaults`).
#![allow(dead_code)]

use glam::{Quat, Vec3};
use pin_core::ids::{CoilId, SwitchId};
use pin_core::profiles::{BUILTIN_PROFILE_NAMES, builtin_profiles};
use pin_core::schema::table::{MechKind, MechParams};
use pin_core::script::SwitchEdge;
use pin_core::{BallProfile, PhysicsDefaults, ProfileId};
use pin_physics::ball::{Ball, BallEvents, BallHandle, BallSolver, BallSolverConfig, Geometry};
use pin_physics::mech::{MechSet, MechSpec};
use pin_physics::world::{StaticShape, StaticWorld, StaticWorldBuilder, layers};

/// The engine tick.
pub const DT: f32 = 0.001;
/// Radius of the standard steel ball.
pub const R: f32 = 0.0135;
/// Appendix C feel window of bumper and sling exit speeds, m/s.
pub const EXIT_MIN: f32 = 1.5;
pub const EXIT_MAX: f32 = 4.0;

pub fn profiles() -> Vec<BallProfile> {
    builtin_profiles().collect()
}

pub fn pid(name: &str) -> ProfileId {
    let i = BUILTIN_PROFILE_NAMES
        .iter()
        .position(|n| *n == name)
        .unwrap_or_else(|| panic!("unknown builtin profile {name}"));
    ProfileId(i as u16)
}

pub fn steel() -> BallProfile {
    profiles()[pid("standard_steel").index()].clone()
}

pub fn defaults() -> PhysicsDefaults {
    PhysicsDefaults::builtin()
}

pub fn solver_with_gravity(gravity: Vec3) -> BallSolver {
    let mut cfg = BallSolverConfig::from_defaults(&defaults());
    cfg.gravity = gravity;
    BallSolver::new(cfg, &profiles())
}

/// Gravity of a playfield pitched by `deg` in the playfield frame (+z the
/// playfield normal, +y up the table).
pub fn pitched_gravity(deg: f32) -> Vec3 {
    let a = deg.to_radians();
    Vec3::new(0.0, -9.81 * a.sin(), -9.81 * a.cos())
}

/// A 4 m × 4 m slab whose top face is the plane z = `top`, on `layer_mask`.
pub fn slab(b: &mut StaticWorldBuilder, top: f32, layer_mask: u8) {
    b.add_collider(
        StaticShape::Cuboid {
            center: Vec3::new(0.0, 0.0, top - 0.05),
            half_extents: Vec3::new(2.0, 2.0, 0.05),
            rotation: Quat::IDENTITY,
        },
        layer_mask,
        true,
    );
}

pub fn floor_world() -> StaticWorld {
    let mut b = StaticWorldBuilder::new();
    slab(&mut b, 0.0, layers::ALL);
    b.build()
}

pub fn empty_world() -> StaticWorld {
    StaticWorldBuilder::new().build()
}

pub fn spec(kind: MechKind, position: Vec3) -> MechSpec {
    MechSpec::new(kind, position)
}

pub fn params() -> MechParams {
    MechParams::default()
}

/// A steel ball centred at `position` with `velocity`.
pub fn steel_ball(position: Vec3, velocity: Vec3) -> Ball {
    let mut b = Ball::new(pid("standard_steel"), position);
    b.velocity = velocity;
    b
}

/// A world, a solver and a mech set stepped through `MechSet::step` (mechs
/// alone: body set 0).
pub struct Rig {
    pub world: StaticWorld,
    pub solver: BallSolver,
    pub mechs: MechSet,
    pub events: BallEvents,
    pub tick: u64,
}

impl Rig {
    pub fn new(world: StaticWorld, gravity: Vec3, specs: &[MechSpec]) -> Self {
        let mechs = MechSet::new(specs, &defaults()).expect("valid mech specs");
        Self {
            world,
            solver: solver_with_gravity(gravity),
            mechs,
            events: BallEvents::new(),
            tick: 0,
        }
    }

    /// Level floor at z = 0 with gravity straight down.
    pub fn on_floor(specs: &[MechSpec]) -> Self {
        Self::new(floor_world(), Vec3::new(0.0, 0.0, -9.81), specs)
    }

    /// No colliders but the mechs, no gravity.
    pub fn in_space(specs: &[MechSpec]) -> Self {
        Self::new(empty_world(), Vec3::ZERO, specs)
    }

    pub fn add(&mut self, ball: Ball) -> BallHandle {
        self.solver.add_ball(ball).expect("ball slot")
    }

    pub fn ball(&self, h: BallHandle) -> Ball {
        *self.solver.ball(h).expect("ball exists")
    }

    /// One 1 ms tick; `events` holds this tick's events afterwards.
    pub fn step(&mut self) {
        self.events.clear();
        let geo = Geometry::new(&self.world);
        self.mechs
            .step(&mut self.solver, &geo, None, DT, &mut self.events);
        self.tick += 1;
    }

    pub fn run(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.step();
        }
    }

    pub fn switch_bit(&self, s: SwitchId) -> bool {
        let mut out = [0u64; 4];
        self.mechs.switch_states(&mut out);
        bit(&out, s)
    }

    pub fn edge(&self, s: SwitchId) -> Option<SwitchEdge> {
        self.mechs
            .edges()
            .iter()
            .find(|(id, _)| *id == s)
            .map(|(_, e)| *e)
    }

    pub fn closed_edge(&self, s: SwitchId) -> bool {
        self.edge(s) == Some(SwitchEdge::Closed)
    }

    pub fn opened_edge(&self, s: SwitchId) -> bool {
        self.edge(s) == Some(SwitchEdge::Opened)
    }
}

pub fn bit(words: &[u64; 4], s: SwitchId) -> bool {
    let i = s.index();
    words[i / 64] >> (i % 64) & 1 == 1
}

/// Horizontal (playfield-plane) part of `v`.
pub fn horiz(v: Vec3) -> Vec3 {
    Vec3::new(v.x, v.y, 0.0)
}

/// Unit vector in the playfield plane at `deg` from +x.
pub fn dir_deg(deg: f32) -> Vec3 {
    let a = deg.to_radians();
    Vec3::new(a.cos(), a.sin(), 0.0)
}

pub fn is_finite_ball(b: &Ball) -> bool {
    b.position.is_finite() && b.velocity.is_finite() && b.angular_velocity.is_finite()
}

/// Every float of a ball's state as raw bits.
pub fn state_bits(b: &Ball) -> [u32; 14] {
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
        u32::from(b.layer),
    ]
}

/// FNV-1a over 32-bit words.
#[derive(Copy, Clone)]
pub struct Hash(pub u64);

impl Hash {
    pub fn new() -> Self {
        Hash(0xcbf2_9ce4_8422_2325)
    }
    pub fn u32(&mut self, x: u32) {
        for b in x.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    pub fn u64(&mut self, x: u64) {
        self.u32(x as u32);
        self.u32((x >> 32) as u32);
    }
    pub fn f32(&mut self, x: f32) {
        self.u32(x.to_bits());
    }
    pub fn vec3(&mut self, v: Vec3) {
        self.f32(v.x);
        self.f32(v.y);
        self.f32(v.z);
    }
}

/// Switch and coil numbers of [`busy_specs`].
pub mod ids {
    use super::{CoilId, SwitchId};
    pub const BUMPER_SW: SwitchId = SwitchId(10);
    pub const BUMPER_COIL: CoilId = CoilId(1);
    pub const SLING_SW: SwitchId = SwitchId(11);
    pub const SLING_COIL: CoilId = CoilId(2);
    pub const DROP_A_SW: SwitchId = SwitchId(12);
    pub const DROP_B_SW: SwitchId = SwitchId(13);
    pub const DROP_COIL: CoilId = CoilId(3);
    pub const SPINNER_SW: SwitchId = SwitchId(70);
    pub const VUK_SW: SwitchId = SwitchId(15);
    pub const VUK_COIL: CoilId = CoilId(4);
    pub const PLUNGER_SW: SwitchId = SwitchId(130);
    pub const PLUNGER_COIL: CoilId = CoilId(5);
    pub const KICKBACK_SW: SwitchId = SwitchId(17);
    pub const KICKBACK_COIL: CoilId = CoilId(6);
    pub const TILT_SW: SwitchId = SwitchId(200);
}

/// Mech indices of [`busy_specs`].
pub mod ix {
    pub const BUMPER: usize = 0;
    pub const SLING: usize = 1;
    pub const DROP_A: usize = 2;
    pub const DROP_B: usize = 3;
    pub const SPINNER: usize = 4;
    pub const VUK: usize = 5;
    pub const PLUNGER: usize = 6;
    pub const KICKBACK: usize = 7;
    pub const GATE: usize = 8;
}

/// One mech of every simulated kind on a 0.5 m × 1.0 m playfield.
pub fn busy_specs() -> Vec<MechSpec> {
    use ids::*;
    let mut sling_p = params();
    sling_p.angle_deg = Some(45.0);
    let mut kick_p = params();
    kick_p.angle_deg = Some(90.0);
    let mut drop_p = params();
    drop_p.angle_deg = Some(270.0);
    vec![
        spec(MechKind::Bumper, Vec3::new(0.0, 0.25, 0.0))
            .with_coil(BUMPER_COIL)
            .with_switch(BUMPER_SW),
        spec(MechKind::Sling, Vec3::new(-0.15, -0.25, 0.0))
            .with_coil(SLING_COIL)
            .with_switch(SLING_SW)
            .with_params(sling_p),
        spec(MechKind::DropTarget, Vec3::new(-0.05, 0.42, 0.0))
            .with_coil(DROP_COIL)
            .with_switch(DROP_A_SW)
            .with_params(drop_p),
        spec(MechKind::DropTarget, Vec3::new(0.05, 0.42, 0.0))
            .with_coil(DROP_COIL)
            .with_switch(DROP_B_SW)
            .with_params(drop_p),
        spec(MechKind::Spinner, Vec3::new(0.12, 0.1, 0.0)).with_switch(SPINNER_SW),
        spec(MechKind::Vuk, Vec3::new(-0.12, 0.1, 0.0))
            .with_coil(VUK_COIL)
            .with_switch(VUK_SW),
        spec(MechKind::Plunger, Vec3::new(0.22, -0.42, 0.0))
            .with_coil(PLUNGER_COIL)
            .with_switch(PLUNGER_SW),
        spec(MechKind::Kickback, Vec3::new(-0.22, -0.40, 0.0))
            .with_coil(KICKBACK_COIL)
            .with_switch(KICKBACK_SW)
            .with_params(kick_p),
        spec(MechKind::Gate, Vec3::new(0.2, 0.3, 0.0)),
    ]
}

/// The playfield of [`busy_specs`]: main deck z = 0 (layer 0), an upper deck
/// with its top at z = 0.04 (layer 1), and four walls on every layer.
pub fn busy_world() -> StaticWorld {
    let mut b = StaticWorldBuilder::new();
    slab(&mut b, 0.0, layers::MAIN_DECK);
    slab(&mut b, 0.04, layers::UPPER_DECK);
    for (c, h) in [
        (Vec3::new(-0.26, 0.0, 0.1), Vec3::new(0.01, 0.52, 0.12)),
        (Vec3::new(0.26, 0.0, 0.1), Vec3::new(0.01, 0.52, 0.12)),
        (Vec3::new(0.0, -0.51, 0.1), Vec3::new(0.27, 0.01, 0.12)),
        (Vec3::new(0.0, 0.51, 0.1), Vec3::new(0.27, 0.01, 0.12)),
    ] {
        b.add_collider(
            StaticShape::Cuboid {
                center: c,
                half_extents: h,
                rotation: Quat::IDENTITY,
            },
            layers::ALL,
            true,
        );
    }
    b.build()
}

/// A rig with [`busy_specs`], four balls and the tilt switch.
pub fn busy_rig() -> Rig {
    let mut rig = Rig::new(busy_world(), pitched_gravity(6.5), &busy_specs());
    rig.mechs.set_tilt_switch(Some(ids::TILT_SW));
    let starts = [
        (
            "standard_steel",
            Vec3::new(0.0, 0.33, 0.0),
            Vec3::new(0.02, -1.0, 0.0),
        ),
        (
            "ceramic_powerball",
            Vec3::new(-0.1, -0.1, 0.0),
            Vec3::new(-0.4, -1.2, 0.0),
        ),
        (
            "standard_steel",
            Vec3::new(0.12, 0.0, 0.0),
            Vec3::new(0.0, 2.0, 0.0),
        ),
        (
            "mini_steel",
            Vec3::new(-0.2, 0.1, 0.0),
            Vec3::new(0.6, 0.0, 0.0),
        ),
        (
            "standard_steel",
            Vec3::new(0.05, 0.3, 0.0),
            Vec3::new(0.0, 1.5, 0.0),
        ),
    ];
    let profs = profiles();
    for (name, p, v) in starts {
        let r = profs[pid(name).index()].radius_m();
        let mut b = Ball::new(pid(name), p + Vec3::new(0.0, 0.0, r + 1.0e-4));
        b.velocity = v;
        rig.add(b);
    }
    rig
}

/// Scripted inputs of the busy scenario for tick `t` (before the step):
/// reactive coil pulses as a rules script would issue them, plus plunger,
/// nudges and resets on a schedule.
pub fn busy_drive(rig: &mut Rig, t: u32) {
    use ids::*;
    // React to last tick's edges.
    let mut fire: [Option<(CoilId, u16)>; 4] = [None; 4];
    for (s, e) in rig.mechs.edges() {
        if *e != SwitchEdge::Closed {
            continue;
        }
        if *s == BUMPER_SW {
            fire[0] = Some((BUMPER_COIL, 25));
        } else if *s == SLING_SW {
            fire[1] = Some((SLING_COIL, 30));
        } else if *s == KICKBACK_SW {
            fire[2] = Some((KICKBACK_COIL, 40));
        } else if *s == VUK_SW {
            fire[3] = Some((VUK_COIL, 50));
        }
    }
    for (c, ms) in fire.into_iter().flatten() {
        rig.mechs.fire_coil(c, ms);
    }
    if t % 700 == 650 {
        rig.mechs.fire_coil(DROP_COIL, 80);
    }
    // Plunger: pull over 200 ms, hold, release; repeat every second.
    let phase = t % 1000;
    let depth = if phase < 200 {
        phase as f32 / 200.0
    } else if phase < 400 {
        1.0
    } else {
        0.0
    };
    rig.mechs.set_plunger(depth);
    if t % 1500 == 900 {
        rig.mechs.hold_coil(PLUNGER_COIL, true);
    }
    if t % 1500 == 1300 {
        rig.mechs.hold_coil(PLUNGER_COIL, false);
    }
    if t % 400 == 123 {
        let a = t as f32 * 0.37;
        rig.mechs
            .nudge(Vec3::new(0.25 * a.cos(), 0.25 * a.sin(), 0.0));
    }
    if t == 777 {
        rig.mechs.set_spinner_speed(ix::SPINNER, 60.0);
    }
}

/// Hash of everything observable: balls, mech states, switches, tilt bob.
pub fn busy_hash(rig: &Rig, h: &mut Hash) {
    for (handle, b) in rig.solver.balls() {
        h.u32(u32::from(handle.0));
        for w in state_bits(b) {
            h.u32(w);
        }
    }
    for i in 0..rig.mechs.len() {
        if let Some((x, v)) = rig.mechs.slider_state(i) {
            h.f32(x);
            h.f32(v);
        }
        if let Some((a, w)) = rig.mechs.spinner_state(i) {
            h.f32(a);
            h.f32(w);
        }
        h.u64(rig.mechs.spinner_pulses(i));
        h.u32(u32::from(rig.mechs.mech_switch_closed(i)));
        h.u32(match rig.mechs.drop_target_down(i) {
            None => 2,
            Some(d) => u32::from(d),
        });
        h.u32(rig.mechs.vuk_ball(i).map_or(99, |b| u32::from(b.0)));
    }
    let mut sw = [0u64; 4];
    rig.mechs.switch_states(&mut sw);
    for w in sw {
        h.u64(w);
    }
    for (s, e) in rig.mechs.edges() {
        h.u32(u32::from(s.0));
        h.u32(u32::from(*e == SwitchEdge::Closed));
    }
    h.vec3(rig.mechs.tilt_bob_offset());
    h.u32(u32::from(rig.mechs.tilt_closed()));
}
