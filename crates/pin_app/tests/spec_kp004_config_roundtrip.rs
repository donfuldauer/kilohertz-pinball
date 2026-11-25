//! KP-004 (#84) black-box tests, REQ-VIEW-006 (SPEC section 7.1), acceptance 1
//! ("a settings change round-trips through config.toml") and SPEC section 4
//! Subphase 4.4 verification ("config.toml persistence").

#[path = "spec_kp004_support/mod.rs"]
mod support;

use pin_app::ui::{
    AppConfig, DisplayMode, MenuController, MenuInput, PipCorner, ScoreDisplayKind, SettingChange,
    StagingMode, default_profile_choices,
};
use pin_core::InputEvent;
use std::path::Path;

fn sink() -> impl FnMut(InputEvent) {
    |_| {}
}

/// Applies a setting change. `MenuState::apply` ignores every input but
/// `Toggle` while the menu is closed (4.4 contract), so the menu is opened
/// first (opening writes nothing; see the missing-file test).
fn apply(c: &mut MenuController, change: SettingChange, ts: u64) {
    if !c.is_open() {
        c.toggle(ts, &mut sink());
    }
    c.apply(MenuInput::Set(change), ts, &mut sink());
}

/// Applies `change` to a fresh controller, then checks the file, a direct
/// `AppConfig::load` and a second `MenuController::load` agree with `check`.
fn roundtrip(name: &str, change: SettingChange, check: impl Fn(&AppConfig) -> bool) {
    let dir = support::scratch(name);
    let path = dir.join("config.toml");
    let (mut c, w) = MenuController::load(&path);
    assert!(w.is_empty(), "{w:?}");
    assert!(!path.exists(), "load writes nothing");
    assert!(
        !check(c.config()),
        "{name}: change must differ from default"
    );
    apply(&mut c, change, 10);
    assert!(
        path.exists(),
        "{name}: file created at the controller's path"
    );
    assert_eq!(c.config_path(), path.as_path());
    assert!(check(c.config()), "{name}: live config");
    let loaded = AppConfig::load(&path);
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert!(check(&loaded.config), "{name}: AppConfig::load");
    let (c2, w2) = MenuController::load(&path);
    assert!(w2.is_empty());
    assert!(check(c2.config()), "{name}: new controller");
    assert!(!c2.is_open());
    let _ = std::fs::remove_dir_all(dir);
}

/// REQ-VIEW-006 acceptance 1: wireframe round-trips through config.toml.
#[test]
fn req_view_006_acc1_wireframe_roundtrip() {
    roundtrip("rt_wire", SettingChange::Wireframe(true), |c| c.wireframe);
}

/// REQ-VIEW-006 acceptance 1: display mode round-trips.
#[test]
fn req_view_006_acc1_display_mode_roundtrip() {
    roundtrip(
        "rt_dm",
        SettingChange::DisplayMode(DisplayMode::Multi),
        |c| c.display_mode == DisplayMode::Multi,
    );
}

/// REQ-VIEW-006 acceptance 1: score display kind round-trips.
#[test]
fn req_view_006_acc1_score_display_roundtrip() {
    roundtrip(
        "rt_sd",
        SettingChange::ScoreDisplay(ScoreDisplayKind::Lcd),
        |c| c.score_display == ScoreDisplayKind::Lcd,
    );
}

/// REQ-VIEW-006 acceptance 1: staging mode round-trips.
#[test]
fn req_view_006_acc1_staging_mode_roundtrip() {
    roundtrip(
        "rt_st",
        SettingChange::StagingMode(StagingMode::DualStage),
        |c| c.staging_mode == StagingMode::DualStage,
    );
}

/// REQ-VIEW-006 acceptance 1: backglass PiP corner round-trips.
#[test]
fn req_view_006_acc1_backglass_pip_corner_roundtrip() {
    roundtrip(
        "rt_pip",
        SettingChange::BackglassPipCorner(PipCorner::TopLeft),
        |c| c.backglass_pip.corner == PipCorner::TopLeft,
    );
}

