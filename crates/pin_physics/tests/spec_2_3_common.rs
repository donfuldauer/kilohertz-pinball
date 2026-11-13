//! Shared helpers for the Subphase 2.3 (flipper solver) spec tests,
//! included with `#[path]` (an empty test crate on its own).
//!
//! Written black-box from the 2.3 brief: the tests use only the fixed public
//! API (`FlipperSet::new`, `set_energised`, `set_zipper`, `on_switch_edge`,
//! `step_tick`, `angle_deg`, `angular_velocity`, `eos_closed`,
//! `base_offset_mm`) and the 2.2-S `MovingBodies` trait the set implements.
//! Conventions the brief leaves open (the in-plane zero direction of the
//! angle, how far the bat axis sits from `base_position_mm` along the
//! normal) are *measured* by probing `MovingBodies::closest_point` on a bat
//! at rest, never assumed.
#![allow(dead_code)]

use glam::{Quat, Vec3};
use pin_core::ids::{CoilId, FlipperId, ProfileId, SwitchId};
use pin_core::{CabinetInput, FlipperConfig, FlipperShape, FlipperStagingMode, PhysicsDefaults};
use pin_physics::ball::{
    Ball, BallEvent, BallEvents, BallHandle, BallSolver, BallSolverConfig, Geometry, ImpactTarget,
    MovingBodies,
};
use pin_physics::flipper::FlipperSet;
use pin_physics::world::{StaticShape, StaticWorld, StaticWorldBuilder, layers};

pub const DT: f32 = 1.0e-3;
pub const STEEL: ProfileId = ProfileId(0);
/// standard_steel radius, m (27 mm ball).
pub const R: f32 = 0.0135;
pub const PITCH_DEG: f32 = 6.5;
/// 76 mm StraightCapsule (Appendix C bat).
pub const L_MM: f32 = 76.0;
pub const BASE_R_MM: f32 = 11.0;
pub const TIP_R_MM: f32 = 6.5;
pub const L: f32 = L_MM * 1.0e-3;
pub const BASE_R: f32 = BASE_R_MM * 1.0e-3;
pub const TIP_R: f32 = TIP_R_MM * 1.0e-3;

/// Height of `base_position_mm` above the playfield used by the flat scenes:
/// the base on the playfield, as in the table schema examples (the bat's
/// extent along the normal is measured, see [`measure_frame`]).
pub const PIVOT_Z_MM: f32 = 0.0;

pub fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

