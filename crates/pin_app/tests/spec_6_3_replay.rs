//! Subphase 6.3 spec tests (Independent Tester): the Wasm rules replay
//! through the recording byte format (brief requirement 4) and never tilt
//! (requirement 5).
//!
//! For each table and each recorded cabinet game (fixed rhythm, seeded PCG
//! stream; cabinet inputs only, since a byte recording holds input events
//! and not test switch injections), the live Wasm run is recorded with
//! `Recorder`, the recording goes to bytes and back, and `replay()` on the
//! Wasm bundle reaches the live run's `state_hash` 3 times out of 3. The
//! live Lua run of the same game reaches the same hash, and the recording
//! is bound to the Wasm bundle (the Lua bundle is a table mismatch).

mod spec_6_3_support;

use pin_core::{InputEvent, SimStep};
use pin_sim::replay::{Recorder, Recording, ReplayError, replay};
use spec_6_3_support::*;

const TICKS: u64 = 40_000;

fn replay_case(t: Table, seed: u64, inputs: Inputs) {
    on_big_stack(move || {
        let ctx = format!("{t:?} seed {seed:#x} {inputs:?}");
        let wdir = wasm_bundle(t);
        let ldir = lua_bundle(t);
        let mut rec = Recorder::new(load(wdir, seed));
        let mut lua = load(ldir, seed);
        let mut igen = InputGen::new(&inputs);
        let mut ev: Vec<InputEvent> = Vec::new();
        let mut out = Box::new(*lua.snapshot());
        for tick in 1..=TICKS {
            igen.events(tick, &mut ev);
            rec.step(&ev);
            lua.step_1ms(tick, &ev, &mut out);
            assert_script_healthy(rec.sim(), &format!("{ctx} wasm tick {tick}"));
        }
        let (sim, recording) = rec.finish();
        assert_script_healthy(&sim, &ctx);
        assert_script_healthy(&lua, &ctx);
        assert!(
            sim.score(0).is_some_and(|s| s > 0),
            "{ctx}: the Wasm game scored"
        );
        let live = sim.state_hash();
        assert_eq!(lua.state_hash(), live, "{ctx}: live Lua vs live Wasm");
        assert_eq!(lua.score(0), sim.score(0), "{ctx}: scores");
        assert_eq!(recording.table_hash, sim.table_hash(), "{ctx}");
        let bytes = recording.to_bytes();
        let back = Recording::from_bytes(&bytes).expect("recording parses");
        assert_eq!(back.to_bytes(), bytes, "{ctx}: byte round trip");
        for run in 0..3 {
            assert_eq!(
                replay(wdir, &back).unwrap_or_else(|e| panic!("{ctx}: replay: {e}")),
                live,
                "{ctx}: replay {run} diverged"
            );
        }
        assert!(
            matches!(replay(ldir, &back), Err(ReplayError::TableMismatch { .. })),
            "{ctx}: the recording is bound to the Wasm bundle"
        );
    });
}

#[test]
fn apex_wasm_rhythm_game_replays_from_bytes() {
    replay_case(Table::Apex, 3, Inputs::Rhythm);
}

#[test]
fn apex_wasm_random_game_replays_from_bytes() {
    replay_case(Table::Apex, 0x1234, Inputs::Random(0x77));
}

#[test]
fn sky_deck_wasm_rhythm_game_replays_from_bytes() {
    replay_case(Table::SkyDeck, 5, Inputs::Rhythm);
}

#[test]
fn sky_deck_wasm_random_game_replays_from_bytes() {
    replay_case(Table::SkyDeck, 0x4321, Inputs::Random(0x78));
}

#[test]
fn feature_lab_wasm_rhythm_game_replays_from_bytes() {
    replay_case(Table::FeatureLab, 9, Inputs::Rhythm);
}

#[test]
fn feature_lab_wasm_random_game_replays_from_bytes() {
    replay_case(Table::FeatureLab, 0xabcd, Inputs::Random(0x79));
}
