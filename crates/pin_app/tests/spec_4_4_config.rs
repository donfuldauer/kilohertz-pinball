//! Subphase 4.4 spec tests (Independent Tester): `config.toml` persistence.

#[path = "fixtures/spec_4_4/support.rs"]
mod support;

use pin_app::ui::{
    AppConfig, CONFIG_FILE, CameraPreset, ConfigWarning, DisplayMode, MAX_CONFIG_BYTES, PipCorner,
    ScoreDisplayKind, SettingRow, StagingMode,
};
use proptest::prelude::*;
use support::*;

/// A config that differs from the defaults in every documented field.
fn custom() -> AppConfig {
    let d = AppConfig::default();
    let mut c = d.clone();
    c.display_mode = if d.display_mode == DisplayMode::Single {
        DisplayMode::Multi
    } else {
        DisplayMode::Single
    };
    c.playfield_display = 1;
    c.backglass_display = Some(2);
    c.score_display_monitor = Some(0);
    c.backglass_pip.corner = if d.backglass_pip.corner == PipCorner::BottomLeft {
        PipCorner::TopLeft
    } else {
        PipCorner::BottomLeft
    };
    c.score_pip.corner = if d.score_pip.corner == PipCorner::BottomRight {
        PipCorner::TopRight
    } else {
        PipCorner::BottomRight
    };
    c.backglass_pip.width_fraction = mid(SettingRow::BackglassPipWidth, 0.25);
    c.score_pip.width_fraction = mid(SettingRow::ScorePipWidth, 0.75);
    c.camera_preset = if d.camera_preset == CameraPreset::TopDown {
        CameraPreset::Cabinet
    } else {
        CameraPreset::TopDown
    };
    c.score_display = if d.score_display == ScoreDisplayKind::Lcd {
        ScoreDisplayKind::Dmd
    } else {
        ScoreDisplayKind::Lcd
    };
    c.input_profile = pin_input::profiles::builtin_profiles()[0].id.to_string();
    c.staging_mode = if d.staging_mode == StagingMode::DualStage {
        StagingMode::Dedicated
    } else {
        StagingMode::DualStage
    };
    c.calibration.plunger.deadzone = mid(SettingRow::PlungerDeadzone, 0.3);
    c.calibration.plunger.rest_tracking_per_s = mid(SettingRow::PlungerRestTracking, 0.3);
    c.calibration.plunger.release_min_speed = mid(SettingRow::PlungerReleaseMinSpeed, 0.3);
    c.calibration.plunger.release_window_ms = mid(SettingRow::PlungerReleaseWindow, 0.3) as u32;
    c.calibration.nudge.high_pass_hz = mid(SettingRow::NudgeHighPass, 0.3);
    c.calibration.nudge.gain = mid(SettingRow::NudgeGain, 0.3);
    c.calibration.nudge.curve_exponent = mid(SettingRow::NudgeCurve, 0.3);
    c.calibration.nudge.deadzone = mid(SettingRow::NudgeDeadzone, 0.3);
    c.calibration.nudge.max = mid(SettingRow::NudgeMax, 0.3);
    c.calibration.dual_stage_lower = mid(SettingRow::DualStageLower, 0.2);
    c.calibration.dual_stage_upper = mid(SettingRow::DualStageUpper, 0.8);
    c.wireframe = !d.wireframe;
    c.tables_dir = Some("/home/player/pinball tables".into());
    c.last_table = Some("/home/player/pinball tables/Apex Circuit.pinbundle".into());
    c
}

/// A value `t` of the way through the row's range, on its step grid.
fn mid(row: SettingRow, t: f32) -> f32 {
    let r = row.range().unwrap_or_else(|| panic!("{row:?} has no range"));
    let (min, max, step) = (r.min as f32, r.max as f32, r.step as f32);
    let steps = ((max - min) * t / step).round();
    (min + steps * step).clamp(min, max)
}

fn check_sane(c: &AppConfig) {
    for row in SettingRow::DISPLAY.iter().chain(SettingRow::INPUT.iter()) {
        if let (Some(r), Some(v)) = (row.range(), row.number(c)) {
            assert!(
                v.is_finite() && v >= r.min && v <= r.max,
                "{row:?} = {v} outside {}..={}",
                r.min,
                r.max
            );
        }
        assert!(!row.label().is_empty());
        let _ = row.value_text(c);
    }
    assert!(!c.input_profile.is_empty());
}

