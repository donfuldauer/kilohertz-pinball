//! Reference moving bodies for the seam tests (`ball_seam_*.rs`, Subphase
//! 2.2-S), included with `#[path]`; an empty test crate on its own.
//!
//! * [`Slab`]: kinematic (infinite-mass) axis-aligned boxes translating at
//!   constant velocity.
//! * [`Rotor`]: 1-DOF capsules rotating about a pivot (axis +z), with finite
//!   inertia, a constant drive torque and optional inelastic end stops.
//!
//! Both follow the `MovingBodies` contract literally: the trajectory over a
//! tick is predicted at `begin_tick` (constant velocity per segment), an
//! impulse starts a new segment at its time, `end_tick` commits.
#![allow(dead_code)]

use arrayvec::ArrayVec;
use glam::Vec3;
use pin_core::Material;
use pin_physics::ball::{BodyPoint, MovingBodies};
use pin_physics::world::layers;

pub const MAX_REF_BODIES: usize = 8;

/// An axis-aligned box translating at constant velocity (kinematic).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SlabBody {
    /// Centre at the start of the tick.
    pub center: Vec3,
    pub half: Vec3,
    pub velocity: Vec3,
    pub layer_mask: u8,
    pub material: Material,
    /// Sum of the impulses received (N·s), for tests.
    pub received: Vec3,
    pub impulses: u32,
}

impl SlabBody {
    pub fn new(center: Vec3, half: Vec3, velocity: Vec3) -> Self {
        Self {
            center,
            half,
            velocity,
            layer_mask: layers::ALL,
            material: Material::Rubber,
            received: Vec3::ZERO,
            impulses: 0,
        }
    }

    pub fn center_at(&self, t: f32) -> Vec3 {
        self.center + self.velocity * t
    }

    /// Signed distance of `p` from the box surface at `t`, with the closest
    /// surface point and outward normal.
    pub fn closest(&self, t: f32, p: Vec3) -> BodyPoint {
        let c = self.center_at(t);
        let local = p - c;
        let h = self.half;
        let clamped = local.clamp(-h, h);
        if clamped != local {
            let d = local - clamped;
            let dist = d.length();
            return BodyPoint {
                point: c + clamped,
                normal: d / dist,
                distance: dist,
            };
        }
        // Inside: the nearest face.
        let mut best = (f32::INFINITY, Vec3::ZERO);
        for axis in 0..3 {
            for sign in [-1.0f32, 1.0] {
                let depth = h[axis] - sign * local[axis];
                if depth < best.0 {
                    let mut n = Vec3::ZERO;
                    n[axis] = sign;
                    best = (depth, n);
                }
            }
        }
        let (depth, n) = best;
        BodyPoint {
            point: p + n * depth,
            normal: n,
            distance: -depth,
        }
    }
}

/// A set of [`SlabBody`]s.
#[derive(Clone, Debug, Default)]
pub struct Slab {
    pub bodies: ArrayVec<SlabBody, MAX_REF_BODIES>,
    pub dt: f32,
    pub ticks: u64,
}

impl Slab {
    pub fn one(b: SlabBody) -> Self {
        let mut s = Self::default();
        s.bodies.push(b);
        s
    }
}

impl MovingBodies for Slab {
    fn body_count(&self) -> usize {
        self.bodies.len()
    }
    fn begin_tick(&mut self, dt: f32) {
        self.dt = dt;
    }
    fn end_tick(&mut self, dt: f32) {
        for b in &mut self.bodies {
            b.center = b.center_at(dt);
        }
        self.ticks += 1;
    }
    fn layer_mask(&self, body: usize) -> u8 {
        self.bodies[body].layer_mask
    }
    fn material(&self, body: usize) -> Material {
        self.bodies[body].material
    }
    fn swept_bounds(&self, body: usize) -> (Vec3, Vec3) {
        let b = &self.bodies[body];
        let (a, c) = (b.center_at(0.0), b.center_at(self.dt));
        (a.min(c) - b.half, a.max(c) + b.half)
    }
    fn max_surface_speed(&self, body: usize) -> f32 {
        self.bodies[body].velocity.length()
    }
    fn closest_point(&self, body: usize, time: f32, point: Vec3) -> Option<BodyPoint> {
        Some(self.bodies[body].closest(time, point))
    }
    fn velocity_at(&self, body: usize, _time: f32, _point: Vec3) -> Vec3 {
        self.bodies[body].velocity
    }
    fn inverse_mass_at(&self, _body: usize, _time: f32, _point: Vec3, _dir: Vec3) -> f32 {
        0.0
    }
    fn apply_impulse(&mut self, body: usize, _time: f32, _point: Vec3, impulse: Vec3) {
        let b = &mut self.bodies[body];
        b.received += impulse;
        b.impulses += 1;
    }
}

