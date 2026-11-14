//! The menu state machine: panels, focus, settings edits, the table-switch
//! lifecycle and the command stream. Driven only by [`MenuInput`] values, so
//! tests need neither SDL nor a GPU.
//!
//! ## Navigation model
//! Each panel is a list of rows. Row 0 is the tab bar; `Left`/`Right` (or
//! `Select`) there switch panels. In the table browser rows `1..` are the
//! (filtered) tables; `Select` loads the focused one. In the display and input
//! panels rows `1..` are [`SettingRow`]s; `Left`/`Right` step the value and
//! `Select` cycles choices / toggles switches. `Back` on a row returns focus
//! to the tab bar; `Back` on the tab bar closes the menu. Mouse clicks and
//! slider drags arrive as the direct inputs ([`MenuInput::ShowPanel`],
//! [`MenuInput::LoadTable`], [`MenuInput::Set`], ...).
//!
//! ## Table switch lifecycle
//! `Idle → LoadRequested(path) → Loading(path) → Loaded(path) | Failed → Idle`:
//! * a request pushes [`UiCommand::LoadTable`] and enters `LoadRequested`;
//! * draining that command with [`MenuState::drain_commands`] enters `Loading`
//!   (the consumer has taken it);
//! * the consumer reports the outcome with [`MenuState::report_load_result`]:
//!   success enters `Loaded`, records [`AppConfig::last_table`] (a
//!   [`UiCommand::ConfigChanged`]) and closes the menu; failure enters
//!   `Failed` with the error, shown in the browser;
//! * `Loaded` / `Failed` return to `Idle` on [`MenuState::acknowledge_load`],
//!   on the next menu input, or when a new request starts.
//!
//! A second request while `LoadRequested` or `Loading` is **refused**
//! ([`LoadRefusal::Busy`]), not queued (DECISIONS.md "4.4 — Menu UI
//! decisions"): a queued switch would fire after the player has already seen
//! the first table start, and a refusal is visible in the menu at once.

use super::config::{
    AUTO_PROFILE, AppConfig, CameraPreset, DisplayMode, MAX_DECODED_BUDGET_MIB, MAX_DISPLAY_INDEX,
    MAX_PIP_WIDTH, MIN_DECODED_BUDGET_MIB, MIN_PIP_WIDTH, PipCorner, ScoreDisplayKind, StagingMode,
};
use super::live::InputMonitor;
use super::scan::TableEntry;
use pin_core::input::CabinetInput;
use pin_input::GamepadButton;
use std::path::{Path, PathBuf};

/// SDL scancode of `Esc` (`SDL_SCANCODE_ESCAPE`), which toggles the menu.
pub const MENU_TOGGLE_SCANCODE: u32 = 41;

/// Gamepad button that toggles the menu: the controller's Guide / home
/// button (`Start` and `Back` are cabinet inputs in the built-in profiles).
pub const MENU_TOGGLE_BUTTON: GamepadButton = GamepadButton::Guide;

/// Whether a key press with this SDL scancode toggles the menu.
pub fn is_menu_toggle_key(scancode: u32) -> bool {
    scancode == MENU_TOGGLE_SCANCODE
}

/// Whether this gamepad button toggles the menu.
pub fn is_menu_toggle_button(button: GamepadButton) -> bool {
    button == MENU_TOGGLE_BUTTON
}

/// The three menu panels.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash)]
pub enum Panel {
    /// Table browser.
    #[default]
    Tables,
    /// Display setup.
    Display,
    /// Input setup.
    Input,
}

impl Panel {
    /// Every panel, in tab order.
    pub const ALL: [Panel; 3] = [Panel::Tables, Panel::Display, Panel::Input];

    /// Tab label.
    pub const fn label(self) -> &'static str {
        match self {
            Panel::Tables => "Tables",
            Panel::Display => "Display",
            Panel::Input => "Input",
        }
    }

    const fn index(self) -> usize {
        match self {
            Panel::Tables => 0,
            Panel::Display => 1,
            Panel::Input => 2,
        }
    }

    /// The next panel (wraps).
    pub const fn next(self) -> Panel {
        Panel::ALL[(self.index() + 1) % 3]
    }

    /// The previous panel (wraps).
    pub const fn prev(self) -> Panel {
        Panel::ALL[(self.index() + 2) % 3]
    }
}

/// One editable setting row of the display or input panel.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum SettingRow {
    /// [`AppConfig::display_mode`].
    DisplayMode,
    /// [`AppConfig::playfield_display`].
    PlayfieldDisplay,
    /// [`AppConfig::backglass_display`].
    BackglassDisplay,
    /// [`AppConfig::score_display_monitor`].
    ScoreDisplayMonitor,
    /// [`AppConfig::backglass_pip`] corner.
    BackglassPipCorner,
    /// [`AppConfig::backglass_pip`] width.
    BackglassPipWidth,
    /// [`AppConfig::score_pip`] corner.
    ScorePipCorner,
    /// [`AppConfig::score_pip`] width.
    ScorePipWidth,
    /// [`AppConfig::camera_preset`].
    CameraPreset,
    /// [`AppConfig::score_display`] (DMD vs. LCD).
    ScoreDisplay,
    /// [`AppConfig::input_profile`].
    InputProfile,
    /// [`AppConfig::staging_mode`].
    StagingMode,
    /// Dual-stage lower threshold.
    DualStageLower,
    /// Dual-stage upper threshold.
    DualStageUpper,
    /// Plunger deadzone.
    PlungerDeadzone,
    /// Plunger rest tracking rate.
    PlungerRestTracking,
    /// Plunger minimum release speed.
    PlungerReleaseMinSpeed,
    /// Plunger release window.
    PlungerReleaseWindow,
    /// Nudge high-pass cut-off.
    NudgeHighPass,
    /// Nudge gain.
    NudgeGain,
    /// Nudge curve exponent.
    NudgeCurve,
    /// Nudge deadzone.
    NudgeDeadzone,
    /// Nudge output clamp.
    NudgeMax,
    /// [`AppConfig::wireframe`].
    Wireframe,
    /// [`AudioConfig::decoded_budget_mib`](super::config::AudioConfig) (advanced;
    /// last row of the display panel; applies to the next table load).
    DecodedAudioBudget,
}

