//! `config.toml` persistence tests (4.4 verification: "config.toml
//! persistence"; gate 5: the parser survives bounded random bytes).

#[path = "ui_fixtures.rs"]
mod fixtures;

use fixtures::temp_dir;
use pin_app::ui::{
    AppConfig, CONFIG_FILE, CameraPreset, ConfigWarning, DisplayMode, MAX_CONFIG_BYTES, PipCorner,
    ScoreDisplayKind, StagingMode,
};
use proptest::prelude::*;
use std::path::PathBuf;

fn custom() -> AppConfig {
    let mut c = AppConfig {
        display_mode: DisplayMode::Multi,
        playfield_display: 1,
        backglass_display: Some(2),
        score_display_monitor: None,
        camera_preset: CameraPreset::Cabinet,
        score_display: ScoreDisplayKind::Lcd,
        input_profile: "xarcade_tankstick".into(),
        staging_mode: StagingMode::DualStage,
        wireframe: true,
        tables_dir: Some(PathBuf::from("/srv/tables")),
        last_table: Some(PathBuf::from("/srv/tables/apex")),
        ..AppConfig::default()
    };
    c.backglass_pip.corner = PipCorner::BottomLeft;
    c.backglass_pip.width_fraction = 0.3;
    c.score_pip.width_fraction = 0.15;
    c.calibration.plunger.deadzone = 0.02;
    c.calibration.plunger.release_window_ms = 80;
    c.calibration.nudge.gain = 2.5;
    c.calibration.dual_stage_lower = 0.2;
    c.calibration.dual_stage_upper = 0.7;
    c.audio.decoded_budget_mib = 1024;
    c
}

#[test]
fn save_load_round_trip() {
    let dir = temp_dir("roundtrip");
    let path = dir.join(CONFIG_FILE);
    let c = custom();
    c.save(&path).unwrap();
    let loaded = AppConfig::load(&path);
    assert_eq!(loaded.warnings, []);
    assert_eq!(loaded.config, c);
    // No temporary file is left behind.
    let names: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    assert_eq!(names, [CONFIG_FILE]);
    // Keys and enum spellings are the documented ones.
    let text = std::fs::read_to_string(&path).unwrap();
    for needle in [
        "display_mode = \"multi\"",
        "camera_preset = \"cabinet\"",
        "score_display = \"lcd\"",
        "staging_mode = \"dual_stage\"",
        "corner = \"bottom_left\"",
        "decoded_budget_mib = 1024",
        "wireframe = true",
    ] {
        assert!(text.contains(needle), "{needle:?} missing in\n{text}");
    }
    // Overwrite with defaults.
    AppConfig::default().save(&path).unwrap();
    assert_eq!(AppConfig::load(&path).config, AppConfig::default());
}

#[test]
fn missing_file_gives_defaults_silently() {
    let dir = temp_dir("missing");
    let l = AppConfig::load(&dir.join(CONFIG_FILE));
    assert_eq!(l.config, AppConfig::default());
    assert!(l.warnings.is_empty());
    assert_eq!(AppConfig::from_toml_str("").config, AppConfig::default());
}

#[test]
fn corrupt_file_falls_back_with_warning() {
    let dir = temp_dir("corrupt");
    let path = dir.join(CONFIG_FILE);
    for bad in [
        &b"display_mode = \"multi\"\n[[[nope"[..],
        b"\xff\xfe\x00 not utf-8",
    ] {
        std::fs::write(&path, bad).unwrap();
        let l = AppConfig::load(&path);
        assert_eq!(l.config, AppConfig::default());
        assert_eq!(l.warnings.len(), 1, "{:?}", l.warnings);
        assert!(matches!(l.warnings[0], ConfigWarning::Corrupt(_)));
        assert!(l.warnings[0].to_string().contains("defaults"));
    }
    // Too large.
    std::fs::write(&path, vec![b'#'; MAX_CONFIG_BYTES as usize + 10]).unwrap();
    let l = AppConfig::load(&path);
    assert_eq!(l.warnings, [ConfigWarning::TooLarge(MAX_CONFIG_BYTES + 10)]);
    assert!(matches!(
        AppConfig::from_toml_bytes(&vec![b' '; MAX_CONFIG_BYTES as usize + 1]).warnings[..],
        [ConfigWarning::TooLarge(_)]
    ));
    // A directory is unreadable.
    let l = AppConfig::load(&dir);
    assert_eq!(l.config, AppConfig::default());
    assert!(matches!(l.warnings[..], [ConfigWarning::Unreadable(_)]));
    assert!(!l.warnings[0].to_string().is_empty());
}

