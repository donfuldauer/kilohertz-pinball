//! Spinner flap: a thin uniform plate hanging from a horizontal pivot at its
//! top edge, a 1-DOF rotor with gravity restoring torque and viscous
//! spin-down.
//!
//! Model: `dω/dt = −d·ω + γ(φ)`, with the gravity term of a uniform plate
//! of length `L` hinged at one edge, `γ(φ) = (3/(2L))·((along(φ) × g)·a)`
//! (`= −(3g/(2L))·sin φ` for vertical gravity; independent of the plate's
//! inertia, which only sets how hard a ball can spin it). Per tick the
//! gravity term is applied to `ω` at the start (semi-implicit Euler), then
//! the damping is integrated exactly over the segment: the per-tick advance
//! is `ω·(1 − e^{−d·dt})/d`. Without gravity the flap turns exactly `ω₀/d`
//! in total (geometric series); with gravity, a fast spin turns `≈ ω₀/d`
//! (the gravity torque averages out over a revolution: `ω₀/d` is the
//! high-speed limit) until it can no longer go over the top
//! (`ω² < 4·3g/(2L)` at the bottom, ≈ 46 rad/s for 28 mm), then it swings
//! and settles hanging (`φ ≡ 0 mod 2π`), ready for the next ball. Within a
//! tick the flap moves at the constant mean rate of the segment (the pose
//! is linear in `t`, as the seam asks); an impulse at `t` changes the
//! instantaneous `ω` by `((p − c) × J)·a / I` and re-predicts the rest of the
//! tick. A switch pulse is counted every time the flap passes a horizontal
//! position (`φ = π/2 + kπ`): one pulse per 180° of rotation.

use glam::Vec3;

use super::geom::OBox;
use crate::ball::BodyPoint;

/// Below this angular speed (rad/s) the flap is at rest.
const REST_OMEGA: f32 = 1.0e-4;

#[derive(Copy, Clone, Debug, PartialEq)]
pub(crate) struct Spinner {
    /// Pivot (centre of the axis).
    pub pivot: Vec3,
    /// Direction balls pass through (flap normal at rest, horizontal).
    pub dir: Vec3,
    /// Rotation axis `a = −(z × dir)`: positive `ω` swings the free edge
    /// toward `dir`.
    pub axis: Vec3,
    pub width: f32,
    pub flap: f32,
    pub thickness: f32,
    pub inertia: f32,
    pub damping: f32,
    /// Gravity (m/s², solver frame) for the restoring torque.
    pub gravity: Vec3,
    /// Committed angle (unwrapped, rad) and angular velocity.
    pub angle: f32,
    pub omega: f32,
    /// Pulses counted so far.
    pub pulses: u64,
    /// A pulse happened in the last committed tick.
    pub pulsed: bool,
    // Segment: start time, angle and instantaneous ω at the start, mean rate.
    seg_t: f32,
    seg_angle: f32,
    seg_omega: f32,
    seg_rate: f32,
    /// d(mean rate)/d(ω at the segment start) for the rest of the tick.
    seg_gain: f32,
    dt: f32,
}

/// `(1 − e^{−d·τ}) / (d·τ)` and `e^{−d·τ}`.
fn decay(d: f32, tau: f32) -> (f32, f32) {
    if tau.is_nan() || tau <= 0.0 {
        return (1.0, 1.0);
    }
    let x = d * tau;
    let e = libm::expf(-x);
    let gain = if x > 1.0e-4 {
        (1.0 - e) / x
    } else {
        1.0 - 0.5 * x
    };
    (gain, e)
}

impl Spinner {
    pub fn new(pivot: Vec3, dir: Vec3) -> Self {
        Self {
            pivot,
            dir,
            axis: -Vec3::Z.cross(dir),
            width: 0.04,
            flap: 0.028,
            thickness: 0.002,
            inertia: 1.0e-6,
            damping: 1.0,
            gravity: Vec3::new(0.0, 0.0, -9.81),
            angle: 0.0,
            omega: 0.0,
            pulses: 0,
            pulsed: false,
            seg_t: 0.0,
            seg_angle: 0.0,
            seg_omega: 0.0,
            seg_rate: 0.0,
            seg_gain: 1.0,
            dt: 0.0,
        }
    }

    fn start_segment(&mut self, t: f32, angle: f32, omega: f32) {
        let omega = if omega.is_finite() && omega.abs() >= REST_OMEGA {
            omega
        } else {
            0.0
        };
        let (gain, _) = decay(self.damping, self.dt - t);
        self.seg_t = t;
        self.seg_angle = angle;
        self.seg_omega = omega;
        self.seg_gain = gain;
        self.seg_rate = omega * gain;
    }

    /// Angular acceleration (rad/s²) from gravity at angle `phi`.
    pub fn gravity_alpha(&self, phi: f32) -> f32 {
        1.5 / self.flap * self.along(phi).cross(self.gravity).dot(self.axis)
    }

    pub fn begin_tick(&mut self, dt: f32) {
        self.dt = dt;
        let omega = self.omega + self.gravity_alpha(self.angle) * dt;
        self.start_segment(0.0, self.angle, omega);
    }

