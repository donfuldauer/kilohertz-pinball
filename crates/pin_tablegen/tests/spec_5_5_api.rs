//! Subphase 5.5 spec tests (Independent Tester): the one place where the
//! tablegen spec tests touch the `pin_tablegen::backbox` API, included with
//! `#[path]` (an empty test crate on its own).
//!
//! BOUND TO THE REAL API (5.5-FIX1, DECISIONS "5.5 — spec adapter bound to
//! the real API"; 5.5-FIX1b, DECISIONS "5.5 — backbox outline and walls
//! (spec-test ruling)"): the real input is `backbox_layout.toml`
//! (`pin_tablegen::backbox` rustdoc), whose `[backbox]` now has a real
//! optional polygon `outline` and real `[[backbox.walls]]` polylines with
//! the same TOML shape as the assumed block. The adapter passes the assumed
//! block's `[backbox]` table through unchanged (outline and walls values
//! 1:1, no bounding-box translation, walls forwarded) and only fills the
//! fields the real file requires that the assumed block lacks:
//! `schema_version = 1`, `origin = [0, 1150, 250]`, `depth = 35`, `width`
//! and `height` = the outline's bounding box (the real rule: they must equal
//! it), and one entry and one drain hole at the quarter points of that box.
//! Outline validation (NaN/inf, 3..=1024 points, zero size,
//! self-intersection) and wall validation are the real generator's. The
//! original header follows.
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

/// The outline's points as `f32` pairs, when it is a non-empty array of
/// `[x, y]` numbers.
fn outline_points(bb: &toml::Table) -> Option<Vec<(f32, f32)>> {
    let outline = bb.get("outline")?.as_array()?;
    let mut pts = Vec::with_capacity(outline.len());
    for p in outline {
        let a = p.as_array().filter(|a| a.len() == 2)?;
        pts.push((as_f32(&a[0])?, as_f32(&a[1])?));
    }
    (!pts.is_empty()).then_some(pts)
}

/// Translates the assumed block (`[backbox] outline = [[x, y], ...]` and
/// `[[backbox.walls]] points = [[x, y], ...]`, mm) into the real
/// `backbox_layout.toml` text: the `[backbox]` table is kept as written
/// (outline and walls passed through 1:1) and the fields the real file
/// requires but the assumed block has no notion of are filled in when
/// absent: `schema_version`, `origin`, `depth`, `width` / `height` (the
/// outline's bounding box, which the real file requires them to equal) and
/// the entry and drain holes at the quarter points of that box. When the
/// outline is not an array of number pairs the size-derived fields are left
/// out and the real parser reports what is missing.
fn translate(text: &str) -> Result<String, GenError> {
    let mut table: toml::Table =
        toml::from_str(text).map_err(|e| err("backbox", e.to_string().trim_end().to_owned()))?;
    let bb = table
        .get_mut("backbox")
        .and_then(toml::Value::as_table_mut)
        .ok_or_else(|| err("backbox", "missing [backbox] table"))?;
    let float = |v: f32| toml::Value::Float(f64::from(v));
    let fill = |bb: &mut toml::Table, key: &str, v: toml::Value| {
        bb.entry(key.to_owned()).or_insert(v);
    };
    fill(
        bb,
        "origin",
        toml::Value::Array(vec![float(0.0), float(1150.0), float(250.0)]),
    );
    fill(bb, "depth", float(35.0));
    if let Some(pts) = outline_points(bb) {
        let (x0, x1) = extent(pts.iter().map(|p| p.0));
        let (y0, y1) = extent(pts.iter().map(|p| p.1));
        let w = x1 - x0;
        fill(bb, "width", float(w));
        fill(bb, "height", float(y1 - y0));
        let mut entry = toml::Table::new();
        entry.insert(
            "vuk".into(),
            toml::Value::Array(vec![float(120.0), float(1000.0)]),
        );
        entry.insert("at".into(), float(x0 + 0.25 * w));
        entry.insert("coil".into(), toml::Value::String("bb_vuk_coil".into()));
        entry.insert("switch".into(), toml::Value::String("bb_vuk_switch".into()));
        fill(bb, "entry", toml::Value::Table(entry));
        let mut drain = toml::Table::new();
        drain.insert("at".into(), float(x0 + 0.75 * w));
        fill(bb, "drain", toml::Value::Table(drain));
    }
    table
        .entry("schema_version".to_owned())
        .or_insert(toml::Value::Integer(1));
    toml::to_string(&table).map_err(|e| err("backbox", e.to_string()))
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