#[test]
fn constants() {
    assert_eq!(CONFIG_FILE, "config.toml");
    assert_eq!(MAX_CONFIG_BYTES as u64, 1024 * 1024);
}

#[test]
fn defaults_are_sane() {
    let d = AppConfig::default();
    check_sane(&d);
    assert_eq!(d.input_profile, "auto");
    assert_eq!(d.last_table, None);
    assert!(d.extra.is_empty());
    assert_eq!(d, AppConfig::default());
}

#[test]
fn missing_file_gives_defaults_without_warning() {
    let tmp = TempDir::new("cfg_missing");
    let r = AppConfig::load(&tmp.join(CONFIG_FILE));
    assert_eq!(r.config, AppConfig::default());
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    // Loading must not create the file.
    assert!(dir_names(tmp.path()).is_empty());
    // A missing parent directory is the same case.
    let r = AppConfig::load(&tmp.join("no/such/dir/config.toml"));
    assert_eq!(r.config, AppConfig::default());
}

#[test]
fn empty_file_is_defaults() {
    let r = AppConfig::from_toml_str("");
    assert_eq!(r.config, AppConfig::default());
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
}

#[test]
fn save_then_load_round_trips_every_field() {
    let tmp = TempDir::new("cfg_rt");
    let path = tmp.join(CONFIG_FILE);
    let c = custom();
    assert_ne!(c, AppConfig::default());
    check_sane(&c);
    c.save(&path).unwrap();
    let r = AppConfig::load(&path);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert_eq!(r.config, c);

    // Defaults round-trip too, and the string form agrees with the file.
    let d = AppConfig::default();
    d.save(&path).unwrap();
    let r = AppConfig::load(&path);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert_eq!(r.config, d);
    let text = c.to_toml_string().unwrap();
    assert_eq!(AppConfig::from_toml_str(&text).config, c);
    assert_eq!(AppConfig::from_toml_bytes(text.as_bytes()).config, c);
}

#[test]
fn file_uses_the_documented_keys_and_enum_spellings() {
    let mut c = AppConfig::default();
    c.display_mode = DisplayMode::Multi;
    c.camera_preset = CameraPreset::TopDown;
    c.score_display = ScoreDisplayKind::Lcd;
    c.staging_mode = StagingMode::DualStage;
    c.backglass_pip.corner = PipCorner::TopRight;
    c.wireframe = true;
    c.last_table = Some("/t/x.pinbundle".into());
    let text = c.to_toml_string().unwrap();
    let doc: toml::Table = text.parse().expect("saved config is valid TOML");
    let s = |k: &str| doc.get(k).and_then(|v| v.as_str()).map(str::to_owned);
    assert_eq!(s("display_mode").as_deref(), Some("multi"), "{text}");
    assert_eq!(s("camera_preset").as_deref(), Some("top_down"));
    assert_eq!(s("score_display").as_deref(), Some("lcd"));
    assert_eq!(s("staging_mode").as_deref(), Some("dual_stage"));
    assert_eq!(s("input_profile").as_deref(), Some("auto"));
    assert_eq!(s("last_table").as_deref(), Some("/t/x.pinbundle"));
    assert_eq!(doc.get("wireframe").and_then(|v| v.as_bool()), Some(true));
    assert_eq!(
        doc["backglass_pip"]["corner"].as_str(),
        Some("top_right"),
        "{text}"
    );
    assert!(doc["backglass_pip"]["width_fraction"].is_float());
    assert!(doc["calibration"]["plunger"]["deadzone"].is_float());
    assert!(doc["calibration"]["plunger"]["release_window_ms"].is_integer());
    assert!(doc["calibration"]["nudge"]["gain"].is_float());

    // A hand-written file with the documented spellings loads.
    let r = AppConfig::from_toml_str(
        "display_mode = \"single\"\ncamera_preset = \"cabinet\"\nscore_display = \"dmd\"\n\
         staging_mode = \"dedicated\"\nwireframe = true\n\
         [score_pip]\ncorner = \"bottom_left\"\n",
    );
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert_eq!(r.config.display_mode, DisplayMode::Single);
    assert_eq!(r.config.camera_preset, CameraPreset::Cabinet);
    assert_eq!(r.config.score_display, ScoreDisplayKind::Dmd);
    assert_eq!(r.config.staging_mode, StagingMode::Dedicated);
    assert_eq!(r.config.score_pip.corner, PipCorner::BottomLeft);
    assert!(r.config.wireframe);
}

