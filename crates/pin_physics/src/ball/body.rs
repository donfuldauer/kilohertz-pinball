//! Moving bodies: rigid bodies that move during a tick and collide with the
//! free balls inside [`BallSolver::step_with_bodies`](super::BallSolver::step_with_bodies).
//!
//! The seam Subphases 2.3 (flippers) and 2.4 (slingshot kickers, plunger
//! tip, spinner flaps, …) build on. Nothing here knows what a body *is*: a
//! body is a surface whose pose over the tick is a function of time that its
//! owner predicts, plus a mass model the solver can push against.
//!
//! # Ownership
//!
//! The owner (a flipper bank, a mech set) implements [`MovingBodies`] for a
//! *set* of bodies and keeps all their state; it integrates its own drive
//! (coil torque, springs, end stops). The ball solver never stores a body: the
//! caller lends the sets to each step,
//!
//! ```ignore
//! solver.step_with_bodies(&geo, &mut [&mut flippers, &mut mechs], dt, &mut events);
//! ```
//!
//! and a set is identified by its index in that slice (`set`), a body by its
//! index within the set (`body`, `0..body_count()`). Impacts on a body are
//! reported as [`BallEvent::Impact`](super::BallEvent::Impact) with
//! [`ImpactTarget::Body`](super::ImpactTarget::Body); switches are the
//! owner's business (it sees its own motion and every impulse).
//!
//! # Time
//!
//! Every query and every impulse carries `time`: seconds since the start of
//! the current tick, in `[0, dt]`.
//!
//! # Call order within one step (the contract)
//!
//! 1. **Begin.** [`begin_tick(dt)`](MovingBodies::begin_tick) for every set,
//!    in slice order, before anything else of the step. The body *predicts*
//!    its trajectory over `[0, dt]` from its committed state and its drive for
//!    this tick (e.g. `ω₁ = ω₀ + α·dt`, then `θ(t) = θ₀ + ω₁·t`, clamped at the
//!    stops). The prediction must be a deterministic function of `t`,
//!    continuous in position (velocities may jump, e.g. at an end stop).
//! 2. **Mid-tick queries**, any number, from the solver's discrete-event
//!    loop: [`swept_bounds`](MovingBodies::swept_bounds),
//!    [`max_surface_speed`](MovingBodies::max_surface_speed),
//!    [`closest_point`](MovingBodies::closest_point),
//!    [`cast_sphere`](MovingBodies::cast_sphere),
//!    [`velocity_at`](MovingBodies::velocity_at),
//!    [`inverse_mass_at`](MovingBodies::inverse_mass_at). They take `&self`
//!    and answer from the current prediction. They never change it.
//! 3. **Impulses.** [`apply_impulse(body, time, point, impulse)`](MovingBodies::apply_impulse)
//!    tells the body it received `impulse` (N·s, world space) at `point` at
//!    `time`. The body keeps its trajectory on `[0, time]` and
//!    *re-predicts* `[time, dt]` from its pose at `time` and its velocity
//!    there plus `M⁻¹·impulse` (for a 1-DOF rotor about axis `a` through the
//!    pivot `c`: `Δω = ((point − c) × impulse)·a / I`), under the same drive.
//!    Impulses arrive in a non-decreasing `time` order, and after an impulse
//!    at `time` **no query is made for an earlier time** (the solver moves
//!    every ball up to `time` and casts them again). One contact solve may
//!    send several impulses at the same `time`, including negative ones that
//!    take back part of an earlier one (accumulated impulses): they add.
//!    A body that reports `inverse_mass_at == 0` in every direction
//!    (kinematic) must not change its trajectory because of an impulse; it
//!    is still told about it.
//! 4. **End.** [`end_tick(dt)`](MovingBodies::end_tick) for every set, in
//!    slice order, after the last ball event of the step and before the
//!    sensors are updated: the body commits its pose and velocity at `dt`.
//!
//! `begin_tick`/`end_tick` are called even when there is no ball. Nothing
//! in a step allocates if the implementations do not; implementations must
//! not allocate, lock or do I/O either (they run on the 1 kHz sim thread).
//!
//! # How the solver uses a body
//!
//! * **Continuous collision.** Between sub-step boundaries a ball moves in a
//!   straight line; its next impact with a body is found by
//!   [`cast_sphere`](MovingBodies::cast_sphere) (default: conservative
//!   advancement, [`conservative_cast`]), together with the casts against
//!   the static world, guides, captive balls and other balls, and the
//!   earliest event of the tick is processed first. A ball whose path comes
//!   near a body (its [`swept_bounds`](MovingBodies::swept_bounds), padded)
//!   counts [`max_surface_speed`](MovingBodies::max_surface_speed) in its
//!   travel per tick, so it takes more sub-steps.
//! * **Contacts.** At every sub-step boundary of a ball, each body within
//!   20 µm of its surface (by [`closest_point`](MovingBodies::closest_point))
//!   joins the ball's contact list with the static, guide, rail, ball and
//!   captive-ball contacts, and they are all solved together by the same
//!   accumulated-impulse (projected Gauss–Seidel) solve. Penetration is
//!   projected out by moving the ball (bodies are never moved by position
//!   correction).
//! * **The two-body impulse** (PLAN.md §3.3). With `w_n` =
//!   [`inverse_mass_at`](MovingBodies::inverse_mass_at) along the contact
//!   normal, the normal impulse is `J = (u_target − u_n) / (1/m + w_n)` on the
//!   relative normal velocity `u_n = (v_ball − v_body(point))·n`, and `−J·n`
//!   goes to the body. Coulomb friction acts on the slip
//!   `v + ω × r_c − v_body(point)` with the effective mass
//!   `1 / (7/(2m) + w_t)` (`w_t` along the slip direction), clamped to
//!   `μ·J`; its reaction goes to the body too. Restitution and friction come
//!   from the material-pair table for (ball material,
//!   [`material`](MovingBodies::material)). When `w_n` and `w_t` are both 0
//!   the contact is solved exactly as a static surface moving with
//!   `v_body(point)` (including static rest), so a kinematic body at rest
//!   behaves like static geometry.
//! * **Layers.** A body collides with balls whose layer bit is in
//!   [`layer_mask`](MovingBodies::layer_mask).
//!
//! # Guarantees
//!
//! With an empty body slice the solver is bit-identical to
//! [`BallSolver::step`](super::BallSolver::step). With bodies, a step is
//! still deterministic (fixed order: sets in slice order, bodies in index
//! order, balls in handle order) and allocation-free. A ball cannot tunnel
//! through a body as long as `closest_point` is exact and
//! `max_surface_speed` really bounds the surface speed. When a
//! finite-mass body is pushed, the other balls are brought to that instant
//! (so a ball far from the body may then take a slightly different
//! floating-point path than without bodies).

