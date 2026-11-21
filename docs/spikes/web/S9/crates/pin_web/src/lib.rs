//! web-spike-integration (S9): ONE `wasm32-unknown-unknown` engine module.
//!
//! * [`engine`]: bundle bytes → VFS (web zip limits) → `Simulation` with the
//!   real Lua backend (vendored Lua 5.4.9 + KHZ 1–32, S2 route b) or the
//!   wasmi backend running the instrumented `rules.wasm` (S4 design A);
//!   fixed-step replay/live stepping; per-tick parity against native hashes.
//! * `web` (wasm32): the wasm-bindgen surface: `WebEngine`, `WebGpu` (wgpu
//!   WebGPU or WebGL2 on a canvas, S3's table renderer) and the probes.
//!
//! Spike tooling (scratch copy only), not a product crate.

pub mod engine;
#[cfg(target_arch = "wasm32")]
pub mod web;

/// One global allocator for the whole module: pin_sim's counting allocator
/// (D5 guard) wrapped by S4's byte high-water tracker.
#[global_allocator]
static GLOBAL: s4_rules::peak::PeakAlloc = s4_rules::peak::PeakAlloc;