/// REQ-VIEW-006 acceptance 1: input profile round-trips (a known id).
#[test]
fn req_view_006_acc1_input_profile_roundtrip() {
    let choices = default_profile_choices();
    let id = choices.last().expect("profiles").clone();
    assert_ne!(id, AppConfig::default().input_profile);
    let want = id.clone();
    roundtrip("rt_prof", SettingChange::InputProfile(id), move |c| {
        c.input_profile == want
    });
}

/// REQ-VIEW-006 acceptance 1: the file uses snake_case enum values.
#[test]
fn req_view_006_acc1_file_text_uses_config_keys() {
    let dir = support::scratch("text");
    let path = dir.join("config.toml");
    let (mut c, _) = MenuController::load(&path);
    apply(&mut c, SettingChange::DisplayMode(DisplayMode::Multi), 1);
    apply(
        &mut c,
        SettingChange::StagingMode(StagingMode::DualStage),
        2,
    );
    apply(&mut c, SettingChange::Wireframe(true), 3);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("display_mode"), "{text}");
    assert!(text.contains("\"multi\""), "{text}");
    assert!(text.contains("\"dual_stage\""), "{text}");
    assert!(text.contains("wireframe = true"), "{text}");
    let _ = std::fs::remove_dir_all(dir);
}

/// REQ-VIEW-006 acceptance 1: several changes accumulate; every one survives.
#[test]
fn req_view_006_acc1_many_changes_accumulate() {
    let dir = support::scratch("many");
    let path = dir.join("config.toml");
    let (mut c, _) = MenuController::load(&path);
    apply(&mut c, SettingChange::Wireframe(true), 1);
    apply(
        &mut c,
        SettingChange::ScoreDisplay(ScoreDisplayKind::Lcd),
        2,
    );
    apply(
        &mut c,
        SettingChange::BackglassPipCorner(PipCorner::TopLeft),
        3,
    );
    let l = AppConfig::load(&path).config;
    assert!(l.wireframe);
    assert_eq!(l.score_display, ScoreDisplayKind::Lcd);
    assert_eq!(l.backglass_pip.corner, PipCorner::TopLeft);
    assert_eq!(&l, c.config());
    let _ = std::fs::remove_dir_all(dir);
}

/// REQ-VIEW-006 / 4.4: unknown keys in a pre-existing file survive a save.
#[test]
fn req_view_006_unknown_keys_preserved_on_save() {
    let dir = support::scratch("unknown");
    let path = dir.join("config.toml");
    std::fs::write(
        &path,
        "future_key = 42\nwireframe = false\n\n[future_table]\nanswer = \"yes\"\n",
    )
    .unwrap();
    let (mut c, _) = MenuController::load(&path);
    apply(&mut c, SettingChange::Wireframe(true), 5);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.contains("future_key"), "{text}");
    assert!(text.contains("future_table"), "{text}");
    assert!(text.contains("answer"), "{text}");
    assert!(AppConfig::load(&path).config.wireframe);
    let _ = std::fs::remove_dir_all(dir);
}

/// REQ-VIEW-006 / 4.4: setting a value equal to the current one writes nothing
/// (no file is created).
#[test]
fn req_view_006_unchanged_value_writes_nothing_missing_file() {
    let dir = support::scratch("nochange");
    let path = dir.join("config.toml");
    let (mut c, _) = MenuController::load(&path);
    let before = c.saves();
    apply(&mut c, SettingChange::Wireframe(false), 1);
    apply(&mut c, SettingChange::DisplayMode(DisplayMode::Single), 2);
    assert!(!path.exists(), "file must not be created");
    assert_eq!(c.saves(), before);
    assert!(c.take_config_change().is_none());
    assert!(c.last_save_error().is_none());
    let _ = std::fs::remove_dir_all(dir);
}