use glam::Vec3;
use pin_core::Material;

/// Gap (metres) within which [`conservative_cast`] reports a contact.
pub const BODY_SKIN: f32 = 1.0e-5;

/// Closing speed (m/s) below which a ball that already touches a body is in
/// resting contact (left to the sub-step boundaries) rather than an impact.
pub const BODY_REST_SPEED: f32 = 1.0e-3;

/// Residual normal speed (m/s) above which the two-body contact solve
/// checks a body's real response against its reported inverse mass.
const GUARD_SPEED: f32 = 1.0e-6;

/// Iterations of [`conservative_cast`] before it reports
/// [`BodyCast::Advance`].
pub const MAX_CAST_ITERATIONS: u32 = 64;

/// The point of a body's surface closest to a query point.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BodyPoint {
    /// Closest surface point, world space, metres.
    pub point: Vec3,
    /// Unit surface normal at `point`, pointing out of the body (toward the
    /// query point when it is outside).
    pub normal: Vec3,
    /// Signed distance from the query point to the surface along `normal`,
    /// metres (negative inside the body).
    pub distance: f32,
}

/// Result of [`MovingBodies::cast_sphere`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum BodyCast {
    /// The sphere touches the body (gap at most [`BODY_SKIN`]) while closing
    /// in, `time` seconds after the start of the cast.
    Hit {
        /// Seconds from the start of the cast (not from the tick).
        time: f32,
        /// Contact point on the body's surface.
        point: Vec3,
        /// Unit normal from the body toward the sphere centre.
        normal: Vec3,
    },
    /// No contact up to `time` (seconds from the start of the cast); the
    /// solver moves the ball there and casts again.
    Advance {
        /// Seconds from the start of the cast.
        time: f32,
    },
}

/// A set of rigid bodies moving during the tick (see the [module
/// documentation](self) for the call order and guarantees).
///
/// All `body` arguments are in `0..body_count()`; all `time` arguments are
/// seconds since the start of the current tick.
pub trait MovingBodies {
    /// Number of bodies in the set (constant during a step).
    fn body_count(&self) -> usize;

