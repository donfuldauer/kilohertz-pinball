//! Gate step 6: `InputIngest::push`/`advance` and `FlipperStagingRouter::update`
//! never allocate after construction. Counting global allocator (per thread).

use arrayvec::ArrayVec;
use pin_core::flipper::FlipperStagingMode;
use pin_core::ids::FlipperId;
use pin_core::input::{CabinetAxis, CabinetInput, InputEvent};
use pin_input::{
    DEFAULT_LOWER_THRESHOLD, DEFAULT_UPPER_THRESHOLD, FlipperStage, FlipperStagingRouter,
    IngestConfig, InputIngest, StagingEntry,
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
    ALLOCS.with(Cell::get)
}

#[test]
fn counter_detects_allocation() {
    let before = allocs();
    let v = std::hint::black_box(vec![1u8; 64]);
    assert!(allocs() > before);
    drop(v);
}

#[test]
fn hot_loop_does_not_allocate() {
    let dual = FlipperStagingMode::DualStageThreshold {
        axis: CabinetAxis::LeftFlipper,
        lower_threshold: DEFAULT_LOWER_THRESHOLD,
        upper_threshold: DEFAULT_UPPER_THRESHOLD,
    };
    let entries = [
        StagingEntry {
            flipper: FlipperId(0),
            mode: FlipperStagingMode::DedicatedButton(CabinetInput::RightFlipper),
            stage: FlipperStage::Lower,
        },
        StagingEntry {
            flipper: FlipperId(1),
            mode: FlipperStagingMode::ParallelLinked(CabinetInput::RightFlipper),
            stage: FlipperStage::Upper,
        },
        StagingEntry {
            flipper: FlipperId(2),
            mode: dual,
            stage: FlipperStage::Lower,
        },
        StagingEntry {
            flipper: FlipperId(3),
            mode: dual,
            stage: FlipperStage::Upper,
        },
    ];
    let mut router = FlipperStagingRouter::new(&entries).unwrap();
    let mut ingest = InputIngest::new(IngestConfig {
        epoch_ns: 0,
        debounce_ms: 3,
    });
    let mut out: ArrayVec<(FlipperId, bool), 16> = ArrayVec::new();
    let mut changes = 0u64;

    let before = allocs();
    for tick in 0..20_000u64 {
        let ns = tick * 1_000_000;
        // Slightly out-of-order delivery, a full queue now and then.
        let v = ((tick % 100) as f32) / 100.0;
        let _ = ingest.push(InputEvent::axis(ns + 300, CabinetAxis::LeftFlipper, v));
        let _ = ingest.push(InputEvent::button(
            ns + 100,
            CabinetInput::RightFlipper,
            tick % 7 < 3,
        ));
        if tick % 1000 == 0 {
            for k in 0..300u64 {
                let _ = ingest.push(InputEvent::axis(ns + k, CabinetAxis::NudgeX, 0.1));
            }
        }
        let frame = ingest.advance(tick);
        out.clear();
        router.update(&frame, &mut out);
        changes += out.len() as u64;
        std::hint::black_box(router.is_active(FlipperId(3)));
    }
    assert_eq!(allocs(), before, "ingest/router hot loop allocated");
    assert!(changes > 0);
}
