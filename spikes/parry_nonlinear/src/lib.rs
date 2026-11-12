//! Spike 0.1/Q6c: `parry3d` nonlinear shape cast, capsule (flipper) vs ball.
//!
//! Flipper model: a capsule whose segment runs along local +X from the pivot
//! (local `-half_len` end) and which rotates about the playfield normal (+Z)
//! around that pivot.

use parry3d::math::{Pose, Real, Vector};
use parry3d::query::{self, NonlinearRigidMotion, ShapeCastHit, Unsupported};
use parry3d::shape::{Ball, Capsule};

/// Geometry of the test flipper.
#[derive(Clone, Copy, Debug)]
pub struct Flipper {
    pub half_len: Real,
    pub radius: Real,
    /// Angular speed about +Z (rad/s).
    pub omega: Real,
}

impl Flipper {
    pub fn shape(&self) -> Capsule {
        Capsule::new_x(self.half_len, self.radius)
    }

    /// Motion with the pivot at the world origin and the segment initially along +X.
    pub fn motion(&self) -> NonlinearRigidMotion {
        NonlinearRigidMotion::new(
            Pose::from_translation(Vector::new(self.half_len, 0.0, 0.0)),
            Vector::new(-self.half_len, 0.0, 0.0),
            Vector::ZERO,
            Vector::new(0.0, 0.0, self.omega),
        )
    }
}

/// Cast the rotating flipper against a ball moving linearly from `ball_pos`.
pub fn flipper_vs_ball(
    flipper: Flipper,
    ball_radius: Real,
    ball_pos: Vector,
    ball_vel: Vector,
    t_end: Real,
) -> Result<Option<ShapeCastHit>, Unsupported> {
    let ball = Ball::new(ball_radius);
    let ball_motion = NonlinearRigidMotion::new(
        Pose::from_translation(ball_pos),
        Vector::ZERO,
        ball_vel,
        Vector::ZERO,
    );
    query::cast_shapes_nonlinear(
        &flipper.motion(),
        &flipper.shape(),
        &ball_motion,
        &ball,
        0.0,
        t_end,
        true,
    )
}