/// A capsule of constant radius from the pivot along angle θ in the xy
/// plane (axis +z through `pivot`), with finite inertia.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct RotorBody {
    pub pivot: Vec3,
    pub length: f32,
    pub radius: f32,
    /// Moment of inertia about the pivot, kg·m² (`f32::INFINITY` = kinematic).
    pub inertia: f32,
    /// Committed angle and angular velocity (start of tick).
    pub angle: f32,
    pub omega: f32,
    /// Constant drive torque, N·m (about +z).
    pub torque: f32,
    /// Inelastic stops (`lo ≤ θ ≤ hi`).
    pub limits: (f32, f32),
    pub layer_mask: u8,
    pub material: Material,
    /// Current segment: start time, angle and angular velocity.
    seg_t: f32,
    seg_angle: f32,
    seg_omega: f32,
    /// Sum of the angular impulses received about the pivot, N·m·s.
    pub angular_impulse: f32,
}

impl RotorBody {
    pub fn new(pivot: Vec3, length: f32, radius: f32, inertia: f32, angle: f32) -> Self {
        Self {
            pivot,
            length,
            radius,
            inertia,
            angle,
            omega: 0.0,
            torque: 0.0,
            limits: (f32::NEG_INFINITY, f32::INFINITY),
            layer_mask: layers::ALL,
            material: Material::Rubber,
            seg_t: 0.0,
            seg_angle: angle,
            seg_omega: 0.0,
            angular_impulse: 0.0,
        }
    }

    fn unclamped(&self, t: f32) -> f32 {
        self.seg_angle + self.seg_omega * (t - self.seg_t)
    }

    pub fn angle_at(&self, t: f32) -> f32 {
        self.unclamped(t).clamp(self.limits.0, self.limits.1)
    }

    /// Angular velocity at `t`: 0 on an engaged stop (at or past it and
    /// moving into it), including the very instant the segment starts.
    pub fn omega_at(&self, t: f32) -> f32 {
        let a = self.unclamped(t);
        let w = self.seg_omega;
        if (a <= self.limits.0 && w < 0.0) || (a >= self.limits.1 && w > 0.0) {
            0.0
        } else {
            w
        }
    }

    pub fn direction(angle: f32) -> Vec3 {
        Vec3::new(libm::cosf(angle), libm::sinf(angle), 0.0)
    }

    pub fn reach(&self) -> f32 {
        self.length + self.radius
    }

    pub fn closest(&self, t: f32, p: Vec3) -> BodyPoint {
        let dir = Self::direction(self.angle_at(t));
        let s = (p - self.pivot).dot(dir).clamp(0.0, self.length);
        let q = self.pivot + dir * s;
        let d = p - q;
        let dist = d.length();
        let normal = if dist > 1.0e-9 {
            d / dist
        } else {
            Vec3::Z.cross(dir)
        };
        BodyPoint {
            point: q + normal * self.radius,
            normal,
            distance: dist - self.radius,
        }
    }

    /// Kinetic energy of the rotor at the start of the tick.
    pub fn energy(&self) -> f64 {
        0.5 * f64::from(self.inertia) * f64::from(self.omega) * f64::from(self.omega)
    }
}

/// A set of [`RotorBody`]s.
#[derive(Clone, Debug, Default)]
pub struct Rotors {
    pub bodies: ArrayVec<RotorBody, MAX_REF_BODIES>,
    pub dt: f32,
}

impl Rotors {
    pub fn one(b: RotorBody) -> Self {
        let mut s = Self::default();
        s.bodies.push(b);
        s
    }
}

