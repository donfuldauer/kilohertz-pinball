//! `pin_app`: Kilohertz Pinball application (library half).
//!
//! The modules live in a library target so integration tests (SDL headless
//! tests in `tests/`) can use them; `main.rs` is a thin binary on top
//! (Subphase 1.3 decision, see DECISIONS.md). Module directories were
//! pre-stubbed by Subphase 1.1 (PLAN.md §2.1, D3); each module names the
//! subphase that owns it.

pub mod dof;
pub mod orchestrator;
pub mod platform;
pub mod profiler;
pub mod ui;
