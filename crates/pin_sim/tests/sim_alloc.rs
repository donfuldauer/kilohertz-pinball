//! Subphase 2.5 verification 2 (gate step 6): the allocation guard passes
//! over 60 000 ticks with a script loaded (Lua, and the Wasm fixture).

mod sim_support;

use pin_core::input::InputEvent;
use pin_core::sim::SimStep;
use pin_core::snapshot::FrameStateSnapshot;
use pin_sim::alloc_guard::{CountingAlloc, assert_no_alloc, count_allocs, is_installed};
use pin_sim::cli::pattern_events;
use pin_sim::{SimConfig, Simulation};
use sim_support::*;
use std::path::PathBuf;

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

const GUARDED_TICKS: u64 = 60_000;
const WARM_UP_TICKS: u64 = 3_000;

/// Inputs for `tick`, built outside the guard.
fn inputs(tick: u64, buf: &mut Vec<InputEvent>) {
    buf.clear();
    pattern_events(tick, buf);
}

fn guarded_run(bundle: PathBuf) -> (u64, u32, u32) {
    on_sim_thread(move || {
        let mut sim = Simulation::load(&bundle, SimConfig::default()).unwrap();
        let mut out = FrameStateSnapshot::default();
        let mut buf = Vec::with_capacity(16);
        for _ in 0..WARM_UP_TICKS {
            let t = sim.tick() + 1;
            inputs(t, &mut buf);
            sim.step_1ms(t, &buf, &mut out);
        }
        let mut total = 0u64;
        for _ in 0..GUARDED_TICKS {
            let t = sim.tick() + 1;
            inputs(t, &mut buf);
            let ((), n) = count_allocs(|| sim.step_1ms(t, &buf, &mut out));
            assert_eq!(n, 0, "tick {t}: {n} allocation(s) in step_1ms");
            total += n;
        }
        assert!(!sim.script_faulted(), "{:?}", sim.script_error());
        let hits = sim.switch_hits("bumper_ring").unwrap() + sim.switch_hits("trough").unwrap();
        (total, hits, sim.counters().balls_drained)
    })
}

#[test]
fn counting_allocator_is_installed_and_detects_allocation() {
    assert!(is_installed());
    let (v, n) = count_allocs(|| std::hint::black_box(vec![1u8; 32]));
    assert!(n >= 1);
    drop(v);
    let r = std::panic::catch_unwind(|| assert_no_alloc("probe", || Box::new(1u32)));
    assert!(r.is_err(), "assert_no_alloc must panic on an allocation");
    assert_eq!(assert_no_alloc("no-op", || 7), 7);
}

#[test]
fn sixty_thousand_lua_ticks_allocate_nothing() {
    let (allocs, hits, drained) = guarded_run(fixture());
    assert_eq!(allocs, 0);
    // The run did something: balls hit the bumper or drained.
    assert!(hits > 0 && drained > 0, "hits {hits}, drained {drained}");
}

#[test]
fn sixty_thousand_wasm_ticks_allocate_nothing() {
    let (allocs, hits, drained) = guarded_run(wasm_fixture("alloc_wasm"));
    assert_eq!(allocs, 0);
    assert!(hits > 0 && drained > 0, "hits {hits}, drained {drained}");
}
