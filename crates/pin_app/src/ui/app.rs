//! The menu in the running app (REQ-VIEW-006, issue #84): `config.toml`
//! load and save, the `Esc` / controller-menu toggle, cabinet routing while
//! the menu is open, and the hand-off of the menu state to the render thread.
//!
//! GPU-free. [`MenuController`] wraps a [`MenuState`] and executes its
//! [`UiCommand`]s; the platform's
//! [`EventForwarder`](crate::platform::EventForwarder) owns one (set with
//! `EventForwarder::set_menu`) and calls [`MenuController::toggle`] for the
//! toggle key or button and [`MenuController::route`] for every cabinet
//! event, so the controller sits between the input router and the input ring.
//! Everything here runs on the main thread (never on the sim or render
//! thread, R4); the render thread only receives clones of the menu state
//! through a [`MenuShare`].

use super::config::{AUTO_PROFILE, AppConfig, ConfigLoad, ConfigWarning};
use super::menu::{InputRoute, MenuInput, MenuState, UiCommand};
use super::scan::scan_tables;
use crate::platform::InputRouter;
use pin_core::input::{InputEvent, InputEventKind};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// The error a menu table load gets until table switching is wired (the
/// table browser, KP-005): the load lifecycle ends in `Failed` at once and
/// never hangs in `Loading`.
pub const LOAD_NOT_WIRED: &str = "table switching from the menu arrives with the table browser";

/// SDL pref-path organisation name of the per-user data directory.
pub const PREF_ORG: &str = "kilohertz";
/// SDL pref-path application name of the per-user data directory.
pub const PREF_APP: &str = "pinball";

/// Most distinct save errors printed on stderr (later ones are still kept in
/// [`MenuController::last_save_error`]).
const MAX_REPORTED_ERRORS: usize = 16;

/// Most command-drain rounds per call (executing a command can queue more:
/// a failed load report, a close; the chain is short and finite).
const MAX_DRAIN_ROUNDS: usize = 8;

/// A latest-value slot that carries the menu state from the main thread to
/// the render thread (cheap to clone; every clone is the same slot).
///
/// The main thread [`publish`](Self::publish)es a clone of the
/// [`MenuState`] whenever the menu changed; the render thread
/// [`take`](Self::take)s it when it is newer than what it shows. The mutex
/// is held only to swap an `Option`; it is used by the main and render
/// threads only, never by the sim thread (R4).
#[derive(Clone, Debug, Default)]
pub struct MenuShare {
    slot: Arc<Mutex<Option<MenuState>>>,
}

impl MenuShare {
    /// An empty slot.
    #[must_use]
    pub fn new() -> MenuShare {
        MenuShare::default()
    }

    /// Replaces the slot's content with a clone of `state` (an untaken older
    /// state is dropped: only the latest matters).
    pub fn publish(&self, state: &MenuState) {
        let copy = state.clone();
        let old = match self.slot.lock() {
            Ok(mut g) => g.replace(copy),
            Err(p) => p.into_inner().replace(copy),
        };
        // Dropped outside the lock.
        drop(old);
    }

    /// The state published since the last `take`, if any.
    pub fn take(&self) -> Option<MenuState> {
        match self.slot.lock() {
            Ok(mut g) => g.take(),
            Err(p) => p.into_inner().take(),
        }
    }
}

/// One of the settings the app already has a use for (issue #84 apply
/// list). Camera is not listed: its consumer is the view (issue #91).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum AppSetting {
    /// Single vs. multi-monitor and the monitor indices
    /// (`display_mode`, `playfield_display`, `backglass_display`,
    /// `score_display_monitor`).
    DisplayMode,
    /// Backglass and score PiP corner and width (`backglass_pip`,
    /// `score_pip`).
    PipPlacement,
    /// DMD vs. LCD score display (`score_display`).
    ScoreDisplay,
    /// Input profile (`input_profile`).
    InputProfile,
    /// Flipper staging override (`staging_mode`).
    StagingMode,
    /// Debug wireframe over the playfield (`wireframe`).
    Wireframe,
}

