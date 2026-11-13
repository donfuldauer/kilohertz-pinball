//! CCD, spin, friction, magnets and Newton balls.
//!
//! Owner: Subphase 2.2. Stub created by Subphase 1.1.
//!
//! The ball dynamics core (PLAN.md §3.2), SI units and `f32` throughout:
//!
//! * [`BallSolver`]: up to [`MAX_BALLS`] free balls; one
//!   [`step`](BallSolver::step) per 1 ms tick with swept-sphere continuous
//!   collision detection against the [`crate::world`] geometry, guide walls,
//!   wireform rails, captive balls and other balls; spin, Coulomb friction
//!   with the sliding → rolling transition, rolling resistance, and
//!   restitution/friction from the material-pair table.
//! * [`Magnet`] and [`Sensor`]: electromagnets
//!   (`F = c·P·K_coil·d̂ / max(d³, d_min³)`, exactly zero for a ball with
//!   zero magnetic coupling) and eddy / opto sensors.
//! * [`NewtonChain`]: captive balls on a 1D spline.
//! * [`Dispenser`]: FIFO of ball profiles with an exit pose.
//! * [`ExternalForce`]: the hook list Subphase 2.7 (Magnus, remanence)
//!   plugs into, with a [`ForceContext`] (magnets, sub-step time) and ball
//!   add/remove notifications.
//! * [`MovingBodies`] ([`body`]): rigid bodies that move during the tick
//!   (flippers, Subphase 2.3; kickers, plunger tip, spinner flaps, Subphase
//!   2.4), lent to [`BallSolver::step_with_bodies`]: continuous collision,
//!   resting contacts and the two-body impulse.
//!   [`SurfaceVelocity`] gives moving surfaces of the static geometry.
//!
//! `step` never allocates and is deterministic: fixed iteration order, no
//! fused multiply-add, transcendental functions from `libm`.

pub mod body;
pub mod contact;
mod dispenser;
mod newton;
mod solver;
mod types;

pub use body::{
    BODY_REST_SPEED, BODY_SKIN, BODY_SPEED_LIMIT, BodyCast, BodyPoint, MAX_BODY_SOLVES_PER_TICK,
    MAX_CAST_ITERATIONS, MovingBodies, conservative_cast,
};
pub use dispenser::{DISPENSER_CAPACITY, Dispenser};
pub use newton::{MAX_CHAIN_BALLS, NewtonChain, NewtonError};
pub use solver::BallSolver;
pub use types::{
    Ball, BallEvent, BallEvents, BallHandle, BallSolverConfig, ExternalForce, ForceContext,
    Geometry, IMPACT_REPORT_SPEED, ImpactTarget, MAX_BALL_EVENTS, MAX_BALLS, Magnet, MagnetState,
    Sensor, SensorKind, SurfaceVelocity,
};
