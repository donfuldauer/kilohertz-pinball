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
//! * [`AppConfig::save`] is atomic: the text is written to a temporary file in
//!   the same directory, flushed to disk, then renamed over the target.
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
                if let FlipperStagingMode::DedicatedButton(_) | FlipperStagingMode::ParallelLinked(_) =
                    table
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
    pub backglass_pip: PipConfig,
    /// Score display PiP placement.
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
    /// Audio settings (`[audio]`).
    pub audio: AudioConfig,
    /// Folder scanned for table bundles; `None` = `tables/` next to the
    /// executable (resolved by the app).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tables_dir: Option<PathBuf>,
    /// The table loaded last (set when a load succeeds).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_table: Option<PathBuf>,
    /// Keys this build does not know, preserved verbatim at their place in
    /// the file: unknown top-level keys and tables, and unknown keys inside
    /// the known tables (e.g. `calibration.plunger.future_key` is stored as
    /// `extra["calibration"]["plunger"]["future_key"]`). Written back by
    /// [`AppConfig::save`] wherever they do not clash with a known setting.
    #[serde(flatten)]
    pub extra: toml::Table,
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
            audio: AudioConfig::default(),
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
                "config.toml is {n} bytes (max {MAX_CONFIG_BYTES}); using defaults"
            ),
            ConfigWarning::Corrupt(e) => {
                write!(f, "config.toml is corrupt; using defaults: {}", e.trim_end())
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
    "audio",
    "tables_dir",
    "last_table",
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

/// Largest monitor index the config accepts.
pub const MAX_DISPLAY_INDEX: u32 = 15;

impl AppConfig {
    /// Loads `path`. Never fails; see the module docs for the fallback rules.
    pub fn load(path: &Path) -> ConfigLoad {
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
                    warnings: vec![ConfigWarning::Corrupt(e.to_string())],
                };
            }
        };
        // Values the ruling says fall back individually even when they do not
        // parse (`audio.decoded_budget_mib`), instead of making the whole
        // file corrupt.
        let mut typed = raw.clone();
        lenient_audio(&mut typed, &mut warnings);
        let mut config = match toml::Value::Table(typed).try_into::<AppConfig>() {
            Ok(mut c) => {
                c.sanitize(&mut warnings);
                c
            }
            Err(e) => {
                return ConfigLoad {
                    config: AppConfig::default(),
                    warnings: vec![ConfigWarning::Corrupt(e.to_string())],
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
    /// they do not clash with a known setting. Fails only for a path
    /// (`tables_dir`, `last_table`) that is not valid UTF-8.
    pub fn to_toml_string(&self) -> Result<String, toml::ser::Error> {
        let mut known = self.clone();
        let extra = std::mem::take(&mut known.extra);
        let mut doc = toml::Table::try_from(&known)?;
        merge_missing(&mut doc, &extra);
        toml::to_string(&doc)
    }

    /// Writes the settings to `path` atomically: a temporary file next to it
    /// (`.<name>.tmp`), `sync_all`, then a rename over `path`. On error the
    /// previous file is untouched and the temporary file is removed.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let text = self
            .to_toml_string()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        let name = path.file_name().ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "config path has no file name",
            )
        })?;
        let mut tmp_name = std::ffi::OsString::from(".");
        tmp_name.push(name);
        tmp_name.push(".tmp");
        let tmp = path.with_file_name(tmp_name);
        let write = || -> std::io::Result<()> {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(text.as_bytes())?;
            f.sync_all()?;
            drop(f);
            std::fs::rename(&tmp, path)
        };
        let result = write();
        if result.is_err() {
            let _ = std::fs::remove_file(&tmp);
        }
        result
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
        let checks: [(&str, &mut f32, f32, f32, f32); 12] = [
            ("backglass_pip.width_fraction", &mut self.backglass_pip.width_fraction, pw, sw, d.backglass_pip.width_fraction),
            ("score_pip.width_fraction", &mut self.score_pip.width_fraction, pw, sw, d.score_pip.width_fraction),
            ("calibration.plunger.deadzone", &mut c.plunger.deadzone, 0.0, 0.5, dc.plunger.deadzone),
            ("calibration.plunger.rest_tracking_per_s", &mut c.plunger.rest_tracking_per_s, 0.0, 100.0, dc.plunger.rest_tracking_per_s),
            ("calibration.plunger.release_min_speed", &mut c.plunger.release_min_speed, 0.0, 1000.0, dc.plunger.release_min_speed),
            ("calibration.nudge.high_pass_hz", &mut c.nudge.high_pass_hz, 0.0, 50.0, dc.nudge.high_pass_hz),
            ("calibration.nudge.gain", &mut c.nudge.gain, 0.0, 10.0, dc.nudge.gain),
            ("calibration.nudge.curve_exponent", &mut c.nudge.curve_exponent, 0.1, 5.0, dc.nudge.curve_exponent),
            ("calibration.nudge.deadzone", &mut c.nudge.deadzone, 0.0, 1.0, dc.nudge.deadzone),
            ("calibration.nudge.max", &mut c.nudge.max, 0.0, 10.0, dc.nudge.max),
            ("calibration.dual_stage_lower", &mut c.dual_stage_lower, 0.0, 1.0, dc.dual_stage_lower),
            ("calibration.dual_stage_upper", &mut c.dual_stage_upper, 0.0, 1.0, dc.dual_stage_upper),
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
const SECTIONS: [(&[&str], &[&str]); 6] = [
    (&["backglass_pip"], PIP_KEYS),
    (&["score_pip"], PIP_KEYS),
    (&["calibration"], CALIBRATION_KEYS),
    (&["calibration", "plunger"], PLUNGER_KEYS),
    (&["calibration", "nudge"], NUDGE_KEYS),
    (&["audio"], AUDIO_KEYS),
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
    for key in raw.keys() {
        if !TOP_KEYS.contains(&key.as_str()) {
            warnings.push(ConfigWarning::UnknownKey(key.clone()));
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
