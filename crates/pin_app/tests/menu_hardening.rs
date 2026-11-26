//! Black-box tests of the #131 menu and config hardening (SPEC §4 Subphase
//! 4.4; D25: scanned folders and images are untrusted): config save
//! durability and edge cases, preview byte budgets, the virtualised table
//! list and the `ui_scale` key and settings row.
//!
//! Written from the delegation brief alone (T1-T15); no wall-clock
//! assertions and no sleeping (R6).

#[path = "ui_fixtures.rs"]
mod fixtures;

use fixtures::*;
use pin_app::ui::config::{DEFAULT_UI_SCALE, MAX_UI_SCALE, MIN_UI_SCALE};
use pin_app::ui::scan::{PREVIEW_BUDGET_BYTES, enforce_preview_budget, preview_bytes};
use pin_app::ui::view::PREVIEW_TEXTURE_BUDGET_BYTES;
use pin_app::ui::{
    AppConfig, ConfigWarning, MenuFrames, MenuInput, MenuState, MenuView, NumericRange, Panel,
    PreviewArt, SettingChange, SettingRow, TableEntry, TableInfo, TableStatus, UiCommand,
    scan_tables,
};
use pin_render::ui::{
    MAX_PIXELS_PER_POINT, MIN_PIXELS_PER_POINT, effective_pixels_per_point, egui,
    menu_panel_rect_px, overlay_style, raw_input,
};
use proptest::prelude::*;
use std::path::{Path, PathBuf};

const THUMB: usize = 256 * 256 * 4;

fn dir_names(dir: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    v.sort();
    v
}

fn no_tmp_files(dir: &Path) {
    for n in dir_names(dir) {
        assert!(!n.contains("tmp"), "temporary file {n:?} left in {dir:?}");
    }
}

// ---------------------------------------------------------------- T1-T4

