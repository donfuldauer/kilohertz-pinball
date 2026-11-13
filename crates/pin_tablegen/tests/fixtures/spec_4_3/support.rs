//! Shared helpers for the Subphase 4.3 spec tests (Independent Tester).
//!
//! Black box: only the brief's fixed API (`pin_tablegen::layout::{generate,
//! write_bundle, GeneratedTable, GenError}`) is used, with `pin_assets` as the
//! oracle. Geometry read back from the glb is mapped into the layout frame
//! (mm, Z up) whatever unit / up-axis convention the generator chose.

#![allow(dead_code)]

use pin_assets::{GltfScene, LoadedTable, NodeExtras, Vfs, load_gltf, load_table};
use pin_core::Material;
use pin_core::schema::layout::{
    Insert, InsertShape, Lane, LaneKind, LayoutManifest, Playfield, Post, Wall,
};
use pin_core::schema::table::{SplineKind, TableManifest};
use pin_tablegen::layout::{GeneratedTable, generate, write_bundle};
use proptest::prelude::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

/// The brief's agreement tolerance, mm.
pub const TOL: f64 = 0.1;

pub type P2 = [f64; 2];
pub type P3 = [f64; 3];
pub type Tri = [P3; 3];

// ---------------------------------------------------------------- fixtures

pub fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/spec_4_3")
}

pub fn base_table_path() -> PathBuf {
    fixture_dir().join("base/table.toml")
}

/// The hand-written base manifest (the Appendix A example plus `lamp_a..lamp_h`).
pub fn base_table() -> TableManifest {
    let text = std::fs::read_to_string(base_table_path()).unwrap();
    TableManifest::from_toml_str(&text).unwrap()
}

pub fn layout_fixture(name: &str) -> LayoutManifest {
    let text = std::fs::read_to_string(fixture_dir().join(name)).unwrap();
    LayoutManifest::from_toml_str(&text).unwrap()
}

/// PLAN.md Appendix A.3, verbatim.
pub fn a3_layout() -> LayoutManifest {
    layout_fixture("a3_layout.toml")
}

/// A hand-built layout exercising every element kind (no ramps).
pub fn geo_layout() -> LayoutManifest {
    layout_fixture("geo_layout.toml")
}

pub const LAMPS: [&str; 8] = [
    "lamp_a", "lamp_b", "lamp_c", "lamp_d", "lamp_e", "lamp_f", "lamp_g", "lamp_h",
];

/// A fresh, empty directory under the cargo test tmpdir.
pub fn temp_dir(tag: &str) -> PathBuf {
    static N: AtomicUsize = AtomicUsize::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "spec_4_3-{tag}-{}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn copy_tree(from: &Path, to: &Path, skip_root: &[&str]) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if skip_root.iter().any(|s| name.to_str() == Some(*s)) {
            continue;
        }
        let dst = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &dst, &[]);
        } else {
            std::fs::copy(entry.path(), dst).unwrap();
        }
    }
}

/// Copies everything a bundle needs besides the generator's two outputs
/// (backglass.toml, rules.lua, audio, display, ...).
pub fn copy_base_extras(dir: &Path) {
    copy_tree(
        &fixture_dir().join("base"),
        dir,
        &["table.toml", "playfield.glb"],
    );
}

// ------------------------------------------------------------ generate glue

/// `generate`, panicking with the error's Display text (GenError is only
/// promised to be `Display`).
pub fn gen_ok(layout: &LayoutManifest, base: &TableManifest) -> GeneratedTable {
    match generate(layout, base) {
        Ok(g) => g,
        Err(e) => panic!("generate failed for a valid layout: {e}"),
    }
}

/// The Display text of the error `generate` must return.
pub fn gen_err(layout: &LayoutManifest, base: &TableManifest) -> String {
    match generate(layout, base) {
        Ok(_) => panic!("generate accepted an invalid layout"),
        Err(e) => e.to_string(),
    }
}

pub struct Loaded {
    pub table: LoadedTable,
    pub scene: GltfScene,
}

