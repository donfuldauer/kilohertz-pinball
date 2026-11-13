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

    pub fn omega_at(&self, t: f32) -> f32 {
        let a = self.unclamped(t);
        if a < self.limits.0 || a > self.limits.1 {
            0.0
        } else {
            self.seg_omega
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
    fn inverse_mass_at(&self, body: usize, _time: f32, point: Vec3, dir: Vec3) -> f32 {
        let b = &self.bodies[body];
        let arm = (point - b.pivot).cross(dir).z;
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
