//! Backbox mini-playfield geometry.
//!
//! Owner: Subphase 5.5.
//!
//! [`generate_backbox`] turns an optional backbox layout block (below) and a
//! base `table.toml` into the backbox's render scene (`backbox.glb`, the
//! scene `backglass.toml`'s `[backbox]` names), its collision geometry
//! (`backbox_colliders.glb` and [`GeneratedBackbox::colliders`]) and the
//! `table.toml` entries that wire it to the main playfield: the mini
//! flippers, the VUK that feeds the backbox, and the layer transitions in and
//! out. [`write_backbox`] writes the three files into a bundle folder. The
//! same inputs always give byte-identical outputs; an invalid layout is a
//! [`GenError`] naming the element (`backbox.flippers[1]`, `backbox.drain`,
//! ...), never a panic.
//!
//! # `backbox_layout.toml`
//!
//! The block lives in its own file next to `layout.toml` (the
//! `layout.toml` schema is closed, `deny_unknown_fields`; DECISIONS.md "5.5 —
//! Backbox layout block"). Units: mm and degrees, like `layout.toml`.
//!
//! ```toml
//! schema_version = 1
//!
//! [backbox]
//! origin = [0.0, 1150.0, 250.0]  # mm, table frame: bottom centre of the glass-side opening
//! width = 400.0                  # mm across (table x)
//! height = 300.0                 # mm, world-vertical
//! depth = 35.0                   # mm, glass to back panel (>= 30: a 27 mm ball plus clearance)
//! collision_layer = 4            # optional, default 4 (1..=7; 0 is the main deck)
//!
//! [[backbox.flippers]]
//! side = "left"                  # left | right
//! position = [-60.0, 40.0]       # mm, plane coordinates [u across, v up] from `origin`
//! length = 50.0                  # optional, mm (30..=100, default 50)
//! coil = "bb_left_coil"          # names from table.toml
//! eos_switch = "bb_left_eos"
//! staging = { type = "ParallelLinked", input = "left_flipper" }   # as [[flippers]] staging
//!
//! [backbox.entry]                # the VUK-fed entry hole in the shelf
//! vuk = [120.0, 1000.0]          # mm, main playfield [x, y] of the VUK scoop (layer 0)
//! at = -120.0                    # mm, u of the hole centre
//! width = 40.0                   # optional, mm (>= 30, default 40)
//! coil = "bb_vuk_coil"
//! switch = "bb_vuk_switch"
//!
//! [backbox.drain]                # the drain hole back to the main playfield
//! at = 150.0                     # mm, u of the hole centre
//! width = 40.0                   # optional, mm (>= 30, default 40)
//! ```
//!
//! ## Optional `outline` and `[[backbox.walls]]`
//!
//! ```toml
//! [backbox]
//! # ... origin, width, height, depth as above ...
//! outline = [[-200.0, 0.0], [200.0, 0.0], [200.0, 300.0], [-200.0, 300.0]]
//!
//! [[backbox.walls]]              # optional, any number
//! points = [[-160.0, 40.0], [-160.0, 260.0]]
//! ```
//!
//! Both are in plane coordinates `[u, v]`, mm from `origin` (`u` across =
//! table `x`, `v` up the backbox), like flipper positions. Without
//! `outline` the panel is the box `u` in `±width/2`, `v` in `0..height`
//! (the output is unchanged by this option). With `outline` the panel is
//! that polygon (3..=1024 points, either winding, non-zero area, not
//! self-intersecting, bounding box 100..=1000 mm each way); `width` and
//! `height` stay required and must equal its bounding box within 0.01 mm
//! (DECISIONS.md "5.5 — backbox outline and walls (spec-test ruling)").
//! Each wall is a polyline of 2..=1024 points, not all equal. Errors name
//! `backbox.outline`, `backbox.outline[i]` (a bad point) or
//! `backbox.walls[i]`.
//!
//! # Frames (DECISIONS.md "5.5 — Backbox vertical-plane convention")
//!
//! The **backbox frame** (`backbox.glb`, the `[backbox]` camera, metres in
//! the glb): origin at `origin`, `x` across (= table `x`), `z` world-up,
//! `y` horizontal into the backbox. The glass is the plane `y = 0`, the back
//! panel `y = depth`, the shelf `z = 0`, the roof `z = height`, the side
//! walls `x = ±width/2`. Plane coordinates `[u, v]` are `[x, z]`.
//!
//! The backbox stands vertical in the world, so in the **table frame** (`z`
//! the playfield normal, pitched by `meta.pitch_deg` = `p`) it is rotated by
//! `−p` about `x`: backbox `y` → table `(0, cos p, −sin p)`, backbox `z` →
//! table `(0, sin p, cos p)`. See [`BackboxFrame`]. Gravity along the
//! backbox plane is the full `g` and the plane normal is horizontal.
//!
//! # What is generated
//!
//! | element | `backbox.glb` (backbox frame) | colliders (table frame, layer `collision_layer`) | `table.toml` |
//! |---|---|---|---|
//! | back panel | `backbox.back` | `backbox.back` (wood), a 90° plane | — |
//! | glass | — (not drawn) | `backbox.glass` (acrylic), a 90° plane | — |
//! | side walls, roof | `backbox.walls[0/1]`, `backbox.roof` | same (wood) | — |
//! | shelf | `backbox.shelf[i]`: the shelf minus the entry and drain holes | same (wood) | — |
//! | `[[backbox.flippers]]` | `backbox.flippers[i]`: the bat at rest (a dynamic mesh) | — (moving body) | `[[flippers]]` `tablegen:backbox.flippers[i]` on the backbox plane |
//! | `[backbox.entry]` | — | — | `[[mechs]]` `tablegen:backbox.vuk` (kind `vuk`, layer 0, `exit_layer` = the backbox layer, `exit_offset` to the entry hole) and `[[layer_transitions]]` `tablegen:backbox.entry` (0 → layer, under the entry hole) |
//! | `[backbox.drain]` | — | — | `[[layer_transitions]]` `tablegen:backbox.drain` (layer → 0, under the drain hole) |
//! | `[[backbox.walls]]` | `backbox.walls[i].segments[j]`, one panel per polyline segment | same (wood) | — |
//!
//! With an `outline`, the back panel and the glass cover its bounding box
//! and the side walls, roof and shelf are replaced by one panel per outline
//! edge, `backbox.outline[i]` (edge `i` runs from point `i` to point
//! `i + 1`), standing on the outside of the edge. The entry and drain holes
//! are vertical slots: every downward-facing outline edge (interior above
//! it) loses its part over `at ± width/2`; a cut edge's pieces are
//! `backbox.outline[i].k`. Holes must lie within the outline's `u` range,
//! flippers' discs inside the polygon, and the VUK exit and transition
//! zones are placed from the outline's bottom over each hole.
//! Wall and edge panels are 5 mm thick, span glass to back panel and are
//! [`StaticShape::Cuboid`]s rotated in the backbox plane; a wall panel is
//! centred on its segment.
//!
//! The flippers' `surface_normal` is the backbox normal towards the glass,
//! `(0, −cos p, sin p)`; their bat angles use the usual plane axes
//! (`pin_physics::flipper::plane_axes`): 0° across (+x), 90° up the backbox.
//! Left flippers rest at −30° and sweep +50°, right ones at 210° and −50°.
//! Collider nodes of `backbox_colliders.glb` carry the `tablegen:` prefix,
//! `collider: true` and the backbox `collision_layer`.
//!
//! Generated `table.toml` entries have ids starting with
//! [`BACKBOX_PREFIX`]; a re-run drops them first. Run the backbox after
//! `layout` (whose re-run drops every `tablegen:` transition).