/// Loads an already written bundle directory through the 4.1 validation.
pub fn load_dir(dir: &Path) -> Result<Loaded, String> {
    let vfs = Vfs::open(dir).map_err(|e| format!("Vfs::open: {e}"))?;
    let table = load_table(&vfs).map_err(|e| format!("load_table: {e}"))?;
    let scene = load_gltf(&vfs, &table.manifest.meta.playfield_mesh)
        .map_err(|e| format!("load_gltf: {e}"))?;
    Ok(Loaded { table, scene })
}

/// Writes `generated` next to the base bundle's other files and loads the
/// result through the 4.1 validation.
pub fn load_bundle(generated: &GeneratedTable, tag: &str) -> Result<Loaded, String> {
    let dir = temp_dir(tag);
    copy_base_extras(&dir);
    write_bundle(generated, &dir).map_err(|e| format!("write_bundle: {e}"))?;
    let out = load_dir(&dir);
    let _ = std::fs::remove_dir_all(&dir);
    out
}

/// Ids of the splines the generator added to `base`.
pub fn generated_spline_ids(generated: &TableManifest, base: &TableManifest) -> Vec<String> {
    generated
        .splines
        .iter()
        .filter(|s| !base.splines.iter().any(|b| b.id == s.id))
        .map(|s| s.id.clone())
        .collect()
}

/// Everything a generated bundle must satisfy whatever the layout: loads
/// without errors, no lint warning on a generated spline, the loader's cap of
/// 4 096 through-points, valid bindings, tagged colliders.
pub fn check_valid_bundle(
    generated: &GeneratedTable,
    base: &TableManifest,
    tag: &str,
) -> Result<Loaded, String> {
    let loaded = load_bundle(generated, tag)?;
    if loaded.table.manifest != generated.table {
        return Err("table.toml on disk does not parse to GeneratedTable::table".into());
    }
    let new_ids = generated_spline_ids(&generated.table, base);
    for (i, s) in generated.table.splines.iter().enumerate() {
        if !new_ids.contains(&s.id) {
            continue;
        }
        if s.points.len() > 4096 {
            return Err(format!(
                "generated spline {:?} has {} points",
                s.id,
                s.points.len()
            ));
        }
        let prefix = format!("splines[{i}]");
        for w in &loaded.table.warnings {
            if w.path == prefix || w.path.starts_with(&format!("{prefix}.")) {
                return Err(format!("generated spline {:?} is linted: {w}", s.id));
            }
        }
    }
    let binding_errors = pin_assets::validate_gltf_bindings(&loaded.scene, &loaded.table.ids);
    if !binding_errors.is_empty() {
        return Err(format!("binding errors: {binding_errors:?}"));
    }
    for m in &loaded.scene.meshes {
        if m.extras.collider {
            match m.extras.collision_layer {
                Some(l) if l <= 7 => {}
                other => {
                    return Err(format!(
                        "collider node {:?} has collision_layer {other:?}",
                        m.name
                    ));
                }
            }
        }
        if m.indices.len() % 3 != 0 || m.indices.iter().any(|&i| i as usize >= m.positions.len())
        {
            return Err(format!("mesh {:?} has a bad index buffer", m.name));
        }
    }
    Ok(loaded)
}

// ------------------------------------------------------------ world meshes

/// One glb primitive in the layout frame (mm, Z up).
pub struct WMesh {
    pub index: usize,
    pub name: String,
    pub extras: NodeExtras,
    pub material: Option<usize>,
    pub tris: Vec<Tri>,
}

impl WMesh {
    pub fn vertices(&self) -> impl Iterator<Item = P3> + '_ {
        self.tris.iter().flat_map(|t| t.iter().copied())
    }
    pub fn is_collider_on(&self, layer: u8) -> bool {
        self.extras.collider && self.extras.collision_layer == Some(layer)
    }
}

fn raw_world(scene: &GltfScene) -> Vec<Vec<Tri>> {
    scene
        .meshes
        .iter()
        .map(|m| {
            let t = m.transform;
            let world: Vec<P3> = m
                .positions
                .iter()
                .map(|p| {
                    let mut out = [0.0f64; 3];
                    for (r, o) in out.iter_mut().enumerate() {
                        *o = f64::from(t[0][r]) * f64::from(p[0])
                            + f64::from(t[1][r]) * f64::from(p[1])
                            + f64::from(t[2][r]) * f64::from(p[2])
                            + f64::from(t[3][r]);
                    }
                    out
                })
                .collect();
            m.indices
                .chunks_exact(3)
                .map(|c| [world[c[0] as usize], world[c[1] as usize], world[c[2] as usize]])
                .collect()
        })
        .collect()
}

