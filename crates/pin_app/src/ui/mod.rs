//! Table browser, display setup and input menu (Subphase 4.4).
//!
//! Owner: Subphase 4.4. Stub created by Subphase 1.1.
//!
//! This module is GPU-free: the menu logic is a state machine
//! ([`MenuState`]) driven by plain [`MenuInput`] events and emitting
//! [`UiCommand`]s, so headless tests need neither SDL nor a GPU. The parts:
//!
//! * [`menu`]: panels, focus, settings edits, table-switch lifecycle, the
//!   command stream and cabinet-input routing while the menu is open;
//! * [`scan`]: the table directory scan ([`scan_tables`]), run on the I/O
//!   worker in the app;
//! * [`config`]: the typed `config.toml` model ([`AppConfig`]) with
//!   [`AppConfig::load`] / [`AppConfig::save`];
//! * [`live`]: live input readings for the calibration display, OR-ing the
//!   input sources per cabinet input (3.2 review N5);
//! * [`view`]: builds the egui widgets of one menu frame from a
//!   [`MenuState`] (CPU only; `pin_render::ui` paints the result).
//!
//! Wiring into the platform pump, the render thread and the I/O worker is
//! Subphase 6.0's: the platform maps `Esc` ([`is_menu_toggle_key`]) and the
//! controller menu button ([`is_menu_toggle_button`]) to
//! [`MenuInput::Toggle`], routes cabinet buttons through
//! [`MenuState::route_cabinet`], executes [`MenuState::drain_commands`] and
//! reports loads with [`MenuState::report_load_result`].

pub mod config;
pub mod live;
pub mod menu;
pub mod scan;
pub mod view;

pub use config::{
    AUTO_PROFILE, AppConfig, AudioConfig, CONFIG_FILE, CalibrationConfig, CameraPreset, ConfigLoad,
    ConfigWarning, DEFAULT_DECODED_BUDGET_MIB, DisplayMode, MAX_CONFIG_BYTES,
    MAX_DECODED_BUDGET_MIB, MIN_DECODED_BUDGET_MIB, NudgeCalibration, PipConfig, PipCorner,
    PlungerCalibration, ScoreDisplayKind, StagingMode,
};
pub use live::{InputMonitor, MAX_INPUT_SOURCES, PlungerReading};
pub use menu::{
    InputRoute, LoadRefusal, MENU_TOGGLE_BUTTON, MENU_TOGGLE_SCANCODE, MenuInput, MenuState,
    NumericRange, Panel, SettingChange, SettingRow, TableLoadState, UiCommand, cabinet_menu_input,
    default_profile_choices, is_menu_toggle_button, is_menu_toggle_key,
};
pub use scan::{
    PreviewArt, TableEntry, TableFeature, TableInfo, TableStatus, decode_preview, inspect_bundle,
    scan_tables,
};
pub use view::MenuView;