mod glb;
mod outline;
#[cfg(test)]
mod tests;

pub use outline::{
    MAX_OUTLINE_POINTS, MIN_OUTLINE_AREA_MM2, OUTLINE_FIT_TOLERANCE_MM, ZERO_SIZE_MM,
};

use crate::layout::{GENERATED_HEADER, GENERATED_PREFIX, GenError, MAX_COORD_MM, MAX_SIZE_MM};
use glam::{Quat, Vec3};
use glb::{FlatMaterial, Mesh, Node};
use pin_core::Material;
use pin_core::schema::table::{
    AabbDef, FlipperDef, FlipperShapeDef, FlipperStagingDef, LayerTransitionDef, MechDef, MechKind,
    TableManifest,
};
use pin_physics::world::{StaticShape, layers};
use serde::Deserialize;
use std::path::Path;

/// File name of the backbox layout block.
pub const BACKBOX_LAYOUT_FILE: &str = "backbox_layout.toml";
/// File name of the generated render scene.
pub const BACKBOX_SCENE_FILE: &str = "backbox.glb";
/// File name of the generated collision geometry (table frame).
pub const BACKBOX_COLLIDERS_FILE: &str = "backbox_colliders.glb";
/// Id prefix of every generated backbox `table.toml` entry.
pub const BACKBOX_PREFIX: &str = "tablegen:backbox.";
/// Default backbox collision layer (mask bit 4, PLAN §2.1).
pub const DEFAULT_BACKBOX_LAYER: u8 = 4;
/// Smallest glass-to-back depth, mm: a 27 mm ball plus 3 mm clearance.
pub const MIN_DEPTH_MM: f32 = 30.0;
/// Largest glass-to-back depth, mm.
pub const MAX_DEPTH_MM: f32 = 200.0;
/// Smallest backbox width and height, mm.
pub const MIN_EXTENT_MM: f32 = 100.0;
/// Smallest entry and drain hole width, mm.
pub const MIN_HOLE_MM: f32 = 30.0;
/// Default entry and drain hole width, mm.
pub const DEFAULT_HOLE_MM: f32 = 40.0;
/// Default mini flipper length, mm.
pub const DEFAULT_FLIPPER_LENGTH_MM: f32 = 50.0;
/// Mini flipper length range, mm.
pub const FLIPPER_LENGTH_MM: (f32, f32) = (30.0, 100.0);
/// Most backbox flippers.
pub const MAX_BACKBOX_FLIPPERS: usize = 8;
/// Panel thickness behind every collider face, mm.
pub const PANEL_MM: f32 = 5.0;
/// Largest `backbox_layout.toml` accepted, bytes.
pub const MAX_LAYOUT_BYTES: usize = 64 << 10;
/// Ball radius the entry geometry is sized for, mm.
const BALL_R_MM: f32 = 13.5;

fn default_layer() -> u8 {
    DEFAULT_BACKBOX_LAYER
}
fn default_hole() -> f32 {
    DEFAULT_HOLE_MM
}
fn default_flipper_length() -> f32 {
    DEFAULT_FLIPPER_LENGTH_MM
}

/// `backbox_layout.toml`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackboxLayoutFile {
    /// Must be 1.
    pub schema_version: u32,
    /// `[backbox]`.
    pub backbox: BackboxLayout,
}

