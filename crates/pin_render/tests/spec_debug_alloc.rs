//! Independent spec test: `DebugLines` and the line builders do not allocate after
//! construction (Subphase 1.4 FIXED API: "no allocation after construction").

use pin_core::snapshot::BallState;
use pin_core::{FrameStateSnapshot, ProfileId};
use pin_render::debug::{DebugLine, DebugLines, box_lines, polyline_lines, snapshot_lines};
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

#[test]
fn filling_overflowing_and_clearing_never_allocates() {
    let mut s = FrameStateSnapshot::default();
    for i in 0..16 {
        s.push_ball(BallState {
            position_m: [0.03 * i as f32, 0.4, 0.0135],
            profile: ProfileId(i % 3),
            ..BallState::default()
        });
    }
    let radius = |p: ProfileId| 0.01 + 0.002 * f32::from(p.0);
    let points: [[f32; 3]; 64] = std::array::from_fn(|i| [i as f32 * 0.01, 0.5, 0.0]);
    let mut lines = DebugLines::new(300);

    let before = allocs();
    let mut total_seen = 0usize;
    for round in 0..50 {
        lines.clear();
        snapshot_lines(&s, &radius, &mut lines);
        box_lines([0.0; 3], [0.5, 1.0, 0.05], [0.0, 1.0, 0.0, 1.0], &mut lines);
        polyline_lines(&points, [1.0, 1.0, 0.0, 1.0], &mut lines);
        // 256 + 12 + 63 = 331 > 300: the tail overflows and is dropped, without growing.
        for i in 0..40 {
            lines.push(DebugLine {
                a: [round as f32, i as f32, 0.0],
                b: [0.0; 3],
                color: [1.0; 4],
            });
        }
        total_seen += lines.as_slice().len();
    }
    let dropped = lines.dropped() as u64;
    let after = allocs();

    assert_eq!(
        after - before,
        0,
        "debug line building allocated {} times",
        after - before
    );
    assert_eq!(
        total_seen,
        50 * 300,
        "the list never grows past its capacity"
    );
    assert!(dropped > 0);
}