#[test]
fn bad_values_reset_individually() {
    let text = r#"
wireframe = true
playfield_display = 99
backglass_display = 40
camera_preset = "top_down"
input_profile = ""
[score_pip]
corner = "top_left"
width_fraction = 7.0
[calibration]
dual_stage_lower = 0.9
dual_stage_upper = 0.1
[calibration.plunger]
deadzone = -1.0
release_window_ms = 0
[calibration.nudge]
gain = 11.0
curve_exponent = 0.01
max = nan
"#;
    let l = AppConfig::from_toml_str(text);
    let d = AppConfig::default();
    let c = &l.config;
    assert!(c.wireframe);
    assert_eq!(c.camera_preset, CameraPreset::TopDown);
    assert_eq!(c.score_pip.corner, PipCorner::TopLeft);
    assert_eq!(c.playfield_display, 0);
    assert_eq!(c.backglass_display, None);
    assert_eq!(c.input_profile, "auto");
    assert_eq!(c.score_pip.width_fraction, d.score_pip.width_fraction);
    assert_eq!(c.calibration, d.calibration);
    let keys: Vec<&str> = l
        .warnings
        .iter()
        .map(|w| match w {
            ConfigWarning::InvalidValue { key, .. } => key.as_str(),
            other => panic!("unexpected {other:?}"),
        })
        .collect();
    for k in [
        "playfield_display",
        "backglass_display",
        "input_profile",
        "score_pip.width_fraction",
        "calibration.dual_stage_lower",
        "calibration.plunger.deadzone",
        "calibration.plunger.release_window_ms",
        "calibration.nudge.gain",
        "calibration.nudge.curve_exponent",
        "calibration.nudge.max",
    ] {
        assert!(keys.contains(&k), "{k} not warned: {keys:?}");
    }
    for w in &l.warnings {
        assert!(w.to_string().contains("default"));
    }
}

#[test]
fn audio_budget_setting() {
    let ok = AppConfig::from_toml_str("[audio]\ndecoded_budget_mib = 2048\n");
    assert!(ok.warnings.is_empty(), "{:?}", ok.warnings);
    assert_eq!(ok.config.audio.decoded_budget_mib, 2048);
    assert_eq!(AppConfig::default().audio.decoded_budget_mib, 512);
    for (text, key) in [
        (
            "[audio]\ndecoded_budget_mib = 63\n",
            "audio.decoded_budget_mib",
        ),
        (
            "[audio]\ndecoded_budget_mib = 8193\n",
            "audio.decoded_budget_mib",
        ),
        (
            "[audio]\ndecoded_budget_mib = -5\n",
            "audio.decoded_budget_mib",
        ),
        (
            "[audio]\ndecoded_budget_mib = 99999999999\n",
            "audio.decoded_budget_mib",
        ),
        (
            "[audio]\ndecoded_budget_mib = \"lots\"\n",
            "audio.decoded_budget_mib",
        ),
        (
            "[audio]\ndecoded_budget_mib = 512.5\n",
            "audio.decoded_budget_mib",
        ),
        ("audio = 7\n", "audio"),
    ] {
        let l = AppConfig::from_toml_str(&format!("wireframe = true\n{text}"));
        assert_eq!(l.config.audio.decoded_budget_mib, 512, "{text}");
        assert!(l.config.wireframe, "{text}: other settings kept");
        assert!(
            matches!(&l.warnings[..], [ConfigWarning::InvalidValue { key: k, .. }] if k == key),
            "{text}: {:?}",
            l.warnings
        );
    }
}

