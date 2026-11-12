//! Plunger and nudge calibration.
//!
//! Owner: Subphase 3.2.
//!
//! * [`PlungerFilter`]: analog plunger conditioning — deadzone, slow rest
//!   tracking while at rest, and release detection with the peak forward speed
//!   of the return stroke.
//! * [`NudgeFilter`]: accelerometer nudge — first-order high-pass (removes
//!   gravity/tilt offsets), deadzone and a gain curve.
//! * [`DigitalNudge`] and [`thumbstick_nudge`]: fallbacks for nudge buttons
//!   and thumbsticks.
//!
//! All per-sample methods are allocation-free, take `&mut self`/`&self` only,
//! and never propagate NaN (a NaN input is replaced by the previous input).
//! Arithmetic is plain IEEE `f32` (no fast-math); the only transcendental
//! function, the gain-curve power, uses `libm` (D9).

mod nudge;
mod plunger;

pub use nudge::{DigitalNudge, MAX_DIGITAL_PRESSES, NudgeConfig, NudgeFilter, thumbstick_nudge};
pub use plunger::{
    ACQUIRE_WINDOW_MS, ARM_REST_MS, MAX_RELEASE_WINDOW_MS, MAX_REST_OFFSET, MIN_PULL_DEPTH,
    MIN_PULL_TICKS, PULL_QUALIFY_DEADZONES, PUSH_IN_REACQUIRE_MS, PlungerConfig, PlungerFilter,
    PlungerOutput, RAW_LIMIT, RELEASE_HOLDOFF_MS, REST_LOOKBACK_TICKS, STABLE_MS,
};
