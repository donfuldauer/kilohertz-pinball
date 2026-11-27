//! `config.toml`: the typed application settings the menu edits.
//!
//! * [`AppConfig::load`] never fails: a missing file gives the defaults
//!   silently (first run); an unreadable, oversized, non-UTF-8 or malformed
//!   file gives the defaults plus a [`ConfigWarning`]; a well-formed file with
//!   out-of-range values keeps the rest and resets each bad value to its
//!   default, with one warning per value.
//! * Unknown keys are **preserved** (DECISIONS.md "4.4 — Menu UI decisions"):
//!   they are kept in the `extra` table of the section they appear in and
//!   written back by [`AppConfig::save`], so a newer build's settings survive a
//!   round trip through an older one. Each unknown key is also reported as a
//!   warning so typos are visible.
//! * [`AppConfig::save`] is atomic and durable: the text is written to a
//!   temporary file in the same directory, flushed to disk, renamed over the
//!   target, and (on Unix) the directory itself is synced so the rename
//!   survives a power cut. The temporary file never outlives a save.
//!   * A symlinked `config.toml` (single or chained, relative or absolute
//!     links) is written **through**: the temporary file is created next to
//!     the link's final target and renamed over that target, so the link stays
//!     a link. A dangling link creates its target. At most
//!     [`MAX_SYMLINK_HOPS`] links are followed; a loop is an error.
//!   * The file mode is kept: on Unix the temporary file is created `0600`
//!     (never wider than the result, even transiently), then given the
//!     existing target's permissions before the rename. A new file stays
//!     `0600` (DECISIONS.md "#131 — config.toml save: symlinks, mode and
//!     directory sync").
//!   * A `tables_dir` / `last_table` that is not valid UTF-8 cannot be written
//!     as TOML text; it is left out of the file (the in-memory value is
//!     unchanged) instead of failing the save.
//! * Text form: every `f32` setting is written as its shortest round-trip
//!   decimal (`deadzone = 0.1`, not `0.10000000149011612`), so a reload gives
//!   the bit-identical value; preserved unknown keys are written back exactly
//!   as read (datetimes stay TOML datetimes, `f64` values are untouched).
//!
//! Units follow the input layer (full-scale units, Hz, ms); `config.toml` is
//! not a table manifest, so the mm/g/deg rule (D17) does not apply.

use pin_core::flipper::FlipperStagingMode;
use pin_core::input::{CabinetAxis, CabinetInput};
use pin_input::{
    DEFAULT_LOWER_THRESHOLD, DEFAULT_UPPER_THRESHOLD, FlipperStage, NudgeConfig, PlungerConfig,
};
use pin_render::topology::{Corner, PipPlacement, TopologyMode};
use serde::{Deserialize, Serialize};
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Conventional file name of the settings file.
pub const CONFIG_FILE: &str = "config.toml";

/// Largest `config.toml` [`AppConfig::load`] reads; a larger file is treated
/// as corrupt (defaults + warning).
pub const MAX_CONFIG_BYTES: u64 = 1 << 20;

/// Input profile value meaning "pick the built-in profile per device"
/// (`pin_input::detect`).
pub const AUTO_PROFILE: &str = "auto";

/// Single window with picture-in-picture, or one window per monitor.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DisplayMode {
    /// One window; backglass and score display are PiP rectangles.
    #[default]
    Single,
    /// Cabinet: playfield, backglass and score display on their own monitors
    /// where assigned (a role without a monitor falls back to a PiP).
    Multi,
}

/// Window corner a PiP is anchored to (mirror of [`Corner`] with serde names).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PipCorner {
    /// Top-left.
    TopLeft,
    /// Top-right.
    TopRight,
    /// Bottom-left.
    BottomLeft,
    /// Bottom-right.
    BottomRight,
}

impl PipCorner {
    /// Every corner, in menu cycling order.
    pub const ALL: [PipCorner; 4] = [
        PipCorner::TopLeft,
        PipCorner::TopRight,
        PipCorner::BottomLeft,
        PipCorner::BottomRight,
    ];

    /// The topology corner.
    pub const fn to_corner(self) -> Corner {
        match self {
            PipCorner::TopLeft => Corner::TopLeft,
            PipCorner::TopRight => Corner::TopRight,
            PipCorner::BottomLeft => Corner::BottomLeft,
            PipCorner::BottomRight => Corner::BottomRight,
        }
    }

    /// Display label.
    pub const fn label(self) -> &'static str {
        match self {
            PipCorner::TopLeft => "Top left",
            PipCorner::TopRight => "Top right",
            PipCorner::BottomLeft => "Bottom left",
            PipCorner::BottomRight => "Bottom right",
        }
    }
}

/// Smallest PiP width fraction the config accepts.
pub const MIN_PIP_WIDTH: f32 = 0.05;
/// Largest PiP width fraction the config accepts.
pub const MAX_PIP_WIDTH: f32 = 1.0;

/// Where a PiP sits and how wide it is.
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PipConfig {
    /// Anchor corner.
    pub corner: PipCorner,
    /// Width as a fraction of the window width, [`MIN_PIP_WIDTH`]..=[`MAX_PIP_WIDTH`].
    pub width_fraction: f32,
}

impl PipConfig {
    /// The topology placement.
    pub fn to_placement(self) -> PipPlacement {
        PipPlacement {
            corner: self.corner.to_corner(),
            width_fraction: self.width_fraction,
        }
    }
}

/// Playfield camera presets.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraPreset {
    /// Perspective view from the player's position, for a desktop monitor.
    #[default]
    Desktop,
    /// Cabinet: a playfield monitor lying flat, viewed from above at the
    /// player's eye point (portrait).
    Cabinet,
    /// Orthographic top-down view of the whole playfield.
    TopDown,
}

impl CameraPreset {
    /// Every preset, in menu cycling order.
    pub const ALL: [CameraPreset; 3] = [
        CameraPreset::Desktop,
        CameraPreset::Cabinet,
        CameraPreset::TopDown,
    ];

    /// Display label.
    pub const fn label(self) -> &'static str {
        match self {
            CameraPreset::Desktop => "Desktop",
            CameraPreset::Cabinet => "Cabinet",
            CameraPreset::TopDown => "Top-down",
        }
    }
}

/// Score display technology the player prefers.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScoreDisplayKind {
    /// Dot-matrix display.
    #[default]
    Dmd,
    /// Full-colour LCD.
    Lcd,
}

impl ScoreDisplayKind {
    /// Display label.
    pub const fn label(self) -> &'static str {
        match self {
            ScoreDisplayKind::Dmd => "DMD",
            ScoreDisplayKind::Lcd => "LCD",
        }
    }
}

/// How cabinet buttons drive the table's flippers.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StagingMode {
    /// Use each flipper's staging from `table.toml`.
    #[default]
    Table,
    /// Every button-driven flipper becomes a dedicated button
    /// (dual-stage flippers use the button of their stage).
    Dedicated,
    /// Analog dual-stage buttons: left/right flippers (lower and upper) follow
    /// the left/right analog axis with the configured thresholds.
    DualStage,
}

impl StagingMode {
    /// Every mode, in menu cycling order.
    pub const ALL: [StagingMode; 3] = [
        StagingMode::Table,
        StagingMode::Dedicated,
        StagingMode::DualStage,
    ];

    /// Display label.
    pub const fn label(self) -> &'static str {
        match self {
            StagingMode::Table => "Table default",
            StagingMode::Dedicated => "Dedicated buttons",
            StagingMode::DualStage => "Dual-stage analog",
        }
    }

    /// The staging a flipper gets under this mode. `table` is the flipper's
    /// staging from `table.toml`, `stage` its stage (lower/upper). Auto
    /// "Thing Flip" flippers and flippers on non-flipper buttons (e.g.
    /// `action`) keep their table staging in every mode.
    pub fn resolve(
        self,
        table: FlipperStagingMode,
        stage: FlipperStage,
        lower_threshold: f32,
        upper_threshold: f32,
    ) -> FlipperStagingMode {
        let side = |input: CabinetInput| match input {
            CabinetInput::LeftFlipper | CabinetInput::LeftUpper => Some(false),
            CabinetInput::RightFlipper | CabinetInput::RightUpper => Some(true),
            _ => None,
        };
        let axis_side = |axis: CabinetAxis| match axis {
            CabinetAxis::LeftFlipper => Some(false),
            CabinetAxis::RightFlipper => Some(true),
            _ => None,
        };
        let flipper_side = match table {
            FlipperStagingMode::DedicatedButton(i) | FlipperStagingMode::ParallelLinked(i) => {
                side(i)
            }
            FlipperStagingMode::DualStageThreshold { axis, .. } => axis_side(axis),
            FlipperStagingMode::AutoSensorThingFlip { .. } => None,
        };
        let Some(right) = flipper_side else {
            return table;
        };
        match self {
            StagingMode::Table => table,
            StagingMode::Dedicated => {
                if let FlipperStagingMode::DedicatedButton(_)
                | FlipperStagingMode::ParallelLinked(_) = table
                {
                    return FlipperStagingMode::DedicatedButton(match table {
                        FlipperStagingMode::DedicatedButton(i)
                        | FlipperStagingMode::ParallelLinked(i) => i,
                        _ => unreachable!(),
                    });
                }
                FlipperStagingMode::DedicatedButton(match (right, stage) {
                    (false, FlipperStage::Lower) => CabinetInput::LeftFlipper,
                    (false, FlipperStage::Upper) => CabinetInput::LeftUpper,
                    (true, FlipperStage::Lower) => CabinetInput::RightFlipper,
                    (true, FlipperStage::Upper) => CabinetInput::RightUpper,
                })
            }
            StagingMode::DualStage => FlipperStagingMode::DualStageThreshold {
                axis: if right {
                    CabinetAxis::RightFlipper
                } else {
                    CabinetAxis::LeftFlipper
                },
                lower_threshold,
                upper_threshold,
            },
        }
    }
}

