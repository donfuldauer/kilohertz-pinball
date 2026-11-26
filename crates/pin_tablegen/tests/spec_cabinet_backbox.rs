//! Black-box spec tests for the cabinet generator's backbox and art-blade nodes (issue #111,
//! KP-030): REQ-CAB-001 (backbox with hinge, translite frame, speaker panel with two grilles and
//! the score-display window; inside art blades), REQ-CAB-003 (backbox sizes, profiles, openings,
//! panel height, grilles and display window are data), SPEC §7.3, Appendix A.4 / E, and the
//! #111 backbox model contract sections A to G.
//!
//! Frame: the file is metres in the SPEC A.4 cabinet frame (x across, player's right positive;
//! y from the body front toward the rear; z = height above the floor). All assertions below are
//! made in millimetres. Expected values are derived from the geometry fields and
//! `floor_height_mm`, never from preset numbers (REQ-CAB-003).
#![allow(dead_code)]

use pin_assets::{GltfScene, MeshData, Vfs, load_gltf};
use pin_core::fmath::{max_f32, max_f64, min_f64};
use pin_core::schema::cabinet::{
    BackboxFrontProfile, BackboxTopProfile, CabinetGeometry, CabinetManifest, Grille,
    ScoreDisplayKind,
};
use pin_tablegen::cabinet::{
    CABINET_GLB_FILE, GeneratedCabinet, MATERIAL_NAMES, MAX_DIMENSION_MM, NODE_NAMES,
    generate_cabinet,
};
use std::sync::atomic::{AtomicUsize, Ordering};

const PRESETS: [&str; 3] = ["classic_standard", "classic_widebody", "modern_standard"];
/// Tolerance of every "within 1 mm" clause, mm.
const TOL: f64 = 1.0;
/// Contract A: FRONT_LEAN_DEG.
const FRONT_LEAN_DEG: f64 = 10.0;
/// Contract B: TOP_SLOPE_DEG.
const TOP_SLOPE_DEG: f64 = 10.0;
/// Contract E: BLADE_THICKNESS, mm.
const BLADE_THICKNESS: f64 = 3.0;
/// Contract C: GRILLE_PROUD, mm.
const GRILLE_PROUD: f64 = 2.0;

/// The 37 stable node names (contract C: the 28 of #110 then indices 28..36).
const NODES: [&str; 37] = [
    "cabinet.body.bottom",
    "cabinet.body.side_left",
    "cabinet.body.side_right",
    "cabinet.body.front",
    "cabinet.body.rear",
    "cabinet.rail.left",
    "cabinet.rail.right",
    "cabinet.lockdown_bar",
    "cabinet.glass",
    "cabinet.leg.front_left",
    "cabinet.leg.front_right",
    "cabinet.leg.rear_left",
    "cabinet.leg.rear_right",
    "cabinet.leg.front_left.bracket",
    "cabinet.leg.front_right.bracket",
    "cabinet.leg.rear_left.bracket",
    "cabinet.leg.rear_right.bracket",
    "cabinet.leg.front_left.leveler",
    "cabinet.leg.front_right.leveler",
    "cabinet.leg.rear_left.leveler",
    "cabinet.leg.rear_right.leveler",
    "cabinet.coin_door",
    "cabinet.button.left_flipper",
    "cabinet.button.right_flipper",
    "cabinet.button.start",
    "cabinet.button.action",
    "cabinet.plunger.housing",
    "cabinet.plunger.rod",
    "cabinet.backbox.shell",
    "cabinet.backbox.speaker_panel",
    "cabinet.backbox.translite_frame",
    "cabinet.backbox.translite",
    "cabinet.backbox.display",
    "cabinet.backbox.grilles",
    "cabinet.backbox.hinge",
    "cabinet.blade.left",
    "cabinet.blade.right",
];

/// Contract F: the 24 materials, in order.
const MATERIALS: [&str; 24] = [
    "cabinet.body",
    "cabinet.side_left",
    "cabinet.side_right",
    "cabinet.front",
    "cabinet.trim_metal",
    "cabinet.glass",
    "cabinet.coin_door",
    "cabinet.leveler",
    "cabinet.leg.front_left",
    "cabinet.leg.front_right",
    "cabinet.leg.rear_left",
    "cabinet.leg.rear_right",
    "cabinet.button.left_flipper",
    "cabinet.button.right_flipper",
    "cabinet.button.start",
    "cabinet.button.action",
    "cabinet.plunger",
    "cabinet.backbox",
    "cabinet.backbox.speaker_panel",
    "cabinet.backbox.grille",
    "cabinet.backbox.translite",
    "cabinet.backbox.display",
    "cabinet.blade_left",
    "cabinet.blade_right",
];

/// Contract F: node to material for the #111 nodes.
const NEW_NODE_MATERIAL: [(&str, &str); 9] = [
    ("cabinet.backbox.shell", "cabinet.backbox"),
    (
        "cabinet.backbox.speaker_panel",
        "cabinet.backbox.speaker_panel",
    ),
    ("cabinet.backbox.translite_frame", "cabinet.trim_metal"),
    ("cabinet.backbox.translite", "cabinet.backbox.translite"),
    ("cabinet.backbox.display", "cabinet.backbox.display"),
    ("cabinet.backbox.grilles", "cabinet.backbox.grille"),
    ("cabinet.backbox.hinge", "cabinet.trim_metal"),
    ("cabinet.blade.left", "cabinet.blade_left"),
    ("cabinet.blade.right", "cabinet.blade_right"),
];

/// Contract C: the bounds set.
const BOUNDS_SET: [&str; 3] = [
    "cabinet.backbox.shell",
    "cabinet.backbox.speaker_panel",
    "cabinet.backbox.translite_frame",
];

// ---------------------------------------------------------------------------
// Loading helpers.
// ---------------------------------------------------------------------------

fn load(cab: &GeneratedCabinet) -> GltfScene {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "spec_cabinet_backbox_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join(CABINET_GLB_FILE), &cab.glb).unwrap();
    let vfs = Vfs::open(&dir).unwrap();
    let scene = load_gltf(&vfs, CABINET_GLB_FILE).unwrap_or_else(|e| panic!("{e:?}"));
    let _ = std::fs::remove_dir_all(&dir);
    scene
}

fn gen_scene(g: &CabinetGeometry, label: &str) -> (GeneratedCabinet, GltfScene) {
    let cab = match generate_cabinet(g) {
        Ok(c) => c,
        Err(e) => panic!(
            "{label}: generate_cabinet failed: {} / {}",
            e.element, e.message
        ),
    };
    let scene = load(&cab);
    (cab, scene)
}

fn preset(id: &str) -> CabinetGeometry {
    CabinetGeometry::preset(id).unwrap_or_else(|e| panic!("{e:?}"))
}

fn from_toml(extra: &str, base: &str) -> CabinetGeometry {
    let text = format!("schema_version = 1\npreset = \"{base}\"\n{extra}");
    CabinetManifest::from_toml_str(&text)
        .unwrap_or_else(|e| panic!("authoring error, parse: {e:?}\n{text}"))
        .resolve()
        .unwrap_or_else(|e| panic!("authoring error, resolve: {e:?}\n{text}"))
}

fn mesh<'a>(s: &'a GltfScene, name: &str) -> &'a MeshData {
    s.meshes
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("no node {name}"))
}

fn is_identity(m: &MeshData) -> bool {
    (0..4).all(|c| {
        (0..4).all(|r| {
            let want = if c == r { 1.0 } else { 0.0 };
            (f64::from(m.transform[c][r]) - want).abs() < 1e-6
        })
    })
}

/// Vertex positions in mm (static nodes: identity transform, asserted separately).
fn pts_mm(m: &MeshData) -> Vec<[f64; 3]> {
    m.positions
        .iter()
        .map(|p| p.map(|c| f64::from(c) * 1000.0))
        .collect()
}

