//! `table.toml` (PLAN.md Appendix A.1). Lengths mm, masses g, angles degrees;
//! elements referenced by name.

use super::physics::{PhysicsOverrides, PhysicsParams};
use super::{Manifest, SchemaError, manifest_io};
use crate::flipper::{FlipperConfig, FlipperShape, FlipperStagingMode};
use crate::ids::{CoilId, FlipperId, SwitchId};
use crate::input::{CabinetAxis, CabinetInput};
use serde::{Deserialize, Serialize};

/// Default playfield pitch, degrees (§3.2).
pub const DEFAULT_PITCH_DEG: f32 = 6.5;

fn default_pitch() -> f32 {
    DEFAULT_PITCH_DEG
}

/// `table.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableManifest {
    /// Must be 1.
    pub schema_version: u32,
    /// `[meta]`.
    pub meta: TableMeta,
    /// `[balls]`.
    pub balls: BallsSection,
    /// Optional `[physics]` overrides of `physics_defaults.toml`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub physics: Option<PhysicsOverrides>,
    /// `[cabinet_switches]`: switch numbers of the reserved cabinet inputs.
    pub cabinet_switches: CabinetSwitches,
    /// `[[switches]]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub switches: Vec<SwitchDef>,
    /// `[[coils]]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub coils: Vec<CoilDef>,
    /// `[[lamps]]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub lamps: Vec<LampDef>,
    /// `[[magnets]]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub magnets: Vec<MagnetDef>,
    /// `[[flippers]]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub flippers: Vec<FlipperDef>,
    /// `[[splines]]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub splines: Vec<SplineDef>,
    /// `[[layer_transitions]]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub layer_transitions: Vec<LayerTransitionDef>,
    /// `[[dispensers]]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub dispensers: Vec<DispenserDef>,
    /// `[[mechs]]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub mechs: Vec<MechDef>,
    /// `[[shots]]` (autoplay coverage).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub shots: Vec<ShotDef>,
    /// `[[modes]]` (soak must see each start).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modes: Vec<ModeDef>,
    /// `[[sounds]]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sounds: Vec<SoundDef>,
    /// `[[scenes]]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub scenes: Vec<SceneDef>,
    /// Optional `[persist]`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub persist: Option<PersistSection>,
    /// Optional `[dof]` (Subphase 3.5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dof: Option<DofSection>,
}

impl Manifest for TableManifest {
    fn schema_version(&self) -> u32 {
        self.schema_version
    }

    fn validate(&self) -> Result<(), SchemaError> {
        if let Some(dof) = &self.dof {
            for m in &dof.map {
                m.validate()?;
            }
        }
        Ok(())
    }
}

manifest_io!(TableManifest);

/// `[meta]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TableMeta {
    /// Table id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Author.
    pub author: String,
    /// Table version string.
    pub version: String,
    /// Rules entry file: `.lua` or `.wasm` selects the backend.
    pub script_entry: String,
    /// Playfield glTF.
    pub playfield_mesh: String,
    /// Playfield pitch, degrees (default 6.5).
    #[serde(default = "default_pitch")]
    pub pitch_deg: f32,
    /// Maximum players.
    pub players_max: u8,
    /// Colour palette (`#rrggbb`) for the placeholder asset kit.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub palette: Vec<String>,
}

/// Script backend chosen by the extension of `script_entry` (§3.4).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ScriptBackendKind {
    /// `.lua`
    Lua,
    /// `.wasm`
    Wasm,
}

impl TableMeta {
    /// The backend implied by `script_entry`, or `None` for an unknown extension.
    pub fn script_backend(&self) -> Option<ScriptBackendKind> {
        let ext = self.script_entry.rsplit_once('.')?.1;
        if ext.eq_ignore_ascii_case("lua") {
            Some(ScriptBackendKind::Lua)
        } else if ext.eq_ignore_ascii_case("wasm") {
            Some(ScriptBackendKind::Wasm)
        } else {
            None
        }
    }
}

/// `[balls]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BallsSection {
    /// Ball profiles the table uses.
    pub profiles: Vec<String>,
    /// Optional per-profile collision-layer restrictions.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub restrict: Vec<BallRestriction>,
}

/// Restricts a profile to the listed collision layers.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BallRestriction {
    /// Profile name.
    pub profile: String,
    /// Allowed layers.
    pub layers: Vec<u8>,
}

