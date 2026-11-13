//! `layout.toml` to `.glb` generation.
//!
//! Owner: Subphase 4.3.
//!
//! [`generate`] turns a declarative [`LayoutManifest`] (PLAN.md Appendix A.3)
//! and a hand-written base [`TableManifest`] (switches, coils, lamps, rules,
//! splines) into a bundle's `playfield.glb` and the matching `table.toml`;
//! [`write_bundle`] writes both into a bundle folder. The same inputs always
//! give byte-identical outputs.
//!
//! # What is generated
//!
//! The glTF uses the table frame (`x` across, `y` up the table, `z` the
//! playfield normal) in **metres**, one node per element with its own mesh
//! (one triangle primitive), identity transforms.
//!
//! | layout element | node(s) | collider | `table.toml` |
//! |---|---|---|---|
//! | `[playfield]` | `floor`: the playfield rectangle at z = 0 as one triangle mesh (layer 0, material `wood`) | yes | — |
//! | `[[decks]]` | `decks[i]`: the outline at `z`, one triangle mesh, layer `layer` | yes | — |
//! | `[[walls]]` | `walls[i]`: the polyline extruded from z = 0 to `wall_height` (zero-thickness, two-sided) | yes | — |
//! | `[[posts]]` | `posts[i]`: a cylinder to `wall_height`, material `rubber` or `steel` | yes | — |
//! | `[[lanes]]` | `tablegen:lanes[i].left` / `.right`: visual walls of the two rails | no | two level guide `[[splines]]` with those ids |
//! | `[[ramps]]` | `ramps[i]`: visual rails (wireform) or wall (guide) of the referenced spline | no | a `[[layer_transitions]]` entry `tablegen:ramps[i].entry` when the spline's layer is not 0 |
//! | `[[inserts]]` | `inserts[i]`: the shape 0.1 mm above the floor, extras `lamp` | no | — |
//!
//! Collider nodes are their own collision geometry (a two-sided triangle
//! mesh): extras `collider: true`, `collision_layer`, and `material` (the
//! physics material name, also the glTF material's name). Guide and ramp
//! nodes are visual only (`collider: false`); their collision shape is the
//! spline the loader builds from `table.toml`, and their extras name it
//! (`guide` / `spline`). Visual surfaces stay within 0.02 mm of the
//! collision shape (sampled splines, faceted posts).
//!
//! Generated `table.toml` entries have ids starting with
//! [`GENERATED_PREFIX`]; a re-run (with the generated `table.toml` as the
//! base) drops them first, so they are replaced, never duplicated.
//! `meta.playfield_mesh` is set to `playfield.glb`.
//!
//! A layout that cannot be generated validly is an error naming the element
//! (`walls[2]`, `lanes[0]`, ...); nothing is written.

mod cli;
mod geom;
mod glb;
mod guides;
mod materials;
mod mesh;

pub use cli::{CliError, run_cli};

use core::fmt;
use glam::Vec3;
use pin_core::Material;
use pin_core::schema::LayoutManifest;
use pin_core::schema::layout::{InsertShape, LaneKind};
use pin_core::schema::table::{AabbDef, LayerTransitionDef, SplineDef, SplineKind, TableManifest};
use pin_core::units::{mm_to_m, mm3_to_m};
use pin_physics::world::{CubicSpline, GuideCurve, Wireform};
use std::path::Path;

use glb::{Extras, FlatMaterial, Node};
use mesh::Mesh;