/// REQ-VIEW-006 / 4.4: an unchanged value leaves an existing file's content
/// and modification time alone.
#[test]
fn req_view_006_unchanged_value_leaves_existing_file_untouched() {
    let dir = support::scratch("nochange2");
    let path = dir.join("config.toml");
    let (mut c, _) = MenuController::load(&path);
    apply(&mut c, SettingChange::Wireframe(true), 1);
    let saves = c.saves();
    let content = std::fs::read(&path).unwrap();
    let mtime = std::fs::metadata(&path).unwrap().modified().unwrap();
    let _ = c.take_config_change();
    apply(&mut c, SettingChange::Wireframe(true), 2);
    assert_eq!(c.saves(), saves);
    assert_eq!(std::fs::read(&path).unwrap(), content);
    assert_eq!(std::fs::metadata(&path).unwrap().modified().unwrap(), mtime);
    assert!(c.take_config_change().is_none());
    let _ = std::fs::remove_dir_all(dir);
}

/// REQ-VIEW-006 / 4.4: a missing file stays missing after load and after
/// opening and closing the menu.
#[test]
fn req_view_006_missing_file_stays_missing_after_load_and_toggle() {
    let dir = support::scratch("missing");
    let path = dir.join("config.toml");
    let (mut c, w) = MenuController::load(&path);
    assert!(w.is_empty());
    assert!(!path.exists());
    c.toggle(10, &mut sink());
    assert!(c.is_open());
    assert!(!path.exists(), "opening writes nothing");
    c.toggle(20, &mut sink());
    assert!(!c.is_open());
    assert!(!path.exists(), "closing writes nothing");
    assert_eq!(c.saves(), 0);
    assert!(c.take_config_change().is_none());
    let entries: Vec<_> = std::fs::read_dir(&dir).unwrap().collect();
    assert!(entries.is_empty(), "directory stays empty");
    let _ = std::fs::remove_dir_all(dir);
}

/// REQ-VIEW-006: take_config_change returns the latest config once, then None;
/// saves() counts successful saves.
#[test]
fn req_view_006_take_config_change_and_saves_counter() {
    let dir = support::scratch("take");
    let path = dir.join("config.toml");
    let (mut c, _) = MenuController::load(&path);
    assert_eq!(c.saves(), 0);
    apply(&mut c, SettingChange::Wireframe(true), 1);
    assert_eq!(c.saves(), 1);
    apply(
        &mut c,
        SettingChange::ScoreDisplay(ScoreDisplayKind::Lcd),
        2,
    );
    assert_eq!(c.saves(), 2);
    let latest = c.take_config_change().expect("a change is pending");
    assert!(latest.wireframe);
    assert_eq!(latest.score_display, ScoreDisplayKind::Lcd);
    assert_eq!(&latest, c.config());
    assert!(c.take_config_change().is_none(), "reported once");
    apply(&mut c, SettingChange::Wireframe(false), 3);
    assert_eq!(c.saves(), 3);
    assert_eq!(c.take_config_change().map(|c| c.wireframe), Some(false));
    assert!(c.take_config_change().is_none());
    assert!(c.last_save_error().is_none());
    let _ = std::fs::remove_dir_all(dir);
}

/// REQ-VIEW-006: a save into an unwritable place sets last_save_error and
/// does not panic.
#[test]
fn req_view_006_save_error_is_reported_not_panicked() {
    let dir = support::scratch("saveerr");
    let blocker = dir.join("blocker");
    std::fs::write(&blocker, b"i am a file").unwrap();
    let path = blocker.join("config.toml");
    let (mut c, _) = MenuController::load(&path);
    apply(&mut c, SettingChange::Wireframe(true), 1);
    assert!(c.last_save_error().is_some(), "error must be reported");
    assert_eq!(c.saves(), 0, "a failed save is not counted");
    assert!(!Path::new(&path).exists());
    let _ = std::fs::remove_dir_all(dir);
}