#[test]
fn partial_file_keeps_defaults_for_the_rest() {
    let d = AppConfig::default();
    let r = AppConfig::from_toml_str("wireframe = true\n[calibration.nudge]\ngain = 1.0\n");
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    let mut expect = d.clone();
    expect.wireframe = true;
    expect.calibration.nudge.gain = r.config.calibration.nudge.gain;
    assert_eq!(r.config, expect);
    check_sane(&r.config);
}

#[test]
fn corrupt_file_falls_back_to_defaults_with_a_warning() {
    let tmp = TempDir::new("cfg_corrupt");
    let path = tmp.join(CONFIG_FILE);
    let cases: [&[u8]; 6] = [
        b"this is not = = toml [[[",
        b"wireframe = true\n[unterminated\n",
        b"\xff\xfe\x00\x01 binary garbage \x80",
        b"wireframe = true\nwireframe = false\n",
        b"\0\0\0\0",
        b"= 3",
    ];
    for bytes in cases {
        std::fs::write(&path, bytes).unwrap();
        let r = AppConfig::load(&path);
        assert_eq!(r.config, AppConfig::default(), "{bytes:?}");
        assert!(
            r.warnings
                .iter()
                .any(|w| matches!(w, ConfigWarning::Corrupt(m) if !m.is_empty())),
            "{bytes:?}: {:?}",
            r.warnings
        );
        let r2 = AppConfig::from_toml_bytes(bytes);
        assert_eq!(r2.config, AppConfig::default());
        assert!(!r2.warnings.is_empty());
        // The corrupt file is left alone by load.
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
}

#[test]
fn unreadable_and_oversized_files_fall_back_with_a_warning() {
    let tmp = TempDir::new("cfg_unreadable");
    // A directory where the file should be.
    let dir = tmp.join("config.toml");
    std::fs::create_dir_all(&dir).unwrap();
    let r = AppConfig::load(&dir);
    assert_eq!(r.config, AppConfig::default());
    assert!(
        r.warnings
            .iter()
            .any(|w| matches!(w, ConfigWarning::Unreadable(_))),
        "{:?}",
        r.warnings
    );

    // Larger than MAX_CONFIG_BYTES (valid TOML otherwise).
    let big = tmp.join("big.toml");
    let mut text = String::from("wireframe = true\n");
    while text.len() <= MAX_CONFIG_BYTES as usize {
        text.push_str("# padding padding padding padding padding padding padding padding\n");
    }
    std::fs::write(&big, &text).unwrap();
    let r = AppConfig::load(&big);
    assert_eq!(r.config, AppConfig::default());
    assert!(
        r.warnings
            .iter()
            .any(|w| matches!(w, ConfigWarning::TooLarge(n) if *n == text.len() as u64)),
        "{:?}",
        r.warnings
    );
    assert_eq!(
        AppConfig::from_toml_str(&text).config,
        AppConfig::default(),
        "the size cap also holds for in-memory input"
    );
}

#[test]
fn wrong_typed_and_unknown_enum_values_are_reported() {
    let d = AppConfig::default();
    for text in [
        "display_mode = \"sideways\"\n",
        "display_mode = 7\n",
        "camera_preset = \"orbital\"\n",
        "wireframe = \"yes\"\n",
        "playfield_display = -1\n",
        "playfield_display = \"left\"\n",
        "backglass_pip = 3\n",
        "calibration = \"none\"\n",
        "last_table = 12\n",
        "[calibration.plunger]\nrelease_window_ms = -5\n",
        "[calibration.plunger]\ndeadzone = \"big\"\n",
        "[score_pip]\ncorner = \"middle\"\n",
    ] {
        let r = AppConfig::from_toml_str(text);
        assert!(!r.warnings.is_empty(), "no warning for {text:?}");
        check_sane(&r.config);
        assert_eq!(r.config.display_mode, d.display_mode, "{text:?}");
        assert_eq!(r.config.camera_preset, d.camera_preset, "{text:?}");
        assert_eq!(r.config.wireframe, d.wireframe, "{text:?}");
        assert_eq!(r.config.playfield_display, d.playfield_display, "{text:?}");
        assert_eq!(r.config.backglass_pip, d.backglass_pip, "{text:?}");
        assert_eq!(r.config.score_pip, d.score_pip, "{text:?}");
        assert_eq!(r.config.calibration, d.calibration, "{text:?}");
        assert_eq!(r.config.last_table, d.last_table, "{text:?}");
    }
}

#[test]
fn out_of_range_numbers_are_sanitised_and_reported() {
    for text in [
        "[backglass_pip]\nwidth_fraction = 99.0\n",
        "[backglass_pip]\nwidth_fraction = -3.0\n",
        "[score_pip]\nwidth_fraction = nan\n",
        "[score_pip]\nwidth_fraction = inf\n",
        "[calibration.nudge]\ngain = -inf\n",
        "[calibration.nudge]\ngain = 1e30\n",
        "[calibration.plunger]\ndeadzone = 55.5\n",
        "[calibration.plunger]\nrelease_window_ms = 4000000000\n",
        "[calibration]\ndual_stage_lower = 9.0\ndual_stage_upper = -9.0\n",
    ] {
        let r = AppConfig::from_toml_str(text);
        check_sane(&r.config);
        assert!(!r.warnings.is_empty(), "no warning for {text:?}");
    }
}

#[test]
fn sanitize_repairs_a_config_built_in_code() {
    let mut c = AppConfig::default();
    c.backglass_pip.width_fraction = f32::NAN;
    c.score_pip.width_fraction = 1e9;
    c.calibration.nudge.gain = f32::NEG_INFINITY;
    c.calibration.plunger.deadzone = -4.0;
    c.calibration.plunger.release_window_ms = u32::MAX;
    c.input_profile = String::new();
    let mut warnings = Vec::new();
    c.sanitize(&mut warnings);
    check_sane(&c);
    assert!(!warnings.is_empty());
    // Sanitising a sane config is silent and changes nothing.
    let mut d = custom();
    let before = d.clone();
    let mut warnings = Vec::new();
    d.sanitize(&mut warnings);
    assert_eq!(d, before);
    assert!(warnings.is_empty(), "{warnings:?}");
}

#[test]
fn unknown_keys_are_preserved_and_reported() {
    let tmp = TempDir::new("cfg_unknown");
    let path = tmp.join(CONFIG_FILE);
    let text = "wireframe = true\nfuture_setting = 42\n\n[future_section]\nname = \"x\"\nlist = [1, 2, 3]\n";
    std::fs::write(&path, text).unwrap();
    let r = AppConfig::load(&path);
    // Known keys still apply.
    assert!(r.config.wireframe);
    // The published choice: kept in `extra`, with an UnknownKey warning each.
    assert_eq!(
        r.config.extra.get("future_setting").and_then(|v| v.as_integer()),
        Some(42)
    );
    assert_eq!(
        r.config.extra["future_section"]["name"].as_str(),
        Some("x")
    );
    for key in ["future_setting", "future_section"] {
        assert!(
            r.warnings
                .iter()
                .any(|w| matches!(w, ConfigWarning::UnknownKey(k) if k.contains(key))),
            "no UnknownKey for {key}: {:?}",
            r.warnings
        );
    }
    assert!(
        r.warnings
            .iter()
            .all(|w| matches!(w, ConfigWarning::UnknownKey(_))),
        "{:?}",
        r.warnings
    );

    // …and they survive a load → change → save cycle.
    let mut c = r.config.clone();
    c.camera_preset = CameraPreset::TopDown;
    c.save(&path).unwrap();
    let doc: toml::Table = std::fs::read_to_string(&path).unwrap().parse().unwrap();
    assert_eq!(doc["future_setting"].as_integer(), Some(42));
    assert_eq!(doc["future_section"]["name"].as_str(), Some("x"));
    assert_eq!(
        doc["future_section"]["list"].as_array().map(Vec::len),
        Some(3)
    );
    assert_eq!(doc["camera_preset"].as_str(), Some("top_down"));
    let again = AppConfig::load(&path);
    assert_eq!(again.config, c);
}

#[test]
fn save_is_atomic_and_leaves_only_the_config_file() {
    let tmp = TempDir::new("cfg_atomic");
    let path = tmp.join(CONFIG_FILE);
    // First save creates the file.
    AppConfig::default().save(&path).unwrap();
    assert_eq!(dir_names(tmp.path()), vec![CONFIG_FILE.to_owned()]);
    // Overwriting replaces it completely, without leftovers, many times.
    let c = custom();
    for i in 0..20 {
        let mut c = c.clone();
        c.playfield_display = i % 2;
        c.save(&path).unwrap();
        assert_eq!(
            dir_names(tmp.path()),
            vec![CONFIG_FILE.to_owned()],
            "temp file left behind"
        );
        let r = AppConfig::load(&path);
        assert!(r.warnings.is_empty());
        assert_eq!(r.config, c);
    }
    // Other files in the directory are untouched.
    std::fs::write(tmp.join("config.toml.bak"), b"keep me").unwrap();
    std::fs::write(tmp.join("other.toml"), b"keep me too").unwrap();
    c.save(&path).unwrap();
    assert_eq!(std::fs::read(tmp.join("config.toml.bak")).unwrap(), b"keep me");
    assert_eq!(std::fs::read(tmp.join("other.toml")).unwrap(), b"keep me too");
    assert_eq!(dir_names(tmp.path()).len(), 3);
}

#[test]
fn failed_save_reports_an_error_and_keeps_the_old_file() {
    let tmp = TempDir::new("cfg_fail");
    // The destination is a non-empty directory: the rename cannot succeed.
    let blocked = tmp.join("blocked");
    std::fs::create_dir_all(blocked.join("child")).unwrap();
    assert!(custom().save(&blocked).is_err());
    assert!(blocked.join("child").is_dir());
    assert_eq!(
        dir_names(tmp.path()),
        vec!["blocked".to_owned()],
        "a failed save left a temp file behind"
    );

    // The parent "directory" is a file.
    let file = tmp.join("plain_file");
    std::fs::write(&file, b"x").unwrap();
    assert!(custom().save(&file.join(CONFIG_FILE)).is_err());
    assert_eq!(std::fs::read(&file).unwrap(), b"x");
}

#[cfg(unix)]
#[test]
fn failed_save_in_a_read_only_directory_keeps_the_previous_config() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = TempDir::new("cfg_ro");
    let dir = tmp.join("ro");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(CONFIG_FILE);
    let old = AppConfig::default();
    old.save(&path).unwrap();
    let old_bytes = std::fs::read(&path).unwrap();
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();
    // Root ignores directory permissions; the check only means something
    // when the directory really is read-only for this process.
    let enforced = std::fs::write(dir.join("probe"), b"x").is_err();
    let result = custom().save(&path);
    std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
    if enforced {
        assert!(result.is_err());
        assert_eq!(std::fs::read(&path).unwrap(), old_bytes);
        assert_eq!(dir_names(&dir), vec![CONFIG_FILE.to_owned()]);
    } else {
        assert!(result.is_ok());
    }
}

