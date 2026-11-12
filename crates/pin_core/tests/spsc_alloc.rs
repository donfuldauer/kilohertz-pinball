//! Gate step 6: `SpscProducer::push` and `SpscConsumer::pop` never allocate.

use pin_core::spsc::channel;
use pin_core::{CabinetInput, InputEvent};
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
fn push_and_pop_do_not_allocate() {
    let (mut tx, mut rx) = channel::<InputEvent>(256);

    let before = allocs();
    for i in 0..10_000u64 {
        let _ = tx.push(InputEvent::button(i, CabinetInput::LeftFlipper, i % 2 == 0));
        if i % 3 == 0 {
            while rx.pop().is_some() {}
        }
        let _ = (rx.len(), tx.len(), rx.peek());
    }
    assert_eq!(allocs(), before, "push/pop allocated");

    let p = std::thread::spawn(move || {
        let before = allocs();
        let mut sent = 0u64;
        while sent < 50_000 {
            if tx
                .push(InputEvent::button(sent, CabinetInput::Start, true))
                .is_ok()
            {
                sent += 1;
            }
        }
        allocs() - before
    });
    let c = std::thread::spawn(move || {
        let before = allocs();
        let mut got = 0u64;
        while got < 50_000 {
            if rx.pop().is_some() {
                got += 1;
            }
        }
        allocs() - before
    });
    assert_eq!(p.join().unwrap(), 0, "producer allocated");
    assert_eq!(c.join().unwrap(), 0, "consumer allocated");
}
