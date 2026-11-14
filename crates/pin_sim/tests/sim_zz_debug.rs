mod sim_support;
use sim_support::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::{Cell, RefCell};
use pin_core::sim::SimStep;
struct A;
thread_local! { static ARM: Cell<bool> = const { Cell::new(false) }; static BT: RefCell<Option<String>> = const { RefCell::new(None) }; }
unsafe impl GlobalAlloc for A {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 { hit(); unsafe { System.alloc(l) } }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 { hit(); unsafe { System.realloc(p, l, n) } }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) { unsafe { System.dealloc(p, l) } }
}
fn hit() {
    let _ = ARM.try_with(|a| if a.get() { a.set(false); let s = format!("{}", std::backtrace::Backtrace::force_capture()); let _ = BT.try_with(|b| *b.borrow_mut() = Some(s)); });
}
#[global_allocator] static G: A = A;
#[test]
fn dbg() {
    on_sim_thread(|| {
        let mut sim = pin_sim::Simulation::load(&fixture(), Default::default()).unwrap();
        let mut out = *sim.snapshot();
        let mut buf = Vec::with_capacity(16);
        for _ in 0..8000 {
            let t = sim.tick() + 1; buf.clear(); pin_sim::cli::pattern_events(t, &mut buf);
            ARM.with(|a| a.set(t > 2000));
            sim.step_1ms(t, &buf, &mut out);
            ARM.with(|a| a.set(false));
            if let Some(b) = BT.with(|b| b.borrow_mut().take()) { eprintln!("tick {t}\n{b}"); break; }
        }
    });
}