fn tris_mm(m: &MeshData) -> Vec<[[f64; 3]; 3]> {
    let p = pts_mm(m);
    m.indices
        .chunks_exact(3)
        .map(|t| [p[t[0] as usize], p[t[1] as usize], p[t[2] as usize]])
        .collect()
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn len(a: [f64; 3]) -> f64 {
    libm::sqrt(dot(a, a))
}

/// Unit geometric normal of a triangle from its winding (counter-clockwise front), or `None`
/// when degenerate.
fn tri_normal(t: &[[f64; 3]; 3]) -> Option<[f64; 3]> {
    let n = cross(sub(t[1], t[0]), sub(t[2], t[0]));
    let l = len(n);
    if l < 1e-6 {
        None
    } else {
        Some([n[0] / l, n[1] / l, n[2] / l])
    }
}

type Bounds = ([f64; 3], [f64; 3]);

fn bounds(pts: &[[f64; 3]]) -> Bounds {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in pts {
        for a in 0..3 {
            if p[a] < lo[a] {
                lo[a] = p[a];
            }
            if p[a] > hi[a] {
                hi[a] = p[a];
            }
        }
    }
    (lo, hi)
}

fn near(a: f64, b: f64, tol: f64, what: &str) {
    assert!(
        (a - b).abs() <= tol,
        "{what}: got {a}, want {b} (tol {tol})"
    );
}

fn deg(d: f64) -> f64 {
    d * core::f64::consts::PI / 180.0
}

// ---------------------------------------------------------------------------
// Contract A: face frame.
// ---------------------------------------------------------------------------

/// The backbox front face, in mm, per contract A.
#[derive(Clone, Copy, Debug)]
struct Face {
    w: f64,
    h: f64,
    d: f64,
    y0: f64,
    zb: f64,
    t: f64,
    /// sin and cos of the lean (0 and 1 for a flat front).
    s: f64,
    c: f64,
    angled: bool,
    top: BackboxTopProfile,
}

impl Face {
    fn of(g: &CabinetGeometry) -> Face {
        let bb = &g.backbox;
        let angled = bb.front_profile == BackboxFrontProfile::Angled;
        let (s, c) = if angled {
            (
                libm::sin(deg(FRONT_LEAN_DEG)),
                libm::cos(deg(FRONT_LEAN_DEG)),
            )
        } else {
            (0.0, 1.0)
        };
        Face {
            w: f64::from(bb.width),
            h: f64::from(bb.height),
            d: f64::from(bb.depth),
            y0: f64::from(bb.front_y),
            zb: g.floor_height_mm(f64::from(bb.front_y), f64::from(bb.bottom_z)),
            t: f64::from(g.body.wall_thickness),
            s,
            c,
            angled,
            top: bb.top_profile,
        }
    }

    /// Outward face normal.
    fn n_out(&self) -> [f64; 3] {
        [0.0, -self.c, self.s]
    }

    /// (u, v, depth behind the face along the inward normal), mm.
    fn uvd(&self, p: [f64; 3]) -> [f64; 3] {
        let ry = p[1] - self.y0;
        let rz = p[2] - self.zb;
        let v = ry * self.s + rz * self.c;
        let out = -ry * self.c + rz * self.s;
        [p[0], v, -out]
    }

    /// In-plane height of the face.
    fn face_h(&self) -> f64 {
        self.h / self.c
    }

    /// Arched crown drop S (contract B).
    fn crown(&self) -> f64 {
        0.06 * self.w
    }

    /// Top-profile height above the floor at x (arched) and y (angled), contract B.
    fn z_top(&self, x: f64, y: f64) -> f64 {
        match self.top {
            BackboxTopProfile::Flat => self.zb + self.h,
            BackboxTopProfile::Angled => {
                let y_ft = self.y0 + self.face_h() * self.s;
                self.zb + self.h - (y - y_ft) * libm::tan(deg(TOP_SLOPE_DEG))
            }
            BackboxTopProfile::Arched => {
                let q = 2.0 * x / self.w;
                self.zb + self.h - self.crown() * q * q
            }
        }
    }

    /// In-plane v of the translite frame's top edge at u (contract A, B).
    fn plate_top_v(&self, u: f64) -> f64 {
        let y_ft = self.y0 + self.face_h() * self.s;
        (self.z_top(u, y_ft) - self.zb) / self.c
    }
}

// ---------------------------------------------------------------------------
// Contract D: opening coverage seen along the face normal.
// ---------------------------------------------------------------------------

/// Front-facing triangles (normal within ~25 degrees of the outward face normal), projected to
/// face (u, v).
fn front_tris_uv(m: &MeshData, f: &Face) -> Vec<[[f64; 2]; 3]> {
    let n_out = f.n_out();
    tris_mm(m)
        .iter()
        .filter(|t| tri_normal(t).is_some_and(|n| dot(n, n_out) > 0.9))
        .map(|t| {
            t.map(|p| {
                let q = f.uvd(p);
                [q[0], q[1]]
            })
        })
        .collect()
}

fn in_tri(t: &[[f64; 2]; 3], p: [f64; 2]) -> bool {
    let e =
        |a: [f64; 2], b: [f64; 2]| (b[0] - a[0]) * (p[1] - a[1]) - (b[1] - a[1]) * (p[0] - a[0]);
    let d0 = e(t[0], t[1]);
    let d1 = e(t[1], t[2]);
    let d2 = e(t[2], t[0]);
    let eps = 1e-9;
    let neg = d0 < -eps || d1 < -eps || d2 < -eps;
    let pos = d0 > eps || d1 > eps || d2 > eps;
    !(neg && pos)
}

fn covered(tris: &[[[f64; 2]; 3]], p: [f64; 2]) -> bool {
    tris.iter().any(|t| in_tri(t, p))
}

/// The hole `hole = [half_w, v_lo, v_hi]` (u in +-half_w, v in [v_lo, v_hi]) in `node`: a point 0.5 mm inside each edge is not
/// covered, a point 0.5 mm outside is (contract D). `plate_v` is the plate's v range at u, so
/// outside points that would fall off the plate are not asked for.
fn check_hole(
    s: &GltfScene,
    node: &str,
    f: &Face,
    hole: [f64; 3],
    plate_v: &dyn Fn(f64) -> (f64, f64),
    label: &str,
) {
    let [half_w, v_lo, v_hi] = hole;
    let tris = front_tris_uv(mesh(s, node), f);
    assert!(
        !tris.is_empty(),
        "{label} {node}: no front-facing triangles"
    );
    let us = [-half_w / 2.0, half_w / 2.0];
    let vs = [v_lo + (v_hi - v_lo) / 3.0, v_lo + 2.0 * (v_hi - v_lo) / 3.0];
    let mut inside = Vec::new();
    let mut outside = Vec::new();
    for u in us {
        inside.push([u, v_lo + 0.5]);
        inside.push([u, v_hi - 0.5]);
        outside.push([u, v_lo - 0.5]);
        outside.push([u, v_hi + 0.5]);
    }
    for v in vs {
        inside.push([-half_w + 0.5, v]);
        inside.push([half_w - 0.5, v]);
        outside.push([-half_w - 0.5, v]);
        outside.push([half_w + 0.5, v]);
    }
    for p in inside {
        assert!(
            !covered(&tris, p),
            "{label} {node}: point {p:?} 0.5 mm inside the hole is covered"
        );
    }
    let mut asked = 0;
    for p in outside {
        let (lo, hi) = plate_v(p[0]);
        if p[0].abs() > f.w / 2.0 - 0.1 || p[1] < lo + 0.1 || p[1] > hi - 0.1 {
            continue;
        }
        asked += 1;
        assert!(
            covered(&tris, p),
            "{label} {node}: point {p:?} 0.5 mm outside the hole is not covered"
        );
    }
    assert!(
        asked >= 4,
        "{label} {node}: only {asked} outside points on the plate"
    );
}

// ---------------------------------------------------------------------------
// Per-node checks (contract C, D, E).
// ---------------------------------------------------------------------------

/// (a) Bounds set: x in +-W/2, y in [Y0, Y0 + D], z in [zb, zb + H], each within 1 mm.
fn check_bounds_set(s: &GltfScene, g: &CabinetGeometry, f: &Face, label: &str) {
    let mut all = Vec::new();
    for n in BOUNDS_SET {
        all.extend(pts_mm(mesh(s, n)));
    }
    let (lo, hi) = bounds(&all);
    near(lo[0], -f.w / 2.0, TOL, &format!("{label} bounds x min"));
    near(hi[0], f.w / 2.0, TOL, &format!("{label} bounds x max"));
    near(lo[1], f.y0, TOL, &format!("{label} bounds y min"));
    near(hi[1], f.y0 + f.d, TOL, &format!("{label} bounds y max"));
    near(lo[2], f.zb, TOL, &format!("{label} bounds z min"));
    near(hi[2], f.zb + f.h, TOL, &format!("{label} bounds z max"));
    near(
        hi[2],
        g.backbox_top_height_mm(),
        TOL,
        &format!("{label} bounds z max vs backbox_top_height_mm"),
    );
}

/// (b) Translite opening in the translite_frame, score-display window in the speaker panel.
fn check_openings(s: &GltfScene, g: &CabinetGeometry, f: &Face, label: &str) {
    let bb = &g.backbox;
    let sph = f64::from(bb.speaker_panel_height);
    let o = &bb.translite_opening;
    let (ob, oh) = (f64::from(o.bottom), f64::from(o.height));
    let ff = *f;
    check_hole(
        s,
        "cabinet.backbox.translite_frame",
        f,
        [f64::from(o.width) / 2.0, ob, ob + oh],
        &move |u| (sph, ff.plate_top_v(u)),
        label,
    );
    let sd = &bb.score_display;
    let (cv, sh) = (f64::from(sd.centre_v), f64::from(sd.height));
    check_hole(
        s,
        "cabinet.backbox.speaker_panel",
        f,
        [f64::from(sd.width) / 2.0, cv - sh / 2.0, cv + sh / 2.0],
        &move |_| (0.0, sph),
        label,
    );
}

/// Finds the vertex index nearest the given (u, v) corner among `idx`.
fn corner_uv(
    m: &MeshData,
    f: &Face,
    idx: &[usize],
    want_u_max: bool,
    want_v_max: bool,
) -> [f32; 2] {
    let p = pts_mm(m);
    let mut best = idx[0];
    let mut best_score = f64::NEG_INFINITY;
    for &i in idx {
        let q = f.uvd(p[i]);
        let su = if want_u_max { q[0] } else { -q[0] };
        let sv = if want_v_max { q[1] } else { -q[1] };
        let score = su + sv;
        if score > best_score {
            best_score = score;
            best = i;
        }
    }
    m.uvs[best]
}

/// Contract C: front-face UVs upper-left (0,0), upper-right (1,0), lower-left (0,1),
/// lower-right (1,1) in the viewer's orientation.
fn check_corner_uvs(m: &MeshData, f: &Face, idx: &[usize], label: &str) {
    let cases = [
        (false, true, [0.0, 0.0], "upper-left"),
        (true, true, [1.0, 0.0], "upper-right"),
        (false, false, [0.0, 1.0], "lower-left"),
        (true, false, [1.0, 1.0], "lower-right"),
    ];
    for (umax, vmax, want, what) in cases {
        let uv = corner_uv(m, f, idx, umax, vmax);
        assert!(
            (f64::from(uv[0]) - want[0]).abs() < 1e-3 && (f64::from(uv[1]) - want[1]).abs() < 1e-3,
            "{label} {}: {what} UV {uv:?}, want {want:?}",
            m.name
        );
    }
}

/// (c) The display quad.
fn check_display(s: &GltfScene, g: &CabinetGeometry, f: &Face, label: &str) {
    let m = mesh(s, "cabinet.backbox.display");
    let sd = &g.backbox.score_display;
    assert_eq!(m.indices.len(), 6, "{label}: display is not 2 triangles");
    let pts = pts_mm(m);
    let mut uniq: Vec<[f64; 3]> = Vec::new();
    for p in &pts {
        if !uniq.iter().any(|q| len(sub(*p, *q)) < 1e-3) {
            uniq.push(*p);
        }
    }
    assert_eq!(uniq.len(), 4, "{label}: display has {} corners", uniq.len());
    let q: Vec<[f64; 3]> = pts.iter().map(|p| f.uvd(*p)).collect();
    let (lo, hi) = bounds(&q);
    let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
    near(
        w,
        f64::from(sd.width),
        TOL,
        &format!("{label} display width"),
    );
    near(
        h,
        f64::from(sd.height),
        TOL,
        &format!("{label} display in-plane height"),
    );
    near(
        (lo[0] + hi[0]) / 2.0,
        0.0,
        TOL,
        &format!("{label} display u centre"),
    );
    near(
        (lo[1] + hi[1]) / 2.0,
        f64::from(sd.centre_v),
        TOL,
        &format!("{label} display v centre"),
    );
    // One plane parallel to the face, behind it by 0..T.
    assert!(
        hi[2] - lo[2] < 0.01,
        "{label}: display not in one face-parallel plane (depth {}..{})",
        lo[2],
        hi[2]
    );
    assert!(
        lo[2] >= -0.01 && hi[2] <= f.t + 0.01,
        "{label}: display depth {}..{} outside 0..T {}",
        lo[2],
        hi[2],
        f.t
    );
    // Aspect within 2 % of the pixel aspect (acceptance).
    let want = f64::from(sd.pixels[0]) / f64::from(sd.pixels[1]);
    let got = w / h;
    assert!(
        (got / want - 1.0).abs() <= 0.02,
        "{label}: display aspect {got} vs pixel aspect {want}"
    );
    // Faces the player.
    for t in tris_mm(m) {
        let n = tri_normal(&t).unwrap_or_else(|| panic!("{label}: degenerate display triangle"));
        assert!(
            dot(n, f.n_out()) > 0.999,
            "{label}: display triangle normal {n:?} does not face the player"
        );
    }
    let idx: Vec<usize> = (0..pts.len()).collect();
    check_corner_uvs(m, f, &idx, label);
}

/// (d) The translite (backglass) slab.
fn check_translite(s: &GltfScene, g: &CabinetGeometry, f: &Face, label: &str) {
    let bb = &g.backbox;
    let bg = &bb.backglass;
    let o = &bb.translite_opening;
    let m = mesh(s, "cabinet.backbox.translite");
    let pts = pts_mm(m);
    let q: Vec<[f64; 3]> = pts.iter().map(|p| f.uvd(*p)).collect();
    let (lo, hi) = bounds(&q);
    near(
        hi[0] - lo[0],
        f64::from(bg.width),
        TOL,
        &format!("{label} translite width"),
    );
    near(
        hi[1] - lo[1],
        f64::from(bg.height),
        TOL,
        &format!("{label} translite height"),
    );
    near(
        hi[2] - lo[2],
        f64::from(bg.thickness),
        TOL,
        &format!("{label} translite thickness"),
    );
    near(
        (lo[0] + hi[0]) / 2.0,
        0.0,
        TOL,
        &format!("{label} translite u centre"),
    );
    let vc = f64::from(o.bottom) + f64::from(o.height) / 2.0;
    near(
        (lo[1] + hi[1]) / 2.0,
        vc,
        TOL,
        &format!("{label} translite v centre"),
    );
    near(
        lo[2],
        f.t,
        TOL,
        &format!("{label} translite front surface depth"),
    );
    // Covers the opening.
    let ow = f64::from(o.width) / 2.0;
    assert!(
        lo[0] <= -ow + TOL && hi[0] >= ow - TOL,
        "{label}: translite u {}..{} does not cover the opening",
        lo[0],
        hi[0]
    );
    assert!(
        lo[1] <= f64::from(o.bottom) + TOL && hi[1] >= f64::from(o.bottom + o.height) - TOL,
        "{label}: translite v {}..{} does not cover the opening",
        lo[1],
        hi[1]
    );
    // Front-face UV corners.
    let n_out = f.n_out();
    let mut idx = Vec::new();
    for t in m.indices.chunks_exact(3) {
        let tri = [pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]];
        if tri_normal(&tri).is_some_and(|n| dot(n, n_out) > 0.99) {
            idx.extend(t.iter().map(|&i| i as usize));
        }
    }
    assert!(!idx.is_empty(), "{label}: translite has no front face");
    for &i in &idx {
        near(
            q[i][2],
            f.t,
            TOL,
            &format!("{label} translite front-face vertex depth"),
        );
    }
    check_corner_uvs(m, f, &idx, label);
}