/// Id prefix of every generated `table.toml` entry.
pub const GENERATED_PREFIX: &str = "tablegen:";
/// File name of the generated playfield glTF.
pub const PLAYFIELD_FILE: &str = "playfield.glb";
/// File name of the generated manifest.
pub const TABLE_FILE: &str = "table.toml";
/// Largest coordinate (absolute, mm) of any layout point, and largest
/// playfield width or length.
pub const MAX_COORD_MM: f32 = 5000.0;
/// Largest wall height, deck height, post radius, lane width, insert size
/// and ramp entry width, mm.
pub const MAX_SIZE_MM: f32 = 1000.0;
/// Most points of one wall, lane or deck outline.
pub const MAX_ELEMENT_POINTS: usize = 1024;
/// Height of an insert above its floor, mm.
pub const INSERT_LIFT_MM: f32 = 0.1;
/// Radius of a visual wireform rail, mm.
const WIRE_RADIUS_MM: f32 = 1.5;
/// Sides of a visual wireform rail.
const WIRE_SIDES: usize = 8;

/// A generated bundle: `playfield.glb` and `table.toml`.
#[derive(Clone, Debug, PartialEq)]
pub struct GeneratedTable {
    /// `playfield.glb` bytes.
    pub glb: Vec<u8>,
    /// The merged manifest (equal to `table_toml` parsed).
    pub table: TableManifest,
    /// `table.toml` text.
    pub table_toml: String,
}

/// Why a layout cannot be generated: the offending element (a TOML path of
/// `layout.toml` such as `walls[2]`, or `ramps[0]` for a missing spline)
/// and what is wrong.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GenError {
    /// Layout element, e.g. `walls[2]` or `playfield`.
    pub element: String,
    /// What is wrong.
    pub message: String,
}

impl GenError {
    fn new(element: impl Into<String>, message: impl Into<String>) -> Self {
        GenError {
            element: element.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for GenError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.element, self.message)
    }
}

impl std::error::Error for GenError {}

/// Parses `layout.toml` text (the CLI's parse path). Errors name the file.
pub fn parse_layout(text: &str) -> Result<LayoutManifest, GenError> {
    LayoutManifest::from_toml_str(text).map_err(|e| GenError::new("layout.toml", e.to_string()))
}

/// Parses `layout.toml` bytes (UTF-8 required).
pub fn parse_layout_bytes(bytes: &[u8]) -> Result<LayoutManifest, GenError> {
    let text = core::str::from_utf8(bytes)
        .map_err(|e| GenError::new("layout.toml", format!("not valid UTF-8 text: {e}")))?;
    parse_layout(text)
}

/// Writes `playfield.glb` and `table.toml` into `dir` (created if needed).
/// Each file is written to a temporary name first and renamed into place.
pub fn write_bundle(generated: &GeneratedTable, dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    for (name, bytes) in [
        (PLAYFIELD_FILE, generated.glb.as_slice()),
        (TABLE_FILE, generated.table_toml.as_bytes()),
    ] {
        let tmp = dir.join(format!(".{name}.tablegen-tmp"));
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, dir.join(name))?;
    }
    Ok(())
}

/// Checks a finite value lies in `lo..=hi` (`lo` exclusive when
/// `positive`).
fn in_range(element: &str, what: &str, v: f32, positive: bool, hi: f32) -> Result<(), GenError> {
    let ok = v.is_finite() && v <= hi && if positive { v > 0.0 } else { v >= -hi };
    if ok {
        Ok(())
    } else if positive {
        Err(GenError::new(
            element,
            format!("{what} must be a number above 0 and at most {hi} mm, got {v}"),
        ))
    } else {
        Err(GenError::new(
            element,
            format!("{what} must lie within ±{hi} mm, got {v}"),
        ))
    }
}

fn check_point(element: &str, what: &str, p: [f32; 2]) -> Result<(), GenError> {
    in_range(element, what, p[0], false, MAX_COORD_MM)?;
    in_range(element, what, p[1], false, MAX_COORD_MM)
}

/// Checks a point list: count, range, no point repeating the previous one.
fn check_points(element: &str, points: &[[f32; 2]], min: usize) -> Result<(), GenError> {
    if points.len() < min {
        return Err(GenError::new(
            element,
            format!("needs at least {min} points, got {}", points.len()),
        ));
    }
    if points.len() > MAX_ELEMENT_POINTS {
        return Err(GenError::new(
            element,
            format!(
                "has {} points, more than the {MAX_ELEMENT_POINTS} allowed",
                points.len()
            ),
        ));
    }
    for (j, &p) in points.iter().enumerate() {
        check_point(element, &format!("points[{j}]"), p)?;
        if j > 0 && p == points[j - 1] {
            return Err(GenError::new(
                element,
                format!("points[{j}] {p:?} repeats the previous point"),
            ));
        }
    }
    Ok(())
}