impl MovingBodies for Rotors {
    fn body_count(&self) -> usize {
        self.bodies.len()
    }
    fn begin_tick(&mut self, dt: f32) {
        self.dt = dt;
        for b in &mut self.bodies {
            // Semi-implicit Euler: the drive changes the velocity first.
            if b.inertia.is_finite() {
                b.omega += b.torque / b.inertia * dt;
            }
            // At a stop and pushed into it: held.
            if (b.angle >= b.limits.1 && b.omega > 0.0) || (b.angle <= b.limits.0 && b.omega < 0.0)
            {
                b.omega = 0.0;
            }
            b.seg_t = 0.0;
            b.seg_angle = b.angle;
            b.seg_omega = b.omega;
        }
    }
    fn end_tick(&mut self, dt: f32) {
        for b in &mut self.bodies {
            b.angle = b.angle_at(dt);
            b.omega = b.omega_at(dt);
            // Queries between ticks (tests) see the committed state.
            b.seg_t = 0.0;
            b.seg_angle = b.angle;
            b.seg_omega = b.omega;
        }
    }
    fn layer_mask(&self, body: usize) -> u8 {
        self.bodies[body].layer_mask
    }
    fn material(&self, body: usize) -> Material {
        self.bodies[body].material
    }
    fn swept_bounds(&self, body: usize) -> (Vec3, Vec3) {
        let b = &self.bodies[body];
        let r = b.reach();
        (
            b.pivot - Vec3::new(r, r, b.radius),
            b.pivot + Vec3::new(r, r, b.radius),
        )
    }
    fn max_surface_speed(&self, body: usize) -> f32 {
        let b = &self.bodies[body];
        b.seg_omega.abs() * b.reach()
    }
    fn closest_point(&self, body: usize, time: f32, point: Vec3) -> Option<BodyPoint> {
        Some(self.bodies[body].closest(time, point))
    }
    fn velocity_at(&self, body: usize, time: f32, point: Vec3) -> Vec3 {
        let b = &self.bodies[body];
        Vec3::Z.cross(point - b.pivot) * b.omega_at(time)
    }
    fn inverse_mass_at(&self, body: usize, time: f32, point: Vec3, dir: Vec3) -> f32 {
        let b = &self.bodies[body];
        let arm = (point - b.pivot).cross(dir).z;
        // An engaged stop takes a push into it.
        let angle = b.angle_at(time);
        if (angle <= b.limits.0 && arm < 0.0) || (angle >= b.limits.1 && arm > 0.0) {
            return 0.0;
        }
        arm * arm / b.inertia
    }
    fn apply_impulse(&mut self, body: usize, time: f32, point: Vec3, impulse: Vec3) {
        let b = &mut self.bodies[body];
        let l = (point - b.pivot).cross(impulse).z;
        b.angular_impulse += l;
        if !b.inertia.is_finite() {
            return;
        }
        let angle = b.angle_at(time);
        let omega = b.omega_at(time) + l / b.inertia;
        b.seg_t = time;
        b.seg_angle = angle;
        b.seg_omega = omega;
    }
}

/// A small flipper table: pitched floor in a walled box, two torque-driven
/// rotors at the bottom (flippers with stops), a kinematic kicker slab on
/// the left wall, four balls of different profiles.
pub struct FlipperTable {
    pub world: pin_physics::world::StaticWorld,
    pub solver: pin_physics::ball::BallSolver,
    pub rotors: Rotors,
    pub kicker: Slab,
}

pub const FLIP_TORQUE: f32 = 1.5;
pub const REST_TORQUE: f32 = 0.05;

