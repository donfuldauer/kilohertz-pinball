//! Plain data types of the ball solver: balls, handles, configuration,
//! magnets, sensors, events and the hook traits.

use arrayvec::ArrayVec;
use glam::{Quat, Vec3};
use pin_core::ids::{ProfileId, SwitchId};
use pin_core::snapshot::BallState;
use pin_core::{BallProfile, MaterialPair, PhysicsDefaults};

use crate::world::{ColliderHandle, GuideCurve, StaticWorld, Wireform};

/// Maximum number of free balls (equals the `pin_core` snapshot capacity).
pub const MAX_BALLS: usize = pin_core::snapshot::MAX_BALLS;

/// Capacity of [`BallEvents`].
pub const MAX_BALL_EVENTS: usize = 64;

/// Normal speed (m/s) below which an impact is not reported, and below
/// which restitution is zero.
pub const IMPACT_REPORT_SPEED: f32 = 0.05;

/// State of one free ball (SI units, world space).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Ball {
    /// Centre position, metres.
    pub position: Vec3,
    /// Linear velocity, m/s.
    pub velocity: Vec3,
    /// Angular velocity, rad/s.
    pub angular_velocity: Vec3,
    /// Orientation (for rendering; does not affect the dynamics).
    pub orientation: Quat,
    /// Ball profile (index into the profile list given to the solver).
    pub profile: ProfileId,
    /// Collision layer index (0..=7), see [`crate::world::layers::layer_bit`].
    pub layer: u8,
}

impl Ball {
    /// A ball at rest at `position`: zero velocities, identity orientation,
    /// layer 0.
    #[must_use]
    pub fn new(profile: ProfileId, position: Vec3) -> Self {
        Self {
            position,
            velocity: Vec3::ZERO,
            angular_velocity: Vec3::ZERO,
            orientation: Quat::IDENTITY,
            profile,
            layer: 0,
        }
    }
}

impl From<&Ball> for BallState {
    fn from(b: &Ball) -> Self {
        BallState {
            position_m: b.position.to_array(),
            orientation: [
                b.orientation.x,
                b.orientation.y,
                b.orientation.z,
                b.orientation.w,
            ],
            velocity_m_s: b.velocity.to_array(),
            angular_velocity_rad_s: b.angular_velocity.to_array(),
            profile: b.profile,
            layer: b.layer,
        }
    }
}

/// Identifies a ball slot of a [`super::BallSolver`] (0..[`MAX_BALLS`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BallHandle(pub u8);

/// Solver configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct BallSolverConfig {
    /// Gravity, world space, m/s² (table pitch comes from the geometry).
    pub gravity: Vec3,
    /// Material-pair table (restitution and friction per unordered pair).
    pub pairs: Vec<MaterialPair>,
    /// Rolling resistance coefficient (dimensionless, Appendix C).
    pub rolling_resistance: f32,
    /// Safety cap on the CCD sub-steps of one ball per tick.
    pub max_substeps: u32,
}

impl BallSolverConfig {
    /// Gravity (0, 0, −9.81), the pair table and rolling resistance of
    /// `d`, and 64 sub-steps at most.
    #[must_use]
    pub fn from_defaults(d: &PhysicsDefaults) -> Self {
        Self {
            gravity: Vec3::new(0.0, 0.0, -9.81),
            pairs: d.material_pairs.clone(),
            rolling_resistance: d.tunables.rolling_resistance.default,
            max_substeps: 64,
        }
    }
}

/// An electromagnet (PLAN.md §3.2):
/// `F = c · P · K_coil · d̂ / max(d³, d_min³)` within `range`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Magnet {
    /// Position of the pole, metres.
    pub position: Vec3,
    /// Coil constant `K_coil`, N·m³.
    pub k_coil: f32,
    /// Distance below which the force stops growing, metres.
    pub d_min: f32,
    /// Distance beyond which the force is zero, metres.
    pub range: f32,
    /// Layers on which the magnet acts.
    pub layer_mask: u8,
}

/// What a [`Sensor`] detects.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SensorKind {
    /// Eddy-current sensor: closes only for conductive balls (D12).
    Eddy,
    /// Opto or rollover: closes for every ball.
    Opto,
}

/// A spherical sensor zone that closes a switch while a ball centre is
/// inside it.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Sensor {
    /// The switch it drives.
    pub switch: SwitchId,
    /// Detection principle.
    pub kind: SensorKind,
    /// Centre of the zone, metres.
    pub center: Vec3,
    /// Radius of the zone, metres.
    pub radius: f32,
    /// Layers the sensor watches.
    pub layer_mask: u8,
}

