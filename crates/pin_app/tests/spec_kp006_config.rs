//! KP-006 (#86) spec tests (Independent Tester): the `[telemetry]` and `[dof]`
//! keys of `config.toml` (SPEC §4 3.4 UDP telemetry, SPEC §4 3.5 DOF bridge).

use pin_app::ui::{AppConfig, ConfigWarning};
use proptest::prelude::*;

fn invalid_keys(w: &[ConfigWarning]) -> Vec<String> {
    let mut v: Vec<String> = w
        .iter()
        .filter_map(|w| match w {
            ConfigWarning::InvalidValue { key, .. } => Some(key.clone()),
            _ => None,
        })
        .collect();
    v.sort();
    v
}

/// KP-006: documented defaults.
#[test]
fn kp006_defaults() {
    let d = AppConfig::default();
    assert!(d.telemetry.enabled);
    assert_eq!(d.telemetry.endpoint, "127.0.0.1:35120");
    assert!(d.dof.enabled);
    let r = AppConfig::from_toml_str("");
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert_eq!(r.config, d);
}

/// KP-006: the three keys load from a hand-written file, and only they change.
#[test]
fn kp006_keys_load() {
    let r = AppConfig::from_toml_str(
        "[telemetry]\nenabled = false\nendpoint = \"10.0.0.5:4000\"\n[dof]\nenabled = false\n",
    );
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert!(!r.config.telemetry.enabled);
    assert_eq!(r.config.telemetry.endpoint, "10.0.0.5:4000");
    assert!(!r.config.dof.enabled);
    let mut expect = AppConfig::default();
    expect.telemetry.enabled = false;
    expect.telemetry.endpoint = "10.0.0.5:4000".into();
    expect.dof.enabled = false;
    assert_eq!(r.config, expect);
}

/// KP-006: missing keys take their defaults, independently of each other.
#[test]
fn kp006_missing_keys_take_defaults() {
    let d = AppConfig::default();
    let r = AppConfig::from_toml_str("[telemetry]\nenabled = false\n");
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert!(!r.config.telemetry.enabled);
    assert_eq!(r.config.telemetry.endpoint, d.telemetry.endpoint);
    assert_eq!(r.config.dof, d.dof);

    let r = AppConfig::from_toml_str("[telemetry]\nendpoint = \"192.168.1.2:9\"\n");
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert!(r.config.telemetry.enabled);
    assert_eq!(r.config.telemetry.endpoint, "192.168.1.2:9");

    let r = AppConfig::from_toml_str("wireframe = true\n[dof]\n");
    assert!(r.config.wireframe);
    assert_eq!(r.config.dof, d.dof);
    assert_eq!(r.config.telemetry, d.telemetry);
}

/// KP-006: a wrong-typed value falls back to its default, with a warning, and
/// leaves its siblings alone.
#[test]
fn kp006_wrong_type_falls_back_with_warning() {
    let d = AppConfig::default();

    let r = AppConfig::from_toml_str(
        "[telemetry]\nenabled = \"yes\"\nendpoint = \"10.0.0.5:4000\"\n[dof]\nenabled = false\n",
    );
    assert!(
        r.config.telemetry.enabled,
        "fell back to the default (true)"
    );
    assert_eq!(r.config.telemetry.endpoint, "10.0.0.5:4000");
    assert!(!r.config.dof.enabled);
    assert_eq!(invalid_keys(&r.warnings), ["telemetry.enabled"]);
    assert!(
        !r.warnings
            .iter()
            .any(|w| matches!(w, ConfigWarning::Corrupt(_))),
        "{:?}",
        r.warnings
    );

    let r = AppConfig::from_toml_str("[telemetry]\nenabled = false\nendpoint = 5\n");
    assert!(!r.config.telemetry.enabled);
    assert_eq!(r.config.telemetry.endpoint, d.telemetry.endpoint);
    assert_eq!(invalid_keys(&r.warnings), ["telemetry.endpoint"]);

    let r = AppConfig::from_toml_str("[dof]\nenabled = 1\n");
    assert_eq!(r.config.dof, d.dof);
    assert_eq!(invalid_keys(&r.warnings), ["dof.enabled"]);

    // Whole tables of the wrong type.
    let r = AppConfig::from_toml_str("telemetry = 3\ndof = \"on\"\nwireframe = true\n");
    assert!(r.config.wireframe);
    assert_eq!(r.config.telemetry, d.telemetry);
    assert_eq!(r.config.dof, d.dof);
    assert_eq!(invalid_keys(&r.warnings), ["dof", "telemetry"]);
}