#[test]
fn unknown_keys_are_preserved_and_reported() {
    let dir = temp_dir("unknown");
    let path = dir.join(CONFIG_FILE);
    let text = r#"
future_top = "keep me"
wireframe = true
[future_section]
a = 1
[calibration.plunger]
deadzone = 0.01
future_nested = [1, 2]
[audio]
future_audio = true
"#;
    std::fs::write(&path, text).unwrap();
    let l = AppConfig::load(&path);
    let unknown: Vec<&str> = l
        .warnings
        .iter()
        .filter_map(|w| match w {
            ConfigWarning::UnknownKey(k) => Some(k.as_str()),
            _ => None,
        })
        .collect();
    assert_eq!(
        unknown,
        [
            "future_section",
            "future_top",
            "calibration.plunger.future_nested",
            "audio.future_audio"
        ]
    );
    assert_eq!(l.warnings.len(), 4);
    assert!(l.config.wireframe);
    assert_eq!(l.config.calibration.plunger.deadzone, 0.01);
    l.config.save(&path).unwrap();
    let again = AppConfig::load(&path);
    assert_eq!(again.config, l.config);
    let saved: toml::Table = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(saved["future_top"].as_str(), Some("keep me"));
    assert_eq!(saved["future_section"]["a"].as_integer(), Some(1));
    assert_eq!(
        saved["calibration"]["plunger"]["future_nested"]
            .as_array()
            .map(|a| a.len()),
        Some(2)
    );
    assert_eq!(saved["audio"]["future_audio"].as_bool(), Some(true));
    assert_eq!(saved["audio"]["decoded_budget_mib"].as_integer(), Some(512));
}

#[test]
fn save_is_atomic_and_cleans_up_on_error() {
    let dir = temp_dir("atomic");
    let path = dir.join(CONFIG_FILE);
    custom().save(&path).unwrap();
    let before = std::fs::read(&path).unwrap();
    // The rename target is a non-empty directory: the rename fails, the
    // temporary file is removed and nothing else changes.
    let blocked = dir.join("blocked");
    std::fs::create_dir_all(blocked.join("child")).unwrap();
    assert!(AppConfig::default().save(&blocked).is_err());
    assert!(!dir.join(".blocked.tmp").exists());
    assert_eq!(std::fs::read(&path).unwrap(), before);
    // No file name.
    assert!(
        AppConfig::default()
            .save(std::path::Path::new("/"))
            .is_err()
    );
    // A stale temporary file from a crash is overwritten.
    std::fs::write(dir.join(format!(".{CONFIG_FILE}.tmp")), b"junk").unwrap();
    AppConfig::default().save(&path).unwrap();
    assert!(!dir.join(format!(".{CONFIG_FILE}.tmp")).exists());
    assert_eq!(AppConfig::load(&path).config, AppConfig::default());
}

