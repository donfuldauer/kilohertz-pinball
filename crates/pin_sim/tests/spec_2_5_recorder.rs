//! Subphase 2.5 spec tests (Independent Tester): `replay::Recorder`.
//!
//! Pinned by the brief: "struct Recorder — wraps a Simulation;
//! fn step(&mut self, inputs: &[InputEvent]) records then steps".
//! NOT pinned: how a Recorder is built and how the Recording is taken out.
//! This file assumes `Recorder::new(Simulation)` and an accessor
//! `Recorder::recording()` (the spelling the compiler reported on first
//! compile; taken by value or by reference, read here through `to_bytes`).
//! It is kept apart from the other spec files so that a different spelling
//! fails only this file.

#[path = "fixtures/spec_2_5/support.rs"]
mod support;

use pin_core::input::InputEvent;
use pin_sim::replay::{Recorder, Recording, replay};
use support::*;

/// Feeds `events` through a Recorder for `ticks` ticks.
fn record(seed: u64, ticks: u64, events: &[(u64, InputEvent)]) -> Recording {
    taken(&recorder(seed, ticks, events))
}

/// A Recorder that has been fed `events` for `ticks` ticks.
fn recorder(seed: u64, ticks: u64, events: &[(u64, InputEvent)]) -> Recorder {
    let mut rec = Recorder::new(load_at(&bundle(), seed));
    let mut buf: Vec<InputEvent> = Vec::new();
    let mut i = 0;
    for tick in 1..=ticks {
        buf.clear();
        while i < events.len() && events[i].0 == tick {
            buf.push(events[i].1);
            i += 1;
        }
        rec.step(&buf);
    }
    rec
}

/// The recording so far, independent of whether `recording()` returns a
/// value or a reference.
fn taken(rec: &Recorder) -> Recording {
    Recording::from_bytes(&rec.recording().to_bytes()).expect("recorder output parses")
}

#[test]
fn recorder_records_seed_table_hash_ticks_and_events() {
    on_sim_thread(|| {
        const TICKS: u64 = 3000;
        let events = input_script(TICKS, 8);
        let rec = record(77, TICKS, &events);
        assert_eq!(rec.seed, 77);
        assert_eq!(rec.ticks, TICKS);
        assert_eq!(rec.table_hash, load().table_hash());
        assert_eq!(rec.events.len(), events.len());
        for (i, (got, want)) in rec.events.iter().zip(&events).enumerate() {
            assert_eq!(
                got.0, want.0,
                "event {i}: recorded at the tick it was stepped in"
            );
            assert_eq!(got.1, want.1, "event {i}");
        }
    });
}

#[test]
fn recorded_run_replays_to_the_hash_of_the_same_run_without_a_recorder() {
    on_sim_thread(|| {
        const TICKS: u64 = 10_000;
        let events = input_script(TICKS, 21);
        // Recording does not change the simulation.
        let live = drive(&bundle(), 5, TICKS, &events);
        assert!(live.score(P_SCORE).unwrap() > 0);
        let recorder = recorder(5, TICKS, &events);
        // The Recorder's own simulation matches the live run.
        assert_eq!(recorder.sim().tick(), live.tick());
        assert_eq!(recorder.sim().state_hash(), live.state_hash());
        let rec = taken(&recorder);
        let bytes = rec.to_bytes();
        for _ in 0..10 {
            assert_eq!(replay(&bundle(), &rec).unwrap(), live.state_hash());
        }
        let back = Recording::from_bytes(&bytes).unwrap();
        assert_eq!(replay(&bundle(), &back).unwrap(), live.state_hash());
    });
}

#[test]
fn recorder_with_no_steps_is_an_empty_recording() {
    on_sim_thread(|| {
        let rec = taken(&Recorder::new(load_at(&bundle(), 3)));
        assert_eq!((rec.seed, rec.ticks, rec.events.len()), (3, 0, 0));
        assert_eq!(
            replay(&bundle(), &rec).unwrap(),
            load_at(&bundle(), 3).state_hash()
        );
    });
}
