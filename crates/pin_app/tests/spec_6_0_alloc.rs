//! Subphase 6.0 spec test (Independent Tester): the real simulation stepped
//! through `pin_app::orchestrator::SimLoop` makes no heap allocation per tick
//! once warmed up (PLAN §2.3 rule 4, D5; gate step 6). The guard is
//! `pin_sim::alloc_guard`; allocations of the Lua VM's own arena are exempt
//! by that guard's rules (counted separately).

mod support;

#[path = "fixtures/spec_6_0/common.rs"]
mod common;

use common::{fixture, greybox_bundle};
use pin_app::orchestrator::{SimLoop, load_validated};
use pin_core::input::{CabinetInput, InputEvent};
use pin_core::sim::TICK_NS;
use pin_core::snapshot::FrameStateSnapshot;
use pin_core::spsc;
use pin_core::triple_buffer::TripleBuffer;
use pin_sim::SimConfig;
use pin_sim::alloc_guard::{CountingAlloc, assert_no_alloc, count_allocs, is_installed};
use std::path::Path;

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

/// Loads `bundle`, warms the loop up for `warm` ticks with a game started
/// (start pressed, launch, flippers), then checks `measured` more ticks,
/// with flipper inputs still flowing, one tick at a time.
fn check(bundle: &Path, warm: u64, measured: u64) {
    assert!(is_installed(), "CountingAlloc is the global allocator");
    let sim = load_validated(bundle, SimConfig::default()).expect("bundle loads validated");
    let (mut input, consumer) = spsc::channel::<InputEvent>(4096);
    let (writer, mut reader) = TripleBuffer::new(FrameStateSnapshot::default());
    let mut sim_loop = SimLoop::new(sim, consumer, writer);

    let mut push = |tick: u64, i: CabinetInput, p: bool| {
        input
            .push(InputEvent::button(tick * TICK_NS - TICK_NS / 2, i, p))
            .expect("input ring has room");
    };
    push(50, CabinetInput::Start, true);
    push(120, CabinetInput::Start, false);
    push(600, CabinetInput::Launch, true);
    push(700, CabinetInput::Launch, false);
    let total = warm + measured;
    let mut k = 900;
    let mut left = true;
    while k + 100 < total {
        let i = if left {
            CabinetInput::LeftFlipper
        } else {
            CabinetInput::RightFlipper
        };
        push(k, i, true);
        push(k + 80, i, false);
        k += 250;
        left = !left;
    }

    sim_loop.run_ticks(u32::try_from(warm).expect("warm-up fits u32"));
    let mut worst = 0u64;
    for _ in 0..measured {
        let ((), n) = count_allocs(|| sim_loop.run_ticks(1));
        worst = worst.max(n);
    }
    assert_eq!(worst, 0, "a warmed-up tick allocated {worst} times");
    // And as one guarded block.
    assert_no_alloc("SimLoop::run_ticks with the real Simulation", || {
        sim_loop.run_ticks(500);
    });
    let snap = reader.read();
    assert_eq!(snap.tick, total + 500, "the loop published every tick");
}

#[test]
fn fixture_table_ticks_do_not_allocate() {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(|| check(&fixture("fits"), 3_000, 3_000))
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
}

#[test]
fn greybox_ticks_do_not_allocate() {
    std::thread::Builder::new()
        .stack_size(64 << 20)
        .spawn(|| check(greybox_bundle(), 5_000, 5_000))
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e));
}