/// `[backbox]`: the mini-playfield box, mm.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackboxLayout {
    /// Table-frame position of the bottom centre of the glass-side opening, mm.
    pub origin: [f32; 3],
    /// Width across, mm.
    pub width: f32,
    /// Height (world-vertical), mm.
    pub height: f32,
    /// Glass to back panel, mm.
    pub depth: f32,
    /// Collision layer index (1..=7, default 4).
    #[serde(default = "default_layer")]
    pub collision_layer: u8,
    /// `[[backbox.flippers]]`.
    #[serde(default)]
    pub flippers: Vec<BackboxFlipper>,
    /// `[backbox.entry]`.
    pub entry: BackboxEntry,
    /// `[backbox.drain]`.
    pub drain: BackboxDrain,
    /// Optional panel polygon, plane coordinates `[u, v]` in mm from
    /// `origin`: `u` across (table `x`, the `width` direction), `v` up the
    /// backbox (the `height` direction). Without it the panel is the box
    /// `u` in `±width/2`, `v` in `0..height`. With it, `width` and `height`
    /// must equal its bounding box.
    #[serde(default)]
    pub outline: Option<Vec<[f32; 2]>>,
    /// `[[backbox.walls]]`: optional polylines on the backbox plane, each
    /// segment a wall panel on the backbox collision layer.
    #[serde(default)]
    pub walls: Vec<BackboxWall>,
}

/// `[[backbox.walls]]`: one polyline wall standing between the glass and
/// the back panel.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackboxWall {
    /// The polyline, plane coordinates `[u, v]` in mm from `origin` (the
    /// axes of [`BackboxLayout::outline`]); 2..=1024 points, not all equal.
    pub points: Vec<[f32; 2]>,
}

/// Which way a backbox flipper points.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FlipperSide {
    /// Pivot on the left, rest −30°, sweep +50°.
    Left,
    /// Pivot on the right, rest 210°, sweep −50°.
    Right,
}

/// `[[backbox.flippers]]`.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackboxFlipper {
    /// Left or right.
    pub side: FlipperSide,
    /// Pivot in plane coordinates `[u, v]`, mm from `origin`.
    pub position: [f32; 2],
    /// Bat length, mm.
    #[serde(default = "default_flipper_length")]
    pub length: f32,
    /// Coil name (`table.toml`).
    pub coil: String,
    /// EOS switch name (`table.toml`).
    pub eos_switch: String,
    /// Input staging, as `[[flippers]]` `staging`.
    pub staging: FlipperStagingDef,
}

/// `[backbox.entry]`: the VUK-fed entry hole.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackboxEntry {
    /// Main-playfield `[x, y]` of the VUK scoop, mm.
    pub vuk: [f32; 2],
    /// `u` of the hole centre, mm.
    pub at: f32,
    /// Hole width, mm.
    #[serde(default = "default_hole")]
    pub width: f32,
    /// VUK coil name (`table.toml`).
    pub coil: String,
    /// VUK switch name (`table.toml`).
    pub switch: String,
}

/// `[backbox.drain]`: the drain hole back to the main playfield.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BackboxDrain {
    /// `u` of the hole centre, mm.
    pub at: f32,
    /// Hole width, mm.
    #[serde(default = "default_hole")]
    pub width: f32,
}

/// Placement of the backbox frame in the table frame (metres).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BackboxFrame {
    /// Table-frame position of the backbox origin, m.
    pub origin_m: Vec3,
    /// Backbox → table rotation (−pitch about `x`).
    pub rotation: Quat,
}

impl BackboxFrame {
    /// The frame at `origin_mm` (table frame) for a playfield pitched by
    /// `pitch_deg`.
    pub fn new(origin_mm: [f32; 3], pitch_deg: f32) -> Self {
        // −pitch about x, with `libm` for platform-independent bits (D9).
        let half = -0.5 * pitch_deg.to_radians();
        BackboxFrame {
            origin_m: Vec3::from(origin_mm) * 1.0e-3,
            rotation: Quat::from_xyzw(libm::sinf(half), 0.0, 0.0, libm::cosf(half)),
        }
    }

    /// A backbox-frame point (m) in the table frame (m).
    pub fn to_table(&self, p: Vec3) -> Vec3 {
        self.origin_m + self.rotation * p
    }

    /// The backbox plane normal towards the glass (the player), table frame.
    pub fn normal(&self) -> Vec3 {
        self.rotation * Vec3::NEG_Y
    }

    /// World-up along the backbox plane, table frame.
    pub fn up(&self) -> Vec3 {
        self.rotation * Vec3::Z
    }
}

/// One generated collider: a thin panel whose inner face is a plane of the
/// backbox, table frame, metres.
#[derive(Clone, Debug, PartialEq)]
pub struct BackboxCollider {
    /// Element name (`backbox.back`, `backbox.shelf[0]`, ...).
    pub name: String,
    /// The panel.
    pub shape: StaticShape,
    /// Collision layer mask (the backbox layer's bit only).
    pub layer_mask: u8,
    /// Contact material.
    pub material: Material,
}

/// The generated backbox.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratedBackbox {
    /// `backbox.glb` bytes (render scene, backbox frame).
    pub scene_glb: Vec<u8>,
    /// `backbox_colliders.glb` bytes (collider nodes, table frame).
    pub colliders_glb: Vec<u8>,
    /// The colliders as physics shapes (table frame).
    pub colliders: Vec<BackboxCollider>,
    /// The backbox frame.
    pub frame: BackboxFrame,
    /// Generated `[[flippers]]`.
    pub flippers: Vec<FlipperDef>,
    /// Generated `[[mechs]]` (the VUK).
    pub mechs: Vec<MechDef>,
    /// Generated `[[layer_transitions]]`.
    pub layer_transitions: Vec<LayerTransitionDef>,
    /// The merged manifest (equal to `table_toml` parsed).
    pub table: TableManifest,
    /// `table.toml` text.
    pub table_toml: String,
}

/// Parses `backbox_layout.toml` text. Errors name the file.
pub fn parse_backbox_layout(text: &str) -> Result<BackboxLayoutFile, GenError> {
    let err = |m: String| gen_err(BACKBOX_LAYOUT_FILE, m);
    if text.len() > MAX_LAYOUT_BYTES {
        return Err(err(format!(
            "{} bytes, more than {MAX_LAYOUT_BYTES}",
            text.len()
        )));
    }
    let file: BackboxLayoutFile =
        toml::from_str(text).map_err(|e| err(e.to_string().trim_end().to_owned()))?;
    if file.schema_version != 1 {
        return Err(err(format!(
            "unsupported schema_version {} (expected 1)",
            file.schema_version
        )));
    }
    Ok(file)
}