/// (e) The grille discs.
fn check_grilles(s: &GltfScene, g: &CabinetGeometry, f: &Face, label: &str) {
    let grilles = &g.backbox.grilles;
    let m = mesh(s, "cabinet.backbox.grilles");
    let q: Vec<[f64; 3]> = pts_mm(m).iter().map(|p| f.uvd(*p)).collect();
    assert!(!q.is_empty(), "{label}: grilles node is empty");
    for (i, gr) in grilles.iter().enumerate() {
        let r = f64::from(gr.diameter) / 2.0;
        let (cu, cv) = (f64::from(gr.u), f64::from(gr.v));
        let near_pts: Vec<f64> = q
            .iter()
            .map(|p| libm::hypot(p[0] - cu, p[1] - cv))
            .filter(|d| *d <= r + TOL)
            .collect();
        assert!(
            near_pts.len() >= 16,
            "{label}: grille {i} has {} vertices",
            near_pts.len()
        );
        let far = near_pts.iter().copied().fold(0.0, max_f64);
        near(far, r, TOL, &format!("{label} grille {i} radius"));
    }
    for p in &q {
        let ok = grilles.iter().any(|gr| {
            libm::hypot(p[0] - f64::from(gr.u), p[1] - f64::from(gr.v))
                <= f64::from(gr.diameter) / 2.0 + TOL
        });
        assert!(ok, "{label}: grille vertex {p:?} is not on any grille disc");
        // The disc protrudes from the face: nothing behind the face, nothing beyond 2 mm proud
        // (DECISIONS "#111 KP-030 (tester) — Grille discs 2 mm proud").
        assert!(
            p[2] <= TOL && p[2] >= -GRILLE_PROUD - TOL,
            "{label}: grille vertex depth {} outside [-2 mm, 0] (1 mm tol)",
            p[2]
        );
    }
    // The player-facing surface of every disc is at y = Y0 - 2 mm (flat), i.e. 2 mm proud, and
    // is a round disc: its area seen along the normal is that of a >= 16-gon of radius d/2.
    let pts = pts_mm(m);
    let mut area = vec![0.0; grilles.len()];
    for t in m.indices.chunks_exact(3) {
        let tri = [pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]];
        if tri_normal(&tri).is_none_or(|n| dot(n, f.n_out()) <= 0.99) {
            continue;
        }
        let q = tri.map(|p| f.uvd(p));
        for p in q {
            near(
                p[2],
                -GRILLE_PROUD,
                TOL,
                &format!("{label} grille front-surface depth (2 mm proud)"),
            );
        }
        let cu = (q[0][0] + q[1][0] + q[2][0]) / 3.0;
        let cv = (q[0][1] + q[1][1] + q[2][1]) / 3.0;
        let a = 0.5
            * ((q[1][0] - q[0][0]) * (q[2][1] - q[0][1])
                - (q[2][0] - q[0][0]) * (q[1][1] - q[0][1]))
                .abs();
        if let Some(i) = grilles.iter().position(|gr| {
            libm::hypot(cu - f64::from(gr.u), cv - f64::from(gr.v)) <= f64::from(gr.diameter) / 2.0
        }) {
            area[i] += a;
        }
    }
    for (i, gr) in grilles.iter().enumerate() {
        let r = f64::from(gr.diameter) / 2.0;
        let full = core::f64::consts::PI * r * r;
        let poly16 = 8.0 * r * r * libm::sin(2.0 * core::f64::consts::PI / 16.0);
        assert!(
            area[i] >= poly16 * 0.999 && area[i] <= full * 1.001,
            "{label}: grille {i} front area {} not a disc of radius {r} (>= 16 segments: {poly16}..{full})",
            area[i]
        );
    }
}

