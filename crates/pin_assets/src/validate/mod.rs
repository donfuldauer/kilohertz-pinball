//! Manifest validation and name-to-ID interning.
//!
//! Owner: Subphase 4.1.
//!
//! [`load_table`] reads `table.toml` and `backglass.toml` from a [`Vfs`],
//! parses them with `pin_core` (syntax, types, unknown keys, schema version),
//! then checks every cross-reference, number range, referenced file and spline
//! here. All errors are collected (not just the first) and sorted by
//! `(file, path)`. On success, names are interned to `u16` ids (see
//! [`TableIds`]) and every positional declaration is converted to SI.

mod guide_lint;
mod ids;
mod rules;

pub use ids::{IdKind, TableIds};

use crate::{AssetError, Vfs};
use core::fmt;
use pin_core::schema::table::{MechKind, SplineKind, SwitchKind};
use pin_core::schema::{BackglassManifest, PhysicsParams, SchemaError, TableManifest};
use pin_core::{
    CoilId, DispenserId, DiverterId, FlipperConfig, MagnetId, ProfileId, SceneId, SwitchId,
};

/// File name of the table manifest at the bundle root.
pub const TABLE_FILE: &str = "table.toml";
/// File name of the backglass manifest at the bundle root.
pub const BACKGLASS_FILE: &str = "backglass.toml";

/// One validation problem: which file, which TOML path, and what is wrong
/// (including the offending value).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ValidationError {
    /// Bundle file, e.g. `table.toml`.
    pub file: String,
    /// TOML key path, e.g. `flippers[0].coil` (empty for a whole-file error).
    pub path: String,
    /// What is wrong.
    pub message: String,
}

impl ValidationError {
    /// Creates an error.
    pub fn new(
        file: impl Into<String>,
        path: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        ValidationError {
            file: file.into(),
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}: {}", self.file, self.message)
        } else {
            write!(f, "{}: {}: {}", self.file, self.path, self.message)
        }
    }
}

impl std::error::Error for ValidationError {}

/// Which flipper button stage a flipper belongs to (Orchestrator Wave 2
/// decision): `Upper` if staged from `left_upper` / `right_upper` or on a
/// collision layer other than 0.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum FlipperStageHint {
    /// Lower (main) flippers.
    Lower,
    /// Upper flippers.
    Upper,
}

/// A `[[switches]]` entry in SI.
#[derive(Clone, Debug, PartialEq)]
pub struct SwitchSi {
    /// Interned id (= declared number).
    pub id: SwitchId,
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: SwitchKind,
    /// Collision layer.
    pub layer: u8,
    /// Position, m.
    pub position_m: Option<[f32; 3]>,
    /// Detection radius, m.
    pub radius_m: Option<f32>,
}

/// A `[[magnets]]` entry in SI.
#[derive(Clone, Debug, PartialEq)]
pub struct MagnetSi {
    /// Interned id (= declared number).
    pub id: MagnetId,
    /// Name.
    pub name: String,
    /// Pole position, m.
    pub position_m: [f32; 3],
    /// `K_coil`, N·m³ at full power.
    pub k_coil: f32,
    /// `d_min`, m.
    pub d_min_m: f32,
    /// Range, m.
    pub range_m: f32,
    /// Remanence time constant, s (table value or the physics default).
    pub tau_s: f32,
}

/// A `[[splines]]` entry in SI, after closure snapping.
#[derive(Clone, Debug, PartialEq)]
pub struct SplineSi {
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: SplineKind,
    /// Collision layer.
    pub layer: u8,
    /// Rail gauge, m (wireforms).
    pub gauge_m: Option<f32>,
    /// Through-points, m. A closed guide ends exactly on its first point.
    pub points_m: Vec<[f32; 3]>,
    /// Bank angle per point, rad (zeros when not given).
    pub roll_rad: Vec<f32>,
    /// True for a closed guide (built with `CubicSpline::interpolate_closed`).
    pub closed: bool,
}

/// A `[[layer_transitions]]` entry in SI.
#[derive(Clone, Debug, PartialEq)]
pub struct TransitionSi {
    /// Name.
    pub name: String,
    /// Source layer.
    pub from_layer: u8,
    /// Target layer.
    pub to_layer: u8,
    /// Zone minimum corner, m.
    pub zone_min_m: [f32; 3],
    /// Zone maximum corner, m.
    pub zone_max_m: [f32; 3],
}

