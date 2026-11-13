//! External-force hooks: Magnus effect and magnetic remanence (PLAN.md §3.2).
//!
//! Owner: Subphase 2.7.
//!
//! Both hooks plug into the ball solver through the Subphase 2.2-S
//! force-context seam: they implement [`crate::ball::ExternalForce::force_in`],
//! which the solver calls once per ball sub-step (`dt` = the sub-step `h`),
//! in ball-handle order, after gravity and the solver's own magnet force,
//! with a [`crate::ball::ForceContext`] (every magnet with its power and
//! remanence τ, and the sub-step time); the returned force (N) is added as
//! `F·h/m`. Per-ball state follows the balls through
//! [`ExternalForce::ball_added`](crate::ball::ExternalForce::ball_added) /
//! [`ball_removed`](crate::ball::ExternalForce::ball_removed) (handles are
//! reused, so state is reset on add). Hooks are registered at setup time
//! with [`crate::ball::BallSolver::add_external_force`] or all at once with
//! [`register_all`].
//!
//! * [`Magnus`]: `F = S_m (ω × v)`, `S_m = magnus_coeff`, integrated as an
//!   exact rotation of `v` over the sub-step, so it does no work.
//! * [`Remanence`]: per-ball magnetisation `M ∈ [0, 1]` that relaxes toward
//!   a magnet's power `P` in its range and decays with `e^(−Δt/τ)` outside,
//!   and the residual pull `c·ρ·M·K_coil·d̂ / max(d³, d_min³)`; read `M`
//!   with [`magnetisation`] through a clone of the hook (shared, lock-free
//!   cells).
//!
//! A hook with nothing to add returns `-0.0` in every component (the
//! identity of float addition), so with `magnus_coeff = 0` and
//! `remanence = 0` the solver is bit-identical to one without the hooks.
//! Nothing here allocates after construction; transcendental functions
//! come from `libm`, and there is no fused multiply-add.

mod magnus;
mod remanence;

pub use magnus::{Magnus, magnus_force, magnus_step_force};
pub use remanence::{Remanence, magnetisation_step, remanence_decay, residual_force};

use crate::ball::{BallHandle, BallSolver};

/// Magnetisation `M ∈ [0, 1]` of the ball in slot `handle` as seen by
/// `remanence` — the hook registered with the solver or any clone of it
/// (clones share their magnetisation state; see [`Remanence`]). Zero for an
/// empty slot.
#[must_use]
pub fn magnetisation(remanence: &Remanence, handle: BallHandle) -> f32 {
    remanence.magnetisation(handle)
}

/// Registers both 2.7 hooks with `solver`, in this order: [`Magnus`]
/// first, then [`Remanence`] (their forces are summed in that order after
/// any hook registered earlier). Returns a clone of the registered
/// [`Remanence`] for reading [`magnetisation`]. Setup-time only (it
/// allocates the boxes and the shared cells).
pub fn register_all(solver: &mut BallSolver) -> Remanence {
    solver.add_external_force(Box::new(Magnus::new()));
    let remanence = Remanence::new();
    solver.add_external_force(Box::new(remanence.clone()));
    remanence
}