/// When a changed [`AppSetting`] takes effect in the app.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum ApplyTiming {
    /// At once, while the game runs.
    Live,
    /// When a table is next loaded (at start-up until the table browser can
    /// switch tables, KP-005).
    NextTableLoad,
    /// At the next start of the app.
    NextStart,
    /// Stored and saved, but nothing in the app reads it yet.
    NoConsumer,
}

impl ApplyTiming {
    /// How the app reports it (`pin_app: config: <setting> changed; <this>`).
    #[must_use]
    pub const fn describe(self) -> &'static str {
        match self {
            ApplyTiming::Live => "applied now",
            ApplyTiming::NextTableLoad => "applies at the next table load",
            ApplyTiming::NextStart => "applies at the next start",
            ApplyTiming::NoConsumer => "saved; nothing in the app uses it yet",
        }
    }
}

impl AppSetting {
    /// Every setting, in menu order.
    pub const ALL: [AppSetting; 6] = [
        AppSetting::DisplayMode,
        AppSetting::PipPlacement,
        AppSetting::ScoreDisplay,
        AppSetting::InputProfile,
        AppSetting::StagingMode,
        AppSetting::Wireframe,
    ];

    /// Name used on stderr.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            AppSetting::DisplayMode => "display mode",
            AppSetting::PipPlacement => "PiP placement",
            AppSetting::ScoreDisplay => "score display",
            AppSetting::InputProfile => "input profile",
            AppSetting::StagingMode => "staging mode",
            AppSetting::Wireframe => "debug wireframe",
        }
    }

    /// When a change takes effect (see [`MenuController`] for the reasons).
    #[must_use]
    pub const fn timing(self) -> ApplyTiming {
        match self {
            AppSetting::DisplayMode | AppSetting::PipPlacement => ApplyTiming::NextStart,
            AppSetting::InputProfile => ApplyTiming::Live,
            AppSetting::Wireframe => ApplyTiming::NextTableLoad,
            AppSetting::ScoreDisplay | AppSetting::StagingMode => ApplyTiming::NoConsumer,
        }
    }

    /// Whether this setting differs between `old` and `new`.
    #[must_use]
    pub fn differs(self, old: &AppConfig, new: &AppConfig) -> bool {
        match self {
            AppSetting::DisplayMode => {
                old.display_mode != new.display_mode
                    || old.playfield_display != new.playfield_display
                    || old.backglass_display != new.backglass_display
                    || old.score_display_monitor != new.score_display_monitor
            }
            AppSetting::PipPlacement => {
                old.backglass_pip != new.backglass_pip || old.score_pip != new.score_pip
            }
            AppSetting::ScoreDisplay => old.score_display != new.score_display,
            AppSetting::InputProfile => old.input_profile != new.input_profile,
            AppSetting::StagingMode => old.staging_mode != new.staging_mode,
            AppSetting::Wireframe => old.wireframe != new.wireframe,
        }
    }
}

/// The listed settings that differ between `old` and `new`, in
/// [`AppSetting::ALL`] order.
#[must_use]
pub fn changed_settings(old: &AppConfig, new: &AppConfig) -> Vec<AppSetting> {
    AppSetting::ALL
        .into_iter()
        .filter(|s| s.differs(old, new))
        .collect()
}

/// The input router for `profile` (the `input_profile` setting): the
/// auto-detected keyboard profile for [`AUTO_PROFILE`], else the built-in
/// profile of that id when it binds keyboard keys.
///
/// # Errors
/// An unknown id, or a profile without keyboard bindings (a gamepad
/// profile): the message says why; the caller keeps the auto profile
/// ([`InputRouter::new`]). Gamepads are always auto-detected.
pub fn keyboard_router(profile: &str) -> Result<InputRouter, String> {
    if profile == AUTO_PROFILE {
        return Ok(InputRouter::new());
    }
    let Some(p) = pin_input::builtin_profiles()
        .iter()
        .find(|p| p.id == profile)
    else {
        return Err(format!(
            "input profile {profile:?} is unknown; the keyboard uses the auto-detected profile"
        ));
    };
    if !p
        .buttons
        .iter()
        .any(|(raw, _)| matches!(raw, pin_input::RawInput::Key(_)))
    {
        return Err(format!(
            "input profile {profile:?} binds no keyboard keys; the keyboard uses the \
             auto-detected profile"
        ));
    }
    Ok(InputRouter::with_keyboard_profile(p))
}

