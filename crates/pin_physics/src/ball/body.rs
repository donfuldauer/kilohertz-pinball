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
//!    this tick, keeping [owner obligation 3](#owner-obligations) (speculative
//!    stops): e.g. `ω₁ = ω₀ + α·dt`, then toward a stop
//!    `ω₁ ← min(ω₁, max((θ_stop − θ₀)/dt, ω₀))`, then `θ(t) = θ₀ + ω₁·t`
//!    clamped at the stop. The prediction must be a
//!    deterministic function of `t`, continuous in position (velocities may
//!    jump, e.g. at an end stop or an impulse).
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
//! # Owner obligations
//!
//! A stop is *engaged* at `time` when the body is at the stop and not moving
//! away from it.
//!
//! 1. **One-sided inverse mass.** [`inverse_mass_at`](MovingBodies::inverse_mass_at)
//!    answers for the direction of the impulse the *body* would receive; it
//!    may be asked with `−n` (the ball needs more normal impulse) or `+n`
//!    (the accumulated solve takes some back), and along the slip or the
//!    friction change. On an engaged stop it is 0 for a push into the stop
//!    and the free value for a push away from it.
//! 2. **Real velocity.** [`velocity_at`](MovingBodies::velocity_at) is the
//!    velocity the body really has: 0 on an engaged stop, also at the very
//!    instant an impulse drove the body into it. At a time where the
//!    velocity jumps (an impulse, reaching a stop) it returns the velocity
//!    *after* the jump.
//! 3. **Speculative stops.** This tick's *drive increment* must not carry
//!    the body past a stop within the tick: at `begin_tick`, toward a stop,
//!    `v₁ ← min(v₁, max(gap/dt, v₀))` (`v₀` the velocity before the drive,
//!    `gap` the distance to the stop). Momentum the body already holds may
//!    carry it to the stop within the tick (e.g. an auto-plunger arriving
//!    fast): the position is then clamped at the stop and `velocity_at`
//!    reports 0 from the instant it arrives (obligation 2). An impulse is
//!    momentum too, so re-prediction after it needs no extra limit. A body
//!    held on its stop thus predicts 0. Predicting the full `α·dt` there and
//!    clamping the position reports a surface speed at `t = 0` that the stop
//!    cancels microseconds later; a ball cradled on a held flipper is then
//!    hit by that phantom speed every tick and never rests.
//!
//! **Over-reporting is allowed.** An `inverse_mass_at` larger than the
//! body's real mobility (e.g. a rubber-loss term, Subphase 2.3) makes the
//! solver ask for a smaller impulse per sweep than the body's real response
//! needs; the accumulated solve then converges from below to the same final
//! impulse (more sweeps, never an overshoot), and a one-shot impact delivers
//! less than the two-body impulse, i.e. a softer, lossier hit. It is a valid
//! way to model compliance or losses.
//!
//! # What the solver guarantees for an owner that answers wrongly
//!
//! For *any* answers (under-reported, NaN, infinite or negative inverse
//! masses, a response opposite to the impulse, stale or NaN velocities):
//!
//! * **No overflow.** Every ball stays finite.
//! * **Speed bound.** A body contact never leaves a ball faster than
//!   [`BODY_SPEED_LIMIT`] (100 m/s) or than it was before the contact, if
//!   that is more; spin likewise (`BODY_SPEED_LIMIT / r`).
//! * **Relative bound, per solve.** The accumulated normal impulse of a
//!   body contact is capped at the impulse that would bring the ball to the
//!   target relative to the body *as if this contact had applied no
//!   impulse*: the contact's own impulse is removed from the ball and the
//!   body's measured response to it is removed from the body's velocity
//!   (as against an immovable body). Pushes from the ball's other contacts,
//!   and from other contacts on the body (another ball knocking it into this
//!   one), raise the cap; the body's answers to this contact's own impulses
//!   do not. So within one solve the ball cannot leave faster than `(1 + e) ×`
//!   its approach to the body's surface velocity; across solves, the body's
//!   own surface speed (the owner's business) is the reference. Measured over a sweep of
//!   wrong answers: the ball never exceeded `2 × max(incoming speed, body
//!   surface speed) + 2 m/s`.
//! * **No amplification.** After each normal impulse the body's real response
//!   is measured; if it moved more than it reported, the solve uses the
//!   observed response from then on and *reduces* the impulse to what that
//!   response needs. A correction never increases an impulse.
//! * **Bounded work.** At most [`MAX_BODY_SOLVES_PER_BALL`] body-contact
//!   solve steps per ball and tick: a ball that spends them is neither
//!   solved nor cast against bodies until the next tick (its penetration is
//!   projected out at its next sub-step boundary), while every other ball is
//!   unaffected, so a starving pinch cannot let another ball through a
//!   flipper. A global safety cap ([`MAX_BODY_SOLVES_PER_TICK`]) bounds the
//!   tick. All within the solver's existing per-tick event budget. Measured
//!   worst step (noisy host): 3–5 ms, for an owner whose response is opposite
//!   to every impulse and whose bat reached 2.5e7 m/s; owners that keep the
//!   contract stay under 1 ms in every pinch tested (an honest 20 m/s kicker
//!   pinch uses at most ~760 solve steps per ball).
//!
//! None of this makes a wrong owner physical: energy is still created when a
//! body's real response differs from what it reports.
//!
//! # Notes for owners
//!
//! * **Drive energy.** Front-loading `α·dt` at `begin_tick` (semi-implicit
//!   Euler) means the drive acts on the whole tick from its start; a driven
//!   body adds its drive work to the system, as it should, at most
//!   `τ·Δθ` per tick.
//! * **The last tick before a stop.** Obligation 3 limits only the drive
//!   increment: a body that would overshoot a stop *because of this tick's
//!   drive* is slowed so that it arrives at `dt` (if it held less speed than
//!   that), and a ball hit in that tick sees the reduced speed; a body that
//!   already holds enough momentum keeps it, reaches the stop inside the
//!   tick and stops there.
//! * **Pushed toward a nearby stop.** A body pushed toward a stop it will
//!   reach within the tick reports its free inverse mass, but its velocity
//!   change is then limited by obligation 3. The solve under-shoots and
//!   converges over the following sweeps; this is harmless.
//! * **Own restitution.** [`MovingBodies::restitution`] (default `None`)
//!   replaces the pair table's restitution for impacts on the body.
//! * **One-shot impacts are not exactly `e·approach`.** The friction
//!   reaction of an impact is applied to the body after the normal impulse
//!   and can change the body's normal velocity at the contact (a rotor hit
//!   off its line of centres), so the post-impact relative normal speed
//!   differs slightly from `e ×` the approach speed.
//! * **Pinned and cradled balls.** A ball squeezed between a body and other
//!   surfaces often uses up its 12 events per sub-step; it then waits (does
//!   not move, is not cast against bodies) until its next sub-step boundary,
//!   where all its contacts are solved together and penetration is projected
//!   out. Owners therefore see several small impulses per tick, or none in
//!   that window: never count on one impulse per contact per tick. A ball
//!   held only by static surfaces, other balls and *kinematic* bodies, in a
//!   real pinch (a kinematic body contact opposed by another contact while
//!   the body closes in, the ball runs into the opposing surface, or the ball
//!   is already 50 µm deep), whose contact solve stops converging stops
//!   iterating after 8 passes instead of 64, so a kicker cannot stall the
//!   tick. Stacks resting on (or riding) kinematic bodies are not pinches and
//!   iterate as usual.
//! * **Several balls on one body.** When a ball's contact with a body is
//!   re-solved between sub-step boundaries (a continuing contact), every
//!   other ball currently touching the same body is brought to that instant
//!   and solved in the same accumulated-impulse loop, so the push one ball
//!   gives the body reaches the others at once (a ball resting on a bat that
//!   another ball knocks is pushed, not passed through). Inside one
//!   sub-step boundary solve this coupling already exists.
//! * **Known limit: sub-steps are planned at tick start.** A ball's number of
//!   sub-steps is fixed at the start of the tick from its own speed and the
//!   bodies near it. A resting ball that a body is knocked into mid-tick
//!   keeps its few sub-steps; its contact is then held by the joint
//!   re-solve and by casts (a ball separating faster than
//!   [`SEPARATING_SPEED`] is cast again, so a body that turns back into it,
//!   e.g. pushed back onto its stop, is found). Measured: under 0.4 mm at a
//!   30 m/s knock, under 0.03 mm in a flip sweep.
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
//!   [`inverse_mass_at`](MovingBodies::inverse_mass_at) along the normal
//!   impulse change the body receives (`−n`, or `+n` when taken back), the normal impulse is `J = (u_target − u_n) / (1/m + w_n)` on the
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