/// The collision geometry a step runs against.
#[derive(Copy, Clone, Debug)]
pub struct Geometry<'a> {
    /// Static colliders.
    pub world: &'a StaticWorld,
    /// Guide walls (not part of the world's BVH).
    pub guides: &'a [GuideCurve],
    /// Wireform ramps (not part of the world's BVH).
    pub wireforms: &'a [Wireform],
}

impl<'a> Geometry<'a> {
    /// `world` with no guides and no wireforms.
    #[must_use]
    pub fn new(world: &'a StaticWorld) -> Self {
        Self {
            world,
            guides: &[],
            wireforms: &[],
        }
    }
}

/// An extra force on the free balls (Magnus, remanence: Subphase 2.7).
pub trait ExternalForce: Send {
    /// Extra force (N) on `ball` this sub-step; called in ball-handle order, after gravity.
    fn force(&mut self, handle: BallHandle, ball: &Ball, profile: &BallProfile, dt: f32) -> Vec3;
}

/// Velocity of moving collision surfaces (spinning discs, conveyors, and
/// the seam Subphase 2.3 uses for kinematic colliders).
pub trait SurfaceVelocity: Send {
    /// Velocity (m/s) of the surface of `target` at the world point `point`.
    fn velocity_at(&self, target: ImpactTarget, point: Vec3) -> Vec3;
}

/// What a ball hit.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ImpactTarget {
    /// A static collider.
    Collider(ColliderHandle),
    /// Guide `index` of [`Geometry::guides`].
    Guide(usize),
    /// Wireform `index` of [`Geometry::wireforms`].
    Wireform(usize),
    /// Another free ball.
    Ball(BallHandle),
    /// A captive ball of Newton chain `index`.
    Newton(usize),
}

/// Something that happened during a step.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum BallEvent {
    /// A ball hit something with at least [`IMPACT_REPORT_SPEED`].
    Impact {
        /// The ball (for ball–ball impacts, the lower handle).
        ball: BallHandle,
        /// What it hit.
        other: ImpactTarget,
        /// Approach speed along the contact normal, m/s.
        normal_speed: f32,
    },
    /// A sensor opened or closed.
    SensorEdge {
        /// The sensor's switch.
        switch: SwitchId,
        /// New state.
        closed: bool,
    },
}

/// Fixed-capacity event list filled by [`super::BallSolver::step`].
#[derive(Clone, Debug, Default)]
pub struct BallEvents {
    list: ArrayVec<BallEvent, MAX_BALL_EVENTS>,
    dropped: u32,
}

impl BallEvents {
    /// An empty list.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Removes all events and resets the dropped counter.
    pub fn clear(&mut self) {
        self.list.clear();
        self.dropped = 0;
    }

    /// The events, in the order they happened.
    #[must_use]
    pub fn as_slice(&self) -> &[BallEvent] {
        &self.list
    }

    /// Number of events that did not fit since the last `clear`.
    #[must_use]
    pub fn dropped(&self) -> u32 {
        self.dropped
    }

    /// Appends an event, or counts it as dropped when the list is full.
    pub fn push(&mut self, e: BallEvent) {
        if self.list.try_push(e).is_err() {
            self.dropped = self.dropped.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_drop_when_full() {
        let mut ev = BallEvents::new();
        for i in 0..(MAX_BALL_EVENTS as u16 + 3) {
            ev.push(BallEvent::SensorEdge {
                switch: SwitchId(i),
                closed: true,
            });
        }
        assert_eq!(ev.as_slice().len(), MAX_BALL_EVENTS);
        assert_eq!(ev.dropped(), 3);
        ev.clear();
        assert!(ev.as_slice().is_empty());
        assert_eq!(ev.dropped(), 0);
    }

    #[test]
    fn ball_state_conversion() {
        let mut b = Ball::new(ProfileId(3), Vec3::new(1.0, 2.0, 3.0));
        b.velocity = Vec3::new(4.0, 5.0, 6.0);
        b.angular_velocity = Vec3::new(7.0, 8.0, 9.0);
        b.layer = 2;
        let s = BallState::from(&b);
        assert_eq!(s.position_m, [1.0, 2.0, 3.0]);
        assert_eq!(s.velocity_m_s, [4.0, 5.0, 6.0]);
        assert_eq!(s.angular_velocity_rad_s, [7.0, 8.0, 9.0]);
        assert_eq!(s.orientation, [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(s.profile, ProfileId(3));
        assert_eq!(s.layer, 2);
    }

    #[test]
    fn config_from_defaults() {
        let d = PhysicsDefaults::builtin();
        let c = BallSolverConfig::from_defaults(&d);
        assert_eq!(c.gravity, Vec3::new(0.0, 0.0, -9.81));
        assert_eq!(c.rolling_resistance, 0.002);
        assert_eq!(c.max_substeps, 64);
        assert_eq!(c.pairs.len(), d.material_pairs.len());
    }
}