impl FlipperTable {
    pub fn new(balls: usize) -> Self {
        use pin_core::PhysicsDefaults;
        use pin_core::ids::ProfileId;
        use pin_physics::ball::{Ball, BallSolver, BallSolverConfig};
        use pin_physics::world::{StaticShape, StaticWorldBuilder};

        let pitch = 6.5f32.to_radians();
        let mut cfg = BallSolverConfig::from_defaults(&PhysicsDefaults::builtin());
        cfg.gravity = Vec3::new(0.0, -9.81 * libm::sinf(pitch), -9.81 * libm::cosf(pitch));
        let profiles: Vec<pin_core::BallProfile> =
            pin_core::profiles::builtin_profiles().collect();
        let mut solver = BallSolver::new(cfg, &profiles);

        let cuboid = |c: Vec3, h: Vec3| StaticShape::Cuboid {
            center: c,
            half_extents: h,
            rotation: glam::Quat::IDENTITY,
        };
        let mut b = StaticWorldBuilder::new();
        b.add_collider(
            cuboid(Vec3::new(0.0, 0.0, -0.05), Vec3::new(0.4, 0.6, 0.05)),
            layers::ALL,
            true,
        );
        for (c, h) in [
            (Vec3::new(0.26, 0.05, 0.05), Vec3::new(0.01, 0.45, 0.05)),
            (Vec3::new(-0.26, 0.05, 0.05), Vec3::new(0.01, 0.45, 0.05)),
            (Vec3::new(0.0, 0.51, 0.05), Vec3::new(0.27, 0.01, 0.05)),
            (Vec3::new(0.0, -0.41, 0.05), Vec3::new(0.27, 0.01, 0.05)),
            // Inlane walls guiding balls onto the flippers.
            (Vec3::new(-0.19, -0.15, 0.05), Vec3::new(0.07, 0.008, 0.05)),
            (Vec3::new(0.19, -0.15, 0.05), Vec3::new(0.07, 0.008, 0.05)),
        ] {
            b.add_collider(cuboid(c, h), layers::ALL, true);
        }
        let world = b.build();

        let z = 0.0135;
        let mut left = RotorBody::new(Vec3::new(-0.115, -0.16, z), 0.075, 0.009, 2.5e-4, -0.5);
        left.limits = (-0.5, 0.5);
        left.torque = -REST_TORQUE;
        let mut right = RotorBody::new(
            Vec3::new(0.115, -0.16, z),
            0.075,
            0.009,
            2.5e-4,
            std::f32::consts::PI + 0.5,
        );
        right.limits = (std::f32::consts::PI - 0.5, std::f32::consts::PI + 0.5);
        right.torque = REST_TORQUE;
        let mut rotors = Rotors::default();
        rotors.bodies.push(left);
        rotors.bodies.push(right);

        let mut kicker = SlabBody::new(
            Vec3::new(-0.24, 0.1, 0.02),
            Vec3::new(0.01, 0.05, 0.02),
            Vec3::ZERO,
        );
        kicker.material = Material::Rubber;
        let kicker = Slab::one(kicker);

        let starts = [
            (0usize, Vec3::new(-0.05, 0.3, 0.0), Vec3::new(-0.6, -0.4, 0.0)),
            (2, Vec3::new(0.1, 0.4, 0.0), Vec3::new(0.3, -1.0, 0.0)),
            (3, Vec3::new(-0.15, 0.0, 0.0), Vec3::new(0.2, 0.5, 0.0)),
            (5, Vec3::new(0.15, 0.1, 0.0), Vec3::new(-0.8, 0.0, 0.0)),
            (4, Vec3::new(0.0, 0.2, 0.0), Vec3::new(0.0, 0.0, 0.0)),
            (0, Vec3::new(0.05, 0.45, 0.0), Vec3::new(0.5, 0.0, 0.0)),
        ];
        for &(p, pos, vel) in starts.iter().take(balls) {
            let r = profiles[p].radius_m();
            let mut ball = Ball::new(ProfileId(p as u16), pos + Vec3::new(0.0, 0.0, r + 0.001));
            ball.velocity = vel;
            solver.add_ball(ball).unwrap();
        }
        Self {
            world,
            solver,
            rotors,
            kicker,
        }
    }

    /// Drive for `tick`: flipper buttons and kicker motion (the owner's
    /// business, set between steps).
    pub fn drive(&mut self, tick: u32) {
        let left = (tick / 300) % 2 == 1;
        let right = (tick / 250) % 3 == 1;
        self.rotors.bodies[0].torque = if left { FLIP_TORQUE } else { -REST_TORQUE };
        self.rotors.bodies[1].torque = if right { -FLIP_TORQUE } else { REST_TORQUE };
        let k = &mut self.kicker.bodies[0];
        k.velocity = match tick % 400 {
            0..=9 => Vec3::new(1.5, 0.0, 0.0),
            10..=59 => Vec3::new(-0.3, 0.0, 0.0),
            _ => Vec3::ZERO,
        };
    }

    pub fn step(&mut self, events: &mut pin_physics::ball::BallEvents) {
        let geo = pin_physics::ball::Geometry::new(&self.world);
        self.solver.step_with_bodies(
            &geo,
            &mut [&mut self.rotors, &mut self.kicker],
            1.0e-3,
            events,
        );
    }

    /// Bit pattern of the balls and bodies.
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
                .chain(b.orientation.to_array())
            {
                out.push(x.to_bits());
            }
        }
        for r in &self.rotors.bodies {
            out.push(r.angle.to_bits());
            out.push(r.omega.to_bits());
            out.push(r.angular_impulse.to_bits());
        }
        for s in &self.kicker.bodies {
            out.extend(s.center.to_array().map(f32::to_bits));
            out.extend(s.received.to_array().map(f32::to_bits));
        }
        out
    }
}