/// Plunger calibration (`pin_input::PlungerConfig` in config form).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PlungerCalibration {
    /// Deadzone around the rest position, full-scale, 0..=0.5.
    pub deadzone: f32,
    /// Rest tracking rate, 1/s, 0..=100 (0 = off).
    pub rest_tracking_per_s: f32,
    /// Minimum release speed, full-scale/s, 0..=1000.
    pub release_min_speed: f32,
    /// Release speed window, ms, 1..=1024.
    pub release_window_ms: u32,
}

impl Default for PlungerCalibration {
    fn default() -> Self {
        PlungerCalibration::from(PlungerConfig::default())
    }
}

impl From<PlungerConfig> for PlungerCalibration {
    fn from(c: PlungerConfig) -> Self {
        PlungerCalibration {
            deadzone: c.deadzone,
            rest_tracking_per_s: c.rest_tracking_per_s,
            release_min_speed: c.release_min_speed,
            release_window_ms: c.release_window_ms,
        }
    }
}

impl PlungerCalibration {
    /// The filter configuration.
    pub fn to_config(self) -> PlungerConfig {
        PlungerConfig {
            deadzone: self.deadzone,
            rest_tracking_per_s: self.rest_tracking_per_s,
            release_min_speed: self.release_min_speed,
            release_window_ms: self.release_window_ms,
        }
    }
}

/// Accelerometer nudge calibration (`pin_input::NudgeConfig` in config form).
#[derive(Copy, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct NudgeCalibration {
    /// High-pass cut-off, Hz, 0..=50 (0 = off).
    pub high_pass_hz: f32,
    /// Output gain, 0..=10.
    pub gain: f32,
    /// Output curve exponent, 0.1..=5.
    pub curve_exponent: f32,
    /// Per-axis deadzone, 0..=1.
    pub deadzone: f32,
    /// Output clamp, 0..=10.
    pub max: f32,
}

impl Default for NudgeCalibration {
    fn default() -> Self {
        NudgeCalibration::from(NudgeConfig::default())
    }
}

impl From<NudgeConfig> for NudgeCalibration {
    fn from(c: NudgeConfig) -> Self {
        NudgeCalibration {
            high_pass_hz: c.high_pass_hz,
            gain: c.gain,
            curve_exponent: c.curve_exponent,
            deadzone: c.deadzone,
            max: c.max,
        }
    }
}

impl NudgeCalibration {
    /// The filter configuration.
    pub fn to_config(self) -> NudgeConfig {
        NudgeConfig {
            high_pass_hz: self.high_pass_hz,
            gain: self.gain,
            curve_exponent: self.curve_exponent,
            deadzone: self.deadzone,
            max: self.max,
        }
    }
}

/// Calibration values edited live in the input panel.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CalibrationConfig {
    /// Plunger filter.
    pub plunger: PlungerCalibration,
    /// Nudge filter.
    pub nudge: NudgeCalibration,
    /// Dual-stage lower threshold, 0..=1 (≤ upper).
    pub dual_stage_lower: f32,
    /// Dual-stage upper threshold, 0..=1 (≥ lower).
    pub dual_stage_upper: f32,
}

impl Default for CalibrationConfig {
    fn default() -> Self {
        CalibrationConfig {
            plunger: PlungerCalibration::default(),
            nudge: NudgeCalibration::default(),
            dual_stage_lower: DEFAULT_LOWER_THRESHOLD,
            dual_stage_upper: DEFAULT_UPPER_THRESHOLD,
        }
    }
}

/// Default per-bundle decoded-audio budget, MiB (human ruling "decoded-audio
/// budget (512 MiB, user setting)").
pub const DEFAULT_DECODED_BUDGET_MIB: u32 = 512;
/// Smallest accepted decoded-audio budget, MiB.
pub const MIN_DECODED_BUDGET_MIB: u32 = 64;
/// Largest accepted decoded-audio budget, MiB.
pub const MAX_DECODED_BUDGET_MIB: u32 = 8192;

/// Audio settings (`[audio]` in `config.toml`).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioConfig {
    /// Per-bundle decoded-audio safety cap, MiB,
    /// [`MIN_DECODED_BUDGET_MIB`]..=[`MAX_DECODED_BUDGET_MIB`] (default
    /// [`DEFAULT_DECODED_BUDGET_MIB`]). Out-of-range or unparsable values fall
    /// back to the default with a warning. A change applies to the next table
    /// load (the loader builds one `DecodeBudget` per bundle from it).
    pub decoded_budget_mib: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        AudioConfig {
            decoded_budget_mib: DEFAULT_DECODED_BUDGET_MIB,
        }
    }
}

impl AudioConfig {
    /// The budget in bytes (`decoded_budget_mib` × 1 MiB).
    pub fn decoded_budget_bytes(&self) -> u64 {
        u64::from(self.decoded_budget_mib) << 20
    }
}

/// Default telemetry destination (`TELEMETRY_PROTOCOL.md` §1: loopback,
/// port 35120).
pub const DEFAULT_TELEMETRY_ENDPOINT: &str = "127.0.0.1:35120";

/// Longest accepted `telemetry.endpoint`, bytes.
pub const MAX_ENDPOINT_BYTES: usize = 255;

/// UDP telemetry settings (`[telemetry]` in `config.toml`, SPEC §4 3.4,
/// KP-006).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct TelemetryConfig {
    /// `telemetry.enabled`: send the cabinet state as UDP telemetry
    /// (`docs/TELEMETRY_PROTOCOL.md`) while a table runs (default `true`).
    /// A change applies to the next table load.
    pub enabled: bool,
    /// `telemetry.endpoint`: the destination `"host:port"` (IPv4,
    /// `[IPv6]:port` or a host name; default [`DEFAULT_TELEMETRY_ENDPOINT`],
    /// loopback only). An endpoint that does not resolve disables telemetry
    /// for the run with one stderr line. Its port is also the port the DOF
    /// bridge listens on. Empty or longer than [`MAX_ENDPOINT_BYTES`]: the
    /// default, with a warning.
    pub endpoint: String,
}

impl Default for TelemetryConfig {
    fn default() -> Self {
        TelemetryConfig {
            enabled: true,
            endpoint: DEFAULT_TELEMETRY_ENDPOINT.to_owned(),
        }
    }
}

/// DirectOutput Framework bridge settings (`[dof]` in `config.toml`, SPEC
/// §4 3.5, KP-006).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct DofConfig {
    /// `dof.enabled`: start the DOF bridge on table load when the table has
    /// `[dof]`, the bridge is installed next to the engine and telemetry is
    /// enabled (Windows only; default `true`). A change applies to the next
    /// table load.
    pub enabled: bool,
}

impl Default for DofConfig {
    fn default() -> Self {
        DofConfig { enabled: true }
    }
}

