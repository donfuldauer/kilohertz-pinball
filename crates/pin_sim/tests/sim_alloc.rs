//! Subphase 2.5 verification 2 (gate step 6): the allocation guard passes
//! over 60 000 ticks with a script loaded (Lua, and the Wasm fixture).
//!
//! Allocations the Lua VM makes inside the simulation's exempt scope (its
//! capped arena, PLAN.md §2.3 rule 4) are counted apart and reported; every
//! other allocation fails the run. Wasm calls are not exempt.

mod sim_support;

use pin_core::input::InputEvent;
use pin_core::sim::SimStep;
use pin_core::snapshot::FrameStateSnapshot;
use pin_sim::alloc_guard::{
    CountingAlloc, ExemptScope, assert_no_alloc, count_allocs, count_allocs_split, is_installed,
};
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

/// Returns (allocations, exempt Lua-arena allocations, hits, drained).
fn guarded_run(bundle: PathBuf) -> (u64, u64, u32, u32) {
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
        let mut exempt = 0u64;
        for _ in 0..GUARDED_TICKS {
            let t = sim.tick() + 1;
            inputs(t, &mut buf);
            let ((), n, e) = count_allocs_split(|| sim.step_1ms(t, &buf, &mut out));
            assert_eq!(n, 0, "tick {t}: {n} allocation(s) in step_1ms");
            total += n;
            exempt += e;
        }
        assert!(!sim.script_faulted(), "{:?}", sim.script_error());
        let hits = sim.switch_hits("bumper_ring").unwrap() + sim.switch_hits("trough").unwrap();
        (total, exempt, hits, sim.counters().balls_drained)
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
    // Exempt scopes nest and only hide what happens inside them.
    let (v, n, e) = count_allocs_split(|| {
        let a = {
            let _outer = ExemptScope::enter();
            let _inner = ExemptScope::enter();
            std::hint::black_box(vec![1u8; 8])
        };
        let b = std::hint::black_box(vec![2u8; 8]);
        (a, b)
    });
    assert_eq!((n, e), (1, 1));
    drop(v);
    assert_no_alloc("exempt", || {
        let _s = ExemptScope::enter();
        drop(std::hint::black_box(Box::new(1u32)));
    });
}

#[test]
fn sixty_thousand_lua_ticks_allocate_nothing() {
    let (allocs, exempt, hits, drained) = guarded_run(fixture());
    assert_eq!(allocs, 0);
    eprintln!("Lua-arena (exempt) allocations over {GUARDED_TICKS} ticks: {exempt}");
    // The arena regrows rarely (after a collection shrank it), not per tick.
    assert!(exempt <= GUARDED_TICKS / 100, "{exempt} exempt allocations");
    // The run did something: balls hit the bumper or drained.
    assert!(hits > 0 && drained > 0, "hits {hits}, drained {drained}");
}

#[test]
fn sixty_thousand_wasm_ticks_allocate_nothing() {
    let bundle = wasm_fixture("alloc_wasm");
    let (allocs, exempt, hits, drained) = guarded_run(bundle.to_path_buf());
    assert_eq!(allocs, 0);
    assert_eq!(exempt, 0, "Wasm calls are not exempt");
    assert!(hits > 0 && drained > 0, "hits {hits}, drained {drained}");
}