/// Numeric range and step of a slider row.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct NumericRange {
    /// Minimum.
    pub min: f64,
    /// Maximum.
    pub max: f64,
    /// Step of one `Left`/`Right` press.
    pub step: f64,
}

impl SettingRow {
    /// Rows of the display panel, in order (menu rows `1..`).
    pub const DISPLAY: [SettingRow; 11] = [
        SettingRow::DisplayMode,
        SettingRow::PlayfieldDisplay,
        SettingRow::BackglassDisplay,
        SettingRow::ScoreDisplayMonitor,
        SettingRow::BackglassPipCorner,
        SettingRow::BackglassPipWidth,
        SettingRow::ScorePipCorner,
        SettingRow::ScorePipWidth,
        SettingRow::CameraPreset,
        SettingRow::ScoreDisplay,
        SettingRow::DecodedAudioBudget,
    ];

    /// Rows of the input panel, in order (menu rows `1..`).
    pub const INPUT: [SettingRow; 14] = [
        SettingRow::InputProfile,
        SettingRow::StagingMode,
        SettingRow::DualStageLower,
        SettingRow::DualStageUpper,
        SettingRow::PlungerDeadzone,
        SettingRow::PlungerRestTracking,
        SettingRow::PlungerReleaseMinSpeed,
        SettingRow::PlungerReleaseWindow,
        SettingRow::NudgeHighPass,
        SettingRow::NudgeGain,
        SettingRow::NudgeCurve,
        SettingRow::NudgeDeadzone,
        SettingRow::NudgeMax,
        SettingRow::Wireframe,
    ];

    /// Row label.
    pub const fn label(self) -> &'static str {
        match self {
            SettingRow::DisplayMode => "Monitors",
            SettingRow::PlayfieldDisplay => "Playfield monitor",
            SettingRow::BackglassDisplay => "Backglass monitor",
            SettingRow::ScoreDisplayMonitor => "Score display monitor",
            SettingRow::BackglassPipCorner => "Backglass PiP corner",
            SettingRow::BackglassPipWidth => "Backglass PiP width",
            SettingRow::ScorePipCorner => "Score PiP corner",
            SettingRow::ScorePipWidth => "Score PiP width",
            SettingRow::CameraPreset => "Camera",
            SettingRow::ScoreDisplay => "Score display",
            SettingRow::InputProfile => "Input profile",
            SettingRow::StagingMode => "Flipper staging",
            SettingRow::DualStageLower => "Dual-stage lower",
            SettingRow::DualStageUpper => "Dual-stage upper",
            SettingRow::PlungerDeadzone => "Plunger deadzone",
            SettingRow::PlungerRestTracking => "Plunger rest tracking (1/s)",
            SettingRow::PlungerReleaseMinSpeed => "Plunger release speed (min)",
            SettingRow::PlungerReleaseWindow => "Plunger release window (ms)",
            SettingRow::NudgeHighPass => "Nudge high-pass (Hz)",
            SettingRow::NudgeGain => "Nudge gain",
            SettingRow::NudgeCurve => "Nudge curve exponent",
            SettingRow::NudgeDeadzone => "Nudge deadzone",
            SettingRow::NudgeMax => "Nudge max",
            SettingRow::Wireframe => "Debug wireframe",
            SettingRow::DecodedAudioBudget => "Audio decode budget",
        }
    }

    /// Range of a numeric (slider) row, `None` for choice and toggle rows.
    pub fn range(self) -> Option<NumericRange> {
        let r = |min, max, step| Some(NumericRange { min, max, step });
        match self {
            SettingRow::BackglassPipWidth | SettingRow::ScorePipWidth => {
                r(MIN_PIP_WIDTH as f64, MAX_PIP_WIDTH as f64, 0.05)
            }
            SettingRow::DualStageLower | SettingRow::DualStageUpper => r(0.0, 1.0, 0.05),
            SettingRow::PlungerDeadzone => r(0.0, 0.5, 0.005),
            SettingRow::PlungerRestTracking => r(0.0, 100.0, 0.1),
            SettingRow::PlungerReleaseMinSpeed => r(0.0, 1000.0, 0.5),
            SettingRow::PlungerReleaseWindow => r(1.0, 1024.0, 10.0),
            SettingRow::NudgeHighPass => r(0.0, 50.0, 0.5),
            SettingRow::NudgeGain => r(0.0, 10.0, 0.1),
            SettingRow::NudgeCurve => r(0.1, 5.0, 0.1),
            SettingRow::NudgeDeadzone => r(0.0, 1.0, 0.005),
            SettingRow::NudgeMax => r(0.0, 10.0, 0.1),
            SettingRow::DecodedAudioBudget => r(
                f64::from(MIN_DECODED_BUDGET_MIB),
                f64::from(MAX_DECODED_BUDGET_MIB),
                64.0,
            ),
            _ => None,
        }
    }

    /// The current numeric value of a slider row (`None` otherwise).
    pub fn number(self, c: &AppConfig) -> Option<f64> {
        let cal = &c.calibration;
        Some(match self {
            SettingRow::BackglassPipWidth => c.backglass_pip.width_fraction.into(),
            SettingRow::ScorePipWidth => c.score_pip.width_fraction.into(),
            SettingRow::DualStageLower => cal.dual_stage_lower.into(),
            SettingRow::DualStageUpper => cal.dual_stage_upper.into(),
            SettingRow::PlungerDeadzone => cal.plunger.deadzone.into(),
            SettingRow::PlungerRestTracking => cal.plunger.rest_tracking_per_s.into(),
            SettingRow::PlungerReleaseMinSpeed => cal.plunger.release_min_speed.into(),
            SettingRow::PlungerReleaseWindow => cal.plunger.release_window_ms.into(),
            SettingRow::NudgeHighPass => cal.nudge.high_pass_hz.into(),
            SettingRow::NudgeGain => cal.nudge.gain.into(),
            SettingRow::NudgeCurve => cal.nudge.curve_exponent.into(),
            SettingRow::NudgeDeadzone => cal.nudge.deadzone.into(),
            SettingRow::NudgeMax => cal.nudge.max.into(),
            SettingRow::DecodedAudioBudget => c.audio.decoded_budget_mib.into(),
            _ => return None,
        })
    }

    /// The value as shown in the menu.
    pub fn value_text(self, c: &AppConfig) -> String {
        let monitor = |m: Option<u32>| match m {
            Some(i) => format!("Monitor {}", i + 1),
            None => "PiP".to_owned(),
        };
        match self {
            SettingRow::DisplayMode => match c.display_mode {
                DisplayMode::Single => "Single (PiP)".to_owned(),
                DisplayMode::Multi => "Multi-monitor".to_owned(),
            },
            SettingRow::PlayfieldDisplay => format!("Monitor {}", c.playfield_display + 1),
            SettingRow::BackglassDisplay => monitor(c.backglass_display),
            SettingRow::ScoreDisplayMonitor => monitor(c.score_display_monitor),
            SettingRow::BackglassPipCorner => c.backglass_pip.corner.label().to_owned(),
            SettingRow::ScorePipCorner => c.score_pip.corner.label().to_owned(),
            SettingRow::CameraPreset => c.camera_preset.label().to_owned(),
            SettingRow::ScoreDisplay => c.score_display.label().to_owned(),
            SettingRow::InputProfile => c.input_profile.clone(),
            SettingRow::StagingMode => c.staging_mode.label().to_owned(),
            SettingRow::Wireframe => if c.wireframe { "On" } else { "Off" }.to_owned(),
            SettingRow::PlungerReleaseWindow => {
                format!("{}", c.calibration.plunger.release_window_ms)
            }
            SettingRow::DecodedAudioBudget => format!("{} MiB", c.audio.decoded_budget_mib),
            SettingRow::BackglassPipWidth | SettingRow::ScorePipWidth => {
                format!("{:.0} %", self.number(c).unwrap_or(0.0) * 100.0)
            }
            _ => format!("{:.3}", self.number(c).unwrap_or(0.0)),
        }
    }
}

