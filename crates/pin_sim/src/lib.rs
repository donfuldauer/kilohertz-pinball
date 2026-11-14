//! `pin_sim`: Headless simulation: tick order, command application, record/replay and autoplay.
//!
//! Module directories are pre-stubbed by Subphase 1.1 (PLAN.md §2.1, D3); each
//! module names the subphase that owns it.
//!
//! [`Simulation`] (Subphase 2.5) loads a table bundle (folder or `.zip`)
//! and runs the fixed 1 kHz tick order of PLAN.md §2.2 with the ball solver,
//! flippers, mechs, the Magnus/remanence hooks, input staging and the rules
//! script, all deterministic and GPU-free. It implements
//! [`pin_core::sim::SimStep`] and offers a test API for state injection and
//! queries. [`replay`] records and replays input streams to an identical
//! [`Simulation::state_hash`]; the `pin_sim` binary is its command line
//! (`--record-inputs`, `--replay-inputs`). [`alloc_guard`] is the counting
//! allocator the tests wrap `step_1ms()` in (D5).
//!
//! * [`tick`]: `step_1ms()` and the tick order.
//! * [`commands`]: `HostCommandQueue` application (tick step 3).
//! * [`load`]: building a [`Simulation`] from a bundle.
//! * [`hash`]: the FNV-1a 64 hash used for state and table hashes.
//! * [`autoplay`]: Subphase 2.9 (stub).

pub mod alloc_guard;
pub mod cli;
pub mod autoplay;
pub mod commands;
pub mod hash;
pub mod load;
pub mod replay;
pub mod sim;
pub mod tick;

pub use sim::{
    BallHandle, DEFAULT_SEED, HAPTICS_CAPACITY, MAX_PENDING_EDGES, SimConfig, SimCounters,
    SimError, Simulation,
};