/// The application settings stored in `config.toml`.
///
/// Field names are the TOML keys. Every key is optional in the file (missing
/// keys take their defaults); keys this build does not know are kept in
/// [`extra`](Self::extra) and written back unchanged.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Single window with PiP, or one window per monitor.
    pub display_mode: DisplayMode,
    /// Monitor (SDL display index) of the playfield window.
    pub playfield_display: u32,
    /// Monitor of the backglass in [`DisplayMode::Multi`]; `None` = PiP.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub backglass_display: Option<u32>,
    /// Monitor of the score display in [`DisplayMode::Multi`]; `None` = PiP.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score_display_monitor: Option<u32>,
    /// Backglass PiP placement (used whenever the backglass has no monitor).
    /// A key missing from `[backglass_pip]` takes its own default.
    #[serde(deserialize_with = "de_backglass_pip")]
    pub backglass_pip: PipConfig,
    /// Score display PiP placement. A key missing from `[score_pip]` takes
    /// its own default.
    #[serde(deserialize_with = "de_score_pip")]
    pub score_pip: PipConfig,
    /// Playfield camera preset.
    pub camera_preset: CameraPreset,
    /// Preferred score display (a table whose backglass only defines one
    /// mode uses that one).
    pub score_display: ScoreDisplayKind,
    /// Input profile id: [`AUTO_PROFILE`] or a profile id
    /// (`pin_input::builtin_profiles`).
    pub input_profile: String,
    /// Flipper staging override.
    pub staging_mode: StagingMode,
    /// Plunger, nudge and dual-stage calibration.
    pub calibration: CalibrationConfig,
    /// Debug wireframe overlay on the playfield.
    pub wireframe: bool,
    /// Menu scale factor, [`MIN_UI_SCALE`]..=[`MAX_UI_SCALE`] (default
    /// [`DEFAULT_UI_SCALE`]): the menu is laid out at
    /// `pixels_per_point × ui_scale`, so it appears this many times larger
    /// (e.g. 2.0 on a 4K cabinet monitor). Applied live. A non-finite,
    /// out-of-range or wrong-type value falls back to the default with a
    /// warning.
    pub ui_scale: f32,
    /// Audio settings (`[audio]`).
    pub audio: AudioConfig,
    /// UDP telemetry settings (`[telemetry]`).
    pub telemetry: TelemetryConfig,
    /// DOF bridge settings (`[dof]`).
    pub dof: DofConfig,
    /// Folder scanned for table bundles; `None` = `tables/` next to the
    /// executable (resolved by the app). A path that is not valid UTF-8 is
    /// kept in memory but left out of the saved file.
    #[serde(skip_serializing_if = "skip_path")]
    pub tables_dir: Option<PathBuf>,
    /// The table loaded last (set when a load succeeds). A path that is not
    /// valid UTF-8 is kept in memory but left out of the saved file.
    #[serde(skip_serializing_if = "skip_path")]
    pub last_table: Option<PathBuf>,
    /// Keys this build does not know, preserved verbatim at their place in
    /// the file: unknown top-level keys and tables, and unknown keys inside
    /// the known tables (e.g. `calibration.plunger.future_key` is stored as
    /// `extra["calibration"]["plunger"]["future_key"]`). Written back by
    /// [`AppConfig::save`] wherever they do not clash with a known setting.
    #[serde(flatten)]
    pub extra: toml::Table,
}

/// Whether an optional path is left out of the saved file: `None`, or a path
/// TOML text cannot hold (not valid UTF-8).
fn skip_path(p: &Option<PathBuf>) -> bool {
    p.as_deref().is_none_or(|p| p.to_str().is_none())
}

/// A `[*_pip]` table as written: every key optional.
#[derive(Deserialize)]
struct PartialPip {
    corner: Option<PipCorner>,
    width_fraction: Option<f32>,
}

impl PartialPip {
    fn or(self, d: PipConfig) -> PipConfig {
        PipConfig {
            corner: self.corner.unwrap_or(d.corner),
            width_fraction: self.width_fraction.unwrap_or(d.width_fraction),
        }
    }
}

fn de_backglass_pip<'de, D: serde::Deserializer<'de>>(d: D) -> Result<PipConfig, D::Error> {
    Ok(PartialPip::deserialize(d)?.or(AppConfig::default().backglass_pip))
}

fn de_score_pip<'de, D: serde::Deserializer<'de>>(d: D) -> Result<PipConfig, D::Error> {
    Ok(PartialPip::deserialize(d)?.or(AppConfig::default().score_pip))
}

impl Default for AppConfig {
    fn default() -> Self {
        AppConfig {
            display_mode: DisplayMode::Single,
            playfield_display: 0,
            backglass_display: None,
            score_display_monitor: None,
            backglass_pip: PipConfig {
                corner: PipCorner::TopRight,
                width_fraction: 0.25,
            },
            score_pip: PipConfig {
                corner: PipCorner::BottomRight,
                width_fraction: 0.25,
            },
            camera_preset: CameraPreset::Desktop,
            score_display: ScoreDisplayKind::Dmd,
            input_profile: AUTO_PROFILE.to_owned(),
            staging_mode: StagingMode::Table,
            calibration: CalibrationConfig::default(),
            wireframe: false,
            ui_scale: DEFAULT_UI_SCALE,
            audio: AudioConfig::default(),
            telemetry: TelemetryConfig::default(),
            dof: DofConfig::default(),
            tables_dir: None,
            last_table: None,
            extra: toml::Table::new(),
        }
    }
}

/// A problem found while loading `config.toml`. Loading still succeeds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConfigWarning {
    /// The file could not be read (other than "not found").
    Unreadable(String),
    /// The file is larger than [`MAX_CONFIG_BYTES`].
    TooLarge(u64),
    /// The file is not valid TOML / does not match the schema; every setting
    /// fell back to its default. The text is the parser's message.
    Corrupt(String),
    /// One value was out of range or invalid and was reset to its default.
    InvalidValue {
        /// Dotted key, e.g. `calibration.plunger.deadzone`.
        key: String,
        /// What was wrong.
        message: String,
    },
    /// A key this build does not know (preserved, not used).
    UnknownKey(String),
}

impl fmt::Display for ConfigWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigWarning::Unreadable(e) => {
                write!(f, "config.toml could not be read ({e}); using defaults")
            }
            ConfigWarning::TooLarge(n) => write!(
                f,
                "config.toml is {n} bytes (max {MAX_CONFIG_BYTES}); using defaults (not backed up)"
            ),
            ConfigWarning::Corrupt(e) => {
                write!(
                    f,
                    "config.toml is corrupt; using defaults: {}",
                    e.trim_end()
                )
            }
            ConfigWarning::InvalidValue { key, message } => {
                write!(f, "config.toml: {key}: {message}; using the default")
            }
            ConfigWarning::UnknownKey(k) => write!(f, "config.toml: unknown key {k} (kept)"),
        }
    }
}

/// Result of [`AppConfig::load`].
#[derive(Clone, Debug, PartialEq)]
pub struct ConfigLoad {
    /// The settings (defaults where the file was missing or bad).
    pub config: AppConfig,
    /// Problems found; empty for a clean file or a missing one.
    pub warnings: Vec<ConfigWarning>,
}

// Known keys of each table, for the unknown-key warnings.
const TOP_KEYS: &[&str] = &[
    "display_mode",
    "playfield_display",
    "backglass_display",
    "score_display_monitor",
    "backglass_pip",
    "score_pip",
    "camera_preset",
    "score_display",
    "input_profile",
    "staging_mode",
    "calibration",
    "wireframe",
    "ui_scale",
    "audio",
    "telemetry",
    "dof",
    "tables_dir",
    "last_table",
];

/// Dotted paths of every `f32` setting, written as shortest round-trip
/// decimals by [`AppConfig::to_toml_string`].
const F32_KEYS: [&[&str]; 13] = [
    &["backglass_pip", "width_fraction"],
    &["score_pip", "width_fraction"],
    &["calibration", "plunger", "deadzone"],
    &["calibration", "plunger", "rest_tracking_per_s"],
    &["calibration", "plunger", "release_min_speed"],
    &["calibration", "nudge", "high_pass_hz"],
    &["calibration", "nudge", "gain"],
    &["calibration", "nudge", "curve_exponent"],
    &["calibration", "nudge", "deadzone"],
    &["calibration", "nudge", "max"],
    &["calibration", "dual_stage_lower"],
    &["calibration", "dual_stage_upper"],
    &["ui_scale"],
];
const PIP_KEYS: &[&str] = &["corner", "width_fraction"];
const CALIBRATION_KEYS: &[&str] = &["plunger", "nudge", "dual_stage_lower", "dual_stage_upper"];
const PLUNGER_KEYS: &[&str] = &[
    "deadzone",
    "rest_tracking_per_s",
    "release_min_speed",
    "release_window_ms",
];
const NUDGE_KEYS: &[&str] = &["high_pass_hz", "gain", "curve_exponent", "deadzone", "max"];
const AUDIO_KEYS: &[&str] = &["decoded_budget_mib"];
const TELEMETRY_KEYS: &[&str] = &["enabled", "endpoint"];
const DOF_KEYS: &[&str] = &["enabled"];

/// Largest monitor index the config accepts.
pub const MAX_DISPLAY_INDEX: u32 = 15;

/// Default menu scale ([`AppConfig::ui_scale`]): the menu at its designed size.
pub const DEFAULT_UI_SCALE: f32 = 1.0;
/// Smallest accepted [`AppConfig::ui_scale`].
pub const MIN_UI_SCALE: f32 = 0.5;
/// Largest accepted [`AppConfig::ui_scale`] (a 4K cabinet monitor viewed
/// from a distance).
pub const MAX_UI_SCALE: f32 = 4.0;

/// Most symbolic links [`AppConfig::save`] follows from the given path to the
/// file it writes (the usual OS limit); more, or a loop, is an error.
pub const MAX_SYMLINK_HOPS: u32 = 40;

impl AppConfig {
    /// Loads `path`. Never fails; see the module docs for the fallback rules.
    ///
    /// When the file exists but some of its content could not be used (it
    /// is corrupt or too large, or a value was dropped or reset), the
    /// original is first copied to `<name>.bak` next to it (overwriting an
    /// older backup), so the next [`save`](Self::save) cannot lose it. A
    /// failed copy is ignored (the load result is unchanged).
    pub fn load(path: &Path) -> ConfigLoad {
        let l = Self::load_inner(path);
        let lost = l.warnings.iter().any(|w| {
            matches!(
                w,
                ConfigWarning::Corrupt(_)
                    | ConfigWarning::TooLarge(_)
                    | ConfigWarning::InvalidValue { .. }
            )
        });
        // Back up once: an existing `.bak` (the oldest known-bad original)
        // is kept, and an oversized file is never copied (its warning says
        // so), so the backup is bounded by `MAX_CONFIG_BYTES`.
        let too_large = l
            .warnings
            .iter()
            .any(|w| matches!(w, ConfigWarning::TooLarge(_)));
        if lost
            && !too_large
            && let Some(bak) = Self::recovery_path(path)
            && !bak.exists()
        {
            let _ = std::fs::copy(path, bak);
        }
        l
    }

