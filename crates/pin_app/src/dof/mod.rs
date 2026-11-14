//! DirectOutput Framework bridge launcher (Subphase 3.5).
//!
//! DOF is a Windows .NET library fed table-element updates `(type, number,
//! value)` by its host; the engine only speaks UDP telemetry
//! (`docs/TELEMETRY_PROTOCOL.md`). The companion process
//! `tools/dof_bridge` (`KilohertzDofBridge.exe`) listens to the telemetry,
//! hosts the user's DOF and forwards every change of a mapped element.
//!
//! This module is the engine side:
//!
//! * [`ResolvedDofMap::resolve`] turns a table's `[dof]` section (element
//!   *names*) into telemetry ids at table load, and
//!   [`ResolvedDofMap::write_file`] writes the JSON map file the bridge reads
//!   (format `kilohertz-dof-map` v1, DECISIONS.md "3.5 — DOF bridge public
//!   API").
//! * [`DofBridge::start_for_table`] spawns the bridge on Windows when the table
//!   has `[dof]` (a no-op returning `Ok(None)` on Linux and macOS);
//!   [`DofBridge::stop`] and `Drop` stop it without blocking the caller.
//!
//! Calling the launcher on table load and unload is the orchestrator's job
//! (Subphase 6.0). Nothing here runs on the sim or render thread. The real-DOF
//! path is **unverified on hardware** (D33).

mod launcher;
mod map;

pub use launcher::{
    BRIDGE_EXE_NAME, DEFAULT_STOP_GRACE, DofBridge, DofLaunchConfig, DofStopHandle,
    platform_supported,
};
pub use map::{DofError, DofMapEntry, DofSourceKind, MAP_FORMAT, MAP_VERSION, ResolvedDofMap};