/// Relative normal speed (m/s) by which a friction impulse may separate a
/// body contact beyond what its normal solve set before the friction change
/// is cut back (see `solve_body_contact`).
const FRICTION_SEPARATION_TOLERANCE: f32 = 1.0e-5;

/// Gap (metres) within which [`conservative_cast`] reports a contact.
pub const BODY_SKIN: f32 = 1.0e-5;

/// Closing speed (m/s) below which a ball that already touches a body is in
/// resting contact (left to the sub-step boundaries) rather than an impact.
pub const BODY_REST_SPEED: f32 = 1.0e-3;

/// A body contact never leaves a ball faster than this (m/s), or than the
/// ball was before the contact if that is more; spin likewise, scaled by
/// the radius. Never reached with an owner that keeps the contract (a flip
/// at a 20 m/s bat tip launches at ≤ 40 m/s); it bounds what a body that
/// answers wrongly can do.
pub const BODY_SPEED_LIMIT: f32 = 100.0;

/// Body-contact solve steps one ball may use per tick. Once it has spent
/// them, that ball's body contacts apply no further impulse and it is not
/// cast against bodies until the next tick (penetration is then projected
/// out at its next sub-step boundary); every other ball is unaffected, so
/// one pinched group cannot let another ball through a flipper. Bounds the
/// work a misbehaving body can cause per ball.
pub const MAX_BODY_SOLVES_PER_BALL: u32 = 2048;