/// The `--config <path>` argument: `None` without the flag.
///
/// # Errors
/// The flag without a value, or with a value starting with `--` (an option,
/// not a path; `./--x` is a path).
pub fn config_path_arg(args: &[String]) -> Result<Option<PathBuf>, String> {
    let Some(i) = args.iter().position(|a| a == "--config") else {
        return Ok(None);
    };
    match args.get(i + 1) {
        Some(v) if !v.starts_with("--") => Ok(Some(PathBuf::from(v))),
        _ => Err("--config needs a path".to_owned()),
    }
}

/// Where `config.toml` lives without `--config`: `config.toml` in SDL's
/// per-user data directory (`SDL_GetPrefPath("kilohertz", "pinball")`,
/// which SDL creates), or `config.toml` in the current directory when SDL
/// cannot give one (with the reason, for one stderr line).
#[must_use]
pub fn default_config_path() -> (PathBuf, Option<String>) {
    match sdl3::filesystem::get_pref_path(PREF_ORG, PREF_APP) {
        Ok(dir) => (dir.join(super::config::CONFIG_FILE), None),
        Err(e) => (
            PathBuf::from(super::config::CONFIG_FILE),
            Some(format!(
                "no per-user data directory ({e}); using ./{}",
                super::config::CONFIG_FILE
            )),
        ),
    }
}

/// Settings compared without the preserved unknown keys (which may hold a
/// `nan`, never equal to itself, and which an edit never touches).
fn same_settings(a: &AppConfig, b: &AppConfig) -> bool {
    let strip = |c: &AppConfig| AppConfig {
        extra: toml::Table::new(),
        ..c.clone()
    };
    strip(a) == strip(b)
}

/// The Subphase 4.4 menu as the running app uses it (REQ-VIEW-006).
///
/// It loads `config.toml` ([`load`](Self::load)), tracks the menu
/// ([`MenuState`]) and executes its commands inside [`toggle`](Self::toggle),
/// [`route`](Self::route) and [`apply`](Self::apply):
///
/// * [`UiCommand::ReleaseToSim`] becomes button releases and neutral axis
///   values stamped with the toggling event's own timestamp, so the
///   simulation applies them on the same tick as the `Esc` press;
/// * [`UiCommand::ConfigChanged`] saves `config.toml` atomically
///   ([`AppConfig::save`]) on the calling (main) thread unless the settings
///   equal the saved ones, and always keeps the change for
///   [`take_config_change`](Self::take_config_change);
/// * [`UiCommand::MenuOpened`] / [`UiCommand::MenuClosed`] pause nothing:
///   the game keeps running under the overlay;
/// * [`UiCommand::LoadTable`] is answered with [`LOAD_NOT_WIRED`] (table
///   switching is the table browser's, KP-005);
/// * [`UiCommand::Rescan`] scans `tables_dir` when it is set.
///
/// # When each setting applies
///
/// | Setting | Applies | Why |
/// |---|---|---|
/// | display mode (and monitors) | next start | the windows and surfaces are planned and created once at start-up from [`AppConfig::topology_mode`] |
/// | PiP placement | next start | part of the same start-up topology plan (the PiP viewports are fixed per surface) |
/// | score display (DMD / LCD) | no consumer yet | the table's own `backglass.toml` picks the display kind; nothing in the app reads the preference |
/// | input profile | live | the main thread replaces the keyboard router ([`keyboard_router`]) after releasing everything the old one holds |
/// | staging mode | no consumer yet | the table loader does not apply a staging override yet |
/// | debug wireframe | next table load | it is a table-renderer option ORed into `TableRenderOptions::debug_overlay` when the renderer is built for a table (at start-up until the table browser switches tables) |
///
/// ([`AppSetting::timing`] states the same table in code.)
#[derive(Debug)]
pub struct MenuController {
    path: PathBuf,
    menu: MenuState,
    /// The settings last written to (or read from) `path`.
    saved: AppConfig,
    change: Option<AppConfig>,
    saves: u64,
    last_save_error: Option<String>,
    reported_errors: Vec<String>,
    share: Option<MenuShare>,
    revision: u64,
}