/// Parses `backbox_layout.toml` bytes (UTF-8 required).
pub fn parse_backbox_layout_bytes(bytes: &[u8]) -> Result<BackboxLayoutFile, GenError> {
    let text = core::str::from_utf8(bytes)
        .map_err(|e| gen_err(BACKBOX_LAYOUT_FILE, format!("not valid UTF-8 text: {e}")))?;
    parse_backbox_layout(text)
}

fn gen_err(element: impl Into<String>, message: impl Into<String>) -> GenError {
    GenError {
        element: element.into(),
        message: message.into(),
    }
}

fn check_range(element: &str, what: &str, v: f32, lo: f32, hi: f32) -> Result<(), GenError> {
    if v.is_finite() && v >= lo && v <= hi {
        Ok(())
    } else {
        Err(gen_err(
            element,
            format!("{what} {v} must be finite and within {lo}..={hi} mm"),
        ))
    }
}

fn check_name<'a>(
    element: &str,
    what: &str,
    name: &str,
    mut known: impl Iterator<Item = &'a str>,
) -> Result<(), GenError> {
    if known.any(|k| k == name) {
        Ok(())
    } else {
        Err(gen_err(
            element,
            format!("{what} {name:?} is not declared in table.toml"),
        ))
    }
}

/// A hole `[at ± width/2]` that fits inside the shelf.
fn check_hole(element: &str, at: f32, width: f32, half_w: f32) -> Result<(f32, f32), GenError> {
    check_range(element, "width", width, MIN_HOLE_MM, 2.0 * half_w)?;
    check_range(element, "at", at, -half_w, half_w)?;
    let (a, b) = (at - 0.5 * width, at + 0.5 * width);
    if a < -half_w || b > half_w {
        return Err(gen_err(
            element,
            format!("the hole {a}..{b} mm reaches past the side walls (±{half_w} mm)"),
        ));
    }
    Ok((a, b))
}

/// A hole `[at ± width/2]` within the outline's `u` range `lo..=hi`.
fn check_hole_in(
    element: &str,
    at: f32,
    width: f32,
    lo: f32,
    hi: f32,
) -> Result<(f32, f32), GenError> {
    check_range(element, "width", width, MIN_HOLE_MM, hi - lo)?;
    check_range(element, "at", at, lo, hi)?;
    let (a, b) = (at - 0.5 * width, at + 0.5 * width);
    if a < lo || b > hi {
        return Err(gen_err(
            element,
            format!("the hole {a}..{b} mm reaches past the outline (u {lo}..{hi} mm)"),
        ));
    }
    Ok((a, b))
}

/// A panel standing on the in-plane segment `a b` (plane coordinates, mm),
/// spanning glass to back, across the segment from `band.0` to `band.1` mm
/// along its left normal.
struct SegmentPanel {
    name: String,
    a: [f32; 2],
    b: [f32; 2],
    band: (f32, f32),
}

impl SegmentPanel {
    /// Centre (m, backbox frame), half extents (m) and the rotation about
    /// the backbox `y` axis taking local `x` along the segment and local
    /// `z` to its left normal; `None` for a zero-length segment.
    fn placement(&self, depth_mm: f32) -> Option<(Vec3, Vec3, Quat)> {
        let (du, dv) = (self.b[0] - self.a[0], self.b[1] - self.a[1]);
        let len = libm::sqrtf(du * du + dv * dv);
        if len.is_nan() || len <= 1.0e-3 {
            return None;
        }
        // Rotation by −θ about y maps x to (cos θ, 0, sin θ) and z to
        // (−sin θ, 0, cos θ): along and left of the segment in (u, v) = (x, z).
        let half_angle = -0.5 * libm::atan2f(dv, du);
        let rot = Quat::from_xyzw(0.0, libm::sinf(half_angle), 0.0, libm::cosf(half_angle));
        let across = 0.5 * (self.band.0 + self.band.1);
        let mid = Vec3::new(
            0.5 * (self.a[0] + self.b[0]),
            0.5 * depth_mm,
            0.5 * (self.a[1] + self.b[1]),
        );
        let center = (mid + rot * Vec3::new(0.0, 0.0, across)) * 1.0e-3;
        let half = Vec3::new(0.5 * len, 0.5 * depth_mm, 0.5 * (self.band.1 - self.band.0)) * 1.0e-3;
        Some((center, half, rot))
    }
}

/// Material indices of the scene.
const MAT_WOOD: usize = 0;
const MAT_BAT: usize = 1;
const MAT_ACRYLIC: usize = 2;

fn materials() -> [FlatMaterial; 3] {
    [
        FlatMaterial {
            name: "wood",
            base_color: [0.58, 0.37, 0.18, 1.0],
            metallic: 0.0,
            roughness: 0.55,
        },
        FlatMaterial {
            name: "rubber",
            base_color: [0.9, 0.9, 0.86, 1.0],
            metallic: 0.0,
            roughness: 0.6,
        },
        FlatMaterial {
            name: "acrylic",
            base_color: [0.85, 0.92, 1.0, 0.3],
            metallic: 0.0,
            roughness: 0.05,
        },
    ]
}

