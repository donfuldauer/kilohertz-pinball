//! 6.2 T5 (D9): a recorded input stream of a scripted Sky Deck game
//! (start, launches, every flipper button including the upper-deck one,
//! Sky Bridge presses on `action`) replays 10 times to an identical
//! `state_hash`, through the replay file format; the same inputs played
//! twice give the same hash.

mod sky_deck_support;

use pin_core::{CabinetInput, InputEvent, TICK_NS};
use pin_sim::replay::{Recorder, Recording, replay};
use pin_sim::{SimConfig, Simulation};
use sky_deck_support::{healthy, key, on_big_stack, sky_deck_bundle};

/// The inputs of tick `t` of a played game.
fn played_inputs(t: u64, out: &mut Vec<InputEvent>) {
    out.clear();
    let ts = t * TICK_NS;
    let mut b = |i: CabinetInput, p: bool| out.push(InputEvent::button(ts, i, p));
    match t {
        10 => b(CabinetInput::Start, true),
        40 => b(CabinetInput::Start, false),
        _ => {}
    }
    // Launch every 2.5 s (only does something with a ball on the plunger).
    if t % 2500 == 1600 {
        b(CabinetInput::Launch, true);
    }
    if t % 2500 == 1630 {
        b(CabinetInput::Launch, false);
    }
    // The Sky Bridge button now and then.
    if t % 9000 == 4000 {
        b(CabinetInput::Action, true);
    }
    if t % 9000 == 4050 {
        b(CabinetInput::Action, false);
    }
    // Every flipper in a fixed rhythm.
    match t % 700 {
        100 => b(CabinetInput::LeftFlipper, true),
        180 => b(CabinetInput::LeftFlipper, false),
        350 => b(CabinetInput::RightFlipper, true),
        430 => b(CabinetInput::RightFlipper, false),
        500 => b(CabinetInput::RightUpper, true),
        560 => b(CabinetInput::RightUpper, false),
        _ => {}
    }
}

const PLAYED_TICKS: u64 = 40_000;

fn record(seed: u64) -> (Simulation, Recording) {
    let cfg = SimConfig {
        seed,
        ..SimConfig::default()
    };
    let mut rec = Recorder::new(Simulation::load(sky_deck_bundle(), cfg).unwrap());
    let mut buf = Vec::new();
    for t in 1..=PLAYED_TICKS {
        played_inputs(t, &mut buf);
        rec.step(&buf);
    }
    rec.finish()
}

#[test]
fn recorded_game_replays_to_the_same_state_hash_10_times() {
    let dir = sky_deck_bundle().to_path_buf();
    on_big_stack(move || {
        let (sim, recording) = record(7);
        healthy(&sim);
        assert!(sim.score(0).is_some_and(|s| s > 0), "the game scored");
        // In a game or past it (game over), not still in attract.
        assert!(
            matches!(sim.persist(key::STATE), 1 | 2),
            "a game was played"
        );
        let hash = sim.state_hash();
        // Through the file format too.
        let recording = Recording::from_bytes(&recording.to_bytes()).unwrap();
        for run in 0..10 {
            assert_eq!(
                replay(&dir, &recording).unwrap(),
                hash,
                "replay {run} diverged"
            );
        }
    });
}

#[test]
fn the_same_inputs_give_the_same_hash_and_seeds_differ() {
    on_big_stack(|| {
        let (a, _) = record(11);
        let (b, _) = record(11);
        assert_eq!(a.state_hash(), b.state_hash());
        let (c, _) = record(12);
        assert_ne!(a.state_hash(), c.state_hash(), "the seed is in the hash");
    });
}