impl MenuController {
    /// Loads `config_path` ([`AppConfig::load`]: a missing file gives the
    /// defaults and creates nothing) and builds a closed
    /// [`MenuState::new`] on it. Writes nothing (except the `.bak` copy
    /// `AppConfig::load` makes of a file it could not fully use). Returns
    /// the load warnings for the caller to print.
    pub fn load(config_path: impl Into<PathBuf>) -> (MenuController, Vec<ConfigWarning>) {
        let path = config_path.into();
        let ConfigLoad { config, warnings } = AppConfig::load(&path);
        let menu = MenuState::new(config);
        let saved = menu.config().clone();
        (
            MenuController {
                path,
                menu,
                saved,
                change: None,
                saves: 0,
                last_save_error: None,
                reported_errors: Vec::new(),
                share: None,
                revision: 0,
            },
            warnings,
        )
    }

    /// The app's menu: `config.toml` from `--config <path>`
    /// ([`config_path_arg`]) or else [`default_config_path`], loaded with
    /// [`load`](Self::load). The second value holds the stderr lines to
    /// print, one per note or load warning, each prefixed
    /// `pin_app: config: `.
    ///
    /// # Errors
    /// A bad `--config` argument.
    pub fn for_app(args: &[String]) -> Result<(MenuController, Vec<String>), String> {
        let mut lines = Vec::new();
        let path = match config_path_arg(args)? {
            Some(p) => p,
            None => {
                let (p, note) = default_config_path();
                lines.extend(note);
                p
            }
        };
        let (menu, warnings) = MenuController::load(path);
        lines.extend(warnings.iter().map(ToString::to_string));
        for l in &mut lines {
            l.insert_str(0, "pin_app: config: ");
        }
        Ok((menu, lines))
    }

    /// The `config.toml` path.
    #[must_use]
    pub fn config_path(&self) -> &Path {
        &self.path
    }

    /// The menu state (read-only: painting and tests).
    #[must_use]
    pub fn menu(&self) -> &MenuState {
        &self.menu
    }

    /// The settings (`== menu().config()`).
    #[must_use]
    pub fn config(&self) -> &AppConfig {
        self.menu.config()
    }

    /// Whether the menu is open.
    #[must_use]
    pub fn is_open(&self) -> bool {
        self.menu.is_open()
    }

    /// How many monitors the display panel offers
    /// ([`MenuState::set_display_count`]).
    pub fn set_display_count(&mut self, n: u32) {
        self.menu.set_display_count(n);
        self.changed();
    }

    /// Publishes the menu state into `share` now and after every visible
    /// change (the render thread paints from it).
    pub fn set_share(&mut self, share: MenuShare) {
        share.publish(&self.menu);
        self.share = Some(share);
    }

    /// Counts the visible menu changes (opening, closing, navigation, edits).
    #[must_use]
    pub fn revision(&self) -> u64 {
        self.revision
    }

    /// `Esc` or the controller menu button was pressed (not a repeat);
    /// `ts_ns` is the SDL timestamp of that event. Opening pushes into
    /// `out`, in this order, `InputEvent::button(ts_ns, input, false)` for
    /// every cabinet button the simulation last saw pressed
    /// (`CabinetInput::ALL` order), then `InputEvent::axis(ts_ns, axis, 0.0)`
    /// for every axis it last saw away from 0.0 (`CabinetAxis::ALL` order).
    /// Closing pushes nothing. Pending commands are executed before
    /// returning.
    pub fn toggle(&mut self, ts_ns: u64, out: &mut impl FnMut(InputEvent)) {
        self.menu.apply(MenuInput::Toggle);
        self.execute(ts_ns, out);
        self.changed();
    }