/// Global safety cap on body-contact solve steps per tick (all balls); once
/// spent, no ball is solved or cast against bodies until the next tick. Far
/// above what the per-ball budgets of the balls touching bodies normally
/// use.
pub const MAX_BODY_SOLVES_PER_TICK: u32 = 16_384;

/// Relative normal speed (m/s) above which a sphere that touches a body at
/// the start of a cast counts as separating (the cast keeps marching)
/// rather than resting (the cast returns `None`).
pub const SEPARATING_SPEED: f32 = super::IMPACT_REPORT_SPEED;

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
    /// `[0, dt]` from the committed state and this tick's drive; the drive
    /// increment must not carry the body past a stop within the tick (owner
    /// obligation 3).
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
    /// back impulses it thinks moved the body. Where the velocity jumps it
    /// returns the value after the jump.
    fn velocity_at(&self, body: usize, time: f32, point: Vec3) -> Vec3;

    /// Inverse effective mass (1/kg) of the body at `point` at `time` for an
    /// impulse the body would receive along the unit vector `direction`: the
    /// change of `velocity_at(point)·direction` per unit of such an impulse.
    /// `0` = kinematic (infinite mass) for that push; never negative. For a
    /// free rigid body, `1/m + ((r × d)·I⁻¹(r × d))`; for a 1-DOF rotor about
    /// unit axis `a`, `((r × d)·a)² / I`; for a 1-DOF slider along `a`,
    /// `(d·a)² / m`.
    ///
    /// The sign of `direction` matters for one-sided constraints: a body on
    /// an engaged end stop (at the stop, not moving away) must return 0 for a
    /// push *into* the stop and its free value for a push away from it. The
    /// solver asks along the impulse change the body actually receives: `−n`
    /// when the normal impulse grows, `+n` when it is taken back (`n` points
    /// from the body to the ball), the slip direction for friction.
    fn inverse_mass_at(&self, body: usize, time: f32, point: Vec3, direction: Vec3) -> f32;

    /// The body received `impulse` (N·s) at `point` at `time`: re-predict
    /// `[time, dt]` (see the module documentation, step 3). The new velocity
    /// is momentum: it may carry the body to a stop, where the position is
    /// clamped and the velocity is 0 from that instant.
    fn apply_impulse(&mut self, body: usize, time: f32, point: Vec3, impulse: Vec3);

    /// Coefficient of restitution for an impact of a ball on `body` at
    /// `point` at `time`, approached at `approach_speed` (m/s, relative
    /// normal speed), or `None` to use the material-pair table (the
    /// default). `Some(e)` is clamped to `[0, 1]` (NaN = pair table) and then
    /// goes through the same rules as every impact (fade-in between
    /// [`IMPACT_REPORT_SPEED`](super::IMPACT_REPORT_SPEED) and twice that,
    /// zero for a continuing contact). Lets an owner model its own rubber
    /// (e.g. `FlipperConfig::rubber_restitution`) or a speed-dependent
    /// restitution.
    fn restitution(&self, body: usize, time: f32, point: Vec3, approach_speed: f32) -> Option<f32> {
        let _ = (body, time, point, approach_speed);
        None
    }

    /// A reported impact ([`BallEvent::Impact`](super::BallEvent::Impact))
    /// of a ball on `body` at `point` at `time`: called once per event, just
    /// before it is pushed, after its impulses were applied. `impulse` is the
    /// total impulse (N·s) the body received from this impact and
    /// `approach_speed` the event's `normal_speed`. Lets an owner judge each
    /// impact by its own direction and speed (default: nothing).
    fn impact(&mut self, body: usize, time: f32, point: Vec3, impulse: Vec3, approach_speed: f32) {
        let _ = (body, time, point, impulse, approach_speed);
    }

    /// The first time, within `window` seconds after `time`, at which a
    /// sphere of `radius` whose centre moves as `center + velocity·s` touches
    /// the body *while closing in* (relative normal speed above
    /// [`BODY_REST_SPEED`]), with the body following its current prediction.
    ///
    /// Must not report a sphere that touches the body at `s = 0` without
    /// closing in (a resting contact: the boundary solve handles it), but
    /// must find a later re-closing of a sphere that separates at `s = 0`.
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
///   closes in faster than [`BODY_REST_SPEED`]; if it separates faster than
///   [`SEPARATING_SPEED`] the cast keeps marching (the body may turn back
///   into it, e.g. pushed back onto its stop); otherwise `None` (resting).
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
            // Touching at the start without closing in: a resting contact
            // (left to the sub-step boundaries), unless the sphere is
            // clearly separating; then the body may still turn back into it
            // within the window (a bat pushed back onto its stop), so keep
            // marching.
            if s == 0.0 && un <= SEPARATING_SPEED {
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

/// A usable inverse mass: finite and positive, else 0 (kinematic).
fn usable(w: f32) -> f32 {
    if w.is_finite() && w > 0.0 { w } else { 0.0 }
}

/// What the solver learns about one body contact during one solve (a
/// sub-step boundary solve, a mid-sub-step re-solve or a one-shot impact).
#[derive(Copy, Clone, Debug)]
pub(super) struct BodyContactState {
    /// Largest relative normal response per unit impulse (1/kg) observed so
    /// far, `1/m + w_observed`; 0 before the first observation. The solve
    /// uses `max(1/m + w_reported, k)`, so a body that moves more than it
    /// reports is never pushed past the target again (no overshoot, no
    /// growth from sweep to sweep).
    pub k: f32,
    /// The normal impulse of this contact was taken back to zero after it
    /// had been positive (Subphase 2.8): from then on a friction change that
    /// would separate the contact is cut back (see `solve_body_contact`).
    pub cycling: bool,
    /// Upper bound of the accumulated normal impulse (N·s): the largest
    /// `m · (u_target − u_free)` seen, where `u_free = (v_ball − (v_body −
    /// Δv_own))·n − J_acc/m` is the relative normal velocity the ball would
    /// have if this contact had applied no impulse at all: its own impulse is
    /// removed from the ball, and the body's measured response to it
    /// (`Δv_own`) from the body. A body that keeps the contract never needs
    /// more (a two-body impulse is at most the impulse against an immovable
    /// body); pushes from the ball's other contacts, and from other contacts
    /// on the *body* (another ball knocking it into this one), raise it;
    /// whatever the body answers to this contact's own impulses does not.
    pub cap: f32,
    /// The body's measured velocity change at the contact point caused by
    /// this contact's own impulses so far (`Δv_own`).
    pub own_v: Vec3,
}

impl BodyContactState {
    pub(super) const NEW: Self = Self {
        k: 0.0,
        cycling: false,
        cap: 0.0,
        own_v: Vec3::ZERO,
    };
}

/// One solver step for the contact of a ball with moving body `body` of
/// `set` at `time` (the two-body version of
/// [`solve_surface_contact`](super::contact::solve_surface_contact); see the
/// module documentation). `normal` points from the body toward the ball
/// centre, `point` is the contact point on the body, `acc` the impulses this
/// contact has applied so far and `st` what the solve has learnt about the
/// body. Every impulse change is passed on to the body (opposite sign).
/// Returns the size of the ball's velocity change (m/s) and whether the body
/// may have changed its trajectory.
///
/// Robustness against a body that answers wrongly (module docs, "Owner
/// obligations"): the response is measured after every normal impulse;
/// if the body moved more than reported, the effective mass is raised to the
/// observed one and the impulse is *reduced* to what the observed response
/// needs (a correction never increases it); the accumulated normal impulse
/// is capped (`st.cap`); and the ball's speed and spin are limited
/// ([`BODY_SPEED_LIMIT`]).
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
    st: &mut BodyContactState,
) -> (f32, bool) {
    use super::contact::{clamp_length, solve_surface_contact, tangential};
    let n = normal;
    let r = sphere.radius;
    let m = sphere.mass;
    let rc = n * -r;
    let before = (ball.velocity, ball.angular_velocity);
    let vs = set.velocity_at(body, time, point);
    let un = (ball.velocity - vs).dot(n);
    if !un.is_finite() || !vs.is_finite() {
        return (0.0, false);
    }
    let u_free = (ball.velocity - (vs - st.own_v)).dot(n) - acc.normal / m;
    let need = m * (target_normal_speed - u_free);
    if need > st.cap {
        st.cap = need;
    }
    // The inverse masses are asked along the impulse change the body
    // actually receives: −n when the ball needs more normal impulse, +n when
    // the accumulated solve takes some back (one-sided stops differ), and
    // the slip direction for friction.
    let push = if target_normal_speed - un >= 0.0 {
        -n
    } else {
        n
    };
    let wn = usable(set.inverse_mass_at(body, time, point, push));
    let slip = tangential(ball.velocity + ball.angular_velocity.cross(rc) - vs, n);
    let wt = slip
        .try_normalize()
        .map_or(0.0, |d| usable(set.inverse_mass_at(body, time, point, d)));
    let k_ball = 1.0 / m;
    let mut moved = false;

    if wn == 0.0 && wt == 0.0 && st.k <= k_ball {
        // Kinematic here: exactly a static surface moving with the body.
        let start = *acc;
        solve_surface_contact(ball, sphere, n, vs, k, target_normal_speed, acc);
        if acc.normal > st.cap {
            ball.velocity -= n * ((acc.normal - st.cap) / m);
            acc.normal = st.cap;
            acc.tangent = clamp_length(acc.tangent, k.friction.max(0.0) * st.cap);
        }
        let j = n * (acc.normal - start.normal) + (acc.tangent - start.tangent);
        if j != Vec3::ZERO {
            set.apply_impulse(body, time, point, -j);
            moved = (-j)
                .try_normalize()
                .is_some_and(|d| usable(set.inverse_mass_at(body, time, point, d)) > 0.0);
            // A body that said it cannot move but did: learn its response
            // and take back what overshot.
            observe(
                ball,
                m,
                set,
                (body, time, point, n),
                (un, start.normal, k_ball),
                target_normal_speed,
                acc,
                st,
            );
            moved |= st.k > k_ball;
            // A body that reports no mobility may still change its surface
            // velocity in response (e.g. against the impulse, which `observe`
            // does not learn from): whatever it did is this contact's own
            // doing and must count in `st.own_v`, or the cap would grow from
            // the contact's own impulses.
            if !moved {
                let own = set.velocity_at(body, time, point) - vs;
                moved = own != Vec3::ZERO && own.is_finite();
            }
        }
    } else {
        let inertia = sphere.inertia();
        // Normal impulse on the relative velocity, with both inverse masses
        // (or the larger response seen).
        let k_n = (k_ball + wn).max(st.k);
        let start = acc.normal;
        let total = (start + (target_normal_speed - un) / k_n).clamp(0.0, st.cap);
        let dj = total - start;
        acc.normal = total;
        if dj != 0.0 {
            ball.velocity += n * (dj / m);
            set.apply_impulse(body, time, point, n * -dj);
            observe(
                ball,
                m,
                set,
                (body, time, point, n),
                (un, start, k_n),
                target_normal_speed,
                acc,
                st,
            );
        }
        let total = acc.normal;
        if start > 0.0 && total <= 0.0 {
            st.cycling = true;
        }

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
        // The clamp may turn the change around (friction taken back): ask
        // the body along the change it really receives.
        if let Some(d) = (acc.tangent - new_t).try_normalize()
            && d.dot(ut) < 0.0
        {
            let wt = usable(set.inverse_mass_at(body, time, point, d));
            new_t = clamp_length(tangential(acc.tangent, n) - ut / (3.5 / m + wt), f_max);
        }
        let j = new_t - acc.tangent;
        if j != Vec3::ZERO && j.is_finite() {
            let un_pre = (ball.velocity - vs).dot(n);
            ball.velocity += j / m;
            ball.angular_velocity += rc.cross(j) / inertia;
            set.apply_impulse(body, time, point, -j);
            acc.tangent = new_t;
            // Friction must not undo the normal impulse it rests on
            // (Subphase 2.8). On a light body the friction impulse can turn
            // the contact apart (it slows the body's surface along the
            // normal too); the next pass then takes the normal impulse back,
            // the friction with it, and starts over: a two-pass cycle that
            // never converges, so the cast keeps reporting the same hit
            // until the ball's event budget runs out and the body sweeps
            // into it. Once a contact has shown that (its normal impulse
            // went back to zero), keep only the part of a friction change
            // that leaves the relative normal speed at (or below) what the
            // normal solve set. Contacts that never drop to zero are solved
            // exactly as before.
            let ceiling = target_normal_speed.max(un_pre);
            let un_post = (ball.velocity - set.velocity_at(body, time, point)).dot(n);
            let rise = un_post - un_pre;
            if st.cycling
                && acc.normal > 0.0
                && rise.is_finite()
                && rise > 0.0
                && un_post > ceiling + FRICTION_SEPARATION_TOLERANCE
            {
                let keep = ((ceiling - un_pre) / rise).clamp(0.0, 1.0);
                let back = j * (1.0 - keep);
                if back != Vec3::ZERO && back.is_finite() {
                    ball.velocity -= back / m;
                    ball.angular_velocity -= rc.cross(back) / inertia;
                    set.apply_impulse(body, time, point, back);
                    acc.tangent -= back;
                    // After a cut the accumulated friction is a blend of
                    // passes: if the normal impulse fell in this pass, it
                    // may sit slightly outside the friction cone (|J_t| >
                    // μ·J_n) until the next pass clamps it again.
                }
            }
        }
        moved = true;
    }

    // What this call's own impulses did to the body (for the cap).
    if moved || st.k > k_ball {
        let vs_end = set.velocity_at(body, time, point);
        let own = vs_end - vs;
        if own.is_finite() {
            st.own_v += own;
        }
    }

    // Backstop: a body contact never makes a ball faster (or spin faster)
    // than the limit, unless it already was.
    let limit = BODY_SPEED_LIMIT.max(before.0.length());
    let speed = ball.velocity.length();
    if speed.is_nan() || speed > limit {
        ball.velocity = if speed.is_finite() {
            ball.velocity * (limit / speed)
        } else {
            before.0
        };
    }
    let spin_limit = (BODY_SPEED_LIMIT / r).max(before.1.length());
    let spin = ball.angular_velocity.length();
    if spin.is_nan() || spin > spin_limit {
        ball.angular_velocity = if spin.is_finite() {
            ball.angular_velocity * (spin_limit / spin)
        } else {
            before.1
        };
    }
    let change =
        (ball.velocity - before.0).length() + (ball.angular_velocity - before.1).length() * r;
    (change, moved)
}

