//! A 1-DOF finite-mass slider with inelastic end stops: the moving part of
//! slingshots, kickbacks and the plunger.
//!
//! Coordinate `x` along the unit axis `u` (metres), `lo ≤ x ≤ hi`. Forces:
//! spring `−k·x`, coil `+F` while the coil is on. Plunger only: a one-sided
//! hand constraint `x ≤ hand` (the hand can hold the rod back, never push it).
//!
//! Prediction per tick (the 2.2-S contract): semi-implicit Euler
//! `v₁ = v₀ + a·dt`, then owner obligation 3 on the drive: toward a stop,
//! `v₁ ← min(v₁, max((stop − x₀)/dt, v₀))` (the drive increment never
//! carries the slider further than the stop allows this tick; momentum it
//! already had is kept, so a rod arriving at full speed hits a ball just
//! before its stop at that speed), then `x(t) = clamp(x₀ + v₁·t)` with the
//! velocity 0 from the instant the stop is reached (obligation 2). A slider
//! resting on (or pushed slightly off) a stop has `v₀ ≈ 0`, so it reports
//! no phantom surface speed. After an impulse the plain limit
//! `(stop − x)/(dt − t)` applies. When the hand
//! holds the rod back the tick is kinematic (`x` follows the hand linearly,
//! inverse mass 0). A slider on its rest stop with nothing driving it away
//! is *locked* (held by its preloaded spring: kinematic in every direction),
//! so a ball resting on a kicker or the plunger tip sees static geometry.

use glam::Vec3;

use super::geom::OBox;
use crate::ball::BodyPoint;

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct Slider {
    /// Body box at `x = 0`.
    pub rest_box: OBox,
    /// Unit axis.
    pub axis: Vec3,
    pub lo: f32,
    pub hi: f32,
    /// Rest position (a stop).
    pub rest: f32,
    pub mass: f32,
    pub spring_k: f32,
    pub force: f32,
    /// Committed position and velocity (start of the tick).
    pub x: f32,
    pub v: f32,
    /// Drive for this tick.
    pub coil_on: bool,
    /// Hand limit at the end of this tick (`x ≤ hand`); `hi` = no hand.
    pub hand: f32,
    // Current segment.
    seg_t: f32,
    seg_x: f32,
    seg_v: f32,
    kinematic: bool,
    locked: bool,
    dt: f32,
}

impl Slider {
    pub fn new(rest_box: OBox, axis: Vec3, (lo, hi): (f32, f32), rest: f32) -> Self {
        Self {
            rest_box,
            axis,
            lo,
            hi,
            rest,
            mass: 1.0,
            spring_k: 0.0,
            force: 0.0,
            x: rest,
            v: 0.0,
            coil_on: false,
            hand: hi,
            seg_t: 0.0,
            seg_x: rest,
            seg_v: 0.0,
            kinematic: false,
            locked: true,
            dt: 0.0,
        }
    }

    /// `v` limited so that `remaining` seconds of travel from `x` end at a
    /// stop at the latest (owner obligation 3).
    fn stop_clamped(&self, x: f32, v: f32, remaining: f32) -> f32 {
        if remaining.is_nan() || remaining <= 0.0 {
            return if (x >= self.hi && v > 0.0) || (x <= self.lo && v < 0.0) {
                0.0
            } else {
                v
            };
        }
        if v > 0.0 {
            v.min((self.hi - x).max(0.0) / remaining)
        } else if v < 0.0 {
            v.max((self.lo - x).min(0.0) / remaining)
        } else {
            v
        }
    }