    pub fn end_tick(&mut self, dt: f32) {
        let before = self.angle;
        let after = self.angle_at(dt);
        let (_, e) = decay(self.damping, dt - self.seg_t);
        self.angle = after;
        self.omega = self.seg_omega * e;
        if self.omega.abs() < REST_OMEGA {
            self.omega = 0.0;
        }
        // Horizontal crossings: φ = π/2 + kπ ⇔ floor((φ − π/2)/π) changes.
        let k = |a: f32| libm::floorf((a - core::f32::consts::FRAC_PI_2) / core::f32::consts::PI);
        let n = (k(after) - k(before)).abs();
        self.pulsed = n > 0.0;
        if n.is_finite() {
            self.pulses = self.pulses.saturating_add(n as u64);
        }
        self.dt = dt;
        self.start_segment(dt, self.angle, self.omega);
        self.seg_t = 0.0;
    }

    pub fn angle_at(&self, t: f32) -> f32 {
        self.seg_angle + self.seg_rate * (t - self.seg_t)
    }

    /// Unit vector from the pivot along the flap at angle `phi`.
    fn along(&self, phi: f32) -> Vec3 {
        self.dir * libm::sinf(phi) - Vec3::Z * libm::cosf(phi)
    }

    pub fn body_box(&self, t: f32) -> OBox {
        let d = self.along(self.angle_at(t));
        let side = self.axis;
        let normal = d.cross(side);
        OBox {
            center: self.pivot + d * (0.5 * self.flap),
            axes: [d, side, normal],
            half: Vec3::new(0.5 * self.flap, 0.5 * self.width, 0.5 * self.thickness),
        }
    }

    pub fn closest(&self, t: f32, p: Vec3) -> BodyPoint {
        self.body_box(t).closest(p)
    }

    pub fn velocity(&self, point: Vec3) -> Vec3 {
        self.axis.cross(point - self.pivot) * self.seg_rate
    }

    pub fn reach(&self) -> f32 {
        let h = 0.5 * self.width;
        libm::sqrtf(self.flap * self.flap + h * h) + self.thickness
    }

    pub fn swept_bounds(&self) -> (Vec3, Vec3) {
        let r = Vec3::splat(self.reach());
        (self.pivot - r, self.pivot + r)
    }

    pub fn max_surface_speed(&self) -> f32 {
        self.seg_rate.abs() * self.reach()
    }

    pub fn inverse_mass(&self, point: Vec3, direction: Vec3) -> f32 {
        let arm = (point - self.pivot).cross(direction).dot(self.axis);
        arm * arm / self.inertia * self.seg_gain
    }

    pub fn apply_impulse(&mut self, t: f32, point: Vec3, impulse: Vec3) {
        let l = (point - self.pivot).cross(impulse).dot(self.axis);
        let (_, e) = decay(self.damping, t - self.seg_t);
        let omega = self.seg_omega * e + l / self.inertia;
        let angle = self.angle_at(t);
        self.start_segment(t, angle, omega);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flap() -> Spinner {
        let mut s = Spinner::new(Vec3::new(0.0, 0.0, 0.042), Vec3::Y);
        s.damping = 0.8;
        s.inertia = 8e-7;
        s
    }

    #[test]
    fn exponential_spin_down_total_angle() {
        for (w0, d) in [(30.0f32, 0.8f32), (100.0, 2.0), (5.0, 0.3)] {
            let mut s = flap();
            s.damping = d;
            s.gravity = Vec3::ZERO;
            s.omega = w0;
            for _ in 0..200_000 {
                s.begin_tick(1e-3);
                s.end_tick(1e-3);
                if s.omega == 0.0 {
                    break;
                }
            }
            let total = s.angle;
            let analytic = w0 / d;
            assert!(
                (total - analytic).abs() / analytic < 1e-3,
                "{total} vs {analytic}"
            );
            let expected = (total / core::f32::consts::PI).floor() as i64;
            assert!((s.pulses as i64 - expected).abs() <= 1);
        }
    }

    #[test]
    fn geometry_and_impulse() {
        let mut s = flap();
        s.begin_tick(1e-3);
        // Hanging: bottom edge below the pivot.
        let b = s.body_box(0.0);
        assert!((b.center - Vec3::new(0.0, 0.0, 0.028)).length() < 1e-6);
        // A push along +dir at the bottom spins it positive.
        let p = Vec3::new(0.0, -0.001, 0.02);
        s.apply_impulse(5e-4, p, Vec3::Y * 1e-4);
        assert!(s.seg_rate > 0.0);
        assert!(s.velocity(p).y > 0.0);
        assert!(s.inverse_mass(p, Vec3::Y) > 0.0);
        assert!(s.max_surface_speed() > 0.0);
        s.end_tick(1e-3);
        assert!(s.omega > 0.0 && s.angle > 0.0);
        let (lo, hi) = s.swept_bounds();
        assert!(lo.z < 0.015 && hi.z > 0.07);
        // A quarter turn later the flap is horizontal: its normal is vertical.
        s.angle = core::f32::consts::FRAC_PI_2;
        s.omega = 0.0;
        s.begin_tick(1e-3);
        let b = s.body_box(0.0);
        assert!(b.axes[2].z.abs() > 0.999);
        assert_eq!(decay(1.0, 0.0), (1.0, 1.0));
        assert!((decay(1e-9, 1e-3).0 - 1.0).abs() < 1e-6);
    }
}