#[cfg(unix)]
mod unix_save {
    use super::*;
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn mode(p: &Path) -> u32 {
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    fn write_real(path: &Path, mode_bits: u32) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "wireframe = false\n").unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode_bits)).unwrap();
    }

    fn is_link(p: &Path) -> bool {
        std::fs::symlink_metadata(p)
            .unwrap()
            .file_type()
            .is_symlink()
    }

    /// Loads through `link`, turns wireframe on, saves through `link`.
    fn save_through(link: &Path) {
        let mut cfg = AppConfig::load(link).config;
        assert!(!cfg.wireframe);
        cfg.wireframe = true;
        cfg.save(link).unwrap();
    }

    /// SPEC §4.4 (#131 item 1): saving through a symlink writes the target,
    /// keeps the link and the target's mode 0600, and leaves no temp file.
    #[test]
    fn symlinked_config_save_updates_target_keeps_mode() {
        let dir = temp_dir("hard_symlink");
        let real = dir.join("real/config.toml");
        write_real(&real, 0o600);
        let link = dir.join("config.toml");
        symlink(&real, &link).unwrap();
        save_through(&link);
        assert!(is_link(&link), "the symlink was replaced by a file");
        let text = std::fs::read_to_string(&real).unwrap();
        assert!(text.contains("wireframe = true"), "{text}");
        assert_eq!(mode(&real), 0o600);
        no_tmp_files(&dir);
        no_tmp_files(&dir.join("real"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// SPEC §4.4 (#131 item 1): a relative symlink target is resolved
    /// against the link's directory.
    #[test]
    fn relative_symlink_save() {
        let dir = temp_dir("hard_relative");
        let real = dir.join("real/config.toml");
        write_real(&real, 0o600);
        let link = dir.join("config.toml");
        symlink("real/config.toml", &link).unwrap();
        save_through(&link);
        assert!(is_link(&link));
        assert!(
            std::fs::read_to_string(&real)
                .unwrap()
                .contains("wireframe = true")
        );
        assert_eq!(mode(&real), 0o600);
        no_tmp_files(&dir);
        no_tmp_files(&dir.join("real"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// SPEC §4.4 (#131 item 1): a chain link -> link -> file reaches the
    /// final target and keeps every link.
    #[test]
    fn chained_symlink_save() {
        let dir = temp_dir("hard_chain");
        let real = dir.join("real/config.toml");
        write_real(&real, 0o600);
        std::fs::create_dir_all(dir.join("b")).unwrap();
        std::fs::create_dir_all(dir.join("a")).unwrap();
        let link1 = dir.join("b/link1.toml");
        let link2 = dir.join("a/config.toml");
        symlink("../real/config.toml", &link1).unwrap();
        symlink("../b/link1.toml", &link2).unwrap();
        save_through(&link2);
        assert!(is_link(&link2));
        assert!(is_link(&link1));
        assert!(
            std::fs::read_to_string(&real)
                .unwrap()
                .contains("wireframe = true")
        );
        assert_eq!(mode(&real), 0o600);
        for d in ["a", "b", "real"] {
            no_tmp_files(&dir.join(d));
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// SPEC §4.4 (#131 item 1): a dangling symlink gets its target created.
    #[test]
    fn dangling_symlink_save_creates_target() {
        let dir = temp_dir("hard_dangling");
        std::fs::create_dir_all(dir.join("real")).unwrap();
        let target = dir.join("real/config.toml");
        let link = dir.join("config.toml");
        symlink("real/config.toml", &link).unwrap();
        assert!(!target.exists());
        let cfg = AppConfig {
            wireframe: true,
            ..AppConfig::default()
        };
        cfg.save(&link).unwrap();
        assert!(is_link(&link));
        assert!(target.is_file());
        assert!(
            std::fs::read_to_string(&target)
                .unwrap()
                .contains("wireframe = true")
        );
        assert!(AppConfig::load(&link).config.wireframe);
        no_tmp_files(&dir);
        no_tmp_files(&dir.join("real"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// SPEC §4.4 (#131 item 1): a symlink loop is an error, not a hang.
    #[test]
    fn symlink_loop_save_errors() {
        let dir = temp_dir("hard_loop");
        let a = dir.join("a.toml");
        let b = dir.join("b.toml");
        symlink("b.toml", &a).unwrap();
        symlink("a.toml", &b).unwrap();
        let cfg = AppConfig::default();
        assert!(cfg.save(&a).is_err());
        assert!(is_link(&a) && is_link(&b));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// SPEC §4.4 (#131 item 2): a save keeps the mode of a regular file.
    #[test]
    fn save_keeps_regular_file_mode() {
        let dir = temp_dir("hard_mode");
        for bits in [0o600, 0o640] {
            let p = dir.join(format!("config_{bits:o}.toml"));
            write_real(&p, bits);
            let mut cfg = AppConfig::load(&p).config;
            cfg.wireframe = true;
            cfg.save(&p).unwrap();
            assert_eq!(mode(&p), bits, "mode {bits:o} changed");
            assert!(AppConfig::load(&p).config.wireframe);
        }
        let fresh = dir.join("fresh.toml");
        AppConfig::default().save(&fresh).unwrap();
        assert_eq!(mode(&fresh) & 0o022, 0, "new file is group/world-writable");
        no_tmp_files(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// SPEC §4.4 (#131 item 3): a non-UTF-8 `last_table` / `tables_dir` is
    /// omitted and never fails the save; the other settings survive.
    #[test]
    fn non_utf8_last_table_does_not_fail_save() {
        let dir = temp_dir("hard_utf8");
        let path = dir.join("config.toml");
        let mut cfg = AppConfig {
            wireframe: true,
            playfield_display: 2,
            ..AppConfig::default()
        };
        cfg.last_table = Some(PathBuf::from(OsStr::from_bytes(b"/tables/caf\xE9")));
        cfg.tables_dir = Some(PathBuf::from(OsStr::from_bytes(b"/tables/dir\xFF\xFE")));
        let before = cfg.clone();
        assert!(cfg.to_toml_string().is_ok());
        cfg.save(&path).unwrap();
        assert_eq!(cfg, before, "save changed the in-memory config");
        let loaded = AppConfig::load(&path);
        assert!(loaded.config.wireframe);
        assert_eq!(loaded.config.playfield_display, 2);
        assert_eq!(loaded.config.last_table, None);
        assert_eq!(loaded.config.tables_dir, None);
        let mut expected = before;
        expected.last_table = None;
        expected.tables_dir = None;
        assert_eq!(loaded.config, expected);
        no_tmp_files(&dir);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// SPEC §4.4 (#131 item 4): a save into a fresh directory leaves only
/// `config.toml` and round-trips.
#[test]
fn save_leaves_no_temp_file_and_round_trips() {
    let dir = temp_dir("hard_roundtrip");
    let path = dir.join("config.toml");
    let cfg = AppConfig {
        wireframe: true,
        playfield_display: 3,
        ui_scale: 1.5,
        ..AppConfig::default()
    };
    cfg.save(&path).unwrap();
    assert_eq!(dir_names(&dir), ["config.toml"]);
    let loaded = AppConfig::load(&path);
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(loaded.config, cfg);
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------- T5

/// SPEC §4.4 (#131 item 5): datetimes in unknown keys, tables and arrays
/// are written back as datetimes, not strings.
#[test]
fn unknown_datetime_keys_stay_datetimes() {
    let src = "released = 2024-05-06T07:08:09Z\n\
               day = 2024-05-06\n\
               clock = 07:08:09\n\
               local = 2024-05-06T07:08:09\n\
               stamps = [2024-05-06T07:08:09Z]\n\
               \n\
               [future]\n\
               when = 2024-05-06T07:08:09Z\n";
    let cfg = AppConfig::from_toml_str(src).config;
    let first = cfg.to_toml_string().unwrap();
    assert!(!first.contains("\"2024-05-06"), "{first}");
    assert!(!first.contains("\"07:08:09"), "{first}");
    let table: toml::Table = toml::from_str(&first).unwrap();
    for key in ["released", "day", "clock", "local"] {
        assert!(
            matches!(table.get(key), Some(toml::Value::Datetime(_))),
            "{key} is not a datetime in\n{first}"
        );
    }
    assert!(
        matches!(table["future"]["when"], toml::Value::Datetime(_)),
        "{first}"
    );
    assert!(
        matches!(table["stamps"][0], toml::Value::Datetime(_)),
        "{first}"
    );
    // Stable on a second round.
    let again = AppConfig::from_toml_str(&first).config;
    assert_eq!(again, cfg);
    assert_eq!(again.to_toml_string().unwrap(), first);

    // Through the file path as well.
    let dir = temp_dir("hard_datetime");
    let path = dir.join("config.toml");
    std::fs::write(&path, src).unwrap();
    let loaded = AppConfig::load(&path);
    loaded.config.save(&path).unwrap();
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("\"2024-05-06"), "{text}");
    let table: toml::Table = toml::from_str(&text).unwrap();
    assert!(matches!(table["released"], toml::Value::Datetime(_)));
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------------- T6

fn has_long_fraction(text: &str) -> bool {
    let b = text.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'.' {
            let n = b[i + 1..].iter().take_while(|c| c.is_ascii_digit()).count();
            if n >= 9 {
                return true;
            }
        }
        i += 1;
    }
    false
}

/// SPEC §4.4 (#131 item 6): f32 values are written as shortest round-trip
/// decimals and reload bit-identical.
#[test]
fn f32_values_are_shortest_round_trip() {
    let src = "ui_scale = 1.1\n\
               [score_pip]\n\
               width_fraction = 0.3\n\
               [calibration.nudge]\n\
               deadzone = 0.1\n\
               [calibration.plunger]\n\
               deadzone = 0.1\n";
    let l = AppConfig::from_toml_str(src);
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    let text = l.config.to_toml_string().unwrap();
    assert!(text.lines().any(|l| l == "deadzone = 0.1"), "{text}");
    assert!(text.lines().any(|l| l == "width_fraction = 0.3"), "{text}");
    assert!(text.lines().any(|l| l == "ui_scale = 1.1"), "{text}");
    assert!(!text.contains("0.10000000149011612"), "{text}");
    assert!(!has_long_fraction(&text), "{text}");
    let again = AppConfig::from_toml_str(&text).config;
    assert_eq!(again.calibration.nudge.deadzone.to_bits(), 0.1f32.to_bits());
    assert_eq!(
        again.calibration.plunger.deadzone.to_bits(),
        0.1f32.to_bits()
    );
    assert_eq!(again.score_pip.width_fraction.to_bits(), 0.3f32.to_bits());
    assert_eq!(again.ui_scale.to_bits(), 1.1f32.to_bits());
}

/// The text of the plunger `deadzone` value in a saved config.
fn plunger_deadzone_text(text: &str) -> String {
    let from = text.find("[calibration.plunger]").unwrap_or(0);
    text[from..]
        .lines()
        .find_map(|l| l.strip_prefix("deadzone = "))
        .unwrap_or_else(|| panic!("no plunger deadzone in\n{text}"))
        .to_owned()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// SPEC §4.4 (#131 item 6): any in-range f32 survives save and load
    /// bit-identically and is written with at most 9 significant digits.
    #[test]
    fn f32_sweep_round_trips(x in 0.001f32..=0.5f32) {
        let reference = AppConfig::from_toml_str(&format!(
            "[calibration.plunger]\ndeadzone = {x:?}\n"
        ))
        .config
        .calibration
        .plunger
        .deadzone;
        let mut cfg = AppConfig::default();
        cfg.calibration.plunger.deadzone = reference;
        let text = cfg.to_toml_string().unwrap();
        let loaded = AppConfig::from_toml_str(&text).config;
        prop_assert_eq!(loaded.calibration.plunger.deadzone.to_bits(), reference.to_bits());
        let token = plunger_deadzone_text(&text);
        let sig = token
            .chars()
            .filter(char::is_ascii_digit)
            .skip_while(|c| *c == '0')
            .count();
        prop_assert!(sig <= 9, "{token} has {sig} significant digits");
    }
}

// ------------------------------------------------------------------- T7

fn invalid_keys(w: &[ConfigWarning]) -> Vec<&str> {
    w.iter()
        .filter_map(|w| match w {
            ConfigWarning::InvalidValue { key, .. } => Some(key.as_str()),
            _ => None,
        })
        .collect()
}

/// SPEC §4.4 (#131 item 7): the `ui_scale` key: default, valid values and
/// boundaries, every invalid spelling, persistence and unknown keys.
#[test]
fn ui_scale_key() {
    assert_eq!(DEFAULT_UI_SCALE, 1.0);
    assert_eq!(MIN_UI_SCALE, 0.5);
    assert_eq!(MAX_UI_SCALE, 4.0);
    assert_eq!(AppConfig::default().ui_scale, DEFAULT_UI_SCALE);

    let l = AppConfig::from_toml_str("wireframe = true\n");
    assert_eq!(l.config.ui_scale, 1.0);
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);

    for (text, want) in [("2.0", 2.0f32), ("0.5", 0.5), ("4.0", 4.0), ("1.25", 1.25)] {
        let l = AppConfig::from_toml_str(&format!("ui_scale = {text}\n"));
        assert_eq!(l.config.ui_scale, want, "ui_scale = {text}");
        assert!(l.warnings.is_empty(), "{text}: {:?}", l.warnings);
    }

    for text in [
        "0.25", "4.5", "-1.0", "nan", "inf", "-inf", "\"big\"", "0.0",
    ] {
        let l = AppConfig::from_toml_str(&format!("wireframe = true\nui_scale = {text}\n"));
        assert_eq!(l.config.ui_scale, 1.0, "ui_scale = {text}");
        assert!(l.config.wireframe, "rest of the file lost for {text}");
        assert_eq!(l.warnings.len(), 1, "{text}: {:?}", l.warnings);
        assert_eq!(invalid_keys(&l.warnings), ["ui_scale"], "{text}");
        assert!(
            !l.warnings
                .iter()
                .any(|w| matches!(w, ConfigWarning::UnknownKey(_))),
            "{text}: reported as unknown"
        );
    }

    // Saved as a float literal and unknown keys still survive.
    let mut cfg = AppConfig::from_toml_str("future_key = 5\n").config;
    cfg.ui_scale = 2.0;
    let text = cfg.to_toml_string().unwrap();
    assert!(text.lines().any(|l| l == "ui_scale = 2.0"), "{text}");
    assert!(text.contains("future_key = 5"), "{text}");
    let l = AppConfig::from_toml_str(&text);
    assert_eq!(l.config.ui_scale, 2.0);
    let old = AppConfig::from_toml_str("future_key = 5\nwireframe = true\n");
    assert!(old.config.extra.contains_key("future_key"));
    assert!(
        old.warnings
            .iter()
            .any(|w| matches!(w, ConfigWarning::UnknownKey(k) if k == "future_key"))
    );
}

// ------------------------------------------------------------------- T8

/// SPEC §4.4 (#131 item 7): `SettingRow::UiScale` sits after `ScoreDisplay`
/// and before `DecodedAudioBudget` with the documented range.
#[test]
fn ui_scale_settings_row() {
    let pos = |f: fn(&SettingRow) -> bool| SettingRow::DISPLAY.iter().position(f).unwrap();
    let score = pos(|r| matches!(r, SettingRow::ScoreDisplay));
    let scale = pos(|r| matches!(r, SettingRow::UiScale));
    let budget = pos(|r| matches!(r, SettingRow::DecodedAudioBudget));
    assert_eq!(scale, score + 1);
    assert_eq!(budget, scale + 1);
    assert_eq!(SettingRow::DISPLAY.len(), 12);

    let r = SettingRow::UiScale.range().expect("a range");
    assert!(
        r == NumericRange {
            min: 0.5,
            max: 4.0,
            step: 0.25
        },
        "{r:?}"
    );
    let cfg = AppConfig {
        ui_scale: 2.5,
        ..AppConfig::default()
    };
    assert_eq!(SettingRow::UiScale.number(&cfg), Some(2.5));
    let label: String = SettingRow::UiScale.label().into();
    assert!(label.to_lowercase().contains("scale"), "{label}");
    assert!(!SettingRow::UiScale.value_text(&cfg).is_empty());

    // Through the menu state machine.
    let mut m = MenuState::default();
    m.apply(MenuInput::Toggle);
    m.apply(MenuInput::ShowPanel(Panel::Display));
    for _ in 0..scale + 1 {
        m.apply(MenuInput::Down);
    }
    assert!(matches!(m.focused_setting(), Some(SettingRow::UiScale)));
    m.drain_commands();

    let changed = |m: &mut MenuState, want: f32| {
        let cmds = m.drain_commands();
        assert!(
            cmds.iter()
                .any(|c| matches!(c, UiCommand::ConfigChanged(c) if c.ui_scale == want)),
            "no ConfigChanged carrying {want}"
        );
    };
    m.apply(MenuInput::Right);
    assert_eq!(m.config().ui_scale, 1.25);
    changed(&mut m, 1.25);
    m.apply(MenuInput::Left);
    m.apply(MenuInput::Left);
    assert_eq!(m.config().ui_scale, 0.75);
    changed(&mut m, 0.75);
    m.apply(MenuInput::Set(SettingChange::UiScale(9.0)));
    assert_eq!(m.config().ui_scale, 4.0);
    changed(&mut m, 4.0);
    m.apply(MenuInput::Set(SettingChange::UiScale(f32::NAN)));
    assert_eq!(m.config().ui_scale, 1.0);
    changed(&mut m, 1.0);
}

// ------------------------------------------------------------------- T9

/// SPEC §4.4 (#131 item 7): the effective scale multiplies, sanitises and
/// clamps.
#[test]
fn effective_pixels_per_point_values() {
    assert_eq!(effective_pixels_per_point(1.0, 2.0), 2.0);
    assert_eq!(effective_pixels_per_point(2.0, 1.5), 3.0);
    assert_eq!(effective_pixels_per_point(1.0, f32::NAN), 1.0);
    assert_eq!(effective_pixels_per_point(1.0, 0.0), 1.0);
    assert_eq!(effective_pixels_per_point(1.0, -3.0), 1.0);
    assert_eq!(effective_pixels_per_point(1.0, f32::INFINITY), 1.0);
    assert_eq!(effective_pixels_per_point(4.0, 4.0), MAX_PIXELS_PER_POINT);
    assert_eq!(effective_pixels_per_point(4.0, 4.0), 8.0);
    assert_eq!(effective_pixels_per_point(1.0, 0.1), MIN_PIXELS_PER_POINT);
    assert_eq!(effective_pixels_per_point(1.0, 0.1), 0.25);
}

// ------------------------------------------------------------------ T10

/// SPEC §4.4 (#131 item 7): the menu frame is laid out at `ui_scale` and
/// stays inside the panel rectangle.
#[test]
fn menu_frame_scales_with_ui_scale() {
    const SIZE: [u32; 2] = [3840, 2160];
    let mut areas = Vec::new();
    for scale in [1.0f32, 2.0] {
        let mut state = MenuState::default();
        state.apply(MenuInput::Toggle);
        state.apply(MenuInput::Set(SettingChange::UiScale(scale)));
        assert_eq!(state.config().ui_scale, scale);
        let mut frames = MenuFrames::new();
        frames.set_state(state);
        let mut frame = None;
        for _ in 0..3 {
            frame = frames.frame(SIZE);
        }
        let frame = frame.expect("an open menu draws");
        assert_eq!(frame.pixels_per_point, scale);
        let ppp = frame.pixels_per_point;
        let [x0, y0, x1, y1] = menu_panel_rect_px(SIZE, ppp);
        // Primitives are in points: the panel in points is px / ppp.
        let pts = [x0, y0, x1, y1].map(|v| v as f32 / ppp);
        let (mut ux0, mut uy0, mut ux1, mut uy1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        assert!(!frame.primitives.is_empty());
        for p in &frame.primitives {
            let c = p.clip_rect;
            let clip_inside = c.min.x >= pts[0] - 0.5
                && c.min.y >= pts[1] - 0.5
                && c.max.x <= pts[2] + 0.5
                && c.max.y <= pts[3] + 0.5;
            let egui::epaint::Primitive::Mesh(m) = &p.primitive else {
                panic!("unexpected callback primitive");
            };
            let verts_inside = m.vertices.iter().all(|v| {
                v.pos.x >= pts[0] - 0.5
                    && v.pos.y >= pts[1] - 0.5
                    && v.pos.x <= pts[2] + 0.5
                    && v.pos.y <= pts[3] + 0.5
            });
            let empty_clip = c.width() <= 0.0 || c.height() <= 0.0;
            assert!(
                clip_inside || verts_inside || empty_clip,
                "primitive (clip {c:?}) at ppp {ppp} outside panel {:?}",
                [x0, y0, x1, y1]
            );
            if empty_clip || m.vertices.is_empty() {
                continue;
            }
            // Effective drawn bounds: vertex bounding box clipped to the clip rect.
            let (mut vx0, mut vy0, mut vx1, mut vy1) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
            for v in &m.vertices {
                vx0 = vx0.min(v.pos.x);
                vy0 = vy0.min(v.pos.y);
                vx1 = vx1.max(v.pos.x);
                vy1 = vy1.max(v.pos.y);
            }
            let (dx0, dy0) = (vx0.max(c.min.x), vy0.max(c.min.y));
            let (dx1, dy1) = (vx1.min(c.max.x), vy1.min(c.max.y));
            if dx1 <= dx0 || dy1 <= dy0 {
                continue;
            }
            ux0 = ux0.min(dx0 * ppp);
            uy0 = uy0.min(dy0 * ppp);
            ux1 = ux1.max(dx1 * ppp);
            uy1 = uy1.max(dy1 * ppp);
        }
        areas.push(f64::from((ux1 - ux0).max(0.0)) * f64::from((uy1 - uy0).max(0.0)));
        frame.discard();
    }
    let ratio = areas[1] / areas[0];
    assert!(
        (2.0..=6.0).contains(&ratio),
        "area ratio {ratio} ({areas:?})"
    );
}

// ------------------------------------------------------------------ T11

fn thumb_entry(k: usize, w: u32, h: u32) -> TableEntry {
    TableEntry {
        path: PathBuf::from(format!("/t/table_{k:04}")),
        file_name: format!("table_{k:04}"),
        status: TableStatus::Ready(TableInfo {
            id: format!("id_{k}"),
            name: format!("table_{k:04}"),
            author: "A".into(),
            version: "1".into(),
            players_max: 1,
            features: vec![],
            preview: Some(PreviewArt {
                file: "preview.png".into(),
                width: w,
                height: h,
                rgba: vec![0u8; (w * h * 4) as usize],
            }),
            preview_error: if k.is_multiple_of(7) {
                Some("note".into())
            } else {
                None
            },
            warning_count: 0,
        }),
    }
}

fn has_preview(e: &TableEntry) -> bool {
    matches!(&e.status, TableStatus::Ready(i) if i.preview.is_some())
}

type Identity = (PathBuf, String, Option<(String, Option<String>)>);

fn identity(e: &TableEntry) -> Identity {
    let info = match &e.status {
        TableStatus::Ready(i) => Some((i.id.clone(), i.preview_error.clone())),
        _ => None,
    };
    (e.path.clone(), e.file_name.clone(), info)
}

/// SPEC §4.4, D25 (#131 item 8): 4,096 entries with 256x256 previews end
/// under the byte budget, keeping the first entries' previews.
#[test]
fn scan_budget_4096_entries() {
    let mut entries: Vec<TableEntry> = (0..4096).map(|k| thumb_entry(k, 256, 256)).collect();
    assert_eq!(preview_bytes(&entries), 4096 * THUMB as u64);
    let before: Vec<Identity> = entries.iter().map(identity).collect();
    enforce_preview_budget(&mut entries, PREVIEW_BUDGET_BYTES);
    assert!(preview_bytes(&entries) <= PREVIEW_BUDGET_BYTES);
    let keep = (PREVIEW_BUDGET_BYTES / THUMB as u64) as usize;
    assert_eq!(keep, 256);
    for (k, e) in entries.iter().enumerate() {
        assert_eq!(has_preview(e), k < keep, "entry {k}");
        assert!(e.is_ready());
    }
    let after: Vec<Identity> = entries.iter().map(identity).collect();
    assert_eq!(before, after);
    // Idempotent.
    enforce_preview_budget(&mut entries, PREVIEW_BUDGET_BYTES);
    assert_eq!(entries.iter().filter(|e| has_preview(e)).count(), keep);
    assert_eq!(preview_bytes(&entries), keep as u64 * THUMB as u64);
    // Zero clears all.
    enforce_preview_budget(&mut entries, 0);
    assert_eq!(preview_bytes(&entries), 0);
    assert!(entries.iter().all(|e| e.is_ready() && !has_preview(e)));
    let after: Vec<Identity> = entries.iter().map(identity).collect();
    assert_eq!(before, after);
}

/// SPEC §4.4, D25 (#131 item 8): the contract holds chunk by chunk with the
/// remaining budget, an exact-one-thumbnail budget keeps one, mixed sizes
/// count by `rgba.len()`, broken entries count zero.
#[test]
fn enforce_preview_budget_contract() {
    // Chunks with a shrinking remaining budget.
    let mut remaining = PREVIEW_BUDGET_BYTES;
    let mut kept = 0usize;
    for c in 0..8 {
        let mut chunk: Vec<TableEntry> = (0..512)
            .map(|k| thumb_entry(c * 512 + k, 256, 256))
            .collect();
        enforce_preview_budget(&mut chunk, remaining);
        let used = preview_bytes(&chunk);
        assert!(used <= remaining);
        kept += chunk.iter().filter(|e| has_preview(e)).count();
        remaining -= used;
        assert!(chunk.iter().all(|e| e.is_ready()));
    }
    assert_eq!(kept, 256);
    assert_eq!(remaining, 0);

    // Exactly one thumbnail.
    let mut v: Vec<TableEntry> = (0..5).map(|k| thumb_entry(k, 256, 256)).collect();
    enforce_preview_budget(&mut v, THUMB as u64);
    assert_eq!(v.iter().filter(|e| has_preview(e)).count(), 1);
    assert!(has_preview(&v[0]));

    // Mixed sizes counted by rgba.len(); broken entries are free.
    let broken = || TableEntry {
        path: PathBuf::from("/t/broken"),
        file_name: "broken".into(),
        status: TableStatus::Broken("bad".into()),
    };
    let mk = || {
        vec![
            thumb_entry(0, 1, 1),
            broken(),
            thumb_entry(1, 17, 5),
            thumb_entry(2, 256, 256),
            thumb_entry(3, 1, 1),
        ]
    };
    let total = (4 + 340 + THUMB) as u64;
    let v = mk();
    assert_eq!(preview_bytes(&v), total + 4);
    let mut v = mk();
    enforce_preview_budget(&mut v, total);
    assert_eq!(preview_bytes(&v), total);
    assert!(has_preview(&v[0]) && has_preview(&v[2]) && has_preview(&v[3]));
    assert!(!has_preview(&v[4]));
    assert!(matches!(&v[1].status, TableStatus::Broken(m) if m == "bad"));
    let mut v = mk();
    enforce_preview_budget(&mut v, total - 1);
    assert!(has_preview(&v[0]) && has_preview(&v[2]));
    assert!(!has_preview(&v[3]) && !has_preview(&v[4]));
    assert!(preview_bytes(&v) < total);
    assert!(matches!(&v[1].status, TableStatus::Broken(m) if m == "bad"));
}

// ------------------------------------------------------------------ T12

/// SPEC §4.4, D25 (#131 item 8): a real scan of 24 valid bundles with
/// 256x256 previews plus a broken one stays under the budget.
#[test]
fn real_scan_stays_under_budget() {
    let root = temp_dir("hard_scan");
    let files = bundle_files();
    let preview = png(256, 256, |_, _| [10, 200, 30, 255]);
    let with_preview = with_file(&files, "preview.png", &preview);
    for k in 0..24 {
        write_dir(&root.join(format!("t{k:02}")), &with_preview);
    }
    write_dir(
        &root.join("z_broken"),
        &with_file(&files, "table.toml", b"this is [not toml"),
    );
    let entries = scan_tables(&root);
    assert_eq!(entries.len(), 25);
    let names: Vec<&str> = entries.iter().map(|e| e.file_name.as_str()).collect();
    let mut want: Vec<String> = (0..24).map(|k| format!("t{k:02}")).collect();
    want.push("z_broken".into());
    assert_eq!(names, want);
    assert!(entries[..24].iter().all(|e| e.is_ready()));
    assert!(!entries[24].is_ready());
    assert!(preview_bytes(&entries) <= PREVIEW_BUDGET_BYTES);
    assert_eq!(preview_bytes(&entries), 24 * THUMB as u64);
    let _ = std::fs::remove_dir_all(&root);
}

// ------------------------------------------------------------------ T13

fn plain_tables(n: usize) -> Vec<TableEntry> {
    (0..n)
        .map(|k| TableEntry {
            path: PathBuf::from(format!("/t/table_{k:04}")),
            file_name: format!("table_{k:04}"),
            status: TableStatus::Ready(TableInfo {
                id: format!("table_{k:04}"),
                name: format!("table_{k:04}"),
                author: "A".into(),
                version: "1".into(),
                players_max: 1,
                features: vec![],
                preview: None,
                preview_error: None,
                warning_count: 0,
            }),
        })
        .collect()
}

struct Harness {
    ctx: egui::Context,
    view: MenuView,
    time: f64,
}

impl Harness {
    fn new() -> Self {
        let ctx = egui::Context::default();
        overlay_style(&ctx);
        Harness {
            ctx,
            view: MenuView::new(),
            time: 0.0,
        }
    }

    /// One frame; returns the number of shapes.
    fn frame(&mut self, state: &mut MenuState) -> usize {
        self.time += 1.0 / 60.0;
        let raw = raw_input([1600, 900], 1.0, Vec::new(), Some(self.time));
        let (out, _inputs) = self.view.run_frame(&self.ctx, raw, state);
        let n = out.shapes.len();
        out.drop_without_applying_deltas();
        n
    }

    fn settle(&mut self, state: &mut MenuState, frames: usize) -> usize {
        let mut n = 0;
        for _ in 0..frames {
            n = self.frame(state);
        }
        n
    }
}

fn open_with(tables: Vec<TableEntry>) -> MenuState {
    let mut m = MenuState::default();
    m.set_tables(tables);
    m.apply(MenuInput::Toggle);
    m.apply(MenuInput::ShowPanel(Panel::Tables));
    m
}

/// SPEC §4.4 (#131 item 9): the table list is virtualised: a frame over
/// 4,096 tables emits a bounded number of shapes.
#[test]
fn table_list_is_virtualised() {
    let mut h = Harness::new();
    let mut small = open_with(plain_tables(8));
    let small_shapes = h.settle(&mut small, 3);

    let mut h = Harness::new();
    let mut big = open_with(plain_tables(4096));
    let big_shapes = h.settle(&mut big, 3);
    assert!(big_shapes < 600, "{big_shapes} shapes for 4096 tables");
    assert!(
        big_shapes <= 3 * small_shapes + 300,
        "{big_shapes} vs small {small_shapes}"
    );

    big.apply(MenuInput::FocusTable(4095));
    let last = h.settle(&mut big, 3);
    assert!(last < 600, "{last} shapes with the last table focused");
    assert_eq!(big.focused_table(), Some(4095));

    big.apply(MenuInput::SetFilter("table_0042".into()));
    assert_eq!(big.visible_tables().iter().count(), 1);
    let filtered = h.settle(&mut big, 3);
    assert!(filtered > 0 && filtered < 600, "{filtered}");
}

// ------------------------------------------------------------------ T14

fn preview_tables(n: usize) -> Vec<TableEntry> {
    let shared = vec![128u8; THUMB];
    (0..n)
        .map(|k| {
            let mut e = plain_tables(1).remove(0);
            e.path = PathBuf::from(format!("/t/table_{k:04}"));
            e.file_name = format!("table_{k:04}");
            if let TableStatus::Ready(i) = &mut e.status {
                i.id = format!("table_{k:04}");
                i.name = format!("table_{k:04}");
                i.preview = Some(PreviewArt {
                    file: "preview.png".into(),
                    width: 256,
                    height: 256,
                    rgba: shared.clone(),
                });
            }
            e
        })
        .collect()
}

fn texture_invariants(view: &MenuView) {
    assert!(view.preview_texture_bytes() <= PREVIEW_TEXTURE_BUDGET_BYTES);
    assert_eq!(
        view.preview_texture_count() as u64 * THUMB as u64,
        view.preview_texture_bytes()
    );
}

/// SPEC §4.4, D25 (#131 item 10): the uploaded preview textures stay under
/// the byte budget while scrolling through many tables, and are dropped
/// with their tables.
#[test]
fn preview_textures_stay_under_budget_and_are_pruned() {
    const N: usize = 300;
    assert!(N as u64 * THUMB as u64 > PREVIEW_TEXTURE_BUDGET_BYTES);
    let mut h = Harness::new();
    let mut state = open_with(preview_tables(N));
    for i in 0..N {
        state.apply(MenuInput::FocusTable(i));
        h.frame(&mut state);
        texture_invariants(&h.view);
    }
    h.settle(&mut state, 2);
    texture_invariants(&h.view);
    let count = h.view.preview_texture_count();
    assert!((1..=256).contains(&count), "{count} textures");

    state.set_tables(Vec::new());
    h.settle(&mut state, 2);
    assert_eq!(h.view.preview_texture_count(), 0);
    assert_eq!(h.view.preview_texture_bytes(), 0);

    state.set_tables(preview_tables(5));
    for i in 0..5 {
        state.apply(MenuInput::FocusTable(i));
        h.settle(&mut state, 2);
        texture_invariants(&h.view);
    }
    assert_eq!(h.view.preview_texture_count(), 5);
    state.set_tables(preview_tables(2));
    h.settle(&mut state, 2);
    assert_eq!(h.view.preview_texture_count(), 2);
    texture_invariants(&h.view);
}

// ------------------------------------------------------------------ T15

fn check_survivor(l: &pin_app::ui::ConfigLoad) -> Result<(), TestCaseError> {
    let c = &l.config;
    prop_assert!(c.ui_scale.is_finite());
    prop_assert!((MIN_UI_SCALE..=MAX_UI_SCALE).contains(&c.ui_scale));
    let text = c.to_toml_string();
    prop_assert!(text.is_ok());
    let text = text.unwrap();
    let again = AppConfig::from_toml_str(&text);
    prop_assert!(
        again.config == *c,
        "round trip changed the config:\n{}",
        text
    );
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// D25 (#131 item 11): bounded random bytes never panic the config
    /// parser and always give a sane, re-loadable config.
    #[test]
    fn config_fuzz_still_survives_random_bytes(
        bytes in proptest::collection::vec(any::<u8>(), 0..4096)
    ) {
        let l = AppConfig::from_toml_bytes(&bytes);
        check_survivor(&l)?;
    }

    /// D25 (#131 item 11): random `ui_scale` tokens and datetimes.
    #[test]
    fn config_fuzz_ui_scale_and_datetimes(
        token in "[-+0-9.eEnaifNIA_]{0,12}",
        dt in "20[0-9]{2}-[0-9]{2}-[0-9]{2}(T[0-9]{2}:[0-9]{2}:[0-9]{2}Z?)?",
        quoted in any::<bool>(),
    ) {
        let tok = if quoted { format!("\"{token}\"") } else { token };
        let text = format!("ui_scale = {tok}\nstamp = {dt}\n[future]\nwhen = {dt}\n");
        let l = AppConfig::from_toml_str(&text);
        check_survivor(&l)?;
    }
}
