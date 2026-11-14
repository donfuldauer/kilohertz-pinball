//! Subphase 4.4 spec tests (Independent Tester): gate 5, the `config.toml`
//! parser survives bounded random input without panicking, and whatever it
//! returns is a usable config.
#![allow(clippy::field_reassign_with_default, clippy::unnecessary_cast)]

#[path = "fixtures/spec_4_4/support.rs"]
mod support;

use pin_app::ui::{AppConfig, ConfigLoad, SettingRow};
use proptest::prelude::*;
use support::*;

/// What every load result must satisfy, whatever the input was.
fn check(r: &ConfigLoad) -> Result<(), TestCaseError> {
    let c = &r.config;
    for row in SettingRow::DISPLAY.iter().chain(SettingRow::INPUT.iter()) {
        if let (Some(range), Some(v)) = (row.range(), row.number(c)) {
            prop_assert!(
                v.is_finite() && v >= range.min && v <= range.max,
                "{:?} = {} outside its range",
                row,
                v
            );
        }
        let _ = row.value_text(c);
    }
    // The result can always be written back and read again unchanged.
    let text = c.to_toml_string();
    prop_assert!(
        text.is_ok(),
        "loaded config cannot be serialised: {:?}",
        text.err()
    );
    let again = AppConfig::from_toml_str(&text.unwrap());
    // Compared through `Debug`: preserved unknown keys may hold NaN, which is
    // never `==` to itself.
    prop_assert_eq!(format!("{:?}", again.config), format!("{c:?}"));
    // Sanitising a loaded config is a no-op.
    let mut s = c.clone();
    let mut w = Vec::new();
    s.sanitize(&mut w);
    prop_assert_eq!(format!("{s:?}"), format!("{c:?}"));
    for w in &r.warnings {
        let shown = format!("{w:?}");
        prop_assert!(!shown.is_empty());
    }
    Ok(())
}

const KEYS: &[&str] = &[
    "display_mode",
    "playfield_display",
    "backglass_display",
    "score_display_monitor",
    "backglass_pip",
    "score_pip",
    "corner",
    "width_fraction",
    "camera_preset",
    "score_display",
    "input_profile",
    "staging_mode",
    "calibration",
    "plunger",
    "nudge",
    "deadzone",
    "rest_tracking_per_s",
    "release_min_speed",
    "release_window_ms",
    "high_pass_hz",
    "gain",
    "curve_exponent",
    "max",
    "dual_stage_lower",
    "dual_stage_upper",
    "wireframe",
    "tables_dir",
    "last_table",
    "extra",
    "mystery",
];

const VALUES: &[&str] = &[
    "true",
    "false",
    "0",
    "1",
    "-1",
    "7",
    "4294967295",
    "4294967296",
    "-9223372036854775808",
    "9223372036854775807",
    "0.0",
    "-0.0",
    "0.5",
    "1e30",
    "-1e30",
    "1e-40",
    "nan",
    "-nan",
    "inf",
    "-inf",
    "\"\"",
    "\"single\"",
    "\"multi\"",
    "\"dmd\"",
    "\"lcd\"",
    "\"top_down\"",
    "\"cabinet\"",
    "\"desktop\"",
    "\"table\"",
    "\"dedicated\"",
    "\"dual_stage\"",
    "\"top_left\"",
    "\"bottom_right\"",
    "\"auto\"",
    "\"x_arcade\"",
    "\"/tmp/a b\"",
    "\"\\u0000\"",
    "[]",
    "[1, 2]",
    "[[1], [\"a\"]]",
    "{}",
    "{ corner = \"top_left\" }",
    "{ width_fraction = nan }",
    "{ deadzone = -1 }",
    "{ plunger = 1, nudge = { gain = inf } }",
    "1979-05-27T07:32:00Z",
    "'''multi\nline'''",
];