/// A direct edit of one setting (from a slider, combo box or checkbox).
/// Values are clamped to the row's range when applied.
#[derive(Clone, Debug, PartialEq)]
pub enum SettingChange {
    /// Single vs. multi-monitor.
    DisplayMode(DisplayMode),
    /// Playfield monitor index.
    PlayfieldDisplay(u32),
    /// Backglass monitor (`None` = PiP).
    BackglassDisplay(Option<u32>),
    /// Score display monitor (`None` = PiP).
    ScoreDisplayMonitor(Option<u32>),
    /// Backglass PiP corner.
    BackglassPipCorner(PipCorner),
    /// Backglass PiP width fraction.
    BackglassPipWidth(f32),
    /// Score PiP corner.
    ScorePipCorner(PipCorner),
    /// Score PiP width fraction.
    ScorePipWidth(f32),
    /// Camera preset.
    CameraPreset(CameraPreset),
    /// DMD vs. LCD.
    ScoreDisplay(ScoreDisplayKind),
    /// Input profile id ([`AUTO_PROFILE`] or a known profile).
    InputProfile(String),
    /// Flipper staging mode.
    StagingMode(StagingMode),
    /// Dual-stage lower threshold (kept ≤ upper).
    DualStageLower(f32),
    /// Dual-stage upper threshold (kept ≥ lower).
    DualStageUpper(f32),
    /// Plunger deadzone.
    PlungerDeadzone(f32),
    /// Plunger rest tracking rate.
    PlungerRestTracking(f32),
    /// Plunger minimum release speed.
    PlungerReleaseMinSpeed(f32),
    /// Plunger release window, ms.
    PlungerReleaseWindow(u32),
    /// Nudge high-pass cut-off.
    NudgeHighPass(f32),
    /// Nudge gain.
    NudgeGain(f32),
    /// Nudge curve exponent.
    NudgeCurve(f32),
    /// Nudge deadzone.
    NudgeDeadzone(f32),
    /// Nudge max.
    NudgeMax(f32),
    /// Debug wireframe on/off.
    Wireframe(bool),
    /// Decoded-audio budget, MiB (clamped to 64..=8192).
    DecodedAudioBudget(u32),
}