fn m2(p: [f32; 2]) -> [f64; 2] {
    [f64::from(mm_to_m(p[0])), f64::from(mm_to_m(p[1]))]
}

fn m3(p: [f32; 2], z_mm: f32) -> [f32; 3] {
    mm3_to_m([p[0], p[1], z_mm])
}

/// Material list built in order of first use.
#[derive(Default)]
struct Materials {
    list: Vec<FlatMaterial>,
}

impl Materials {
    fn index(&mut self, m: FlatMaterial) -> usize {
        match self.list.iter().position(|x| x.name == m.name) {
            Some(i) => i,
            None => {
                self.list.push(m);
                self.list.len() - 1
            }
        }
    }

    fn physical(&mut self, m: Material) -> usize {
        self.index(materials::physical(m))
    }
}

/// Extras of a collider node.
fn collider(layer: u8, material: Material) -> Extras {
    Extras {
        collider: true,
        collision_layer: layer,
        lamp: None,
        strings: vec![("material", material.name().to_owned())],
    }
}

struct Builder<'a> {
    layout: &'a LayoutManifest,
    base: &'a TableManifest,
    nodes: Vec<Node>,
    materials: Materials,
    splines: Vec<SplineDef>,
    transitions: Vec<LayerTransitionDef>,
}

/// Generates `playfield.glb` and the merged `table.toml` from `layout` and
/// the hand-written `base` manifest (see the [module docs](self)).
///
/// # Errors
/// A [`GenError`] naming the first layout element that cannot be generated
/// validly: a value out of range, a repeated point, a self-intersecting
/// closed wall or deck outline, a lane too tight for its width or with
/// more than 4096 through-points per rail, a ramp whose spline is not in
/// `base`, or an insert whose lamp is not in `base`.
pub fn generate(layout: &LayoutManifest, base: &TableManifest) -> Result<GeneratedTable, GenError> {
    let mut b = Builder {
        layout,
        base,
        nodes: Vec::new(),
        materials: Materials::default(),
        splines: Vec::new(),
        transitions: Vec::new(),
    };
    b.playfield()?;
    for i in 0..layout.decks.len() {
        b.deck(i)?;
    }
    for i in 0..layout.walls.len() {
        b.wall(i)?;
    }
    for i in 0..layout.posts.len() {
        b.post(i)?;
    }
    for i in 0..layout.lanes.len() {
        b.lane(i)?;
    }
    for i in 0..layout.ramps.len() {
        b.ramp(i)?;
    }
    for i in 0..layout.inserts.len() {
        b.insert(i)?;
    }
    b.finish()
}

