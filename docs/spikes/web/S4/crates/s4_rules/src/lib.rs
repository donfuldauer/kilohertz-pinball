//! web-spike-rules (S4) harness. Spike tooling, not a product crate.
//!
//! * [`prescan`] / [`abi`]: the native load-time checks over plain wasmparser.
//! * [`hostcore`]: the `pinball` host functions without wasmtime types.
//! * [`instrument`]: wasmtime-exact fuel, depth counter, caps, host-error
//!   wrappers, by module rewriting.
//! * [`wasmi_backend`]: design A (wasmi in the engine module).
//! * `js_backend` (wasm32-unknown-unknown): design B (browser JIT).
//! * [`run`]: designs, recorder, scenario playback and benchmarks.
//! * [`adversarial`]: the adversarial corpus and runner.
//! * [`zipcheck`]: bundle-hardening checks (native and wasm).

pub mod abi;
pub mod adversarial;
pub mod driver;
pub mod hashes;
pub mod hostcore;
pub mod instrument;
pub mod io;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub mod js_backend;
#[cfg(feature = "native")]
pub mod native;
pub mod prescan;
pub mod run;
pub mod sha256;
pub mod wasmi_backend;
#[cfg(feature = "native")]
pub mod wasmtime_instr;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub mod web;
pub mod worst;
pub mod zipcheck;

/// Counting allocator (D5 guard) plus a byte high-water mark.
pub mod peak {
    use std::alloc::{GlobalAlloc, Layout};
    use std::sync::atomic::{AtomicUsize, Ordering};

    static CUR: AtomicUsize = AtomicUsize::new(0);
    static PEAK: AtomicUsize = AtomicUsize::new(0);

    /// The global allocator.
    pub struct PeakAlloc;

    fn add(n: usize) {
        let c = CUR.fetch_add(n, Ordering::Relaxed) + n;
        PEAK.fetch_max(c, Ordering::Relaxed);
    }

    // SAFETY: forwards to pin_sim's CountingAlloc (which forwards to System)
    // with the same arguments; only adds relaxed atomic counters.
    unsafe impl GlobalAlloc for PeakAlloc {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            // SAFETY: forwarded unchanged.
            let p = unsafe { pin_sim::alloc_guard::CountingAlloc.alloc(l) };
            if !p.is_null() {
                add(l.size());
            }
            p
        }
        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            // SAFETY: forwarded unchanged.
            let p = unsafe { pin_sim::alloc_guard::CountingAlloc.alloc_zeroed(l) };
            if !p.is_null() {
                add(l.size());
            }
            p
        }
        unsafe fn realloc(&self, ptr: *mut u8, l: Layout, n: usize) -> *mut u8 {
            // SAFETY: forwarded unchanged.
            let p = unsafe { pin_sim::alloc_guard::CountingAlloc.realloc(ptr, l, n) };
            if !p.is_null() {
                CUR.fetch_sub(l.size(), Ordering::Relaxed);
                add(n);
            }
            p
        }
        unsafe fn dealloc(&self, ptr: *mut u8, l: Layout) {
            // SAFETY: forwarded unchanged.
            unsafe { pin_sim::alloc_guard::CountingAlloc.dealloc(ptr, l) };
            CUR.fetch_sub(l.size(), Ordering::Relaxed);
        }
    }

    /// Bytes currently allocated.
    #[must_use]
    pub fn current() -> usize {
        CUR.load(Ordering::Relaxed)
    }

    /// Resets the high-water mark to the current level.
    pub fn reset() {
        PEAK.store(CUR.load(Ordering::Relaxed), Ordering::Relaxed);
    }

    /// High-water mark since the last reset.
    #[must_use]
    pub fn peak() -> usize {
        PEAK.load(Ordering::Relaxed)
    }
}

#[global_allocator]
static GLOBAL: peak::PeakAlloc = peak::PeakAlloc;