    /// Start of a step of `dt` seconds: predict the trajectory over
    /// `[0, dt]` from the committed state and this tick's drive.
    fn begin_tick(&mut self, dt: f32);

    /// End of a step: commit the pose and velocity at `dt`.
    fn end_tick(&mut self, dt: f32);

    /// Layers (bit mask, see [`crate::world::layers`]) the body collides on.
    fn layer_mask(&self, body: usize) -> u8;

    /// Surface material for the material-pair table.
    fn material(&self, body: usize) -> Material;

    /// An axis-aligned box `(min, max)` that contains the body's surface for
    /// the rest of the tick (from the last impulse, or 0, to `dt`). Used
    /// only to skip far balls; an infinite box is always correct.
    fn swept_bounds(&self, body: usize) -> (Vec3, Vec3);

    /// An upper bound (m/s) on the speed of every surface point of the body
    /// for the rest of the tick (0 for a body at rest).
    fn max_surface_speed(&self, body: usize) -> f32;

    /// The closest surface point to `point` at `time`, or `None` when the
    /// body has no surface then (e.g. retracted below the playfield).
    fn closest_point(&self, body: usize, time: f32, point: Vec3) -> Option<BodyPoint>;

    /// Velocity (m/s) at `time` of the body's material point at `point`
    /// (the surface velocity used in the slip and normal terms). It must be
    /// the velocity the body really has then: a body held on an engaged end
    /// stop reports the stop's velocity (0), also at the very instant an
    /// impulse pushed it into the stop, or the contact solve keeps taking
    /// back impulses it thinks moved the body.
    fn velocity_at(&self, body: usize, time: f32, point: Vec3) -> Vec3;

    /// Inverse effective mass (1/kg) of the body at `point` at `time` for an
    /// impulse the body would receive along the unit vector `direction`: the
    /// change of `velocity_at(point)·direction` per unit of such an impulse.
    /// `0` = kinematic (infinite mass) for that push; never negative. For a
    /// free rigid body, `1/m + ((r × d)·I⁻¹(r × d))`; for a 1-DOF rotor about
    /// unit axis `a`, `((r × d)·a)² / I`; for a 1-DOF slider along `a`,
    /// `(d·a)² / m`.
    ///
    /// The sign of `direction` matters for one-sided constraints: a body
    /// resting on an engaged end stop must return 0 for a push *into* the
    /// stop (it cannot move that way) and its free value for a push away from
    /// it. The solver asks with `−n` for the normal impulse (`n` points from
    /// the body to the ball) and with the slip direction for friction.
    fn inverse_mass_at(&self, body: usize, time: f32, point: Vec3, direction: Vec3) -> f32;

    /// The body received `impulse` (N·s) at `point` at `time`: re-predict
    /// `[time, dt]` (see the module documentation, step 3).
    fn apply_impulse(&mut self, body: usize, time: f32, point: Vec3, impulse: Vec3);

    /// The first time, within `window` seconds after `time`, at which a
    /// sphere of `radius` whose centre moves as `center + velocity·s` touches
    /// the body *while closing in* (relative normal speed above
    /// [`BODY_REST_SPEED`]), with the body following its current prediction.
    ///
    /// Must not report a sphere that touches the body at `s = 0` without
    /// closing in (a resting contact: the boundary solve handles it).
    /// The default is [`conservative_cast`]; owners may override it with an
    /// exact cast (e.g. in the body's local frame).
    fn cast_sphere(
        &self,
        body: usize,
        time: f32,
        center: Vec3,
        radius: f32,
        velocity: Vec3,
        window: f32,
    ) -> Option<BodyCast> {
        conservative_cast(self, body, time, center, radius, velocity, window)
    }
}

