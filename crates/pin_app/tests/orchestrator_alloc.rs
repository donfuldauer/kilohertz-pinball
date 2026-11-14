//! Verification criterion 4 (D5): `SimLoop::run_ticks` does not allocate,
//! checked with a counting global allocator armed only on this thread.

use pin_app::orchestrator::{IdleSim, SimLoop};
use pin_core::spsc::channel;
use pin_core::{
    CabinetAxis, CabinetInput, FrameStateSnapshot, InputEvent, SimStep, TICK_NS, TripleBuffer,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
thread_local! { static ARMED: Cell<bool> = const { Cell::new(false) }; }

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if ARMED.try_with(Cell::get).unwrap_or(false) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if ARMED.try_with(Cell::get).unwrap_or(false) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn guarded<R>(f: impl FnOnce() -> R) -> (R, u64) {
    let before = ALLOCS.load(Ordering::Relaxed);
    ARMED.with(|a| a.set(true));
    let r = f();
    ARMED.with(|a| a.set(false));
    (r, ALLOCS.load(Ordering::Relaxed) - before)
}

/// A sim that touches its inputs and the snapshot like a real one would.
struct Busy(u64);
impl SimStep for Busy {
    fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
        self.0 += inputs.len() as u64;
        out.tick = tick;
        out.lamps[(tick % 256) as usize] = self.0 as f32;
    }
}

#[test]
fn run_ticks_is_allocation_free() {
    // Arming works.
    let (_, n) = guarded(|| Box::new(1u8));
    assert!(n >= 1, "allocator guard not armed");

    let (mut tx, rx) = channel(4096);
    let (w, mut r) = TripleBuffer::new(FrameStateSnapshot::default());
    let mut sim = SimLoop::new(Busy(0), rx, w);
    for i in 0..3000u64 {
        let ev = if i % 2 == 0 {
            InputEvent::button(i * TICK_NS / 3, CabinetInput::LeftFlipper, i % 4 == 0)
        } else {
            InputEvent::axis(i * TICK_NS / 3, CabinetAxis::Plunger, 0.5)
        };
        tx.push(ev).unwrap();
    }
    // Including a burst larger than the per-tick buffer.
    let (_, allocs) = guarded(|| {
        sim.run_ticks(1);
        sim.run_ticks(8);
        sim.run_ticks(2000);
        sim.set_epoch_ns(0);
        sim.skip_ticks(3);
        sim.run_ticks(5);
    });
    assert_eq!(allocs, 0, "run_ticks allocated {allocs} times");
    assert_eq!(sim.tick(), 2014);
    assert_eq!(r.read().tick, 2014);
    assert_eq!(sim.inputs_delivered(), 3000);

    let (tx2, rx2) = channel::<InputEvent>(1);
    drop(tx2);
    let (w2, _r2) = TripleBuffer::new(FrameStateSnapshot::default());
    let mut boxed = SimLoop::new(Box::new(IdleSim) as Box<dyn SimStep>, rx2, w2);
    let (_, allocs) = guarded(|| boxed.run_ticks(1000));
    assert_eq!(allocs, 0, "boxed trait object path allocated");
}
