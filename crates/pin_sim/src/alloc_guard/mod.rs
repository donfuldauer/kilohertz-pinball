//! The allocation guard around `step_1ms()` (PLAN.md §2.3 rule 4, D5).
//!
//! [`CountingAlloc`] forwards to the system allocator and counts, per
//! thread, every allocation, zeroed allocation and reallocation made while
//! a guard is open on that thread. A test (or a tool) installs it once:
//!
//! ```ignore
//! #[global_allocator]
//! static GLOBAL: pin_sim::alloc_guard::CountingAlloc = pin_sim::alloc_guard::CountingAlloc;
//! ```
//!
//! and wraps the ticks in [`assert_no_alloc`], which panics (after the
//! closure has returned, never inside the allocator, which must not unwind)
//! when anything allocated, or uses [`count_allocs`] to read the number.
//! Without the allocator installed every count is 0; [`is_installed`] tells
//! the two apart.
//!
//! Exemptions (PLAN.md §2.3 rule 4): the Lua VM allocates from its own capped
//! arena (mlua routes it through the global allocator), and a hot-reload swap
//! tick may allocate. The simulation marks its calls into a Lua backend with
//! an [`ExemptScope`]: allocations inside one are counted separately
//! ([`count_allocs_split`]) and do not fail [`assert_no_alloc`]. Even a script
//! that creates no objects makes the VM allocate now and then, because the
//! collector shrinks the VM's call-info list and the next call regrows it
//! (DECISIONS.md "2.5 — allocation guard and the Lua arena"). Wasm backends
//! are not exempt.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static DEPTH: Cell<u32> = const { Cell::new(0) };
    static COUNT: Cell<u64> = const { Cell::new(0) };
    static EXEMPT: Cell<u32> = const { Cell::new(0) };
    static EXEMPT_COUNT: Cell<u64> = const { Cell::new(0) };
}

fn bump(c: &'static std::thread::LocalKey<Cell<u64>>) {
    let _ = c.try_with(|c| c.set(c.get().saturating_add(1)));
}

fn note_alloc() {
    // `try_with`: the allocator also runs during thread teardown.
    let _ = DEPTH.try_with(|d| {
        if d.get() > 0 {
            if EXEMPT.try_with(Cell::get).unwrap_or(0) > 0 {
                bump(&EXEMPT_COUNT);
            } else {
                bump(&COUNT);
            }
        }
    });
}

/// While alive, allocations on this thread are exempt (rule 4: the Lua VM's
/// capped arena). Scopes nest. Entering and leaving never allocates.
#[derive(Debug)]
pub struct ExemptScope(());

impl ExemptScope {
    /// Opens an exempt scope on the current thread.
    #[must_use]
    pub fn enter() -> Self {
        let _ = EXEMPT.try_with(|e| e.set(e.get().saturating_add(1)));
        ExemptScope(())
    }
}

impl Drop for ExemptScope {
    fn drop(&mut self) {
        let _ = EXEMPT.try_with(|e| e.set(e.get().saturating_sub(1)));
    }
}

/// A global allocator that counts the allocations made inside a guard
/// ([`count_allocs`], [`assert_no_alloc`]) on the current thread.
#[derive(Copy, Clone, Debug, Default)]
pub struct CountingAlloc;

// SAFETY: every method forwards to `System` with the same arguments; the only
// addition is a thread-local counter update, which neither allocates nor
// unwinds.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        note_alloc();
        // SAFETY: forwarded unchanged; the caller upholds `alloc`'s contract.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        note_alloc();
        // SAFETY: forwarded unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        note_alloc();
        // SAFETY: forwarded unchanged.
        unsafe { System.realloc(ptr, layout, new_size) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: forwarded unchanged.
        unsafe { System.dealloc(ptr, layout) }
    }
}

/// Runs `f` and returns its result and the number of allocations it made on
/// this thread (0 when [`CountingAlloc`] is not the global allocator).
/// Guards nest; the inner count is included in the outer one.
pub fn count_allocs<R>(f: impl FnOnce() -> R) -> (R, u64) {
    let (r, n, _) = count_allocs_split(f);
    (r, n)
}

/// Like [`count_allocs`], also returning the allocations made inside an
/// [`ExemptScope`] (third value), which the second value excludes.
pub fn count_allocs_split<R>(f: impl FnOnce() -> R) -> (R, u64, u64) {
    let before = COUNT.with(Cell::get);
    let exempt_before = EXEMPT_COUNT.with(Cell::get);
    DEPTH.with(|d| d.set(d.get() + 1));
    struct Close;
    impl Drop for Close {
        fn drop(&mut self) {
            DEPTH.with(|d| d.set(d.get().saturating_sub(1)));
        }
    }
    let close = Close;
    let r = f();
    drop(close);
    let after = COUNT.with(Cell::get);
    let exempt_after = EXEMPT_COUNT.with(Cell::get);
    (r, after - before, exempt_after - exempt_before)
}

/// Runs `f` and panics if it allocated on this thread outside an
/// [`ExemptScope`] (D5). The panic
/// message names `what` and the count.
///
/// # Panics
/// When `f` made at least one allocation.
pub fn assert_no_alloc<R>(what: &str, f: impl FnOnce() -> R) -> R {
    let (r, n) = count_allocs(f);
    assert!(n == 0, "{what}: {n} heap allocation(s) inside the guard");
    r
}

/// Whether [`CountingAlloc`] is the global allocator of this program (a
/// probe allocation inside a guard is counted).
#[must_use]
pub fn is_installed() -> bool {
    let ((), n) = count_allocs(|| {
        let b = std::hint::black_box(Box::new(0u64));
        drop(b);
    });
    n > 0
}