    fn load_inner(path: &Path) -> ConfigLoad {
        let defaults = |w: ConfigWarning| ConfigLoad {
            config: AppConfig::default(),
            warnings: vec![w],
        };
        let file = match std::fs::File::open(path) {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return ConfigLoad {
                    config: AppConfig::default(),
                    warnings: Vec::new(),
                };
            }
            Err(e) => return defaults(ConfigWarning::Unreadable(e.to_string())),
        };
        let mut bytes = Vec::new();
        // Read one byte past the cap so an oversized file is detected without
        // reading all of it.
        use std::io::Read;
        if let Err(e) = file.take(MAX_CONFIG_BYTES + 1).read_to_end(&mut bytes) {
            return defaults(ConfigWarning::Unreadable(e.to_string()));
        }
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            let size = std::fs::metadata(path)
                .map(|m| m.len())
                .unwrap_or(bytes.len() as u64);
            return defaults(ConfigWarning::TooLarge(size));
        }
        AppConfig::from_toml_bytes(&bytes)
    }

    /// Parses `config.toml` bytes (the same rules as [`load`](Self::load)
    /// after the file was read). Never panics.
    pub fn from_toml_bytes(bytes: &[u8]) -> ConfigLoad {
        if bytes.len() as u64 > MAX_CONFIG_BYTES {
            return ConfigLoad {
                config: AppConfig::default(),
                warnings: vec![ConfigWarning::TooLarge(bytes.len() as u64)],
            };
        }
        match std::str::from_utf8(bytes) {
            Ok(text) => AppConfig::from_toml_str(text),
            Err(e) => ConfigLoad {
                config: AppConfig::default(),
                warnings: vec![ConfigWarning::Corrupt(format!("not valid UTF-8: {e}"))],
            },
        }
    }

    /// Parses `config.toml` text. Never panics.
    pub fn from_toml_str(text: &str) -> ConfigLoad {
        let mut warnings = Vec::new();
        let raw = match toml::from_str::<toml::Table>(text) {
            Ok(raw) => raw,
            Err(e) => {
                return ConfigLoad {
                    config: AppConfig::default(),
                    warnings: vec![ConfigWarning::Corrupt(truncate_message(e.to_string()))],
                };
            }
        };
        // Values the ruling says fall back individually even when they do not
        // parse (`audio.decoded_budget_mib`), instead of making the whole
        // file corrupt.
        let mut typed = raw.clone();
        lenient_audio(&mut typed, &mut warnings);
        // Any other known value of the wrong type (or an unknown enum
        // variant) is dropped alone: that setting takes its default, the
        // rest of the file is kept (4.4 review N1).
        let typed = drop_bad_values(typed, &mut warnings);
        let mut config = match toml::Value::Table(typed).try_into::<AppConfig>() {
            Ok(mut c) => {
                c.sanitize(&mut warnings);
                c
            }
            Err(e) => {
                // Not expected after `drop_bad_values`; kept as a safety net.
                return ConfigLoad {
                    config: AppConfig::default(),
                    warnings: vec![ConfigWarning::Corrupt(truncate_message(e.to_string()))],
                };
            }
        };
        // Unknown keys: every top-level extra, plus the nested tables'
        // unknown keys (serde ignores those; report them, and keep them by
        // re-reading the raw table).
        collect_unknown(&raw, &mut config, &mut warnings);
        ConfigLoad { config, warnings }
    }

    /// The TOML text [`save`](Self::save) writes.
    ///
    /// Preserved unknown keys ([`extra`](Self::extra)) are merged in where
    /// they do not clash with a known setting, exactly as they were read
    /// (datetimes stay datetimes). Every `f32` setting is written as its
    /// shortest round-trip decimal. A `tables_dir` / `last_table` that is not
    /// valid UTF-8 is left out (DECISIONS.md "#131 — Non-UTF-8 paths are
    /// left out of config.toml"). No error is expected; the `Result` is the
    /// TOML serialiser's.
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        let mut known = self.clone();
        let extra = std::mem::take(&mut known.extra);
        let mut doc = toml::Table::try_from(&known)?;
        for path in F32_KEYS {
            shorten_f32(&mut doc, path);
        }
        merge_missing(&mut doc, &extra);
        toml::to_string(&doc)
    }

    /// The recovery copy [`load`](Self::load) makes of a configuration file
    /// at `path` whose content could not all be used: `<name>.bak` next to
    /// it. `None` when `path` has no file name.
    pub fn recovery_path(path: &Path) -> Option<PathBuf> {
        let mut bak = path.file_name()?.to_os_string();
        bak.push(".bak");
        Some(path.with_file_name(bak))
    }

    /// The temporary file [`save`](Self::save) writes a configuration file
    /// at `path` through before renaming it over `path`: `.<name>.tmp` next
    /// to it. `None` when `path` has no file name.
    pub fn save_scratch_path(path: &Path) -> Option<PathBuf> {
        let mut tmp = std::ffi::OsString::from(".");
        tmp.push(path.file_name()?);
        tmp.push(".tmp");
        Some(path.with_file_name(tmp))
    }

    /// Writes the settings to `path` atomically and durably: a temporary
    /// file (`.<name>.tmp`) next to the file finally written, `sync_all`, a
    /// rename over that file, then (Unix) a sync of its directory.
    ///
    /// * `path` may be a symlink (chained, relative or absolute, or
    ///   dangling): the link's final target is written and the link stays
    ///   a link. More than [`MAX_SYMLINK_HOPS`] links (or a loop) is an
    ///   error and nothing is written.
    /// * Unix: the temporary file is created with mode `0600` and given the
    ///   existing target's permissions before the rename, so the file mode
    ///   is kept (a new file is `0600`) and never widened, even transiently.
    /// * On error before the rename the previous file is untouched; the
    ///   temporary file is removed in every case. A failure to sync the
    ///   directory after the rename is reported (the new settings are in
    ///   place but may not survive a power cut), except where the platform
    ///   or file system refuses directory syncs (ignored).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = self
            .to_toml_string()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        let target = resolve_symlinks(path)?;
        // The scratch file sits next to the file finally written (the
        // resolved target); `save_scratch_path` is its one computation.
        let tmp = Self::save_scratch_path(&target).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "config path has no file name",
            )
        })?;
        let write = || -> std::io::Result<()> {
            let mut f = create_temp(&tmp)?;
            f.write_all(text.as_bytes())?;
            keep_mode(&f, &target)?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, &target)
        };
        let result = write();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result?;
        sync_parent_dir(&target)
    }

    /// The display topology these settings describe.
    pub fn topology_mode(&self) -> TopologyMode {
        let display = self.playfield_display as usize;
        match self.display_mode {
            DisplayMode::Single => TopologyMode::SingleMonitor {
                display,
                backglass: self.backglass_pip.to_placement(),
                score: self.score_pip.to_placement(),
            },
            DisplayMode::Multi => TopologyMode::Cabinet {
                playfield: display,
                backglass: self.backglass_display.map(|d| d as usize),
                score: self.score_display_monitor.map(|d| d as usize),
            },
        }
    }

    /// Resets every out-of-range value to its default, pushing one
    /// [`ConfigWarning::InvalidValue`] each.
    pub fn sanitize(&mut self, warnings: &mut Vec<ConfigWarning>) {
        let d = AppConfig::default();
        let w = warnings;
        if self.playfield_display > MAX_DISPLAY_INDEX {
            bad(w, "playfield_display", monitor_msg(self.playfield_display));
            self.playfield_display = d.playfield_display;
        }
        for (key, v) in [
            ("backglass_display", &mut self.backglass_display),
            ("score_display_monitor", &mut self.score_display_monitor),
        ] {
            if let Some(i) = *v
                && i > MAX_DISPLAY_INDEX
            {
                bad(w, key, monitor_msg(i));
                *v = None;
            }
        }
        let (pw, sw) = (MIN_PIP_WIDTH, MAX_PIP_WIDTH);
        let c = &mut self.calibration;
        let dc = &d.calibration;
        let checks: [(&str, &mut f32, f32, f32, f32); 13] = [
            (
                "ui_scale",
                &mut self.ui_scale,
                MIN_UI_SCALE,
                MAX_UI_SCALE,
                d.ui_scale,
            ),
            (
                "backglass_pip.width_fraction",
                &mut self.backglass_pip.width_fraction,
                pw,
                sw,
                d.backglass_pip.width_fraction,
            ),
            (
                "score_pip.width_fraction",
                &mut self.score_pip.width_fraction,
                pw,
                sw,
                d.score_pip.width_fraction,
            ),
            (
                "calibration.plunger.deadzone",
                &mut c.plunger.deadzone,
                0.0,
                0.5,
                dc.plunger.deadzone,
            ),
            (
                "calibration.plunger.rest_tracking_per_s",
                &mut c.plunger.rest_tracking_per_s,
                0.0,
                100.0,
                dc.plunger.rest_tracking_per_s,
            ),
            (
                "calibration.plunger.release_min_speed",
                &mut c.plunger.release_min_speed,
                0.0,
                1000.0,
                dc.plunger.release_min_speed,
            ),
            (
                "calibration.nudge.high_pass_hz",
                &mut c.nudge.high_pass_hz,
                0.0,
                50.0,
                dc.nudge.high_pass_hz,
            ),
            (
                "calibration.nudge.gain",
                &mut c.nudge.gain,
                0.0,
                10.0,
                dc.nudge.gain,
            ),
            (
                "calibration.nudge.curve_exponent",
                &mut c.nudge.curve_exponent,
                0.1,
                5.0,
                dc.nudge.curve_exponent,
            ),
            (
                "calibration.nudge.deadzone",
                &mut c.nudge.deadzone,
                0.0,
                1.0,
                dc.nudge.deadzone,
            ),
            (
                "calibration.nudge.max",
                &mut c.nudge.max,
                0.0,
                10.0,
                dc.nudge.max,
            ),
            (
                "calibration.dual_stage_lower",
                &mut c.dual_stage_lower,
                0.0,
                1.0,
                dc.dual_stage_lower,
            ),
            (
                "calibration.dual_stage_upper",
                &mut c.dual_stage_upper,
                0.0,
                1.0,
                dc.dual_stage_upper,
            ),
        ];
        for (key, v, lo, hi, dv) in checks {
            if !(v.is_finite() && *v >= lo && *v <= hi) {
                bad(w, key, format!("{v} is outside {lo}..={hi}"));
                *v = dv;
            }
        }
        let max_ms = pin_input::calibration::MAX_RELEASE_WINDOW_MS;
        if !(1..=max_ms).contains(&c.plunger.release_window_ms) {
            bad(
                w,
                "calibration.plunger.release_window_ms",
                format!("{} is outside 1..={max_ms}", c.plunger.release_window_ms),
            );
            c.plunger.release_window_ms = dc.plunger.release_window_ms;
        }
        if c.dual_stage_lower > c.dual_stage_upper {
            bad(
                w,
                "calibration.dual_stage_lower",
                format!(
                    "lower threshold {} is above the upper threshold {}",
                    c.dual_stage_lower, c.dual_stage_upper
                ),
            );
            c.dual_stage_lower = dc.dual_stage_lower;
            c.dual_stage_upper = dc.dual_stage_upper;
        }
        let budget = self.audio.decoded_budget_mib;
        if !(MIN_DECODED_BUDGET_MIB..=MAX_DECODED_BUDGET_MIB).contains(&budget) {
            bad(
                w,
                "audio.decoded_budget_mib",
                format!(
                    "{budget} MiB is outside {MIN_DECODED_BUDGET_MIB}..={MAX_DECODED_BUDGET_MIB}"
                ),
            );
            self.audio.decoded_budget_mib = d.audio.decoded_budget_mib;
        }
        let endpoint = &self.telemetry.endpoint;
        if endpoint.trim().is_empty() || endpoint.len() > MAX_ENDPOINT_BYTES {
            bad(
                w,
                "telemetry.endpoint",
                format!("must be \"host:port\" (1..={MAX_ENDPOINT_BYTES} bytes)"),
            );
            self.telemetry.endpoint = d.telemetry.endpoint.clone();
        }
        if self.input_profile.trim().is_empty() || self.input_profile.len() > 128 {
            bad(
                w,
                "input_profile",
                "must be \"auto\" or a profile id (1..=128 bytes)".to_owned(),
            );
            self.input_profile = d.input_profile;
        }
    }
}

