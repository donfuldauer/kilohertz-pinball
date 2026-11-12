//! The per-tick frame state published by the sim thread (§2.2 step 6, D7).
//!
//! `FrameStateSnapshot` is `Copy` and fixed-capacity so it can travel through a
//! [`TripleBuffer`](crate::TripleBuffer) without allocating. All quantities are SI
//! (m, m/s, rad, rad/s). Quaternions are `[x, y, z, w]`.

use crate::ids::{FlipperId, ProfileId};

/// Maximum balls on the table at once.
pub const MAX_BALLS: usize = 16;
/// Maximum flippers.
pub const MAX_FLIPPERS: usize = 16;
/// Maximum dynamic (moving) meshes: drop targets, diverters, spinners, toys, ...
pub const MAX_DYNAMIC_MESHES: usize = 64;
/// Maximum lamps (inserts, GI strings, flashers), indexed by `LampId`.
pub const MAX_LAMPS: usize = 256;

/// The identity quaternion `[x, y, z, w]`.
pub const IDENTITY_QUAT: [f32; 4] = [0.0, 0.0, 0.0, 1.0];

/// State of one ball.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BallState {
    /// Centre position, metres, world space.
    pub position_m: [f32; 3],
    /// Orientation quaternion `[x, y, z, w]`.
    pub orientation: [f32; 4],
    /// Linear velocity, m/s.
    pub velocity_m_s: [f32; 3],
    /// Angular velocity, rad/s.
    pub angular_velocity_rad_s: [f32; 3],
    /// Ball profile.
    pub profile: ProfileId,
    /// Collision layer the ball is currently on.
    pub layer: u8,
}

impl Default for BallState {
    fn default() -> Self {
        Self {
            position_m: [0.0; 3],
            orientation: IDENTITY_QUAT,
            velocity_m_s: [0.0; 3],
            angular_velocity_rad_s: [0.0; 3],
            profile: ProfileId(0),
            layer: 0,
        }
    }
}

/// State of one flipper.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct FlipperState {
    /// Which flipper this is.
    pub id: FlipperId,
    /// Bat angle about the surface normal, radians, relative to the rest angle.
    pub angle_rad: f32,
    /// Bat angular velocity, rad/s.
    pub angular_velocity_rad_s: f32,
    /// Zipper slide offset along the slide axis, metres (0 = home).
    pub zipper_offset_m: f32,
}

/// World transform of one dynamic mesh.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MeshTransform {
    /// Translation, metres.
    pub position_m: [f32; 3],
    /// Rotation quaternion `[x, y, z, w]`.
    pub orientation: [f32; 4],
}

impl Default for MeshTransform {
    fn default() -> Self {
        Self {
            position_m: [0.0; 3],
            orientation: IDENTITY_QUAT,
        }
    }
}

/// Everything the renderer and audio need from one sim tick.
///
/// Only the first `ball_count` / `flipper_count` / `dynamic_mesh_count` entries
/// of the fixed arrays are meaningful.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct FrameStateSnapshot {
    /// Sim tick (1 tick = 1 ms of simulated time).
    pub tick: u64,
    /// Number of valid entries in `balls`.
    pub ball_count: u8,
    /// Ball states.
    pub balls: [BallState; MAX_BALLS],
    /// Number of valid entries in `flippers`.
    pub flipper_count: u8,
    /// Flipper states.
    pub flippers: [FlipperState; MAX_FLIPPERS],
    /// Number of valid entries in `dynamic_meshes`.
    pub dynamic_mesh_count: u8,
    /// Dynamic mesh transforms.
    pub dynamic_meshes: [MeshTransform; MAX_DYNAMIC_MESHES],
    /// Lamp intensities `0..=1`, indexed by `LampId`.
    pub lamps: [f32; MAX_LAMPS],
    /// General-illumination tint `[r, g, b, intensity]`, each `0..=1`.
    pub gi: [f32; 4],
}