#[test]
fn topology_mode_follows_display_settings() {
    use pin_render::topology::TopologyMode;
    let c = custom();
    match c.topology_mode() {
        TopologyMode::Cabinet {
            playfield,
            backglass,
            score,
        } => assert_eq!((playfield, backglass, score), (1, Some(2), None)),
        other => panic!("{other:?}"),
    }
    match AppConfig::default().topology_mode() {
        TopologyMode::SingleMonitor {
            display,
            backglass,
            score,
        } => {
            assert_eq!(display, 0);
            assert_eq!(backglass.width_fraction, 0.25);
            assert_eq!(score.width_fraction, 0.25);
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn staging_mode_resolution() {
    use pin_core::flipper::FlipperStagingMode as F;
    use pin_core::input::{CabinetAxis, CabinetInput};
    use pin_input::FlipperStage;
    let ded = F::DedicatedButton(CabinetInput::LeftFlipper);
    let par = F::ParallelLinked(CabinetInput::RightUpper);
    let dual = F::DualStageThreshold {
        axis: CabinetAxis::RightFlipper,
        lower_threshold: 0.3,
        upper_threshold: 0.8,
    };
    let lo = FlipperStage::Lower;
    let up = FlipperStage::Upper;
    assert_eq!(StagingMode::Table.resolve(par, up, 0.1, 0.9), par);
    assert_eq!(
        StagingMode::Dedicated.resolve(par, up, 0.1, 0.9),
        F::DedicatedButton(CabinetInput::RightUpper)
    );
    assert_eq!(StagingMode::Dedicated.resolve(ded, lo, 0.1, 0.9), ded);
    assert_eq!(
        StagingMode::Dedicated.resolve(dual, up, 0.1, 0.9),
        F::DedicatedButton(CabinetInput::RightUpper)
    );
    assert_eq!(
        StagingMode::Dedicated.resolve(dual, lo, 0.1, 0.9),
        F::DedicatedButton(CabinetInput::RightFlipper)
    );
    assert_eq!(
        StagingMode::DualStage.resolve(ded, lo, 0.1, 0.9),
        F::DualStageThreshold {
            axis: CabinetAxis::LeftFlipper,
            lower_threshold: 0.1,
            upper_threshold: 0.9
        }
    );
    // Non-flipper buttons keep their staging.
    let action = F::DedicatedButton(CabinetInput::Action);
    for m in StagingMode::ALL {
        assert_eq!(m.resolve(action, lo, 0.1, 0.9), action);
        assert!(!m.label().is_empty());
    }
    for c in CameraPreset::ALL {
        assert!(!c.label().is_empty());
    }
    for c in PipCorner::ALL {
        assert!(!c.label().is_empty());
    }
    assert_eq!(ScoreDisplayKind::Lcd.label(), "LCD");
}

#[test]
fn plunger_and_nudge_conversions_round_trip() {
    let c = custom().calibration;
    let p = c.plunger.to_config();
    assert_eq!(pin_app::ui::PlungerCalibration::from(p), c.plunger);
    let n = c.nudge.to_config();
    assert_eq!(pin_app::ui::NudgeCalibration::from(n), c.nudge);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 512, .. ProptestConfig::default() })]

    /// Random bytes never panic; the result is always sanitized.
    #[test]
    fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let l = AppConfig::from_toml_bytes(&bytes);
        let mut c = l.config.clone();
        let mut w = Vec::new();
        c.sanitize(&mut w);
        prop_assert!(w.is_empty(), "{w:?}");
        prop_assert_eq!(c, l.config);
    }

    /// TOML-shaped noise (known keys with random values) never panics, and
    /// whatever loads saves and reloads to the same settings.
    #[test]
    fn toml_noise_round_trips(
        lines in proptest::collection::vec(
            (prop::sample::select(vec![
                "wireframe", "display_mode", "playfield_display", "backglass_display",
                "camera_preset", "input_profile", "unknown", "audio.decoded_budget_mib",
                "calibration.nudge.gain", "score_pip.width_fraction", "last_table",
            ]),
            prop::sample::select(vec![
                "true", "1", "-1", "0.5", "1e400", "-0.0", "\"multi\"", "\"x\"", "[1]",
                "{a = 1}", "99999999999", "\"\"", "inf",
            ])),
            0..8,
        )
    ) {
        let text: String = lines.iter().map(|(k, v)| format!("{k} = {v}\n")).collect();
        let l = AppConfig::from_toml_str(&text);
        let saved = l.config.to_toml_string().unwrap();
        let again = AppConfig::from_toml_str(&saved);
        prop_assert!(
            again.warnings.iter().all(|w| matches!(w, ConfigWarning::UnknownKey(_))),
            "{:?}\n{saved}", again.warnings
        );
        prop_assert_eq!(again.config, l.config);
    }
}

