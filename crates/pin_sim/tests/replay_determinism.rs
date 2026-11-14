//! Subphase 2.5 verification 1: a 10 000-tick recording (balls moving,
//! flippers firing from recorded inputs, a Lua script scoring) replays to an
//! identical state hash 10 times in a row; a recording of another table is
//! refused.

mod sim_support;

use pin_sim::cli::record_pattern;
use pin_sim::replay::{Recorder, Recording, ReplayError, replay};
use pin_sim::{SimConfig, Simulation};
use sim_support::*;

const TICKS: u64 = 10_000;

fn record(seed: u64) -> (u64, Recording, Simulation) {
    let bundle = fixture();
    on_sim_thread(move || {
        let sim = Simulation::load(
            &bundle,
            SimConfig {
                seed,
                ..SimConfig::default()
            },
        )
        .unwrap();
        let (sim, rec) = record_pattern(sim, TICKS, false);
        (sim.state_hash(), rec, sim)
    })
}

#[test]
fn ten_thousand_tick_recording_replays_identically_ten_times() {
    let (recorded_hash, rec, sim) = record(0x4B485A);
    // The run exercised what the criterion names.
    assert_eq!(rec.ticks, TICKS);
    assert_eq!(rec.seed, 0x4B485A);
    assert_eq!(rec.table_hash, sim.table_hash());
    assert!(rec.events.len() >= 50, "{} events", rec.events.len());
    assert!(sim.switch_hits("left_flipper").unwrap() >= 25);
    assert!(sim.switch_hits("left_eos").unwrap() >= 25, "flippers fired");
    assert!(
        sim.switch_hits("right_eos").unwrap() >= 20,
        "flippers fired"
    );
    assert!(sim.score(0).unwrap() > 0, "the script scored");
    assert!(sim.counters().balls_drained >= 1, "balls moved and drained");
    assert!(!sim.script_faulted());

    let bytes = rec.to_bytes();
    let back = Recording::from_bytes(&bytes).unwrap();
    assert_eq!(back, rec);

    let bundle = fixture();
    let hashes = on_sim_thread(move || {
        (0..10)
            .map(|_| replay(&bundle, &back).unwrap())
            .collect::<Vec<u64>>()
    });
    for (i, h) in hashes.iter().enumerate() {
        assert_eq!(
            *h, recorded_hash,
            "replay {i}: {h:016x} != {recorded_hash:016x}"
        );
    }
}

#[test]
fn seed_and_inputs_change_the_hash() {
    let bundle = fixture();
    on_sim_thread(move || {
        let load = |seed| {
            Simulation::load(
                &bundle,
                SimConfig {
                    seed,
                    ..SimConfig::default()
                },
            )
            .unwrap()
        };
        let (a, _) = record_pattern(load(1), 1_500, false);
        let (b, _) = record_pattern(load(1), 1_500, false);
        let (c, _) = record_pattern(load(1), 1_500, true);
        let (d, _) = record_pattern(load(2), 1_500, false);
        assert_eq!(a.state_hash(), b.state_hash());
        assert_ne!(a.state_hash(), c.state_hash(), "inputs matter");
        assert_ne!(a.state_hash(), d.state_hash(), "the seed matters");
    });
}

#[test]
fn replay_refuses_another_table() {
    let (_, rec, _) = record(7);
    let mut short = rec.clone();
    short.ticks = 200;
    short.events.retain(|(t, _)| *t <= 200);
    // Same table, another file content: a different table hash.
    let other = fixture_variant("other_table", &[("rules.lua", Some(b"-- other\n"))]);
    let err = on_sim_thread(move || replay(&other, &short).unwrap_err());
    match err {
        ReplayError::TableMismatch { recorded, bundle } => {
            assert_eq!(recorded, rec.table_hash);
            assert_ne!(bundle, recorded);
        }
        e => panic!("expected a table mismatch, got {e}"),
    }
    let missing = std::env::temp_dir().join("pin_sim_2_5_no_bundle_here");
    let e = replay(&missing, &rec).unwrap_err();
    assert!(matches!(e, ReplayError::Sim(_)));
    assert!(!e.to_string().is_empty());
}

#[test]
fn recorder_play_continues_a_loaded_sim() {
    let bundle = fixture();
    on_sim_thread(move || {
        let sim = Simulation::load(&bundle, SimConfig::default()).unwrap();
        let (sim, rec) = record_pattern(sim, 800, false);
        let mut fresh = Simulation::load(&bundle, SimConfig::default()).unwrap();
        assert_eq!(Recorder::play(&mut fresh, &rec), sim.state_hash());
        assert_eq!(fresh.tick(), 800);
        let recorder = Recorder::new(fresh);
        assert_eq!(recorder.recording().ticks, 0);
        assert_eq!(recorder.sim().tick(), 800);
    });
}