impl Default for FrameStateSnapshot {
    fn default() -> Self {
        Self {
            tick: 0,
            ball_count: 0,
            balls: [BallState::default(); MAX_BALLS],
            flipper_count: 0,
            flippers: [FlipperState::default(); MAX_FLIPPERS],
            dynamic_mesh_count: 0,
            dynamic_meshes: [MeshTransform::default(); MAX_DYNAMIC_MESHES],
            lamps: [0.0; MAX_LAMPS],
            gi: [1.0, 1.0, 1.0, 1.0],
        }
    }
}

impl FrameStateSnapshot {
    /// The valid balls.
    pub fn balls(&self) -> &[BallState] {
        &self.balls[..usize::from(self.ball_count).min(MAX_BALLS)]
    }

    /// The valid flippers.
    pub fn flippers(&self) -> &[FlipperState] {
        &self.flippers[..usize::from(self.flipper_count).min(MAX_FLIPPERS)]
    }

    /// The valid dynamic mesh transforms.
    pub fn dynamic_meshes(&self) -> &[MeshTransform] {
        &self.dynamic_meshes[..usize::from(self.dynamic_mesh_count).min(MAX_DYNAMIC_MESHES)]
    }

    /// Appends a ball; returns `false` (and drops it) when full.
    pub fn push_ball(&mut self, ball: BallState) -> bool {
        let n = usize::from(self.ball_count);
        if n >= MAX_BALLS {
            return false;
        }
        self.balls[n] = ball;
        self.ball_count += 1;
        true
    }

    /// Appends a flipper; returns `false` (and drops it) when full.
    pub fn push_flipper(&mut self, flipper: FlipperState) -> bool {
        let n = usize::from(self.flipper_count);
        if n >= MAX_FLIPPERS {
            return false;
        }
        self.flippers[n] = flipper;
        self.flipper_count += 1;
        true
    }

    /// Appends a dynamic mesh transform; returns `false` (and drops it) when full.
    pub fn push_dynamic_mesh(&mut self, mesh: MeshTransform) -> bool {
        let n = usize::from(self.dynamic_mesh_count);
        if n >= MAX_DYNAMIC_MESHES {
            return false;
        }
        self.dynamic_meshes[n] = mesh;
        self.dynamic_mesh_count += 1;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_is_empty_with_identity_orientations() {
        let s = FrameStateSnapshot::default();
        assert_eq!(s.tick, 0);
        assert!(s.balls().is_empty());
        assert!(s.flippers().is_empty());
        assert!(s.dynamic_meshes().is_empty());
        assert!(s.balls.iter().all(|b| b.orientation == IDENTITY_QUAT));
        assert!(s.dynamic_meshes.iter().all(|m| m.orientation == IDENTITY_QUAT));
        assert_eq!(s.lamps.len(), MAX_LAMPS);
    }

    #[test]
    fn push_respects_capacity() {
        let mut s = FrameStateSnapshot::default();
        for i in 0..MAX_BALLS {
            assert!(s.push_ball(BallState {
                layer: i as u8,
                ..BallState::default()
            }));
        }
        assert!(!s.push_ball(BallState::default()));
        assert_eq!(s.balls().len(), MAX_BALLS);
        assert_eq!(s.balls()[5].layer, 5);
        for _ in 0..MAX_FLIPPERS {
            assert!(s.push_flipper(FlipperState::default()));
        }
        assert!(!s.push_flipper(FlipperState::default()));
        assert_eq!(s.flippers().len(), MAX_FLIPPERS);
        for _ in 0..MAX_DYNAMIC_MESHES {
            assert!(s.push_dynamic_mesh(MeshTransform::default()));
        }
        assert!(!s.push_dynamic_mesh(MeshTransform::default()));
        assert_eq!(s.dynamic_meshes().len(), MAX_DYNAMIC_MESHES);
    }

    #[test]
    fn is_copy_and_bounded_size() {
        fn assert_copy<T: Copy + Send + Sync + 'static>() {}
        assert_copy::<FrameStateSnapshot>();
        // Keep the hand-off cheap: well under one page pair.
        assert!(core::mem::size_of::<FrameStateSnapshot>() < 8192);
    }
}