/// The `f32` `x` as the TOML float whose shortest decimal form reads back as
/// exactly `x`: the shortest round-trip decimal of `x` as an `f32` (Rust's
/// `Display`), parsed as `f64`. If that `f64` would not narrow back to the
/// same bits (never observed; kept as a guard), `x` widened exactly is used.
fn f32_value(x: f32) -> f64 {
    if !x.is_finite() {
        return f64::from(x);
    }
    match x.to_string().parse::<f64>() {
        Ok(d) if (d as f32).to_bits() == x.to_bits() => d,
        _ => f64::from(x),
    }
}

/// Rewrites the float at the dotted `path` of `doc` (an `f32` setting
/// widened to `f64` by the serialiser) as its shortest round-trip decimal.
fn shorten_f32(doc: &mut toml::Table, path: &[&str]) {
    let Some((last, parents)) = path.split_last() else {
        return;
    };
    let mut t = doc;
    for key in parents {
        match t.get_mut(*key) {
            Some(toml::Value::Table(next)) => t = next,
            _ => return,
        }
    }
    if let Some(toml::Value::Float(v)) = t.get_mut(*last) {
        *v = f32_value(*v as f32);
    }
}

/// The file `path` names after following symbolic links (at most
/// [`MAX_SYMLINK_HOPS`]): `path` itself when it is not a link, else the final
/// target (which may not exist yet: a dangling link). Relative link targets
/// are resolved against the link's directory.
fn resolve_symlinks(path: &Path) -> std::io::Result<PathBuf> {
    let mut p = path.to_path_buf();
    let mut hops = 0;
    loop {
        match std::fs::symlink_metadata(&p) {
            Ok(m) if m.file_type().is_symlink() => {
                if hops == MAX_SYMLINK_HOPS {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidInput,
                        format!(
                            "{}: more than {MAX_SYMLINK_HOPS} symbolic links (a loop?)",
                            path.display()
                        ),
                    ));
                }
                hops += 1;
                let link = std::fs::read_link(&p)?;
                p = match p.parent() {
                    Some(dir) if !link.is_absolute() => dir.join(link),
                    _ => link,
                };
            }
            Ok(_) => return Ok(p),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(p),
            Err(e) => return Err(e),
        }
    }
}

/// Creates the temporary file of a save. Unix: a left-over temporary file is
/// removed first and the new one is created exclusively with mode `0600`, so
/// its content is never readable by others, whatever the umask.
#[cfg(unix)]
fn create_temp(tmp: &Path) -> std::io::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    let _ = std::fs::remove_file(tmp);
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(tmp)
}

/// Creates the temporary file of a save.
#[cfg(not(unix))]
fn create_temp(tmp: &Path) -> std::io::Result<std::fs::File> {
    std::fs::File::create(tmp)
}

/// Gives the temporary file `f` the permission bits of the existing regular
/// file `target` (Unix; nothing when `target` does not exist yet).
#[cfg(unix)]
fn keep_mode(f: &std::fs::File, target: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    match std::fs::metadata(target) {
        Ok(m) if m.is_file() => f.set_permissions(std::fs::Permissions::from_mode(
            m.permissions().mode() & 0o7777,
        )),
        _ => Ok(()),
    }
}

