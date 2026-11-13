//! Shared helpers of the Subphase 2.7 independent spec tests
//! (`spec_2_7_*.rs`), written from the brief alone.
//!
//! Included with `#[path = "spec_2_7_common.rs"] mod common;`; compiled on its
//! own it is an empty test binary.
//!
//! Everything that depends on how the hooks are constructed and how the
//! magnetisation is read back is in the "ADAPTER" block, so the spec tests
//! themselves only use `register`, `register_magnus`, `register_remanence` and
//! `mag`.
#![allow(dead_code)]

use glam::{Quat, Vec3};
use pin_core::ids::MagnetId;
use pin_core::profiles::{BUILTIN_PROFILE_NAMES, builtin_profiles};
use pin_core::{BallProfile, PhysicsDefaults, ProfileId};
use pin_physics::ball::{
    Ball, BallEvents, BallHandle, BallSolver, BallSolverConfig, Geometry, Magnet, NewtonChain,
    Sensor, SensorKind,
};
use pin_physics::world::{
    CubicSpline, GuideCurve, StaticShape, StaticWorld, StaticWorldBuilder, Wireform, layers,
};

// ------------------------------------------------------------------ ADAPTER

/// What `mag` reads the per-ball magnetisation from.
pub type MagProbe = pin_physics::ball_ext::Remanence;

/// Registers both hooks through the fixed `register_all`.
pub fn register(s: &mut BallSolver) -> MagProbe {
    pin_physics::ball_ext::register_all(s)
}

/// Registers only the Magnus hook.
pub fn register_magnus(s: &mut BallSolver) {
    s.add_external_force(Box::new(pin_physics::ball_ext::Magnus::default()));
}

/// Registers only the remanence hook.
pub fn register_remanence(s: &mut BallSolver) -> MagProbe {
    let r = pin_physics::ball_ext::Remanence::default();
    s.add_external_force(Box::new(r.clone()));
    r
}

/// Magnetisation `M` of the ball in slot `h` (fixed API `magnetisation`).
pub fn mag(p: &MagProbe, h: BallHandle) -> f32 {
    pin_physics::ball_ext::magnetisation(p, h)
}

// ------------------------------------------------------------------ helpers

/// The engine tick.
pub const DT: f32 = 0.001;

/// The seven built-in profiles; `ProfileId(i)` is index `i`.
pub fn builtin() -> Vec<BallProfile> {
    builtin_profiles().collect()
}

/// The built-in profiles with `remanence = 0` and `magnus_coeff = 0`
/// (verification 1: the 2.2 baseline).
pub fn zeroed() -> Vec<BallProfile> {
    builtin_profiles()
        .map(|mut p| {
            p.remanence = 0.0;
            p.magnus_coeff = 0.0;
            p
        })
        .collect()
}

pub fn pid(name: &str) -> ProfileId {
    let i = BUILTIN_PROFILE_NAMES
        .iter()
        .position(|n| *n == name)
        .unwrap_or_else(|| panic!("unknown builtin profile {name}"));
    ProfileId(i as u16)
}

pub fn profile(name: &str) -> BallProfile {
    builtin()[pid(name).index()].clone()
}

pub fn config(gravity: Vec3) -> BallSolverConfig {
    let mut cfg = BallSolverConfig::from_defaults(&PhysicsDefaults::builtin());
    cfg.gravity = gravity;
    cfg
}

pub const G: Vec3 = Vec3::new(0.0, 0.0, -9.81);

pub fn solver(profiles: &[BallProfile], gravity: Vec3) -> BallSolver {
    BallSolver::new(config(gravity), profiles)
}

pub fn cuboid(center: Vec3, half: Vec3) -> StaticShape {
    StaticShape::Cuboid {
        center,
        half_extents: half,
        rotation: Quat::IDENTITY,
    }
}

pub fn empty_world() -> StaticWorld {
    StaticWorldBuilder::new().build()
}

/// A 20 m × 20 m slab whose top face is the plane z = 0.
pub fn floor_world() -> StaticWorld {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        cuboid(Vec3::new(0.0, 0.0, -0.05), Vec3::new(10.0, 10.0, 0.05)),
        layers::ALL,
        true,
    );
    b.build()
}

