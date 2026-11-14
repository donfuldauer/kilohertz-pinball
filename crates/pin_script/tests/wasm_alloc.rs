//! Subphase 4.5 verification criterion 4 (gate step 6): steady-state
//! `on_tick` / `on_switch_edge` of a guest that calls every `pinball` import
//! perform zero heap allocations on the host side (wasmtime included).

use pin_core::ids::SwitchId;
use pin_core::{
    HostCommandQueue, HostCtx, HostView, Pcg32, SeedableRng, SwitchEdge, TableScriptBackend,
};
use pin_script::NameTables;
use pin_script::wasm::{WasmConfig, WasmScriptBackend};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

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

const GUEST: &str = include_str!("fixtures/wasm/all_imports.wat");

fn backend(cfg: WasmConfig) -> WasmScriptBackend {
    let names = NameTables {
        coils: vec![("kick".into(), pin_core::CoilId(5))],
        ..NameTables::default()
    };
    WasmScriptBackend::new(GUEST.as_bytes(), &names, cfg).unwrap()
}

fn steady_state(cfg: WasmConfig) {
    let mut b = backend(cfg);
    let mut view = HostView::default();
    let mut rng = Pcg32::seed_from_u64(7);
    let mut cmd = HostCommandQueue::default();
    b.on_init(&mut HostCtx {
        view: &view,
        rng: &mut rng,
        cmd: &mut cmd,
    });
    let mut run = |view: &mut HostView, ticks: u64| {
        for t in 0..ticks {
            view.tick += 1;
            view.set_switch(SwitchId(3), t % 3 == 0);
            cmd.clear();
            let mut ctx = HostCtx {
                view,
                rng: &mut rng,
                cmd: &mut cmd,
            };
            b.on_tick(&mut ctx);
            let edge = if t % 2 == 0 {
                SwitchEdge::Closed
            } else {
                SwitchEdge::Opened
            };
            b.on_switch_edge(SwitchId((t % 16) as u16), edge, &mut ctx);
            assert_eq!(ctx.cmd.coil_pulses[0], (pin_core::CoilId(5), 20));
        }
    };
    run(&mut view, 200);
    let before = allocs();
    run(&mut view, 10_000);
    let n = allocs() - before;
    assert_eq!(n, 0, "{n} allocations in 10k steady-state ticks");
    assert!(!b.is_faulted(), "{:?}", b.last_error());
}

#[test]
fn steady_state_callbacks_do_not_allocate() {
    steady_state(WasmConfig::default());
}

#[test]
fn steady_state_callbacks_do_not_allocate_with_engine_limits() {
    steady_state(WasmConfig::engine());
}