/// `[cabinet_switches]`: switch numbers of the eight reserved inputs (§3.4).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CabinetSwitches {
    /// Left flipper button.
    pub left_flipper: u16,
    /// Right flipper button.
    pub right_flipper: u16,
    /// Upper-left flipper button.
    pub left_upper: u16,
    /// Upper-right flipper button.
    pub right_upper: u16,
    /// Action button.
    pub action: u16,
    /// Start button.
    pub start: u16,
    /// Launch button.
    pub launch: u16,
    /// Tilt bob.
    pub tilt: u16,
}

impl CabinetSwitches {
    /// Switch number of `input`.
    pub fn number(&self, input: CabinetInput) -> u16 {
        match input {
            CabinetInput::LeftFlipper => self.left_flipper,
            CabinetInput::RightFlipper => self.right_flipper,
            CabinetInput::LeftUpper => self.left_upper,
            CabinetInput::RightUpper => self.right_upper,
            CabinetInput::Action => self.action,
            CabinetInput::Start => self.start,
            CabinetInput::Launch => self.launch,
            CabinetInput::Tilt => self.tilt,
        }
    }

    /// The runtime switch id of `input` (switch number = `SwitchId`).
    pub fn switch_id(&self, input: CabinetInput) -> SwitchId {
        SwitchId(self.number(input))
    }
}

/// Switch kinds.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SwitchKind {
    /// Optical switch: closes for every ball.
    Opto,
    /// Rollover: closes for every ball.
    Rollover,
    /// Eddy sensor: closes only for conductive balls (D12).
    Eddy,
    /// Leaf switch.
    Leaf,
    /// Target.
    Target,
    /// Trough switch (drain detection).
    Trough,
    /// Flipper end-of-stroke.
    Eos,
    /// Spinner.
    Spinner,
    /// Bumper ring.
    Ring,
}

/// `[[switches]]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SwitchDef {
    /// Name.
    pub id: String,
    /// Switch number.
    pub number: u16,
    /// Kind.
    pub kind: SwitchKind,
    /// Collision layer.
    #[serde(default)]
    pub layer: u8,
    /// Position, mm (omit when bound through glTF extras).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f32; 3]>,
    /// Detection radius, mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub radius: Option<f32>,
}

/// Coil kinds.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CoilKind {
    /// Flipper coil.
    Flipper,
    /// Pop bumper.
    Bumper,
    /// Slingshot.
    Sling,
    /// Kicker / saucer eject.
    Kicker,
    /// Vertical up-kicker.
    Vuk,
    /// Drop-target bank reset.
    DropReset,
    /// Diverter.
    Diverter,
    /// Autoplunger.
    Autoplunger,
    /// Knocker.
    Knocker,
    /// Flasher (driven as a coil).
    Flasher,
}

/// `[[coils]]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CoilDef {
    /// Name.
    pub id: String,
    /// Driver number.
    pub number: u16,
    /// Kind.
    pub kind: CoilKind,
}

/// Lamp kinds.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LampKind {
    /// Playfield insert.
    Insert,
    /// General-illumination string.
    Gi,
    /// Flasher.
    Flasher,
}

/// Lamp response curve.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LampResponse {
    /// Instant on/off.
    #[default]
    Led,
    /// Filament warm-up and cool-down.
    Incandescent,
}

/// `[[lamps]]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LampDef {
    /// Name.
    pub id: String,
    /// Lamp number.
    pub number: u16,
    /// Kind.
    pub kind: LampKind,
    /// Colour `#rrggbb`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// Response curve (default `led`).
    #[serde(default)]
    pub response: LampResponse,
}

/// `[[magnets]]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MagnetDef {
    /// Name.
    pub id: String,
    /// Magnet number.
    pub number: u16,
    /// Pole position, mm.
    pub position: [f32; 3],
    /// `K_coil`, N·m³ at full power.
    pub k_coil: f32,
    /// `d_min`, mm.
    pub d_min: f32,
    /// Range, mm.
    pub range: f32,
    /// Remanence time constant, ms (default from `physics_defaults.toml`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tau_ms: Option<f32>,
}

/// On-disk flipper shape: `{ type = "StraightCapsule", length, base_radius, tip_radius }`, mm.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum FlipperShapeDef {
    /// Straight tapered capsule.
    StraightCapsule {
        /// mm.
        length: f32,
        /// mm.
        base_radius: f32,
        /// mm.
        tip_radius: f32,
    },
    /// Curved "Banana" bat.
    CurvedBanana {
        /// mm.
        length: f32,
        /// mm.
        arc_radius: f32,
        /// Degrees.
        arc_sweep_deg: f32,
        /// mm.
        tube_radius: f32,
    },
}

