//! 6.4 R10: determinism and allocation on the Feature Lab.
//!
//! * A recorded input replay of a full game (start, launches, flipper
//!   rhythm on all three flipper buttons and the action button, until game
//!   over) replays 10 times to the same `Simulation::state_hash` (D9).
//! * The real sim stepped through `run_headless_with` / `SimLoop` performs
//!   no allocation per tick after warm-up outside the Lua VM's arena
//!   (`pin_sim::alloc_guard`, PLAN §2.3 rule 4).

mod feature_lab_support;

use feature_lab_support::*;
use pin_app::orchestrator::{SimLoop, TableSim, load_validated, run_headless_with};
use pin_core::spsc::channel;
use pin_core::{CabinetInput, FrameStateSnapshot, InputEvent, TICK_NS, TripleBuffer};
use pin_sim::alloc_guard::{CountingAlloc, count_allocs, count_allocs_split, is_installed};
use pin_sim::replay::{Recorder, Recording, replay};
use pin_sim::{SimConfig, Simulation};

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// The inputs of tick `t`: start, a launch every 2.5 s, a flipper rhythm
/// on the lower flippers and the backbox flipper, an action press every
/// 7 s.
fn inputs(t: u64, out: &mut Vec<InputEvent>) {
    out.clear();
    let ts = t * TICK_NS;
    let mut b = |i: CabinetInput, p: bool| out.push(InputEvent::button(ts, i, p));
    match t {
        10 => b(CabinetInput::Start, true),
        40 => b(CabinetInput::Start, false),
        _ => {}
    }
    match t % 2500 {
        1600 => b(CabinetInput::Launch, true),
        1630 => b(CabinetInput::Launch, false),
        _ => {}
    }
    match t % 7000 {
        3000 => b(CabinetInput::Action, true),
        3040 => b(CabinetInput::Action, false),
        _ => {}
    }
    match t % 700 {
        100 => b(CabinetInput::LeftFlipper, true),
        180 => b(CabinetInput::LeftFlipper, false),
        350 => b(CabinetInput::RightFlipper, true),
        430 => b(CabinetInput::RightFlipper, false),
        500 => b(CabinetInput::LeftUpper, true),
        560 => b(CabinetInput::LeftUpper, false),
        _ => {}
    }
}

/// Most ticks recorded (a game ends long before on this input pattern).
const CAP_TICKS: u64 = 600_000;

#[test]
fn a_recorded_full_game_replays_to_the_same_state_hash_10_times() {
    let dir = bundle().to_path_buf();
    on_big_stack(move || {
        let mut rec = Recorder::new(Simulation::load(&dir, SimConfig::default()).unwrap());
        let mut buf = Vec::new();
        let mut states = vec![0i64];
        let mut t = 0;
        while t < CAP_TICKS {
            t += 1;
            inputs(t, &mut buf);
            rec.step(&buf);
            let st = persist(rec.sim(), "game_state");
            if *states.last().unwrap() != st {
                states.push(st);
            }
            if st == 2 {
                break;
            }
        }
        let (sim, recording) = rec.finish();
        healthy(&sim);
        assert_eq!(states, [0, 1, 2], "a whole game was recorded");
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

#[test]
fn the_real_sim_does_not_allocate_per_tick() {
    assert!(is_installed());
    let (v, n) = count_allocs(|| std::hint::black_box(vec![7u8; 16]));
    assert!(n >= 1 && v.len() == 16, "guard not armed");
    let dir = bundle().to_path_buf();
    let allocs = on_big_stack(move || {
        // run_headless_with drives the real table sim.
        let sim = load_validated(&dir, SimConfig::default()).unwrap();
        assert_eq!(run_headless_with(TableSim::new(sim, None), 2000), Ok(2000));
        let sim = load_validated(&dir, SimConfig::default()).unwrap();
        let (mut tx, rx) = channel(1 << 16);
        let mut buf = Vec::new();
        for t in 1..=60_000u64 {
            inputs(t, &mut buf);
            for ev in &buf {
                tx.push(*ev).unwrap();
            }
        }
        let (w, mut r) = TripleBuffer::new(FrameStateSnapshot::default());
        let mut lp = SimLoop::new(TableSim::new(sim, None), rx, w);
        // Warm-up outside the guard (first Lua calls size the VM).
        lp.run_ticks(500);
        let mut allocs = 0;
        let mut exempt = 0;
        for _ in 0..595 {
            let ((), n, e) = count_allocs_split(|| lp.run_ticks(100));
            allocs += n;
            exempt += e;
        }
        assert_eq!(r.read().tick, 60_000);
        let sim = lp.sim().sim();
        healthy(sim);
        assert!(
            persist(sim, "game_state") == 1 || persist(sim, "games_played") >= 1,
            "a game was played"
        );
        eprintln!("Lua arena (exempt) allocations: {exempt}");
        allocs
    });
    assert_eq!(allocs, 0, "step_1ms allocated outside the Lua arena");
}
