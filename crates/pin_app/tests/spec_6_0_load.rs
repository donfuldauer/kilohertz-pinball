//! Subphase 6.0 spec tests (Independent Tester): `pin_app::orchestrator::load_validated`
//! and `run_headless_with` (brief 6.0 item 3).
//!
//! `load_validated(bundle, cfg)` runs `pin_assets::load_table`, the 2.8
//! clearance validator for every declared ball profile, then
//! `Simulation::load`. A bundle whose geometry cannot carry a declared ball
//! is refused with a message naming the offending feature.
//!
//! Fixtures (`tests/fixtures/spec_6_0/`, hand-made from the 2.5 replay
//! fixture with its orbit lane narrowed to 32 mm):
//! * `fits`: profiles `standard_steel` and `ceramic_powerball` (27 mm);
//! * `oversized`: profiles `standard_steel` and `jumbo_xl_resin` (79.4 mm),
//!   which fits neither the 32 mm lane `tablegen:lanes[0]` nor the flipper gap.
//!   `Simulation::load` alone accepts it (it does not check clearance).

#[allow(dead_code, unused_imports)]
mod support;

#[path = "fixtures/spec_6_0/common.rs"]
mod common;

use common::{fixture, greybox_bundle, scratch};
use pin_app::orchestrator::{load_validated, run_headless_with};
use pin_sim::{SimConfig, Simulation};
use proptest::prelude::*;
use std::path::Path;

#[test]
fn fitting_fixture_loads() {
    let sim = load_validated(&fixture("fits"), SimConfig::default())
        .expect("a table that carries every declared ball loads");
    assert_eq!(sim.tick(), 0);
}

#[test]
fn oversized_ball_is_refused_naming_the_feature() {
    let bundle = fixture("oversized");
    // The fixture itself is a loadable table: only clearance refuses it.
    assert!(
        Simulation::load(&bundle, SimConfig::default()).is_ok(),
        "fixture precondition: Simulation::load accepts it"
    );
    let err = match load_validated(&bundle, SimConfig::default()) {
        Ok(_) => panic!("jumbo_xl_resin (79.4 mm) in a 32 mm lane must be refused"),
        Err(e) => e,
    };
    assert!(
        err.contains("tablegen:lanes[0]"),
        "the error names the 32 mm lane: {err}"
    );
    assert!(
        err.contains("jumbo_xl_resin"),
        "the error names the ball profile: {err}"
    );
}

#[test]
fn greybox_bundle_loads_validated() {
    let mut sim = load_validated(greybox_bundle(), SimConfig::default())
        .expect("the grey-box table passes load_table, clearance and Simulation::load");
    sim.advance(100);
    assert_eq!(sim.tick(), 100);
    assert!(!sim.script_faulted());
}

#[test]
fn missing_bundle_is_an_error() {
    let dir = scratch("missing");
    assert!(load_validated(&dir.join("no_such_bundle"), SimConfig::default()).is_err());
    assert!(
        load_validated(&dir, SimConfig::default()).is_err(),
        "empty folder"
    );
}

/// A broken `table.toml` in an otherwise valid bundle is an error, not a panic.
#[test]
fn broken_manifest_is_an_error() {
    let dir = scratch("broken_manifest");
    for f in ["backglass.toml", "rules.lua", "playfield.glb"] {
        std::fs::copy(fixture("fits").join(f), dir.join(f)).expect("copy fixture file");
    }
    std::fs::write(dir.join("table.toml"), "schema_version = 1\n[meta\n").expect("write");
    assert!(load_validated(&dir, SimConfig::default()).is_err());
}

/// `run_headless_with` runs the real simulation on simulated time.
#[test]
fn run_headless_with_runs_the_real_sim() {
    let sim = load_validated(&fixture("fits"), SimConfig::default()).expect("loads");
    assert_eq!(run_headless_with(sim, 1), Ok(1));
    let sim = load_validated(&fixture("fits"), SimConfig::default()).expect("loads");
    // 60 s of simulated time returns without real-time pacing (D8: no
    // wall-clock assertion; a sleeping loop shows up as a hung test).
    assert_eq!(run_headless_with(sim, 60_000), Ok(60_000));
}

fn try_load(path: &Path) {
    let _ = load_validated(path, SimConfig::default());
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, .. ProptestConfig::default() })]

    /// Bounded random bytes as a `.zip` bundle: an error, never a panic.
    #[test]
    fn random_zip_bytes_do_not_panic(bytes in prop::collection::vec(any::<u8>(), 0..4096)) {
        let dir = scratch("fuzz_zip");
        let path = dir.join("bundle.zip");
        std::fs::write(&path, &bytes).expect("write");
        try_load(&path);
        // A zip header followed by random bytes.
        let mut z = b"PK\x03\x04".to_vec();
        z.extend_from_slice(&bytes);
        std::fs::write(&path, &z).expect("write");
        try_load(&path);
    }

    /// Bounded random bytes as `table.toml` of a folder bundle.
    #[test]
    fn random_manifest_bytes_do_not_panic(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        let dir = scratch("fuzz_manifest");
        for f in ["backglass.toml", "rules.lua", "playfield.glb"] {
            std::fs::copy(fixture("fits").join(f), dir.join(f)).expect("copy");
        }
        std::fs::write(dir.join("table.toml"), &bytes).expect("write");
        try_load(&dir);
    }

    /// Random path names (no panic on odd or non-existent paths).
    #[test]
    fn random_paths_do_not_panic(name in "[a-zA-Z0-9 ._/-]{0,64}") {
        let dir = scratch("fuzz_path");
        try_load(&dir.join(name));
    }
}