impl SettingChange {
    /// The change setting numeric `row` to `value` (`None` for a row that is
    /// not numeric). The value is clamped when applied.
    pub fn number(row: SettingRow, value: f64) -> Option<SettingChange> {
        let v = value as f32;
        Some(match row {
            SettingRow::BackglassPipWidth => SettingChange::BackglassPipWidth(v),
            SettingRow::ScorePipWidth => SettingChange::ScorePipWidth(v),
            SettingRow::DualStageLower => SettingChange::DualStageLower(v),
            SettingRow::DualStageUpper => SettingChange::DualStageUpper(v),
            SettingRow::PlungerDeadzone => SettingChange::PlungerDeadzone(v),
            SettingRow::PlungerRestTracking => SettingChange::PlungerRestTracking(v),
            SettingRow::PlungerReleaseMinSpeed => SettingChange::PlungerReleaseMinSpeed(v),
            SettingRow::PlungerReleaseWindow => {
                // Saturating float → int cast; NaN → 0 (clamped to 1 on apply).
                SettingChange::PlungerReleaseWindow(value.round() as u32)
            }
            SettingRow::NudgeHighPass => SettingChange::NudgeHighPass(v),
            SettingRow::NudgeGain => SettingChange::NudgeGain(v),
            SettingRow::NudgeCurve => SettingChange::NudgeCurve(v),
            SettingRow::NudgeDeadzone => SettingChange::NudgeDeadzone(v),
            SettingRow::NudgeMax => SettingChange::NudgeMax(v),
            SettingRow::DecodedAudioBudget => {
                // Saturating cast; NaN → 0 (clamped to the minimum on apply).
                SettingChange::DecodedAudioBudget(value.round() as u32)
            }
            _ => return None,
        })
    }
}

/// Everything that drives the menu.
#[derive(Clone, Debug, PartialEq)]
pub enum MenuInput {
    /// `Esc` / controller menu button: open or close.
    Toggle,
    /// Focus up.
    Up,
    /// Focus down.
    Down,
    /// Previous panel (tab bar) / decrease the value (setting row).
    Left,
    /// Next panel (tab bar) / increase the value (setting row).
    Right,
    /// Activate the focused row: next panel, load table, cycle a choice, toggle.
    Select,
    /// Focus the tab bar, or close the menu from the tab bar.
    Back,
    /// Show a panel (tab click); focus goes to the tab bar.
    ShowPanel(Panel),
    /// Focus table `i` of the full table list (list click).
    FocusTable(usize),
    /// Focus row `r` of the shown panel (clamped; 0 = tab bar).
    FocusRow(usize),
    /// One-click load of table `i` of the full table list.
    LoadTable(usize),
    /// Browser filter text changed (case-insensitive substring of title,
    /// author or file name; empty = all).
    SetFilter(String),
    /// Direct setting edit.
    Set(SettingChange),
    /// Ask for a new directory scan.
    Rescan,
}

/// What the menu asks the application to do. Drain with
/// [`MenuState::drain_commands`].
#[derive(Clone, Debug, PartialEq)]
pub enum UiCommand {
    /// The menu opened (pause the game; cabinet inputs now drive the menu).
    MenuOpened,
    /// The menu closed (resume).
    MenuClosed,
    /// Load this table bundle; report back with
    /// [`MenuState::report_load_result`].
    LoadTable(PathBuf),
    /// Settings changed: apply them and save `config.toml` (on the I/O
    /// worker). Consecutive changes are coalesced: at most one is pending,
    /// holding the latest settings.
    ConfigChanged(Box<AppConfig>),
    /// Scan the tables folder again (on the I/O worker) and pass the result
    /// to [`MenuState::set_tables`].
    Rescan,
}

/// Table switch lifecycle (see the module docs).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum TableLoadState {
    /// Nothing in progress.
    #[default]
    Idle,
    /// A [`UiCommand::LoadTable`] is pending in the command stream.
    LoadRequested(PathBuf),
    /// The consumer took the command and is loading.
    Loading(PathBuf),
    /// The load succeeded.
    Loaded(PathBuf),
    /// The load failed.
    Failed {
        /// The bundle.
        path: PathBuf,
        /// Why.
        error: String,
    },
}

impl TableLoadState {
    /// Whether a load is pending or running (new requests are refused).
    pub fn is_busy(&self) -> bool {
        matches!(
            self,
            TableLoadState::LoadRequested(_) | TableLoadState::Loading(_)
        )
    }
}

/// Why [`MenuState::request_load`] refused a request.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadRefusal {
    /// Another load is pending or running (that one's path).
    Busy(PathBuf),
    /// The table is listed as broken (its validation error).
    Broken(String),
    /// No such table index.
    NoSuchTable(usize),
}

impl std::fmt::Display for LoadRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            LoadRefusal::Busy(p) => write!(f, "busy loading {}", p.display()),
            LoadRefusal::Broken(e) => write!(f, "table is broken: {e}"),
            LoadRefusal::NoSuchTable(i) => write!(f, "no table #{i}"),
        }
    }
}

/// Where a cabinet input goes.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum InputRoute {
    /// Menu closed: forward to the simulation.
    Sim,
    /// Menu open: consumed by the menu (never forwarded).
    Menu,
}

/// The menu navigation a cabinet input means while the menu is open:
/// left/right flipper = up/down, left/right upper = left/right,
/// start/launch = select, action = back, tilt = nothing.
pub fn cabinet_menu_input(input: CabinetInput) -> Option<MenuInput> {
    match input {
        CabinetInput::LeftFlipper => Some(MenuInput::Up),
        CabinetInput::RightFlipper => Some(MenuInput::Down),
        CabinetInput::LeftUpper => Some(MenuInput::Left),
        CabinetInput::RightUpper => Some(MenuInput::Right),
        CabinetInput::Start | CabinetInput::Launch => Some(MenuInput::Select),
        CabinetInput::Action => Some(MenuInput::Back),
        CabinetInput::Tilt => None,
    }
}

