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
        let config = match toml::from_str::<AppConfig>(text) {
            Ok(mut c) => {
                c.sanitize(&mut warnings);
                c
            }
            Err(e) => {
                warnings.push(ConfigWarning::Corrupt(e.to_string()));
                AppConfig::default()
            }
        };
        // Unknown keys: every top-level extra, plus the nested tables'
        // unknown keys (serde ignores those; report them, and keep them by
        // re-reading the raw table).
        let mut config = config;
        if warnings
            .iter()
            .all(|w| !matches!(w, ConfigWarning::Corrupt(_)))
        {
            if let Ok(raw) = toml::from_str::<toml::Table>(text) {
                collect_unknown(&raw, &mut config, &mut warnings);
            }
        }
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
        let mut bad = |key: &str, message: String| {
            warnings.push(ConfigWarning::InvalidValue {
                key: key.to_owned(),
                message,
            })
        };
        let mut index = |key: &str, v: &mut u32, dv: u32| {
            if *v > MAX_DISPLAY_INDEX {
                bad(key, format!("{v} is not a monitor index 0..={MAX_DISPLAY_INDEX}"));
                *v = dv;
            }
        };
        index("playfield_display", &mut self.playfield_display, 0);
        let mut opt = |key: &str, v: &mut Option<u32>| {
            if let Some(i) = *v
                && i > MAX_DISPLAY_INDEX
            {
                warnings.push(ConfigWarning::InvalidValue {
                    key: key.to_owned(),
                    message: format!("{i} is not a monitor index 0..={MAX_DISPLAY_INDEX}"),
                });
                *v = None;
            }
        };
        opt("backglass_display", &mut self.backglass_display);
        opt("score_display_monitor", &mut self.score_display_monitor);
        let mut range = |key: &str, v: &mut f32, lo: f32, hi: f32, dv: f32| {
            if !(v.is_finite() && *v >= lo && *v <= hi) {
                warnings.push(ConfigWarning::InvalidValue {
                    key: key.to_owned(),
                    message: format!("{v} is outside {lo}..={hi}"),
                });
                *v = dv;
            }
        };
        range(
            "backglass_pip.width_fraction",
            &mut self.backglass_pip.width_fraction,
            MIN_PIP_WIDTH,
            MAX_PIP_WIDTH,
            d.backglass_pip.width_fraction,
        );
        range(
            "score_pip.width_fraction",
            &mut self.score_pip.width_fraction,
            MIN_PIP_WIDTH,
            MAX_PIP_WIDTH,
            d.score_pip.width_fraction,
        );
        let (c, dc) = (&mut self.calibration, &d.calibration);
        let (p, dp) = (&mut c.plunger, &dc.plunger);
        range(
            "calibration.plunger.deadzone",
            &mut p.deadzone,
            0.0,
            0.5,
            dp.deadzone,
        );
        range(
            "calibration.plunger.rest_tracking_per_s",
            &mut p.rest_tracking_per_s,
            0.0,
            100.0,
            dp.rest_tracking_per_s,
        );
        range(
            "calibration.plunger.release_min_speed",
            &mut p.release_min_speed,
            0.0,
            1000.0,
            dp.release_min_speed,
        );
        if !(1..=pin_input::calibration::MAX_RELEASE_WINDOW_MS).contains(&p.release_window_ms) {
            warnings.push(ConfigWarning::InvalidValue {
                key: "calibration.plunger.release_window_ms".to_owned(),
                message: format!(
                    "{} is outside 1..={}",
                    p.release_window_ms,
                    pin_input::calibration::MAX_RELEASE_WINDOW_MS
                ),
            });
            p.release_window_ms = dp.release_window_ms;
        }
        let (n, dn) = (&mut c.nudge, &dc.nudge);
        range(
            "calibration.nudge.high_pass_hz",
            &mut n.high_pass_hz,
            0.0,
            50.0,
            dn.high_pass_hz,
        );
        range("calibration.nudge.gain", &mut n.gain, 0.0, 10.0, dn.gain);
        range(
            "calibration.nudge.curve_exponent",
            &mut n.curve_exponent,
            0.1,
            5.0,
            dn.curve_exponent,
        );
        range(
            "calibration.nudge.deadzone",
            &mut n.deadzone,
            0.0,
            1.0,
            dn.deadzone,
        );
        range("calibration.nudge.max", &mut n.max, 0.0, 10.0, dn.max);
        range(
            "calibration.dual_stage_lower",
            &mut c.dual_stage_lower,
            0.0,
            1.0,
            dc.dual_stage_lower,
        );
        range(
            "calibration.dual_stage_upper",
            &mut c.dual_stage_upper,
            0.0,
            1.0,
            dc.dual_stage_upper,
        );
        if c.dual_stage_lower > c.dual_stage_upper {
            warnings.push(ConfigWarning::InvalidValue {
                key: "calibration.dual_stage_lower".to_owned(),
                message: format!(
                    "lower threshold {} is above the upper threshold {}",
                    c.dual_stage_lower, c.dual_stage_upper
                ),
            });
            c.dual_stage_lower = dc.dual_stage_lower;
            c.dual_stage_upper = dc.dual_stage_upper;
        }
        if self.input_profile.trim().is_empty() || self.input_profile.len() > 128 {
            warnings.push(ConfigWarning::InvalidValue {
                key: "input_profile".to_owned(),
                message: "must be \"auto\" or a profile id (1..=128 characters)".to_owned(),
            });
            self.input_profile = d.input_profile;
        }
    }
}

/// Known keys of the nested tables (dotted path, keys).
const SECTIONS: [(&[&str], &[&str]); 5] = [
    (&["backglass_pip"], PIP_KEYS),
    (&["score_pip"], PIP_KEYS),
    (&["calibration"], CALIBRATION_KEYS),
    (&["calibration", "plunger"], PLUNGER_KEYS),
    (&["calibration", "nudge"], NUDGE_KEYS),
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