fn line() -> impl Strategy<Value = String> {
    let key = prop_oneof![
        4 => proptest::sample::select(KEYS).prop_map(str::to_owned),
        1 => "[a-z_]{1,8}",
        1 => (proptest::sample::select(KEYS), proptest::sample::select(KEYS))
            .prop_map(|(a, b)| format!("{a}.{b}")),
    ];
    let value = prop_oneof![
        6 => proptest::sample::select(VALUES).prop_map(str::to_owned),
        1 => any::<f64>().prop_map(|v| format!("{v:?}")),
        1 => any::<i64>().prop_map(|v| v.to_string()),
        1 => "[ -~]{0,12}",
    ];
    prop_oneof![
        8 => (key.clone(), value).prop_map(|(k, v)| format!("{k} = {v}")),
        2 => key.clone().prop_map(|k| format!("[{k}]")),
        1 => key.prop_map(|k| format!("[[{k}]]")),
        1 => Just("# comment".to_owned()),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 2000, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let r = AppConfig::from_toml_bytes(&bytes);
        check(&r)?;
    }

    #[test]
    fn random_text_never_panics(text in "\\PC{0,400}") {
        let r = AppConfig::from_toml_str(&text);
        check(&r)?;
        prop_assert_eq!(AppConfig::from_toml_bytes(text.as_bytes()).config, r.config);
    }

    /// TOML-shaped input built from the real keys with hostile values.
    #[test]
    fn toml_shaped_noise_never_panics(lines in proptest::collection::vec(line(), 0..24)) {
        let text = lines.join("\n");
        let r = AppConfig::from_toml_str(&text);
        check(&r)?;
        // Parsing is a pure function of the text.
        let again = AppConfig::from_toml_str(&text);
        prop_assert_eq!(format!("{:?}", again.config), format!("{:?}", r.config));
        prop_assert_eq!(again.warnings.len(), r.warnings.len());
    }

    /// A valid file with a random splice (truncation, bit damage, insertion).
    #[test]
    fn damaged_valid_files_never_panic(
        cut in any::<prop::sample::Index>(),
        len in 0usize..40,
        insert in proptest::collection::vec(any::<u8>(), 0..24),
        truncate in any::<bool>(),
    ) {
        let mut c = AppConfig::default();
        c.last_table = Some("/tables/apex.pinbundle".into());
        c.tables_dir = Some("/tables".into());
        c.backglass_display = Some(1);
        c.score_display_monitor = Some(2);
        let mut bytes = c.to_toml_string().unwrap().into_bytes();
        let at = cut.index(bytes.len() + 1);
        if truncate {
            bytes.truncate(at);
            bytes.extend(&insert);
        } else {
            let end = (at + len).min(bytes.len());
            bytes.splice(at..end, insert);
        }
        let r = AppConfig::from_toml_bytes(&bytes);
        check(&r)?;
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, failure_persistence: None, ..ProptestConfig::default() })]

    /// The file path: random bytes on disk load without panicking and are
    /// not modified by the load.
    #[test]
    fn random_files_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let tmp = TempDir::new("cfg_fuzz");
        let path = tmp.join("config.toml");
        std::fs::write(&path, &bytes).unwrap();
        let r = AppConfig::load(&path);
        check(&r)?;
        prop_assert_eq!(format!("{:?}", r.config), format!("{:?}", AppConfig::from_toml_bytes(&bytes).config));
        prop_assert_eq!(std::fs::read(&path).unwrap(), bytes);
        prop_assert_eq!(dir_names(tmp.path()), vec!["config.toml".to_owned()]);
    }
}

#[test]
fn pathological_inputs_do_not_panic_or_hang() {
    let deep_arrays = format!("x = {}{}", "[".repeat(5000), "]".repeat(5000));
    let deep_tables = format!("x = {}1{}", "{a=".repeat(2000), "}".repeat(2000));
    let dotted = format!("{} = 1", vec!["a"; 5000].join("."));
    let long_string = format!("last_table = \"{}\"", "x".repeat(900_000));
    let many_keys: String = (0..20_000).map(|i| format!("k{i} = {i}\n")).collect();
    for text in [deep_arrays, deep_tables, dotted, long_string, many_keys] {
        let r = AppConfig::from_toml_str(&text);
        let _ = r.config.to_toml_string();
    }
}
