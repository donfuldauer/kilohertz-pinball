//! Contract: `versions.toml` at the repo root pins every crate exactly and
//! records the toolchain, so Subphase 1.1 can consume it unchanged.

use std::collections::BTreeMap;

use regex::Regex;
use spec_0_1::{is_exact_pin, pin_triple, read_repo_file};
use toml::{Table, Value};

const FILE: &str = "versions.toml";
const REQUIRED_CRATES: [&str; 6] = ["sdl3", "sdl3-sys", "wgpu", "parry3d", "mlua", "wasmtime"];

fn load() -> Table {
    let text = read_repo_file(FILE);
    match text.parse::<Table>() {
        Ok(table) => table,
        Err(err) => panic!("{FILE} is not valid TOML: {err}"),
    }
}

fn table<'a>(root: &'a Table, name: &str) -> &'a Table {
    match root.get(name) {
        Some(Value::Table(t)) => t,
        Some(other) => panic!(
            "{FILE}: `{name}` must be a table, found {}",
            other.type_str()
        ),
        None => panic!(
            "{FILE}: missing required [{name}] table (top-level keys present: {:?})",
            root.keys().collect::<Vec<_>>()
        ),
    }
}

/// `[crates]` as name -> pin string; panics if any value is not a string.
fn crate_pins(root: &Table) -> BTreeMap<String, String> {
    let crates = table(root, "crates");
    let non_strings: Vec<String> = crates
        .iter()
        .filter(|(_, v)| !v.is_str())
        .map(|(k, v)| format!("{k} ({})", v.type_str()))
        .collect();
    assert!(
        non_strings.is_empty(),
        "{FILE}: every [crates] value must be a string of the form \"=x.y.z\"; non-string entries: {non_strings:?}"
    );
    crates
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().unwrap_or_default().to_owned()))
        .collect()
}

#[test]
fn versions_toml_parses_as_toml() {
    let root = load();
    assert!(!root.is_empty(), "{FILE} parsed but is empty");
}

#[test]
fn crates_table_is_non_empty_and_every_value_is_an_exact_pin() {
    let pins = crate_pins(&load());
    assert!(!pins.is_empty(), "{FILE}: [crates] table is empty");
    let bad: Vec<String> = pins
        .iter()
        .filter(|(_, v)| !is_exact_pin(v))
        .map(|(k, v)| format!("{k} = {v:?}"))
        .collect();
    assert!(
        bad.is_empty(),
        "{FILE}: these [crates] entries are not exact pins (expected \"=MAJOR.MINOR.PATCH\" with optional pre-release suffix): {bad:?}"
    );
}

#[test]
fn required_crates_are_pinned() {
    let pins = crate_pins(&load());
    let missing: Vec<&str> = REQUIRED_CRATES
        .iter()
        .copied()
        .filter(|name| !pins.contains_key(*name))
        .collect();
    assert!(
        missing.is_empty(),
        "{FILE}: [crates] is missing required pins {missing:?} (present: {:?})",
        pins.keys().collect::<Vec<_>>()
    );
}

#[test]
fn crate_names_are_valid_and_unique_modulo_dash_underscore() {
    let pins = crate_pins(&load());
    let name_re = Regex::new(r"^[A-Za-z][A-Za-z0-9_-]*$").expect("regex");
    let mut seen: BTreeMap<String, &str> = BTreeMap::new();
    for name in pins.keys() {
        assert!(
            name_re.is_match(name),
            "{FILE}: [crates] key {name:?} is not a valid crates.io package name"
        );
        let canonical = name.to_ascii_lowercase().replace('_', "-");
        if let Some(previous) = seen.insert(canonical, name) {
            panic!("{FILE}: [crates] pins the same crate twice: {previous:?} and {name:?}");
        }
    }
}

#[test]
fn sdl3_and_sdl3_sys_are_pinned_together() {
    let pins = crate_pins(&load());
    let (hi, sys) = (pins.contains_key("sdl3"), pins.contains_key("sdl3-sys"));
    assert_eq!(
        hi, sys,
        "{FILE}: sdl3 and sdl3-sys must be pinned together (sdl3 pinned: {hi}, sdl3-sys pinned: {sys}); \
         an unpinned -sys crate lets the native SDL version drift"
    );
}