#[cfg(unix)]
#[test]
fn save_replaces_the_file_by_rename() {
    use std::os::unix::fs::MetadataExt;
    let tmp = TempDir::new("cfg_rename");
    let path = tmp.join(CONFIG_FILE);
    AppConfig::default().save(&path).unwrap();
    // A reader that opened the old file keeps seeing the old, complete
    // contents: the save writes a new inode and renames it over the path.
    let old_bytes = std::fs::read(&path).unwrap();
    let old_ino = std::fs::metadata(&path).unwrap().ino();
    let mut held = std::fs::File::open(&path).unwrap();
    custom().save(&path).unwrap();
    let new_ino = std::fs::metadata(&path).unwrap().ino();
    assert_ne!(old_ino, new_ino, "save rewrote the file in place");
    let mut seen = Vec::new();
    std::io::Read::read_to_end(&mut held, &mut seen).unwrap();
    assert_eq!(seen, old_bytes);
    assert_eq!(AppConfig::load(&path).config, custom());
}

#[test]
fn topology_mode_follows_the_display_mode() {
    let mut c = AppConfig::default();
    c.display_mode = DisplayMode::Single;
    let single = c.topology_mode();
    c.display_mode = DisplayMode::Multi;
    let multi = c.topology_mode();
    assert_ne!(format!("{single:?}"), format!("{multi:?}"));
}