    /// A cabinet event from the input router. Menu closed: passed to `out`
    /// unchanged (and remembered through [`MenuState::route_cabinet`] /
    /// [`MenuState::route_axis`], so opening can release it). Menu open:
    /// consumed (a button press navigates, [`cabinet_menu_input`](super::cabinet_menu_input));
    /// `out` is not called except for commands the navigation caused.
    /// Every event also updates the input panel's live readings.
    pub fn route(&mut self, ev: InputEvent, out: &mut impl FnMut(InputEvent)) {
        let route = match ev.kind {
            InputEventKind::Button { input, pressed } => {
                self.menu.monitor_mut().set_button(0, input, pressed);
                self.menu.route_cabinet(input, pressed)
            }
            InputEventKind::Axis { axis, value } => {
                self.menu.monitor_mut().set_axis(axis, value);
                self.menu.route_axis(axis, value)
            }
        };
        match route {
            InputRoute::Sim => out(ev),
            InputRoute::Menu => {
                self.execute(ev.timestamp_ns, out);
                self.changed();
            }
        }
    }

    /// Applies a menu input that did not come from a cabinet button (what
    /// the egui widgets produce; [`MenuInput::Toggle`] works like
    /// [`toggle`](Self::toggle)). `ts_ns` stamps any release it causes.
    pub fn apply(&mut self, input: MenuInput, ts_ns: u64, out: &mut impl FnMut(InputEvent)) {
        self.menu.apply(input);
        self.execute(ts_ns, out);
        self.changed();
    }

    /// The latest changed settings since the previous call (`None` if
    /// nothing changed): what the app applies live. A change is returned
    /// even when saving it failed (it still applies to this run;
    /// [`last_save_error`](Self::last_save_error) says why it was not
    /// saved).
    pub fn take_config_change(&mut self) -> Option<AppConfig> {
        self.change.take()
    }

    /// Successful `config.toml` saves so far.
    #[must_use]
    pub fn saves(&self) -> u64 {
        self.saves
    }

    /// The error of the last save attempt, `None` when it succeeded or none
    /// was made. Each distinct error is also printed once on stderr.
    #[must_use]
    pub fn last_save_error(&self) -> Option<&str> {
        self.last_save_error.as_deref()
    }

    fn changed(&mut self) {
        self.revision += 1;
        if let Some(s) = &self.share {
            s.publish(&self.menu);
        }
    }

    /// Executes the menu's pending commands.
    fn execute(&mut self, ts_ns: u64, out: &mut impl FnMut(InputEvent)) {
        for _ in 0..MAX_DRAIN_ROUNDS {
            if self.menu.pending_commands().is_empty() {
                return;
            }
            for cmd in self.menu.drain_commands() {
                match cmd {
                    UiCommand::ReleaseToSim { buttons, axes } => {
                        for input in buttons {
                            out(InputEvent::button(ts_ns, input, false));
                        }
                        for axis in axes {
                            out(InputEvent::axis(ts_ns, axis, 0.0));
                        }
                    }
                    UiCommand::ConfigChanged(cfg) => self.persist(*cfg),
                    // The game keeps running under the menu (issue #84).
                    UiCommand::MenuOpened | UiCommand::MenuClosed => {}
                    UiCommand::LoadTable(_) => {
                        if let Some(id) = self.menu.pending_load_id() {
                            self.menu
                                .report_load_result_for(id, Err(LOAD_NOT_WIRED.to_owned()));
                        }
                    }
                    UiCommand::Rescan => {
                        if let Some(dir) = self.menu.config().tables_dir.clone() {
                            self.menu.set_tables(scan_tables(&dir));
                        }
                    }
                }
            }
        }
    }

