//! `pin_core`: every cross-crate contract of Kilohertz Pinball (PLAN.md §3, D4).
//!
//! * [`ids`]: interned `u16` identifiers (no `String` crosses into the sim thread).
//! * [`units`]: mm/g/deg ⇄ SI conversion helpers (D17).
//! * [`triple_buffer`]: lock-free, allocation-free sim → render snapshot hand-off (D7).
//! * [`snapshot`]: fixed-capacity [`FrameStateSnapshot`].
//! * [`profiles`]: data-driven [`BallProfile`]s and the seven built-ins (§3.2).
//! * [`flipper`]: runtime flipper contract (§3.3).
//! * [`script`]: scripting contract (§3.4).
//! * [`input`]: input events, cabinet inputs and haptics events.
//! * [`materials`]: material pairs (restitution and friction).
//! * [`schema`]: on-disk TOML schemas (Appendix A, Appendix C).
//! * [`telemetry`]: UDP telemetry protocol v1 (Appendix B).
//!
//! Changes after Subphase 1.1 go through `contracts/CHANGE_REQUESTS.md`.

pub mod flipper;
pub mod ids;
pub mod input;
pub mod materials;
pub mod profiles;
pub mod schema;
pub mod script;
pub mod snapshot;
pub mod telemetry;
pub mod triple_buffer;
pub mod units;

pub use flipper::{FlipperConfig, FlipperShape, FlipperStagingMode};
pub use ids::{
    CoilId, DispenserId, DiverterId, FlipperId, LampId, MagnetId, PersistKey, ProfileId, SceneId,
    SoundId, SwitchId,
};
pub use input::{CabinetAxis, CabinetInput, HapticsTelemetryEvent, InputEvent, InputEventKind};
pub use materials::{Material, MaterialPair};
pub use profiles::{BallProfile, builtin_profile};
pub use schema::{BackglassManifest, LayoutManifest, PhysicsDefaults, SchemaError, TableManifest};
pub use script::{
    HostCommandQueue, HostCtx, HostView, ScriptError, SwitchEdge, TableScriptBackend,
};
pub use snapshot::FrameStateSnapshot;
pub use triple_buffer::{TripleBuffer, TripleReader, TripleWriter};

/// The sim-owned, seeded random generator handed to scripts (§3.4).
pub use rand_pcg::Pcg32;
/// Seeding and drawing traits for [`Pcg32`] (re-exported so dependants need no extra pin).
pub use rand_core::{Rng, SeedableRng};
/// Fixed-capacity vector used by the scripting contract (re-exported for dependants).
pub use arrayvec::ArrayVec;