/// Profile ids offered by default: [`AUTO_PROFILE`] then the built-ins.
pub fn default_profile_choices() -> Vec<String> {
    std::iter::once(AUTO_PROFILE.to_owned())
        .chain(pin_input::builtin_profiles().iter().map(|p| p.id.clone()))
        .collect()
}

/// The menu: open/closed, panel, focus, the scanned tables, the settings
/// being edited, the table-load lifecycle and the outgoing commands.
#[derive(Clone, Debug)]
pub struct MenuState {
    open: bool,
    panel: Panel,
    /// Focused row per panel (0 = tab bar).
    focus: [usize; 3],
    tables: Vec<TableEntry>,
    filter: String,
    /// Indices into `tables` passing the filter.
    visible: Vec<usize>,
    config: AppConfig,
    profiles: Vec<String>,
    display_count: u32,
    load: TableLoadState,
    notice: Option<String>,
    commands: Vec<UiCommand>,
    monitor: InputMonitor,
}

impl Default for MenuState {
    fn default() -> Self {
        MenuState::new(AppConfig::default())
    }
}

/// One step of `v` on the grid of multiples of `r.step` (anchored at 0, so
/// round defaults such as a 100 ms release window stay on it), clamped to the
/// range. A value off the grid snaps to the nearest grid point first.
fn step_value(v: f64, dir: i32, r: NumericRange) -> f64 {
    let steps = (v / r.step).round() + f64::from(dir.signum());
    (steps * r.step).clamp(r.min, r.max)
}

fn cycle<T: Copy + PartialEq>(all: &[T], cur: T, dir: i32) -> T {
    let n = all.len() as i64;
    let i = all.iter().position(|x| *x == cur).unwrap_or(0) as i64;
    all[(i + i64::from(dir.signum())).rem_euclid(n) as usize]
}

impl MenuState {
    /// A closed menu on the table browser, editing `config`, with no tables
    /// (call [`set_tables`](Self::set_tables)), one monitor and the default
    /// profile choices.
    pub fn new(config: AppConfig) -> Self {
        MenuState {
            open: false,
            panel: Panel::Tables,
            focus: [0; 3],
            tables: Vec::new(),
            filter: String::new(),
            visible: Vec::new(),
            config,
            profiles: default_profile_choices(),
            display_count: 1,
            load: TableLoadState::Idle,
            notice: None,
            commands: Vec::new(),
            monitor: InputMonitor::new(),
        }
    }

    /// Whether the menu is shown.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The shown panel.
    pub fn panel(&self) -> Panel {
        self.panel
    }

    /// The focused row of the shown panel (0 = tab bar).
    pub fn focus(&self) -> usize {
        self.focus[self.panel.index()]
    }

    /// Number of rows of the shown panel, tab bar included.
    pub fn row_count(&self) -> usize {
        1 + match self.panel {
            Panel::Tables => self.visible.len(),
            Panel::Display => SettingRow::DISPLAY.len(),
            Panel::Input => SettingRow::INPUT.len(),
        }
    }

    /// The setting row under the focus (display/input panels).
    pub fn focused_setting(&self) -> Option<SettingRow> {
        let row = self.focus().checked_sub(1)?;
        match self.panel {
            Panel::Tables => None,
            Panel::Display => SettingRow::DISPLAY.get(row).copied(),
            Panel::Input => SettingRow::INPUT.get(row).copied(),
        }
    }

    /// Index (into [`tables`](Self::tables)) of the focused table.
    pub fn focused_table(&self) -> Option<usize> {
        let row = self.focus[Panel::Tables.index()].checked_sub(1)?;
        self.visible.get(row).copied()
    }

    /// The scanned tables, in scan order.
    pub fn tables(&self) -> &[TableEntry] {
        &self.tables
    }

    /// Indices into [`tables`](Self::tables) shown by the current filter.
    pub fn visible_tables(&self) -> &[usize] {
        &self.visible
    }

    /// The browser filter text.
    pub fn filter(&self) -> &str {
        &self.filter
    }

    /// The settings being edited (always sanitized).
    pub fn config(&self) -> &AppConfig {
        &self.config
    }

    /// The table-switch lifecycle state.
    pub fn load_state(&self) -> &TableLoadState {
        &self.load
    }

    /// The last refusal or hint shown at the bottom of the menu.
    pub fn notice(&self) -> Option<&str> {
        self.notice.as_deref()
    }

    /// Profile ids the input panel cycles through.
    pub fn profile_choices(&self) -> &[String] {
        &self.profiles
    }

    /// Number of monitors the display panel offers.
    pub fn display_count(&self) -> u32 {
        self.display_count
    }

    /// Live input readings for the input panel.
    pub fn monitor(&self) -> &InputMonitor {
        &self.monitor
    }

    /// Live input readings, for the app to update.
    pub fn monitor_mut(&mut self) -> &mut InputMonitor {
        &mut self.monitor
    }

    /// Pending commands (not drained).
    pub fn pending_commands(&self) -> &[UiCommand] {
        &self.commands
    }

    /// Takes the pending commands, oldest first. Taking a
    /// [`UiCommand::LoadTable`] moves `LoadRequested` to `Loading`.
    pub fn drain_commands(&mut self) -> Vec<UiCommand> {
        let cmds = std::mem::take(&mut self.commands);
        if let TableLoadState::LoadRequested(p) = &self.load
            && cmds
                .iter()
                .any(|c| matches!(c, UiCommand::LoadTable(q) if q == p))
        {
            self.load = TableLoadState::Loading(p.clone());
        }
        cmds
    }