#[test]
fn partial_tables_keep_per_key_defaults() {
    let l = AppConfig::from_toml_str(
        "wireframe = true\n[score_pip]\ncorner = \"bottom_left\"\n[backglass_pip]\nwidth_fraction = 0.5\n[calibration.nudge]\ngain = 1.5\n[audio]\n",
    );
    assert!(l.warnings.is_empty(), "{:?}", l.warnings);
    let d = AppConfig::default();
    let c = &l.config;
    assert!(c.wireframe);
    assert_eq!(c.score_pip.corner, PipCorner::BottomLeft);
    assert_eq!(c.score_pip.width_fraction, d.score_pip.width_fraction);
    assert_eq!(c.backglass_pip.corner, d.backglass_pip.corner);
    assert_eq!(c.backglass_pip.width_fraction, 0.5);
    assert_eq!(c.calibration.nudge.gain, 1.5);
    assert_eq!(c.calibration.nudge.max, d.calibration.nudge.max);
    assert_eq!(c.calibration.plunger, d.calibration.plunger);
    assert_eq!(c.audio, d.audio);
    // An empty pip table and wrong types inside it.
    let l = AppConfig::from_toml_str("[score_pip]\n");
    assert_eq!(l.config, d);
    let l = AppConfig::from_toml_str("[score_pip]\ncorner = 3\nwidth_fraction = 0.5\n");
    assert!(
        matches!(&l.warnings[..], [ConfigWarning::InvalidValue { key, .. }] if key == "score_pip.corner"),
        "{:?}",
        l.warnings
    );
    assert_eq!(l.config.score_pip.corner, d.score_pip.corner);
    assert_eq!(l.config.score_pip.width_fraction, 0.5);
}

#[test]
fn wrong_types_fall_back_per_field() {
    let text = r#"
wireframe = true
display_mode = "sideways"
camera_preset = "cabinet"
playfield_display = -1
input_profile = 7
calibration = { dual_stage_upper = 0.9, plunger = { deadzone = "big", release_window_ms = 50 }, nudge = 3 }
score_pip = "corner"
last_table = 5
future = "kept"
"#;
    let l = AppConfig::from_toml_str(text);
    let d = AppConfig::default();
    let c = &l.config;
    assert!(c.wireframe);
    assert_eq!(c.camera_preset, CameraPreset::Cabinet);
    assert_eq!(c.display_mode, d.display_mode);
    assert_eq!(c.playfield_display, 0);
    assert_eq!(c.input_profile, "auto");
    assert_eq!(c.calibration.dual_stage_upper, 0.9);
    assert_eq!(c.calibration.plunger.release_window_ms, 50);
    assert_eq!(
        c.calibration.plunger.deadzone,
        d.calibration.plunger.deadzone
    );
    assert_eq!(c.calibration.nudge, d.calibration.nudge);
    assert_eq!(c.score_pip, d.score_pip);
    assert_eq!(c.last_table, None);
    assert_eq!(c.extra.get("future").and_then(|v| v.as_str()), Some("kept"));
    let mut keys: Vec<&str> = l
        .warnings
        .iter()
        .filter_map(|w| match w {
            ConfigWarning::InvalidValue { key, .. } => Some(key.as_str()),
            _ => None,
        })
        .collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "calibration.nudge",
            "calibration.plunger.deadzone",
            "display_mode",
            "input_profile",
            "last_table",
            "playfield_display",
            "score_pip"
        ]
    );
    for w in &l.warnings {
        assert!(w.to_string().chars().count() < 800, "{w}");
    }
}

