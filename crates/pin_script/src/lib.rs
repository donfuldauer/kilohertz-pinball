//! `pin_script`: Table scripting backends: Lua 5.4 and WebAssembly.
//!
//! Module directories are pre-stubbed by Subphase 1.1 (PLAN.md §2.1, D3); each
//! module names the subphase that owns it.

pub mod guest_sdk;
pub mod lua;
pub mod wasm;
