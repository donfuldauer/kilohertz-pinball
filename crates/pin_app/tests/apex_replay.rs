//! 6.1 T4: a recorded input replay of a played Apex Circuit game (start,
//! launches, all three flipper buttons and the action button in a fixed
//! rhythm) reaches the same `state_hash` 10 out of 10 times, also through
//! the recording's byte format (D9).

mod apex_support;

use apex_support::{apex_bundle, on_big_stack};
use pin_core::{CabinetInput, InputEvent, TICK_NS};
use pin_sim::replay::{Recorder, Recording, replay};
use pin_sim::{SimConfig, Simulation};

/// The inputs of tick `t`.
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
    // Flippers in a fixed rhythm; the action button now and then.
    match t % 700 {
        100 => b(CabinetInput::LeftFlipper, true),
        180 => b(CabinetInput::LeftFlipper, false),
        350 => b(CabinetInput::RightFlipper, true),
        430 => b(CabinetInput::RightFlipper, false),
        500 => b(CabinetInput::LeftUpper, true),
        560 => b(CabinetInput::LeftUpper, false),
        _ => {}
    }
    match t % 9000 {
        4000 => b(CabinetInput::Action, true),
        4050 => b(CabinetInput::Action, false),
        _ => {}
    }
}

const PLAYED_TICKS: u64 = 40_000;

#[test]
fn recorded_game_replays_to_the_same_state_hash_10_times() {
    let dir = apex_bundle().to_path_buf();
    on_big_stack(move || {
        let mut rec = Recorder::new(Simulation::load(&dir, SimConfig::default()).unwrap());
        let mut buf = Vec::new();
        for t in 1..=PLAYED_TICKS {
            played_inputs(t, &mut buf);
            rec.step(&buf);
        }
        let (sim, recording) = rec.finish();
        assert!(!sim.script_faulted(), "{:?}", sim.script_error());
        assert_eq!(sim.script_queue_overflows(), 0);
        assert!(sim.score(0).is_some_and(|s| s > 0), "the game scored");
        let hash = sim.state_hash();
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