    /// Replaces the table list (a scan result). The focused table is kept by
    /// path when it is still listed.
    pub fn set_tables(&mut self, tables: Vec<TableEntry>) {
        let focused = self.focused_table().map(|i| self.tables[i].path.clone());
        self.tables = tables;
        self.refilter(focused.as_deref());
    }

    /// Sets the profile ids the input panel offers (e.g. built-ins plus user
    /// profiles). [`AUTO_PROFILE`] is always offered first.
    pub fn set_profile_choices(&mut self, ids: Vec<String>) {
        let mut v = vec![AUTO_PROFILE.to_owned()];
        for id in ids {
            if !v.contains(&id) {
                v.push(id);
            }
        }
        self.profiles = v;
    }

    /// Sets how many monitors exist (≥ 1). Monitor settings beyond the count
    /// are kept (the monitor may come back) but the panel only cycles through
    /// existing ones.
    pub fn set_display_count(&mut self, n: u32) {
        self.display_count = n.clamp(1, MAX_DISPLAY_INDEX + 1);
    }

    /// Replaces the settings (e.g. after the app loaded `config.toml`).
    /// Emits nothing.
    pub fn set_config(&mut self, mut config: AppConfig) {
        config.sanitize(&mut Vec::new());
        self.config = config;
    }

    /// Opens the menu (no-op when open).
    pub fn open(&mut self) {
        if !self.open {
            self.open = true;
            self.acknowledge_load();
            self.commands.push(UiCommand::MenuOpened);
        }
    }

    /// Closes the menu (no-op when closed).
    pub fn close(&mut self) {
        if self.open {
            self.open = false;
            self.commands.push(UiCommand::MenuClosed);
        }
    }

    /// Routes a cabinet button event: while the menu is open it is consumed
    /// (a press becomes [`cabinet_menu_input`] navigation, a release does
    /// nothing) and must not reach the simulation; while closed it goes to
    /// the simulation untouched.
    pub fn route_cabinet(&mut self, input: CabinetInput, pressed: bool) -> InputRoute {
        if !self.open {
            return InputRoute::Sim;
        }
        if pressed && let Some(m) = cabinet_menu_input(input) {
            self.apply(m);
        }
        InputRoute::Menu
    }

    /// Whether analog cabinet input (plunger, nudge, flipper axes) goes to
    /// the simulation: only while the menu is closed. (The live readings in
    /// the input panel still update through [`monitor_mut`](Self::monitor_mut).)
    pub fn forwards_to_sim(&self) -> bool {
        !self.open
    }

    /// Requests a load of the bundle at `path` (the app may call this too,
    /// e.g. for [`AppConfig::last_table`] at start-up).
    pub fn request_load(&mut self, path: &Path) -> Result<(), LoadRefusal> {
        if let TableLoadState::LoadRequested(p) | TableLoadState::Loading(p) = &self.load {
            let r = LoadRefusal::Busy(p.clone());
            self.notice = Some(r.to_string());
            return Err(r);
        }
        self.load = TableLoadState::LoadRequested(path.to_path_buf());
        self.notice = None;
        self.commands.push(UiCommand::LoadTable(path.to_path_buf()));
        Ok(())
    }

    /// Requests a load of table `index` of [`tables`](Self::tables).
    pub fn request_load_index(&mut self, index: usize) -> Result<(), LoadRefusal> {
        let Some(entry) = self.tables.get(index) else {
            return Err(LoadRefusal::NoSuchTable(index));
        };
        if let Some(e) = entry.error() {
            let r = LoadRefusal::Broken(e.to_owned());
            self.notice = Some(format!("{}: {r}", entry.title()));
            return Err(r);
        }
        let path = entry.path.clone();
        self.request_load(&path)
    }

    /// Reports the outcome of the pending load. Returns `false` (and changes
    /// nothing) when no load is pending or running. Success records
    /// [`AppConfig::last_table`] and closes the menu.
    pub fn report_load_result(&mut self, result: Result<(), String>) -> bool {
        let path = match &self.load {
            TableLoadState::LoadRequested(p) | TableLoadState::Loading(p) => p.clone(),
            _ => return false,
        };
        // A request reported before its command was drained: drop the command.
        self.commands
            .retain(|c| !matches!(c, UiCommand::LoadTable(q) if *q == path));
        match result {
            Ok(()) => {
                self.config.last_table = Some(path.clone());
                self.push_config_changed();
                self.load = TableLoadState::Loaded(path);
                self.notice = None;
                self.close();
            }
            Err(error) => {
                self.notice = Some(format!("Loading {} failed: {error}", path.display()));
                self.load = TableLoadState::Failed { path, error };
            }
        }
        true
    }

    /// `Loaded` / `Failed` → `Idle` (no-op otherwise).
    pub fn acknowledge_load(&mut self) {
        if matches!(
            self.load,
            TableLoadState::Loaded(_) | TableLoadState::Failed { .. }
        ) {
            self.load = TableLoadState::Idle;
        }
    }