/// A box mesh `a..b` with outward faces.
fn box_mesh(a: Vec3, b: Vec3) -> Mesh {
    let p = |x: f32, y: f32, z: f32| [x, y, z];
    let mut m = Mesh::default();
    m.quad(
        p(a.x, a.y, a.z),
        p(a.x, a.y, b.z),
        p(a.x, b.y, b.z),
        p(a.x, b.y, a.z),
        [-1.0, 0.0, 0.0],
    );
    m.quad(
        p(b.x, a.y, a.z),
        p(b.x, b.y, a.z),
        p(b.x, b.y, b.z),
        p(b.x, a.y, b.z),
        [1.0, 0.0, 0.0],
    );
    m.quad(
        p(a.x, a.y, a.z),
        p(b.x, a.y, a.z),
        p(b.x, a.y, b.z),
        p(a.x, a.y, b.z),
        [0.0, -1.0, 0.0],
    );
    m.quad(
        p(a.x, b.y, a.z),
        p(a.x, b.y, b.z),
        p(b.x, b.y, b.z),
        p(b.x, b.y, a.z),
        [0.0, 1.0, 0.0],
    );
    m.quad(
        p(a.x, a.y, a.z),
        p(a.x, b.y, a.z),
        p(b.x, b.y, a.z),
        p(b.x, a.y, a.z),
        [0.0, 0.0, -1.0],
    );
    m.quad(
        p(a.x, a.y, b.z),
        p(b.x, a.y, b.z),
        p(b.x, b.y, b.z),
        p(a.x, b.y, b.z),
        [0.0, 0.0, 1.0],
    );
    m
}

/// `m` carried into the table frame by `frame`.
fn to_table(frame: &BackboxFrame, m: &Mesh) -> Mesh {
    Mesh {
        positions: m
            .positions
            .iter()
            .map(|&p| frame.to_table(Vec3::from(p)).to_array())
            .collect(),
        normals: m
            .normals
            .iter()
            .map(|&n| (frame.rotation * Vec3::from(n)).to_array())
            .collect(),
        indices: m.indices.clone(),
    }
}

/// The table-frame axis-aligned bounds (mm) of a backbox-frame box (m).
fn aabb_mm(frame: &BackboxFrame, lo: Vec3, hi: Vec3) -> AabbDef {
    let mut min = Vec3::splat(f32::INFINITY);
    let mut max = Vec3::splat(f32::NEG_INFINITY);
    for k in 0..8 {
        let c = Vec3::new(
            if k & 1 == 0 { lo.x } else { hi.x },
            if k & 2 == 0 { lo.y } else { hi.y },
            if k & 4 == 0 { lo.z } else { hi.z },
        );
        let t = frame.to_table(c) * 1.0e3;
        min = min.min(t);
        max = max.max(t);
    }
    AabbDef {
        min: min.to_array(),
        max: max.to_array(),
    }
}

/// A straight bat outline: tapered capsule of `length` with base radius `rb`
/// and tip radius `rt`, pivot `[u, v]`, at `angle` rad; metres.
struct BatShape {
    pivot: [f32; 2],
    angle: f32,
    length: f32,
    rb: f32,
    rt: f32,
}

/// The bat of `shape` standing out of the back panel (`y = depth`) towards
/// the glass by `height`; metres, backbox frame.
fn bat_mesh(shape: &BatShape, depth: f32, height: f32) -> Mesh {
    const ARC: usize = 8;
    let BatShape {
        pivot,
        angle,
        length,
        rb,
        rt,
    } = *shape;
    let alpha = libm::acosf(((rb - rt) / length).clamp(-1.0, 1.0));
    let (s, c) = (libm::sinf(angle), libm::cosf(angle));
    let rot = |x: f32, y: f32| [pivot[0] + c * x - s * y, pivot[1] + s * x + c * y];
    let mut outline: Vec<[f32; 2]> = Vec::with_capacity(2 * ARC + 2);
    // Tip arc −α..α, then base arc α..2π−α: counter-clockwise in (u, v).
    for k in 0..=ARC {
        let t = -alpha + 2.0 * alpha * k as f32 / ARC as f32;
        outline.push(rot(length + rt * libm::cosf(t), rt * libm::sinf(t)));
    }
    let span = 2.0 * core::f32::consts::PI - 2.0 * alpha;
    for k in 0..=ARC {
        let t = alpha + span * k as f32 / ARC as f32;
        outline.push(rot(rb * libm::cosf(t), rb * libm::sinf(t)));
    }
    let front = depth - height; // the face towards the glass
    let at = |q: [f32; 2], y: f32| [q[0], y, q[1]];
    let mut m = Mesh::default();
    let n = outline.len();
    for i in 0..n {
        let (a, b) = (outline[i], outline[(i + 1) % n]);
        let (du, dv) = (b[0] - a[0], b[1] - a[1]);
        let len = libm::sqrtf(du * du + dv * dv).max(1e-9);
        // Outward normal of a counter-clockwise edge in (u, v) = (x, z).
        m.quad(
            at(a, depth),
            at(b, depth),
            at(b, front),
            at(a, front),
            [dv / len, 0.0, -du / len],
        );
    }
    let cu = outline.iter().map(|q| q[0]).sum::<f32>() / n as f32;
    let cv = outline.iter().map(|q| q[1]).sum::<f32>() / n as f32;
    let base = m.positions.len() as u32;
    m.positions.push([cu, front, cv]);
    m.normals.push([0.0, -1.0, 0.0]);
    for q in &outline {
        m.positions.push(at(*q, front));
        m.normals.push([0.0, -1.0, 0.0]);
    }
    for i in 0..n as u32 {
        let j = (i + 1) % n as u32;
        m.indices
            .extend_from_slice(&[base, base + 1 + i, base + 1 + j]);
    }
    m
}

