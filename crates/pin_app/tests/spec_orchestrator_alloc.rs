//! Independent spec test: `SimLoop::run_ticks` never allocates (Subphase 1.4 criterion 4,
//! gate step 6, D5). A counting global allocator with a thread-local counter wraps the
//! system allocator; only allocations made on the calling thread during `run_ticks` count.

use pin_app::orchestrator::{IdleSim, SimLoop};
use pin_core::sim::{SimStep, TICK_NS};
use pin_core::snapshot::BallState;
use pin_core::spsc;
use pin_core::{
    CabinetAxis, CabinetInput, FrameStateSnapshot, InputEvent, InputEventKind, TripleBuffer,
};
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
    ALLOCS.with(|c| c.get())
}

/// Allocation-free test sim whose output depends on the inputs it is given.
struct Mixer {
    acc: f32,
    seen: u32,
}

impl SimStep for Mixer {
    fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
        for ev in inputs {
            self.seen += 1;
            if let InputEventKind::Axis { value, .. } = ev.kind {
                self.acc += value;
            }
        }
        *out = FrameStateSnapshot::default();
        out.tick = tick;
        out.push_ball(BallState {
            position_m: [self.acc, self.seen as f32, 0.0],
            ..BallState::default()
        });
    }
}

fn fill(input: &mut spsc::SpscProducer<InputEvent>, ticks: u64) -> u32 {
    let mut n = 0;
    let mut ts = 0u64;
    while ts <= ticks * TICK_NS {
        let ev = if n % 2 == 0 {
            InputEvent::axis(ts, CabinetAxis::Plunger, 0.5)
        } else {
            InputEvent::button(ts, CabinetInput::RightFlipper, n % 4 == 1)
        };
        input.push(ev).unwrap();
        n += 1;
        ts += 333_333;
    }
    n
}

#[test]
fn run_ticks_does_not_allocate_with_idle_sim() {
    let (mut input, consumer) = spsc::channel::<InputEvent>(8192);
    let (writer, mut reader) = TripleBuffer::new(FrameStateSnapshot::default());
    let mut sim_loop = SimLoop::new(IdleSim, consumer, writer);
    fill(&mut input, 2_000);

    // The very first tick is measured too: no lazy first-use allocation is allowed.
    let before = allocs();
    sim_loop.run_ticks(1);
    sim_loop.run_ticks(999);
    sim_loop.run_ticks(1_000);
    let after = allocs();
    assert_eq!(
        after - before,
        0,
        "run_ticks allocated {} times",
        after - before
    );
    assert_eq!(sim_loop.tick(), 2_000);
    assert_eq!(reader.read().tick, 2_000);
}

#[test]
fn run_ticks_does_not_allocate_with_inputs_flowing() {
    let (mut input, consumer) = spsc::channel::<InputEvent>(8192);
    let (writer, mut reader) = TripleBuffer::new(FrameStateSnapshot::default());
    let mut sim_loop = SimLoop::new(Mixer { acc: 0.0, seen: 0 }, consumer, writer);
    let pushed = fill(&mut input, 2_000);

    let before = allocs();
    for _ in 0..200 {
        sim_loop.run_ticks(10);
    }
    let after = allocs();
    assert_eq!(
        after - before,
        0,
        "run_ticks allocated {} times",
        after - before
    );

    let s = *reader.read();
    assert_eq!(s.tick, 2_000);
    // Every queued event (timestamps ≤ 2000 ms) reached the sim.
    assert_eq!(s.balls()[0].position_m[1], pushed as f32);
}

#[test]
fn a_full_burst_tick_does_not_allocate() {
    let (mut input, consumer) = spsc::channel::<InputEvent>(1024);
    let (writer, _reader) = TripleBuffer::new(FrameStateSnapshot::default());
    let mut sim_loop = SimLoop::new(Mixer { acc: 0.0, seen: 0 }, consumer, writer);
    for i in 0..256u64 {
        input
            .push(InputEvent::button(i, CabinetInput::Action, i % 2 == 0))
            .unwrap();
    }
    let before = allocs();
    sim_loop.run_ticks(4);
    let after = allocs();
    assert_eq!(after - before, 0);
    assert_eq!(sim_loop.tick(), 4);
}
