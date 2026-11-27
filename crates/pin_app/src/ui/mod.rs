//! Table browser, display setup and input menu (Subphase 4.4).
//!
//! Owner: Subphase 4.4. Stub created by Subphase 1.1.
//!
//! This module is GPU-free except [`overlay`]: the menu logic is a state
//! machine ([`MenuState`]) driven by plain [`MenuInput`] events and emitting
//! [`UiCommand`]s, so headless tests need neither SDL nor a GPU. The parts:
//!
//! * [`menu`]: panels, focus, settings edits, table-switch lifecycle, the
//!   command stream and cabinet-input routing while the menu is open;
//! * [`scan`]: the table directory scan ([`scan_tables`]);
//! * [`config`]: the typed `config.toml` model ([`AppConfig`]) with
//!   [`AppConfig::load`] / [`AppConfig::save`];
//! * [`live`]: live input readings for the calibration display, OR-ing the
//!   input sources per cabinet input (3.2 review N5);
//! * [`view`]: builds the egui widgets of one menu frame from a
//!   [`MenuState`] (CPU only; `pin_render::ui` paints the result);
//! * [`app`]: the menu in the running app (REQ-VIEW-006, issue #84):
//!   [`MenuController`] loads `config.toml` at start, executes the menu's
//!   commands (releases to the simulation, atomic saves) and sits between
//!   the input router and the input ring; [`MenuShare`] hands the state to
//!   the render thread;
//! * [`overlay`]: [`MenuOverlay`], the menu painted over the playfield on
//!   the render thread.
//!
//! Wiring (issue #84): the platform's
//! [`EventForwarder`](crate::platform::EventForwarder) owns the
//! [`MenuController`] and maps `Esc` ([`is_menu_toggle_key`]) and the
//! controller menu button ([`is_menu_toggle_button`]) to
//! [`MenuController::toggle`] before the input router sees them; every
//! cabinet event goes through [`MenuController::route`]. The render thread
//! paints the latest state published into its [`MenuShare`]
//! (`RenderConfig::menu`).
//!
//! Table browser (issue #85, KP-005): a run without `--table` scans
//! [`browser_dir`] (`<exe dir>/tables` unless `config.toml`'s `tables_dir`
//! says otherwise) on the app's I/O worker, opens the menu on its Tables
//! panel and, with [`MenuController::set_browser_enabled`], hands each menu
//! load and rescan to the app, which validates and loads the table off the
//! main thread and swaps it in ([`MenuController::finish_load`]). A
//! `--table` run keeps the old answer: a menu load fails at once with
//! [`LOAD_NOT_WIRED`].

pub mod app;
pub mod config;
pub mod live;
pub mod menu;
pub mod overlay;
pub mod scan;
pub mod view;

pub use app::{
    AppSetting, ApplyTiming, LOAD_NOT_WIRED, MenuController, MenuShare, PREF_APP, PREF_ORG,
    changed_settings, config_path_arg, default_config_path, keyboard_router,
};
pub use config::{
    AUTO_PROFILE, AppConfig, AudioConfig, CONFIG_FILE, CalibrationConfig, CameraPreset, ConfigLoad,
    ConfigWarning, DEFAULT_DECODED_BUDGET_MIB, DEFAULT_TELEMETRY_ENDPOINT, DisplayMode, DofConfig,
    MAX_CONFIG_BYTES, MAX_DECODED_BUDGET_MIB, MAX_ENDPOINT_BYTES, MIN_DECODED_BUDGET_MIB,
    NudgeCalibration, PipConfig, PipCorner, PlungerCalibration, ScoreDisplayKind, StagingMode,
    TelemetryConfig,
};
pub use live::{InputMonitor, MAX_INPUT_SOURCES, PlungerReading};
pub use menu::{
    InputRoute, LoadRefusal, MENU_TOGGLE_BUTTON, MENU_TOGGLE_SCANCODE, MenuInput, MenuState,
    NumericRange, Panel, SettingChange, SettingRow, TableLoadState, UiCommand, cabinet_menu_input,
    default_profile_choices, is_menu_toggle_button, is_menu_toggle_key,
};
pub use overlay::{
    MENU_PIXELS_PER_POINT, MenuDebugHooks, MenuFrames, MenuOverlay, menu_overlay_for,
};
pub use scan::{
    PreviewArt, TABLES_DIR_NAME, TableEntry, TableFeature, TableInfo, TableStatus, browser_dir,
    decode_preview, inspect_bundle, resolve_tables_dir, scan_tables,
};
pub use view::MenuView;