/// KP-006: save then load round-trips the new keys, in the documented layout.
#[test]
fn kp006_round_trip() {
    let mut c = AppConfig::default();
    c.telemetry.enabled = false;
    c.telemetry.endpoint = "10.0.0.5:4000".into();
    c.dof.enabled = false;
    let text = c.to_toml_string().unwrap();
    let doc: toml::Table = text.parse().unwrap();
    assert_eq!(doc["telemetry"]["enabled"].as_bool(), Some(false), "{text}");
    assert_eq!(
        doc["telemetry"]["endpoint"].as_str(),
        Some("10.0.0.5:4000"),
        "{text}"
    );
    assert_eq!(doc["dof"]["enabled"].as_bool(), Some(false), "{text}");
    let r = AppConfig::from_toml_str(&text);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert_eq!(r.config, c);

    // Through a file too, and defaults.
    let dir = std::env::temp_dir().join(format!("kp006-cfg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(pin_app::ui::CONFIG_FILE);
    c.save(&path).unwrap();
    let r = AppConfig::load(&path);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert_eq!(r.config, c);
    let d = AppConfig::default();
    d.save(&path).unwrap();
    let r = AppConfig::load(&path);
    assert!(r.warnings.is_empty(), "{:?}", r.warnings);
    assert_eq!(r.config, d);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// KP-006: unknown keys, also inside `[telemetry]` and `[dof]`, survive a save.
#[test]
fn kp006_unknown_keys_round_trip() {
    let r = AppConfig::from_toml_str(
        "future_top = 3\n[telemetry]\nenabled = false\nfuture_t = \"x\"\n[dof]\nfuture_d = 7\n",
    );
    assert!(!r.config.telemetry.enabled);
    assert!(
        r.warnings
            .iter()
            .all(|w| matches!(w, ConfigWarning::UnknownKey(_))),
        "{:?}",
        r.warnings
    );
    for key in ["future_top", "future_t", "future_d"] {
        assert!(
            r.warnings
                .iter()
                .any(|w| matches!(w, ConfigWarning::UnknownKey(k) if k.contains(key))),
            "no warning for {key}: {:?}",
            r.warnings
        );
    }
    let text = r.config.to_toml_string().unwrap();
    let doc: toml::Table = text.parse().unwrap();
    assert_eq!(doc["future_top"].as_integer(), Some(3), "{text}");
    assert_eq!(doc["telemetry"]["future_t"].as_str(), Some("x"), "{text}");
    assert_eq!(doc["telemetry"]["enabled"].as_bool(), Some(false), "{text}");
    assert_eq!(doc["dof"]["future_d"].as_integer(), Some(7), "{text}");
    let again = AppConfig::from_toml_str(&text);
    assert_eq!(again.config, r.config);
}

const KEYS: &[&str] = &["telemetry", "dof", "enabled", "endpoint", "extra", "x"];
const VALUES: &[&str] = &[
    "true",
    "false",
    "0",
    "-1",
    "nan",
    "inf",
    "\"\"",
    "\"a:1\"",
    "\"127.0.0.1:35120\"",
    "\"\\u0000\"",
    "[]",
    "{}",
    "{ enabled = 1 }",
    "{ endpoint = [] }",
];

fn line() -> impl Strategy<Value = String> {
    (
        proptest::sample::select(KEYS),
        proptest::sample::select(VALUES),
        0u8..4,
    )
        .prop_map(|(k, v, shape)| match shape {
            0 => format!("[{k}]"),
            1 => format!("{k} = {v}"),
            2 => format!("[telemetry]\n{k} = {v}"),
            _ => format!("[dof]\n{k} = {v}"),
        })
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 1000, failure_persistence: None, ..ProptestConfig::default() })]

    /// KP-006 / R11: bounded random bytes never panic the loader, and the
    /// result re-serialises and re-loads to the same config.
    #[test]
    fn kp006_random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let r = AppConfig::from_toml_bytes(&bytes);
        let text = r.config.to_toml_string().unwrap();
        let again = AppConfig::from_toml_str(&text);
        prop_assert_eq!(format!("{:?}", again.config), format!("{:?}", r.config));
    }

    /// Hostile values on the new keys: the loader stays total and the new
    /// fields always hold a bool / string.
    #[test]
    fn kp006_hostile_new_keys_never_panic(lines in proptest::collection::vec(line(), 0..16)) {
        let text = lines.join("\n");
        let r = AppConfig::from_toml_str(&text);
        let _ = r.config.telemetry.endpoint.len();
        let back = AppConfig::from_toml_str(&r.config.to_toml_string().unwrap());
        prop_assert_eq!(format!("{:?}", back.config), format!("{:?}", r.config));
    }
}