#[test]
fn crate_families_share_a_release_line() {
    let pins = crate_pins(&load());
    // wasmtime-* crates are released in lockstep with wasmtime: identical versions.
    if let Some(base) = pins.get("wasmtime") {
        for (name, pin) in pins.iter().filter(|(n, _)| n.starts_with("wasmtime-")) {
            assert_eq!(
                pin, base,
                "{FILE}: {name} is pinned to {pin} but wasmtime is pinned to {base}; the wasmtime family must match exactly"
            );
        }
    }
    // wgpu-core / wgpu-hal / wgpu-types / naga share wgpu's major version.
    if let Some(base) = pins.get("wgpu").and_then(|p| pin_triple(p)) {
        for name in ["wgpu-core", "wgpu-hal", "wgpu-types", "naga"] {
            if let Some(other) = pins.get(name).and_then(|p| pin_triple(p)) {
                assert_eq!(
                    other.0, base.0,
                    "{FILE}: {name} major version {} does not match wgpu major version {}",
                    other.0, base.0
                );
            }
        }
    }
}

#[test]
fn toolchain_records_rust_version_and_edition_2024() {
    let root = load();
    let toolchain = table(&root, "toolchain");
    match toolchain.get("edition") {
        Some(Value::String(e)) => {
            assert_eq!(
                e, "2024",
                "{FILE}: [toolchain] edition must be \"2024\", found {e:?}"
            )
        }
        Some(other) => panic!(
            "{FILE}: [toolchain] edition must be the string \"2024\", found {} `{other}`",
            other.type_str()
        ),
        None => panic!("{FILE}: [toolchain] is missing `edition`"),
    }
    let keys = ["rust", "rust-version", "rust_version", "channel", "version"];
    let Some((key, value)) = keys.iter().find_map(|k| toolchain.get(*k).map(|v| (*k, v))) else {
        panic!(
            "{FILE}: [toolchain] has no rust version (looked for keys {keys:?}; present: {:?})",
            toolchain.keys().collect::<Vec<_>>()
        );
    };
    let Value::String(version) = value else {
        panic!(
            "{FILE}: [toolchain] {key} must be a string, found {}",
            value.type_str()
        );
    };
    let version_re = Regex::new(r"^=?[0-9]+\.[0-9]+(\.[0-9]+)?$").expect("regex");
    assert!(
        version_re.is_match(version),
        "{FILE}: [toolchain] {key} = {version:?} is not a concrete Rust release (expected e.g. \"1.97.0\"; \
         floating channels like \"stable\" are not a pin)"
    );
    let minor: u64 = version
        .trim_start_matches('=')
        .split('.')
        .nth(1)
        .and_then(|m| m.parse().ok())
        .unwrap_or(0);
    assert!(
        version.trim_start_matches('=').starts_with("1.") && minor >= 85,
        "{FILE}: [toolchain] {key} = {version:?} cannot build edition 2024 (needs Rust >= 1.85)"
    );
}

#[test]
fn notes_section_exists_and_is_not_empty() {
    let root = load();
    match root.get("notes") {
        Some(Value::Table(t)) => assert!(!t.is_empty(), "{FILE}: [notes] is empty"),
        Some(Value::String(s)) => assert!(!s.trim().is_empty(), "{FILE}: notes is empty"),
        Some(Value::Array(a)) => assert!(!a.is_empty(), "{FILE}: notes is empty"),
        Some(other) => panic!(
            "{FILE}: `notes` must be a table, found {}",
            other.type_str()
        ),
        None => panic!("{FILE}: missing required [notes] section"),
    }
}

#[test]
fn sdl3_build_approach_is_recorded() {
    // Checked against the raw text so the record may live in [notes], a
    // dedicated features table, or a comment next to the sdl3 pin.
    let text = read_repo_file(FILE);
    let approach =
        Regex::new(r"(?i)build[-_ ]from[-_ ]source|from[-_ ]source|vendored|bundled|\bcmake\b")
            .expect("regex");
    assert!(
        approach.is_match(&text),
        "{FILE}: the SDL3 build approach is not recorded (expected a mention of the sdl3 \
         `build-from-source` style feature, e.g. in [notes])"
    );
    let lower = text.to_lowercase();
    assert!(
        lower.matches("sdl").count() > 2,
        "{FILE}: SDL is only mentioned in the two pins; the build approach note must say it is about SDL3"
    );
}