/// (f) The hinge.
fn check_hinge(s: &GltfScene, g: &CabinetGeometry, f: &Face, label: &str) {
    let m = mesh(s, "cabinet.backbox.hinge");
    let pts = pts_mm(m);
    assert!(
        !pts.is_empty() && !m.indices.is_empty(),
        "{label}: hinge empty"
    );
    assert!(
        pts.iter().flatten().all(|v| v.is_finite()),
        "{label}: hinge"
    );
    let (lo, hi) = bounds(&pts);
    near(lo[0], -hi[0], TOL, &format!("{label} hinge x symmetry"));
    assert!(
        lo[0] >= -f.w / 2.0 - TOL && hi[0] <= f.w / 2.0 + TOL,
        "{label}: hinge x {}..{} outside +-W/2",
        lo[0],
        hi[0]
    );
    assert!(
        lo[1] >= f.y0 - TOL && hi[1] <= f.y0 + f.d + TOL,
        "{label}: hinge y {}..{} outside [Y0, Y0 + D]",
        lo[1],
        hi[1]
    );
    let b = &g.body;
    let rear_top = g.floor_height_mm(f64::from(b.length), f64::from(b.rear_height));
    let z_lo = min_f64(f.zb, rear_top);
    assert!(
        lo[2] >= z_lo - TOL && hi[2] <= f.zb + 100.0 + TOL,
        "{label}: hinge z {}..{} outside [{z_lo}, {}]",
        lo[2],
        hi[2],
        f.zb + 100.0
    );
}

/// Contract E: z_bot(y) and z_top(y) of the blades, mm above the floor.
fn blade_z(g: &CabinetGeometry, y: f64) -> (f64, f64) {
    let m = &g.mount;
    let b = &g.body;
    let z_bot = g.floor_height_mm(
        y,
        f64::from(m.height) + (y - f64::from(m.inset)) * libm::tan(deg(f64::from(m.pitch_deg))),
    );
    let (fh, rh, l) = (
        f64::from(b.front_height),
        f64::from(b.rear_height),
        f64::from(b.length),
    );
    let w_top = fh + (rh - fh) * y / l;
    let z_top = g.floor_height_mm(y, w_top - 2.0 * f64::from(b.glass_channel.width));
    (z_bot, z_top)
}

/// (g) The art-blade wall panels.
fn check_blades(s: &GltfScene, g: &CabinetGeometry, label: &str) {
    let b = &g.body;
    let t = f64::from(b.wall_thickness);
    let inner = f64::from(b.width) / 2.0 - t;
    let left = mesh(s, "cabinet.blade.left");
    let right = mesh(s, "cabinet.blade.right");
    let (lp, rp) = (pts_mm(left), pts_mm(right));
    assert!(!lp.is_empty() && !rp.is_empty(), "{label}: blade empty");
    // Mirror images about x = 0.
    for (a, b2, what) in [(&lp, &rp, "left->right"), (&rp, &lp, "right->left")] {
        for p in a.iter() {
            let m = [-p[0], p[1], p[2]];
            let ok = b2.iter().any(|q| len(sub(*q, m)) <= TOL);
            assert!(ok, "{label}: blade {what} has no mirror of {p:?}");
        }
    }
    let y_lo = f64::from(g.mount.inset);
    let y_hi = f64::from(b.length) - t;
    for (pts, sign, side) in [(&lp, -1.0, "left"), (&rp, 1.0, "right")] {
        let (lo, hi) = bounds(pts);
        // The 3 mm band against the inner wall face, inside the interior.
        let (band_lo, band_hi) = if sign < 0.0 {
            (-inner, -inner + BLADE_THICKNESS)
        } else {
            (inner - BLADE_THICKNESS, inner)
        };
        assert!(
            lo[0] >= band_lo - TOL && hi[0] <= band_hi + TOL,
            "{label} {side} blade x {}..{} outside the band {band_lo}..{band_hi}",
            lo[0],
            hi[0]
        );
        near(
            hi[0] - lo[0],
            BLADE_THICKNESS,
            TOL,
            &format!("{label} {side} blade thickness"),
        );
        assert!(
            lo[1] >= y_lo - TOL && hi[1] <= y_hi + TOL,
            "{label} {side} blade y {}..{} outside [{y_lo}, {y_hi}]",
            lo[1],
            hi[1]
        );
        for p in pts.iter() {
            let (zb, zt) = blade_z(g, p[1]);
            assert!(
                p[2] >= zb - TOL && p[2] <= zt + TOL,
                "{label} {side} blade vertex {p:?} outside [{zb}, {zt}]"
            );
        }
    }
    // Inner faces' UVs cover [0, 1].
    for (m, nx, side) in [(left, 1.0, "left"), (right, -1.0, "right")] {
        let pts = pts_mm(m);
        let mut uvs = Vec::new();
        for t in m.indices.chunks_exact(3) {
            let tri = [pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]];
            if tri_normal(&tri).is_some_and(|n| n[0] * nx > 0.9) {
                uvs.extend(t.iter().map(|&i| m.uvs[i as usize]));
            }
        }
        assert!(!uvs.is_empty(), "{label} {side}: no inner face");
        for a in 0..2 {
            let lo = uvs
                .iter()
                .map(|u| f64::from(u[a]))
                .fold(f64::INFINITY, min_f64);
            let hi = uvs
                .iter()
                .map(|u| f64::from(u[a]))
                .fold(f64::NEG_INFINITY, max_f64);
            assert!(
                lo.abs() < 1e-3 && (hi - 1.0).abs() < 1e-3,
                "{label} {side} blade inner-face UV axis {a}: {lo}..{hi}"
            );
        }
    }
}

