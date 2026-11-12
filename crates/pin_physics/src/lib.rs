//! `pin_physics`: Physics: collision world, ball solver, flippers and mechs (1 kHz, SI units).
//!
//! Module directories are pre-stubbed by Subphase 1.1 (PLAN.md §2.1, D3); each
//! module names the subphase that owns it. [`world`] (Subphase 2.1) holds the
//! static collision world, spline colliders and layer transitions; [`ball`]
//! (Subphase 2.2) is the ball solver: CCD, spin, friction, magnets, sensors,
//! captive Newton balls and dispensers.

pub mod ball;
pub mod ball_ext;
pub mod debug;
pub mod flipper;
pub mod flipper_banana;
pub mod mech;
pub mod world;
