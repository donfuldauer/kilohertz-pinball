//! The cabinet tilt bob: a damped spherical pendulum inside a contact ring.
//!
//! The bob is a point mass on a massless rod of length `L` hanging from a
//! pivot fixed to the cabinet. In the cabinet frame it feels gravity and,
//! when the cabinet is nudged by `Δv`, an instantaneous velocity change
//! `−Δv` (the pivot moves, the bob lags). Integration: 4 sub-steps per
//! tick of symplectic Euler on the position, each followed by projection
//! back onto the sphere `|r| = L` and removal of the radial velocity
//! (RATTLE-style), with viscous damping `−c·v`. The small-swing period is
//! `2π·√(L/g)` (checked within 2 %, Gate A1).
//!
//! The contact ring surrounds the bob at rest: the bob touches it when its
//! offset perpendicular to gravity reaches `gap`; it then bounces back with
//! `restitution` (inelastic below), and the tilt switch is closed for every
//! step in which the bob touched the ring.

use glam::Vec3;
use pin_core::PhysicsDefaults;

/// Sub-steps per [`TiltBob::step`].
const SUBSTEPS: u32 = 4;

/// Parameters of a [`TiltBob`] (SI).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TiltBobParams {
    /// Pendulum length (pivot to bob centre), m.
    pub length: f32,
    /// Bob offset (perpendicular to gravity) at which it touches the ring, m.
    pub gap: f32,
    /// Viscous damping rate `c` of the swing, 1/s.
    pub damping: f32,
    /// Bob–ring restitution, 0..1.
    pub restitution: f32,
    /// Gravity in the cabinet frame, m/s² (default `(0, 0, −9.81)`).
    pub gravity: Vec3,
}

impl TiltBobParams {
    /// The `[mechs.tilt_bob]` defaults of `d` with gravity `(0, 0, −9.81)`.
    #[must_use]
    pub fn from_defaults(d: &PhysicsDefaults) -> Self {
        let t = &d.mechs.tilt_bob;
        Self {
            length: t.length,
            gap: t.gap,
            damping: t.damping,
            restitution: t.restitution,
            gravity: Vec3::new(0.0, 0.0, -9.81),
        }
    }

    /// Whether the parameters can be simulated: positive finite length and
    /// gap (`gap < length`), finite non-negative damping, restitution in
    /// 0..=1, finite non-zero gravity.
    #[must_use]
    pub fn is_valid(&self) -> bool {
        self.length.is_finite()
            && self.length > 0.0
            && self.gap.is_finite()
            && self.gap > 0.0
            && self.gap < self.length
            && self.damping.is_finite()
            && self.damping >= 0.0
            && (0.0..=1.0).contains(&self.restitution)
            && self.gravity.is_finite()
            && self.gravity != Vec3::ZERO
    }
}

impl Default for TiltBobParams {
    fn default() -> Self {
        Self::from_defaults(&PhysicsDefaults::builtin())
    }
}

/// A damped spherical-pendulum tilt bob (see the [module docs](self)).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct TiltBob {
    params: TiltBobParams,
    /// Bob position relative to the pivot.
    r: Vec3,
    v: Vec3,
    /// Touched the ring during the last step.
    closed: bool,
}

impl TiltBob {
    /// A bob at rest. Invalid parameters ([`TiltBobParams::is_valid`]) are
    /// replaced by the built-in defaults (with the given gravity if usable).
    #[must_use]
    pub fn new(params: TiltBobParams) -> Self {
        let params = if params.is_valid() {
            params
        } else {
            let mut d = TiltBobParams::default();
            if params.gravity.is_finite() && params.gravity != Vec3::ZERO {
                d.gravity = params.gravity;
            }
            d
        };
        Self {
            params,
            r: params.gravity.normalize() * params.length,
            v: Vec3::ZERO,
            closed: false,
        }
    }

    /// The parameters in use.
    #[must_use]
    pub fn params(&self) -> &TiltBobParams {
        &self.params
    }

    /// Changes gravity (the bob keeps its position; the rest direction
    /// follows gravity). Ignored when not finite or zero.
    pub fn set_gravity(&mut self, gravity: Vec3) {
        if gravity.is_finite() && gravity != Vec3::ZERO && gravity != self.params.gravity {
            self.params.gravity = gravity;
        }
    }

    /// Cabinet velocity change `Δv` (m/s): the bob's velocity relative to
    /// the cabinet changes by `−Δv` (radial part removed by the rod).
    /// Non-finite input is ignored.
    pub fn nudge(&mut self, cabinet_dv: Vec3) {
        if !cabinet_dv.is_finite() {
            return;
        }
        self.v -= cabinet_dv;
        let n = self.r / self.params.length;
        self.v -= n * self.v.dot(n);
    }