/// A `[[dispensers]]` entry in SI.
#[derive(Clone, Debug, PartialEq)]
pub struct DispenserSi {
    /// Interned id (= declared number).
    pub id: DispenserId,
    /// Name.
    pub name: String,
    /// Exit position, m.
    pub exit_position_m: [f32; 3],
    /// Exit velocity, m/s.
    pub exit_velocity_m_s: [f32; 3],
    /// Initial contents, in dispense order.
    pub contents: Vec<ProfileId>,
}

/// A `[[mechs]]` entry with resolved names, in SI.
#[derive(Clone, Debug, PartialEq)]
pub struct MechSi {
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: MechKind,
    /// Driving coil.
    pub coil: Option<CoilId>,
    /// Associated switch.
    pub switch: Option<SwitchId>,
    /// Diverter id (kind `diverter` only).
    pub diverter: Option<DiverterId>,
    /// Position, m.
    pub position_m: Option<[f32; 3]>,
}

/// A `[[shots]]` entry with resolved switches.
#[derive(Clone, Debug, PartialEq)]
pub struct ShotSi {
    /// Name.
    pub name: String,
    /// Switches in order.
    pub sequence: Vec<SwitchId>,
    /// Time window, s.
    pub window_s: f32,
}

/// A `[[modes]]` entry with its resolved scene.
#[derive(Clone, Debug, PartialEq)]
pub struct ModeSi {
    /// Name.
    pub name: String,
    /// Scene shown at mode start.
    pub start_scene: SceneId,
}

/// The `[backbox]` of `backglass.toml` in SI.
#[derive(Clone, Debug, PartialEq)]
pub struct BackboxSi {
    /// Scene glTF path.
    pub scene: String,
    /// Camera eye, m.
    pub camera_position_m: [f32; 3],
    /// Camera target, m.
    pub camera_look_at_m: [f32; 3],
    /// Vertical field of view, rad.
    pub fov_rad: f32,
    /// Collision layer.
    pub collision_layer: u8,
}

/// A validated table: the parsed manifests plus interned, SI runtime data.
#[derive(Clone, Debug, PartialEq)]
pub struct LoadedTable {
    /// `table.toml` as parsed.
    pub manifest: TableManifest,
    /// `backglass.toml` as parsed.
    pub backglass: BackglassManifest,
    /// Name → id tables.
    pub ids: TableIds,
    /// Physics defaults with the table's `[physics]` overrides applied.
    pub physics: PhysicsParams,
    /// Playfield pitch, rad.
    pub pitch_rad: f32,
    /// Flippers (`FlipperId` = index), via `FlipperDef::to_config`.
    pub flippers: Vec<FlipperConfig>,
    /// Stage of each flipper (same order as `flippers`).
    pub flipper_stages: Vec<FlipperStageHint>,
    /// Declared switches.
    pub switches: Vec<SwitchSi>,
    /// Magnets.
    pub magnets: Vec<MagnetSi>,
    /// Splines.
    pub splines: Vec<SplineSi>,
    /// Layer transitions.
    pub transitions: Vec<TransitionSi>,
    /// Dispensers.
    pub dispensers: Vec<DispenserSi>,
    /// Mechs.
    pub mechs: Vec<MechSi>,
    /// Shots.
    pub shots: Vec<ShotSi>,
    /// Modes.
    pub modes: Vec<ModeSi>,
    /// Backbox mini-playfield, if any.
    pub backbox: Option<BackboxSi>,
    /// Non-fatal findings, sorted by (file, path).
    pub warnings: Vec<ValidationError>,
}

fn schema_error(file: &str, e: SchemaError) -> ValidationError {
    match e {
        SchemaError::UnsupportedVersion(v) => ValidationError::new(
            file,
            "schema_version",
            format!(
                "unsupported schema_version {v} (expected {})",
                pin_core::schema::SCHEMA_VERSION
            ),
        ),
        SchemaError::Parse(m) => ValidationError::new(file, "", m.trim_end().to_owned()),
        other => ValidationError::new(file, "", other.to_string()),
    }
}

/// Largest manifest (`table.toml`, `backglass.toml`) accepted, bytes.
pub const MAX_MANIFEST_BYTES: usize = 4 << 20;