/// Swept-sphere test against a moving body by conservative advancement:
/// the gap `closest_point(..).distance − radius` cannot shrink faster than
/// `|velocity| + max_surface_speed(body)`, so stepping by gap / that bound
/// never steps over a contact.
///
/// * At `s = 0`, a sphere within [`BODY_SKIN`] of the surface is a hit if it
///   closes in faster than [`BODY_REST_SPEED`], otherwise `None` (resting).
/// * Later, the first point within the skin that closes in is a hit; a
///   sphere grazing the skin without closing in creeps forward by
///   `BODY_SKIN / bound`.
/// * After [`MAX_CAST_ITERATIONS`] steps without a decision it returns
///   [`BodyCast::Advance`] with the time reached.
pub fn conservative_cast<B: MovingBodies + ?Sized>(
    set: &B,
    body: usize,
    time: f32,
    center: Vec3,
    radius: f32,
    velocity: Vec3,
    window: f32,
) -> Option<BodyCast> {
    if window.is_nan() || window <= 0.0 {
        return None;
    }
    let bound = velocity.length() + set.max_surface_speed(body).max(0.0);
    if !bound.is_finite() {
        return None;
    }
    let mut s = 0.0f32;
    for _ in 0..MAX_CAST_ITERATIONS {
        let p = center + velocity * s;
        let cp = set.closest_point(body, time + s, p)?;
        let gap = cp.distance - radius;
        if gap.is_nan() {
            return None;
        }
        let step = if gap <= BODY_SKIN {
            let un = (velocity - set.velocity_at(body, time + s, cp.point)).dot(cp.normal);
            if un < -BODY_REST_SPEED {
                return Some(BodyCast::Hit {
                    time: s,
                    point: cp.point,
                    normal: cp.normal,
                });
            }
            if s == 0.0 {
                return None;
            }
            BODY_SKIN / bound
        } else {
            (gap - 0.5 * BODY_SKIN) / bound
        };
        if step.is_nan() || step <= 0.0 || !step.is_finite() {
            return None;
        }
        s += step;
        if s > window {
            return None;
        }
    }
    Some(BodyCast::Advance { time: s })
}

/// Divergence guard of the two-body normal solve. `dj` was just applied
/// (ball and body) starting from relative normal speed `un`. If the body moved
/// more than its reported inverse mass says (an owner under-reporting it, e.g.
/// at a stop), the relative velocity overshoots the target and the iterated
/// solve would grow by a factor `−w·m` per sweep. The real response is then
/// measured and the impulse corrected with it (a secant step); if the
/// response is unusable the impulse is taken back. Bounded: at most one
/// extra query and impulse per call.
#[allow(clippy::too_many_arguments)]
fn normal_guard(
    ball: &mut super::Ball,
    m: f32,
    set: &mut dyn MovingBodies,
    (body, time, point, n): (usize, f32, Vec3, Vec3),
    un: f32,
    dj: f32,
    target: f32,
    acc: &mut super::contact::ContactImpulse,
) {
    if dj == 0.0 {
        return;
    }
    let un1 = (ball.velocity - set.velocity_at(body, time, point)).dot(n);
    let (r0, r1) = (target - un, target - un1);
    if !(r1.abs() > r0.abs() && r1.abs() > GUARD_SPEED) {
        return;
    }
    let k_obs = (un1 - un) / dj;
    let fixed = if k_obs.is_finite() && k_obs > 0.0 {
        (acc.normal + r1 / k_obs).max(0.0)
    } else {
        acc.normal - dj
    };
    let dj2 = fixed - acc.normal;
    if dj2.is_finite() && dj2 != 0.0 {
        ball.velocity += n * (dj2 / m);
        set.apply_impulse(body, time, point, n * -dj2);
        acc.normal = fixed;
    }
}

/// A usable inverse mass: finite and positive, else 0 (kinematic).
fn usable(w: f32) -> f32 {
    if w.is_finite() && w > 0.0 { w } else { 0.0 }
}