    /// Keeps `cfg` for [`take_config_change`](Self::take_config_change)
    /// (always: after a failed save the app may run settings other than the
    /// saved ones, so a revert to them is a change too) and saves it unless
    /// it equals the saved settings, in which case the file already matches
    /// and any earlier save error no longer applies.
    fn persist(&mut self, cfg: AppConfig) {
        if same_settings(&cfg, &self.saved) {
            self.last_save_error = None;
            self.change = Some(cfg);
            return;
        }
        match cfg.save(&self.path) {
            Ok(()) => {
                self.saves += 1;
                self.last_save_error = None;
                self.saved = cfg.clone();
            }
            Err(e) => {
                let text = format!("saving {}: {e}", self.path.display());
                if !self.reported_errors.contains(&text) {
                    eprintln!("pin_app: config: {text}");
                    if self.reported_errors.len() < MAX_REPORTED_ERRORS {
                        self.reported_errors.push(text.clone());
                    }
                }
                self.last_save_error = Some(text);
            }
        }
        self.change = Some(cfg);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::{Panel, SettingChange};
    use pin_core::input::{CabinetAxis, CabinetInput};

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "pin_app_ui_app_{tag}_{}_{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn collect(
        c: &mut MenuController,
        f: impl FnOnce(&mut MenuController, &mut dyn FnMut(InputEvent)),
    ) -> Vec<InputEvent> {
        let mut v = Vec::new();
        f(c, &mut |e| v.push(e));
        v
    }

    #[test]
    fn share_keeps_only_the_latest_state() {
        let s = MenuShare::new();
        assert!(s.take().is_none());
        let mut m = MenuState::default();
        s.publish(&m);
        m.open();
        s.clone().publish(&m);
        let got = s.take().unwrap();
        assert!(got.is_open());
        assert!(s.take().is_none());
    }

    #[test]
    fn settings_timing_table() {
        use ApplyTiming::*;
        let t: Vec<_> = AppSetting::ALL.iter().map(|s| s.timing()).collect();
        assert_eq!(
            t,
            [
                NextStart,
                NextStart,
                NoConsumer,
                Live,
                NoConsumer,
                NextTableLoad
            ]
        );
        for s in AppSetting::ALL {
            assert!(!s.label().is_empty());
            assert!(!s.timing().describe().is_empty());
        }
    }

    #[test]
    fn changed_settings_lists_each_difference() {
        let a = AppConfig::default();
        assert!(changed_settings(&a, &a).is_empty());
        let mut b = a.clone();
        b.display_mode = crate::ui::DisplayMode::Multi;
        b.score_pip.width_fraction = 0.5;
        b.score_display = crate::ui::ScoreDisplayKind::Lcd;
        b.input_profile = "generic_keyboard".into();
        b.staging_mode = crate::ui::StagingMode::Dedicated;
        b.wireframe = true;
        assert_eq!(changed_settings(&a, &b), AppSetting::ALL.to_vec());
        let mut c = a.clone();
        c.backglass_display = Some(1);
        assert_eq!(changed_settings(&a, &c), [AppSetting::DisplayMode]);
        let mut d = a.clone();
        d.camera_preset = crate::ui::CameraPreset::TopDown;
        assert!(changed_settings(&a, &d).is_empty(), "camera is #91's");
    }

    #[test]
    fn keyboard_router_profiles() {
        assert!(keyboard_router(AUTO_PROFILE).is_ok());
        let kb = pin_input::builtin_profiles()
            .iter()
            .find(|p| p.matches.contains(&pin_input::DeviceMatch::Keyboard))
            .unwrap();
        let r = keyboard_router(&kb.id).unwrap();
        assert_eq!(r.keyboard_profile(), kb.id);
        assert!(
            keyboard_router("no_such_profile")
                .err()
                .unwrap()
                .contains("unknown")
        );
        let pad = pin_input::builtin_profiles()
            .iter()
            .find(|p| {
                !p.buttons
                    .iter()
                    .any(|(r, _)| matches!(r, pin_input::RawInput::Key(_)))
            })
            .unwrap();
        assert!(
            keyboard_router(&pad.id)
                .err()
                .unwrap()
                .contains("no keyboard")
        );
    }

