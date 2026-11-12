//! Impulse-based contact response for spheres: normal impulse with
//! restitution, Coulomb friction with the sliding → rolling transition, and
//! rolling resistance.
//!
//! All functions work on velocities only, never allocate, and (with a
//! surface at rest) never increase kinetic energy.

use glam::{Quat, Vec3};

use super::types::{Ball, IMPACT_REPORT_SPEED};

/// Mass properties of a solid sphere.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SphereBody {
    /// Radius, metres.
    pub radius: f32,
    /// Mass, kilograms.
    pub mass: f32,
}

impl SphereBody {
    /// Moment of inertia `I = 2/5 · m · r²`.
    #[must_use]
    pub fn inertia(&self) -> f32 {
        0.4 * self.mass * self.radius * self.radius
    }
}

/// Coefficients of one contact.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ContactCoefficients {
    /// Coefficient of restitution of the material pair.
    pub restitution: f32,
    /// Sliding friction coefficient μ.
    pub friction: f32,
    /// Rolling resistance coefficient (0 for ball–ball contacts).
    pub rolling_resistance: f32,
}

/// Result of a contact resolution.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct ContactOutcome {
    /// Normal impulse applied, N·s (0 when the bodies were separating).
    pub normal_impulse: f32,
    /// Approach speed along the normal before the impulse, m/s (≥ 0).
    pub approach_speed: f32,
}

impl ContactOutcome {
    const NONE: Self = Self {
        normal_impulse: 0.0,
        approach_speed: 0.0,
    };
}

/// Restitution actually used for an impact.
///
/// A `persistent` contact (the ball was already supported in that direction)
/// is inelastic. Otherwise restitution fades in linearly between
/// [`IMPACT_REPORT_SPEED`] and twice that speed, so that a ball settling
/// under gravity comes to rest instead of bouncing forever.
#[must_use]
pub fn effective_restitution(restitution: f32, approach_speed: f32, persistent: bool) -> f32 {
    if persistent {
        return 0.0;
    }
    let ramp = ((approach_speed - IMPACT_REPORT_SPEED) / IMPACT_REPORT_SPEED).clamp(0.0, 1.0);
    restitution.clamp(0.0, 1.0) * ramp
}

/// Component of `v` perpendicular to the unit vector `n`.
fn tangential(v: Vec3, n: Vec3) -> Vec3 {
    v - n * v.dot(n)
}

/// Impulses a contact has applied so far while the contacts of a sub-step
/// boundary are solved together. Accumulating them lets a later sweep take
/// back part of an earlier impulse, which is what makes a ball held by
/// several contacts (a groove, a corner, a stack of balls) come to rest.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct ContactImpulse {
    /// Normal impulse, N·s (never negative).
    pub normal: f32,
    /// Friction impulse, N·s (tangential, at most `μ · normal` long).
    pub tangent: Vec3,
    /// Angular impulse of the rolling and pivoting resistance, N·m·s.
    pub rolling: Vec3,
}

/// Clamps the length of `v` to `max`.
fn clamp_length(v: Vec3, max: f32) -> Vec3 {
    let len = v.length();
    if len > max {
        if max > 0.0 {
            v * (max / len)
        } else {
            Vec3::ZERO
        }
    } else {
        v
    }
}

