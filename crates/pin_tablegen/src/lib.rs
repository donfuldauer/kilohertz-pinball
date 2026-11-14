//! `pin_tablegen`: Procedural table generator: layout.toml to .glb, clearance checks, backbox.
//!
//! Module directories are pre-stubbed by Subphase 1.1 (PLAN.md §2.1, D3); each
//! module names the subphase that owns it.
//!
//! [`layout`] (Subphase 4.3) turns a declarative `layout.toml` and a base
//! `table.toml` into a bundle's `playfield.glb` and merged `table.toml`; the
//! `pin_tablegen` binary is its command line
//! (`pin_tablegen <layout.toml> --table <table.toml> --out <bundle_dir>`).
//! Authoring rules: `docs/AUTHORING.md`.

pub mod backbox;
pub mod clearance;
pub mod layout;

pub use layout::{GenError, GeneratedTable, LOCK_FILE, generate, write_bundle};