/// After a normal impulse change (`acc.normal` now, `start` before, relative
/// normal speed `un` before, assumed response `assumed` = `1/m + w`): measure
/// the body's real response. If the relative normal velocity moved more per
/// unit impulse than the solve assumed (the body moved more than it
/// reported), record the observed
/// response in `st.k` and reduce the impulse to what that response needs.
/// The correction only ever brings the impulse closer to `start`, so it
/// cannot amplify anything.
#[allow(clippy::too_many_arguments)]
fn observe(
    ball: &mut super::Ball,
    m: f32,
    set: &mut dyn MovingBodies,
    (body, time, point, n): (usize, f32, Vec3, Vec3),
    (un, start, assumed): (f32, f32, f32),
    target: f32,
    acc: &mut super::contact::ContactImpulse,
    st: &mut BodyContactState,
) {
    let dj = acc.normal - start;
    if dj == 0.0 {
        return;
    }
    let vs1 = set.velocity_at(body, time, point);
    let un1 = (ball.velocity - vs1).dot(n);
    let k_obs = (un1 - un) / dj;
    // Compared with the response the solve assumed for this impulse (the
    // reported inverse mass, or a larger one seen before); 0.1 % tolerance
    // so that rounding of an honest response does not count.
    if !(k_obs.is_finite() && k_obs > assumed.max(st.k) * 1.001) {
        return;
    }
    st.k = k_obs;
    let wanted = (start + (target - un) / k_obs).clamp(0.0, st.cap);
    // Only move back toward `start` (never past the impulse already given).
    let fixed = if dj > 0.0 {
        wanted.clamp(start, acc.normal)
    } else {
        wanted.clamp(acc.normal, start)
    };
    let dj2 = fixed - acc.normal;
    if dj2 != 0.0 && dj2.is_finite() {
        ball.velocity += n * (dj2 / m);
        set.apply_impulse(body, time, point, n * -dj2);
        acc.normal = fixed;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ball::contact::{ContactCoefficients, ContactImpulse, SphereBody};
    use pin_core::ids::ProfileId;

    /// Reports zero mobility but moves against every impulse it receives.
    struct Contrary {
        v: Vec3,
    }

    impl MovingBodies for Contrary {
        fn body_count(&self) -> usize {
            1
        }
        fn begin_tick(&mut self, _dt: f32) {}
        fn end_tick(&mut self, _dt: f32) {}
        fn layer_mask(&self, _body: usize) -> u8 {
            0xff
        }
        fn material(&self, _body: usize) -> Material {
            Material::Steel
        }
        fn swept_bounds(&self, _body: usize) -> (Vec3, Vec3) {
            (Vec3::splat(-1.0), Vec3::splat(1.0))
        }
        fn max_surface_speed(&self, _body: usize) -> f32 {
            self.v.length()
        }
        fn closest_point(&self, _body: usize, _time: f32, _point: Vec3) -> Option<BodyPoint> {
            None
        }
        fn velocity_at(&self, _body: usize, _time: f32, _point: Vec3) -> Vec3 {
            self.v
        }
        fn inverse_mass_at(&self, _b: usize, _t: f32, _p: Vec3, _d: Vec3) -> f32 {
            0.0
        }
        fn apply_impulse(&mut self, _body: usize, _time: f32, _point: Vec3, impulse: Vec3) {
            // Opposite to the impulse, as if 0.08 kg.
            self.v -= impulse / 0.08;
        }
    }

    #[test]
    fn zero_mobility_body_moving_against_impulse_cannot_inflate_cap() {
        let sphere = SphereBody {
            radius: 0.0135,
            mass: 0.08,
        };
        let k = ContactCoefficients {
            restitution: 0.0,
            friction: 0.0,
            rolling_resistance: 0.0,
        };
        let mut ball = crate::ball::Ball::new(ProfileId(0), Vec3::ZERO);
        ball.velocity = Vec3::new(0.0, 0.0, -1.0);
        let mut set = Contrary { v: Vec3::ZERO };
        let mut acc = ContactImpulse::default();
        let mut st = BodyContactState::NEW;
        for _ in 0..3 {
            solve_body_contact(
                &mut ball,
                sphere,
                &mut set,
                0,
                0.0,
                Vec3::ZERO,
                Vec3::Z,
                k,
                0.0,
                &mut acc,
                &mut st,
            );
        }
        // m·(target − u_free) = 0.08 N·s against an immovable body.
        assert!(st.cap <= 0.08 * 1.001, "cap grew to {}", st.cap);
        assert!(acc.normal <= 0.08 * 1.001, "impulse {}", acc.normal);
    }
}