/// Generates the backbox of `layout` against the base `table.toml`.
pub fn generate_backbox(
    layout: &BackboxLayout,
    base: &TableManifest,
) -> Result<GeneratedBackbox, GenError> {
    let e = "backbox";
    for (k, &c) in layout.origin.iter().enumerate() {
        check_range(
            "backbox.origin",
            &format!("[{k}]"),
            c,
            -MAX_COORD_MM,
            MAX_COORD_MM,
        )?;
    }
    // The outline and walls first: their errors name them, not the box.
    let outline = match &layout.outline {
        Some(pts) => Some(outline::check_outline(pts)?),
        None => None,
    };
    outline::check_walls(&layout.walls)?;
    check_range(e, "width", layout.width, MIN_EXTENT_MM, MAX_SIZE_MM)?;
    check_range(e, "height", layout.height, MIN_EXTENT_MM, MAX_SIZE_MM)?;
    check_range(e, "depth", layout.depth, MIN_DEPTH_MM, MAX_DEPTH_MM)?;
    if let Some(o) = &outline {
        o.check_fits(layout.width, layout.height)?;
    }
    let layer = layout.collision_layer;
    if !(1..=7).contains(&layer) {
        return Err(gen_err(
            "backbox.collision_layer",
            format!("layer {layer} must be 1..=7 (0 is the main deck)"),
        ));
    }
    let pitch = base.meta.pitch_deg;
    if !(pitch.is_finite() && (0.0..=45.0).contains(&pitch)) {
        return Err(gen_err(
            "table.toml meta.pitch_deg",
            format!("pitch {pitch} degrees must be finite and within 0..=45"),
        ));
    }
    let frame = BackboxFrame::new(layout.origin, pitch);
    let mm = |v: f32| v * 1.0e-3;
    let (half_w, h, d, t) = (0.5 * layout.width, layout.height, layout.depth, PANEL_MM);
    let coils = || base.coils.iter().map(|c| c.id.as_str());
    let switches = || base.switches.iter().map(|s| s.id.as_str());

    // Holes.
    let ee = "backbox.entry";
    for (k, &c) in layout.entry.vuk.iter().enumerate() {
        check_range(ee, &format!("vuk[{k}]"), c, -MAX_COORD_MM, MAX_COORD_MM)?;
    }
    let hole = |element: &str, at: f32, width: f32| match &outline {
        None => check_hole(element, at, width, half_w),
        Some(o) => check_hole_in(element, at, width, o.lo[0], o.hi[0]),
    };
    let entry = hole(ee, layout.entry.at, layout.entry.width)?;
    check_name(ee, "coil", &layout.entry.coil, coils())?;
    check_name(ee, "switch", &layout.entry.switch, switches())?;
    let de = "backbox.drain";
    let drain = hole(de, layout.drain.at, layout.drain.width)?;
    if entry.0 < drain.1 && drain.0 < entry.1 {
        return Err(gen_err(
            de,
            format!(
                "the drain hole {}..{} mm overlaps the entry hole {}..{} mm",
                drain.0, drain.1, entry.0, entry.1
            ),
        ));
    }

    // Panels: (name, lo, hi, material, drawn), backbox frame, mm.
    let v3 = Vec3::new;
    let (first, second) = if entry.0 < drain.0 {
        (entry, drain)
    } else {
        (drain, entry)
    };
    let mut panels: Vec<(String, Vec3, Vec3, Material, bool)>;
    // Rotated in-plane panels (outline edges, walls).
    let mut segments: Vec<SegmentPanel> = Vec::new();
    match &outline {
        None => {
            panels = vec![
                (
                    "backbox.back".into(),
                    v3(-half_w, d, 0.0),
                    v3(half_w, d + t, h),
                    Material::Wood,
                    true,
                ),
                (
                    "backbox.glass".into(),
                    v3(-half_w, -t, 0.0),
                    v3(half_w, 0.0, h),
                    Material::Acrylic,
                    false,
                ),
                (
                    "backbox.walls[0]".into(),
                    v3(-half_w - t, 0.0, 0.0),
                    v3(-half_w, d, h),
                    Material::Wood,
                    true,
                ),
                (
                    "backbox.walls[1]".into(),
                    v3(half_w, 0.0, 0.0),
                    v3(half_w + t, d, h),
                    Material::Wood,
                    true,
                ),
                (
                    "backbox.roof".into(),
                    v3(-half_w - t, 0.0, h),
                    v3(half_w + t, d, h + t),
                    Material::Wood,
                    true,
                ),
            ];
            let mut shelf = 0;
            for (a, b) in [(-half_w, first.0), (first.1, second.0), (second.1, half_w)] {
                if b - a > 1.0e-3 {
                    panels.push((
                        format!("backbox.shelf[{shelf}]"),
                        v3(a, 0.0, -t),
                        v3(b, d, 0.0),
                        Material::Wood,
                        true,
                    ));
                    shelf += 1;
                }
            }
        }
        Some(o) => {
            panels = vec![
                (
                    "backbox.back".into(),
                    v3(o.lo[0], d, o.lo[1]),
                    v3(o.hi[0], d + t, o.hi[1]),
                    Material::Wood,
                    true,
                ),
                (
                    "backbox.glass".into(),
                    v3(o.lo[0], -t, o.lo[1]),
                    v3(o.hi[0], 0.0, o.hi[1]),
                    Material::Acrylic,
                    false,
                ),
            ];
            // Outside of each edge: the interior is on the left of a
            // counter-clockwise edge (local +z of its panel).
            let band = if o.ccw { (-t, 0.0) } else { (0.0, t) };
            for i in 0..o.len() {
                let pieces = o.edge_pieces(i, &[first, second]);
                let split = pieces.len() > 1;
                for (k, (a, b)) in pieces.into_iter().enumerate() {
                    segments.push(SegmentPanel {
                        name: if split {
                            format!("backbox.outline[{i}].{k}")
                        } else {
                            format!("backbox.outline[{i}]")
                        },
                        a,
                        b,
                        band,
                    });
                }
            }
        }
    }
    for (i, w) in layout.walls.iter().enumerate() {
        for (j, pair) in w.points.windows(2).enumerate() {
            segments.push(SegmentPanel {
                name: format!("backbox.walls[{i}].segments[{j}]"),
                a: pair[0],
                b: pair[1],
                band: (-0.5 * t, 0.5 * t),
            });
        }
    }

    let mask = layers::layer_bit(layer);
    let mut scene_nodes = Vec::new();
    let mut collider_nodes = Vec::new();
    let mut colliders = Vec::new();
    for (name, lo, hi, material, drawn) in &panels {
        let (lo_m, hi_m) = (*lo * 1.0e-3, *hi * 1.0e-3);
        let local = box_mesh(lo_m, hi_m);
        let mat = if *material == Material::Acrylic {
            MAT_ACRYLIC
        } else {
            MAT_WOOD
        };
        if *drawn {
            scene_nodes.push(Node {
                name: name.clone(),
                mesh: local.clone(),
                material: mat,
                collider: false,
                collision_layer: layer,
                strings: Vec::new(),
            });
        }
        collider_nodes.push(Node {
            name: format!("{GENERATED_PREFIX}{name}"),
            mesh: to_table(&frame, &local),
            material: mat,
            collider: true,
            collision_layer: layer,
            strings: Vec::new(),
        });
        colliders.push(BackboxCollider {
            name: name.clone(),
            shape: StaticShape::Cuboid {
                center: frame.to_table(0.5 * (lo_m + hi_m)),
                half_extents: 0.5 * (hi_m - lo_m),
                rotation: frame.rotation,
            },
            layer_mask: mask,
            material: *material,
        });
    }
    for seg in &segments {
        let Some((center, half, rot)) = seg.placement(d) else {
            continue; // a zero-length wall segment (repeated point)
        };
        let local = box_mesh(-half, half);
        let placed = Mesh {
            positions: local
                .positions
                .iter()
                .map(|&p| (center + rot * Vec3::from(p)).to_array())
                .collect(),
            normals: local
                .normals
                .iter()
                .map(|&n| (rot * Vec3::from(n)).to_array())
                .collect(),
            indices: local.indices,
        };
        scene_nodes.push(Node {
            name: seg.name.clone(),
            mesh: placed.clone(),
            material: MAT_WOOD,
            collider: false,
            collision_layer: layer,
            strings: Vec::new(),
        });
        collider_nodes.push(Node {
            name: format!("{GENERATED_PREFIX}{}", seg.name),
            mesh: to_table(&frame, &placed),
            material: MAT_WOOD,
            collider: true,
            collision_layer: layer,
            strings: Vec::new(),
        });
        colliders.push(BackboxCollider {
            name: seg.name.clone(),
            shape: StaticShape::Cuboid {
                center: frame.to_table(center),
                half_extents: half,
                rotation: frame.rotation * rot,
            },
            layer_mask: mask,
            material: Material::Wood,
        });
    }

    // Flippers.
    if layout.flippers.len() > MAX_BACKBOX_FLIPPERS {
        return Err(gen_err(
            "backbox.flippers",
            format!(
                "{} flippers, at most {MAX_BACKBOX_FLIPPERS}",
                layout.flippers.len()
            ),
        ));
    }
    let normal = frame.normal();
    let mut flippers = Vec::new();
    for (i, f) in layout.flippers.iter().enumerate() {
        let fe = format!("backbox.flippers[{i}]");
        check_range(
            &fe,
            "length",
            f.length,
            FLIPPER_LENGTH_MM.0,
            FLIPPER_LENGTH_MM.1,
        )?;
        let (u_range, v_range) = match &outline {
            None => ((-half_w, half_w), (0.0, h)),
            Some(o) => ((o.lo[0], o.hi[0]), (o.lo[1], o.hi[1])),
        };
        check_range(&fe, "position[0]", f.position[0], u_range.0, u_range.1)?;
        check_range(&fe, "position[1]", f.position[1], v_range.0, v_range.1)?;
        let (rest, sweep) = match f.side {
            FlipperSide::Left => (-30.0f32, 50.0f32),
            FlipperSide::Right => (210.0, -50.0),
        };
        let (rb, rt) = (0.145 * f.length, 0.085 * f.length);
        // The whole stroke must stay inside the box.
        for deg in [rest, rest + 0.5 * sweep, rest + sweep] {
            let a = deg.to_radians();
            let tip = [
                f.position[0] + f.length * libm::cosf(a),
                f.position[1] + f.length * libm::sinf(a),
            ];
            for (q, r) in [(f.position, rb), (tip, rt)] {
                let outside = match &outline {
                    None => {
                        q[0] - r < -half_w || q[0] + r > half_w || q[1] - r < 0.0 || q[1] + r > h
                    }
                    Some(o) => !o.contains_disc(q, r),
                };
                if outside {
                    return Err(gen_err(
                        &fe,
                        format!(
                            "the bat reaches outside the backbox at {deg} degrees ({:.1}, {:.1}) mm",
                            q[0], q[1]
                        ),
                    ));
                }
            }
        }
        check_name(&fe, "coil", &f.coil, coils())?;
        check_name(&fe, "eos_switch", &f.eos_switch, switches())?;
        if let FlipperStagingDef::AutoSensorThingFlip { trigger_switch, .. } = &f.staging {
            check_name(&fe, "trigger_switch", trigger_switch, switches())?;
        }
        let pivot = frame.to_table(Vec3::new(mm(f.position[0]), mm(d), mm(f.position[1])));
        let id = format!("{BACKBOX_PREFIX}flippers[{i}]");
        flippers.push(FlipperDef {
            id: id.clone(),
            coil: f.coil.clone(),
            eos_switch: f.eos_switch.clone(),
            collision_layer: layer,
            base_position: (pivot * 1.0e3).to_array(),
            surface_normal: normal.to_array(),
            rest_angle_deg: rest,
            sweep_angle_deg: sweep,
            eos_angle_fraction: 0.92,
            shape: FlipperShapeDef::StraightCapsule {
                length: f.length,
                base_radius: rb,
                tip_radius: rt,
            },
            staging: f.staging.clone(),
            inertia_kg_m2: None,
            power_torque_nm: None,
            hold_torque_nm: None,
            return_spring_torque_nm: None,
            stop_restitution: None,
            rubber_restitution: None,
            rubber_damping: None,
            zipper_slide_axis: None,
            zipper_slide_speed: None,
        });
        let shape = BatShape {
            pivot: [mm(f.position[0]), mm(f.position[1])],
            angle: rest.to_radians(),
            length: mm(f.length),
            rb: mm(rb),
            rt: mm(rt),
        };
        scene_nodes.push(Node {
            name: format!("backbox.flippers[{i}]"),
            mesh: bat_mesh(&shape, mm(d), mm((d - 1.0).min(22.0))),
            material: MAT_BAT,
            collider: false,
            collision_layer: layer,
            strings: vec![("flipper", id)],
        });
    }

    // VUK feed and transitions.
    let slot = mm(d - BALL_R_MM);
    // Heights of the bottom (mm): 0 for the box shelf.
    let (entry_floor, entry_low, drain_low) = match &outline {
        None => (0.0, 0.0, 0.0),
        Some(o) => (
            o.floor_at(0.5 * (entry.0 + entry.1)),
            o.lowest_floor(entry),
            o.lowest_floor(drain),
        ),
    };
    let entry_mid = mm(0.5 * (entry.0 + entry.1));
    let exit = frame.to_table(Vec3::new(
        entry_mid,
        slot,
        mm(BALL_R_MM + 2.0 + entry_floor),
    )) * 1.0e3;
    let vuk = Vec3::new(layout.entry.vuk[0], layout.entry.vuk[1], 0.0);
    let mechs = vec![MechDef {
        id: format!("{BACKBOX_PREFIX}vuk"),
        kind: MechKind::Vuk,
        coil: Some(layout.entry.coil.clone()),
        switch: Some(layout.entry.switch.clone()),
        position: Some(vuk.to_array()),
        angle_deg: None,
        layer: Some(0),
        radius: None,
        length: None,
        height: None,
        thickness: None,
        kick_speed_m_s: None,
        kick_reach: None,
        mass_g: None,
        force_n: None,
        stroke: None,
        spring_k_n_m: None,
        threshold_speed_m_s: None,
        inertia_kg_m2: None,
        damping_per_s: None,
        eject_speed_m_s: None,
        exit_layer: Some(layer),
        exit_offset: Some((exit - vuk).to_array()),
        auto_pull: None,
    }];
    let layer_transitions = vec![
        LayerTransitionDef {
            id: format!("{BACKBOX_PREFIX}entry"),
            from_layer: 0,
            to_layer: layer,
            zone: aabb_mm(
                &frame,
                Vec3::new(mm(entry.0), 0.0, mm(-60.0 + entry_low)),
                Vec3::new(mm(entry.1), mm(d), mm(-20.0 + entry_low)),
            ),
        },
        LayerTransitionDef {
            id: format!("{BACKBOX_PREFIX}drain"),
            from_layer: layer,
            to_layer: 0,
            zone: aabb_mm(
                &frame,
                Vec3::new(mm(drain.0), 0.0, mm(-80.0 + drain_low)),
                Vec3::new(mm(drain.1), mm(d), mm(-30.0 + drain_low)),
            ),
        },
    ];

    // Merge into the base manifest.
    let mut table = base.clone();
    table.flippers.retain(|f| !f.id.starts_with(BACKBOX_PREFIX));
    table.mechs.retain(|m| !m.id.starts_with(BACKBOX_PREFIX));
    table
        .layer_transitions
        .retain(|t| !t.id.starts_with(BACKBOX_PREFIX));
    table.flippers.extend(flippers.iter().cloned());
    table.mechs.extend(mechs.iter().cloned());
    table
        .layer_transitions
        .extend(layer_transitions.iter().cloned());
    let body = table
        .to_toml_string()
        .map_err(|err| gen_err("table.toml", err.to_string()))?;
    let table_toml = format!(
        "{GENERATED_HEADER}\
         # Entries whose id starts with \"{GENERATED_PREFIX}\" are replaced on every run.\n\n{body}"
    );
    let table = TableManifest::from_toml_str(&table_toml)
        .map_err(|err| gen_err("table.toml", format!("does not parse back: {err}")))?;
    let mats = materials();
    Ok(GeneratedBackbox {
        scene_glb: glb::write("backbox", &scene_nodes, &mats),
        colliders_glb: glb::write("backbox_colliders", &collider_nodes, &mats),
        colliders,
        frame,
        flippers,
        mechs,
        layer_transitions,
        table,
        table_toml,
    })
}

/// Writes `backbox.glb`, `backbox_colliders.glb` and `table.toml` into `dir`
/// (created if needed). Each file is written under a temporary name and
/// renamed over the old one.
pub fn write_backbox(generated: &GeneratedBackbox, dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, bytes) in [
        (BACKBOX_SCENE_FILE, generated.scene_glb.as_slice()),
        (BACKBOX_COLLIDERS_FILE, generated.colliders_glb.as_slice()),
        (crate::layout::TABLE_FILE, generated.table_toml.as_bytes()),
    ] {
        let tmp = dir.join(format!(".{name}.tablegen-tmp"));
        std::fs::write(&tmp, bytes)?;
        if let Err(err) = std::fs::rename(&tmp, dir.join(name)) {
            let _ = std::fs::remove_file(&tmp);
            return Err(err);
        }
    }
    Ok(())
}