/// One solver step for the contact of a ball with a surface.
///
/// `normal` is the unit normal from the surface toward the ball centre and
/// `surface_velocity` the velocity of the surface at the contact point. The
/// contact slip velocity is `v + ω × r_c − v_surface` with `r_c = −r · n`.
/// `target_normal_speed` is the separation speed the contact asks for (the
/// rebound speed of an impact, 0 for a resting contact) and `acc` the
/// impulses this contact has applied so far in the current solve.
///
/// In order: the normal impulse (total clamped to ≥ 0); then, if friction
/// and rolling resistance together can hold the ball still on the surface,
/// it stops (static rest); otherwise the rolling-resistance torque (total at
/// most `c · J_n · r`, for rolling and for pivoting) opposes the angular
/// velocity and Coulomb friction (total at most `μ · J_n`) acts on the slip,
/// up to the impulse that ends it (sliding → rolling).
///
/// Returns the size of the change in m/s (for convergence tests).
pub fn solve_surface_contact(
    ball: &mut Ball,
    body: SphereBody,
    normal: Vec3,
    surface_velocity: Vec3,
    k: ContactCoefficients,
    target_normal_speed: f32,
    acc: &mut ContactImpulse,
) -> f32 {
    let n = normal;
    let r = body.radius;
    let m = body.mass;
    let inertia = body.inertia();
    let rc = n * -r;
    let before = (ball.velocity, ball.angular_velocity);

    let un = (ball.velocity - surface_velocity).dot(n);
    if un.is_nan() {
        return 0.0;
    }
    let total = (acc.normal + m * (target_normal_speed - un)).max(0.0);
    ball.velocity += n * ((total - acc.normal) / m);
    acc.normal = total;

    let f_max = k.friction.max(0.0) * total;
    let l_max = k.rolling_resistance.max(0.0) * total * r;

    // Static rest: friction stops the centre, the resistance torque takes
    // what is left of the spin.
    let vt = tangential(ball.velocity - surface_velocity, n);
    let w = ball.angular_velocity;
    let stop_t = acc.tangent - vt * m;
    let stop_l = acc.rolling - w * inertia - n.cross(vt) * (r * m);
    let stop_ln = n * stop_l.dot(n);
    let rest = total > 0.0
        && stop_t.length() <= f_max
        && (stop_l - stop_ln).length() <= l_max
        && stop_ln.length() <= l_max;
    if rest {
        ball.velocity -= vt;
        ball.angular_velocity = Vec3::ZERO;
        acc.tangent = stop_t;
        acc.rolling = stop_l;
    } else {
        // Rolling and pivoting resistance, each limited on its own.
        let want = acc.rolling - w * inertia;
        let want_n = n * want.dot(n);
        let new_l = clamp_length(want - want_n, l_max) + clamp_length(want_n, l_max);
        ball.angular_velocity += (new_l - acc.rolling) / inertia;
        acc.rolling = new_l;

        // Coulomb friction on the slip velocity. 1/m + r²/I = 7/(2m).
        let ut = tangential(
            ball.velocity + ball.angular_velocity.cross(rc) - surface_velocity,
            n,
        );
        let new_t = clamp_length(tangential(acc.tangent, n) - ut * (m * (2.0 / 7.0)), f_max);
        let j = new_t - acc.tangent;
        ball.velocity += j / m;
        ball.angular_velocity += rc.cross(j) / inertia;
        acc.tangent = new_t;
    }
    (ball.velocity - before.0).length() + (ball.angular_velocity - before.1).length() * r
}

/// Resolves an impact of a ball on a surface in one shot (see
/// [`solve_surface_contact`]): restitution from [`effective_restitution`],
/// then friction and rolling resistance for the impulse of this impact.
/// Nothing happens unless the ball approaches the surface faster than
/// `min_approach`.
pub fn resolve_surface_contact(
    ball: &mut Ball,
    body: SphereBody,
    normal: Vec3,
    surface_velocity: Vec3,
    k: ContactCoefficients,
    persistent: bool,
    min_approach: f32,
) -> ContactOutcome {
    let approach = -(ball.velocity - surface_velocity).dot(normal);
    if approach.is_nan() || approach <= min_approach {
        return ContactOutcome::NONE;
    }
    let e = effective_restitution(k.restitution, approach, persistent);
    let mut acc = ContactImpulse::default();
    solve_surface_contact(
        ball,
        body,
        normal,
        surface_velocity,
        k,
        e * approach,
        &mut acc,
    );
    ContactOutcome {
        normal_impulse: acc.normal,
        approach_speed: approach,
    }
}