/// Contract C and section (k): every new node is static with an identity transform, one
/// triangle primitive with finite data, UVs in [0, 1], indices in range, and counter-clockwise
/// front faces (winding normal agrees with the vertex normals).
fn check_new_node_meshes(s: &GltfScene, label: &str) {
    for name in &NODES[28..] {
        if *name == "cabinet.backbox.grilles" && !s.meshes.iter().any(|m| m.name == *name) {
            continue;
        }
        let m = mesh(s, name);
        assert!(is_identity(m), "{label} {name}: not an identity transform");
        assert!(!m.positions.is_empty(), "{label} {name}: no positions");
        assert_eq!(m.normals.len(), m.positions.len(), "{label} {name}");
        assert_eq!(m.uvs.len(), m.positions.len(), "{label} {name}");
        assert!(m.indices.len().is_multiple_of(3) && !m.indices.is_empty());
        assert!(m.indices.iter().all(|&i| (i as usize) < m.positions.len()));
        let pts = pts_mm(m);
        for t in m.indices.chunks_exact(3) {
            let tri = [pts[t[0] as usize], pts[t[1] as usize], pts[t[2] as usize]];
            let Some(n) = tri_normal(&tri) else { continue };
            let mut vn = [0.0; 3];
            for &i in t {
                for (acc, c) in vn.iter_mut().zip(m.normals[i as usize]) {
                    *acc += f64::from(c);
                }
            }
            assert!(
                dot(n, vn) > 0.0,
                "{label} {name}: triangle {t:?} winding disagrees with its normals"
            );
        }
    }
}

/// Runs (a) to (g) on `g`.
fn check_all(g: &CabinetGeometry, label: &str) -> (GeneratedCabinet, GltfScene) {
    let (cab, s) = gen_scene(g, label);
    let f = Face::of(g);
    check_new_node_meshes(&s, label);
    check_bounds_set(&s, g, &f, label);
    check_openings(&s, g, &f, label);
    check_display(&s, g, &f, label);
    check_translite(&s, g, &f, label);
    check_grilles(&s, g, &f, label);
    check_hinge(&s, g, &f, label);
    check_blades(&s, g, label);
    (cab, s)
}

// ---------------------------------------------------------------------------
// glTF JSON chunk (for the node list, which must list every name even for a mesh-less node).
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
enum Json {
    Null,
    Bool(bool),
    Num(f64),
    Str(String),
    Arr(Vec<Json>),
    Obj(Vec<(String, Json)>),
}

impl Json {
    fn get(&self, k: &str) -> Option<&Json> {
        match self {
            Json::Obj(v) => v.iter().find(|(n, _)| n == k).map(|(_, j)| j),
            _ => None,
        }
    }
    fn arr(&self) -> &[Json] {
        match self {
            Json::Arr(v) => v,
            _ => &[],
        }
    }
    fn str(&self) -> Option<&str> {
        match self {
            Json::Str(s) => Some(s),
            _ => None,
        }
    }
}

struct Parser<'a> {
    b: &'a [u8],
    i: usize,
}

