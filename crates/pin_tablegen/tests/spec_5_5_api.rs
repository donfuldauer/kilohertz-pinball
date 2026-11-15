//! Subphase 5.5 spec tests (Independent Tester): the one place where the
//! tablegen spec tests touch the `pin_tablegen::backbox` API, included with
//! `#[path]` (an empty test crate on its own).
//!
//! The 5.5 brief fixes only the module (`pin_tablegen::backbox`) and the
//! behaviour ("generates deterministic backbox geometry from an optional
//! backbox layout block; errors name the offending element; never
//! panics"), not the names. The names below follow the conventions of the
//! merged `pin_tablegen::layout` module (`parse_layout`,
//! `parse_layout_bytes`, `generate`, `GenError { element, message }`,
//! `GeneratedTable { glb, .. }`); see DECISIONS "5.5-T — Assumed backbox API
//! of the spec tests". If the implementation differs, only this file needs
//! to change.
//!
//! Assumed layout block (TOML, mm, like `layout.toml`):
//!
//! ```toml
//! [backbox]
//! outline = [[x, y], ...]      # in-plane polygon of the mini-playfield, mm
//! [[backbox.walls]]            # optional
//! points = [[x, y], ...]
//! ```
#![allow(dead_code)]

pub use pin_tablegen::backbox::{BackboxLayout, GeneratedBackbox};
pub use pin_tablegen::layout::GenError;

/// Parses the text of a backbox layout block.
pub fn parse(text: &str) -> Result<BackboxLayout, GenError> {
    pin_tablegen::backbox::parse_backbox(text)
}

/// Parses raw bytes (the fuzzed path).
pub fn parse_bytes(bytes: &[u8]) -> Result<BackboxLayout, GenError> {
    pin_tablegen::backbox::parse_backbox_bytes(bytes)
}

/// Generates the backbox geometry; returns the `.glb` bytes.
pub fn generate(layout: &BackboxLayout) -> Result<Vec<u8>, GenError> {
    pin_tablegen::backbox::generate_backbox(layout).map(|g: GeneratedBackbox| g.glb)
}

/// The offending element named by an error.
pub fn element(e: &GenError) -> &str {
    &e.element
}

/// The whole message (element and text).
pub fn message(e: &GenError) -> String {
    e.to_string()
}