/// One solver step for the contact of a ball with moving body `body` of
/// `set` at `time` (the two-body version of
/// [`solve_surface_contact`](super::contact::solve_surface_contact); see the
/// module documentation). `normal` points from the body toward the ball
/// centre, `point` is the contact point on the body, `acc` the impulses this
/// contact has applied so far. Every impulse change is passed on to the body
/// (opposite sign). Returns the size of the ball's velocity change (m/s) and
/// whether the body may have changed its trajectory (it has a finite mass
/// in a direction it was pushed).
#[allow(clippy::too_many_arguments)]
pub(super) fn solve_body_contact(
    ball: &mut super::Ball,
    sphere: super::contact::SphereBody,
    set: &mut dyn MovingBodies,
    body: usize,
    time: f32,
    point: Vec3,
    normal: Vec3,
    k: super::contact::ContactCoefficients,
    target_normal_speed: f32,
    acc: &mut super::contact::ContactImpulse,
) -> (f32, bool) {
    use super::contact::{clamp_length, solve_surface_contact, tangential};
    let n = normal;
    let r = sphere.radius;
    let m = sphere.mass;
    let rc = n * -r;
    let vs = set.velocity_at(body, time, point);
    // The inverse masses are asked along the impulse change the body
    // actually receives: −n when the ball needs more normal impulse, +n when
    // the accumulated solve takes some back (one-sided stops differ), and
    // the slip direction for friction.
    let un0 = (ball.velocity - vs).dot(n);
    let push = if target_normal_speed - un0 >= 0.0 { -n } else { n };
    let wn = usable(set.inverse_mass_at(body, time, point, push));
    let slip = tangential(ball.velocity + ball.angular_velocity.cross(rc) - vs, n);
    let wt = slip
        .try_normalize()
        .map_or(0.0, |d| usable(set.inverse_mass_at(body, time, point, d)));

    if wn == 0.0 && wt == 0.0 {
        // Kinematic here: exactly a static surface moving with the body.
        let before = *acc;
        let change = solve_surface_contact(ball, sphere, n, vs, k, target_normal_speed, acc);
        let j = n * (acc.normal - before.normal) + (acc.tangent - before.tangent);
        let mut moved = false;
        if j != Vec3::ZERO {
            set.apply_impulse(body, time, point, -j);
            moved = (-j)
                .try_normalize()
                .is_some_and(|d| usable(set.inverse_mass_at(body, time, point, d)) > 0.0);
            // A body that said it cannot move but did breaks the contract;
            // the guard keeps the solve bounded anyway.
            if set.velocity_at(body, time, point) != vs {
                moved = true;
                normal_guard(
                    ball,
                    m,
                    set,
                    (body, time, point, n),
                    un0,
                    acc.normal - before.normal,
                    target_normal_speed,
                    acc,
                );
            }
        }
        return (change, moved);
    }

    let inertia = sphere.inertia();
    let before = (ball.velocity, ball.angular_velocity);
    let un = (ball.velocity - vs).dot(n);
    if un.is_nan() {
        return (0.0, false);
    }
    // Normal impulse on the relative velocity, with both inverse masses.
    let total = (acc.normal + (target_normal_speed - un) / (1.0 / m + wn)).max(0.0);
    let dj = total - acc.normal;
    acc.normal = total;
    if dj != 0.0 {
        ball.velocity += n * (dj / m);
        set.apply_impulse(body, time, point, n * -dj);
        normal_guard(
            ball,
            m,
            set,
            (body, time, point, n),
            un,
            dj,
            target_normal_speed,
            acc,
        );
    }
    let total = acc.normal;

    // Rolling and pivoting resistance act on the ball's spin only.
    let f_max = k.friction.max(0.0) * total;
    let l_max = k.rolling_resistance.max(0.0) * total * r;
    let want = acc.rolling - ball.angular_velocity * inertia;
    let want_n = n * want.dot(n);
    let new_l = clamp_length(want - want_n, l_max) + clamp_length(want_n, l_max);
    ball.angular_velocity += (new_l - acc.rolling) / inertia;
    acc.rolling = new_l;

    // Coulomb friction on the slip against the body's (new) surface
    // velocity; effective mass 1 / (7/(2m) + w_t).
    let vs = set.velocity_at(body, time, point);
    let ut = tangential(ball.velocity + ball.angular_velocity.cross(rc) - vs, n);
    let wt = ut
        .try_normalize()
        .map_or(0.0, |d| usable(set.inverse_mass_at(body, time, point, d)));
    let mut new_t = clamp_length(tangential(acc.tangent, n) - ut / (3.5 / m + wt), f_max);
    // The clamp may turn the change around (friction taken back): ask the
    // body along the change it really receives.
    if let Some(d) = (acc.tangent - new_t).try_normalize()
        && d.dot(ut) < 0.0
    {
        let wt = usable(set.inverse_mass_at(body, time, point, d));
        new_t = clamp_length(tangential(acc.tangent, n) - ut / (3.5 / m + wt), f_max);
    }
    let j = new_t - acc.tangent;
    if j != Vec3::ZERO && j.is_finite() {
        ball.velocity += j / m;
        ball.angular_velocity += rc.cross(j) / inertia;
        set.apply_impulse(body, time, point, -j);
        acc.tangent = new_t;
    }
    let change =
        (ball.velocity - before.0).length() + (ball.angular_velocity - before.1).length() * r;
    (change, true)
}