    #[test]
    fn config_path_argument() {
        let a = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert_eq!(config_path_arg(&a(&["--table", "x"])), Ok(None));
        assert_eq!(
            config_path_arg(&a(&["--config", "a/b.toml"])),
            Ok(Some(PathBuf::from("a/b.toml")))
        );
        assert!(config_path_arg(&a(&["--config"])).is_err());
        assert!(config_path_arg(&a(&["--config", "--run-ms"])).is_err());
        assert_eq!(
            config_path_arg(&a(&["--config", "./--x"])),
            Ok(Some(PathBuf::from("./--x")))
        );
    }

    #[test]
    fn default_path_ends_in_config_toml() {
        let (p, _) = default_config_path();
        assert_eq!(p.file_name().unwrap(), "config.toml");
    }

    #[test]
    fn for_app_uses_the_config_flag_and_prefixes_warnings() {
        let d = dir("for_app");
        let path = d.join("config.toml");
        std::fs::write(&path, "wireframe = true\nfuture_key = 1\n").unwrap();
        let args: Vec<String> = ["--config", path.to_str().unwrap()]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (c, lines) = MenuController::for_app(&args).unwrap();
        assert_eq!(c.config_path(), path);
        assert!(c.config().wireframe);
        assert_eq!(lines.len(), 1, "{lines:?}");
        assert!(lines[0].starts_with("pin_app: config: "), "{lines:?}");
        assert!(lines[0].contains("future_key"), "{lines:?}");
        assert!(MenuController::for_app(&["--config".to_owned()]).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn toggle_releases_and_saves_on_change() {
        let d = dir("toggle");
        let path = d.join("config.toml");
        let (mut c, w) = MenuController::load(&path);
        assert!(w.is_empty());
        assert_eq!(c.config_path(), path);
        assert!(!c.is_open());
        let share = MenuShare::new();
        c.set_share(share.clone());
        assert!(!share.take().unwrap().is_open());
        c.set_display_count(2);
        assert_eq!(c.menu().display_count(), 2);
        let fwd = collect(&mut c, |c, mut o| {
            c.route(
                InputEvent::button(5, CabinetInput::LeftFlipper, true),
                &mut o,
            );
            c.route(InputEvent::axis(6, CabinetAxis::Plunger, 0.5), &mut o);
        });
        assert_eq!(fwd.len(), 2);
        let r0 = c.revision();
        let rel = collect(&mut c, |c, mut o| c.toggle(9, &mut o));
        assert_eq!(
            rel,
            [
                InputEvent::button(9, CabinetInput::LeftFlipper, false),
                InputEvent::axis(9, CabinetAxis::Plunger, 0.0)
            ]
        );
        assert!(c.is_open() && c.revision() > r0);
        assert!(share.take().unwrap().is_open());
        // Open: consumed, navigates.
        let none = collect(&mut c, |c, mut o| {
            c.route(
                InputEvent::button(10, CabinetInput::RightUpper, true),
                &mut o,
            );
            c.route(InputEvent::axis(11, CabinetAxis::Plunger, 0.7), &mut o);
        });
        assert!(none.is_empty());
        assert_eq!(c.menu().panel(), Panel::Display);
        // A change saves once; the same value again does not.
        collect(&mut c, |c, mut o| {
            c.apply(MenuInput::Set(SettingChange::Wireframe(true)), 12, &mut o)
        });
        assert_eq!(c.saves(), 1);
        assert!(c.take_config_change().unwrap().wireframe);
        assert!(c.take_config_change().is_none());
        collect(&mut c, |c, mut o| {
            c.apply(MenuInput::Set(SettingChange::Wireframe(true)), 13, &mut o)
        });
        assert_eq!(c.saves(), 1);
        assert!(c.last_save_error().is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn load_table_is_answered_and_rescan_scans() {
        let d = dir("load");
        std::fs::create_dir_all(d.join("tables")).unwrap();
        let path = d.join("config.toml");
        std::fs::write(
            &path,
            format!("tables_dir = {:?}\n", d.join("tables").to_str().unwrap()),
        )
        .unwrap();
        let (mut c, _) = MenuController::load(&path);
        collect(&mut c, |c, mut o| c.toggle(1, &mut o));
        collect(&mut c, |c, mut o| c.apply(MenuInput::Rescan, 2, &mut o));
        assert!(c.menu().tables().is_empty());
        // A direct request is refused with the deferral message, not hung.
        let mut m = c.menu().clone();
        assert!(m.request_load(Path::new("/x")).is_ok());
        let mut c2 = MenuController {
            menu: m,
            ..MenuController::load(&path).0
        };
        collect(&mut c2, |c, mut o| c.apply(MenuInput::Up, 3, &mut o));
        assert!(matches!(
            c2.menu().load_state(),
            crate::ui::TableLoadState::Failed { error, .. } if error == LOAD_NOT_WIRED
        ));
        assert!(c2.menu().pending_load_id().is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn save_errors_are_reported_without_panicking() {
        let d = dir("err");
        // A directory where the file should be: the rename fails.
        let path = d.join("config.toml");
        std::fs::create_dir_all(path.join("sub")).unwrap();
        let (mut c, _) = MenuController::load(&path);
        collect(&mut c, |c, mut o| c.toggle(1, &mut o));
        collect(&mut c, |c, mut o| {
            c.apply(MenuInput::Set(SettingChange::Wireframe(true)), 2, &mut o)
        });
        let e = c.last_save_error().unwrap().to_owned();
        assert!(e.contains("config.toml"), "{e}");
        collect(&mut c, |c, mut o| {
            c.apply(MenuInput::Set(SettingChange::Wireframe(false)), 3, &mut o);
            c.apply(MenuInput::Set(SettingChange::Wireframe(true)), 4, &mut o);
        });
        assert_eq!(c.saves(), 0);
        assert_eq!(c.reported_errors.len(), 1, "one line per distinct error");
        assert!(c.take_config_change().unwrap().wireframe);
        let _ = std::fs::remove_dir_all(&d);
    }
    /// PR #197 finding 2 (issue #84 live input-profile change): when saving
    /// profile B fails, the app still applies B; reverting to the saved
    /// profile A must then be reported too, or the menu shows A while the
    /// app keeps B's router. A later successful save still works.
    #[test]
    fn persist_reports_change_when_save_failed_then_reverted() {
        let d = dir("revert");
        // The config's parent is a regular file: every save fails.
        let blocker = d.join("not_a_dir");
        std::fs::write(&blocker, "x").unwrap();
        let path = blocker.join("config.toml");
        let (mut c, _) = MenuController::load(&path);
        let a = c.config().clone();
        let b_id = "xarcade_keyboard";
        assert_ne!(a.input_profile, b_id);
        collect(&mut c, |c, mut o| c.toggle(1, &mut o));
        collect(&mut c, |c, mut o| {
            c.apply(
                MenuInput::Set(SettingChange::InputProfile(b_id.into())),
                2,
                &mut o,
            )
        });
        assert!(c.last_save_error().is_some());
        assert_eq!(c.saves(), 0);
        assert!(same_settings(&c.saved, &a), "saved stays A");
        let b = c.take_config_change().expect("change = Some(B)");
        assert_eq!(b.input_profile, b_id);
        // Back to A: equal to the saved settings, still a change.
        collect(&mut c, |c, mut o| {
            c.apply(
                MenuInput::Set(SettingChange::InputProfile(a.input_profile.clone())),
                3,
                &mut o,
            )
        });
        let back = c.take_config_change().expect("revert to A is reported");
        assert!(same_settings(&back, &a));
        assert!(c.last_save_error().is_none(), "the file matches A");
        assert_eq!(c.saves(), 0, "no write for the saved settings");
        // The path becomes writable: B saves.
        std::fs::remove_file(&blocker).unwrap();
        std::fs::create_dir_all(&blocker).unwrap();
        collect(&mut c, |c, mut o| {
            c.apply(
                MenuInput::Set(SettingChange::InputProfile(b_id.into())),
                4,
                &mut o,
            )
        });
        assert_eq!(c.saves(), 1);
        assert!(c.last_save_error().is_none());
        assert_eq!(c.take_config_change().unwrap().input_profile, b_id);
        assert_eq!(AppConfig::load(&path).config.input_profile, b_id);
        let _ = std::fs::remove_dir_all(&d);
    }
}