    fn down(&self) -> Vec3 {
        self.params.gravity.normalize()
    }

    /// Offset of the bob from its rest position, perpendicular to gravity.
    #[must_use]
    pub fn offset(&self) -> Vec3 {
        let g = self.down();
        self.r - g * self.r.dot(g)
    }

    /// Bob velocity relative to the cabinet, m/s.
    #[must_use]
    pub fn velocity(&self) -> Vec3 {
        self.v
    }

    /// Whether the bob touched the ring during the last [`step`](Self::step).
    #[must_use]
    pub fn closed(&self) -> bool {
        self.closed
    }

    /// Small-swing period `2π·√(L/g)`, seconds (undamped).
    #[must_use]
    pub fn small_swing_period(&self) -> f32 {
        2.0 * core::f32::consts::PI * libm::sqrtf(self.params.length / self.params.gravity.length())
    }

    /// Advances `dt` seconds. Non-finite or non-positive `dt` does nothing.
    pub fn step(&mut self, dt: f32) {
        self.closed = false;
        if dt.is_nan() || dt <= 0.0 || !dt.is_finite() {
            return;
        }
        let p = self.params;
        let h = dt / SUBSTEPS as f32;
        let g = self.down();
        for _ in 0..SUBSTEPS {
            self.v += (p.gravity - self.v * p.damping) * h;
            let target = self.r + self.v * h;
            let mut r = target.normalize_or(g) * p.length;
            // Contact ring: offset perpendicular to gravity at most `gap`.
            let along = r.dot(g);
            let off = r - g * along;
            let rho = off.length();
            if rho >= p.gap {
                self.closed = true;
                let u = off / rho;
                let along = libm::sqrtf((p.length * p.length - p.gap * p.gap).max(0.0));
                r = u * p.gap + g * along;
                // New velocity from the projected motion, then the bounce.
                self.v = (r - self.r) / h;
                let vn = self.v.dot(u);
                if vn > 0.0 {
                    self.v -= u * (vn * (1.0 + p.restitution));
                }
            } else {
                self.v = (r - self.r) / h;
            }
            let n = r / p.length;
            self.v -= n * self.v.dot(n);
            self.r = r;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_swing_period_matches_the_pendulum_formula() {
        let mut p = TiltBobParams::default();
        p.damping = 0.0;
        p.gap = 0.05;
        let mut bob = TiltBob::new(p);
        bob.nudge(Vec3::new(0.01, 0.0, 0.0));
        // Time between successive upward zero crossings of x.
        let mut last = bob.offset().x;
        let mut crossings = Vec::new();
        for i in 0..5000 {
            bob.step(1e-3);
            let x = bob.offset().x;
            if last < 0.0 && x >= 0.0 {
                crossings.push(i as f32 * 1e-3);
            }
            last = x;
        }
        let measured = (crossings[3] - crossings[0]) / 3.0;
        let analytic = bob.small_swing_period();
        assert!((measured - analytic).abs() / analytic < 0.02, "{measured} vs {analytic}");
        assert!(!bob.closed());
    }

    #[test]
    fn hard_nudge_closes_and_bounces() {
        let mut bob = TiltBob::new(TiltBobParams::default());
        bob.nudge(Vec3::new(0.0, 1.0, 0.0));
        let mut any = false;
        for _ in 0..300 {
            bob.step(1e-3);
            any |= bob.closed();
            assert!(bob.offset().length() <= bob.params().gap + 1e-5);
        }
        assert!(any);
        bob.nudge(Vec3::NAN);
        bob.step(f32::NAN);
        assert!(!bob.closed());
    }

    #[test]
    fn invalid_params_fall_back() {
        let mut p = TiltBobParams::default();
        p.length = -1.0;
        p.gravity = Vec3::new(0.0, -1.0, -9.0);
        let bob = TiltBob::new(p);
        assert!(bob.params().is_valid());
        assert_eq!(bob.params().gravity, Vec3::new(0.0, -1.0, -9.0));
        assert_eq!(bob.offset().length(), 0.0);
        let mut bob = bob;
        bob.set_gravity(Vec3::ZERO);
        bob.set_gravity(Vec3::new(0.0, 0.0, -9.81));
        assert_eq!(bob.params().gravity, Vec3::new(0.0, 0.0, -9.81));
        assert!(bob.offset().length() > 0.0);
        assert!(bob.velocity().length() == 0.0);
    }
}
