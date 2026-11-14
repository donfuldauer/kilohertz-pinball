//! `pin_input`: Input ingestion, flipper staging, device profiles, calibration and haptics.
//!
//! Module directories are pre-stubbed by Subphase 1.1 (PLAN.md §2.1, D3); each
//! module names the subphase that owns it.

pub mod calibration;
pub mod events;
pub mod haptics;
pub mod profiles;
pub mod staging;

pub use calibration::{
    DigitalNudge, NudgeConfig, NudgeFilter, PlungerConfig, PlungerFilter, PlungerOutput,
    thumbstick_nudge,
};
pub use events::{CabinetFrame, IngestConfig, InputIngest};
pub use profiles::{
    AxisBinding, DeviceMatch, DeviceProfile, GamepadAxis, GamepadButton, NudgeDirection,
    ProfileError, ProfileMapper, RawInput, SdlGuid, builtin_profiles, detect, detect_in,
    parse_profile,
};
pub use staging::{
    DEFAULT_LOWER_THRESHOLD, DEFAULT_UPPER_THRESHOLD, FlipperStage, FlipperStagingRouter,
    StagingEntry, StagingError,
};
