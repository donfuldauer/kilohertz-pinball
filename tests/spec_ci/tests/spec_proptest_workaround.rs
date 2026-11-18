//! Tripwire for the proptest debug-assertions workaround in the root `Cargo.toml`.
//!
//! The root manifest turns off `debug-assertions` for the `proptest` package because
//! proptest 1.12.0 has a wrong-sign `debug_assert` in `FloatUniform::sample`. This test
//! fails when the locked proptest version changes while the workaround is still present,
//! so removing the workaround is noticed automatically instead of being remembered.

use spec_ci::repo_root;

/// The only proptest version the workaround has been verified against.
const PINNED_PROPTEST: &str = "1.12.0";

const STANZA_HEADERS: [&str; 2] = [
    "[profile.dev.package.proptest]",
    "[profile.test.package.proptest]",
];

/// Version of the `proptest` package in `Cargo.lock` text, if present.
fn locked_proptest_version(cargo_lock: &str) -> Option<String> {
    let mut in_proptest = false;
    for raw in cargo_lock.lines() {
        let line = raw.trim();
        if line == "[[package]]" {
            in_proptest = false;
        } else if line == "name = \"proptest\"" {
            in_proptest = true;
        } else if in_proptest && let Some(v) = line.strip_prefix("version = \"") {
            return v.strip_suffix('"').map(str::to_owned);
        }
    }
    None
}

/// Whether the table `header` exists in `cargo_toml` with `debug-assertions = false`.
fn has_stanza(cargo_toml: &str, header: &str) -> bool {
    let mut in_table = false;
    for raw in cargo_toml.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            in_table = line == header;
        } else if in_table {
            let compact: String = line.split_whitespace().collect();
            if compact == "debug-assertions=false" {
                return true;
            }
        }
    }
    false
}

/// Check the workaround against the locked proptest version.
fn check_workaround(cargo_toml: &str, cargo_lock: &str) -> Result<(), String> {
    let version = locked_proptest_version(cargo_lock)
        .ok_or_else(|| "Cargo.lock has no proptest package".to_owned())?;
    let present: Vec<bool> = STANZA_HEADERS
        .iter()
        .map(|h| has_stanza(cargo_toml, h))
        .collect();
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
