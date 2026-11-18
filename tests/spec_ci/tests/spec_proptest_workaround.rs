//! Tripwire for the proptest debug-assertions workaround in the root `Cargo.toml`.
//!
//! The root manifest turns off `debug-assertions` for the `proptest` package because
//! proptest 1.12.0 has a wrong-sign `debug_assert` in `FloatUniform::sample`. This test
//! fails when the locked proptest version changes while the workaround is still present,
//! so removing the workaround is noticed automatically instead of being remembered.

use spec_ci::repo_root;

/// The only proptest version the workaround has been verified against.
const PINNED_PROPTEST: &str = "1.12.0";

const PROFILES: [&str; 2] = ["dev", "test"];

/// Parse `text` as a TOML table, naming `what` in the error.
fn parse_table(text: &str, what: &str) -> Result<toml::Table, String> {
    text.parse::<toml::Table>()
        .map_err(|e| format!("{what} is not valid TOML: {e}"))
}

/// Version of the `proptest` package in `Cargo.lock` text.
///
/// Errors when the lock is malformed, has no proptest entry, or has more than one.
fn locked_proptest_version(cargo_lock: &str) -> Result<String, String> {
    let table = parse_table(cargo_lock, "Cargo.lock")?;
    let packages = table
        .get("package")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| "Cargo.lock has no `package` array".to_owned())?;
    let versions: Vec<&str> = packages
        .iter()
        .filter(|p| p.get("name").and_then(toml::Value::as_str) == Some("proptest"))
        .map(|p| p.get("version").and_then(toml::Value::as_str).unwrap_or(""))
        .collect();
    match versions.as_slice() {
        [] => Err("Cargo.lock has no proptest package".to_owned()),
        [""] => Err("Cargo.lock proptest entry has no version".to_owned()),
        [v] => Ok((*v).to_owned()),
        many => Err(format!(
            "Cargo.lock has {} proptest entries ({many:?}); expected exactly one",
            many.len()
        )),
    }
}

/// Whether `profile.<profile>.package.proptest.debug-assertions` is the boolean `false`.
///
/// Works on the parsed table, so quoted keys and inline tables are equivalent to bare headers.
fn has_stanza(manifest: &toml::Table, profile: &str) -> bool {
    manifest
        .get("profile")
        .and_then(|v| v.get(profile))
        .and_then(|v| v.get("package"))
        .and_then(|v| v.get("proptest"))
        .and_then(|v| v.get("debug-assertions"))
        .and_then(toml::Value::as_bool)
        == Some(false)
}

/// Check the workaround against the locked proptest version.
fn check_workaround(cargo_toml: &str, cargo_lock: &str) -> Result<(), String> {
    let manifest = parse_table(cargo_toml, "Cargo.toml")?;
    let version = locked_proptest_version(cargo_lock)?;
    let present: Vec<bool> = PROFILES.iter().map(|p| has_stanza(&manifest, p)).collect();
    let any = present.iter().any(|p| *p);
    let all = present.iter().all(|p| *p);
    if any && version != PINNED_PROPTEST {
        return Err(format!(
            "proptest was upgraded to {version} (workaround verified only for {PINNED_PROPTEST}). \
             Check whether the negative-step debug_assert in `FloatUniform::sample` is fixed. \
             If it is fixed, remove both [profile.dev.package.proptest] and \
             [profile.test.package.proptest] stanzas from the root Cargo.toml and confirm \
             crates/pin_physics/tests/proptest_f32_sampler.rs passes without them. \
             If it is not fixed, bump PINNED_PROPTEST in this test."
        ));
    }
    if version == PINNED_PROPTEST && !all {
        return Err(format!(
            "proptest is {PINNED_PROPTEST} but the root Cargo.toml no longer has both \
             `debug-assertions = false` stanzas for it; the workaround for the \
             FloatUniform::sample debug_assert must stay until proptest is fixed"
        ));
    }
    Ok(())
}

fn read_root(name: &str) -> String {
    let path = repo_root().join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {}: {e}", path.display()))
}

