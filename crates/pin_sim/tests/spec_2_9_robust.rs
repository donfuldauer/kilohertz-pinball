//! Subphase 2.9 spec tests (Independent Tester), T7: robustness. Any
//! `SoakConfig` in the brief's ranges returns `Ok` on the clean fixture; a
//! bundle path of random bytes (as a zip, a folder or a path name) returns
//! `Err`, never a panic.

#[path = "fixtures/spec_2_9/support.rs"]
mod support;

#[path = "fixtures/spec_2_9/soak_support.rs"]
mod soak_support;

use pin_sim::autoplay::run_soak;
use proptest::prelude::*;
use soak_support::*;
use support::*;

proptest! {
    #![proptest_config(ProptestConfig { cases: 8, ..ProptestConfig::default() })]

    #[test]
    fn any_config_in_range_is_ok_on_the_clean_fixture(
        games in 1u32..6,
        threads in 1usize..8,
        max_game_ticks in 1000u64..20000,
        seed in any::<u64>(),
        sk in 0usize..3,
    ) {
        let c = cfg(games, seed, skill(sk), threads, max_game_ticks);
        let b = clean();
        let r = on_sim_thread(move || run_soak(&b, &c).map(|r| r.to_json()));
        let json = match r {
            Ok(j) => j,
            Err(e) => return Err(TestCaseError::fail(format!("run_soak failed: {e}"))),
        };
        let v = parse_json(&json);
        prop_assert_eq!(v.get("schema").and_then(serde_json::Value::as_u64), Some(1));
        for f in json_findings(&v) {
            if !f["game"].is_null() {
                let game = f["game"].as_u64();
                prop_assert!(
                    game.is_some_and(|g| g < u64::from(games)),
                    "game {:?} of {}",
                    game,
                    games
                );
            }
            let tick = f["tick"].as_u64().unwrap_or(u64::MAX);
            prop_assert!(tick <= max_game_ticks, "tick {} > max {}", tick, max_game_ticks);
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    #[test]
    fn random_bytes_bundle_is_an_error_not_a_panic(
        bytes in proptest::collection::vec(any::<u8>(), 0..4096),
        as_zip in any::<bool>(),
    ) {
        let dir = temp_dir("rand");
        let path = if as_zip {
            let p = dir.join("table.zip");
            std::fs::write(&p, &bytes).unwrap();
            p
        } else {
            // A folder whose table.toml is the random bytes.
            let p = dir.join("bundle");
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("table.toml"), &bytes).unwrap();
            p
        };
        let c = cfg(1, 1, skill(1), 1, 1000);
        let r = on_sim_thread(move || run_soak(&path, &c).is_err());
        prop_assert!(r, "random bytes accepted as a bundle");
    }

    #[test]
    fn random_path_names_are_errors(name in "[a-zA-Z0-9_. -]{1,40}") {
        let dir = temp_dir("rand-path");
        let path = dir.join(name);
        let c = cfg(1, 1, skill(1), 1, 1000);
        let r = on_sim_thread(move || run_soak(&path, &c).is_err());
        prop_assert!(r, "a nonexistent path was accepted");
    }
}

#[test]
fn a_damaged_copy_of_the_clean_zip_is_an_error() {
    // The clean bundle zipped and truncated at several points.
    let dir = temp_dir("trunc");
    let zip = dir.join("clean.zip");
    zip_dir(&clean(), &zip);
    let full = std::fs::read(&zip).unwrap();
    let ok = {
        let z = zip.clone();
        on_sim_thread(move || run_soak(&z, &cfg(1, 1, skill(1), 1, 1000)).is_ok())
    };
    assert!(ok, "the intact zip must soak");
    for cut in [10usize, full.len() / 3, full.len() / 2, full.len() - 30] {
        let p = dir.join(format!("cut{cut}.zip"));
        std::fs::write(&p, &full[..cut]).unwrap();
        let err = on_sim_thread(move || run_soak(&p, &cfg(1, 1, skill(1), 1, 1000)).is_err());
        assert!(err, "zip cut at {cut} of {} accepted", full.len());
    }
}