impl From<FlipperShapeDef> for FlipperShape {
    fn from(d: FlipperShapeDef) -> Self {
        match d {
            FlipperShapeDef::StraightCapsule {
                length,
                base_radius,
                tip_radius,
            } => FlipperShape::StraightCapsule {
                length_mm: length,
                base_radius_mm: base_radius,
                tip_radius_mm: tip_radius,
            },
            FlipperShapeDef::CurvedBanana {
                length,
                arc_radius,
                arc_sweep_deg,
                tube_radius,
            } => FlipperShape::CurvedBanana {
                length_mm: length,
                arc_radius_mm: arc_radius,
                arc_sweep_deg,
                tube_radius_mm: tube_radius,
            },
        }
    }
}

impl From<FlipperShape> for FlipperShapeDef {
    fn from(s: FlipperShape) -> Self {
        match s {
            FlipperShape::StraightCapsule {
                length_mm,
                base_radius_mm,
                tip_radius_mm,
            } => FlipperShapeDef::StraightCapsule {
                length: length_mm,
                base_radius: base_radius_mm,
                tip_radius: tip_radius_mm,
            },
            FlipperShape::CurvedBanana {
                length_mm,
                arc_radius_mm,
                arc_sweep_deg,
                tube_radius_mm,
            } => FlipperShapeDef::CurvedBanana {
                length: length_mm,
                arc_radius: arc_radius_mm,
                arc_sweep_deg,
                tube_radius: tube_radius_mm,
            },
        }
    }
}

/// On-disk staging: `{ type = "DedicatedButton", input = "left_flipper" }`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", deny_unknown_fields)]
pub enum FlipperStagingDef {
    /// One button drives this flipper.
    DedicatedButton {
        /// Cabinet input name.
        input: CabinetInput,
    },
    /// Fires together with the others linked to the same button.
    ParallelLinked {
        /// Cabinet input name.
        input: CabinetInput,
    },
    /// Analog two-stage button.
    DualStageThreshold {
        /// Axis name.
        axis: CabinetAxis,
        /// 0..1.
        lower_threshold: f32,
        /// 0..1.
        upper_threshold: f32,
    },
    /// Automatic "Thing Flip".
    AutoSensorThingFlip {
        /// Sensor switch name.
        trigger_switch: String,
        /// ms.
        delay_ms: f32,
        /// ms.
        hold_duration_ms: f32,
    },
}

/// `[[flippers]]`: the fields of [`FlipperConfig`] in on-disk units. Lengths are
/// mm without a suffix (as in Appendix A); torque/inertia keep their SI suffixes.
/// Omitted physical parameters default from `physics_defaults.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FlipperDef {
    /// Name.
    pub id: String,
    /// Coil name.
    pub coil: String,
    /// EOS switch name.
    pub eos_switch: String,
    /// Collision layer.
    #[serde(default)]
    pub collision_layer: u8,
    /// Pivot, mm.
    pub base_position: [f32; 3],
    /// Unit normal of the sweep plane.
    pub surface_normal: [f32; 3],
    /// Degrees.
    pub rest_angle_deg: f32,
    /// Degrees; + = CCW about `surface_normal`.
    pub sweep_angle_deg: f32,
    /// Fraction of sweep where EOS opens.
    pub eos_angle_fraction: f32,
    /// Bat geometry.
    pub shape: FlipperShapeDef,
    /// Input staging.
    pub staging: FlipperStagingDef,
    /// kg·m².
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inertia_kg_m2: Option<f32>,
    /// N·m.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_torque_nm: Option<f32>,
    /// N·m.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hold_torque_nm: Option<f32>,
    /// N·m.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub return_spring_torque_nm: Option<f32>,
    /// End-stop restitution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stop_restitution: Option<f32>,
    /// Rubber restitution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rubber_restitution: Option<f32>,
    /// Rubber damping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rubber_damping: Option<f32>,
    /// Zipper slide displacement, mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zipper_slide_axis: Option<[f32; 3]>,
    /// Zipper slide speed, mm/s.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zipper_slide_speed: Option<f32>,
}

