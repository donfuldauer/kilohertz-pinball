//! Subphase 6.4 spec tests (Independent Tester), brief S2: the Feature Lab
//! bundle passes `load_validated` and the 2.8 clearance validator, the XL
//! restriction is what makes it pass (removing or shrinking it is refused
//! with a message naming the feature and the XL profile), and broken
//! manifests are errors, never panics.

#[path = "fixtures/spec_6_4/common.rs"]
mod common;

use common::*;
use pin_app::orchestrator::load_validated;
use pin_assets::Vfs;
use pin_sim::SimConfig;
use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use std::path::Path;

/// Feature labels of the clearance validator (`pin_tablegen::clearance::Feature::label`).
const FEATURE_LABELS: [&str; 7] = [
    "lane",
    "flipper gap",
    "wireform",
    "scoop",
    "vuk exit",
    "deck headroom",
    "restriction",
];

#[test]
fn bundle_loads_validated() {
    big_stack(|| {
        let mut sim = load();
        sim.advance(100);
        assert_eq!(sim.tick(), 100);
        assert_healthy(&sim, "after load");
    });
}

#[test]
fn tablegen_clearance_passes_on_the_bundle() {
    let vfs = Vfs::open(bundle()).expect("open bundle");
    let report = pin_tablegen::clearance::validate_bundle(&vfs).expect("bundle loads");
    assert!(
        report.is_ok(),
        "clearance errors: {:#?}",
        report
            .errors
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
    );
}

/// Asserts `err` names a feature (a label plus a non-empty name or key
/// path) and the XL profile or its diameter.
fn assert_names_feature_and_xl(err: &str, what: &str) {
    assert!(
        err.contains(XL) || err.contains("79.4"),
        "{what}: the error mentions {XL} or 79.4 mm: {err}"
    );
    let named = err.lines().any(|l| {
        let l = l.trim();
        FEATURE_LABELS.iter().any(|lab| {
            l.strip_prefix(lab)
                .and_then(|rest| rest.strip_prefix(' '))
                .and_then(|rest| rest.split(" (").next())
                .is_some_and(|name| !name.trim().is_empty())
        })
    });
    let key_path = err.contains("table.toml:") || err.contains("restrict") || err.contains('[');
    assert!(
        named && key_path,
        "{what}: the error names a feature (label + name) and a key path: {err}"
    );
}

fn xl_restrict_index(t: &toml::Table) -> usize {
    t["balls"]["restrict"]
        .as_array()
        .expect("[balls] restrict array")
        .iter()
        .position(|e| e.get("profile").and_then(|p| p.as_str()) == Some(XL))
        .expect("restrict entry for the XL ball")
}

#[test]
fn without_the_xl_restriction_the_bundle_is_refused() {
    let dir = modified_bundle("no_restrict", |t| {
        let i = xl_restrict_index(t);
        let balls = t["balls"].as_table_mut().unwrap();
        let arr = balls["restrict"].as_array_mut().unwrap();
        arr.remove(i);
        if arr.is_empty() {
            balls.remove("restrict");
        }
    });
    big_stack(move || match load_validated(&dir, SimConfig::default()) {
        Ok(_) => panic!("an unrestricted 79.4 mm ball must not pass clearance"),
        Err(e) => assert_names_feature_and_xl(&e, "restriction removed"),
    });
}

#[test]
fn a_40mm_square_xl_area_is_refused() {
    let exit = table()
        .manifest
        .dispensers
        .iter()
        .find(|d| d.id == "xl_feed")
        .expect("xl_feed")
        .exit_position;
    let (cx, cy) = (f64::from(exit[0]), f64::from(exit[1]));
    let dir = modified_bundle("square_area", |t| {
        let i = xl_restrict_index(t);
        let e = t["balls"]["restrict"].as_array_mut().unwrap()[i]
            .as_table_mut()
            .unwrap();
        let sq: Vec<toml::Value> = [(-20.0, -20.0), (20.0, -20.0), (20.0, 20.0), (-20.0, 20.0)]
            .iter()
            .map(|(dx, dy)| {
                toml::Value::Array(vec![
                    toml::Value::Float(cx + dx),
                    toml::Value::Float(cy + dy),
                ])
            })
            .collect();
        e.insert("area".into(), toml::Value::Array(sq));
    });
    big_stack(move || match load_validated(&dir, SimConfig::default()) {
        Ok(_) => panic!("a 40 mm square cannot carry the 79.4 mm ball"),
        Err(e) => assert_names_feature_and_xl(&e, "40 mm square area"),
    });
}

fn try_load(path: &Path) -> Result<(), String> {
    load_validated(path, SimConfig::default()).map(|_| ())
}

/// 100 random-byte manifests and 100 truncated copies of the real one
/// (200 cases, fixed ChaCha seed, no persistence): `load_validated`
/// returns `Ok` or `Err`, never panics.
#[test]
fn broken_manifests_never_panic() {
    big_stack(|| {
        let dir = scratch("fuzz");
        copy_dir(bundle(), &dir);
        let original = std::fs::read(bundle().join("table.toml")).expect("table.toml");
        let cfg = Config {
            cases: 100,
            failure_persistence: None,
            ..Config::default()
        };
        let mut runner = TestRunner::new_with_rng(
            cfg.clone(),
            TestRng::from_seed(RngAlgorithm::ChaCha, &[0x64; 32]),
        );
        runner
            .run(&prop::collection::vec(any::<u8>(), 0..4096), |bytes| {
                std::fs::write(dir.join("table.toml"), &bytes).expect("write");
                let r = std::panic::catch_unwind(|| try_load(&dir));
                prop_assert!(r.is_ok(), "load_validated panicked on random bytes");
                Ok(())
            })
            .unwrap();
        let mut runner =
            TestRunner::new_with_rng(cfg, TestRng::from_seed(RngAlgorithm::ChaCha, &[0x65; 32]));
        let n = original.len();
        runner
            .run(&(0..n), |cut| {
                std::fs::write(dir.join("table.toml"), &original[..cut]).expect("write");
                let r = std::panic::catch_unwind(|| try_load(&dir));
                prop_assert!(
                    r.is_ok(),
                    "load_validated panicked on a {cut}-byte truncation"
                );
                Ok(())
            })
            .unwrap();
        // And the untouched manifest still loads from the same folder.
        std::fs::write(dir.join("table.toml"), &original).expect("restore");
        assert!(try_load(&dir).is_ok(), "the restored copy loads");
    });
}