/// Keeps the file mode (Unix only; nothing to do elsewhere).
#[cfg(not(unix))]
fn keep_mode(_f: &std::fs::File, _target: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Syncs the directory holding `target` so a rename into it is durable
/// (Unix). A directory that cannot be opened for reading (permission) or a
/// file system that refuses directory syncs (`EINVAL`, unsupported) is
/// ignored; other errors are returned.
#[cfg(unix)]
fn sync_parent_dir(target: &Path) -> std::io::Result<()> {
    use std::io::ErrorKind;
    let dir = match target.parent() {
        Some(d) if !d.as_os_str().is_empty() => d,
        _ => Path::new("."),
    };
    let d = match std::fs::File::open(dir) {
        Ok(d) => d,
        Err(e) if e.kind() == ErrorKind::PermissionDenied => return Ok(()),
        Err(e) => return Err(e),
    };
    match d.sync_all() {
        Err(e) if matches!(e.kind(), ErrorKind::InvalidInput | ErrorKind::Unsupported) => Ok(()),
        r => r,
    }
}

/// Directory sync is a Unix step; elsewhere the rename is the last step.
#[cfg(not(unix))]
fn sync_parent_dir(_target: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Drops an `[audio]` value that would not deserialize (wrong type, negative,
/// too large for `u32`) or an `audio` key that is not a table, with a warning,
/// so it takes its default instead of failing the whole file.
fn lenient_audio(raw: &mut toml::Table, w: &mut Vec<ConfigWarning>) {
    match raw.get_mut("audio") {
        None => {}
        Some(toml::Value::Table(audio)) => {
            if let Some(v) = audio.get("decoded_budget_mib") {
                let ok = v.as_integer().is_some_and(|i| u32::try_from(i).is_ok());
                if !ok {
                    bad(
                        w,
                        "audio.decoded_budget_mib",
                        format!("{v} is not a whole number of MiB"),
                    );
                    audio.remove("decoded_budget_mib");
                }
            }
        }
        Some(v) => {
            bad(w, "audio", format!("{v} is not a table"));
            raw.remove("audio");
        }
    }
}

/// Longest parser message kept in [`ConfigWarning`] texts, characters.
pub const MAX_WARNING_CHARS: usize = 512;

/// Cuts `m` to [`MAX_WARNING_CHARS`] characters (marking the cut with `…`).
fn truncate_message(m: String) -> String {
    match m.char_indices().nth(MAX_WARNING_CHARS) {
        Some((i, _)) => format!("{}…", &m[..i]),
        None => m,
    }
}

/// Whether `raw` alone deserializes as an [`AppConfig`].
fn deserializes(raw: &toml::Table) -> Result<(), String> {
    toml::Value::Table(raw.clone())
        .try_into::<AppConfig>()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

/// `value` wrapped in the tables of `path` (`["a", "b"]` → `{a = {b = value}}`).
fn wrap(path: &[&str], value: toml::Value) -> toml::Table {
    let mut v = value;
    for key in path[1..].iter().rev() {
        let mut t = toml::Table::new();
        t.insert((*key).to_owned(), v);
        v = toml::Value::Table(t);
    }
    let mut t = toml::Table::new();
    t.insert(path[0].to_owned(), v);
    t
}

/// Drops the value at `path` (a known table key) when it is not a table.
fn drop_non_table(raw: &mut toml::Table, path: &[&str], w: &mut Vec<ConfigWarning>) {
    let (last, parents) = path.split_last().expect("non-empty path");
    let mut t = raw;
    for key in parents {
        match t.get_mut(*key) {
            Some(toml::Value::Table(next)) => t = next,
            _ => return,
        }
    }
    if let Some(v) = t.get(*last)
        && !v.is_table()
    {
        let kind = v.type_str();
        bad(
            w,
            &path.join("."),
            format!("expected a table, found {kind}"),
        );
        t.remove(*last);
    }
}

/// Keeps every known value of `raw` that deserializes on its own (checked
/// per top-level key, then per key inside a failing known section of
/// [`SECTIONS`]) and drops the rest with one [`ConfigWarning::InvalidValue`] each.
/// Unknown keys are kept untouched (they end up in `extra`).
fn drop_bad_values(mut raw: toml::Table, w: &mut Vec<ConfigWarning>) -> toml::Table {
    // serde accepts an array of tables (`[[calibration]]`) where a struct is
    // expected; every known table key must hold a table (4.4 review N13).
    for (path, _) in SECTIONS {
        drop_non_table(&mut raw, path, w);
    }
    if deserializes(&raw).is_ok() {
        return raw;
    }
    fn clean(
        path: &mut Vec<String>,
        value: toml::Value,
        w: &mut Vec<ConfigWarning>,
    ) -> Option<toml::Value> {
        let keys: Vec<&str> = path.iter().map(String::as_str).collect();
        let err = match deserializes(&wrap(&keys, value.clone())) {
            Ok(()) => return Some(value),
            Err(e) => e,
        };
        // Only a known section is cleaned key by key; a table where a scalar
        // setting belongs (`wireframe = {}`) is dropped whole, or the cleaned
        // (still invalid) table would fail the final conversion and reset
        // every setting.
        let section = SECTIONS.iter().any(|(p, _)| *p == keys.as_slice());
        match value {
            toml::Value::Table(t) if section => {
                let mut out = toml::Table::new();
                for (k, v) in t {
                    path.push(k.clone());
                    if let Some(v) = clean(path, v, w) {
                        out.insert(k, v);
                    }
                    path.pop();
                }
                Some(toml::Value::Table(out))
            }
            v => {
                let key = keys.join(".");
                bad(
                    w,
                    &key,
                    truncate_message(format!("{v} is not valid here ({})", err.trim_end())),
                );
                None
            }
        }
    }
    let mut out = toml::Table::new();
    for (k, v) in raw {
        if !TOP_KEYS.contains(&k.as_str()) {
            out.insert(k, v);
            continue;
        }
        let mut path = vec![k.clone()];
        if let Some(v) = clean(&mut path, v, w) {
            out.insert(k, v);
        }
    }
    out
}

fn bad(w: &mut Vec<ConfigWarning>, key: &str, message: String) {
    w.push(ConfigWarning::InvalidValue {
        key: key.to_owned(),
        message,
    });
}

fn monitor_msg(i: u32) -> String {
    format!("{i} is not a monitor index 0..={MAX_DISPLAY_INDEX}")
}

/// Known keys of the nested tables (dotted path, keys).
const SECTIONS: [(&[&str], &[&str]); 8] = [
    (&["backglass_pip"], PIP_KEYS),
    (&["score_pip"], PIP_KEYS),
    (&["calibration"], CALIBRATION_KEYS),
    (&["calibration", "plunger"], PLUNGER_KEYS),
    (&["calibration", "nudge"], NUDGE_KEYS),
    (&["audio"], AUDIO_KEYS),
    (&["telemetry"], TELEMETRY_KEYS),
    (&["dof"], DOF_KEYS),
];

/// Adds every key of `src` missing from `dst`, recursing into tables both
/// have. Keys `dst` already has (known settings) win.
fn merge_missing(dst: &mut toml::Table, src: &toml::Table) {
    for (k, v) in src {
        match (dst.get_mut(k), v) {
            (None, _) => {
                dst.insert(k.clone(), v.clone());
            }
            (Some(toml::Value::Table(d)), toml::Value::Table(s)) => merge_missing(d, s),
            (Some(_), _) => {}
        }
    }
}

/// Reports unknown keys and stores the nested ones in `config.extra` at their
/// paths (top-level ones are already there through `#[serde(flatten)]`).
fn collect_unknown(raw: &toml::Table, config: &mut AppConfig, warnings: &mut Vec<ConfigWarning>) {
    for (key, value) in raw {
        if !TOP_KEYS.contains(&key.as_str()) {
            warnings.push(ConfigWarning::UnknownKey(key.clone()));
            // Kept exactly as parsed: the copy `#[serde(flatten)]` made went
            // through serde's buffered form, which turns TOML datetimes
            // into tables (so they would be written back as tables).
            config.extra.insert(key.clone(), value.clone());
        }
    }
    for (path, known) in SECTIONS {
        let mut v = Some(raw);
        for key in path {
            v = v.and_then(|t| t.get(*key)).and_then(|x| x.as_table());
        }
        let Some(table) = v else { continue };
        let mut unknown = toml::Table::new();
        for (k, val) in table {
            if !known.contains(&k.as_str()) {
                warnings.push(ConfigWarning::UnknownKey(format!("{}.{k}", path.join("."))));
                unknown.insert(k.clone(), val.clone());
            }
        }
        if unknown.is_empty() {
            continue;
        }
        // Wrap `unknown` in its section path and merge it into `extra`.
        let mut wrapped = unknown;
        for key in path.iter().rev() {
            let mut t = toml::Table::new();
            t.insert((*key).to_owned(), toml::Value::Table(wrapped));
            wrapped = t;
        }
        merge_missing(&mut config.extra, &wrapped);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// KP-006 (SPEC §4 3.4 / 3.5): `[telemetry]` and `[dof]` default to
    /// enabled, loopback port 35120; missing keys take their defaults.
    #[test]
    fn kp006_telemetry_and_dof_defaults() {
        let d = AppConfig::default();
        assert!(d.telemetry.enabled && d.dof.enabled);
        assert_eq!(d.telemetry.endpoint, "127.0.0.1:35120");
        assert_eq!(d.telemetry.endpoint, DEFAULT_TELEMETRY_ENDPOINT);
        let l = AppConfig::from_toml_str("[telemetry]\nenabled = false\n");
        assert_eq!(l.warnings, []);
        assert!(!l.config.telemetry.enabled);
        assert_eq!(l.config.telemetry.endpoint, DEFAULT_TELEMETRY_ENDPOINT);
        assert!(l.config.dof.enabled);
    }

    /// KP-006: the keys round-trip through `to_toml_string`, and unknown
    /// keys inside the new sections are kept and reported.
    #[test]
    fn kp006_telemetry_and_dof_round_trip() {
        let mut c = AppConfig::default();
        c.telemetry.enabled = false;
        c.telemetry.endpoint = "[::1]:40000".into();
        c.dof.enabled = false;
        let text = c.to_toml_string().unwrap();
        assert!(
            text.contains("[telemetry]") && text.contains("[dof]"),
            "{text}"
        );
        let l = AppConfig::from_toml_str(&text);
        assert_eq!(l.warnings, []);
        assert_eq!(l.config, c);
        let l = AppConfig::from_toml_str("[telemetry]\nfuture = 1\n[dof]\nfuture = 2\n");
        assert_eq!(
            l.warnings,
            [
                ConfigWarning::UnknownKey("telemetry.future".into()),
                ConfigWarning::UnknownKey("dof.future".into()),
            ]
        );
        let back = AppConfig::from_toml_str(&l.config.to_toml_string().unwrap());
        assert_eq!(back.config, l.config);
    }

    /// KP-006: a value of the wrong type, a section that is not a table or
    /// an empty endpoint takes its default with a warning; the rest of the
    /// file is kept.
    #[test]
    fn kp006_wrong_values_fall_back_with_a_warning() {
        let l = AppConfig::from_toml_str(
            "wireframe = true\n[telemetry]\nenabled = \"yes\"\nendpoint = 5\n[dof]\nenabled = 1\n",
        );
        assert!(l.config.wireframe);
        assert_eq!(l.config.telemetry, TelemetryConfig::default());
        assert_eq!(l.config.dof, DofConfig::default());
        let keys: Vec<&str> = l
            .warnings
            .iter()
            .filter_map(|w| match w {
                ConfigWarning::InvalidValue { key, .. } => Some(key.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(
            keys,
            ["dof.enabled", "telemetry.enabled", "telemetry.endpoint"]
        );
        let l = AppConfig::from_toml_str("telemetry = 3\ndof = \"off\"\n");
        assert_eq!(l.config, AppConfig::default());
        assert_eq!(l.warnings.len(), 2, "{:?}", l.warnings);
        for bad in ["", "   ", &"x".repeat(MAX_ENDPOINT_BYTES + 1)] {
            let l = AppConfig::from_toml_str(&format!("[telemetry]\nendpoint = {bad:?}\n"));
            assert_eq!(l.config.telemetry.endpoint, DEFAULT_TELEMETRY_ENDPOINT);
            assert!(
                l.warnings.iter().any(|w| matches!(
                    w,
                    ConfigWarning::InvalidValue { key, .. } if key == "telemetry.endpoint"
                )),
                "{:?}",
                l.warnings
            );
        }
    }

    /// A fresh scratch directory under the system temp dir, removed on drop.
    struct Scratch(PathBuf);

    impl Scratch {
        fn new(tag: &str) -> Scratch {
            use std::sync::atomic::{AtomicU32, Ordering};
            static N: AtomicU32 = AtomicU32::new(0);
            let dir = std::env::temp_dir().join(format!(
                "kp_cfg131_{tag}_{}_{}",
                std::process::id(),
                N.fetch_add(1, Ordering::Relaxed)
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();
            Scratch(dir)
        }

        fn join(&self, p: impl AsRef<Path>) -> PathBuf {
            self.0.join(p)
        }

        /// Sorted entry names (to spot a left-over temporary file).
        fn names(&self, sub: &str) -> Vec<String> {
            let mut v: Vec<String> = std::fs::read_dir(self.0.join(sub))
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            v.sort();
            v
        }
    }

    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn custom() -> AppConfig {
        let mut c = AppConfig {
            wireframe: true,
            ui_scale: 2.0,
            ..AppConfig::default()
        };
        c.calibration.plunger.deadzone = 0.1;
        c
    }

    fn invalid_keys(w: &[ConfigWarning]) -> Vec<&str> {
        w.iter()
            .filter_map(|w| match w {
                ConfigWarning::InvalidValue { key, .. } => Some(key.as_str()),
                _ => None,
            })
            .collect()
    }

    /// SPEC §4 Subphase 4.4 (#131): saving through a symlink writes the
    /// link's target, the link stays a link and no temporary file is left in
    /// either directory.
    #[cfg(unix)]
    #[test]
    fn spec_4_4_save_writes_through_a_symlink() {
        let s = Scratch::new("symlink");
        std::fs::create_dir_all(s.join("real")).unwrap();
        std::fs::create_dir_all(s.join("links")).unwrap();
        let target = s.join("real/config.toml");
        AppConfig::default().save(&target).unwrap();
        let link = s.join("links/config.toml");
        // Relative link target.
        std::os::unix::fs::symlink("../real/config.toml", &link).unwrap();
        custom().save(&link).unwrap();
        assert!(
            std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(AppConfig::load(&target).config, custom());
        assert_eq!(AppConfig::load(&link).config, custom());
        assert_eq!(s.names("real"), ["config.toml"]);
        assert_eq!(s.names("links"), ["config.toml"]);
        // Absolute link target.
        let abs = s.join("abs.toml");
        std::os::unix::fs::symlink(&target, &abs).unwrap();
        AppConfig::default().save(&abs).unwrap();
        assert!(std::fs::symlink_metadata(&abs).unwrap().is_symlink());
        assert_eq!(AppConfig::load(&target).config, AppConfig::default());
    }

    /// SPEC §4 Subphase 4.4 (#131): chained links are followed to the end; a
    /// dangling link creates its target.
    #[cfg(unix)]
    #[test]
    fn spec_4_4_save_follows_chained_and_dangling_symlinks() {
        let s = Scratch::new("chain");
        let target = s.join("final.toml");
        std::os::unix::fs::symlink("final.toml", s.join("b.toml")).unwrap();
        std::os::unix::fs::symlink(s.join("b.toml"), s.join("a.toml")).unwrap();
        // Dangling: `final.toml` does not exist yet.
        custom().save(&s.join("a.toml")).unwrap();
        assert!(target.is_file());
        for l in ["a.toml", "b.toml"] {
            assert!(std::fs::symlink_metadata(s.join(l)).unwrap().is_symlink());
        }
        assert_eq!(AppConfig::load(&target).config, custom());
        assert_eq!(s.names(""), ["a.toml", "b.toml", "final.toml"]);
    }

    /// SPEC §4 Subphase 4.4 (#131): a symlink loop (or a chain longer than
    /// `MAX_SYMLINK_HOPS`) is an error and writes nothing.
    #[cfg(unix)]
    #[test]
    fn spec_4_4_save_rejects_a_symlink_loop() {
        let s = Scratch::new("loop");
        std::os::unix::fs::symlink("y.toml", s.join("x.toml")).unwrap();
        std::os::unix::fs::symlink("x.toml", s.join("y.toml")).unwrap();
        assert!(custom().save(&s.join("x.toml")).is_err());
        assert_eq!(s.names(""), ["x.toml", "y.toml"]);
        // A chain of exactly MAX_SYMLINK_HOPS links is fine, one more is not.
        let n = MAX_SYMLINK_HOPS as usize;
        for i in 0..=n {
            std::os::unix::fs::symlink(format!("l{}", i + 1), s.join(format!("l{i}"))).unwrap();
        }
        assert!(custom().save(&s.join("l1")).is_ok());
        assert!(s.join(format!("l{}", n + 1)).is_file());
        std::fs::remove_file(s.join(format!("l{}", n + 1))).unwrap();
        assert!(custom().save(&s.join("l0")).is_err());
        assert!(!s.join(format!("l{}", n + 1)).exists());
    }

    /// SPEC §4 Subphase 4.4 (#131): an existing file's mode is kept (0600
    /// stays 0600, also through a symlink); a new file is not readable by
    /// group or others.
    #[cfg(unix)]
    #[test]
    fn spec_4_4_save_keeps_the_file_mode() {
        use std::os::unix::fs::PermissionsExt;
        let s = Scratch::new("mode");
        let path = s.join("config.toml");
        custom().save(&path).unwrap();
        let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&path) & 0o077, 0, "new file is {:o}", mode(&path));
        for m in [0o600, 0o640, 0o644] {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(m)).unwrap();
            AppConfig::default().save(&path).unwrap();
            assert_eq!(mode(&path), m);
        }
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = s.join("link.toml");
        std::os::unix::fs::symlink(&path, &link).unwrap();
        custom().save(&link).unwrap();
        assert_eq!(mode(&path), 0o600);
        assert_eq!(AppConfig::load(&path).config, custom());
        assert_eq!(s.names(""), ["config.toml", "link.toml"]);
    }

    /// SPEC §4 Subphase 4.4 (#131): no temporary file survives a save, a
    /// failed save, or a stale one left by a crash.
    #[test]
    fn spec_4_4_no_temp_file_remains() {
        let s = Scratch::new("tmp");
        let path = s.join("config.toml");
        // A stale temp file from a crashed save is replaced, not appended.
        std::fs::write(s.join(".config.toml.tmp"), b"garbage").unwrap();
        custom().save(&path).unwrap();
        assert_eq!(s.names(""), ["config.toml"]);
        assert_eq!(AppConfig::load(&path).config, custom());
        // The target is a non-empty directory: the rename fails.
        std::fs::create_dir_all(s.join("blocked/child")).unwrap();
        assert!(custom().save(&s.join("blocked")).is_err());
        assert_eq!(s.names(""), ["blocked", "config.toml"]);
        // No file name.
        assert!(custom().save(Path::new("/")).is_err());
    }

    /// SPEC §4 Subphase 4.4 (#131): a `last_table` / `tables_dir` that is
    /// not valid UTF-8 does not fail the save; it is left out, every other
    /// setting is written and the in-memory value is unchanged.
    #[cfg(unix)]
    #[test]
    fn spec_4_4_non_utf8_paths_do_not_fail_the_save() {
        use std::os::unix::ffi::OsStrExt;
        let bad = PathBuf::from(std::ffi::OsStr::from_bytes(b"/t/bad_\xff\xfe.pinbundle"));
        let mut c = custom();
        c.last_table = Some(bad.clone());
        c.tables_dir = Some(bad.clone());
        let text = c.to_toml_string().expect("non-UTF-8 paths are left out");
        assert!(!text.contains("last_table") && !text.contains("tables_dir"));
        assert!(text.contains("wireframe = true"), "{text}");
        let s = Scratch::new("utf8");
        let path = s.join("config.toml");
        c.save(&path).unwrap();
        assert_eq!(c.last_table.as_deref(), Some(bad.as_path()));
        let back = AppConfig::load(&path);
        assert_eq!(back.warnings, []);
        let mut want = c.clone();
        want.last_table = None;
        want.tables_dir = None;
        assert_eq!(back.config, want);
        // A valid path next to a bad one is still written.
        c.tables_dir = Some("/t".into());
        let text = c.to_toml_string().unwrap();
        assert!(text.contains("tables_dir = \"/t\"") && !text.contains("last_table"));
    }

    /// SPEC §4 Subphase 4.4 (#131): preserved unknown keys keep datetimes as
    /// TOML datetimes (offset, local datetime, local date, local time; at the
    /// top level, in unknown tables, in arrays and in known sections).
    #[test]
    fn spec_4_4_unknown_datetimes_stay_datetimes() {
        let text = "released = 2024-05-06T07:08:09Z\n\
                    local = 2024-05-06T07:08:09\n\
                    day = 2024-05-06\n\
                    at = 07:08:09\n\
                    list = [2024-01-02, 1, \"s\"]\n\
                    [future]\n\
                    when = 1999-12-31T23:59:59.5+01:00\n\
                    [future.deep]\n\
                    t = 12:00:00\n\
                    [calibration]\n\
                    stamp = 2020-02-02\n";
        let l = AppConfig::from_toml_str(text);
        assert!(
            l.warnings
                .iter()
                .all(|w| matches!(w, ConfigWarning::UnknownKey(_))),
            "{:?}",
            l.warnings
        );
        let saved = l.config.to_toml_string().unwrap();
        let doc: toml::Table = saved.parse().unwrap();
        let want: toml::Table = text.parse().unwrap();
        let dt = |v: Option<&toml::Value>| matches!(v, Some(toml::Value::Datetime(_)));
        for k in ["released", "local", "day", "at"] {
            assert!(dt(doc.get(k)), "{k} in\n{saved}");
            assert_eq!(doc.get(k), want.get(k), "{k}");
        }
        assert_eq!(doc.get("list"), want.get("list"));
        assert!(dt(doc["list"].as_array().unwrap().first()));
        assert_eq!(doc.get("future"), want.get("future"), "{saved}");
        assert!(dt(doc["future"].get("when")));
        assert!(dt(doc["future"]["deep"].get("t")));
        assert!(dt(doc["calibration"].get("stamp")), "{saved}");
        assert!(saved.contains("released = 2024-05-06T07:08:09Z"), "{saved}");
        // And the second round trip is stable.
        let again = AppConfig::from_toml_str(&saved);
        assert_eq!(again.config.to_toml_string().unwrap(), saved);
    }

    /// SPEC §4 Subphase 4.4 (#131): `deadzone = 0.1` is written as `0.1`
    /// and reloads bit-identical; `f64` values in unknown keys are untouched.
    #[test]
    fn spec_4_4_f32_written_as_shortest_decimal() {
        let l = AppConfig::from_toml_str(
            "ui_scale = 1.25\nfuture = 0.10000000149011612\n[calibration.plunger]\ndeadzone = 0.1\n",
        );
        assert_eq!(
            l.warnings,
            [ConfigWarning::UnknownKey("future".into())],
            "{:?}",
            l.warnings
        );
        assert_eq!(l.config.calibration.plunger.deadzone, 0.1f32);
        let text = l.config.to_toml_string().unwrap();
        assert!(text.contains("deadzone = 0.1\n"), "{text}");
        assert!(!text.contains("0.10000000149011612\n[") || text.contains("future"));
        assert!(text.contains("future = 0.10000000149011612"), "{text}");
        assert!(text.contains("ui_scale = 1.25"), "{text}");
        let back = AppConfig::from_toml_str(&text).config;
        assert_eq!(
            back.calibration.plunger.deadzone.to_bits(),
            0.1f32.to_bits()
        );
        // The defaults print short too (no 17-digit noise anywhere).
        let d = AppConfig::default().to_toml_string().unwrap();
        for line in d.lines() {
            if let Some((_, v)) = line.split_once(" = ")
                && let Ok(x) = v.parse::<f64>()
            {
                assert!(v.len() <= 12, "{line}");
                assert!(x.is_finite());
            }
        }
        assert_eq!(f32_value(-0.0).to_bits(), (-0.0f64).to_bits());
        assert!(f32_value(f32::NAN).is_nan());
    }

    proptest::proptest! {
        #![proptest_config(proptest::prelude::ProptestConfig {
            cases: 512,
            failure_persistence: None,
            ..proptest::prelude::ProptestConfig::default()
        })]

        /// SPEC §4 Subphase 4.4 (#131): every in-range f32 setting saves
        /// and reloads bit-identical.
        #[test]
        fn spec_4_4_f32_settings_round_trip_exactly(
            dz in 0.0f32..=0.5,
            gain in 0.0f32..=10.0,
            rest in 0.0f32..=100.0,
            speed in 0.0f32..=1000.0,
            pip in MIN_PIP_WIDTH..=MAX_PIP_WIDTH,
            scale in MIN_UI_SCALE..=MAX_UI_SCALE,
            bits in proptest::prelude::any::<u32>(),
        ) {
            let mut c = AppConfig::default();
            c.calibration.plunger.deadzone = dz;
            c.calibration.nudge.gain = gain;
            c.calibration.plunger.rest_tracking_per_s = rest;
            c.calibration.plunger.release_min_speed = speed;
            c.backglass_pip.width_fraction = pip;
            c.ui_scale = scale;
            // Any finite f32 bit pattern, mapped into the deadzone range of
            // the nudge filter (0..=1) without losing low mantissa bits.
            let any = f32::from_bits(bits & 0x3f7f_ffff);
            c.calibration.nudge.deadzone = any;
            let text = c.to_toml_string().unwrap();
            let r = AppConfig::from_toml_str(&text);
            proptest::prop_assert!(r.warnings.is_empty(), "{:?}\n{}", r.warnings, text);
            let b = &r.config;
            for (x, y) in [
                (b.calibration.plunger.deadzone, dz),
                (b.calibration.nudge.gain, gain),
                (b.calibration.plunger.rest_tracking_per_s, rest),
                (b.calibration.plunger.release_min_speed, speed),
                (b.backglass_pip.width_fraction, pip),
                (b.ui_scale, scale),
                (b.calibration.nudge.deadzone, any),
            ] {
                proptest::prop_assert_eq!(x.to_bits(), y.to_bits(), "{}", text);
            }
            proptest::prop_assert_eq!(b.to_toml_string().unwrap(), text);
        }

        /// SPEC §4 Subphase 4.4 (#131): `f32_value` is exact for every
        /// finite f32 bit pattern.
        #[test]
        fn spec_4_4_f32_value_is_exact(bits in proptest::prelude::any::<u32>()) {
            let x = f32::from_bits(bits);
            proptest::prop_assume!(x.is_finite());
            let d = f32_value(x);
            proptest::prop_assert_eq!((d as f32).to_bits(), x.to_bits());
            let printed: f64 = toml::Value::Float(d).to_string().parse().unwrap();
            proptest::prop_assert_eq!((printed as f32).to_bits(), x.to_bits());
        }
    }

    /// SPEC §4 Subphase 4.4 (#131): `ui_scale` defaults to 1.0 (also when
    /// missing), keeps an in-range value, and resets a value that is out of
    /// range, NaN or of the wrong type with one `InvalidValue` warning.
    #[test]
    fn spec_4_4_ui_scale_key() {
        assert_eq!(AppConfig::default().ui_scale, DEFAULT_UI_SCALE);
        assert_eq!(DEFAULT_UI_SCALE, 1.0);
        assert!(TOP_KEYS.contains(&"ui_scale"));
        let l = AppConfig::from_toml_str("wireframe = true\n");
        assert_eq!((l.config.ui_scale, l.warnings.len()), (1.0, 0));
        for (v, want) in [("0.5", 0.5), ("2", 2.0), ("2.5", 2.5), ("4.0", 4.0)] {
            let l = AppConfig::from_toml_str(&format!("ui_scale = {v}\n"));
            assert_eq!(l.warnings, [], "{v}");
            assert_eq!(l.config.ui_scale, want);
        }
        for v in [
            "0.49", "4.01", "-1.0", "0", "nan", "inf", "-inf", "\"big\"", "[2.0]", "{}", "true",
        ] {
            let l = AppConfig::from_toml_str(&format!("ui_scale = {v}\nwireframe = true\n"));
            assert_eq!(l.config.ui_scale, DEFAULT_UI_SCALE, "{v}");
            assert!(l.config.wireframe, "{v}: the rest of the file is kept");
            assert_eq!(
                invalid_keys(&l.warnings),
                ["ui_scale"],
                "{v}: {:?}",
                l.warnings
            );
        }
        let mut c = AppConfig {
            ui_scale: f32::NAN,
            ..AppConfig::default()
        };
        let mut w = Vec::new();
        c.sanitize(&mut w);
        assert_eq!((c.ui_scale, invalid_keys(&w)), (1.0, vec!["ui_scale"]));
        let text = custom().to_toml_string().unwrap();
        assert!(text.contains("ui_scale = 2.0"), "{text}");
    }
}