pub fn magnet_at(position: Vec3) -> Magnet {
    Magnet {
        position,
        k_coil: 2.0e-5,
        d_min: 0.01,
        range: 0.08,
        layer_mask: layers::ALL,
    }
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

/// Raw bits of every ball (handle order) and every captive ball.
pub fn solver_bits(s: &BallSolver) -> Vec<u32> {
    let mut out = Vec::new();
    for (h, b) in s.balls() {
        out.push(u32::from(h.0));
        out.extend_from_slice(&state_bits(b));
    }
    for i in 0..s.newton_chain_count() {
        let c = s.newton_chain(i);
        for k in 0..c.len() {
            out.push(c.ball_t(k).to_bits());
            out.push(c.ball_speed(k).to_bits());
        }
    }
    out
}

// ------------------------------------------------------- busy table scenario

/// A busy table: pitched playfield (6.5°) with walls, posts, a guide arc, a
/// wireform, a Newton chain, a magnet (driven by the test), two sensors and
/// four balls (standard_steel, ceramic_powerball, mini_steel, uv_globall) of
/// the given profile list (built-in order).
pub struct Busy {
    pub world: StaticWorld,
    pub guides: Vec<GuideCurve>,
    pub wireforms: Vec<Wireform>,
    pub solver: BallSolver,
    pub magnet: MagnetId,
    pub probe: Option<MagProbe>,
}

impl Busy {
    pub fn geo(&self) -> Geometry<'_> {
        Geometry {
            world: &self.world,
            guides: &self.guides,
            wireforms: &self.wireforms,
        }
    }
}

/// When the hooks are registered relative to the balls.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum Hooks {
    None,
    /// `register_all` before any ball is added.
    Before,
    /// `register_all` after the balls are added (`ball_added` replayed).
    After,
}