impl Builder<'_> {
    fn wall_height_m(&self) -> f32 {
        mm_to_m(self.layout.playfield.wall_height)
    }

    fn push(&mut self, name: String, mesh: Mesh, material: usize, extras: Extras) {
        self.nodes.push(Node {
            name,
            mesh,
            material,
            extras,
        });
    }

    fn playfield(&mut self) -> Result<(), GenError> {
        let pf = &self.layout.playfield;
        let e = "playfield";
        if self.layout.schema_version != pin_core::schema::SCHEMA_VERSION {
            return Err(GenError::new(
                "schema_version",
                format!("unsupported schema_version {}", self.layout.schema_version),
            ));
        }
        in_range(e, "width", pf.width, true, MAX_COORD_MM)?;
        in_range(e, "length", pf.length, true, MAX_COORD_MM)?;
        in_range(e, "wall_height", pf.wall_height, true, MAX_SIZE_MM)?;
        let (w, l) = (f64::from(mm_to_m(pf.width)), f64::from(mm_to_m(pf.length)));
        let outline = [[0.0, 0.0], [w, 0.0], [w, l], [0.0, l]];
        let mut mesh = Mesh::default();
        mesh.flat(&outline, &[[0, 1, 2], [0, 2, 3]], 0.0);
        let mat = self.materials.physical(Material::Wood);
        self.push("floor".into(), mesh, mat, collider(0, Material::Wood));
        Ok(())
    }

    fn deck(&mut self, i: usize) -> Result<(), GenError> {
        let d = &self.layout.decks[i];
        let e = format!("decks[{i}]");
        if !(1..=7).contains(&d.layer) {
            return Err(GenError::new(
                &e,
                format!(
                    "layer must be 1..=7 (layer 0 is the playfield floor), got {}",
                    d.layer
                ),
            ));
        }
        in_range(&e, "z", d.z, true, MAX_SIZE_MM)?;
        let mut outline = d.outline.clone();
        if outline.len() > 3 && outline.first() == outline.last() {
            outline.pop();
        }
        check_points(&e, &outline, 3)?;
        let mut poly: Vec<[f64; 2]> = outline.iter().map(|&p| m2(p)).collect();
        if let Some((a, c)) = geom::self_intersection(&poly) {
            return Err(GenError::new(
                &e,
                format!("the outline crosses or touches itself (edges {a} and {c})"),
            ));
        }
        let area = geom::signed_area2(&poly);
        if area == 0.0 {
            return Err(GenError::new(&e, "the outline has no area"));
        }
        if area < 0.0 {
            poly.reverse();
        }
        let tris = geom::triangulate(&poly)
            .ok_or_else(|| GenError::new(&e, "the outline cannot be triangulated"))?;
        let mut mesh = Mesh::default();
        mesh.flat(&poly, &tris, mm_to_m(d.z));
        let mat = self.materials.physical(d.material);
        self.push(e, mesh, mat, collider(d.layer, d.material));
        Ok(())
    }

    fn wall(&mut self, i: usize) -> Result<(), GenError> {
        let w = &self.layout.walls[i];
        let e = format!("walls[{i}]");
        let mut points = w.points.clone();
        if w.closed && points.len() > 3 && points.first() == points.last() {
            points.pop();
        }
        check_points(&e, &points, if w.closed { 3 } else { 2 })?;
        if w.closed {
            let poly: Vec<[f64; 2]> = points.iter().map(|&p| m2(p)).collect();
            if let Some((a, c)) = geom::self_intersection(&poly) {
                return Err(GenError::new(
                    &e,
                    format!(
                        "the closed wall crosses or touches itself (segments {a} and {c}); \
                         a closed wall must be a simple polygon"
                    ),
                ));
            }
        }
        let pts: Vec<[f32; 3]> = points.iter().map(|&p| m3(p, 0.0)).collect();
        let mesh = mesh::ribbon(&pts, w.closed, self.wall_height_m());
        let mat = self.materials.physical(w.material);
        self.push(e, mesh, mat, collider(0, w.material));
        Ok(())
    }

    fn post(&mut self, i: usize) -> Result<(), GenError> {
        let p = self.layout.posts[i];
        let e = format!("posts[{i}]");
        check_point(&e, "position", p.position)?;
        in_range(&e, "radius", p.radius, true, MAX_SIZE_MM)?;
        let material = if p.rubber {
            Material::Rubber
        } else {
            Material::Steel
        };
        let mesh = mesh::post(
            m2(p.position),
            f64::from(mm_to_m(p.radius)),
            0.0,
            self.wall_height_m(),
        );
        let mat = self.materials.physical(material);
        self.push(e, mesh, mat, collider(0, material));
        Ok(())
    }

    fn lane(&mut self, i: usize) -> Result<(), GenError> {
        let lane = &self.layout.lanes[i];
        let e = format!("lanes[{i}]");
        in_range(&e, "width", lane.width, true, MAX_SIZE_MM)?;
        let mut points = lane.points.clone();
        check_points(&e, &points, 2)?;
        let (first, last) = (points[0], points[points.len() - 1]);
        let gap = f64::from(first[0] - last[0]).hypot(f64::from(first[1] - last[1]));
        let closed = gap <= guides::CLOSE_SNAP_MM;
        if closed {
            if points.len() < 4 {
                return Err(GenError::new(
                    &e,
                    format!(
                        "a closed lane (last point on the first) needs at least 4 points, got {}",
                        points.len()
                    ),
                ));
            }
            let n = points.len() - 1;
            points[n] = points[0];
            if points[n - 1] == points[n] {
                return Err(GenError::new(
                    &e,
                    "the closing point repeats the previous point",
                ));
            }
        } else if gap < guides::HAIRLINE_GAP_MM {
            return Err(GenError::new(
                &e,
                format!(
                    "the lane's ends are only {gap:.3} mm apart: close the lane (repeat the \
                     first point last) or open the gap"
                ),
            ));
        }
        let rails =
            guides::lane_rails(&points, lane.width, closed).map_err(|m| GenError::new(&e, m))?;
        let material = Material::Steel;
        let mat = self.materials.physical(material);
        for (side, rail) in ["left", "right"].into_iter().zip(rails) {
            let id = format!("{GENERATED_PREFIX}{e}.{side}");
            let samples = mesh::spline_samples(&rail.spline_m);
            let mesh = mesh::ribbon(&samples, false, self.wall_height_m());
            let extras = Extras {
                collider: false,
                collision_layer: 0,
                lamp: None,
                strings: vec![
                    ("guide", id.clone()),
                    ("lane_kind", lane_kind(lane.kind).to_owned()),
                    ("material", material.name().to_owned()),
                ],
            };
            self.push(id.clone(), mesh, mat, extras);
            self.splines.push(SplineDef {
                id,
                kind: SplineKind::Guide,
                layer: 0,
                gauge: None,
                points: rail.points_mm,
                roll_deg: None,
            });
        }
        Ok(())
    }

    fn ramp(&mut self, i: usize) -> Result<(), GenError> {
        let r = &self.layout.ramps[i];
        let e = format!("ramps[{i}]");
        in_range(&e, "entry_width", r.entry_width, true, MAX_SIZE_MM)?;
        let spline = self
            .base
            .splines
            .iter()
            .find(|s| s.id == r.spline && !s.id.starts_with(GENERATED_PREFIX))
            .ok_or_else(|| {
                GenError::new(
                    &e,
                    format!(
                        "spline {:?} is not declared in the base table.toml [[splines]]",
                        r.spline
                    ),
                )
            })?;
        let (points, closed) = loader_points(spline);
        let bad = |m: String| GenError::new(&e, format!("spline {:?}: {m}", r.spline));
        if points.len() < 2 || points.iter().any(|p| !p.is_finite()) {
            return Err(bad("needs at least 2 finite points".into()));
        }
        let centre = if closed {
            CubicSpline::interpolate_closed(&points)
        } else {
            CubicSpline::interpolate(&points)
        }
        .map_err(|err| bad(format!("does not construct: {err}")))?;
        let mat = self.materials.physical(Material::Steel);
        let mesh = match spline.kind {
            SplineKind::Wireform => {
                let roll = spline
                    .roll_deg
                    .clone()
                    .unwrap_or_else(|| vec![0.0; spline.points.len()]);
                let gauge = mm_to_m(spline.gauge.unwrap_or(0.0));
                let wf = Wireform::new(centre.clone(), &roll, gauge, Vec3::Z)
                    .map_err(|err| bad(format!("does not construct: {err}")))?;
                wire_mesh(&wf)
            }
            SplineKind::Guide => {
                GuideCurve::new(centre.clone(), 0.0)
                    .map_err(|err| bad(format!("does not construct: {err}")))?;
                let samples = mesh::spline_samples(&centre);
                mesh::ribbon(&samples, false, self.wall_height_m())
            }
        };
        let extras = Extras {
            collider: false,
            collision_layer: spline.layer,
            lamp: None,
            strings: vec![
                ("spline", spline.id.clone()),
                ("material", Material::Steel.name().to_owned()),
            ],
        };
        self.push(e.clone(), mesh, mat, extras);
        if spline.layer != 0 {
            let p0 = spline.points[0];
            let half = 0.5 * r.entry_width;
            let top = p0[2].max(0.0) + half;
            self.transitions.push(LayerTransitionDef {
                id: format!("{GENERATED_PREFIX}{e}.entry"),
                from_layer: 0,
                to_layer: spline.layer,
                zone: AabbDef {
                    min: [p0[0] - half, p0[1] - half, p0[2].min(0.0)],
                    max: [p0[0] + half, p0[1] + half, top],
                },
            });
        }
        Ok(())
    }

    fn insert(&mut self, i: usize) -> Result<(), GenError> {
        let ins = &self.layout.inserts[i];
        let e = format!("inserts[{i}]");
        let lamp = self
            .base
            .lamps
            .iter()
            .find(|l| l.id == ins.lamp)
            .ok_or_else(|| {
                GenError::new(
                    &e,
                    format!(
                        "lamp {:?} is not declared in the base table.toml [[lamps]]",
                        ins.lamp
                    ),
                )
            })?;
        check_point(&e, "position", ins.position)?;
        in_range(&e, "size", ins.size, true, MAX_SIZE_MM)?;
        let rgb = lamp
            .color
            .as_deref()
            .and_then(materials::parse_hex)
            .unwrap_or([255, 255, 255]);
        let outline = insert_outline(ins.shape, m2(ins.position), f64::from(mm_to_m(ins.size)));
        let tris = geom::triangulate(&outline)
            .ok_or_else(|| GenError::new(&e, "the shape cannot be triangulated"))?;
        let mut mesh = Mesh::default();
        mesh.flat(&outline, &tris, mm_to_m(INSERT_LIFT_MM));
        let mat = self.materials.index(materials::insert(rgb));
        let extras = Extras {
            collider: false,
            collision_layer: 0,
            lamp: Some(ins.lamp.clone()),
            strings: Vec::new(),
        };
        self.push(e, mesh, mat, extras);
        Ok(())
    }

    fn finish(self) -> Result<GeneratedTable, GenError> {
        let mut table = self.base.clone();
        table
            .splines
            .retain(|s| !s.id.starts_with(GENERATED_PREFIX));
        table
            .layer_transitions
            .retain(|t| !t.id.starts_with(GENERATED_PREFIX));
        table.splines.extend(self.splines);
        table.layer_transitions.extend(self.transitions);
        table.meta.playfield_mesh = PLAYFIELD_FILE.to_owned();
        let body = table
            .to_toml_string()
            .map_err(|e| GenError::new("table.toml", e.to_string()))?;
        let table_toml = format!(
            "# Generated by pin_tablegen from layout.toml and a base table.toml.\n\
             # Entries whose id starts with \"{GENERATED_PREFIX}\" are replaced on every run.\n\n{body}"
        );
        let table = TableManifest::from_toml_str(&table_toml)
            .map_err(|e| GenError::new("table.toml", format!("does not parse back: {e}")))?;
        let glb = glb::write(&self.nodes, &self.materials.list);
        Ok(GeneratedTable {
            glb,
            table,
            table_toml,
        })
    }
}

