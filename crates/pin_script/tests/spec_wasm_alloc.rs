//! Spec (4.5 criterion 4, gate step 6): the host side of guest calls performs
//! zero heap allocations per callback after warm-up, for a guest that calls
//! EVERY import from both `on_tick` and `on_switch_edge`. A counting global
//! allocator observes everything on the calling thread (wasmtime included).

#[path = "spec_fixtures/wasm/support.rs"]
mod support;

use pin_core::{HostCommandQueue, HostCtx, HostView, Pcg32, SeedableRng, SwitchEdge, SwitchId};
use pin_core::TableScriptBackend;
use pin_script::wasm::{WasmConfig, WasmScriptBackend};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use support::{module, names};

struct Counting;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

fn bump() {
    let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
}

// SAFETY: forwards to the system allocator, only adding a thread-local counter.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn allocs() -> u64 {
    ALLOCS.with(Cell::get)
}

#[test]
fn counter_detects_allocation() {
    let before = allocs();
    let v = std::hint::black_box(vec![1u8; 64]);
    assert!(allocs() > before);
    drop(v);
}

/// Calls every import with valid arguments. `$all` is used by both callbacks.
fn every_import_guest() -> String {
    module(
        r#"(memory (export "memory") 1)
           (data (i32.const 64) "trough_eject")
           (data (i32.const 96) "no_such_name")
           (global $n (mut i64) (i64.const 0))
           (func $all (param $x i32) (local $r i64) (local $id i32)
             (global.set $n (i64.add (global.get $n) (i64.const 1)))
             (local.set $id (call $id_of (i32.const 1) (i32.const 64) (i32.const 12)))
             (drop (call $id_of (i32.const 2) (i32.const 96) (i32.const 12)))
             (call $pulse_coil (local.get $id) (i32.const 20))
             (call $hold_coil (i32.const 2) (i32.wrap_i64 (i64.rem_u (global.get $n) (i64.const 2))))
             (call $set_magnet (i32.const 4) (f32.const 0.5))
             (call $set_zipper (i32.const 6) (i32.and (local.get $x) (i32.const 1)))
             (call $set_diverter (i32.const 5) (i32.const 1))
             (call $spawn_ball (i32.const 9) (i32.const 11))
             (call $set_lamp (i32.wrap_i64 (i64.rem_u (global.get $n) (i64.const 8))) (f32.const 1))
             (call $set_gi_tint (f32.const 1) (f32.const 0.5) (f32.const 0.25) (f32.const 1))
             (local.set $r (call $random (i64.const 1) (i64.const 6)))
             (local.set $r (i64.add (local.get $r) (call $random (i64.const -1000) (i64.const 1000))))
             (call $set_score (i32.const 1) (i64.add (global.get $n) (local.get $r)))
             (call $show_scene (i32.const 14))
             (call $play_sound (i32.const 12) (f32.const 0.75))
             (call $persist_set (i32.const 16)
               (i64.add (call $persist_get (i32.const 16))
                 (i64.add (call $tick) (i64.extend_i32_u (call $balls_in_play)))))
             (if (call $switch (i32.const 3)) (then (call $pulse_coil (i32.const 31) (i32.const 5))))
             (i64.store (i32.const 1024) (global.get $n)))
           (func (export "on_init") (call $all (i32.const 0)))
           (func (export "on_tick") (call $all (i32.const 1)))
           (func (export "on_switch_edge") (param $id i32) (param $closed i32)
             (call $all (i32.add (local.get $id) (local.get $closed))))"#,
    )
}

struct World {
    view: HostView,
    rng: Pcg32,
    cmd: HostCommandQueue,
}

fn run(b: &mut WasmScriptBackend, w: &mut World, ticks: u64) {
    for t in 0..ticks {
        w.view.tick += 1;
        w.view.set_switch(SwitchId(3), t % 3 == 0);
        w.view.balls_in_play = (t % 4) as u8;
        w.cmd.clear();
        let mut ctx = HostCtx {
            view: &w.view,
            rng: &mut w.rng,
            cmd: &mut w.cmd,
        };
        let edge = if t % 2 == 0 {
            SwitchEdge::Closed
        } else {
            SwitchEdge::Opened
        };
        b.on_switch_edge(SwitchId((t % 300) as u16), edge, &mut ctx);
        b.on_tick(&mut ctx);
    }
}