    /// Applies one input. Inputs other than [`MenuInput::Toggle`] are ignored
    /// while the menu is closed.
    pub fn apply(&mut self, input: MenuInput) {
        if input == MenuInput::Toggle {
            if self.open {
                self.close();
            } else {
                self.open();
            }
            return;
        }
        if !self.open {
            return;
        }
        self.acknowledge_load();
        match input {
            MenuInput::Toggle => {}
            MenuInput::Up => self.move_focus(-1),
            MenuInput::Down => self.move_focus(1),
            MenuInput::Left => self.horizontal(-1),
            MenuInput::Right => self.horizontal(1),
            MenuInput::Select => self.select(),
            MenuInput::Back => {
                if self.focus() == 0 {
                    self.close();
                } else {
                    self.focus[self.panel.index()] = 0;
                }
            }
            MenuInput::ShowPanel(p) => {
                self.panel = p;
                self.focus[p.index()] = 0;
            }
            MenuInput::FocusTable(i) => {
                if let Some(row) = self.visible.iter().position(|v| *v == i) {
                    self.panel = Panel::Tables;
                    self.focus[Panel::Tables.index()] = row + 1;
                }
            }
            MenuInput::FocusRow(r) => {
                let n = self.row_count();
                self.focus[self.panel.index()] = r.min(n - 1);
            }
            MenuInput::LoadTable(i) => {
                let _ = self.request_load_index(i);
            }
            MenuInput::SetFilter(text) => {
                let focused = self.focused_table().map(|i| self.tables[i].path.clone());
                self.filter = text;
                self.refilter(focused.as_deref());
            }
            MenuInput::Set(change) => self.set(change),
            MenuInput::Rescan => self.commands.push(UiCommand::Rescan),
        }
    }

    /// Applies several inputs in order.
    pub fn apply_all(&mut self, inputs: impl IntoIterator<Item = MenuInput>) {
        for i in inputs {
            self.apply(i);
        }
    }

    fn move_focus(&mut self, d: i64) {
        let n = self.row_count() as i64;
        let f = &mut self.focus[self.panel.index()];
        *f = (*f as i64 + d).clamp(0, n - 1) as usize;
    }

    fn horizontal(&mut self, dir: i32) {
        if self.focus() == 0 {
            self.panel = if dir > 0 {
                self.panel.next()
            } else {
                self.panel.prev()
            };
            self.focus[self.panel.index()] = 0;
        } else if let Some(row) = self.focused_setting() {
            self.step(row, dir);
        }
    }

    fn select(&mut self) {
        if self.focus() == 0 {
            self.horizontal(1);
            return;
        }
        match self.panel {
            Panel::Tables => {
                if let Some(i) = self.focused_table() {
                    let _ = self.request_load_index(i);
                }
            }
            Panel::Display | Panel::Input => {
                if let Some(row) = self.focused_setting()
                    && row.range().is_none()
                {
                    self.step(row, 1);
                }
            }
        }
    }