/// Resolves element names to interned ids.
pub trait NameResolver {
    /// Coil id for `name`.
    fn coil(&self, name: &str) -> Option<CoilId>;
    /// Switch id for `name`.
    fn switch(&self, name: &str) -> Option<SwitchId>;
}

impl FlipperDef {
    /// Converts to the runtime [`FlipperConfig`], resolving names with `names`
    /// and filling omitted parameters from `params`
    /// ([`PhysicsDefaults::resolve`](super::PhysicsDefaults::resolve)).
    pub fn to_config(
        &self,
        id: FlipperId,
        names: &impl NameResolver,
        params: &PhysicsParams,
    ) -> Result<FlipperConfig, SchemaError> {
        let coil = names
            .coil(&self.coil)
            .ok_or_else(|| SchemaError::UnknownName {
                kind: "coil",
                name: self.coil.clone(),
            })?;
        let switch = |name: &str| {
            names.switch(name).ok_or_else(|| SchemaError::UnknownName {
                kind: "switch",
                name: name.to_owned(),
            })
        };
        let eos_switch = switch(&self.eos_switch)?;
        let staging = match &self.staging {
            FlipperStagingDef::DedicatedButton { input } => {
                FlipperStagingMode::DedicatedButton(*input)
            }
            FlipperStagingDef::ParallelLinked { input } => {
                FlipperStagingMode::ParallelLinked(*input)
            }
            FlipperStagingDef::DualStageThreshold {
                axis,
                lower_threshold,
                upper_threshold,
            } => FlipperStagingMode::DualStageThreshold {
                axis: *axis,
                lower_threshold: *lower_threshold,
                upper_threshold: *upper_threshold,
            },
            FlipperStagingDef::AutoSensorThingFlip {
                trigger_switch,
                delay_ms,
                hold_duration_ms,
            } => FlipperStagingMode::AutoSensorThingFlip {
                trigger_switch: switch(trigger_switch)?,
                delay_ms: *delay_ms,
                hold_duration_ms: *hold_duration_ms,
            },
        };
        Ok(FlipperConfig {
            id,
            coil,
            eos_switch,
            collision_layer: self.collision_layer,
            base_position_mm: self.base_position,
            surface_normal: self.surface_normal,
            rest_angle_deg: self.rest_angle_deg,
            sweep_angle_deg: self.sweep_angle_deg,
            eos_angle_fraction: self.eos_angle_fraction,
            shape: self.shape.into(),
            inertia_kg_m2: self.inertia_kg_m2.unwrap_or(params.flipper_inertia),
            power_torque_nm: self.power_torque_nm.unwrap_or(params.flipper_power_torque),
            hold_torque_nm: self.hold_torque_nm.unwrap_or(params.flipper_hold_torque),
            return_spring_torque_nm: self
                .return_spring_torque_nm
                .unwrap_or(params.flipper_return_spring_torque),
            stop_restitution: self
                .stop_restitution
                .unwrap_or(params.flipper_stop_restitution),
            rubber_restitution: self
                .rubber_restitution
                .unwrap_or(params.flipper_rubber_restitution),
            rubber_damping: self.rubber_damping.unwrap_or(params.flipper_rubber_damping),
            staging,
            zipper_slide_axis_mm: self.zipper_slide_axis,
            zipper_slide_speed_mm_s: self.zipper_slide_speed.unwrap_or(0.0),
        })
    }
}

/// Spline kinds.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplineKind {
    /// Wire ramp, modelled as two rails (D13).
    Wireform,
    /// Guide rail.
    Guide,
}

/// `[[splines]]`: C² interpolating spline through `points` (D13).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SplineDef {
    /// Name.
    pub id: String,
    /// Kind.
    pub kind: SplineKind,
    /// Collision layer.
    #[serde(default)]
    pub layer: u8,
    /// Wire gauge (rail spacing), mm; wireform only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gauge: Option<f32>,
    /// Control points, mm.
    pub points: Vec<[f32; 3]>,
    /// Bank angle per point, degrees.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub roll_deg: Option<Vec<f32>>,
}

/// Axis-aligned box, mm.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AabbDef {
    /// Minimum corner.
    pub min: [f32; 3],
    /// Maximum corner.
    pub max: [f32; 3],
}

/// `[[layer_transitions]]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LayerTransitionDef {
    /// Name.
    pub id: String,
    /// Source layer.
    pub from_layer: u8,
    /// Target layer.
    pub to_layer: u8,
    /// Trigger zone.
    pub zone: AabbDef,
}

