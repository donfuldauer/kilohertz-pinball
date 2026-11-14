//! BVH, collision layers and C² spline colliders.
//!
//! Owner: Subphase 2.1 (PLAN.md §4, D13).
//!
//! Everything here is SI (metres, seconds) and `f32`. Per-tick queries
//! (closest point, sphere cast, contacts, layer transitions) never allocate,
//! use no fused multiply-add, and take transcendental functions from `libm`,
//! so results are bit-identical across platforms (PLAN.md §2.4).
//!
//! * [`CubicSpline`]: C² interpolating cubic spline stored as Bézier segments.
//! * [`Wireform`]: two-rail wireform framed by a rotation-minimizing frame.
//! * [`GuideCurve`]: single-curve guide (orbit wall) with a wall height.
//! * [`StaticWorld`]: static colliders in a `parry3d` BVH with layer masks
//!   and per-collider enable flags.
//! * [`TransitionSet`]: layer-transition trigger zones.

mod curve_bounds;
mod shapes;
mod sloped_bounds;
mod spline;
mod static_world;
mod transition;
mod wireform;

pub use spline::{BezierSegment, ClosestPoint, CubicSpline, SplineError};
pub use static_world::{
    ColliderHandle, SphereContact, SphereHit, StaticShape, StaticWorld, StaticWorldBuilder,
};
pub use transition::{LayerTransition, TransitionSet};
pub use wireform::{Frame, GuideCast, GuideCurve, GuideHit, Wireform};

/// Collision layer bits (8-bit mask).
pub mod layers {
    /// Main playfield deck (bit 0).
    pub const MAIN_DECK: u8 = 1 << 0;
    /// Upper playfield deck (bit 1).
    pub const UPPER_DECK: u8 = 1 << 1;
    /// Wireform ramps (bit 2).
    pub const WIREFORM: u8 = 1 << 2;
    /// Subway below the playfield (bit 3).
    pub const SUBWAY: u8 = 1 << 3;
    /// Backbox (bit 4).
    pub const BACKBOX: u8 = 1 << 4;
    /// Every layer.
    pub const ALL: u8 = 0xFF;

    /// Mask bit of layer index `layer` (0..=7); 0 for an index above 7.
    #[must_use]
    pub const fn layer_bit(layer: u8) -> u8 {
        if layer > 7 { 0 } else { 1 << layer }
    }
}
