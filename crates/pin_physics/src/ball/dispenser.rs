//! Ball dispensers (troughs, locks): a FIFO of ball profiles that releases
//! balls into the solver at a fixed exit.

use arrayvec::ArrayVec;
use glam::Vec3;
use pin_core::ids::ProfileId;

use super::solver::BallSolver;
use super::types::{Ball, BallHandle};

/// Capacity of a [`Dispenser`].
pub const DISPENSER_CAPACITY: usize = 16;

/// A first-in-first-out queue of ball profiles with an exit pose.
#[derive(Clone, Debug, PartialEq)]
pub struct Dispenser {
    queue: ArrayVec<ProfileId, DISPENSER_CAPACITY>,
    /// Where dispensed balls appear (ball centre), metres.
    pub exit_position: Vec3,
    /// Velocity of a dispensed ball, m/s.
    pub exit_velocity: Vec3,
    /// Collision layer index of a dispensed ball.
    pub exit_layer: u8,
}

impl Dispenser {
    /// An empty dispenser that releases balls at `exit_position` with
    /// `exit_velocity` on layer 0.
    #[must_use]
    pub fn new(exit_position: Vec3, exit_velocity: Vec3) -> Self {
        Self {
            queue: ArrayVec::new(),
            exit_position,
            exit_velocity,
            exit_layer: 0,
        }
    }

    /// Queues a ball; false when the dispenser is full.
    pub fn push(&mut self, profile: ProfileId) -> bool {
        self.queue.try_push(profile).is_ok()
    }

    /// Removes the oldest queued ball.
    pub fn pop(&mut self) -> Option<ProfileId> {
        if self.queue.is_empty() {
            None
        } else {
            Some(self.queue.remove(0))
        }
    }

    /// Number of queued balls.
    #[must_use]
    pub fn len(&self) -> usize {
        self.queue.len()
    }

    /// Whether no ball is queued.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    /// Releases the oldest queued ball into `solver` at the exit. Returns
    /// `None`, leaving the queue as it is, when the dispenser is empty or
    /// the solver cannot take the ball (full, or unknown profile).
    pub fn dispense(&mut self, solver: &mut BallSolver) -> Option<BallHandle> {
        let profile = *self.queue.first()?;
        let mut ball = Ball::new(profile, self.exit_position);
        ball.velocity = self.exit_velocity;
        ball.layer = self.exit_layer;
        let handle = solver.add_ball(ball)?;
        self.queue.remove(0);
        Some(handle)
    }
}
