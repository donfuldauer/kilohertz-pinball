//! REQ-CAB-008 (R4): `playfield_attitude` never allocates, so it can run at load and from
//! sim setup.

use pin_core::schema::cabinet::{CabinetGeometry, playfield_attitude};
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

/// REQ-CAB-008: no allocation in `playfield_attitude` on any preset.
#[test]
fn req_cab_008_playfield_attitude_does_not_allocate() {
    let geoms = [
        CabinetGeometry::preset("classic_standard").unwrap(),
        CabinetGeometry::preset("classic_widebody").unwrap(),
        CabinetGeometry::preset("modern_standard").unwrap(),
    ];
    let before = allocs();
    let mut sum = 0.0;
    for g in &geoms {
        for _ in 0..1000 {
            let a = playfield_attitude(std::hint::black_box(g));
            sum += a.pitch_rad + a.roll_rad;
        }
    }
    assert_eq!(allocs(), before);
    assert!(sum.is_finite());
}