    fn refilter(&mut self, keep: Option<&Path>) {
        let needle = self.filter.trim().to_lowercase();
        self.visible = self
            .tables
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                needle.is_empty()
                    || t.title().to_lowercase().contains(&needle)
                    || t.file_name.to_lowercase().contains(&needle)
                    || t.info()
                        .is_some_and(|i| i.author.to_lowercase().contains(&needle))
            })
            .map(|(i, _)| i)
            .collect();
        let f = &mut self.focus[Panel::Tables.index()];
        *f = match keep.and_then(|p| {
            self.visible
                .iter()
                .position(|i| self.tables[*i].path.as_path() == p)
        }) {
            Some(row) => row + 1,
            None => (*f).min(self.visible.len()),
        };
    }

    /// One `Left` (`dir < 0`) or `Right` / `Select` (`dir > 0`) step of `row`.
    fn step(&mut self, row: SettingRow, dir: i32) {
        let c = &self.config;
        let n = self.display_count;
        let monitors: Vec<Option<u32>> = std::iter::once(None).chain((0..n).map(Some)).collect();
        let change = if let Some(r) = row.range() {
            let v = step_value(row.number(c).unwrap_or(r.min), dir, r);
            SettingChange::number(row, v)
        } else {
            match row {
                SettingRow::DisplayMode => Some(SettingChange::DisplayMode(cycle(
                    &[DisplayMode::Single, DisplayMode::Multi],
                    c.display_mode,
                    dir,
                ))),
                SettingRow::PlayfieldDisplay => {
                    let all: Vec<u32> = (0..n).collect();
                    let cur = c.playfield_display.min(n - 1);
                    Some(SettingChange::PlayfieldDisplay(cycle(&all, cur, dir)))
                }
                SettingRow::BackglassDisplay => Some(SettingChange::BackglassDisplay(cycle(
                    &monitors,
                    c.backglass_display.filter(|i| *i < n),
                    dir,
                ))),
                SettingRow::ScoreDisplayMonitor => Some(SettingChange::ScoreDisplayMonitor(
                    cycle(&monitors, c.score_display_monitor.filter(|i| *i < n), dir),
                )),
                SettingRow::BackglassPipCorner => Some(SettingChange::BackglassPipCorner(cycle(
                    &PipCorner::ALL,
                    c.backglass_pip.corner,
                    dir,
                ))),
                SettingRow::ScorePipCorner => Some(SettingChange::ScorePipCorner(cycle(
                    &PipCorner::ALL,
                    c.score_pip.corner,
                    dir,
                ))),
                SettingRow::CameraPreset => Some(SettingChange::CameraPreset(cycle(
                    &CameraPreset::ALL,
                    c.camera_preset,
                    dir,
                ))),
                SettingRow::ScoreDisplay => Some(SettingChange::ScoreDisplay(cycle(
                    &[ScoreDisplayKind::Dmd, ScoreDisplayKind::Lcd],
                    c.score_display,
                    dir,
                ))),
                SettingRow::InputProfile => {
                    let len = self.profiles.len() as i64;
                    let i = self
                        .profiles
                        .iter()
                        .position(|p| *p == c.input_profile)
                        .map(|i| i as i64)
                        // An unlisted profile: Right goes to the first, Left to the last.
                        .unwrap_or(if dir > 0 { -1 } else { 0 });
                    let next = (i + i64::from(dir.signum())).rem_euclid(len) as usize;
                    Some(SettingChange::InputProfile(self.profiles[next].clone()))
                }
                SettingRow::StagingMode => Some(SettingChange::StagingMode(cycle(
                    &StagingMode::ALL,
                    c.staging_mode,
                    dir,
                ))),
                SettingRow::Wireframe => Some(SettingChange::Wireframe(!c.wireframe)),
                _ => None,
            }
        };
        if let Some(ch) = change {
            self.set(ch);
        }
    }

    /// Applies a direct edit (clamped), emitting [`UiCommand::ConfigChanged`]
    /// when the settings actually changed.
    fn set(&mut self, change: SettingChange) {
        // Compared without the preserved unknown keys (`extra`), which an
        // edit never touches and which may hold a `nan` (never equal).
        let mut before = self.config.clone();
        before.extra = toml::Table::new();
        let c = &mut self.config;
        let clamp = |v: f32, row: SettingRow| -> f32 {
            let r = row.range().expect("numeric row");
            if v.is_finite() {
                v.clamp(r.min as f32, r.max as f32)
            } else {
                row.number(&AppConfig::default()).unwrap_or(r.min) as f32
            }
        };
        let monitor = |m: Option<u32>| m.filter(|i| *i <= MAX_DISPLAY_INDEX);
        match change {
            SettingChange::DisplayMode(m) => c.display_mode = m,
            SettingChange::PlayfieldDisplay(i) => {
                c.playfield_display = i.min(MAX_DISPLAY_INDEX);
            }
            SettingChange::BackglassDisplay(m) => c.backglass_display = monitor(m),
            SettingChange::ScoreDisplayMonitor(m) => c.score_display_monitor = monitor(m),
            SettingChange::BackglassPipCorner(k) => c.backglass_pip.corner = k,
            SettingChange::BackglassPipWidth(v) => {
                c.backglass_pip.width_fraction = clamp(v, SettingRow::BackglassPipWidth);
            }
            SettingChange::ScorePipCorner(k) => c.score_pip.corner = k,
            SettingChange::ScorePipWidth(v) => {
                c.score_pip.width_fraction = clamp(v, SettingRow::ScorePipWidth);
            }
            SettingChange::CameraPreset(p) => c.camera_preset = p,
            SettingChange::ScoreDisplay(k) => c.score_display = k,
            SettingChange::InputProfile(id) => {
                if self.profiles.contains(&id) {
                    c.input_profile = id;
                } else {
                    self.notice = Some(format!("unknown input profile {id:?}"));
                }
            }
            SettingChange::StagingMode(m) => c.staging_mode = m,
            SettingChange::DualStageLower(v) => {
                let v = clamp(v, SettingRow::DualStageLower);
                c.calibration.dual_stage_lower = v.min(c.calibration.dual_stage_upper);
            }
            SettingChange::DualStageUpper(v) => {
                let v = clamp(v, SettingRow::DualStageUpper);
                c.calibration.dual_stage_upper = v.max(c.calibration.dual_stage_lower);
            }
            SettingChange::PlungerDeadzone(v) => {
                c.calibration.plunger.deadzone = clamp(v, SettingRow::PlungerDeadzone);
            }
            SettingChange::PlungerRestTracking(v) => {
                c.calibration.plunger.rest_tracking_per_s =
                    clamp(v, SettingRow::PlungerRestTracking);
            }
            SettingChange::PlungerReleaseMinSpeed(v) => {
                c.calibration.plunger.release_min_speed =
                    clamp(v, SettingRow::PlungerReleaseMinSpeed);
            }
            SettingChange::PlungerReleaseWindow(ms) => {
                c.calibration.plunger.release_window_ms =
                    ms.clamp(1, pin_input::calibration::MAX_RELEASE_WINDOW_MS);
            }
            SettingChange::NudgeHighPass(v) => {
                c.calibration.nudge.high_pass_hz = clamp(v, SettingRow::NudgeHighPass);
            }
            SettingChange::NudgeGain(v) => c.calibration.nudge.gain = clamp(v, SettingRow::NudgeGain),
            SettingChange::NudgeCurve(v) => {
                c.calibration.nudge.curve_exponent = clamp(v, SettingRow::NudgeCurve);
            }
            SettingChange::NudgeDeadzone(v) => {
                c.calibration.nudge.deadzone = clamp(v, SettingRow::NudgeDeadzone);
            }
            SettingChange::NudgeMax(v) => c.calibration.nudge.max = clamp(v, SettingRow::NudgeMax),
            SettingChange::Wireframe(on) => c.wireframe = on,
            SettingChange::DecodedAudioBudget(mib) => {
                c.audio.decoded_budget_mib = mib.clamp(MIN_DECODED_BUDGET_MIB, MAX_DECODED_BUDGET_MIB);
            }
        }
        let extra = std::mem::take(&mut self.config.extra);
        let changed = self.config != before;
        self.config.extra = extra;
        if changed {
            self.push_config_changed();
        }
    }

    fn push_config_changed(&mut self) {
        self.commands
            .retain(|c| !matches!(c, UiCommand::ConfigChanged(_)));
        self.commands
            .push(UiCommand::ConfigChanged(Box::new(self.config.clone())));
    }
}