#[test]
fn corrupt_message_is_truncated() {
    let long = format!("key = \"{}", "x".repeat(5000));
    let l = AppConfig::from_toml_str(&long);
    match &l.warnings[..] {
        [ConfigWarning::Corrupt(m)] => {
            assert!(
                m.chars().count() <= pin_app::ui::config::MAX_WARNING_CHARS + 1,
                "{}",
                m.len()
            )
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn lossy_load_backs_up_the_original() {
    let dir = temp_dir("backup");
    let path = dir.join(CONFIG_FILE);
    let bak = dir.join(format!("{CONFIG_FILE}.bak"));
    // Clean file: no backup.
    custom().save(&path).unwrap();
    AppConfig::load(&path);
    assert!(!bak.exists());
    // A dropped value: the original is copied before anything is saved.
    let original = "wireframe = true\ncamera_preset = \"sideways\"\n";
    std::fs::write(&path, original).unwrap();
    let l = AppConfig::load(&path);
    assert!(l.config.wireframe);
    assert_eq!(std::fs::read_to_string(&bak).unwrap(), original);
    l.config.save(&path).unwrap();
    assert_eq!(std::fs::read_to_string(&bak).unwrap(), original);
    // Reloading the saved file is clean and keeps the backup.
    assert!(AppConfig::load(&path).warnings.is_empty());
    assert_eq!(std::fs::read_to_string(&bak).unwrap(), original);
    // Corrupt syntax is backed up too (once the old backup is gone; an
    // existing backup is never overwritten).
    std::fs::write(&path, "[[[").unwrap();
    AppConfig::load(&path);
    assert_eq!(std::fs::read_to_string(&bak).unwrap(), original);
    std::fs::remove_file(&bak).unwrap();
    AppConfig::load(&path);
    assert_eq!(std::fs::read_to_string(&bak).unwrap(), "[[[");
}

/// 4.4 review N13: an array of tables where a table is expected is a bad
/// value (warned, dropped, the rest kept), not silently accepted.
#[test]
fn arrays_of_tables_for_known_tables_are_rejected() {
    for (text, key) in [
        ("[[calibration]]\nx = 1\n", "calibration"),
        ("[[score_pip]]\ncorner = \"top_left\"\n", "score_pip"),
        (
            "[calibration]\n[[calibration.plunger]]\ndeadzone = 0.1\n",
            "calibration.plunger",
        ),
        ("backglass_pip = [1, 2]\n", "backglass_pip"),
    ] {
        let l = AppConfig::from_toml_str(&format!("wireframe = true\n{text}"));
        assert!(l.config.wireframe, "{text}");
        assert!(
            l.warnings
                .iter()
                .any(|w| matches!(w, ConfigWarning::InvalidValue { key: k, .. } if k == key)),
            "{text}: {:?}",
            l.warnings
        );
    }
    // And the file is backed up before the value is lost.
    let dir = temp_dir("aot");
    let path = dir.join(CONFIG_FILE);
    std::fs::write(&path, "[[calibration]]\nx = 1\n").unwrap();
    AppConfig::load(&path);
    assert!(dir.join(format!("{CONFIG_FILE}.bak")).exists());
}

/// 4.4 review N14: an existing backup is kept, and an oversized file is not
/// copied.
#[test]
fn backup_is_kept_and_bounded() {
    let dir = temp_dir("bakkeep");
    let path = dir.join(CONFIG_FILE);
    let bak = dir.join(format!("{CONFIG_FILE}.bak"));
    std::fs::write(&path, "camera_preset = \"first\"\n").unwrap();
    AppConfig::load(&path);
    std::fs::write(&path, "camera_preset = \"second\"\n").unwrap();
    AppConfig::load(&path);
    assert_eq!(
        std::fs::read_to_string(&bak).unwrap(),
        "camera_preset = \"first\"\n"
    );
    let dir = temp_dir("baklarge");
    let path = dir.join(CONFIG_FILE);
    std::fs::write(&path, vec![b'#'; MAX_CONFIG_BYTES as usize * 3]).unwrap();
    let l = AppConfig::load(&path);
    assert!(matches!(l.warnings[..], [ConfigWarning::TooLarge(_)]));
    assert!(l.warnings[0].to_string().contains("not backed up"));
    assert!(!dir.join(format!("{CONFIG_FILE}.bak")).exists());
}
