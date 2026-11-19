//! Subphase 6.1 spec tests (Independent Tester), S4: deterministic replay
//! (brief item 6). A recorded cabinet input stream (start, launch, lower
//! and upper flippers, action) on the Apex Circuit bundle replays 10 times
//! to an identical `Simulation::state_hash` (`pin_sim::replay`), and the
//! same stream played through `Recorder::play` on fresh loads agrees.

#[path = "fixtures/spec_6_1/common.rs"]
mod common;

use common::*;
use pin_core::input::{CabinetInput, InputEvent};
use pin_core::sim::TICK_NS;
use pin_sim::replay::Recorder;
use pin_sim::{SimConfig, Simulation};

/// The scripted input schedule: (tick, input, pressed), tick-sorted.
fn schedule(ticks: u64) -> Vec<(u64, CabinetInput, bool)> {
    let mut v = vec![
        (100, CabinetInput::Start, true),
        (180, CabinetInput::Start, false),
        (1_500, CabinetInput::Launch, true),
        (1_600, CabinetInput::Launch, false),
    ];
    let mut k = 2_000;
    let mut n = 0u32;
    while k < ticks - 500 {
        let i = match n % 3 {
            0 => CabinetInput::LeftFlipper,
            1 => CabinetInput::RightFlipper,
            _ => CabinetInput::LeftUpper,
        };
        v.push((k, i, true));
        v.push((k + 120, i, false));
        if n % 4 == 1 {
            v.push((k + 40, CabinetInput::RightUpper, true));
            v.push((k + 90, CabinetInput::RightUpper, false));
        }
        if n % 7 == 3 {
            v.push((k + 200, CabinetInput::Action, true));
            v.push((k + 260, CabinetInput::Action, false));
        }
        if n % 11 == 5 {
            v.push((k + 150, CabinetInput::Launch, true));
            v.push((k + 230, CabinetInput::Launch, false));
        }
        k += 377;
        n += 1;
    }
    v.sort_by_key(|e| e.0);
    v
}

#[test]
fn recorded_inputs_replay_identically_10_times() {
    big_stack(|| {
        let ticks = 30_000u64;
        let sim = Simulation::load(bundle(), SimConfig::default()).expect("loads");
        let mut rec = Recorder::new(sim);
        let sched = schedule(ticks);
        let mut next = 0;
        let mut evs: Vec<InputEvent> = Vec::new();
        for tick in 1..=ticks {
            evs.clear();
            while next < sched.len() && sched[next].0 == tick {
                let (_, i, p) = sched[next];
                evs.push(InputEvent::button(tick * TICK_NS - TICK_NS / 2, i, p));
                next += 1;
            }
            rec.step(&evs);
        }
        let (sim, recording) = rec.finish();
        for (name, min) in [
            ("start", 1),
            ("launch", 1),
            ("left_flipper", 1),
            ("right_flipper", 1),
            ("left_upper", 1),
            ("right_upper", 1),
            ("action", 1),
        ] {
            assert!(
                sim.switch_hits(name).expect(name) >= min,
                "the stream pressed {name}"
            );
        }
        assert!(
            sim.score(0).is_some(),
            "a game was started and reported a score"
        );
        assert_healthy(&sim, "recorded run");
        let expected = sim.state_hash();
        for run in 0..10 {
            let got = pin_sim::replay::replay(bundle(), &recording).expect("replays");
            assert_eq!(got, expected, "run {run}: replay diverged");
        }
        for run in 0..2 {
            let mut s = Simulation::load(bundle(), SimConfig::default()).expect("loads");
            assert_eq!(
                Recorder::play(&mut s, &recording),
                expected,
                "play run {run}"
            );
        }
    });
}
