//! Gate step 6: `TripleWriter::publish` and `TripleReader::read` never allocate.
//! A counting global allocator (per thread) observes the hot loops.

use pin_core::{FrameStateSnapshot, TripleBuffer};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

fn bump() {
    // `try_with` so allocations during thread teardown are not a problem.
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
fn publish_and_read_do_not_allocate() {
    let (mut writer, mut reader) = TripleBuffer::new(FrameStateSnapshot::default());

    // Same thread.
    let before = allocs();
    for tick in 1..=10_000u64 {
        writer.publish(FrameStateSnapshot {
            tick,
            ..FrameStateSnapshot::default()
        });
        if tick % 3 == 0 {
            assert_eq!(reader.read().tick, tick);
        }
    }
    assert_eq!(allocs(), before, "publish/read allocated");

    // Two threads, each counting its own allocations inside the hot loop.
    let w = std::thread::spawn(move || {
        let before = allocs();
        for tick in 10_001..=60_000u64 {
            writer.publish(FrameStateSnapshot {
                tick,
                ..FrameStateSnapshot::default()
            });
        }
        allocs() - before
    });
    let r = std::thread::spawn(move || {
        let before = allocs();
        let mut last = 0;
        while last < 60_000 {
            last = reader.read().tick;
        }
        allocs() - before
    });
    assert_eq!(w.join().unwrap(), 0, "writer allocated");
    assert_eq!(r.join().unwrap(), 0, "reader allocated");
}
