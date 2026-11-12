//! Runtime flipper contract (PLAN.md §3.3, D16).
//!
//! The on-disk `[[flippers]]` entries of `table.toml` (mm, names) are
//! [`schema::table::FlipperDef`](crate::schema::table::FlipperDef); they convert
//! into [`FlipperConfig`] with
//! [`FlipperDef::to_config`](crate::schema::table::FlipperDef::to_config).
//! Field names keep their unit suffixes exactly as §3.3 specifies.

use crate::ids::{CoilId, FlipperId, SwitchId};
use crate::input::{CabinetAxis, CabinetInput};

/// Bat geometry.
#[derive(Copy, Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum FlipperShape {
    /// Straight tapered capsule (standard bat).
    StraightCapsule {
        /// Pivot-to-tip length, mm.
        length_mm: f32,
        /// Pivot-end radius, mm.
        base_radius_mm: f32,
        /// Tip radius, mm.
        tip_radius_mm: f32,
    },
    /// Curved "Banana" bat (Subphase 2.6).
    CurvedBanana {
        /// Chord length, mm.
        length_mm: f32,
        /// Arc radius, mm.
        arc_radius_mm: f32,
        /// Arc sweep, degrees.
        arc_sweep_deg: f32,
        /// Tube radius, mm.
        tube_radius_mm: f32,
    },
}

/// How player input drives a flipper.
#[derive(Copy, Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub enum FlipperStagingMode {
    /// One button drives this flipper.
    DedicatedButton(CabinetInput),
    /// This flipper fires together with the others linked to the same button.
    ParallelLinked(CabinetInput),
    /// An analog button: lower stage above `lower_threshold`, upper above `upper_threshold`.
    DualStageThreshold {
        /// Analog axis.
        axis: CabinetAxis,
        /// Lower-stage threshold, 0..1.
        lower_threshold: f32,
        /// Upper-stage threshold, 0..1.
        upper_threshold: f32,
    },
    /// Automatic "Thing Flip": fires `delay_ms` after `trigger_switch` closes.
    AutoSensorThingFlip {
        /// Sensor switch.
        trigger_switch: SwitchId,
        /// Delay after the switch closes, ms.
        delay_ms: f32,
        /// How long the flipper is held, ms.
        hold_duration_ms: f32,
    },
}

/// Complete runtime description of one flipper.
#[derive(Clone, Debug, PartialEq, serde::Deserialize, serde::Serialize)]
pub struct FlipperConfig {
    /// Flipper id.
    pub id: FlipperId,
    /// Flipper coil.
    pub coil: CoilId,
    /// End-of-stroke switch.
    pub eos_switch: SwitchId,
    /// Collision layer (bit index).
    pub collision_layer: u8,
    /// Pivot position, mm.
    pub base_position_mm: [f32; 3],
    /// Unit normal of the plane the bat sweeps in.
    pub surface_normal: [f32; 3],
    /// Rest angle, degrees.
    pub rest_angle_deg: f32,
    /// Sweep, degrees: + = CCW about `surface_normal`, − = CW.
    pub sweep_angle_deg: f32,
    /// Fraction of sweep where EOS opens (e.g. 0.92).
    pub eos_angle_fraction: f32,
    /// Bat geometry.
    pub shape: FlipperShape,
    /// Bat + shaft + plunger inertia about the pivot, kg·m².
    pub inertia_kg_m2: f32,
    /// Power-winding torque, N·m.
    pub power_torque_nm: f32,
    /// Hold-winding torque, N·m.
    pub hold_torque_nm: f32,
    /// Return-spring torque, N·m.
    pub return_spring_torque_nm: f32,
    /// Bat-against-end-stop restitution.
    pub stop_restitution: f32,
    /// Ball-against-rubber restitution.
    pub rubber_restitution: f32,
    /// Velocity-dependent loss in the rubber.
    pub rubber_damping: f32,
    /// Input staging.
    pub staging: FlipperStagingMode,
    /// Zipper slide displacement, mm (`None` = not a zipper flipper).
    pub zipper_slide_axis_mm: Option<[f32; 3]>,
    /// Zipper slide speed, mm/s.
    pub zipper_slide_speed_mm_s: f32,
}

impl FlipperShape {
    /// Pivot-to-tip length, mm.
    pub fn length_mm(&self) -> f32 {
        match *self {
            FlipperShape::StraightCapsule { length_mm, .. }
            | FlipperShape::CurvedBanana { length_mm, .. } => length_mm,
        }
    }
}

impl FlipperConfig {
    /// Angle (degrees, same frame as `rest_angle_deg`) at which EOS opens.
    pub fn eos_angle_deg(&self) -> f32 {
        self.rest_angle_deg + self.sweep_angle_deg * self.eos_angle_fraction
    }

    /// Fully-energised angle, degrees.
    pub fn end_angle_deg(&self) -> f32 {
        self.rest_angle_deg + self.sweep_angle_deg
    }
}
