//! `pin_assets`: Bundle VFS, zip hardening, glTF loader and manifest validation.
//!
//! Module directories are pre-stubbed by Subphase 1.1 (PLAN.md §2.1, D3); each
//! module names the subphase that owns it.

pub mod gltf;
pub mod validate;
pub mod vfs;
pub mod zip;
