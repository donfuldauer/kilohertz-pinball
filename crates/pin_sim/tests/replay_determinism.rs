//! Subphase 2.5 verification 1: a 10 000-tick recording (balls moving,
//! flippers firing from recorded inputs, a Lua script scoring) replays to an
//! identical state hash 10 times in a row; a recording of another table is
//! refused.

mod sim_support;

use pin_core::input::{CabinetAxis, InputEvent};
use pin_core::sim::SimStep;
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
    // A different bundle whose script would not even load is still
    // reported as the wrong table (the hash is checked before loading).
    let broken = fixture_variant(
        "broken_table",
        &[("rules.lua", Some(b"this is not lua (\n"))],
    );
    let (rec2, b2) = (rec.clone(), broken.to_path_buf());
    let err = on_sim_thread(move || replay(&b2, &rec2).unwrap_err());
    assert!(
        matches!(err, ReplayError::TableMismatch { .. }),
        "expected a table mismatch, got {err}"
    );
    let direct = on_sim_thread(move || {
        Simulation::load(&broken, SimConfig::default())
            .err()
            .map(|e| e.to_string())
    });
    assert!(direct.is_some(), "the broken bundle must not load");
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
        // A recording replays from load, so it cannot start mid-run.
        let started = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
            let _ = Recorder::new(fresh);
        }));
        assert!(started.is_err(), "Recorder::new accepted a sim at tick 800");
        let at_load = Recorder::new(Simulation::load(&bundle, SimConfig::default()).unwrap());
        assert_eq!(at_load.recording().ticks, 0);
    });
}

/// A recording stores only the seed and replays with the default script
/// limits, so recording a simulation with other limits is refused instead of
/// producing a file that replays differently.
#[test]
fn recorder_refuses_non_default_script_limits() {
    let bundle = fixture();
    on_sim_thread(move || {
        let defaults = SimConfig::default();
        let mut lua = defaults.lua;
        lua.instruction_budget /= 2;
        let mut wasm = defaults.wasm;
        wasm.fuel_per_callback /= 2;
        for cfg in [
            SimConfig {
                lua,
                ..SimConfig::default()
            },
            SimConfig {
                wasm,
                ..SimConfig::default()
            },
        ] {
            let sim = Simulation::load(&bundle, cfg).unwrap();
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(move || {
                let _ = record_pattern(sim, 10, false);
            }));
            assert!(r.is_err(), "recorded with non-default limits {cfg:?}");
        }
        let sim = Simulation::load(&bundle, SimConfig::default()).unwrap();
        let (_, rec) = record_pattern(sim, 10, false);
        assert_eq!(rec.ticks, 10);
    });
}

/// Non-finite axis inputs: the recorder stores what ingestion does with
/// them (NaN ignored, ±inf clamped to the axis range), so the saved file
/// parses back, replays to the recorded hash, and the recorded run matches a
/// plain run fed the raw values.
#[test]
fn non_finite_axis_inputs_round_trip() {
    let bundle = fixture();
    on_sim_thread(move || {
        let ts = |t: u64| (t - 1) * 1_000_000 + 500_000;
        let raw = |t: u64| -> Vec<InputEvent> {
            match t {
                5 => vec![
                    InputEvent::axis(ts(5), CabinetAxis::Plunger, f32::INFINITY),
                    InputEvent::axis(ts(5), CabinetAxis::NudgeX, f32::NAN),
                ],
                6 => vec![
                    InputEvent::axis(ts(6), CabinetAxis::NudgeY, f32::NEG_INFINITY),
                    InputEvent::axis(ts(6), CabinetAxis::NudgeX, 0.5),
                ],
                7 => vec![
                    InputEvent::axis(ts(7), CabinetAxis::NudgeX, f32::NAN),
                    InputEvent::axis(ts(7), CabinetAxis::NudgeY, 0.0),
                ],
                9 => vec![InputEvent::axis(ts(9), CabinetAxis::NudgeX, -0.0)],
                40 => vec![InputEvent::axis(ts(40), CabinetAxis::Plunger, 0.0)],
                _ => Vec::new(),
            }
        };
        const T: u64 = 300;
        let mut rec = Recorder::new(Simulation::load(&bundle, SimConfig::default()).unwrap());
        for t in 1..=T {
            rec.step(&raw(t));
        }
        let (sim, rec) = rec.finish();

        let mut plain = Simulation::load(&bundle, SimConfig::default()).unwrap();
        let mut out = *plain.snapshot();
        for t in 1..=T {
            plain.step_1ms(t, &raw(t), &mut out);
        }
        assert_eq!(plain.state_hash(), sim.state_hash());

        let back = Recording::from_bytes(&rec.to_bytes()).expect("the recording parses back");
        assert_eq!(back, rec);
        let axes = |tick| -> Vec<(CabinetAxis, f32)> {
            rec.events_on(tick)
                .iter()
                .map(|(_, e)| match e.kind {
                    pin_core::input::InputEventKind::Axis { axis, value } => (axis, value),
                    pin_core::input::InputEventKind::Button { .. } => panic!("{e:?}"),
                })
                .collect()
        };
        assert_eq!(axes(5), [(CabinetAxis::Plunger, 1.0)]);
        assert_eq!(
            axes(6),
            [(CabinetAxis::NudgeY, -1.0), (CabinetAxis::NudgeX, 0.5)]
        );
        assert_eq!(axes(7), [(CabinetAxis::NudgeY, 0.0)]);

        assert_eq!(replay(&bundle, &back).unwrap(), sim.state_hash());
    });
}