/// One solver step for the contact of two free balls. `normal` is the unit
/// vector from `b` toward `a`; `target_normal_speed` and `acc` are as in
/// [`solve_surface_contact`].
///
/// The contact point divides the line of centres in the ratio of the radii,
/// whatever the current distance, so the impulse pair conserves linear and
/// angular momentum exactly even when the balls overlap or are a rounding
/// error apart.
#[allow(clippy::too_many_arguments)]
pub fn solve_ball_contact(
    a: &mut Ball,
    body_a: SphereBody,
    b: &mut Ball,
    body_b: SphereBody,
    normal: Vec3,
    k: ContactCoefficients,
    target_normal_speed: f32,
    acc: &mut ContactImpulse,
) -> f32 {
    let n = normal;
    let d = (a.position - b.position).dot(n);
    let rsum = body_a.radius + body_b.radius;
    let arm_a = n * (-d * body_a.radius / rsum);
    let arm_b = n * (d * body_b.radius / rsum);
    let (ima, imb) = (1.0 / body_a.mass, 1.0 / body_b.mass);
    let kn = ima + imb;
    let before = (a.velocity, b.velocity);

    let un = (a.velocity - b.velocity).dot(n);
    if un.is_nan() {
        return 0.0;
    }
    let total = (acc.normal + (target_normal_speed - un) / kn).max(0.0);
    let dj = total - acc.normal;
    a.velocity += n * (dj * ima);
    b.velocity -= n * (dj * imb);
    acc.normal = total;

    let f_max = k.friction.max(0.0) * total;
    if f_max > 0.0 || acc.tangent != Vec3::ZERO {
        let ut = tangential(
            a.velocity + a.angular_velocity.cross(arm_a)
                - b.velocity
                - b.angular_velocity.cross(arm_b),
            n,
        );
        let (iia, iib) = (1.0 / body_a.inertia(), 1.0 / body_b.inertia());
        let kt = kn + arm_a.length_squared() * iia + arm_b.length_squared() * iib;
        let new_t = clamp_length(tangential(acc.tangent, n) - ut / kt, f_max);
        let j = new_t - acc.tangent;
        a.velocity += j * ima;
        b.velocity -= j * imb;
        a.angular_velocity += arm_a.cross(j) * iia;
        b.angular_velocity -= arm_b.cross(j) * iib;
        acc.tangent = new_t;
    }
    (a.velocity - before.0).length() + (b.velocity - before.1).length()
}

/// Resolves an impact of two free balls in one shot (see
/// [`solve_ball_contact`]). `normal` is the unit vector from `b` toward
/// `a`. Nothing happens unless the balls approach each other faster than
/// `min_approach`.
#[allow(clippy::too_many_arguments)]
pub fn resolve_ball_contact(
    a: &mut Ball,
    body_a: SphereBody,
    b: &mut Ball,
    body_b: SphereBody,
    normal: Vec3,
    k: ContactCoefficients,
    persistent: bool,
    min_approach: f32,
) -> ContactOutcome {
    let approach = -(a.velocity - b.velocity).dot(normal);
    if approach.is_nan() || approach <= min_approach {
        return ContactOutcome::NONE;
    }
    let e = effective_restitution(k.restitution, approach, persistent);
    let mut acc = ContactImpulse::default();
    solve_ball_contact(a, body_a, b, body_b, normal, k, e * approach, &mut acc);
    ContactOutcome {
        normal_impulse: acc.normal,
        approach_speed: approach,
    }
}

/// First time in `[0, window]` at which two spheres touch, given the
/// relative position `p = x_a − x_b`, relative velocity `w = v_a − v_b` and
/// the sum of the radii. Spheres that already touch or overlap hit at time 0
/// only if they close in faster than 1e-5 of their relative speed.
#[must_use]
pub fn sphere_toi(p: Vec3, w: Vec3, radius_sum: f32, window: f32) -> Option<f32> {
    let ww = w.length_squared();
    let pw = p.dot(w);
    if !(ww > 0.0 && pw < 0.0) {
        return None;
    }
    let c = p.length_squared() - radius_sum * radius_sum;
    if c <= 0.0 {
        let d = p.length();
        return (d > 1.0e-9 && -pw > 1.0e-5 * ww.sqrt() * d).then_some(0.0);
    }
    let disc = pw * pw - ww * c;
    if disc.is_nan() || disc < 0.0 {
        return None;
    }
    let t = c / (-pw + disc.sqrt());
    (t <= window).then_some(t)
}