fn world() -> World {
    World {
        view: HostView::default(),
        rng: Pcg32::seed_from_u64(7),
        cmd: HostCommandQueue::default(),
    }
}

#[test]
fn steady_state_callbacks_calling_every_import_do_not_allocate() {
    for cfg in [WasmConfig::default(), WasmConfig::engine()] {
        let mut b = WasmScriptBackend::new(every_import_guest().as_bytes(), &names(), cfg)
            .unwrap_or_else(|e| panic!("fixture failed to load: {e}"));
        let mut w = world();
        b.on_init(&mut HostCtx {
            view: &w.view,
            rng: &mut w.rng,
            cmd: &mut w.cmd,
        });
        assert_eq!(w.cmd.coil_pulses.len(), 1, "on_init ran every import");
        run(&mut b, &mut w, 500); // warm-up
        assert!(!b.is_faulted(), "{:?}", b.last_error());
        // The guest really does something each tick.
        assert_eq!(w.cmd.scores.len(), 2);
        assert_eq!(w.cmd.sounds.len(), 2);
        assert_eq!(w.cmd.persist_writes.len(), 2);
        assert_eq!(w.cmd.coil_pulses[0].0, pin_core::CoilId(1), "id_of resolved trough_eject");

        let before = allocs();
        run(&mut b, &mut w, 5_000);
        let n = allocs() - before;
        assert_eq!(n, 0, "{n} heap allocations in 5000 steady-state ticks ({cfg:?})");
        assert!(!b.is_faulted());
        assert_eq!(b.overflow_count(), 0);
    }
}

#[test]
fn queue_overflow_and_accessors_do_not_allocate() {
    // 40 pulses per callback: 8 dropped and counted every time.
    let src = module(
        r#"(memory (export "memory") 1)
           (func $burst (local $i i32)
             (loop $l
               (call $pulse_coil (i32.const 2) (local.get $i))
               (local.set $i (i32.add (local.get $i) (i32.const 1)))
               (br_if $l (i32.lt_u (local.get $i) (i32.const 40)))))
           (func (export "on_init"))
           (func (export "on_tick") (call $burst))
           (func (export "on_switch_edge") (param i32 i32))"#,
    );
    let mut b = WasmScriptBackend::new(src.as_bytes(), &names(), WasmConfig::engine()).unwrap();
    let mut w = world();
    run(&mut b, &mut w, 200);
    let before = allocs();
    run(&mut b, &mut w, 2_000);
    let faulted = b.is_faulted();
    let stats = (b.fault_count(), b.overflow_count(), b.memory_size(), b.config());
    let none = b.last_error().is_none();
    let n = allocs() - before;
    assert_eq!(n, 0, "{n} heap allocations with a full queue");
    assert!(!faulted && none);
    assert_eq!(stats.1, 2_200 * 8);
    assert_eq!(w.cmd.coil_pulses.len(), 32);
}

#[test]
fn a_tilted_backend_does_not_allocate_per_callback() {
    // After the fault itself (which may allocate, as in Lua), the no-op
    // callbacks of a tilted script are allocation-free.
    let src = module(
        r#"(memory (export "memory") 1)
           (func (export "on_init"))
           (func (export "on_tick") (call $hold_coil (i32.const 3) (i32.const 1)) (unreachable))
           (func (export "on_switch_edge") (param i32 i32))"#,
    );
    let mut b = WasmScriptBackend::new(src.as_bytes(), &names(), WasmConfig::engine()).unwrap();
    let mut w = world();
    run(&mut b, &mut w, 10);
    assert!(b.is_faulted());
    let before = allocs();
    run(&mut b, &mut w, 2_000);
    let n = allocs() - before;
    assert_eq!(n, 0, "{n} heap allocations in tilted callbacks");
    assert!(w.cmd.is_empty());
    assert_eq!(b.fault_count(), 1);
}