fn lane_kind(k: LaneKind) -> &'static str {
    match k {
        LaneKind::Inlane => "inlane",
        LaneKind::Outlane => "outlane",
        LaneKind::Shooter => "shooter",
        LaneKind::Orbit => "orbit",
    }
}

/// Through-points of `s` in metres as the loader takes them (a guide whose
/// ends are within 1 µm is snapped closed).
fn loader_points(s: &SplineDef) -> (Vec<Vec3>, bool) {
    let mut pts: Vec<Vec3> = s
        .points
        .iter()
        .map(|&p| Vec3::from_array(mm3_to_m(p)))
        .collect();
    let n = pts.len();
    let closed = s.kind == SplineKind::Guide && n >= 2 && pts[0].distance(pts[n - 1]) <= 1.0e-6;
    if closed {
        pts[n - 1] = pts[0];
    }
    (pts, closed)
}

/// Two visual rail tubes along a wireform, sampled densely enough that the
/// tube axes stay within the facet tolerance of the rails.
fn wire_mesh(wf: &Wireform) -> Mesh {
    let centre = wf.centerline();
    let samples_per_segment: Vec<usize> = centre
        .segments()
        .iter()
        .map(|seg| {
            let [p0, p1, p2, p3] = seg.p;
            let a = (p0 - p1 * 2.0 + p2).as_dvec3().length();
            let b = (p1 - p2 * 2.0 + p3).as_dvec3().length();
            // Rails bend somewhat more than the centre line: margin 4.
            let k = libm::sqrt(4.0 * 6.0 * a.max(b) / (8.0 * mesh::FACET_TOLERANCE_M)).ceil();
            if k.is_finite() {
                (k as usize).clamp(2, 4096)
            } else {
                4096
            }
        })
        .collect();
    let mut ts = vec![0.0f32];
    for (i, &k) in samples_per_segment.iter().enumerate() {
        for j in 1..=k {
            ts.push(i as f32 + j as f32 / k as f32);
        }
    }
    let r = mm_to_m(WIRE_RADIUS_MM);
    let mut out = Mesh::default();
    for side in 0..2 {
        let mut pts = Vec::with_capacity(ts.len());
        let mut frames = Vec::with_capacity(ts.len());
        for &t in &ts {
            let (a, b) = wf.rail_points(t);
            pts.push(if side == 0 { a } else { b }.to_array());
            let f = wf.frame_at(t);
            frames.push((f.normal.to_array(), f.binormal.to_array()));
        }
        let tube = mesh::tube(&pts, &frames, r, WIRE_SIDES);
        let base = out.positions.len() as u32;
        out.positions.extend(tube.positions);
        out.normals.extend(tube.normals);
        out.indices.extend(tube.indices.iter().map(|k| k + base));
    }
    out
}

