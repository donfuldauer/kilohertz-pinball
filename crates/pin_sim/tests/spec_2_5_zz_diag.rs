#[path = "fixtures/spec_2_5/support.rs"]
mod support;
use pin_core::sim::SimStep;
use pin_core::snapshot::FrameStateSnapshot;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use support::*;
struct Trap;
thread_local! { static ARMED: Cell<bool> = const { Cell::new(false) }; }
fn hit(size: usize) {
    if ARMED.try_with(|c| c.replace(false)).unwrap_or(false) {
        let bt = std::backtrace::Backtrace::force_capture();
        eprintln!("ALLOC of {size} bytes:\n{bt}");
        let _ = ARMED.try_with(|c| c.set(true));
    }
}
unsafe impl GlobalAlloc for Trap {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 { hit(l.size()); unsafe { System.alloc(l) } }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 { hit(l.size()); unsafe { System.alloc_zeroed(l) } }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 { hit(n); unsafe { System.realloc(p, l, n) } }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { unsafe { System.dealloc(p, l) } }
}
#[global_allocator]
static G: Trap = Trap;

#[test]
fn diag() {
    on_sim_thread(|| {
        let dir = bundle_with_script("diag", "-- no callbacks\n");
        let mut sim = load_at(&dir, DEFAULT_SEED);
        let mut out = Box::new(FrameStateSnapshot::default());
        for tick in 1..=3000u64 {
            ARMED.with(|c| c.set(true));
            sim.step_1ms(tick, &[], &mut out);
            ARMED.with(|c| c.set(false));
        }
        eprintln!("--- fixture lua");
        let mut sim = load();
        let ev = input_script(3000, 11);
        let mut i = 0;
        let mut buf = Vec::with_capacity(8);
        for tick in 1..=3000u64 {
            buf.clear();
            while i < ev.len() && ev[i].0 == tick { buf.push(ev[i].1); i += 1; }
            ARMED.with(|c| c.set(true));
            sim.step_1ms(tick, &buf, &mut out);
            ARMED.with(|c| c.set(false));
        }
        eprintln!("--- eos: {:?} {:?}", sim.switch_closed("lower_left_eos"), sim.switch_hits("lower_left_eos"));
        let s = load();
        eprintln!("--- eos at load: {:?}", s.switch_closed("lower_left_eos"));
    });
}