/// A flipper config with the Appendix C defaults and a 76 mm capsule
/// (base radius 11 mm, tip 6.5 mm), EOS at 0.92 of the sweep.
pub fn config(
    id: u16,
    base_mm: [f32; 3],
    normal: Vec3,
    rest_deg: f32,
    sweep_deg: f32,
) -> FlipperConfig {
    // Appendix C defaults resolved from `physics_defaults.toml`.
    let p = PhysicsDefaults::builtin().resolve(None);
    FlipperConfig {
        id: FlipperId(id),
        coil: CoilId(100 + id),
        eos_switch: SwitchId(200 + id),
        collision_layer: 0,
        base_position_mm: base_mm,
        surface_normal: normal.to_array(),
        rest_angle_deg: rest_deg,
        sweep_angle_deg: sweep_deg,
        eos_angle_fraction: 0.92,
        shape: FlipperShape::StraightCapsule {
            length_mm: L_MM,
            base_radius_mm: BASE_R_MM,
            tip_radius_mm: TIP_R_MM,
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

pub fn profiles() -> Vec<pin_core::BallProfile> {
    pin_core::profiles::builtin_profiles().collect()
}

/// Gravity of a 6.5° playfield (y up the table, z out of the playfield),
/// rotated by `rot`.
pub fn table_gravity(rot: Quat) -> Vec3 {
    let p = PITCH_DEG.to_radians();
    rot * v(0.0, -9.81 * libm::sinf(p), -9.81 * libm::cosf(p))
}

/// A playfield slab (top face at z = 0), rotated by `rot`.
pub fn table_world(rot: Quat) -> StaticWorld {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        StaticShape::Cuboid {
            center: rot * v(0.0, 0.0, -0.05),
            half_extents: v(0.6, 0.8, 0.05),
            rotation: rot,
        },
        layers::ALL,
        true,
    );
    b.build()
}

/// A world with one tiny box far away (free-space scenes).
pub fn empty_world() -> StaticWorld {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        StaticShape::Cuboid {
            center: v(50.0, 50.0, 50.0),
            half_extents: v(0.01, 0.01, 0.01),
            rotation: Quat::IDENTITY,
        },
        layers::ALL,
        true,
    );
    b.build()
}

pub fn solver(gravity: Vec3) -> BallSolver {
    let mut cfg = BallSolverConfig::from_defaults(&PhysicsDefaults::builtin());
    cfg.gravity = gravity;
    BallSolver::new(cfg, &profiles())
}

pub fn ball(pos: Vec3, vel: Vec3) -> Ball {
    let mut b = Ball::new(STEEL, pos);
    b.velocity = vel;
    b
}

/// One flipper set on a ball solver: the engine order is
/// `[&mut flippers]` (set 0).
pub struct Scene {
    pub world: StaticWorld,
    pub solver: BallSolver,
    pub set: FlipperSet,
    /// Index of the next tick to run.
    pub tick: u64,
    pub ev: BallEvents,
}

impl Scene {
    pub fn new(world: StaticWorld, gravity: Vec3, configs: &[FlipperConfig]) -> Self {
        let set = match FlipperSet::new(configs) {
            Ok(s) => s,
            Err(e) => panic!("FlipperSet::new rejected a valid config: {e:?}"),
        };
        Self {
            world,
            solver: solver(gravity),
            set,
            tick: 0,
            ev: BallEvents::new(),
        }
    }

    /// One 1 ms tick in the documented order: the flipper set's per-tick
    /// hook, then the ball step with the set lent as body set 0.
    pub fn advance(&mut self) {
        self.ev.clear();
        self.set.step_tick(self.tick);
        let geo = Geometry::new(&self.world);
        self.solver
            .step_with_bodies(&geo, &mut [&mut self.set], DT, &mut self.ev);
        self.tick += 1;
    }

    /// True when this tick reported an impact of `h` with flipper `body`.
    pub fn hit(&self, h: BallHandle, body: usize) -> bool {
        self.ev.as_slice().iter().any(|e| {
            matches!(e, BallEvent::Impact { ball, other: ImpactTarget::Body { set: 0, body: b }, .. }
                if *ball == h && *b == body)
        })
    }

    pub fn ball(&self, h: BallHandle) -> Ball {
        *self.solver.ball(h).expect("ball present")
    }
}

/// An orthonormal in-plane basis (u, w) for normal `n` (u × w = n).
pub fn basis(n: Vec3) -> (Vec3, Vec3) {
    let a = if n.x.abs() < 0.9 { Vec3::X } else { Vec3::Y };
    let u = (a - n * a.dot(n)).normalize();
    let w = n.cross(u);
    (u, w)
}

fn probe(set: &FlipperSet, body: usize, p: Vec3) -> f32 {
    set.closest_point(body, 0.0, p)
        .map_or(f32::INFINITY, |bp| bp.distance)
}

/// The world-space frame of one flipper, measured on a bat at rest.
#[derive(Copy, Clone, Debug)]
pub struct BatFrame {
    /// Point on the rotation axis level with the bat (base position moved
    /// along the normal to the bat's mid-plane).
    pub pivot: Vec3,
    /// Unit normal.
    pub n: Vec3,
    /// Unit direction pivot → tip at rest.
    pub e_rest: Vec3,
    /// `rest_angle_deg` of the config.
    pub rest_deg: f32,
    /// Offset of the bat's mid-plane from `base_position` along `n`, m.
    pub lift: f32,
}

impl BatFrame {
    /// Direction pivot → tip at absolute angle `angle_deg` (rotation about
    /// `n` by `angle − rest`, CCW positive: the §3.3 sign convention).
    pub fn dir(&self, angle_deg: f32) -> Vec3 {
        Quat::from_axis_angle(self.n, (angle_deg - self.rest_deg).to_radians()) * self.e_rest
    }

    /// Distance from `p` to the bat axis segment at `angle_deg` (pivot
    /// moved by `shift`).
    pub fn axis_distance(&self, angle_deg: f32, shift: Vec3, p: Vec3) -> f32 {
        let a = self.pivot + shift;
        let e = self.dir(angle_deg);
        let t = (p - a).dot(e).clamp(0.0, L);
        (p - (a + e * t)).length()
    }

    /// (along the bat, signed distance on the CCW side, along the normal)
    /// of `p` at `angle_deg`.
    pub fn local(&self, angle_deg: f32, shift: Vec3, p: Vec3) -> (f32, f32, f32) {
        let d = p - (self.pivot + shift);
        let e = self.dir(angle_deg);
        let m = self.n.cross(e);
        (d.dot(e), d.dot(m), d.dot(self.n))
    }
}

/// Measures the frame of `body` in `set` (which must be at rest, after at
/// least one tick): the bat direction from the in-plane minimum of the
/// signed distance on a circle of half the bat length, the mid-plane offset
/// from distances above and below.
pub fn measure_frame(set: &FlipperSet, body: usize, cfg: &FlipperConfig) -> BatFrame {
    let n = Vec3::from_array(cfg.surface_normal).normalize();
    let base = Vec3::from_array(cfg.base_position_mm) * 1.0e-3;
    let rho = 0.5 * cfg.shape.length_mm() * 1.0e-3;
    // Pass 1: direction just off the base plane, then the mid-plane offset;
    // pass 2: direction again in the mid-plane.
    let e1 = probe_direction(set, body, base + n * 0.002, n, rho);
    let mid = base + e1 * rho;
    let d_up = probe(set, body, mid + n * 0.1);
    let d_dn = probe(set, body, mid - n * 0.1);
    let lift = 0.5 * (d_dn - d_up);
    let e_rest = probe_direction(set, body, base + n * lift, n, rho);
    BatFrame {
        pivot: base + n * lift,
        n,
        e_rest,
        rest_deg: cfg.rest_angle_deg,
        lift,
    }
}

fn probe_direction(set: &FlipperSet, body: usize, centre: Vec3, n: Vec3, rho: f32) -> Vec3 {
    let (u, w) = basis(n);
    let at = |phi: f32| centre + (u * libm::cosf(phi) + w * libm::sinf(phi)) * rho;
    let mut best = (f32::INFINITY, 0.0f32);
    for k in 0..720 {
        let phi = k as f32 * (std::f32::consts::TAU / 720.0);
        let d = probe(set, body, at(phi));
        if d < best.0 {
            best = (d, phi);
        }
    }
    // Golden-section refinement within ±0.5°.
    let (mut lo, mut hi) = (best.1 - 0.5f32.to_radians(), best.1 + 0.5f32.to_radians());
    for _ in 0..40 {
        let m1 = lo + (hi - lo) * 0.382;
        let m2 = lo + (hi - lo) * 0.618;
        if probe(set, body, at(m1)) < probe(set, body, at(m2)) {
            hi = m2;
        } else {
            lo = m1;
        }
    }
    let phi = 0.5 * (lo + hi);
    (u * libm::cosf(phi) + w * libm::sinf(phi)).normalize()
}

/// Frames of every flipper of `configs`, measured on a throw-away set
/// after one empty tick.
pub fn frames(configs: &[FlipperConfig]) -> Vec<BatFrame> {
    let mut s = Scene::new(empty_world(), Vec3::ZERO, configs);
    s.advance();
    (0..configs.len())
        .map(|i| measure_frame(&s.set, i, &configs[i]))
        .collect()
}

/// Signed angle (degrees) from `a` to `b` about `n`.
pub fn signed_angle_deg(a: Vec3, b: Vec3, n: Vec3) -> f32 {
    libm::atan2f(a.cross(b).dot(n), a.dot(b)).to_degrees()
}

/// `rest_angle_deg` that makes the bat of a flipper with normal `n` at
/// `base_mm` point along `want` at rest (measured: the in-plane zero
/// direction is the implementation's choice).
pub fn rest_for(base_mm: [f32; 3], n: Vec3, sweep_deg: f32, want: Vec3) -> f32 {
    let c = config(0, base_mm, n, 0.0, sweep_deg);
    let f = frames(&[c])[0];
    let mut a = signed_angle_deg(f.e_rest, want, n);
    if a <= -180.0 {
        a += 360.0;
    }
    a
}

/// World direction (in the z = 0 playfield plane) at `deg` from +x.
pub fn flat_dir(deg: f32) -> Vec3 {
    let r = deg.to_radians();
    v(libm::cosf(r), libm::sinf(r), 0.0)
}

/// Result of one shot measurement.
#[derive(Copy, Clone, Debug)]
pub struct Shot {
    /// Tick (index) of the first impact with the bat.
    pub impact_tick: Option<u64>,
    /// Ball speed when it first cleared the bat by `CLEAR` (or at the end).
    pub speed: f32,
    pub velocity: Vec3,
    /// True when the ball never cleared the bat within the window.
    pub caught: bool,
    /// Tick at which the ball cleared the bat.
    pub clear_tick: Option<u64>,
}

/// Clearance (beyond ball radius + base radius from the bat axis) at which
/// the ball counts as having left the bat.
pub const CLEAR: f32 = 0.015;

/// Runs `ticks` ticks of `scene` calling `drive(tick, set)` before each,
/// and measures the ball `h` against flipper `body`: the speed when it
/// first clears the bat axis by `R + BASE_R + CLEAR` after its first impact
/// with the bat. "Rebound speed" of the 2.3 verification criteria.
pub fn run_shot(
    scene: &mut Scene,
    h: BallHandle,
    frame: &BatFrame,
    body: usize,
    ticks: u64,
    mut drive: impl FnMut(u64, &mut FlipperSet),
) -> Shot {
    let id = FlipperId(body as u16);
    let mut impact = None;
    let mut contact_seen = false;
    for _ in 0..ticks {
        drive(scene.tick, &mut scene.set);
        let t = scene.tick;
        scene.advance();
        if impact.is_none() && scene.hit(h, body) {
            impact = Some(t);
        }
        let b = scene.ball(h);
        let d = frame.axis_distance(scene.set.angle_deg(id), Vec3::ZERO, b.position);
        if impact.is_some() {
            if d < R + BASE_R + 0.002 {
                contact_seen = true;
            }
            if contact_seen && d >= R + BASE_R + CLEAR {
                return Shot {
                    impact_tick: impact,
                    speed: b.velocity.length(),
                    velocity: b.velocity,
                    caught: false,
                    clear_tick: Some(t),
                };
            }
        }
    }
    let b = scene.ball(h);
    Shot {
        impact_tick: impact,
        speed: b.velocity.length(),
        velocity: b.velocity,
        caught: impact.is_some(),
        clear_tick: None,
    }
}

/// Ball + bat mechanical energy: ball translational + rotational kinetic
/// energy, gravitational potential (relative to the origin) and the bats'
/// rotational kinetic energy `½ I ω²`.
pub fn energy(scene: &Scene, gravity: Vec3, inertias: &[f32]) -> f64 {
    let prof = pin_core::builtin_profile("standard_steel").unwrap();
    let m = f64::from(prof.mass_kg);
    let r = f64::from(prof.radius_m());
    let mut e = 0.0f64;
    for (_, b) in scene.solver.balls() {
        let vv = f64::from(b.velocity.length_squared());
        let ww = f64::from(b.angular_velocity.length_squared());
        e += 0.5 * m * vv + 0.5 * (0.4 * m * r * r) * ww - m * f64::from(gravity.dot(b.position));
    }
    for (i, &inertia) in inertias.iter().enumerate() {
        let w = f64::from(scene.set.angular_velocity(FlipperId(i as u16)));
        e += 0.5 * f64::from(inertia) * w * w;
    }
    e
}