/// Counter-clockwise outline of an insert shape centred at `c` (m), `size`
/// m: circle of diameter `size`; equilateral triangle of side `size`
/// pointing up the table (+y); rectangle `size` wide by `size / 2` long;
/// arrow `size` long and `0.6 size` wide pointing +y.
fn insert_outline(shape: InsertShape, c: [f64; 2], size: f64) -> Vec<[f64; 2]> {
    let at = |x: f64, y: f64| [c[0] + x * size, c[1] + y * size];
    match shape {
        InsertShape::Circle => {
            let r = 0.5 * size;
            let n = mesh::circle_sides(r);
            (0..n)
                .map(|k| {
                    let u = mesh::ring(k, n);
                    [c[0] + r * u[0], c[1] + r * u[1]]
                })
                .collect()
        }
        InsertShape::Triangle => {
            let h = 0.75f64.sqrt();
            vec![
                at(-0.5, -h / 3.0),
                at(0.5, -h / 3.0),
                at(0.0, 2.0 * h / 3.0),
            ]
        }
        InsertShape::Rect => vec![
            at(-0.5, -0.25),
            at(0.5, -0.25),
            at(0.5, 0.25),
            at(-0.5, 0.25),
        ],
        InsertShape::Arrow => vec![
            at(-0.12, -0.5),
            at(0.12, -0.5),
            at(0.12, 0.0),
            at(0.3, 0.0),
            at(0.0, 0.5),
            at(-0.3, 0.0),
            at(-0.12, 0.0),
        ],
    }
}

#[cfg(test)]
mod tests;