const STANZA: &str = "[profile.dev.package.proptest]\ndebug-assertions = false\n\n\
                      [profile.test.package.proptest]\ndebug-assertions = false\n";
const NO_STANZA: &str = "[workspace]\nmembers = []\n";

fn lock(version: &str) -> String {
    format!(
        "[[package]]\nname = \"other\"\nversion = \"9.9.9\"\n\n\
         [[package]]\nname = \"proptest\"\nversion = \"{version}\"\n"
    )
}

#[test]
fn repo_proptest_workaround_matches_locked_version() {
    let result = check_workaround(&read_root("Cargo.toml"), &read_root("Cargo.lock"));
    assert!(result.is_ok(), "{}", result.unwrap_err());
}

#[test]
fn newer_proptest_with_stanza_fails() {
    let err = check_workaround(STANZA, &lock("1.13.0")).unwrap_err();
    assert!(err.contains("upgraded") && err.contains("FloatUniform::sample"));
    assert!(err.contains("remove both") && err.contains("bump"));
}

#[test]
fn pinned_proptest_with_stanza_passes() {
    assert_eq!(check_workaround(STANZA, &lock("1.12.0")), Ok(()));
}

#[test]
fn pinned_proptest_without_stanza_fails() {
    assert!(check_workaround(NO_STANZA, &lock("1.12.0")).is_err());
}

#[test]
fn pinned_proptest_with_one_stanza_fails() {
    let one = "[profile.dev.package.proptest]\ndebug-assertions = false\n";
    assert!(check_workaround(one, &lock("1.12.0")).is_err());
}

#[test]
fn newer_proptest_without_stanza_passes() {
    assert_eq!(check_workaround(NO_STANZA, &lock("1.13.0")), Ok(()));
}

#[test]
fn missing_proptest_in_lock_fails() {
    assert!(check_workaround(STANZA, "[[package]]\nname = \"x\"\nversion = \"1.0.0\"\n").is_err());
}

const STANZA_QUOTED: &str = "[profile.dev.package.\"proptest\"]\ndebug-assertions = false\n\n\
                             [profile.test.package.\"proptest\"]\ndebug-assertions = false\n";
const STANZA_INLINE: &str = "[profile.dev.package]\nproptest = { debug-assertions = false }\n\n\
                             [profile.test.package]\nproptest = { debug-assertions = false }\n";
const STANZA_TRUE: &str = "[profile.dev.package.proptest]\ndebug-assertions = true\n\n\
                           [profile.test.package.proptest]\ndebug-assertions = true\n";

#[test]
fn quoted_headers_match_bare_headers() {
    assert_eq!(check_workaround(STANZA_QUOTED, &lock("1.12.0")), Ok(()));
    let err = check_workaround(STANZA_QUOTED, &lock("1.13.0")).unwrap_err();
    assert!(err.contains("upgraded"));
}

#[test]
fn inline_table_form_is_detected() {
    assert_eq!(check_workaround(STANZA_INLINE, &lock("1.12.0")), Ok(()));
    assert!(check_workaround(STANZA_INLINE, &lock("1.13.0")).is_err());
}

#[test]
fn debug_assertions_true_counts_as_absent() {
    assert!(check_workaround(STANZA_TRUE, &lock("1.12.0")).is_err());
    assert_eq!(check_workaround(STANZA_TRUE, &lock("1.13.0")), Ok(()));
}

#[test]
fn malformed_manifest_is_an_error() {
    let err = check_workaround("[profile.dev.package.proptest\n", &lock("1.12.0")).unwrap_err();
    assert!(err.contains("Cargo.toml is not valid TOML"), "{err}");
}

#[test]
fn malformed_lock_is_an_error() {
    let err = check_workaround(STANZA, "[[package\n").unwrap_err();
    assert!(err.contains("Cargo.lock is not valid TOML"), "{err}");
}

#[test]
fn duplicate_proptest_entries_fail() {
    let dup = format!("{}\n{}", lock("1.12.0"), lock("1.13.0"));
    let err = check_workaround(STANZA, &dup).unwrap_err();
    assert!(err.contains("2 proptest entries"), "{err}");
}