pub fn busy(profiles: &[BallProfile], hooks: Hooks) -> Busy {
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
    for y in [-0.61f32, 0.61] {
        b.add_collider(
            StaticShape::Cuboid {
                center: rot * Vec3::new(0.0, y, 0.03),
                half_extents: Vec3::new(0.3, 0.01, 0.04),
                rotation: rot,
            },
            layers::ALL,
            true,
        );
    }
    let n = rot * Vec3::Z;
    for (i, (x, y)) in [
        (-0.1f32, 0.2f32),
        (0.1, 0.2),
        (0.0, 0.0),
        (-0.15, -0.2),
        (0.15, -0.25),
    ]
    .into_iter()
    .enumerate()
    {
        b.add_collider(
            StaticShape::Cylinder {
                base: rot * Vec3::new(x, y, -0.005),
                axis: n * 0.05,
                radius: 0.012 + 0.002 * i as f32,
            },
            layers::ALL,
            true,
        );
    }
    let world = b.build();
    let guide_pts: Vec<Vec3> = (0..9)
        .map(|i| {
            let a = std::f32::consts::PI * i as f32 / 8.0;
            Vec3::new(0.25 * libm::cosf(a), 0.4 + 0.15 * libm::sinf(a), 0.08)
        })
        .collect();
    let guides =
        vec![GuideCurve::new(CubicSpline::interpolate(&guide_pts).unwrap(), 0.05).unwrap()];
    let wf_pts: Vec<Vec3> = (0..6)
        .map(|i| Vec3::new(-0.25 + 0.1 * i as f32, 0.55, 0.3 - 0.03 * i as f32))
        .collect();
    let wireforms = vec![
        Wireform::new(
            CubicSpline::interpolate(&wf_pts).unwrap(),
            &[0.0; 6],
            0.02,
            Vec3::Z,
        )
        .unwrap(),
    ];
    let mut s = solver(profiles, G);
    let mut probe = None;
    if hooks == Hooks::Before {
        probe = Some(register(&mut s));
    }
    s.set_collider_material(pf, pin_core::Material::Wood);
    // Low on the table, where the balls collect.
    let magnet = s.add_magnet(Magnet {
        position: rot * Vec3::new(0.0, -0.5, -0.005),
        k_coil: 2.0e-5,
        d_min: 0.01,
        range: 0.12,
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
        (
            "standard_steel",
            Vec3::new(-0.2, 0.45, 0.0),
            Vec3::new(1.2, -0.5, 0.0),
        ),
        (
            "ceramic_powerball",
            Vec3::new(0.05, 0.5, 0.0),
            Vec3::new(-0.8, -1.5, 0.0),
        ),
        (
            "mini_steel",
            Vec3::new(0.2, 0.3, 0.0),
            Vec3::new(-1.5, 0.3, 0.0),
        ),
        (
            "uv_globall",
            Vec3::new(-0.05, 0.1, 0.0),
            Vec3::new(0.4, 2.0, 0.0),
        ),
    ];
    for (name, p, v) in starts {
        let r = profiles[pid(name).index()].radius_m();
        let mut bl = Ball::new(pid(name), rot * (p + Vec3::new(0.0, 0.0, r + 0.001)));
        bl.velocity = rot * v;
        bl.angular_velocity = Vec3::new(3.0, -7.0, 11.0);
        s.add_ball(bl).unwrap();
    }
    if hooks == Hooks::After {
        probe = Some(register(&mut s));
    }
    Busy {
        world,
        guides,
        wireforms,
        solver: s,
        magnet,
        probe,
    }
}

/// Power command of the busy scenario's magnet at `tick`: on, off, half.
pub fn busy_power(tick: u32) -> f32 {
    match (tick / 300) % 3 {
        0 => 1.0,
        1 => 0.0,
        _ => 0.5,
    }
}

// ---------------------------------------------------- holding-magnet scenario

/// Pitch of the holding-magnet table (gravity is tilted, the floor is z = 0).
pub const HOLD_PITCH_DEG: f32 = 6.5;
/// Height of the magnet pole below the floor.
pub const HOLD_POLE_DEPTH: f32 = 0.005;
/// Horizontal distance from the magnet axis beyond which the ball "has left".
pub const LEFT_DISTANCE: f32 = 0.04;

/// One tick of a holding-magnet run.
#[derive(Copy, Clone, Debug)]
pub struct HoldSample {
    pub bits: [u32; 14],
    pub position: Vec3,
    pub velocity: Vec3,
    /// Magnetisation after the tick (0 when no hook is registered).
    pub m: f32,
}

#[derive(Clone, Debug)]
pub struct HoldRun {
    /// Samples of the hold phase (coil on).
    pub hold: Vec<HoldSample>,
    /// Samples after the coil was switched off.
    pub after: Vec<HoldSample>,
}

impl HoldRun {
    /// First tick after switch-off at which the ball is more than
    /// [`LEFT_DISTANCE`] from the magnet axis.
    pub fn leave_tick(&self) -> Option<usize> {
        self.after
            .iter()
            .position(|s| Vec3::new(s.position.x, s.position.y, 0.0).length() > LEFT_DISTANCE)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct HoldSpec {
    /// Remanence τ of the magnet (`None`: the solver default, 60 ms).
    pub tau_s: Option<f32>,
    /// Register the 2.7 hooks.
    pub hooks: bool,
    pub hold_ticks: usize,
    pub after_ticks: usize,
    /// Put the ball in one canonical state (at rest, 0.5 mm downhill of the
    /// magnet axis) at the instant the coil switches off, so that runs with
    /// different remanence start the release from the same state.
    pub canonical_release: bool,
    pub power: f32,
}

impl Default for HoldSpec {
    fn default() -> Self {
        Self {
            tau_s: None,
            hooks: true,
            hold_ticks: 600,
            after_ticks: 1500,
            canonical_release: true,
            power: 1.0,
        }
    }
}

/// A ball of `profile` held on a pitched table over a magnet under the
/// playfield; the coil is switched off after `hold_ticks` and the ball rolls
/// away downhill (−y).
pub fn hold_run(profile: &BallProfile, spec: HoldSpec) -> HoldRun {
    let world = floor_world();
    let geo = Geometry::new(&world);
    let pitch = HOLD_PITCH_DEG.to_radians();
    let gravity = Vec3::new(0.0, -9.81 * libm::sinf(pitch), -9.81 * libm::cosf(pitch));
    let profiles = [profile.clone()];
    let mut s = solver(&profiles, gravity);
    let id = s.add_magnet(magnet_at(Vec3::new(0.0, 0.0, -HOLD_POLE_DEPTH)));
    if let Some(tau) = spec.tau_s {
        s.set_magnet_remanence_tau(id, tau);
    }
    let probe = spec.hooks.then(|| register(&mut s));
    let r = profile.radius_m();
    let start = Ball::new(ProfileId(0), Vec3::new(0.0, -0.0005, r));
    let h = s.add_ball(start).unwrap();
    let mut ev = BallEvents::new();
    let sample = |s: &BallSolver| {
        let b = s.ball(h).unwrap();
        HoldSample {
            bits: state_bits(b),
            position: b.position,
            velocity: b.velocity,
            m: probe.as_ref().map_or(0.0, |p| mag(p, h)),
        }
    };
    s.set_magnet_power(id, spec.power);
    let mut hold = Vec::with_capacity(spec.hold_ticks);
    for _ in 0..spec.hold_ticks {
        ev.clear();
        s.step(&geo, DT, &mut ev);
        hold.push(sample(&s));
    }
    s.set_magnet_power(id, 0.0);
    if spec.canonical_release {
        *s.ball_mut(h).unwrap() = start;
    }
    let mut after = Vec::with_capacity(spec.after_ticks);
    for _ in 0..spec.after_ticks {
        ev.clear();
        s.step(&geo, DT, &mut ev);
        after.push(sample(&s));
    }
    HoldRun { hold, after }
}

/// `standard_steel` with another remanence (and optionally coupling).
pub fn steel_with(remanence: f32, coupling: f32) -> BallProfile {
    let mut p = profile("standard_steel");
    p.remanence = remanence;
    p.magnetic_coupling = coupling;
    p
}

/// `uv_globall` with another Magnus coefficient.
pub fn globall_with(magnus: f32) -> BallProfile {
    let mut p = profile("uv_globall");
    p.magnus_coeff = magnus;
    p
}

// --------------------------------------------------- pinned-ball magnetiser

/// A solver with no gravity, no geometry, one weak magnet at the origin
/// (`K_coil` so small that its pull does not move the ball measurably, range
/// 0.08 m) and the hooks registered. A ball at [`NEAR`] is in range, a ball at
/// [`FAR`] is not.
pub struct Bench {
    pub world: StaticWorld,
    pub solver: BallSolver,
    pub magnet: MagnetId,
    pub probe: MagProbe,
}

pub const NEAR: Vec3 = Vec3::new(0.03, 0.0, 0.0);
pub const FAR: Vec3 = Vec3::new(5.0, 0.0, 0.0);

pub fn bench(profiles: &[BallProfile], k_coil: f32) -> Bench {
    let mut s = solver(profiles, Vec3::ZERO);
    let mut m = magnet_at(Vec3::ZERO);
    m.k_coil = k_coil;
    let magnet = s.add_magnet(m);
    let probe = register(&mut s);
    Bench {
        world: empty_world(),
        solver: s,
        magnet,
        probe,
    }
}

impl Bench {
    /// Steps `ticks` ticks holding ball `h` at `at` (state reset before every
    /// tick: at rest at `at`).
    pub fn pinned(&mut self, h: BallHandle, at: Vec3, ticks: usize) {
        let mut ev = BallEvents::new();
        let geo = Geometry::new(&self.world);
        for _ in 0..ticks {
            let b = self.solver.ball_mut(h).unwrap();
            b.position = at;
            b.velocity = Vec3::ZERO;
            b.angular_velocity = Vec3::ZERO;
            ev.clear();
            self.solver.step(&geo, DT, &mut ev);
        }
    }

    pub fn m(&self, h: BallHandle) -> f32 {
        mag(&self.probe, h)
    }
}