/// Maps the glb into the layout frame. The brief does not fix the glb's unit
/// or up axis, so mm / m and Z-up / glTF Y-up are all accepted; the frame is
/// the one in which the geometry fits the playfield box.
pub fn world_meshes(
    scene: &GltfScene,
    layout: &LayoutManifest,
    table: &TableManifest,
) -> Vec<WMesh> {
    let raw = raw_world(scene);
    let pf = layout.playfield;
    let (w, l) = (f64::from(pf.width), f64::from(pf.length));
    let mut z_top = f64::from(pf.wall_height);
    for d in &layout.decks {
        z_top = z_top.max(f64::from(d.z) + f64::from(pf.wall_height));
    }
    for s in &table.splines {
        for p in &s.points {
            z_top = z_top.max(f64::from(p[2]));
        }
    }
    let margin = 40.0;
    type Map = fn(P3) -> P3;
    let maps: [(&str, Map); 3] = [
        ("z-up", |p| p),
        ("y-up", |p| [p[0], -p[2], p[1]]),
        ("y-up (unmirrored)", |p| [p[0], p[2], p[1]]),
    ];
    for (_, map) in maps {
        for scale in [1.0, 1000.0] {
            let mut lo = [f64::INFINITY; 3];
            let mut hi = [f64::NEG_INFINITY; 3];
            for p in raw.iter().flatten().flatten() {
                let q = map(*p);
                for k in 0..3 {
                    lo[k] = lo[k].min(q[k] * scale);
                    hi[k] = hi[k].max(q[k] * scale);
                }
            }
            let fits = lo[0] >= -margin
                && hi[0] <= w + margin
                && lo[1] >= -margin
                && hi[1] <= l + margin
                && lo[2] >= -margin
                && hi[2] <= z_top + margin + 20.0
                && hi[1] - lo[1] >= 0.5 * l
                && hi[0] - lo[0] >= 0.5 * w;
            if !fits {
                continue;
            }
            return scene
                .meshes
                .iter()
                .zip(&raw)
                .enumerate()
                .map(|(index, (m, tris))| WMesh {
                    index,
                    name: m.name.clone(),
                    extras: m.extras.clone(),
                    material: m.material,
                    tris: tris
                        .iter()
                        .map(|t| {
                            let f = |p: P3| {
                                let q = map(p);
                                [q[0] * scale, q[1] * scale, q[2] * scale]
                            };
                            [f(t[0]), f(t[1]), f(t[2])]
                        })
                        .collect(),
                })
                .collect();
        }
    }
    panic!(
        "the glb geometry does not fit the {w} x {l} mm playfield (origin at the layout's \
         [0, 0]) in mm or m, Z-up or Y-up"
    );
}

// ---------------------------------------------------------------- geometry

pub fn sub(a: P3, b: P3) -> P3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn cross(a: P3, b: P3) -> P3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub fn tri_area(t: &Tri) -> f64 {
    let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
    0.5 * (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt()
}

/// |n_z| of the unit normal, or `None` for a degenerate triangle.
pub fn tri_nz(t: &Tri) -> Option<f64> {
    let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
    let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
    (len > 1.0e-9).then(|| (n[2] / len).abs())
}

pub fn is_horizontal(t: &Tri) -> bool {
    tri_nz(t).is_some_and(|nz| nz > 0.999)
}

/// A wall-like face (within 30 degrees of vertical).
pub fn is_vertical(t: &Tri) -> bool {
    tri_nz(t).is_some_and(|nz| nz < 0.5)
}

pub fn plan(p: P3) -> P2 {
    [p[0], p[1]]
}

pub fn dist2(a: P2, b: P2) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2)).sqrt()
}