fn arb_config() -> impl Strategy<Value = AppConfig> {
    let unit = || 0.0f32..=1.0;
    (
        (any::<bool>(), 0u32..4, proptest::option::of(0u32..4), proptest::option::of(0u32..4)),
        (0usize..4, 0usize..4, unit(), unit(), 0usize..3, any::<bool>()),
        (0usize..3, proptest::collection::vec(unit(), 11), any::<bool>()),
        (
            proptest::option::of("[ -~]{1,24}"),
            proptest::option::of("[a-zA-Z0-9 /._\u{e9}\u{4e2d}-]{1,24}"),
            0usize..8,
        ),
    )
        .prop_map(|((multi, pf, bg, sd), (c1, c2, w1, w2, cam, lcd), (stg, cal, wire), (dir, last, prof))| {
            let corners = [
                PipCorner::TopLeft,
                PipCorner::TopRight,
                PipCorner::BottomLeft,
                PipCorner::BottomRight,
            ];
            let at = |row: SettingRow, t: f32| {
                let r = row.range().unwrap();
                (r.min as f32 + (r.max as f32 - r.min as f32) * t).clamp(r.min as f32, r.max as f32)
            };
            let mut c = AppConfig::default();
            c.display_mode = if multi { DisplayMode::Multi } else { DisplayMode::Single };
            c.playfield_display = pf;
            c.backglass_display = bg;
            c.score_display_monitor = sd;
            c.backglass_pip.corner = corners[c1];
            c.score_pip.corner = corners[c2];
            c.backglass_pip.width_fraction = at(SettingRow::BackglassPipWidth, w1);
            c.score_pip.width_fraction = at(SettingRow::ScorePipWidth, w2);
            c.camera_preset =
                [CameraPreset::Desktop, CameraPreset::Cabinet, CameraPreset::TopDown][cam];
            c.score_display = if lcd { ScoreDisplayKind::Lcd } else { ScoreDisplayKind::Dmd };
            c.staging_mode =
                [StagingMode::Table, StagingMode::Dedicated, StagingMode::DualStage][stg];
            c.calibration.plunger.deadzone = at(SettingRow::PlungerDeadzone, cal[0]);
            c.calibration.plunger.rest_tracking_per_s = at(SettingRow::PlungerRestTracking, cal[1]);
            c.calibration.plunger.release_min_speed = at(SettingRow::PlungerReleaseMinSpeed, cal[2]);
            c.calibration.plunger.release_window_ms =
                at(SettingRow::PlungerReleaseWindow, cal[3]).round() as u32;
            c.calibration.nudge.high_pass_hz = at(SettingRow::NudgeHighPass, cal[4]);
            c.calibration.nudge.gain = at(SettingRow::NudgeGain, cal[5]);
            c.calibration.nudge.curve_exponent = at(SettingRow::NudgeCurve, cal[6]);
            c.calibration.nudge.deadzone = at(SettingRow::NudgeDeadzone, cal[7]);
            c.calibration.nudge.max = at(SettingRow::NudgeMax, cal[8]);
            // Keep the dual-stage thresholds ordered (lower ≤ upper).
            let (lo, hi) = if cal[9] <= cal[10] { (cal[9], cal[10]) } else { (cal[10], cal[9]) };
            c.calibration.dual_stage_lower = at(SettingRow::DualStageLower, lo * 0.4);
            c.calibration.dual_stage_upper = at(SettingRow::DualStageUpper, 0.6 + hi * 0.4);
            c.wireframe = wire;
            c.tables_dir = dir.map(Into::into);
            c.last_table = last.map(Into::into);
            let profiles = pin_input::profiles::builtin_profiles();
            c.input_profile = if prof == 0 {
                "auto".to_owned()
            } else {
                profiles[(prof - 1) % profiles.len()].id.to_string()
            };
            c
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 200, failure_persistence: None, ..ProptestConfig::default() })]

    /// Any config the menu can hold survives text and file round trips.
    #[test]
    fn any_valid_config_round_trips(c in arb_config()) {
        // Only configs that are already sane are in scope.
        let mut sane = c.clone();
        let mut w = Vec::new();
        sane.sanitize(&mut w);
        prop_assume!(sane == c && w.is_empty());

        let text = c.to_toml_string().unwrap();
        let r = AppConfig::from_toml_str(&text);
        prop_assert!(r.warnings.is_empty(), "{:?}\n{}", r.warnings, text);
        prop_assert_eq!(&r.config, &c);
        // Serialising is deterministic and stable across a round trip.
        prop_assert_eq!(r.config.to_toml_string().unwrap(), text);
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn any_valid_config_round_trips_through_a_file(c in arb_config()) {
        let mut sane = c.clone();
        let mut w = Vec::new();
        sane.sanitize(&mut w);
        prop_assume!(sane == c && w.is_empty());
        let tmp = TempDir::new("cfg_prop");
        let path = tmp.join(CONFIG_FILE);
        c.save(&path).unwrap();
        let r = AppConfig::load(&path);
        prop_assert!(r.warnings.is_empty());
        prop_assert_eq!(r.config, c);
        prop_assert_eq!(dir_names(tmp.path()), vec![CONFIG_FILE.to_owned()]);
    }
}