/// `[[dispensers]]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DispenserDef {
    /// Name.
    pub id: String,
    /// Dispenser number.
    pub number: u16,
    /// Exit position, mm.
    pub exit_position: [f32; 3],
    /// Exit velocity, m/s.
    pub exit_velocity: [f32; 3],
    /// Initial contents (profile names), in dispense order.
    #[serde(default)]
    pub contents: Vec<String>,
}

/// Mech kinds.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MechKind {
    /// Pop bumper.
    Bumper,
    /// Slingshot.
    Sling,
    /// Drop target.
    DropTarget,
    /// Spinner.
    Spinner,
    /// Vertical up-kicker.
    Vuk,
    /// Plunger.
    Plunger,
    /// Kickback.
    Kickback,
    /// Diverter.
    Diverter,
    /// Captive Newton ball.
    NewtonBall,
    /// One-way gate.
    Gate,
}

/// `[[mechs]]`. Kind-specific parameters default from `physics_defaults.toml`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MechDef {
    /// Name.
    pub id: String,
    /// Kind.
    pub kind: MechKind,
    /// Driving coil name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coil: Option<String>,
    /// Associated switch name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switch: Option<String>,
    /// Position, mm.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub position: Option<[f32; 3]>,
}

/// `[[shots]]`: a switch sequence completed within `window_ms`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShotDef {
    /// Name.
    pub id: String,
    /// Switch names in order.
    pub sequence: Vec<String>,
    /// Time window, ms.
    pub window_ms: u32,
}

/// `[[modes]]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModeDef {
    /// Name.
    pub id: String,
    /// Scene shown when the mode starts.
    pub start_scene: String,
}

/// Audio buses.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SoundBus {
    /// Music.
    Music,
    /// Voice callouts.
    Callout,
    /// Sound effects.
    Sfx,
}

/// `[[sounds]]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SoundDef {
    /// Name.
    pub id: String,
    /// Bundle path.
    pub file: String,
    /// Bus.
    pub bus: SoundBus,
}

/// `[[scenes]]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneDef {
    /// Name.
    pub id: String,
    /// Bundle path.
    pub file: String,
}

/// `[persist]`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PersistSection {
    /// Persistent key names (at most 64 are addressable by `HostView::persist`).
    pub keys: Vec<String>,
}

/// `[dof]` (Subphase 3.5).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DofSection {
    /// ROM name DOF configs are keyed by.
    pub rom_name: String,
    /// Element mappings.
    #[serde(default)]
    pub map: Vec<DofMapping>,
}

/// DOF table-element types.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DofElementType {
    /// Solenoid.
    S,
    /// Switch.
    W,
    /// Lamp.
    L,
    /// GI string.
    G,
    /// Named element.
    E,
}

/// One `[dof].map` entry: exactly one of `coil`, `switch`, `lamp` names the source.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DofMapping {
    /// Coil name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub coil: Option<String>,
    /// Switch name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub switch: Option<String>,
    /// Lamp name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lamp: Option<String>,
    /// DOF element type.
    #[serde(rename = "type")]
    pub element_type: DofElementType,
    /// DOF element number.
    pub number: u16,
}

impl DofMapping {
    fn validate(&self) -> Result<(), SchemaError> {
        let sources = [&self.coil, &self.switch, &self.lamp]
            .iter()
            .filter(|s| s.is_some())
            .count();
        if sources == 1 {
            Ok(())
        } else {
            Err(SchemaError::Invalid(format!(
                "[dof].map entry {} must name exactly one of coil, switch, lamp (found {sources})",
                self.number
            )))
        }
    }
}

/// A [`NameResolver`] over a manifest that maps names to their element
/// *numbers* (switch number = `SwitchId`, coil number = `CoilId`). Provisional:
/// `pin_assets` (Subphase 4.1) owns final interning.
pub struct NumberResolver<'a>(pub &'a TableManifest);

impl NameResolver for NumberResolver<'_> {
    fn coil(&self, name: &str) -> Option<CoilId> {
        self.0
            .coils
            .iter()
            .find(|c| c.id == name)
            .map(|c| CoilId(c.number))
    }

    fn switch(&self, name: &str) -> Option<SwitchId> {
        self.0
            .switches
            .iter()
            .find(|s| s.id == name)
            .map(|s| SwitchId(s.number))
    }
}