pub fn d_pt_seg(p: P2, a: P2, b: P2) -> f64 {
    let ab = [b[0] - a[0], b[1] - a[1]];
    let len2 = ab[0] * ab[0] + ab[1] * ab[1];
    let t = if len2 > 0.0 {
        (((p[0] - a[0]) * ab[0] + (p[1] - a[1]) * ab[1]) / len2).clamp(0.0, 1.0)
    } else {
        0.0
    };
    dist2(p, [a[0] + t * ab[0], a[1] + t * ab[1]])
}

fn orient(a: P2, b: P2, c: P2) -> f64 {
    (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
}

pub fn pt_in_tri(p: P2, t: &[P2; 3]) -> bool {
    let d = [
        orient(t[0], t[1], p),
        orient(t[1], t[2], p),
        orient(t[2], t[0], p),
    ];
    let eps = 1.0e-9;
    d.iter().all(|&v| v >= -eps) || d.iter().all(|&v| v <= eps)
}

/// Plan-view distance from `p` to a (projected) triangle.
pub fn d_pt_tri(p: P2, t: &[P2; 3]) -> f64 {
    if orient(t[0], t[1], t[2]).abs() > 1.0e-9 && pt_in_tri(p, t) {
        return 0.0;
    }
    d_pt_seg(p, t[0], t[1])
        .min(d_pt_seg(p, t[1], t[2]))
        .min(d_pt_seg(p, t[2], t[0]))
}

pub fn pt_in_poly(p: P2, poly: &[P2]) -> bool {
    let mut inside = false;
    let n = poly.len();
    for i in 0..n {
        let (a, b) = (poly[i], poly[(i + 1) % n]);
        if (a[1] > p[1]) != (b[1] > p[1]) {
            let x = a[0] + (p[1] - a[1]) / (b[1] - a[1]) * (b[0] - a[0]);
            if p[0] < x {
                inside = !inside;
            }
        }
    }
    inside
}

pub fn d_pt_polyline(p: P2, line: &[P2], closed: bool) -> f64 {
    let mut d = f64::INFINITY;
    for w in line.windows(2) {
        d = d.min(d_pt_seg(p, w[0], w[1]));
    }
    if closed && line.len() > 2 {
        d = d.min(d_pt_seg(p, line[line.len() - 1], line[0]));
    }
    if line.len() == 1 {
        d = dist2(p, line[0]);
    }
    d
}

pub fn to_f64(points: &[[f32; 2]]) -> Vec<P2> {
    points
        .iter()
        .map(|p| [f64::from(p[0]), f64::from(p[1])])
        .collect()
}

/// Points every `step` mm along a segment, ends included.
pub fn seg_samples(a: P2, b: P2, step: f64) -> Vec<P2> {
    let n = (dist2(a, b) / step).ceil().max(1.0) as usize;
    (0..=n)
        .map(|i| {
            let t = i as f64 / n as f64;
            [a[0] + t * (b[0] - a[0]), a[1] + t * (b[1] - a[1])]
        })
        .collect()
}

pub fn polyline_samples(line: &[P2], closed: bool, step: f64) -> Vec<P2> {
    let mut out = Vec::new();
    for w in line.windows(2) {
        out.extend(seg_samples(w[0], w[1], step));
    }
    if closed && line.len() > 2 {
        out.extend(seg_samples(line[line.len() - 1], line[0], step));
    }
    out
}

/// True when two segments properly cross or touch.
pub fn segs_intersect(a: P2, b: P2, c: P2, d: P2) -> bool {
    let (o1, o2) = (orient(a, b, c), orient(a, b, d));
    let (o3, o4) = (orient(c, d, a), orient(c, d, b));
    (o1 * o2 < 0.0) && (o3 * o4 < 0.0)
}

// ------------------------------------------------------- collision queries

/// The collision geometry of one layer as the brief exposes it: collider
/// nodes of the glb (wall-like faces, plan view) and the generated guide
/// splines of table.toml (zero-thickness ribbons over the curve).
pub struct Collision {
    pub faces: Vec<[P2; 3]>,
    /// Densely sampled generated guide curves, mm.
    pub guides: Vec<Vec<P2>>,
}

impl Collision {
    pub fn dist(&self, p: P2) -> f64 {
        let mut d = f64::INFINITY;
        for t in &self.faces {
            d = d.min(d_pt_tri(p, t));
        }
        for g in &self.guides {
            d = d.min(d_pt_polyline(p, g, false));
        }
        d
    }
}

/// Wall-like faces (plan view) of the meshes selected by `pick`.
pub fn vertical_faces(meshes: &[WMesh], pick: impl Fn(&WMesh) -> bool) -> Vec<[P2; 3]> {
    meshes
        .iter()
        .filter(|m| pick(m))
        .flat_map(|m| m.tris.iter())
        .filter(|t| is_vertical(t))
        .map(|t| [plan(t[0]), plan(t[1]), plan(t[2])])
        .collect()
}

/// The generated guide splines on `layer`, evaluated exactly as the loader
/// builds them (C2 interpolating spline, closed when the loader says so),
/// sampled 48 times per segment, mm.
pub fn guide_curves(loaded: &LoadedTable, base: &TableManifest, layer: u8) -> Vec<Vec<P2>> {
    let new_ids = generated_spline_ids(&loaded.manifest, base);
    let mut out = Vec::new();
    for s in &loaded.splines {
        if s.kind != SplineKind::Guide || s.layer != layer || !new_ids.contains(&s.name) {
            continue;
        }
        let pts: Vec<_> = s.points_m.iter().map(|p| (*p).into()).collect();
        let spline = if s.closed {
            pin_physics::world::CubicSpline::interpolate_closed(&pts)
        } else {
            pin_physics::world::CubicSpline::interpolate(&pts)
        };
        let spline = match spline {
            Ok(s) => s,
            Err(e) => panic!("generated guide {:?} cannot be built: {e:?}", s.name),
        };
        let mut line = Vec::new();
        for seg in spline.segments() {
            for k in 0..=48 {
                let p = seg.point(k as f32 / 48.0).to_array();
                line.push([f64::from(p[0]) * 1000.0, f64::from(p[1]) * 1000.0]);
            }
        }
        out.push(line);
    }
    out
}

pub fn collision_on(
    meshes: &[WMesh],
    loaded: &LoadedTable,
    base: &TableManifest,
    layer: u8,
) -> Collision {
    Collision {
        faces: vertical_faces(meshes, |m| m.is_collider_on(layer)),
        guides: guide_curves(loaded, base, layer),
    }
}

/// Checks that exactly one collider mesh on `layer` provides the horizontal
/// surface at height `z` and that it covers `outline` without gaps. Returns
/// the mesh index.
pub fn check_surface(
    meshes: &[WMesh],
    layer: u8,
    z: f64,
    outline: &[P2],
    what: &str,
) -> Result<usize, String> {
    let poly_area = {
        let mut a = 0.0;
        for i in 0..outline.len() {
            let (p, q) = (outline[i], outline[(i + 1) % outline.len()]);
            a += p[0] * q[1] - q[0] * p[1];
        }
        (a * 0.5).abs()
    };
    let mut candidates: Vec<(usize, Vec<[P2; 3]>, f64)> = Vec::new();
    for m in meshes {
        if !m.is_collider_on(layer) || m.extras.lamp.is_some() {
            continue;
        }
        let tris: Vec<[P2; 3]> = m
            .tris
            .iter()
            .filter(|t| is_horizontal(t) && t.iter().all(|p| (p[2] - z).abs() <= TOL))
            .map(|t| [plan(t[0]), plan(t[1]), plan(t[2])])
            .collect();
        // Only what lies over the outline counts (a neighbouring surface at
        // the same height is not this surface).
        let area: f64 = tris
            .iter()
            .filter(|t| {
                let c = [
                    (t[0][0] + t[1][0] + t[2][0]) / 3.0,
                    (t[0][1] + t[1][1] + t[2][1]) / 3.0,
                ];
                pt_in_poly(c, outline)
            })
            .map(|t| 0.5 * orient(t[0], t[1], t[2]).abs())
            .sum();
        // A quarter of the outline: the underside of a wall standing on the
        // surface is not a second surface.
        if area > 0.25 * poly_area {
            candidates.push((m.index, tris, area));
        }
    }
    if candidates.len() != 1 {
        return Err(format!(
            "{what}: expected exactly one collider mesh on layer {layer} forming the surface at \
             z = {z} mm, found {} (meshes {:?})",
            candidates.len(),
            candidates.iter().map(|c| c.0).collect::<Vec<_>>()
        ));
    }
    let (index, tris, area) = &candidates[0];
    if *area < 0.999 * poly_area || *area > 1.001 * poly_area {
        return Err(format!(
            "{what}: surface area over the outline is {area:.1} mm^2, the outline has \
             {poly_area:.1} mm^2 (gaps or overlapping triangles)"
        ));
    }
    // Coverage on a grid of interior points.
    let (mut lo, mut hi) = ([f64::INFINITY; 2], [f64::NEG_INFINITY; 2]);
    for p in outline {
        for k in 0..2 {
            lo[k] = lo[k].min(p[k]);
            hi[k] = hi[k].max(p[k]);
        }
    }
    let step = ((hi[0] - lo[0]).max(hi[1] - lo[1]) / 61.0).max(1.0);
    let mut y = lo[1] + 0.37;
    while y < hi[1] {
        let mut x = lo[0] + 0.41;
        while x < hi[0] {
            let p = [x, y];
            if pt_in_poly(p, outline)
                && d_pt_polyline(p, outline, true) > 0.2
                && !tris.iter().any(|t| pt_in_tri(p, t))
            {
                return Err(format!(
                    "{what}: the surface at z = {z} has a hole at ({x:.2}, {y:.2})"
                ));
            }
            x += step;
        }
        y += step;
    }
    Ok(*index)
}

pub fn playfield_outline(pf: &Playfield) -> Vec<P2> {
    let (w, l) = (f64::from(pf.width), f64::from(pf.length));
    vec![[0.0, 0.0], [w, 0.0], [w, l], [0.0, l]]
}

// ------------------------------------------------- random valid layouts

/// One element of a random layout, placed in its own cell of the playfield
/// so that elements never overlap.
#[derive(Clone, Debug)]
pub enum Elem {
    WallOpen {
        ys: Vec<f32>,
        material: usize,
    },
    WallClosed {
        radius: f32,
        base_deg: f32,
        jitter_deg: Vec<f32>,
        material: usize,
    },
    Post {
        dx: f32,
        dy: f32,
        radius: f32,
        rubber: bool,
    },
    Lane {
        kind: usize,
        theta_deg: f32,
        bend_deg: f32,
        three: bool,
        spacing: f32,
        width: f32,
    },
    Insert {
        dx: f32,
        dy: f32,
        shape: usize,
        size: f32,
    },
}

pub fn elem() -> impl Strategy<Value = Elem> {
    prop_oneof![
        (prop::collection::vec(-25.0f32..25.0, 2..=4), 0usize..7)
            .prop_map(|(ys, material)| Elem::WallOpen { ys, material }),
        (
            30.0f32..40.0,
            0.0f32..360.0,
            prop::collection::vec(-15.0f32..15.0, 3..=5),
            0usize..7
        )
            .prop_map(|(radius, base_deg, jitter_deg, material)| Elem::WallClosed {
                radius,
                base_deg,
                jitter_deg,
                material
            }),
        (-40.0f32..40.0, -30.0f32..30.0, 3.0f32..12.0, any::<bool>()).prop_map(
            |(dx, dy, radius, rubber)| Elem::Post {
                dx,
                dy,
                radius,
                rubber
            }
        ),
        (
            0usize..4,
            0.0f32..360.0,
            -20.0f32..20.0,
            any::<bool>(),
            40.0f32..50.0,
            28.0f32..40.0
        )
            .prop_map(|(kind, theta_deg, bend_deg, three, spacing, width)| {
                Elem::Lane {
                    kind,
                    theta_deg,
                    bend_deg,
                    three,
                    spacing,
                    width,
                }
            }),
        (-40.0f32..40.0, -30.0f32..30.0, 0usize..4, 10.0f32..30.0).prop_map(
            |(dx, dy, shape, size)| Elem::Insert {
                dx,
                dy,
                shape,
                size
            }
        ),
    ]
}

fn r2(v: f32) -> f32 {
    (v * 100.0).round() / 100.0
}

/// Builds the layout: a 2 x 6 grid of cells (each at least 200 x 150 mm), one
/// element per cell, everything at least 25 mm inside its cell.
pub fn build_layout(
    width: f32,
    length: f32,
    wall_height: f32,
    outer_wall: bool,
    elems: &[Elem],
    cells: &[usize],
) -> LayoutManifest {
    let mut layout = LayoutManifest {
        schema_version: 1,
        playfield: Playfield {
            width: r2(width),
            length: r2(length),
            wall_height: r2(wall_height),
        },
        walls: Vec::new(),
        posts: Vec::new(),
        lanes: Vec::new(),
        ramps: Vec::new(),
        decks: Vec::new(),
        inserts: Vec::new(),
    };
    let (w, l) = (layout.playfield.width, layout.playfield.length);
    if outer_wall {
        layout.walls.push(Wall {
            points: vec![[0.0, 0.0], [0.0, l], [w, l], [w, 0.0]],
            closed: false,
            material: Material::Wood,
        });
    }
    for (e, &cell) in elems.iter().zip(cells) {
        let cx = w * (0.25 + 0.5 * (cell % 2) as f32);
        let cy = l * ((cell / 2) as f32 + 0.5) / 6.0;
        match e {
            Elem::WallOpen { ys, material } => {
                let n = ys.len();
                let points = ys
                    .iter()
                    .enumerate()
                    .map(|(i, y)| {
                        [
                            r2(cx - 60.0 + 120.0 * i as f32 / (n - 1) as f32),
                            r2(cy + y),
                        ]
                    })
                    .collect();
                layout.walls.push(Wall {
                    points,
                    closed: false,
                    material: Material::ALL[*material],
                });
            }
            Elem::WallClosed {
                radius,
                base_deg,
                jitter_deg,
                material,
            } => {
                let n = jitter_deg.len();
                let points = jitter_deg
                    .iter()
                    .enumerate()
                    .map(|(i, j)| {
                        let a = (base_deg + 360.0 * i as f32 / n as f32 + j).to_radians();
                        [r2(cx + radius * a.cos()), r2(cy + radius * a.sin())]
                    })
                    .collect();
                layout.walls.push(Wall {
                    points,
                    closed: true,
                    material: Material::ALL[*material],
                });
            }
            Elem::Post {
                dx,
                dy,
                radius,
                rubber,
            } => layout.posts.push(Post {
                position: [r2(cx + dx), r2(cy + dy)],
                radius: r2(*radius),
                rubber: *rubber,
            }),
            Elem::Lane {
                kind,
                theta_deg,
                bend_deg,
                three,
                spacing,
                width,
            } => {
                let a = theta_deg.to_radians();
                let b = (theta_deg + bend_deg).to_radians();
                let mut points = vec![
                    [r2(cx - spacing * a.cos()), r2(cy - spacing * a.sin())],
                    [r2(cx), r2(cy)],
                ];
                if *three {
                    points.push([r2(cx + spacing * b.cos()), r2(cy + spacing * b.sin())]);
                }
                layout.lanes.push(Lane {
                    kind: [
                        LaneKind::Inlane,
                        LaneKind::Outlane,
                        LaneKind::Shooter,
                        LaneKind::Orbit,
                    ][*kind],
                    points,
                    width: r2(*width),
                });
            }
            Elem::Insert {
                dx,
                dy,
                shape,
                size,
            } => {
                if layout.inserts.len() < LAMPS.len() {
                    layout.inserts.push(Insert {
                        lamp: LAMPS[layout.inserts.len()].to_owned(),
                        position: [r2(cx + dx), r2(cy + dy)],
                        shape: [
                            InsertShape::Circle,
                            InsertShape::Arrow,
                            InsertShape::Triangle,
                            InsertShape::Rect,
                        ][*shape],
                        size: r2(*size),
                    });
                }
            }
        }
    }
    layout
}

/// Random valid layouts: walls, posts, lanes and inserts within the playfield.
pub fn valid_layout() -> impl Strategy<Value = LayoutManifest> {
    (
        400.0f32..600.0,
        900.0f32..1200.0,
        20.0f32..40.0,
        any::<bool>(),
        prop::collection::vec(elem(), 0..=12),
        Just((0..12usize).collect::<Vec<_>>()).prop_shuffle(),
    )
        .prop_map(|(w, l, h, outer, elems, cells)| build_layout(w, l, h, outer, &elems, &cells))
}