fn read_text(vfs: &Vfs, file: &str) -> Result<Result<String, ValidationError>, AssetError> {
    let bytes = vfs.read(file)?;
    if bytes.len() > MAX_MANIFEST_BYTES {
        return Ok(Err(ValidationError::new(
            file,
            "",
            format!(
                "manifest is {} bytes (max {MAX_MANIFEST_BYTES})",
                bytes.len()
            ),
        )));
    }
    Ok(String::from_utf8(bytes)
        .map_err(|e| ValidationError::new(file, "", format!("not valid UTF-8 text: {e}"))))
}

/// Reads, parses and validates `table.toml` and `backglass.toml` at the bundle
/// root. A missing manifest is [`AssetError::NotFound`]; every other problem is
/// collected into [`AssetError::Invalid`], sorted by `(file, path)`.
pub fn load_table(vfs: &Vfs) -> Result<LoadedTable, AssetError> {
    let mut errors = Vec::new();
    let table = match read_text(vfs, TABLE_FILE)? {
        Ok(text) => TableManifest::from_toml_str(&text)
            .map_err(|e| errors.push(schema_error(TABLE_FILE, e)))
            .ok(),
        Err(e) => {
            errors.push(e);
            None
        }
    };
    let backglass = match read_text(vfs, BACKGLASS_FILE)? {
        Ok(text) => BackglassManifest::from_toml_str(&text)
            .map_err(|e| errors.push(schema_error(BACKGLASS_FILE, e)))
            .ok(),
        Err(e) => {
            errors.push(e);
            None
        }
    };
    let mut checker = rules::Checker::new(vfs);
    checker.errors = errors;
    let result = checker.run(table.as_ref(), backglass.as_ref());
    let (mut errors, mut warnings) = (checker.errors, checker.warnings);
    sort(&mut errors);
    sort(&mut warnings);
    match (table, backglass, result) {
        (Some(manifest), Some(backglass), Some(parts)) if errors.is_empty() => {
            Ok(parts.finish(manifest, backglass, warnings))
        }
        _ => Err(AssetError::Invalid(errors)),
    }
}

/// Longest spline the loader accepts, m.
pub const MAX_SPLINE_LENGTH_M: f32 = guide_lint::MAX_SPLINE_LENGTH_M;
/// Most through-points per spline the loader accepts.
pub const MAX_SPLINE_POINTS: usize = rules::MAX_SPLINE_POINTS;

/// Checks one `table.toml` `[[splines]]` entry exactly as [`load_table`]
/// does (construction rules, coordinate and length bounds, spacing and guide
/// lints), reported at `splines[index]…`: returns `(errors, warnings)`, each
/// sorted by path. The ball-profile lint (c) needs the table's profiles and
/// is skipped. Used by `pin_tablegen` to refuse generated guides the loader
/// would reject or warn about (Subphase 4.3).
pub fn check_spline(
    index: usize,
    spline: &pin_core::schema::table::SplineDef,
) -> (Vec<ValidationError>, Vec<ValidationError>) {
    let mut checker = rules::Checker::detached();
    checker.lone_spline(index, spline);
    let (mut errors, mut warnings) = (checker.errors, checker.warnings);
    sort(&mut errors);
    sort(&mut warnings);
    (errors, warnings)
}

/// Nearest name of `kind` within edit distance 2.
pub(crate) fn suggest_name(ids: &TableIds, kind: IdKind, name: &str) -> Option<String> {
    ids::suggest(name, ids.entries(kind).iter().map(|(n, _)| n.as_str())).map(str::to_owned)
}

fn sort(v: &mut [ValidationError]) {
    v.sort_by(|a, b| (&a.file, &a.path).cmp(&(&b.file, &b.path)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_formats() {
        let e = ValidationError::new("table.toml", "flippers[0].coil", "bad");
        assert_eq!(e.to_string(), "table.toml: flippers[0].coil: bad");
        let e = ValidationError::new("table.toml", "", "bad");
        assert_eq!(e.to_string(), "table.toml: bad");
    }

    #[test]
    fn schema_errors_map() {
        let e = schema_error("t", SchemaError::UnsupportedVersion(2));
        assert_eq!(e.path, "schema_version");
        assert!(e.message.contains('2'));
        let e = schema_error("t", SchemaError::Invalid("x".into()));
        assert_eq!(e.path, "");
        let e = schema_error("t", SchemaError::Parse("y\n".into()));
        assert_eq!(e.message, "y");
    }
}