impl Parser<'_> {
    fn ws(&mut self) {
        while self.i < self.b.len() && self.b[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }
    fn value(&mut self) -> Json {
        self.ws();
        match self.b[self.i] {
            b'{' => {
                self.i += 1;
                let mut v = Vec::new();
                loop {
                    self.ws();
                    if self.b[self.i] == b'}' {
                        self.i += 1;
                        return Json::Obj(v);
                    }
                    let Json::Str(k) = self.value() else {
                        panic!("JSON key")
                    };
                    self.ws();
                    assert_eq!(self.b[self.i], b':');
                    self.i += 1;
                    let val = self.value();
                    v.push((k, val));
                    self.ws();
                    if self.b[self.i] == b',' {
                        self.i += 1;
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut v = Vec::new();
                loop {
                    self.ws();
                    if self.b[self.i] == b']' {
                        self.i += 1;
                        return Json::Arr(v);
                    }
                    v.push(self.value());
                    self.ws();
                    if self.b[self.i] == b',' {
                        self.i += 1;
                    }
                }
            }
            b'"' => {
                self.i += 1;
                let mut out = Vec::new();
                while self.b[self.i] != b'"' {
                    if self.b[self.i] == b'\\' {
                        self.i += 1;
                        let c = self.b[self.i];
                        match c {
                            b'n' => out.push(b'\n'),
                            b't' => out.push(b'\t'),
                            b'u' => {
                                // Not expected in generated names; keep the escape verbatim.
                                out.extend_from_slice(b"\\u");
                            }
                            _ => out.push(c),
                        }
                    } else {
                        out.push(self.b[self.i]);
                    }
                    self.i += 1;
                }
                self.i += 1;
                Json::Str(String::from_utf8(out).unwrap())
            }
            b't' => {
                self.i += 4;
                Json::Bool(true)
            }
            b'f' => {
                self.i += 5;
                Json::Bool(false)
            }
            b'n' => {
                self.i += 4;
                Json::Null
            }
            _ => {
                let st = self.i;
                while self.i < self.b.len() && b"+-0123456789.eE".contains(&self.b[self.i]) {
                    self.i += 1;
                }
                let t = std::str::from_utf8(&self.b[st..self.i]).unwrap();
                Json::Num(t.parse().unwrap_or_else(|_| panic!("JSON number {t:?}")))
            }
        }
    }
}

fn glb_json(glb: &[u8]) -> Json {
    assert_eq!(&glb[0..4], b"glTF");
    let n = u32::from_le_bytes(glb[12..16].try_into().unwrap()) as usize;
    assert_eq!(&glb[16..20], b"JSON");
    let mut p = Parser {
        b: &glb[20..20 + n],
        i: 0,
    };
    p.value()
}

fn json_names(j: &Json, key: &str) -> Vec<String> {
    j.get(key)
        .map(|a| {
            a.arr()
                .iter()
                .map(|n| n.get("name").and_then(Json::str).unwrap_or("").to_owned())
                .collect()
        })
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Tests.
// ---------------------------------------------------------------------------

/// REQ-CAB-001 / REQ-CAB-003, acceptance "backbox size and opening positions match the preset
/// within 1 mm; the display window aspect matches the score display mode within 2 %"
/// (contract A, C, D, E): every preset passes checks (a) to (g).
#[test]
fn req_cab_001_backbox_matches_every_preset() {
    for id in PRESETS {
        let g = preset(id);
        assert_eq!(g.backbox.front_profile, BackboxFrontProfile::Flat);
        check_all(&g, id);
    }
}

/// REQ-CAB-001 (SPEC §7.3 "speaker panel with two speaker grilles"; Appendix E): sanity of the
/// preset numbers this suite derives its expectations from, and the absolute backbox top
/// (1930 mm classic, 1911 mm modern) and the DMD / LCD display aspects.
#[test]
fn req_cab_001_preset_sanity_absolute_numbers() {
    for (id, top, kind, px) in [
        ("classic_standard", 1930.0, ScoreDisplayKind::Dmd, [128, 32]),
        ("classic_widebody", 1930.0, ScoreDisplayKind::Dmd, [128, 32]),
        (
            "modern_standard",
            1911.0,
            ScoreDisplayKind::Lcd,
            [1920, 1080],
        ),
    ] {
        let g = preset(id);
        assert_eq!(g.backbox.grilles.len(), 2, "{id}: two grilles");
        assert_eq!(g.backbox.score_display.kind, kind);
        assert_eq!(g.backbox.score_display.pixels, px);
        let (_, s) = gen_scene(&g, id);
        let mut all = Vec::new();
        for n in BOUNDS_SET {
            all.extend(pts_mm(mesh(&s, n)));
        }
        let (lo, hi) = bounds(&all);
        near(hi[2], top, TOL, &format!("{id} backbox top"));
        near(hi[0] - lo[0], f64::from(g.backbox.width), TOL, id);
    }
}

/// REQ-CAB-001 / REQ-CAB-003 (contract C, item 33): overrides with 1, 3 and 8 grilles give that
/// many discs, each at its data position and diameter.
#[test]
fn req_cab_003_grille_count_is_data() {
    let one = "[backbox]\ngrilles = [{ u = 0.0, v = 30.0, diameter = 40.0 }]\n".to_owned();
    let three = "[backbox]\ngrilles = [{ u = -290.0, v = 100.0, diameter = 100.0 }, \
                 { u = 290.0, v = 100.0, diameter = 100.0 }, \
                 { u = 0.0, v = 30.0, diameter = 40.0 }]\n"
        .to_owned();
    let mut eight = "[backbox]\ngrilles = [".to_owned();
    for k in 0..8 {
        if k > 0 {
            eight += ", ";
        }
        eight += &format!(
            "{{ u = {:?}, v = 170.0, diameter = 40.0 }}",
            -297.5 + 85.0 * f64::from(k)
        );
    }
    eight += "]\n";
    for id in PRESETS {
        for (n, text) in [(1, &one), (3, &three), (8, &eight)] {
            let g = from_toml(text, id);
            assert_eq!(g.backbox.grilles.len(), n);
            check_all(&g, &format!("{id} {n} grilles"));
        }
    }
}

/// REQ-CAB-001 (contract G, zero grilles): either Ok with the JSON `nodes` array still listing
/// all 37 names (and the file loads), or `GenError` naming `backbox.grilles`; never a panic.
#[test]
fn req_cab_001_zero_grilles_ok_with_37_names_or_named_error() {
    for id in PRESETS {
        let mut g = preset(id);
        g.backbox.grilles.clear();
        match generate_cabinet(&g) {
            Ok(cab) => {
                let j = glb_json(&cab.glb);
                assert_eq!(json_names(&j, "nodes"), NODES.to_vec(), "{id}: JSON nodes");
                assert_eq!(cab.nodes, NODES.to_vec(), "{id}: GeneratedCabinet::nodes");
                let _ = load(&cab);
            }
            Err(e) => assert_eq!(e.element, "backbox.grilles", "{id}: {}", e.message),
        }
    }
}

/// Contract A and B profiles (REQ-CAB-003 "front profile (flat or angled), top profile (flat,
/// angled or arched)"), section (h): every profile combination generates, the bounds set still
/// reaches zb + H and spans [Y0, Y0 + D], a non-flat top dips below the apex by the contract's
/// amount, and the display quad keeps its data size; the angled-front display tilts up 10 deg.
#[test]
fn req_cab_003_front_and_top_profiles() {
    let fronts = [BackboxFrontProfile::Flat, BackboxFrontProfile::Angled];
    let tops = [
        BackboxTopProfile::Flat,
        BackboxTopProfile::Angled,
        BackboxTopProfile::Arched,
    ];
    for id in PRESETS {
        for front in fronts {
            for top in tops {
                let mut g = preset(id);
                g.backbox.front_profile = front;
                g.backbox.top_profile = top;
                if top == BackboxTopProfile::Arched {
                    lower_opening_for_arch(&mut g);
                }
                let label = format!("{id} {front:?} front, {top:?} top");
                let (_, s) = check_all(&g, &label);
                let f = Face::of(&g);
                let shell = pts_mm(mesh(&s, "cabinet.backbox.shell"));
                match top {
                    BackboxTopProfile::Flat => {}
                    BackboxTopProfile::Angled => {
                        let rear: Vec<f64> = shell
                            .iter()
                            .filter(|p| p[1] >= f.y0 + f.d - 0.5)
                            .map(|p| p[2])
                            .collect();
                        let z = rear.iter().copied().fold(f64::NEG_INFINITY, max_f64);
                        assert!(z < f.zb + f.h - TOL, "{label}: rear top {z} not below apex");
                        near(
                            z,
                            f.z_top(0.0, f.y0 + f.d),
                            TOL,
                            &format!("{label} rear top height"),
                        );
                    }
                    BackboxTopProfile::Arched => {
                        let side: Vec<f64> = shell
                            .iter()
                            .filter(|p| p[0].abs() >= f.w / 2.0 - 0.5)
                            .map(|p| p[2])
                            .collect();
                        let z = side.iter().copied().fold(f64::NEG_INFINITY, max_f64);
                        assert!(
                            z < f.zb + f.h - TOL,
                            "{label}: crown edge {z} not below apex"
                        );
                        near(
                            z,
                            f.zb + f.h - f.crown(),
                            TOL,
                            &format!("{label} arched side top height"),
                        );
                    }
                }
                if front == BackboxFrontProfile::Angled {
                    let m = mesh(&s, "cabinet.backbox.display");
                    let pts = pts_mm(m);
                    let (lo, hi) = bounds(&pts);
                    let sd = &g.backbox.score_display;
                    near(
                        hi[0] - lo[0],
                        f64::from(sd.width),
                        TOL,
                        &format!("{label} 3D width"),
                    );
                    near(
                        (hi[2] - lo[2]) / libm::cos(deg(FRONT_LEAN_DEG)),
                        f64::from(sd.height),
                        TOL,
                        &format!("{label} in-plane height from vertical extent"),
                    );
                    for t in tris_mm(m) {
                        let n = tri_normal(&t).unwrap();
                        let tilt = libm::asin(n[2]) * 180.0 / core::f64::consts::PI;
                        assert!(
                            n[1] < 0.0,
                            "{label}: display normal {n:?} not toward the player"
                        );
                        near(
                            tilt,
                            FRONT_LEAN_DEG,
                            1.0,
                            &format!("{label} display tilt deg"),
                        );
                    }
                }
            }
        }
    }
}

/// Moves the translite opening (and backglass) down so the arched crown at +-opening width/2
/// clears the opening's top edge (contract B), keeping pin_core's rules.
fn lower_opening_for_arch(g: &mut CabinetGeometry) {
    let bb = &mut g.backbox;
    let w = f64::from(bb.width);
    let h = f64::from(bb.height);
    let ow = f64::from(bb.translite_opening.width);
    let crown_at = h - 0.06 * w * (ow / w) * (ow / w);
    let top = f64::from(bb.translite_opening.bottom + bb.translite_opening.height);
    if top > crown_at - 2.0 {
        let new_h = (crown_at - 2.0 - f64::from(bb.translite_opening.bottom)) as f32;
        bb.translite_opening.height = new_h;
    }
}

/// Contract B error rule exercised on the preset data: modern_standard's opening top edge is
/// above the arched crown at +-opening width/2, so arched there is `backbox.top_profile`.
/// (Recorded in DECISIONS: "#111 KP-030 (tester) — Arched top on modern_standard".)
#[test]
fn req_cab_003_modern_standard_arched_is_top_profile_error() {
    let mut g = preset("modern_standard");
    let bb = &g.backbox;
    let w = f64::from(bb.width);
    let ow = f64::from(bb.translite_opening.width);
    let crown_at = f64::from(bb.height) - 0.06 * w * (ow / w) * (ow / w);
    let top = f64::from(bb.translite_opening.bottom + bb.translite_opening.height);
    assert!(top > crown_at, "precondition: {top} vs {crown_at}");
    g.backbox.top_profile = BackboxTopProfile::Arched;
    match generate_cabinet(&g) {
        Ok(_) => panic!("arched top below the opening accepted"),
        Err(e) => assert_eq!(e.element, "backbox.top_profile", "{}", e.message),
    }
}

fn same_mesh(a: &MeshData, b: &MeshData) -> bool {
    a.name == b.name
        && a.positions == b.positions
        && a.normals == b.normals
        && a.uvs == b.uvs
        && a.indices == b.indices
        && a.material == b.material
        && a.transform == b.transform
}

/// REQ-CAB-003 "nothing about the cabinet is hard-coded" (contract A to E, section (i)): each
/// `[backbox]` key overridden through `cabinet.toml` text moves the matching geometry (checks
/// (a) to (g) hold against the new data, and the file changes) and leaves the first 28 nodes'
/// meshes identical.
#[test]
fn req_cab_003_backbox_overrides_are_data() {
    let overrides: [(&str, &str); 19] = [
        ("width +", "[backbox]\nwidth = 760.0\n"),
        ("width -", "[backbox]\nwidth = 700.0\n"),
        ("height", "[backbox]\nheight = 760.0\n"),
        ("depth", "[backbox]\ndepth = 230.0\n"),
        ("front_y", "[backbox]\nfront_y = 1045.0\n"),
        ("bottom_z", "[backbox]\nbottom_z = 735.0\n"),
        (
            "speaker_panel_height",
            "[backbox]\nspeaker_panel_height = 190.0\n",
        ),
        ("backglass.width", "[backbox.backglass]\nwidth = 690.0\n"),
        ("backglass.height", "[backbox.backglass]\nheight = 470.0\n"),
        (
            "backglass.thickness",
            "[backbox.backglass]\nthickness = 4.0\n",
        ),
        (
            "translite_opening.width",
            "[backbox.translite_opening]\nwidth = 600.0\n",
        ),
        (
            "translite_opening.height",
            "[backbox.translite_opening]\nheight = 420.0\n",
        ),
        (
            "translite_opening.bottom",
            "[backbox.translite_opening]\nbottom = 230.0\n",
        ),
        (
            "score_display lcd",
            "[backbox.score_display]\nkind = \"lcd\"\npixels = [1920, 1080]\nwidth = 320.0\n\
             height = 180.0\ncentre_v = 100.0\n",
        ),
        (
            "score_display.width",
            "[backbox.score_display]\nwidth = 300.0\nheight = 75.0\n",
        ),
        (
            "score_display.centre_v",
            "[backbox.score_display]\ncentre_v = 110.0\n",
        ),
        (
            "grilles",
            "[backbox]\ngrilles = [{ u = -250.0, v = 90.0, diameter = 120.0 }, \
             { u = 250.0, v = 90.0, diameter = 120.0 }]\n",
        ),
        ("front_profile", "[backbox]\nfront_profile = \"angled\"\n"),
        ("top_profile", "[backbox]\ntop_profile = \"angled\"\n"),
    ];
    let base_g = preset("classic_standard");
    let (base_cab, base_s) = gen_scene(&base_g, "classic_standard");
    for (what, text) in overrides {
        let g = from_toml(text, "classic_standard");
        assert_ne!(g.backbox, base_g.backbox, "{what}: override had no effect");
        if what == "score_display lcd" {
            assert_eq!(g.backbox.score_display.kind, ScoreDisplayKind::Lcd);
        }
        let (cab, s) = check_all(&g, what);
        assert_ne!(cab.glb, base_cab.glb, "{what}: file unchanged");
        for i in 0..28 {
            assert!(
                same_mesh(&s.meshes[i], &base_s.meshes[i]),
                "{what}: node {} changed with a pure backbox override",
                s.meshes[i].name
            );
        }
        // The new nodes moved: at least one of the backbox nodes differs.
        let moved = (28..35).any(|i| !same_mesh(&s.meshes[i], &base_s.meshes[i]));
        assert!(moved, "{what}: no backbox node changed");
    }
}

/// Contract E / REQ-CAB-002 surface list (via REQ-CAB-001's brief): the blades follow the mount
/// data (inset, height, pitch) and the body.
#[test]
fn req_cab_003_blades_follow_mount_and_body() {
    for id in PRESETS {
        for text in [
            "[mount]\ninset = 60.0\n",
            "[mount]\nheight = 200.0\n",
            "[mount]\npitch_deg = 4.0\n",
            "[body]\nrear_height = 620.0\n",
        ] {
            let g = from_toml(text, id);
            let (_, s) = gen_scene(&g, id);
            check_blades(&s, &g, &format!("{id} {text:?}"));
        }
    }
}

type Mut = Box<dyn Fn(&mut CabinetGeometry)>;

fn expect_err(g: &CabinetGeometry, key: &str, label: &str) {
    match generate_cabinet(g) {
        Ok(_) => panic!("{label}: accepted, expected error {key}"),
        Err(e) => assert_eq!(e.element, key, "{label}: {}", e.message),
    }
}

/// Contract G.1 value checks (REQ-CAB-003, R11): NaN, infinities, 0, negative and above
/// MAX_DIMENSION_MM for each key name exactly that key.
#[test]
fn contract_g1_value_checks_name_the_key() {
    let max = MAX_DIMENSION_MM;
    let size_bad = [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        0.0,
        -1.0,
        max * 2.0,
    ];
    let nonneg_bad = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, max * 2.0];
    let pos_bad = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.0, max * 2.0];
    let u_bad = [
        f32::NAN,
        f32::INFINITY,
        f32::NEG_INFINITY,
        max * 2.0,
        -max * 2.0,
    ];
    type Set = fn(&mut CabinetGeometry, f32);
    let keys: [(&str, Set, &[f32]); 20] = [
        ("backbox.width", |g, v| g.backbox.width = v, &size_bad),
        ("backbox.height", |g, v| g.backbox.height = v, &size_bad),
        ("backbox.depth", |g, v| g.backbox.depth = v, &size_bad),
        ("backbox.front_y", |g, v| g.backbox.front_y = v, &nonneg_bad),
        (
            "backbox.bottom_z",
            |g, v| g.backbox.bottom_z = v,
            &nonneg_bad,
        ),
        (
            "backbox.speaker_panel_height",
            |g, v| g.backbox.speaker_panel_height = v,
            &size_bad,
        ),
        (
            "backbox.backglass.width",
            |g, v| g.backbox.backglass.width = v,
            &size_bad,
        ),
        (
            "backbox.backglass.height",
            |g, v| g.backbox.backglass.height = v,
            &size_bad,
        ),
        (
            "backbox.backglass.thickness",
            |g, v| g.backbox.backglass.thickness = v,
            &size_bad,
        ),
        (
            "backbox.translite_opening.width",
            |g, v| g.backbox.translite_opening.width = v,
            &size_bad,
        ),
        (
            "backbox.translite_opening.height",
            |g, v| g.backbox.translite_opening.height = v,
            &size_bad,
        ),
        (
            "backbox.translite_opening.bottom",
            |g, v| g.backbox.translite_opening.bottom = v,
            &pos_bad,
        ),
        (
            "backbox.score_display.width",
            |g, v| g.backbox.score_display.width = v,
            &size_bad,
        ),
        (
            "backbox.score_display.height",
            |g, v| g.backbox.score_display.height = v,
            &size_bad,
        ),
        (
            "backbox.score_display.centre_v",
            |g, v| g.backbox.score_display.centre_v = v,
            &pos_bad,
        ),
        (
            "backbox.grilles[1].u",
            |g, v| g.backbox.grilles[1].u = v,
            &u_bad,
        ),
        (
            "backbox.grilles[1].v",
            |g, v| g.backbox.grilles[1].v = v,
            &pos_bad,
        ),
        (
            "backbox.grilles[1].diameter",
            |g, v| g.backbox.grilles[1].diameter = v,
            &size_bad,
        ),
        (
            "backbox.grilles[0].v",
            |g, v| g.backbox.grilles[0].v = v,
            &[0.0],
        ),
        (
            "backbox.score_display.centre_v",
            |g, v| g.backbox.score_display.centre_v = v,
            &[0.0],
        ),
    ];
    for id in PRESETS {
        for (key, set, bad) in &keys {
            for &v in *bad {
                let mut g = preset(id);
                set(&mut g, v);
                expect_err(&g, key, &format!("{id} {key} = {v}"));
            }
        }
        for px in [[0, 32], [128, 0], [0, 0]] {
            let mut g = preset(id);
            g.backbox.score_display.pixels = px;
            expect_err(
                &g,
                "backbox.score_display.pixels",
                &format!("{id} pixels {px:?}"),
            );
        }
        let mut g = preset(id);
        g.backbox.grilles = (0..9)
            .map(|k| Grille {
                u: -320.0 + 80.0 * k as f32,
                v: 170.0,
                diameter: 20.0,
            })
            .collect();
        expect_err(&g, "backbox.grilles", &format!("{id} 9 grilles"));
        // Control: 8 of the same grilles are accepted.
        g.backbox.grilles.truncate(8);
        gen_scene(&g, &format!("{id} 8 grilles control"));
    }
}

/// Contract G.2 fit checks and G.3 profile checks (REQ-CAB-003): each case breaks exactly one
/// rule and names exactly its key.
#[test]
fn contract_g2_g3_fit_and_profile_checks_name_the_key() {
    let tan10 = libm::tan(deg(FRONT_LEAN_DEG));
    let cases: Vec<(&str, Mut)> = vec![
        (
            "backbox.translite_opening.width",
            Box::new(|g| g.backbox.translite_opening.width = g.backbox.width + 10.0),
        ),
        (
            "backbox.translite_opening.bottom",
            Box::new(|g| {
                let bb = &mut g.backbox;
                let h = bb.translite_opening.height;
                bb.translite_opening.bottom = bb.height - h + 5.0;
                // Backglass shorter than the opening so its own v extent stays inside.
                bb.backglass.height = h - 40.0;
            }),
        ),
        (
            "backbox.score_display.width",
            Box::new(|g| g.backbox.score_display.width = g.backbox.width + 10.0),
        ),
        (
            "backbox.score_display.centre_v",
            Box::new(|g| {
                let bb = &mut g.backbox;
                bb.score_display.centre_v =
                    bb.speaker_panel_height - bb.score_display.height / 2.0 + 5.0;
            }),
        ),
        (
            "backbox.grilles[0].u",
            Box::new(|g| {
                let bb = &mut g.backbox;
                let r = bb.grilles[0].diameter / 2.0;
                bb.grilles[0].u = -(bb.width / 2.0 - r + 5.0);
            }),
        ),
        (
            "backbox.grilles[0].v",
            Box::new(|g| {
                let bb = &mut g.backbox;
                let r = bb.grilles[0].diameter / 2.0;
                bb.grilles[0].v = bb.height - r + 5.0;
            }),
        ),
        (
            "backbox.grilles[1].v",
            Box::new(|g| {
                let r = g.backbox.grilles[1].diameter / 2.0;
                g.backbox.grilles[1].v = r - 5.0;
            }),
        ),
        (
            "backbox.backglass.width",
            Box::new(|g| g.backbox.backglass.width = g.backbox.width + 10.0),
        ),
        (
            "backbox.backglass.height",
            Box::new(|g| g.backbox.backglass.height = 2.0 * g.backbox.height),
        ),
        (
            "backbox.front_profile",
            Box::new(move |g| {
                let bb = &mut g.backbox;
                let t = f64::from(g.body.wall_thickness);
                bb.front_profile = BackboxFrontProfile::Angled;
                bb.depth = (2.0 * t + f64::from(bb.height) * tan10 * 0.5) as f32;
            }),
        ),
        (
            "backbox.top_profile",
            Box::new(|g| {
                let bb = &mut g.backbox;
                let h = bb.translite_opening.height;
                bb.translite_opening.bottom = bb.height - h - 5.0;
                bb.backglass.height = h + 6.0;
                bb.top_profile = BackboxTopProfile::Arched;
            }),
        ),
    ];
    for id in PRESETS {
        for (key, edit) in &cases {
            let mut g = preset(id);
            edit(&mut g);
            expect_err(&g, key, &format!("{id} {key}"));
        }
        // Controls: the profile cases are accepted with the default profiles.
        let mut g = preset(id);
        let t = f64::from(g.body.wall_thickness);
        g.backbox.depth = (2.0 * t + f64::from(g.backbox.height) * tan10 * 0.5) as f32;
        gen_scene(&g, &format!("{id} shallow flat-front control"));
        let mut g = preset(id);
        let h = g.backbox.translite_opening.height;
        g.backbox.translite_opening.bottom = g.backbox.height - h - 5.0;
        g.backbox.backglass.height = h + 6.0;
        gen_scene(&g, &format!("{id} raised opening flat-top control"));
    }
}

/// Contract E / G.3 blade checks: `mount.inset`, `mount.height`, `mount.pitch_deg`.
#[test]
fn contract_g3_blade_checks_name_the_key() {
    type Set = fn(&mut CabinetGeometry);
    let cases: [(&str, Set); 13] = [
        ("mount.inset", |g| g.mount.inset = -5.0),
        ("mount.inset", |g| g.mount.inset = f32::NAN),
        ("mount.inset", |g| g.mount.inset = f32::INFINITY),
        ("mount.inset", |g| {
            g.mount.inset = g.body.length - g.body.wall_thickness;
        }),
        ("mount.inset", |g| {
            g.mount.inset = g.body.length - g.body.wall_thickness + 10.0;
        }),
        ("mount.height", |g| g.mount.height = 0.0),
        ("mount.height", |g| g.mount.height = -10.0),
        ("mount.height", |g| g.mount.height = f32::NAN),
        ("mount.height", |g| {
            // Above the side wall top everywhere: no blade height remains.
            g.mount.height = max_f32(g.body.rear_height, g.body.front_height) + 100.0;
            g.mount.pitch_deg = 0.0;
        }),
        ("mount.pitch_deg", |g| g.mount.pitch_deg = f32::NAN),
        ("mount.pitch_deg", |g| g.mount.pitch_deg = f32::INFINITY),
        // |pitch| >= 89, nose-down so z_top - z_bot stays positive and only the pitch rule
        // fails (brief: "build the case so only the intended rule fails").
        ("mount.pitch_deg", |g| g.mount.pitch_deg = -89.0),
        ("mount.pitch_deg", |g| g.mount.pitch_deg = -120.0),
    ];
    for id in PRESETS {
        for (key, edit) in &cases {
            let mut g = preset(id);
            edit(&mut g);
            expect_err(&g, key, &format!("{id} {key}"));
        }
    }
}

/// Section (k): the JSON node and material name lists equal `NODE_NAMES` (37) and
/// `MATERIAL_NAMES` (24) in order; contract F's node-to-material mapping and material
/// properties; art materials white and opaque.
#[test]
fn contract_f_materials_and_stable_lists() {
    assert_eq!(NODE_NAMES.len(), 37);
    assert_eq!(MATERIAL_NAMES.len(), 24);
    assert_eq!(NODE_NAMES.to_vec(), NODES.to_vec());
    assert_eq!(MATERIAL_NAMES.to_vec(), MATERIALS.to_vec());
    for id in PRESETS {
        let (cab, s) = gen_scene(&preset(id), id);
        let j = glb_json(&cab.glb);
        assert_eq!(json_names(&j, "nodes"), NODES.to_vec(), "{id}: JSON nodes");
        assert_eq!(
            json_names(&j, "materials"),
            MATERIALS.to_vec(),
            "{id}: JSON materials"
        );
        let mats: Vec<&str> = s.materials.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(mats, MATERIALS.to_vec(), "{id}: loaded materials");
        for (node, mat) in NEW_NODE_MATERIAL {
            let m = mesh(&s, node);
            let idx = m
                .material
                .unwrap_or_else(|| panic!("{id} {node}: no material"));
            assert_eq!(s.materials[idx].name, mat, "{id} {node}");
        }
        let jm = j.get("materials").unwrap().arr();
        for name in [
            "cabinet.backbox",
            "cabinet.backbox.translite",
            "cabinet.backbox.display",
            "cabinet.blade_left",
            "cabinet.blade_right",
        ] {
            let m = s.materials.iter().find(|m| m.name == name).unwrap();
            for (k, c) in m.base_color.iter().enumerate() {
                assert!(
                    (c - 1.0).abs() < 1e-6,
                    "{id} {name}: base colour[{k}] = {c}"
                );
            }
            let jmat = jm
                .iter()
                .find(|x| x.get("name").and_then(Json::str) == Some(name))
                .unwrap();
            let mode = jmat
                .get("alphaMode")
                .and_then(Json::str)
                .unwrap_or("OPAQUE");
            assert_eq!(mode, "OPAQUE", "{id} {name}: alphaMode");
        }
        let bbm = s
            .materials
            .iter()
            .find(|m| m.name == "cabinet.backbox")
            .unwrap();
        assert!(bbm.metallic.abs() < 1e-6, "{id}: cabinet.backbox metallic");
        assert!(
            (bbm.roughness - 0.5).abs() < 1e-6,
            "{id}: cabinet.backbox roughness"
        );
        let sp = s
            .materials
            .iter()
            .find(|m| m.name == "cabinet.backbox.speaker_panel")
            .unwrap();
        assert!(
            sp.base_color[..3].iter().all(|c| *c <= 0.1),
            "{id}: speaker panel not near-black: {:?}",
            sp.base_color
        );
        assert!(
            (sp.roughness - 0.7).abs() < 1e-6,
            "{id}: speaker panel roughness"
        );
        for m in &s.materials {
            assert!(
                m.base_color.iter().all(|v| v.is_finite()),
                "{id} {}",
                m.name
            );
        }
    }
}

/// Section (k), R3 / R11: the same geometry twice gives identical bytes (presets and profile
/// variants); every float finite, every UV in [0, 1], indices in range, for all 37 nodes.
#[test]
fn deterministic_bytes_and_finite_output() {
    for id in PRESETS {
        for front in [BackboxFrontProfile::Flat, BackboxFrontProfile::Angled] {
            for top in [
                BackboxTopProfile::Flat,
                BackboxTopProfile::Angled,
                BackboxTopProfile::Arched,
            ] {
                let mut g = preset(id);
                g.backbox.front_profile = front;
                g.backbox.top_profile = top;
                if top == BackboxTopProfile::Arched {
                    lower_opening_for_arch(&mut g);
                }
                let label = format!("{id} {front:?} {top:?}");
                let a = generate_cabinet(&g).unwrap_or_else(|e| panic!("{label}: {e:?}"));
                let b = generate_cabinet(&g).unwrap_or_else(|e| panic!("{label}: {e:?}"));
                assert_eq!(a.glb, b.glb, "{label}: bytes differ");
                assert_eq!(a.nodes, NODES.to_vec(), "{label}: nodes");
                let s = load(&a);
                let names: Vec<&str> = s.meshes.iter().map(|m| m.name.as_str()).collect();
                assert_eq!(names, NODES.to_vec(), "{label}: one mesh per node");
                for m in &s.meshes {
                    let n = &m.name;
                    assert!(
                        m.positions.iter().flatten().all(|v| v.is_finite()),
                        "{label} {n}"
                    );
                    assert!(
                        m.normals.iter().flatten().all(|v| v.is_finite()),
                        "{label} {n}"
                    );
                    assert!(
                        m.uvs
                            .iter()
                            .flatten()
                            .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
                        "{label} {n}: UV outside [0, 1]"
                    );
                    assert!(
                        m.indices.iter().all(|&i| (i as usize) < m.positions.len()),
                        "{label} {n}: index out of range"
                    );
                }
            }
        }
    }
}
