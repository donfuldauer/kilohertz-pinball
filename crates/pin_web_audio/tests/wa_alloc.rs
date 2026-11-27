//! REQ-WEB-022 / R4 allocation guard: after `pwa_new` (and load-time
//! registration), `pwa_render`, `pwa_trigger` and `pwa_submit` make zero
//! heap allocations (counting global allocator, per thread), including when
//! the ring and the pending list overflow.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use pin_web_audio::*;

struct Counting;

thread_local! {
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn count<R>(f: impl FnOnce() -> R) -> (R, u64) {
    let before = COUNT.with(Cell::get);
    let r = f();
    (r, COUNT.with(Cell::get) - before)
}

/// REQ-WEB-022: zero allocations in render, trigger and submit.
#[test]
fn req_web_022_zero_alloc_render_trigger_submit() {
    let e = pwa_new(48_000, 128, 8, 4);
    assert!(!e.is_null());
    let mono: Vec<f32> = (0..6_000).map(|i| (i % 50) as f32 / 100.0).collect();
    for (id, bus, ch) in [(1u32, 2u32, 1u32), (2, 0, 2), (3, 1, 2)] {
        let p = unsafe { pwa_stage(e, mono.len() as u32) };
        unsafe { std::ptr::copy_nonoverlapping(mono.as_ptr(), p, mono.len()) };
        let frames = mono.len() as u32 / ch;
        assert_eq!(unsafe { pwa_register(e, id, bus, ch, frames) }, 0);
    }
    let buf = unsafe { pwa_cmd_buffer(e) };
    let mut records = Vec::new();
    for i in 0..256u32 {
        records.extend_from_slice(&encode_trigger(1 + i % 4, 0.3, i % 40));
    }
    let mut total = 0;
    for round in 0..50u32 {
        let (_, n) = count(|| unsafe {
            // Immediate triggers, some unknown or out of range.
            for k in 0..12 {
                pwa_trigger(e, k % 5, 0.4);
            }
            pwa_trigger(e, 70_000, 1.0);
        });
        total += n;
        if round % 10 == 0 {
            unsafe { std::ptr::copy_nonoverlapping(records.as_ptr(), buf, records.len()) };
            let (_, n) = count(|| unsafe {
                pwa_submit(e, 4096); // fills the pending list
                pwa_submit(e, 37); // two records then a partial one
                pwa_submit(e, u32::MAX);
            });
            total += n;
        }
        let (p, n) = count(|| unsafe { pwa_render(e, 128) });
        assert!(!p.is_null());
        total += n;
        let (_, n) = count(|| unsafe {
            pwa_render(e, 0);
            pwa_render(e, 64);
            pwa_stat(e, 8)
        });
        total += n;
    }
    assert_eq!(total, 0, "allocations in render/trigger/submit");
    unsafe {
        assert!(pwa_stat(e, 0) > 0.0, "the overflow paths ran");
        assert!(pwa_stat(e, 1) > 0.0);
        assert!(pwa_stat(e, 3) > 0.0, "voice stealing ran");
        pwa_free(e);
    }
}
