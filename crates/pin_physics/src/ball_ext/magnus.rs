//! Magnus force `F = S_m (ω × v)` (PLAN.md §3.2).

use glam::Vec3;
use pin_core::BallProfile;

use crate::ball::{Ball, BallHandle, ExternalForce, ForceContext};

/// "No force" as a hook result: `-0.0` in every component, the identity
/// of IEEE addition (`x + -0.0 == x` bit for bit, also for `x = -0.0`),
/// so adding it to the solver's force sum cannot change a single bit.
/// It compares equal to [`Vec3::ZERO`].
pub(crate) const NO_FORCE: Vec3 = Vec3::splat(-0.0);

/// The instantaneous Magnus force `S_m (ω × v)` (N), with `S_m` in kg,
/// `ω` in rad/s and `v` in m/s. Perpendicular to `v`.
#[must_use]
pub fn magnus_force(magnus_coeff: f32, angular_velocity: Vec3, velocity: Vec3) -> Vec3 {
    angular_velocity.cross(velocity) * magnus_coeff
}

/// The Magnus force the hook applies over one sub-step of `h` seconds: the
/// mean force of the exact solution of `m·dv/dt = S_m (ω × v)` with `ω`
/// held constant over the sub-step, i.e. `m (R v − v) / h` where `R`
/// rotates `v` about `ω` by `φ = S_m |ω| h / m`.
///
/// To first order in `φ` this is [`magnus_force`]; unlike the explicit
/// force it leaves `|v + F h / m|` equal to `|v|` up to rounding, so the
/// Magnus term does no work even at large spin (the explicit force adds
/// `(F h / m)²` to `|v|²` every sub-step). Returns exactly zero (as `-0.0`)
/// for `S_m = 0`, `ω = 0`, a non-positive mass or step, or any non-finite
/// input.
#[must_use]
pub fn magnus_step_force(
    magnus_coeff: f32,
    mass: f32,
    angular_velocity: Vec3,
    velocity: Vec3,
    h: f32,
) -> Vec3 {
    let positive = |x: f32| x.is_finite() && x > 0.0;
    if magnus_coeff == 0.0 || !magnus_coeff.is_finite() || !positive(mass) || !positive(h) {
        return NO_FORCE;
    }
    if angular_velocity == Vec3::ZERO || !velocity.is_finite() {
        return NO_FORCE;
    }
    let w = angular_velocity.length();
    if !positive(w) {
        return NO_FORCE;
    }
    let axis = angular_velocity / w;
    let phi = magnus_coeff * w * h / mass;
    let v_par = axis * axis.dot(velocity);
    let v_perp = velocity - v_par;
    let side = axis.cross(velocity);
    // R v − v = (ω̂ × v) sin φ + v⊥ (cos φ − 1), with cos φ − 1 written as
    // −2 sin²(φ/2) so it keeps its precision for small φ.
    let half = libm::sinf(0.5 * phi);
    let dv = side * libm::sinf(phi) - v_perp * (2.0 * half * half);
    let f = dv * (mass / h);
    if f.is_finite() { f } else { NO_FORCE }
}

/// Magnus effect hook (PLAN.md §3.2): `F = S_m (ω × v)` with
/// `S_m = BallProfile::magnus_coeff`, integrated over each sub-step as an
/// exact rotation of the velocity ([`magnus_step_force`]), so it bends
/// the path without changing the speed.
///
/// Stateless. A ball whose profile has `magnus_coeff = 0` (every built-in
/// profile but `uv_globall`) or that does not spin gets exactly no force,
/// so it is bit-identical to a solver without the hook.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Magnus;

impl Magnus {
    /// The Magnus hook.
    #[must_use]
    pub const fn new() -> Self {
        Self
    }
}

impl ExternalForce for Magnus {
    fn force_in(
        &mut self,
        _ctx: &ForceContext<'_>,
        _handle: BallHandle,
        ball: &Ball,
        profile: &BallProfile,
        dt: f32,
    ) -> Vec3 {
        magnus_step_force(
            profile.magnus_coeff,
            profile.mass_kg,
            ball.angular_velocity,
            ball.velocity,
            dt,
        )
    }

    fn force(&mut self, _handle: BallHandle, ball: &Ball, profile: &BallProfile, dt: f32) -> Vec3 {
        magnus_step_force(
            profile.magnus_coeff,
            profile.mass_kg,
            ball.angular_velocity,
            ball.velocity,
            dt,
        )
    }
}