/// Rotates `q` by the angular velocity `w` (rad/s) over `t` seconds
/// (exact for constant `w`; `libm` trigonometry).
#[must_use]
pub fn integrate_orientation(q: Quat, w: Vec3, t: f32) -> Quat {
    let rate = w.length();
    let half = 0.5 * rate * t;
    if half.is_nan() || half <= 0.0 || !half.is_finite() {
        return q;
    }
    let s = libm::sinf(half) / rate;
    let dq = Quat::from_xyzw(w.x * s, w.y * s, w.z * s, libm::cosf(half));
    (dq * q).normalize()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pin_core::ids::ProfileId;
    use proptest::prelude::*;

    const BODY: SphereBody = SphereBody {
        radius: 0.0135,
        mass: 0.08,
    };

    fn ke(b: &Ball, body: SphereBody) -> f64 {
        0.5 * f64::from(body.mass) * b.velocity.as_dvec3().length_squared()
            + 0.5 * f64::from(body.inertia()) * b.angular_velocity.as_dvec3().length_squared()
    }

    fn v3(range: f32) -> impl Strategy<Value = Vec3> {
        (-range..range, -range..range, -range..range).prop_map(|(x, y, z)| Vec3::new(x, y, z))
    }

    fn unit() -> impl Strategy<Value = Vec3> {
        v3(1.0)
            .prop_filter("non-zero", |v| v.length() > 0.1)
            .prop_map(Vec3::normalize)
    }

    #[test]
    fn restitution_ramp() {
        assert_eq!(effective_restitution(0.8, 10.0, true), 0.0);
        assert_eq!(effective_restitution(0.8, 0.04, false), 0.0);
        assert_eq!(effective_restitution(0.8, 0.05, false), 0.0);
        assert!((effective_restitution(0.8, 0.075, false) - 0.4).abs() < 1e-6);
        assert_eq!(effective_restitution(0.8, 1.0, false), 0.8);
        assert_eq!(effective_restitution(3.0, 1.0, false), 1.0);
    }

    #[test]
    fn head_on_bounce_uses_restitution() {
        let mut b = Ball::new(ProfileId(0), Vec3::ZERO);
        b.velocity = Vec3::new(0.0, 0.0, -2.0);
        let k = ContactCoefficients {
            restitution: 0.5,
            friction: 0.2,
            rolling_resistance: 0.0,
        };
        let out = resolve_surface_contact(&mut b, BODY, Vec3::Z, Vec3::ZERO, k, false, 0.0);
        assert!((b.velocity.z - 1.0).abs() < 1e-6);
        assert!((out.approach_speed - 2.0).abs() < 1e-6);
        assert!((out.normal_impulse - 0.08 * 3.0).abs() < 1e-6);
        // Separating now: nothing more happens.
        let before = b;
        let out = resolve_surface_contact(&mut b, BODY, Vec3::Z, Vec3::ZERO, k, false, 0.0);
        assert_eq!(out.normal_impulse, 0.0);
        assert_eq!(b, before);
    }

    #[test]
    fn sliding_becomes_rolling_with_five_sevenths() {
        // A sliding, non-spinning ball pressed onto the floor with ample
        // friction ends at 5/7 of its speed, rolling.
        let mut b = Ball::new(ProfileId(0), Vec3::ZERO);
        b.velocity = Vec3::new(0.7, 0.0, -1.0);
        let k = ContactCoefficients {
            restitution: 0.0,
            friction: 5.0,
            rolling_resistance: 0.0,
        };
        resolve_surface_contact(&mut b, BODY, Vec3::Z, Vec3::ZERO, k, true, 0.0);
        assert!((b.velocity.x - 0.5).abs() < 1e-6);
        let slip = b.velocity + b.angular_velocity.cross(Vec3::Z * -BODY.radius);
        assert!(slip.length() < 1e-6);
    }

    #[test]
    fn static_rest_stops_a_creeping_ball() {
        let mut b = Ball::new(ProfileId(0), Vec3::ZERO);
        b.velocity = Vec3::new(1.0e-6, 0.0, -0.00981);
        b.angular_velocity = Vec3::new(0.0, 1.0e-4, 1.0e-4);
        let k = ContactCoefficients {
            restitution: 0.3,
            friction: 0.2,
            rolling_resistance: 0.002,
        };
        resolve_surface_contact(&mut b, BODY, Vec3::Z, Vec3::ZERO, k, true, 0.0);
        assert_eq!(b.velocity, Vec3::ZERO);
        assert_eq!(b.angular_velocity, Vec3::ZERO);
    }

    #[test]
    fn moving_surface_drags_the_ball() {
        let mut b = Ball::new(ProfileId(0), Vec3::ZERO);
        b.velocity = Vec3::new(0.0, 0.0, -1.0);
        let k = ContactCoefficients {
            restitution: 0.0,
            friction: 0.5,
            rolling_resistance: 0.0,
        };
        resolve_surface_contact(
            &mut b,
            BODY,
            Vec3::Z,
            Vec3::new(2.0, 0.0, 0.0),
            k,
            true,
            0.0,
        );
        assert!(b.velocity.x > 0.1);
    }

    #[test]
    fn accumulated_solve_brings_a_ball_to_rest_in_a_groove() {
        // Two planes 0.6 rad either side of level; gravity for one tick.
        let normals = [
            Vec3::new(libm::sinf(0.6), 0.0, libm::cosf(0.6)),
            Vec3::new(-libm::sinf(0.6), 0.0, libm::cosf(0.6)),
        ];
        let k = ContactCoefficients {
            restitution: 0.25,
            friction: 0.2,
            rolling_resistance: 0.002,
        };
        let mut b = Ball::new(ProfileId(0), Vec3::ZERO);
        b.velocity = Vec3::new(0.0, 0.0, -0.00981);
        let mut acc = [ContactImpulse::default(); 2];
        for _ in 0..10 {
            for (n, acc) in normals.iter().zip(&mut acc) {
                solve_surface_contact(&mut b, BODY, *n, Vec3::ZERO, k, 0.0, acc);
            }
        }
        assert!(b.velocity.length() < 1e-7, "{:?}", b.velocity);
        assert!(b.angular_velocity.length() < 1e-5);
        // Normal and friction impulses together carry the weight.
        let up: f32 = acc
            .iter()
            .zip(normals)
            .map(|(a, n)| a.normal * n.z + a.tangent.z)
            .sum();
        assert!((up - BODY.mass * 0.00981).abs() < 1e-3 * BODY.mass * 0.00981);
        assert!(acc.iter().all(|a| a.normal > 0.0));
        // A contact the ball leaves gives its impulses back.
        b.velocity = Vec3::new(0.0, 0.0, 1.0);
        solve_surface_contact(&mut b, BODY, normals[0], Vec3::ZERO, k, 0.0, &mut acc[0]);
        assert_eq!(acc[0].normal, 0.0);
        assert_eq!(acc[0].tangent, Vec3::ZERO);
    }

    #[test]
    fn min_approach_gates_one_shot_impacts() {
        let k = ContactCoefficients {
            restitution: 0.5,
            friction: 0.2,
            rolling_resistance: 0.0,
        };
        let mut b = Ball::new(ProfileId(0), Vec3::ZERO);
        b.velocity = Vec3::new(0.0, 0.0, -0.04);
        let before = b;
        let out = resolve_surface_contact(&mut b, BODY, Vec3::Z, Vec3::ZERO, k, false, 0.05);
        assert_eq!((out.normal_impulse, b), (0.0, before));
        let mut a = Ball::new(ProfileId(0), Vec3::Z * 0.027);
        a.velocity = Vec3::new(0.0, 0.0, -0.04);
        let mut c = Ball::new(ProfileId(0), Vec3::ZERO);
        let out = resolve_ball_contact(&mut a, BODY, &mut c, BODY, Vec3::Z, k, false, 0.05);
        assert_eq!(out.normal_impulse, 0.0);
        let out = resolve_ball_contact(&mut a, BODY, &mut c, BODY, Vec3::Z, k, false, 0.0);
        assert!(out.normal_impulse > 0.0);
        assert!((a.velocity.z - c.velocity.z).abs() < 1e-7);
    }

    #[test]
    fn toi_cases() {
        // Head-on from 1 m apart at 2 m/s, radii sum 0.2: 0.4 s.
        let t = sphere_toi(Vec3::X, Vec3::new(-2.0, 0.0, 0.0), 0.2, 1.0).unwrap();
        assert!((t - 0.4).abs() < 1e-6);
        assert!(sphere_toi(Vec3::X, Vec3::new(-2.0, 0.0, 0.0), 0.2, 0.3).is_none());
        // Receding, missing, at rest.
        assert!(sphere_toi(Vec3::X, Vec3::X, 0.2, 9.0).is_none());
        assert!(sphere_toi(Vec3::X, Vec3::new(-1.0, 1.0, 0.0), 0.2, 9.0).is_none());
        assert!(sphere_toi(Vec3::X, Vec3::ZERO, 0.2, 9.0).is_none());
        // Overlapping: only when closing in.
        assert_eq!(sphere_toi(Vec3::X * 0.1, -Vec3::X, 0.2, 1.0), Some(0.0));
        assert!(sphere_toi(Vec3::X * 0.1, Vec3::Y, 0.2, 1.0).is_none());
        assert!(sphere_toi(Vec3::ZERO, Vec3::Y, 0.2, 1.0).is_none());
    }

    #[test]
    fn orientation_quarter_turn() {
        let q = integrate_orientation(Quat::IDENTITY, Vec3::Z * std::f32::consts::FRAC_PI_2, 1.0);
        let x = q * Vec3::X;
        assert!((x - Vec3::Y).length() < 1e-6);
        assert_eq!(
            integrate_orientation(Quat::IDENTITY, Vec3::ZERO, 1.0),
            Quat::IDENTITY
        );
        assert_eq!(
            integrate_orientation(Quat::IDENTITY, Vec3::X, 0.0),
            Quat::IDENTITY
        );
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(512))]

        #[test]
        fn surface_contact_never_gains_energy(
            v in v3(10.0), w in v3(300.0), n in unit(),
            e in 0.0f32..1.0, mu in 0.0f32..1.5, c in 0.0f32..0.02, persistent: bool,
        ) {
            let mut b = Ball::new(ProfileId(0), Vec3::ZERO);
            b.velocity = v;
            b.angular_velocity = w;
            let before = ke(&b, BODY);
            let k = ContactCoefficients { restitution: e, friction: mu, rolling_resistance: c };
            let out = resolve_surface_contact(&mut b, BODY, n, Vec3::ZERO, k, persistent, 0.0);
            prop_assert!(ke(&b, BODY) <= before * (1.0 + 1e-5) + 1e-12);
            prop_assert!(b.velocity.is_finite() && b.angular_velocity.is_finite());
            if out.normal_impulse > 0.0 {
                // The ball no longer approaches the surface.
                prop_assert!(b.velocity.dot(n) >= -1e-4);
            }
        }

        #[test]
        fn ball_contact_conserves_momentum_and_loses_energy(
            va in v3(8.0), vb in v3(8.0), wa in v3(200.0), wb in v3(200.0), n in unit(),
            e in 0.0f32..1.0, mu in 0.0f32..1.0, gap in -1.0e-4f32..1.0e-4,
        ) {
            let ba = SphereBody { radius: 0.00794, mass: 0.0165 };
            let bb = SphereBody { radius: 0.015875, mass: 0.130 };
            let mut a = Ball::new(ProfileId(0), n * (ba.radius + bb.radius + gap));
            let mut b = Ball::new(ProfileId(0), Vec3::ZERO);
            a.velocity = va; a.angular_velocity = wa;
            b.velocity = vb; b.angular_velocity = wb;
            let p0 = va.as_dvec3() * f64::from(ba.mass) + vb.as_dvec3() * f64::from(bb.mass);
            let l = |a: &Ball, b: &Ball| {
                a.position.as_dvec3().cross(a.velocity.as_dvec3()) * f64::from(ba.mass)
                    + a.angular_velocity.as_dvec3() * f64::from(ba.inertia())
                    + b.position.as_dvec3().cross(b.velocity.as_dvec3()) * f64::from(bb.mass)
                    + b.angular_velocity.as_dvec3() * f64::from(bb.inertia())
            };
            let l0 = l(&a, &b);
            let e0 = ke(&a, ba) + ke(&b, bb);
            let k = ContactCoefficients { restitution: e, friction: mu, rolling_resistance: 0.0 };
            resolve_ball_contact(&mut a, ba, &mut b, bb, n, k, false, 0.0);
            let p1 = a.velocity.as_dvec3() * f64::from(ba.mass)
                + b.velocity.as_dvec3() * f64::from(bb.mass);
            prop_assert!((p1 - p0).length() <= 1e-6 * (1.0 + p0.length()));
            prop_assert!((l(&a, &b) - l0).length() <= 1e-6 * (1e-3 + l0.length()));
            prop_assert!(ke(&a, ba) + ke(&b, bb) <= e0 * (1.0 + 1e-5) + 1e-12);
        }

        #[test]
        fn toi_matches_distance(
            p in v3(1.0), w in v3(20.0), rsum in 0.01f32..0.1,
        ) {
            if let Some(t) = sphere_toi(p, w, rsum, 10.0) {
                prop_assert!(t >= 0.0);
                if t > 0.0 {
                    let d = (p + w * t).length();
                    prop_assert!((d - rsum).abs() < 1e-4 * (1.0 + p.length()));
                }
            }
        }

        #[test]
        fn orientation_stays_unit(w in v3(500.0), t in 0.0f32..0.01) {
            let q = integrate_orientation(Quat::from_xyzw(0.5, 0.5, 0.5, 0.5), w, t);
            prop_assert!((q.length() - 1.0).abs() < 1e-5);
        }
    }
}