    pub fn begin_tick(&mut self, dt: f32) {
        self.dt = dt;
        let mut a = -self.spring_k * self.x;
        if self.coil_on {
            a += self.force;
        }
        let mut v1 = self.v + a / self.mass * dt;
        let free_end = self.x + v1 * dt;
        self.kinematic = false;
        if self.hand < self.hi && self.hand < free_end {
            // The hand holds the rod back: follow it.
            v1 = (self.hand.clamp(self.lo, self.hi) - self.x) / dt;
            self.kinematic = true;
        } else {
            // Obligation 3 on the drive: the increment `a·dt` toward a stop
            // is limited to what the stop allows this tick, but the momentum
            // the slider already had is real and kept (it reaches the stop
            // within the tick and stops there: `x_at` clamps, `v_at` is 0).
            let toward = if v1 > 0.0 {
                self.v.max(0.0)
            } else {
                self.v.min(0.0)
            };
            let limited = self.stop_clamped(self.x, v1, dt);
            v1 = if v1 > 0.0 {
                limited.max(toward.min(v1))
            } else {
                limited.min(toward.max(v1))
            };
        }
        if !v1.is_finite() {
            v1 = 0.0;
        }
        self.locked = !self.kinematic && self.x == self.rest && v1 == 0.0;
        self.seg_t = 0.0;
        self.seg_x = self.x;
        self.seg_v = v1;
    }

    pub fn end_tick(&mut self, dt: f32) {
        self.x = self.x_at(dt);
        self.v = self.v_at(dt);
        self.seg_t = 0.0;
        self.seg_x = self.x;
        self.seg_v = self.v;
        self.kinematic = false;
    }

    pub fn x_at(&self, t: f32) -> f32 {
        (self.seg_x + self.seg_v * (t - self.seg_t)).clamp(self.lo, self.hi)
    }

    /// Velocity at `t`: 0 on an engaged stop.
    pub fn v_at(&self, t: f32) -> f32 {
        let x = self.x_at(t);
        let w = self.seg_v;
        if (x >= self.hi && w > 0.0) || (x <= self.lo && w < 0.0) {
            0.0
        } else {
            w
        }
    }

    pub fn body_box(&self, t: f32) -> OBox {
        let mut b = self.rest_box;
        b.center += self.axis * self.x_at(t);
        b
    }

    pub fn closest(&self, t: f32, p: Vec3) -> BodyPoint {
        self.body_box(t).closest(p)
    }

    pub fn velocity(&self, t: f32) -> Vec3 {
        self.axis * self.v_at(t)
    }

    pub fn swept_bounds(&self) -> (Vec3, Vec3) {
        let mut a = self.rest_box;
        let mut b = self.rest_box;
        a.center += self.axis * self.lo;
        b.center += self.axis * self.hi;
        let (a0, a1) = a.aabb();
        let (b0, b1) = b.aabb();
        (a0.min(b0), a1.max(b1))
    }

    pub fn max_surface_speed(&self) -> f32 {
        self.seg_v.abs()
    }

    pub fn inverse_mass(&self, t: f32, direction: Vec3) -> f32 {
        if self.kinematic || self.locked {
            return 0.0;
        }
        let s = direction.dot(self.axis);
        let x = self.x_at(t);
        if (x >= self.hi && s > 0.0) || (x <= self.lo && s < 0.0) {
            return 0.0;
        }
        s * s / self.mass
    }

    pub fn apply_impulse(&mut self, t: f32, impulse: Vec3) {
        if self.kinematic || self.locked {
            return;
        }
        let x = self.x_at(t);
        let v = self.v_at(t) + impulse.dot(self.axis) / self.mass;
        let v = self.stop_clamped(x, v, self.dt - t);
        self.seg_t = t;
        self.seg_x = x;
        self.seg_v = if v.is_finite() { v } else { 0.0 };
    }

