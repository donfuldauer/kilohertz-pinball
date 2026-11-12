//! `pin_render`: Rendering: display topology, wgpu context, PBR, reflections, post and displays.
//!
//! Module directories are pre-stubbed by Subphase 1.1 (PLAN.md §2.1, D3); each
//! module names the subphase that owns it.

pub mod topology;
pub mod gpu;
pub mod debug;
pub mod pbr;
pub mod reflections;
pub mod raytrace;
pub mod post;
pub mod displays;
pub mod backbox;
pub mod ui;
