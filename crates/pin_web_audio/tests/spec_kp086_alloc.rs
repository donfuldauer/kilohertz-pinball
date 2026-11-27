//! Independent Tester, KP-086 (#180): allocation guard for the web audio
//! render path (REQ-WEB-022, R4).
//!
//! A counting global allocator (per-thread count, the pattern of
//! `pin_audio/tests/mixer_alloc.rs`). After `pwa_new` and registration,
//! thousands of `pwa_render`, `pwa_trigger` and `pwa_submit` calls,
//! including rejected records, unknown ids, voice stealing, a ring overflow
//! and a full pending list, allocate (and free and reallocate) nothing.

#[path = "spec_kp086/eng.rs"]
mod eng;
#[path = "spec_kp086/oracle.rs"]
mod oracle;

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use eng::Eng;
use oracle::{NEXT, noise, record};

struct Counting;

thread_local! {
    static COUNT: Cell<u64> = const { Cell::new(0) };
}

// SAFETY: forwards to `System`; only counts.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        // SAFETY: same contract as the caller's.
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        // SAFETY: same contract as the caller's.
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let _ = COUNT.try_with(|c| c.set(c.get() + 1));
        // SAFETY: same contract as the caller's.
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

/// REQ-WEB-022, R4: 0 allocations in `pwa_render`, `pwa_trigger` and
/// `pwa_submit` after setup, from the very first render.
#[test]
fn req_web_022_r4_render_trigger_submit_allocate_nothing() {
    let mut e = Eng::new(48_000, 128, 8, 64).unwrap();
    assert_eq!(e.register(0, 0, 2, &noise(20_000, 1)), 0);
    assert_eq!(e.register(1, 1, 1, &noise(3_000, 2)), 0);
    assert_eq!(e.register(2, 2, 1, &noise(500, 3)), 0);
    assert_eq!(e.register(3, 2, 2, &noise(260, 4)), 0);

    // Command batches prepared before counting: good records with past,
    // equal, future and "next" `at`, unknown ids, rejected ops and ids,
    // NaN/inf gains and a trailing partial record.
    let mut good = Vec::new();
    for k in 0..40u32 {
        let at = match k % 4 {
            0 => NEXT,
            1 => 0,
            2 => k / 2,
            _ => 3,
        };
        good.extend_from_slice(&record(1, k % 6, (0.05 * k as f32).to_bits(), at));
    }
    let mut bad = Vec::new();
    bad.extend_from_slice(&record(0, 1, 0, 0));
    bad.extend_from_slice(&record(1, 70_000, 0, 0));
    bad.extend_from_slice(&record(1, 2, f32::NAN.to_bits(), NEXT));
    bad.extend_from_slice(&record(1, 2, f32::INFINITY.to_bits(), NEXT));
    bad.extend_from_slice(&[9, 9, 9]);
    // Far-future records to fill the 256-entry pending list.
    let mut far = Vec::new();
    for _ in 0..256 {
        far.extend_from_slice(&record(1, 2, 0.1f32.to_bits(), 0xFFFF_FFF0));
    }

    let mut total = 0;
    let mut sum = 0.0f64;
    for round in 0..2_000u32 {
        let (s, a) = count(|| {
            let mut s = 0.0f64;
            if round % 3 == 0 {
                e.submit(&good);
            }
            if round % 5 == 0 {
                e.submit(&bad);
            }
            if round == 1_000 {
                e.submit(&far);
            }
            for k in 0..6 {
                e.trigger((k + round) % 9, 0.5);
            }
            e.trigger(1 << 20, 1.0);
            let _ = e.render(0);
            let _ = e.render(129);
            if let Some(out) = e.render(128) {
                s += f64::from(out[0]);
            }
            if let Some(out) = e.render(1 + round % 128) {
                s += f64::from(out[out.len() - 1]);
            }
            for w in 0..10 {
                s += e.stat(w);
            }
            s
        });
        sum += s;
        total += a;
    }
    assert!(sum.is_nan() || sum.is_finite());
    assert!(
        e.stat(0) > 0.0,
        "ring overflow and full pending list exercised"
    );
    assert!(e.stat(1) > 0.0, "rejected records exercised");
    assert!(e.stat(2) > 0.0, "unknown sounds exercised");
    assert!(e.stat(3) > 0.0, "voice stealing exercised");
    assert_eq!(e.stat(5), 4_000.0);
    assert_eq!(total, 0, "allocations in pwa_render/pwa_trigger/pwa_submit");
}
