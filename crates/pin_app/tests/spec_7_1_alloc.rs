//! 7.1 requirement 6: recording from the sim thread performs no heap
//! allocation (D5). A counting global allocator armed only on the calling
//! thread, scoped to the record calls.

use pin_app::profiler::{CommandQueueOverflows, ProfiledSim, Profiler};
use pin_core::{CabinetAxis, CabinetInput, FrameStateSnapshot, InputEvent, SimStep};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};

struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
thread_local! { static ARMED: Cell<bool> = const { Cell::new(false) }; }

fn armed() -> bool {
    ARMED.try_with(Cell::get).unwrap_or(false)
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if armed() {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if armed() {
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

struct Toy(u64);
impl SimStep for Toy {
    fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
        self.0 += inputs.len() as u64;
        out.tick = tick;
    }
}
impl CommandQueueOverflows for Toy {
    fn command_queue_overflows(&self) -> u64 {
        self.0 / 3
    }
}

#[test]
fn recording_a_million_samples_does_not_allocate() {
    let (_, n) = guarded(|| Box::new(1u8));
    assert!(n >= 1, "allocator guard not armed");

    let mut p = Profiler::new();
    let evs = [
        InputEvent::button(5, CabinetInput::LeftFlipper, true),
        InputEvent::button(6, CabinetInput::Action, true),
        InputEvent::axis(7, CabinetAxis::Plunger, 0.25),
        InputEvent::button(u64::MAX, CabinetInput::Launch, false),
    ];
    let (_, allocs) = guarded(|| {
        for i in 0..500_000u64 {
            p.sim.record_tick_ns(100_000 + i % 400_000);
            p.sim.record_tick(i, i + 123_456);
            let _ = p.sim.record_input(i, i + i % 2_000_000);
            if i % 4 == 0 {
                p.sim.record_inputs(&evs, i + 1_000);
            }
            p.present.record_frame_ns(10_000_000 + i % 9_000_000);
            p.present.record_frame(i, i + 16_000_000);
            p.sim.set_script_queue_overflows(i / 1000);
            p.sim_loop.set_input_overflow_ticks(i / 5000);
        }
        // Saturating samples too.
        p.sim.record_tick_ns(u64::MAX);
        p.present.record_frame_ns(u64::MAX);
    });
    assert_eq!(allocs, 0, "recording allocated {allocs} times");
    let s = p.reader.snapshot();
    assert!(s.tick.count >= 1_000_000, "{}", s.tick.count);
    assert!(s.input.count >= 500_000);
    assert!(s.gpu_frame.count >= 1_000_000);
}

#[test]
fn a_profiled_step_does_not_allocate() {
    let p = Profiler::new();
    let mut t = 0u64;
    let mut sim = ProfiledSim::new(Toy(0), p.sim, move || {
        t += 40_000;
        t
    });
    let evs = [InputEvent::button(1, CabinetInput::RightFlipper, true)];
    let mut out = FrameStateSnapshot::default();
    sim.step_1ms(1, &evs, &mut out);
    let (_, allocs) = guarded(|| {
        for tick in 2..=100_000u64 {
            sim.step_1ms(tick, &evs, &mut out);
        }
    });
    assert_eq!(allocs, 0, "ProfiledSim::step_1ms allocated");
    let s = p.reader.snapshot();
    assert_eq!(s.tick.count, 100_000);
    assert_eq!(
        s.script_queue_overflows,
        sim.inner().command_queue_overflows()
    );
}