    /// Whether the segment is kinematic (tests).
    #[cfg(test)]
    pub fn is_locked(&self) -> bool {
        self.locked
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kicker() -> Slider {
        let b = OBox {
            center: Vec3::ZERO,
            axes: [Vec3::Y, -Vec3::X, Vec3::Z],
            half: Vec3::new(0.005, 0.05, 0.01),
        };
        let mut s = Slider::new(b, Vec3::Y, (0.0, 0.01), 0.0);
        s.mass = 0.06;
        s.force = 40.0;
        s.spring_k = 200.0;
        s
    }

    #[test]
    fn coil_drives_to_the_stop_and_spring_returns() {
        let mut s = kicker();
        s.begin_tick(1e-3);
        assert!(s.is_locked());
        assert_eq!(s.inverse_mass(0.0, -Vec3::Y), 0.0);
        s.end_tick(1e-3);
        s.coil_on = true;
        let mut t = 0;
        while s.x < s.hi && t < 100 {
            let v0 = s.v;
            s.begin_tick(1e-3);
            // Obligation 3 on the drive: beyond the momentum it had, the
            // slider is never predicted further than the stop.
            assert!(s.seg_v <= v0.max((s.hi - s.x) / 1e-3) + 1e-6);
            assert!(s.x_at(1e-3) <= s.hi);
            s.end_tick(1e-3);
            t += 1;
        }
        assert_eq!(s.x, s.hi);
        assert_eq!(s.v, 0.0);
        // Engaged on the stop: no push into it, free away from it.
        s.begin_tick(1e-3);
        assert_eq!(s.inverse_mass(0.0, Vec3::Y), 0.0);
        assert!((s.inverse_mass(0.0, -Vec3::Y) - 1.0 / 0.06).abs() < 1e-3);
        assert_eq!(s.velocity(0.0), Vec3::ZERO);
        s.end_tick(1e-3);
        s.coil_on = false;
        for _ in 0..200 {
            s.begin_tick(1e-3);
            s.end_tick(1e-3);
        }
        assert_eq!(s.x, 0.0);
        assert_eq!(s.v, 0.0);
    }

    #[test]
    fn impulse_repredicts_and_respects_stops() {
        let mut s = kicker();
        s.coil_on = true;
        s.begin_tick(1e-3);
        let v0 = s.v_at(0.0);
        assert!(v0 > 0.0);
        s.apply_impulse(5e-4, Vec3::Y * -0.01);
        assert!(s.v_at(5e-4) < v0);
        // Kicked hard backward into the rest stop: velocity 0 at once.
        s.apply_impulse(5e-4, Vec3::Y * -10.0);
        let x = s.x_at(5e-4);
        assert!(x >= 0.0);
        s.apply_impulse(1e-3, Vec3::Y * -10.0);
        assert_eq!(s.v_at(1e-3), 0.0);
        s.end_tick(1e-3);
        assert_eq!(s.v, 0.0);
        // stop_clamped with no time left.
        assert_eq!(s.stop_clamped(s.lo, -1.0, 0.0), 0.0);
        assert_eq!(s.stop_clamped(0.005, 1.0, 0.0), 1.0);
    }

    #[test]
    fn hand_holds_back_kinematically() {
        let b = OBox {
            center: Vec3::ZERO,
            axes: [Vec3::Y, -Vec3::X, Vec3::Z],
            half: Vec3::new(0.006, 0.012, 0.012),
        };
        let mut s = Slider::new(b, Vec3::Y, (-0.05, 0.0), 0.0);
        s.mass = 0.05;
        s.spring_k = 1000.0;
        s.hand = -0.03;
        s.begin_tick(1e-3);
        assert_eq!(s.inverse_mass(0.0, Vec3::Y), 0.0);
        assert!(s.max_surface_speed() > 29.0);
        s.apply_impulse(5e-4, Vec3::Y);
        s.end_tick(1e-3);
        assert!((s.x + 0.03).abs() < 1e-6);
        // The committed velocity is the real (hand) motion.
        assert!((s.v + 30.0).abs() < 1e-3);
        // Held still for a tick: at rest.
        s.begin_tick(1e-3);
        s.end_tick(1e-3);
        assert_eq!(s.v, 0.0);
        // Released: the spring drives it forward to the rest stop.
        s.hand = s.hi;
        let mut peak = 0.0f32;
        for _ in 0..50 {
            s.begin_tick(1e-3);
            peak = peak.max(s.v_at(0.0));
            s.end_tick(1e-3);
        }
        assert_eq!(s.x, 0.0);
        let analytic = 0.03 * libm::sqrtf(1000.0 / 0.05);
        assert!(
            (peak - analytic).abs() / analytic < 0.1,
            "{peak} vs {analytic}"
        );
        let (lo, hi) = s.swept_bounds();
        assert!(lo.y < -0.05 && hi.y > 0.0);
    }
}
