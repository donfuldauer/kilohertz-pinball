//! Subphase 5.5 spec tests (Independent Tester): the one place where the
//! tablegen spec tests touch the `pin_tablegen::backbox` API, included with
//! `#[path]` (an empty test crate on its own).
//!
//! BOUND TO THE REAL API (5.5-FIX1, DECISIONS "5.5 — spec adapter bound to
//! the real API"): the real input is `backbox_layout.toml`
//! (`pin_tablegen::backbox` rustdoc), a box with entry and drain holes, not
//! an outline. Each call below translates the assumed block (text and
//! values) onto the real one and calls the real functions; assumed concepts
//! with no real equivalent (polygon outlines, walls, the 1024-point limit,
//! self-intersection) are NOT emulated here. The original header follows.
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

use pin_core::schema::table::{CoilDef, CoilKind, SwitchDef, SwitchKind, TableManifest};
use pin_tablegen::backbox::{generate_backbox, parse_backbox_layout};

pub use pin_tablegen::backbox::BackboxLayout;
pub use pin_tablegen::layout::GenError;

fn err(element: &str, message: impl Into<String>) -> GenError {
    GenError {
        element: element.into(),
        message: message.into(),
    }
}

/// A TOML float literal for `v` (including `nan` and `inf`).
fn num(v: f32) -> String {
    if v.is_nan() {
        "nan".into()
    } else if v.is_infinite() {
        if v > 0.0 { "inf".into() } else { "-inf".into() }
    } else {
        format!("{v:?}")
    }
}

fn as_f32(v: &toml::Value) -> Option<f32> {
    match v {
        toml::Value::Float(f) => Some(*f as f32),
        toml::Value::Integer(i) => Some(*i as f32),
        _ => None,
    }
}

/// Min and max of `vals`, a NaN anywhere makes both NaN.
fn extent(vals: impl Iterator<Item = f32>) -> (f32, f32) {
    vals.fold((f32::INFINITY, f32::NEG_INFINITY), |(lo, hi), v| {
        if v.is_nan() || lo.is_nan() {
            (f32::NAN, f32::NAN)
        } else {
            (lo.min(v), hi.max(v))
        }
    })
}

/// Translates the assumed block (`[backbox] outline = [[x, y], ...]`, mm)
/// into the real `backbox_layout.toml` text: the outline's bounding box
/// becomes `width` and `height` (centred on `origin`), with a fixed
/// `depth` and one entry and one drain hole at the quarter points. The
/// `[[backbox.walls]]` have no real equivalent and are not translated.
fn translate(text: &str) -> Result<String, GenError> {
    let table: toml::Table =
        toml::from_str(text).map_err(|e| err("backbox", e.to_string().trim_end().to_owned()))?;
    let bb = table
        .get("backbox")
        .and_then(toml::Value::as_table)
        .ok_or_else(|| err("backbox", "missing [backbox] table"))?;
    let outline = bb
        .get("outline")
        .and_then(toml::Value::as_array)
        .ok_or_else(|| err("backbox.outline", "missing outline array"))?;
    let mut pts = Vec::with_capacity(outline.len());
    for p in outline {
        let xy = p.as_array().filter(|a| a.len() == 2);
        let (x, y) = xy
            .and_then(|a| Some((as_f32(&a[0])?, as_f32(&a[1])?)))
            .ok_or_else(|| err("backbox.outline", "points must be [x, y] numbers"))?;
        pts.push((x, y));
    }
    if pts.is_empty() {
        return Err(err("backbox.outline", "empty outline"));
    }
    let (x0, x1) = extent(pts.iter().map(|p| p.0));
    let (y0, y1) = extent(pts.iter().map(|p| p.1));
    let (w, h) = (x1 - x0, y1 - y0);
    Ok(format!(
        "schema_version = 1\n\n[backbox]\norigin = [{}, 1150.0, 250.0]\nwidth = {}\nheight = {}\n\
         depth = 35.0\n\n[backbox.entry]\nvuk = [120.0, 1000.0]\nat = {}\n\
         coil = \"bb_vuk_coil\"\nswitch = \"bb_vuk_switch\"\n\n[backbox.drain]\nat = {}\n",
        num(0.5 * (x0 + x1)),
        num(w),
        num(h),
        num(-0.25 * w),
        num(0.25 * w),
    ))
}

/// Parses the text of the assumed backbox layout block (through the real
/// `parse_backbox_layout`).
pub fn parse(text: &str) -> Result<BackboxLayout, GenError> {
    parse_backbox_layout(&translate(text)?).map(|f| f.backbox)
}

/// Parses raw bytes (the fuzzed path).
pub fn parse_bytes(bytes: &[u8]) -> Result<BackboxLayout, GenError> {
    let text = core::str::from_utf8(bytes)
        .map_err(|e| err("backbox", format!("not valid UTF-8 text: {e}")))?;
    parse(text)
}

/// The apex `table.toml` plus the VUK coil and switch the translated
/// layout names.
fn base() -> TableManifest {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../pin_assets/tests/fixtures/apex/table.toml");
    let text = std::fs::read_to_string(dir).expect("apex table.toml");
    let mut t = TableManifest::from_toml_str(&text).expect("apex table.toml parses");
    t.coils.push(CoilDef {
        id: "bb_vuk_coil".into(),
        number: 90,
        kind: CoilKind::Vuk,
    });
    t.switches.push(SwitchDef {
        id: "bb_vuk_switch".into(),
        number: 180,
        kind: SwitchKind::Opto,
        layer: 0,
        position: None,
        radius: None,
    });
    t
}

/// Generates the backbox geometry; returns the collision `.glb` bytes
/// (`backbox_colliders.glb`: the only generated file with collider nodes).
pub fn generate(layout: &BackboxLayout) -> Result<Vec<u8>, GenError> {
    generate_backbox(layout, &base()).map(|g| g.colliders_glb)
}

/// The offending element named by an error.
pub fn element(e: &GenError) -> &str {
    &e.element
}

/// The whole message (element and text).
pub fn message(e: &GenError) -> String {
    e.to_string()
}
