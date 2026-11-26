//! KP-030 (#111): the backbox and the inside art blades in `cabinet.glb`
//! (SPEC §7.3, REQ-CAB-001, REQ-CAB-002 surfaces, REQ-CAB-003), checked
//! analytically against the backbox model contract: the node and material
//! lists, the bounds, the translite opening and score-display window (by
//! front-facing triangle coverage), the display and translite quads and
//! their UVs, the grilles, the hinge, the profiles, the blades, data-driven
//! overrides and the error keys.

#[path = "cabinet_common.rs"]
mod common;

use common::*;
use pin_assets::{GltfScene, MeshData};
use pin_core::fmath::{max_f32, max_f64, min_f32, min_f64};
use pin_core::schema::cabinet::{
    BackboxFrontProfile, BackboxTopProfile, CabinetGeometry, ScoreDisplayKind,
};
use pin_tablegen::cabinet::{MATERIAL_NAMES, MAX_DIMENSION_MM, NODE_NAMES, generate_cabinet};

/// The nine KP-030 nodes, in file order, with their materials (section F).
const NEW_NODES: [(&str, &str); 9] = [
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

/// The bounds set of the contract (section C).
const BOUNDS_SET: [&str; 3] = [
    "cabinet.backbox.shell",
    "cabinet.backbox.speaker_panel",
    "cabinet.backbox.translite_frame",
];

/// The face frame of the contract (section A), computed independently of
/// the generator: `(u, v, w)` of a cabinet-frame point in mm.
#[derive(Clone, Copy, Debug)]
struct Face {
    y0: f64,
    zb: f64,
    s: f64,
    c: f64,
}

impl Face {
    fn of(g: &CabinetGeometry) -> Face {
        let bb = &g.backbox;
        let (s, c) = match bb.front_profile {
            BackboxFrontProfile::Flat => (0.0, 1.0),
            BackboxFrontProfile::Angled => {
                let a = 10.0f64.to_radians();
                (libm::sin(a), libm::cos(a))
            }
        };
        Face {
            y0: f64::from(bb.front_y),
            zb: g.floor_height_mm(f64::from(bb.front_y), f64::from(bb.bottom_z)),
            s,
            c,
        }
    }

    /// `(u, v, w)`: across, up the face, behind the face.
    fn uvw(&self, p: [f64; 3]) -> [f64; 3] {
        let (dy, dz) = (p[1] - self.y0, p[2] - self.zb);
        [p[0], dy * self.s + dz * self.c, dy * self.c - dz * self.s]
    }

    /// Outward normal of the face.
    fn out(&self) -> [f64; 3] {
        [0.0, -self.c, self.s]
    }
}

fn mm(p: [f32; 3]) -> [f64; 3] {
    p.map(|c| f64::from(c) * 1000.0)
}

/// Vertices of a node in mm (cabinet frame).
fn verts(scene: &GltfScene, name: &str) -> Vec<[f64; 3]> {
    world(mesh(scene, name)).into_iter().map(mm).collect()
}

/// Bounds in mm of a vertex list.
fn extent(ps: &[[f64; 3]]) -> ([f64; 3], [f64; 3]) {
    let mut lo = [f64::INFINITY; 3];
    let mut hi = [f64::NEG_INFINITY; 3];
    for p in ps {
        for k in 0..3 {
            lo[k] = min_f64(lo[k], p[k]);
            hi[k] = max_f64(hi[k], p[k]);
        }
    }
    (lo, hi)
}

fn near(a: f64, b: f64, tol: f64, what: &str) {
    assert!((a - b).abs() <= tol, "{what}: {a} vs {b} (tolerance {tol})");
}

/// `true` when a front-facing triangle of `m` covers face point `(u, v)`
/// seen along the face normal (section D).
fn covered(m: &MeshData, f: &Face, u: f64, v: f64) -> bool {
    let out = f.out();
    let ps: Vec<[f64; 3]> = world(m).into_iter().map(mm).collect();
    m.indices.chunks_exact(3).any(|t| {
        let [a, b, c] = [ps[t[0] as usize], ps[t[1] as usize], ps[t[2] as usize]];
        let e1 = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let e2 = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [
            e1[1] * e2[2] - e1[2] * e2[1],
            e1[2] * e2[0] - e1[0] * e2[2],
            e1[0] * e2[1] - e1[1] * e2[0],
        ];
        let len = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt();
        if len < 1e-9 || (n[0] * out[0] + n[1] * out[1] + n[2] * out[2]) / len < 0.5 {
            return false;
        }
        let q = [a, b, c].map(|p| {
            let w = f.uvw(p);
            [w[0], w[1]]
        });
        let cross = |o: [f64; 2], p: [f64; 2], r: [f64; 2]| {
            (p[0] - o[0]) * (r[1] - o[1]) - (p[1] - o[1]) * (r[0] - o[0])
        };
        let x = [u, v];
        let d = [
            cross(q[0], q[1], x),
            cross(q[1], q[2], x),
            cross(q[2], q[0], x),
        ];
        d.iter().all(|&s| s >= 0.0) || d.iter().all(|&s| s <= 0.0)
    })
}

/// Asserts the hole of plate `node` is exactly `u` in `±half`, `v` in
/// `lo..hi`: 0.5 mm inside each edge is uncovered and 0.5 mm outside is
/// covered, at several points along every edge.
fn assert_hole(scene: &GltfScene, f: &Face, node: &str, half: f64, lo: f64, hi: f64) {
    let m = mesh(scene, node);
    let e = 0.5;
    for t in [0.1, 0.5, 0.9] {
        let u = -half + 2.0 * half * t;
        let v = lo + (hi - lo) * t;
        for (pu, pv, inside, edge) in [
            (u, lo + e, true, "bottom"),
            (u, lo - e, false, "bottom"),
            (u, hi - e, true, "top"),
            (u, hi + e, false, "top"),
            (-half + e, v, true, "left"),
            (-half - e, v, false, "left"),
            (half - e, v, true, "right"),
            (half + e, v, false, "right"),
        ] {
            assert_eq!(
                covered(m, f, pu, pv),
                !inside,
                "{node} {edge} edge at ({pu}, {pv}): {}",
                if inside {
                    "inside covered"
                } else {
                    "outside open"
                }
            );
        }
    }
}

/// The 37 nodes and 24 materials, in order, for every preset.
fn assert_lists(scene: &GltfScene) {
    let names: Vec<&str> = scene.meshes.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, NODE_NAMES.to_vec());
    let mats: Vec<&str> = scene.materials.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(mats, MATERIAL_NAMES.to_vec());
}

/// REQ-CAB-001: 37 nodes and 24 materials in the contract order for every
/// preset; the nine backbox and blade nodes follow the 28 #110 nodes, are
/// non-empty, finite, UV-mapped in [0, 1], static (identity transform, no
/// extras) and use the section F materials; the file loads through
/// `pin_assets::load_gltf`.
#[test]
fn req_cab_001_backbox_nodes_and_materials() {
    assert_eq!(NODE_NAMES.len(), 37);
    assert_eq!(MATERIAL_NAMES.len(), 24);
    for (i, (name, _)) in NEW_NODES.iter().enumerate() {
        assert_eq!(NODE_NAMES[28 + i], *name);
    }
    assert_eq!(
        MATERIAL_NAMES[17..].to_vec(),
        [
            "cabinet.backbox",
            "cabinet.backbox.speaker_panel",
            "cabinet.backbox.grille",
            "cabinet.backbox.translite",
            "cabinet.backbox.display",
            "cabinet.blade_left",
            "cabinet.blade_right",
        ]
    );
    for id in PRESETS {
        let out = generate(&preset(id));
        let scene = load(&out.glb);
        assert_lists(&scene);
        let js = json(&out.glb);
        for (name, material) in NEW_NODES {
            let m = mesh(&scene, name);
            assert!(
                !m.indices.is_empty() && m.indices.len().is_multiple_of(3),
                "{name}"
            );
            assert_eq!(m.uvs.len(), m.positions.len(), "{name}");
            assert!(m.positions.iter().flatten().all(|v| v.is_finite()));
            assert!(m.normals.iter().flatten().all(|v| v.is_finite()));
            assert!(m.uvs.iter().flatten().all(|v| (0.0..=1.0).contains(v)));
            assert_eq!(
                scene.materials[m.material.unwrap()].name,
                material,
                "{id} {name}"
            );
            let nj = node_json(&js, name);
            assert!(
                !nj.contains("translation") && !nj.contains("extras"),
                "{nj}"
            );
        }
    }
}

/// REQ-CAB-003 and acceptance: for every preset the bounds set spans
/// `x` in `±W/2`, `y` in `[front_y, front_y + depth]` and `z` from
/// `floor_height_mm(front_y, bottom_z)` to `backbox_top_height_mm()`,
/// within 1 mm.
#[test]
fn req_cab_003_backbox_size_matches_the_preset() {
    for id in PRESETS {
        let g = preset(id);
        let bb = &g.backbox;
        let scene = load(&generate(&g).glb);
        let ps: Vec<[f64; 3]> = BOUNDS_SET.iter().flat_map(|n| verts(&scene, n)).collect();
        let (lo, hi) = extent(&ps);
        let f = Face::of(&g);
        let hw = f64::from(bb.width) / 2.0;
        near(lo[0], -hw, 1.0, "x min");
        near(hi[0], hw, 1.0, "x max");
        near(lo[1], f64::from(bb.front_y), 1.0, "y min");
        near(hi[1], f64::from(bb.front_y + bb.depth), 1.0, "y max");
        near(lo[2], f.zb, 1.0, "z min");
        near(hi[2], g.backbox_top_height_mm(), 1.0, "z max");
        near(hi[2] - lo[2], f64::from(bb.height), 1.0, "height");
    }
    // The preset's published top heights (Appendix E).
    near(
        preset("classic_standard").backbox_top_height_mm(),
        1930.0,
        1e-3,
        "classic top",
    );
    near(
        preset("modern_standard").backbox_top_height_mm(),
        1911.0,
        1e-3,
        "modern top",
    );
}

/// Acceptance (opening positions): the translite frame's through hole is
/// exactly the translite opening and the speaker panel's is exactly the
/// score-display window, for every preset (0.5 mm in, 0.5 mm out).
#[test]
fn req_cab_003_openings_match_the_preset() {
    for id in PRESETS {
        let g = preset(id);
        let bb = &g.backbox;
        let f = Face::of(&g);
        let scene = load(&generate(&g).glb);
        let o = &bb.translite_opening;
        let ob = f64::from(o.bottom);
        assert_hole(
            &scene,
            &f,
            "cabinet.backbox.translite_frame",
            f64::from(o.width) / 2.0,
            ob,
            ob + f64::from(o.height),
        );
        let sd = &bb.score_display;
        let (cv, dh) = (f64::from(sd.centre_v), f64::from(sd.height) / 2.0);
        assert_hole(
            &scene,
            &f,
            "cabinet.backbox.speaker_panel",
            f64::from(sd.width) / 2.0,
            cv - dh,
            cv + dh,
        );
        // Each plate covers the face outside its hole: the speaker panel
        // from v = 0 to speaker_panel_height, the frame above it.
        let sph = f64::from(bb.speaker_panel_height);
        let sp = mesh(&scene, "cabinet.backbox.speaker_panel");
        let fr = mesh(&scene, "cabinet.backbox.translite_frame");
        let hw = f64::from(bb.width) / 2.0;
        assert!(covered(sp, &f, -hw + 1.0, 1.0) && covered(sp, &f, hw - 1.0, sph - 1.0));
        assert!(!covered(sp, &f, 0.0, sph + 1.0));
        assert!(covered(fr, &f, hw - 1.0, sph + 1.0) && !covered(fr, &f, 0.0, sph - 1.0));
        assert!(covered(fr, &f, 0.0, f64::from(bb.height) - 1.0));
    }
}

/// The four corners `[upper-left, upper-right, lower-left, lower-right]`
/// (seen from the front) of the face-parallel vertices at depth `w`, with
/// their UVs.
fn corners(m: &MeshData, f: &Face, w: f64) -> Vec<([f64; 3], [f32; 2])> {
    let ps: Vec<[f64; 3]> = world(m).into_iter().map(mm).collect();
    let pts: Vec<([f64; 3], [f32; 2])> = ps
        .iter()
        .zip(&m.uvs)
        .map(|(p, uv)| (f.uvw(*p), *uv))
        .filter(|(q, _)| (q[2] - w).abs() < 0.05)
        .collect();
    let (lo, hi) = extent(&pts.iter().map(|p| p.0).collect::<Vec<_>>());
    let pick = |u: f64, v: f64| -> Vec<([f64; 3], [f32; 2])> {
        pts.iter()
            .filter(|(q, _)| (q[0] - u).abs() < 0.05 && (q[1] - v).abs() < 0.05)
            .copied()
            .collect()
    };
    [
        (lo[0], hi[1]),
        (hi[0], hi[1]),
        (lo[0], lo[1]),
        (hi[0], lo[1]),
    ]
    .into_iter()
    .flat_map(|(u, v)| pick(u, v))
    .collect()
}

/// Asserts every vertex at a corner of the quad at depth `w` carries that
/// corner's UV: upper-left (0, 0), upper-right (1, 0), lower-left (0, 1),
/// lower-right (1, 1).
fn assert_corner_uvs(m: &MeshData, f: &Face, w: f64, what: &str) {
    let cs = corners(m, f, w);
    assert!(cs.len() >= 4, "{what}: {} corner vertices", cs.len());
    let (lo, hi) = extent(&cs.iter().map(|c| c.0).collect::<Vec<_>>());
    for (q, uv) in cs {
        let want = [
            if (q[0] - lo[0]).abs() < 0.05 {
                0.0
            } else {
                1.0
            },
            if (q[1] - hi[1]).abs() < 0.05 {
                0.0
            } else {
                1.0
            },
        ];
        assert!(
            (uv[0] - want[0]).abs() < 1e-5 && (uv[1] - want[1]).abs() < 1e-5,
            "{what}: corner {q:?} has UV {uv:?}, want {want:?}"
        );
    }
}

/// Acceptance (display aspect) and REQ-CAB-003: the display is one quad
/// (4 vertices, 2 triangles) facing the player, `score_display.width` by
/// `height` in the face plane, centred at `(0, centre_v)`, 5 mm behind the
/// face, its aspect within 2 % of `pixels`, its UVs the whole texture.
#[test]
fn req_cab_003_display_quad_size_aspect_and_uvs() {
    for id in PRESETS {
        let g = preset(id);
        let sd = &g.backbox.score_display;
        let f = Face::of(&g);
        let scene = load(&generate(&g).glb);
        let m = mesh(&scene, "cabinet.backbox.display");
        assert_eq!((m.positions.len(), m.indices.len()), (4, 6), "{id}");
        let q: Vec<[f64; 3]> = verts(&scene, "cabinet.backbox.display")
            .into_iter()
            .map(|p| f.uvw(p))
            .collect();
        let (lo, hi) = extent(&q);
        let (w, h) = (hi[0] - lo[0], hi[1] - lo[1]);
        near(w, f64::from(sd.width), 1.0, "display width");
        near(h, f64::from(sd.height), 1.0, "display height");
        near((lo[0] + hi[0]) / 2.0, 0.0, 1.0, "display centre u");
        near(
            (lo[1] + hi[1]) / 2.0,
            f64::from(sd.centre_v),
            1.0,
            "centre v",
        );
        near(lo[2], 5.0, 0.01, "recess");
        near(hi[2], 5.0, 0.01, "recess");
        let px = f64::from(sd.pixels[0]) / f64::from(sd.pixels[1]);
        assert!(
            ((w / h) / px - 1.0).abs() <= 0.02,
            "{id}: {} vs {px}",
            w / h
        );
        let out = f.out();
        for n in &m.normals {
            let d = f64::from(n[0]) * out[0] + f64::from(n[1]) * out[1] + f64::from(n[2]) * out[2];
            assert!(d > 0.999, "{id}: display faces the player");
        }
        assert_corner_uvs(m, &f, 5.0, id);
    }
    // The presets' modes: a 128x32 DMD and a 1920x1080 LCD.
    assert_eq!(
        preset("classic_standard").backbox.score_display.kind,
        ScoreDisplayKind::Dmd
    );
    assert_eq!(
        preset("modern_standard").backbox.score_display.pixels,
        [1920, 1080]
    );
}

/// REQ-CAB-003: the backglass sheet is `backglass.width x height x
/// thickness`, centred on the translite opening, its front on the frame's
/// rear face (`w = T`), covering the opening; its front face is UV-mapped
/// upper-left (0, 0) to lower-right (1, 1).
#[test]
fn req_cab_003_translite_sheet_covers_the_opening() {
    for id in PRESETS {
        let g = preset(id);
        let bb = &g.backbox;
        let f = Face::of(&g);
        let t = f64::from(g.body.wall_thickness);
        let scene = load(&generate(&g).glb);
        let q: Vec<[f64; 3]> = verts(&scene, "cabinet.backbox.translite")
            .into_iter()
            .map(|p| f.uvw(p))
            .collect();
        let (lo, hi) = extent(&q);
        let bg = &bb.backglass;
        let o = &bb.translite_opening;
        let vc = f64::from(o.bottom) + f64::from(o.height) / 2.0;
        near(hi[0] - lo[0], f64::from(bg.width), 1.0, "backglass width");
        near(hi[1] - lo[1], f64::from(bg.height), 1.0, "backglass height");
        near(hi[2] - lo[2], f64::from(bg.thickness), 0.01, "thickness");
        near((lo[0] + hi[0]) / 2.0, 0.0, 1.0, "centre u");
        near((lo[1] + hi[1]) / 2.0, vc, 1.0, "centre v");
        near(lo[2], t, 0.01, "front on the frame's rear face");
        assert!(lo[0] <= -f64::from(o.width) / 2.0 && hi[0] >= f64::from(o.width) / 2.0);
        assert!(lo[1] <= f64::from(o.bottom) && hi[1] >= f64::from(o.bottom + o.height));
        assert_corner_uvs(mesh(&scene, "cabinet.backbox.translite"), &f, t, id);
    }
}

/// The vertices of disc `i` of the grilles node: those nearest grille `i`.
fn grille_vertices(g: &CabinetGeometry, scene: &GltfScene) -> Vec<Vec<[f64; 3]>> {
    let f = Face::of(g);
    let grs = &g.backbox.grilles;
    let mut out = vec![Vec::new(); grs.len()];
    for p in verts(scene, "cabinet.backbox.grilles") {
        let q = f.uvw(p);
        let d = |i: usize| {
            let (du, dv) = (q[0] - f64::from(grs[i].u), q[1] - f64::from(grs[i].v));
            du * du + dv * dv
        };
        let best = (0..grs.len())
            .min_by(|&a, &b| d(a).total_cmp(&d(b)))
            .unwrap();
        out[best].push(q);
    }
    out
}

/// Asserts each grille disc's centre and diameter (within 1 mm), at least
/// 16 segments, and that it stands 2 mm proud of the face.
fn assert_grilles(g: &CabinetGeometry, scene: &GltfScene) {
    for (i, (gr, qs)) in g
        .backbox
        .grilles
        .iter()
        .zip(grille_vertices(g, scene))
        .enumerate()
    {
        let (lo, hi) = extent(&qs);
        near(
            hi[0] - lo[0],
            f64::from(gr.diameter),
            1.0,
            "grille diameter u",
        );
        near(
            hi[1] - lo[1],
            f64::from(gr.diameter),
            1.0,
            "grille diameter v",
        );
        near((lo[0] + hi[0]) / 2.0, f64::from(gr.u), 1.0, "grille u");
        near((lo[1] + hi[1]) / 2.0, f64::from(gr.v), 1.0, "grille v");
        near(lo[2], -2.0, 0.01, "grille proud");
        near(hi[2], 0.0, 0.01, "grille on the face");
        // Distinct rim points on the front cap.
        let mut rim: Vec<(i64, i64)> = qs
            .iter()
            .filter(|q| (q[2] + 2.0).abs() < 0.01)
            .filter(|q| {
                let (du, dv) = (q[0] - f64::from(gr.u), q[1] - f64::from(gr.v));
                ((du * du + dv * dv).sqrt() - f64::from(gr.diameter) / 2.0).abs() < 0.5
            })
            .map(|q| ((q[0] * 100.0).round() as i64, (q[1] * 100.0).round() as i64))
            .collect();
        rim.sort_unstable();
        rim.dedup();
        assert!(rim.len() >= 16, "grille {i}: {} rim points", rim.len());
    }
}

/// REQ-CAB-003: one disc per grille at its `(u, v)` and diameter, 2 mm
/// proud, at least 16 segments, for every preset.
#[test]
fn req_cab_003_grille_discs() {
    for id in PRESETS {
        let g = preset(id);
        assert_eq!(g.backbox.grilles.len(), 2);
        let scene = load(&generate(&g).glb);
        assert_grilles(&g, &scene);
    }
}

/// REQ-CAB-001 (hinge): two mirrored bracket plates at least 40 mm wide,
/// inside `±W/2` and `[front_y, front_y + depth]`, between the lower of the
/// backbox bottom and the rear wall top and 100 mm above the backbox
/// bottom; they reach both.
#[test]
fn req_cab_001_hinge_bounds() {
    for id in PRESETS {
        let g = preset(id);
        let bb = &g.backbox;
        let f = Face::of(&g);
        let scene = load(&generate(&g).glb);
        let ps = verts(&scene, "cabinet.backbox.hinge");
        let (lo, hi) = extent(&ps);
        let rwt = g.floor_height_mm(f64::from(g.body.length), f64::from(g.body.rear_height));
        let hw = f64::from(bb.width) / 2.0;
        assert!(lo[0] >= -hw - 1e-3 && hi[0] <= hw + 1e-3);
        assert!(lo[1] >= f64::from(bb.front_y) - 1e-3);
        assert!(hi[1] <= f64::from(bb.front_y + bb.depth) + 1e-3);
        near(lo[2], min_f64(f.zb, rwt), 0.01, "hinge bottom");
        assert!(
            hi[2] <= f.zb + 100.0 + 1e-3 && hi[2] > f.zb,
            "hinge top {}",
            hi[2]
        );
        let right: Vec<[f64; 3]> = ps.iter().filter(|p| p[0] > 0.0).copied().collect();
        let (rlo, rhi) = extent(&right);
        assert!(
            rhi[0] - rlo[0] >= 40.0,
            "{id}: plate width {}",
            rhi[0] - rlo[0]
        );
        for p in &ps {
            assert!(
                ps.iter().any(|q| (q[0] + p[0]).abs() < 1e-3
                    && (q[1] - p[1]).abs() < 1e-3
                    && (q[2] - p[2]).abs() < 1e-3),
                "{id}: hinge not mirrored at {p:?}"
            );
        }
    }
}

/// The blade edges of the contract (section E) at `y`, mm above the floor.
fn blade_edges(g: &CabinetGeometry, y: f64) -> (f64, f64) {
    let b = &g.body;
    let m = &g.mount;
    let (fh, rh) = (f64::from(b.front_height), f64::from(b.rear_height));
    let w_top = fh + (rh - fh) * y / f64::from(b.length);
    let pitch = libm::tan(f64::from(m.pitch_deg).to_radians());
    (
        g.floor_height_mm(y, f64::from(m.height) + (y - f64::from(m.inset)) * pitch),
        g.floor_height_mm(y, w_top - 2.0 * f64::from(b.glass_channel.width)),
    )
}

/// REQ-CAB-001 / REQ-CAB-002 surfaces: each blade is 3 mm thick against its
/// side wall's inner face, from `mount.inset` to the rear wall's inner
/// face, its bottom edge on the playfield plane and its top under the glass
/// channel notch; the two are mirror images; the inner face's UVs cover
/// 0..1.
#[test]
fn req_cab_001_blades_line_the_inner_walls() {
    let mut geoms: Vec<CabinetGeometry> = PRESETS.iter().map(|id| preset(id)).collect();
    geoms.push(from_toml(
        "[mount]\ninset = 60.0\nheight = 250.0\npitch_deg = 7.5\n",
    ));
    for g in &geoms {
        let scene = load(&generate(g).glb);
        let b = &g.body;
        let xi = f64::from(b.width) / 2.0 - f64::from(b.wall_thickness);
        let (y0, y1) = (
            f64::from(g.mount.inset),
            f64::from(b.length - b.wall_thickness),
        );
        let left = verts(&scene, "cabinet.blade.left");
        let right = verts(&scene, "cabinet.blade.right");
        let (lo, hi) = extent(&left);
        near(lo[0], -xi, 0.01, "left blade outer x");
        near(hi[0], -xi + 3.0, 0.01, "left blade inner x");
        near(lo[1], y0, 0.01, "blade front");
        near(hi[1], y1, 0.01, "blade rear");
        for p in &left {
            let (zb, zt) = blade_edges(g, p[1]);
            assert!(
                (p[2] - zb).abs() < 0.05 || (p[2] - zt).abs() < 0.05,
                "blade vertex {p:?} off its edges {zb} / {zt}"
            );
        }
        // Both edges present at both ends.
        for y in [y0, y1] {
            let (zb, zt) = blade_edges(g, y);
            assert!(zt - zb > 1.0);
            for z in [zb, zt] {
                assert!(
                    left.iter()
                        .any(|p| (p[1] - y).abs() < 0.05 && (p[2] - z).abs() < 0.05)
                );
            }
        }
        // Mirror images.
        assert_eq!(left.len(), right.len());
        for p in &left {
            assert!(right.iter().any(|q| (q[0] + p[0]).abs() < 1e-3
                && (q[1] - p[1]).abs() < 1e-3
                && (q[2] - p[2]).abs() < 1e-3));
        }
        // Inner faces: +x on the left, -x on the right; UVs cover 0..1.
        for (name, sign) in [
            ("cabinet.blade.left", 1.0f32),
            ("cabinet.blade.right", -1.0),
        ] {
            let m = mesh(&scene, name);
            let mut ulo = [f32::INFINITY; 2];
            let mut uhi = [f32::NEG_INFINITY; 2];
            for (n, uv) in m.normals.iter().zip(&m.uvs) {
                if (n[0] - sign).abs() < 1e-6 {
                    for k in 0..2 {
                        ulo[k] = min_f32(ulo[k], uv[k]);
                        uhi[k] = max_f32(uhi[k], uv[k]);
                    }
                }
            }
            assert_eq!((ulo, uhi), ([0.0, 0.0], [1.0, 1.0]), "{name}");
        }
    }
}

/// Bytes of every node's mesh data (positions, normals, UVs, indices) by
/// name, for comparing two models node by node.
fn node_data(scene: &GltfScene) -> Vec<(String, Vec<u32>)> {
    scene
        .meshes
        .iter()
        .map(|m| {
            let mut v: Vec<u32> = m.positions.iter().flatten().map(|f| f.to_bits()).collect();
            v.extend(m.normals.iter().flatten().map(|f| f.to_bits()));
            v.extend(m.uvs.iter().flatten().map(|f| f.to_bits()));
            v.extend(&m.indices);
            (m.name.clone(), v)
        })
        .collect()
}

/// The node names whose data differ between two scenes.
fn changed(a: &GltfScene, b: &GltfScene) -> Vec<String> {
    node_data(a)
        .into_iter()
        .zip(node_data(b))
        .filter(|(x, y)| x != y)
        .map(|(x, _)| x.0)
        .collect()
}

/// REQ-CAB-003 (data-driven): a `cabinet.toml` override of each `[backbox]`
/// key moves the corresponding geometry and only backbox nodes: the first
/// 28 nodes and the blades never change (not even for `front_y` or
/// `bottom_z`).
#[test]
fn req_cab_003_every_backbox_key_is_data() {
    let base_g = preset("classic_standard");
    let base = load(&generate(&base_g).glb);
    let cases: &[(&str, &[&str])] = &[
        (
            "[backbox]\nwidth = 700.0\n",
            // The hinge sits inside the narrower of the backbox and the
            // body (560 mm), so a 700 mm backbox leaves it in place.
            &["shell", "speaker_panel", "translite_frame"],
        ),
        ("[backbox]\nheight = 760.0\n", &["shell", "translite_frame"]),
        ("[backbox]\ndepth = 270.0\n", &["shell", "hinge"]),
        ("[backbox]\nfront_y = 1050.0\n", &["*"]),
        ("[backbox]\nbottom_z = 690.0\n", &["*"]),
        (
            "[backbox]\nspeaker_panel_height = 210.0\n",
            &["speaker_panel", "translite_frame"],
        ),
        ("[backbox.backglass]\nwidth = 676.0\n", &["translite"]),
        ("[backbox.backglass]\nheight = 470.0\n", &["translite"]),
        ("[backbox.backglass]\nthickness = 4.0\n", &["translite"]),
        (
            "[backbox.translite_opening]\nwidth = 640.0\n",
            &["translite_frame"],
        ),
        (
            "[backbox.translite_opening]\nheight = 440.0\n",
            &["translite_frame", "translite"],
        ),
        (
            "[backbox.translite_opening]\nbottom = 225.0\n",
            &["translite_frame", "translite"],
        ),
        (
            "[backbox.score_display]\nwidth = 300.0\n",
            &["speaker_panel", "display"],
        ),
        (
            "[backbox.score_display]\nheight = 75.0\n",
            &["speaker_panel", "display"],
        ),
        (
            "[backbox.score_display]\ncentre_v = 105.0\n",
            &["speaker_panel", "display"],
        ),
        (
            "[backbox]\ngrilles = [{ u = -250.0, v = 90.0, diameter = 120.0 }]\n",
            &["grilles"],
        ),
    ];
    for (text, nodes) in cases {
        let g = from_toml(text);
        let scene = load(&generate(&g).glb);
        assert_lists(&scene);
        let diff = changed(&base, &scene);
        assert!(!diff.is_empty(), "{text}: nothing moved");
        for d in &diff {
            assert!(d.starts_with("cabinet.backbox."), "{text}: {d} changed");
            if nodes != &["*"] {
                let short = d.trim_start_matches("cabinet.backbox.");
                assert!(nodes.contains(&short), "{text}: {d} changed unexpectedly");
            }
        }
        for n in *nodes {
            if *n != "*" {
                let full = format!("cabinet.backbox.{n}");
                assert!(diff.contains(&full), "{text}: {full} did not move");
            }
        }
        // The moved geometry follows the data.
        let f = Face::of(&g);
        let bb = &g.backbox;
        let ps: Vec<[f64; 3]> = BOUNDS_SET.iter().flat_map(|n| verts(&scene, n)).collect();
        let (lo, hi) = extent(&ps);
        near(hi[0] - lo[0], f64::from(bb.width), 1.0, text);
        near(hi[1] - lo[1], f64::from(bb.depth), 1.0, text);
        near(lo[1], f64::from(bb.front_y), 1.0, text);
        near(lo[2], f.zb, 1.0, text);
        near(hi[2], g.backbox_top_height_mm(), 1.0, text);
        let o = &bb.translite_opening;
        assert_hole(
            &scene,
            &f,
            "cabinet.backbox.translite_frame",
            f64::from(o.width) / 2.0,
            f64::from(o.bottom),
            f64::from(o.bottom + o.height),
        );
        let sd = &bb.score_display;
        assert_hole(
            &scene,
            &f,
            "cabinet.backbox.speaker_panel",
            f64::from(sd.width) / 2.0,
            f64::from(sd.centre_v - sd.height / 2.0),
            f64::from(sd.centre_v + sd.height / 2.0),
        );
        assert_grilles(&g, &scene);
        let sp = verts(&scene, "cabinet.backbox.speaker_panel");
        let top = sp
            .iter()
            .map(|p| f.uvw(*p)[1])
            .fold(f64::NEG_INFINITY, max_f64);
        near(
            top,
            f64::from(bb.speaker_panel_height),
            0.01,
            "speaker panel top",
        );
        let tq: Vec<[f64; 3]> = verts(&scene, "cabinet.backbox.translite")
            .into_iter()
            .map(|p| f.uvw(p))
            .collect();
        let (tlo, thi) = extent(&tq);
        near(thi[0] - tlo[0], f64::from(bb.backglass.width), 0.01, text);
        near(thi[1] - tlo[1], f64::from(bb.backglass.height), 0.01, text);
        near(
            thi[2] - tlo[2],
            f64::from(bb.backglass.thickness),
            0.01,
            text,
        );
    }
}

/// REQ-CAB-003: an LCD mode (`kind`, `pixels`, window size) and 1, 2 and 8
/// grilles from `cabinet.toml`.
#[test]
fn req_cab_003_lcd_mode_and_grille_counts() {
    let g = from_toml(
        "[backbox.score_display]\nkind = \"lcd\"\npixels = [1920, 1080]\nwidth = 320.0\nheight = 180.0\ncentre_v = 100.0\n\
         [backbox]\ngrilles = []\n",
    );
    assert_eq!(g.backbox.score_display.kind, ScoreDisplayKind::Lcd);
    let scene = load(&generate(&g).glb);
    let f = Face::of(&g);
    let q: Vec<[f64; 3]> = verts(&scene, "cabinet.backbox.display")
        .into_iter()
        .map(|p| f.uvw(p))
        .collect();
    let (lo, hi) = extent(&q);
    let aspect = (hi[0] - lo[0]) / (hi[1] - lo[1]);
    assert!((aspect / (1920.0 / 1080.0) - 1.0).abs() <= 0.02, "{aspect}");
    for n in [1usize, 2, 8] {
        let list: Vec<String> = (0..n)
            .map(|i| {
                let u = -300.0 + 600.0 * (i as f64 + 0.5) / n as f64;
                format!("{{ u = {u:?}, v = 100.0, diameter = 30.0 }}")
            })
            .collect();
        let g = from_toml(&format!("[backbox]\ngrilles = [{}]\n", list.join(", ")));
        assert_eq!(g.backbox.grilles.len(), n);
        let scene = load(&generate(&g).glb);
        assert_lists(&scene);
        assert_grilles(&g, &scene);
        let per = verts(&scene, "cabinet.backbox.grilles").len() / n;
        assert_eq!(
            per * n,
            mesh(&scene, "cabinet.backbox.grilles").positions.len()
        );
    }
}

/// G (zero grilles): the node stays in the stable list, written with no
/// mesh; the file loads and lists every other node.
#[test]
fn zero_grilles_keep_the_node_name() {
    let g = from_toml("[backbox]\ngrilles = []\n");
    let out = generate(&g);
    assert_eq!(out.nodes, NODE_NAMES.map(String::from).to_vec());
    let js = json(&out.glb);
    assert!(
        js.contains("{\"name\":\"cabinet.backbox.grilles\"}"),
        "mesh-less node"
    );
    let scene = load(&out.glb);
    let names: Vec<&str> = scene.meshes.iter().map(|m| m.name.as_str()).collect();
    let want: Vec<&str> = NODE_NAMES
        .iter()
        .copied()
        .filter(|n| *n != "cabinet.backbox.grilles")
        .collect();
    assert_eq!(names, want);
}

/// Profiles (sections A, B): an angled front, an angled top and an arched
/// top on a preset generate; the apex is `backbox_top_height_mm()` within
/// 1 mm; a non-flat top's lowest top point is below the apex; the display
/// keeps its in-plane size and aspect and faces the player.
#[test]
fn profiles_keep_the_apex_and_display_aspect() {
    for (front, top) in [
        ("angled", "flat"),
        ("flat", "angled"),
        ("flat", "arched"),
        ("angled", "angled"),
        ("angled", "arched"),
    ] {
        let text = format!("[backbox]\nfront_profile = \"{front}\"\ntop_profile = \"{top}\"\n");
        let g = from_toml(&text);
        let bb = &g.backbox;
        let scene = load(&generate(&g).glb);
        let f = Face::of(&g);
        let ps: Vec<[f64; 3]> = BOUNDS_SET.iter().flat_map(|n| verts(&scene, n)).collect();
        let (lo, hi) = extent(&ps);
        near(hi[2], g.backbox_top_height_mm(), 1.0, &text);
        near(lo[2], f.zb, 1.0, &text);
        near(hi[0] - lo[0], f64::from(bb.width), 1.0, &text);
        near(lo[1], f64::from(bb.front_y), 1.0, &text);
        near(hi[1], f64::from(bb.front_y + bb.depth), 1.0, &text);
        // The shell's top panel's outer surface: its highest point at each
        // (x, y) corner of the box.
        let shell = verts(&scene, "cabinet.backbox.shell");
        let top_at = |x: f64, y: f64| {
            shell
                .iter()
                .filter(|p| (p[0] - x).abs() < 0.5 && (p[1] - y).abs() < 0.5)
                .map(|p| p[2])
                .fold(f64::NEG_INFINITY, max_f64)
        };
        let rear = f64::from(bb.front_y + bb.depth);
        let hw = f64::from(bb.width) / 2.0;
        let lowest = min_f64(top_at(hw, rear), top_at(-hw, rear));
        if top == "flat" {
            near(lowest, f.zb + f64::from(bb.height), 1.0, "flat top");
        } else {
            assert!(
                lowest < hi[2] - 5.0,
                "{text}: lowest top {lowest} vs apex {}",
                hi[2]
            );
        }
        // Holes stay exact on the leaning face too.
        let o = &bb.translite_opening;
        assert_hole(
            &scene,
            &f,
            "cabinet.backbox.translite_frame",
            f64::from(o.width) / 2.0,
            f64::from(o.bottom),
            f64::from(o.bottom + o.height),
        );
        let sd = &bb.score_display;
        let q: Vec<[f64; 3]> = verts(&scene, "cabinet.backbox.display")
            .into_iter()
            .map(|p| f.uvw(p))
            .collect();
        let (dlo, dhi) = extent(&q);
        near(dhi[0] - dlo[0], f64::from(sd.width), 0.01, "display width");
        near(
            dhi[1] - dlo[1],
            f64::from(sd.height),
            0.01,
            "display height",
        );
        let px = f64::from(sd.pixels[0]) / f64::from(sd.pixels[1]);
        let aspect = (dhi[0] - dlo[0]) / (dhi[1] - dlo[1]);
        assert!((aspect / px - 1.0).abs() <= 0.02);
        assert_corner_uvs(mesh(&scene, "cabinet.backbox.display"), &f, 5.0, &text);
        let t = f64::from(g.body.wall_thickness);
        assert_corner_uvs(mesh(&scene, "cabinet.backbox.translite"), &f, t, &text);
        assert_grilles(&g, &scene);
    }
}

/// Profiles: an angled front that leans past the depth and an arched top
/// that dips below the opening are errors naming the profile key.
#[test]
fn profile_errors_name_their_key() {
    let mut g = from_toml("[backbox]\nfront_profile = \"angled\"\n");
    g.backbox.depth = 150.0; // 730 tan 10 = 128.7 > 150 - 2 * 19
    assert_eq!(
        generate_cabinet(&g).unwrap_err().element,
        "backbox.front_profile"
    );
    let mut g = from_toml("[backbox]\ntop_profile = \"arched\"\n");
    g.backbox.translite_opening.height = 500.0; // top 715 above the 694.6 mm crown
    g.backbox.backglass.height = 510.0;
    assert_eq!(
        generate_cabinet(&g).unwrap_err().element,
        "backbox.top_profile"
    );
    let mut g = preset("modern_standard");
    g.backbox.top_profile = BackboxTopProfile::Arched; // crown 677.4 < 692
    assert_eq!(
        generate_cabinet(&g).unwrap_err().element,
        "backbox.top_profile"
    );
    let mut g = from_toml("[backbox]\ntop_profile = \"angled\"\n");
    g.backbox.depth = 4000.0; // slopes below the bottom long before the rear
    assert_eq!(
        generate_cabinet(&g).unwrap_err().element,
        "backbox.top_profile"
    );
}

type Mutate = fn(&mut CabinetGeometry, f32);
type Fit = fn(&mut CabinetGeometry);

/// Section G: one field mutated at a time names exactly its key, for NaN,
/// infinity, zero, a negative value and one beyond `MAX_DIMENSION_MM`
/// (zero is allowed for `front_y` and `bottom_z`); never a panic.
#[test]
fn every_backbox_value_error_names_its_key() {
    let sizes: &[(&str, Mutate)] = &[
        ("backbox.width", |g, v| g.backbox.width = v),
        ("backbox.height", |g, v| g.backbox.height = v),
        ("backbox.depth", |g, v| g.backbox.depth = v),
        ("backbox.speaker_panel_height", |g, v| {
            g.backbox.speaker_panel_height = v
        }),
        ("backbox.backglass.width", |g, v| {
            g.backbox.backglass.width = v
        }),
        ("backbox.backglass.height", |g, v| {
            g.backbox.backglass.height = v
        }),
        ("backbox.backglass.thickness", |g, v| {
            g.backbox.backglass.thickness = v
        }),
        ("backbox.translite_opening.width", |g, v| {
            g.backbox.translite_opening.width = v
        }),
        ("backbox.translite_opening.height", |g, v| {
            g.backbox.translite_opening.height = v
        }),
        ("backbox.translite_opening.bottom", |g, v| {
            g.backbox.translite_opening.bottom = v
        }),
        ("backbox.score_display.width", |g, v| {
            g.backbox.score_display.width = v
        }),
        ("backbox.score_display.height", |g, v| {
            g.backbox.score_display.height = v
        }),
        ("backbox.score_display.centre_v", |g, v| {
            g.backbox.score_display.centre_v = v
        }),
        ("backbox.grilles[1].v", |g, v| g.backbox.grilles[1].v = v),
        ("backbox.grilles[0].diameter", |g, v| {
            g.backbox.grilles[0].diameter = v
        }),
    ];
    let base = preset("classic_standard");
    let big = MAX_DIMENSION_MM * 2.0;
    for (key, set) in sizes {
        for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -1.0, big] {
            let mut g = base.clone();
            set(&mut g, v);
            let e = generate_cabinet(&g).unwrap_err();
            assert_eq!(&e.element, key, "{v}: {e}");
            assert!(!e.message.is_empty());
        }
    }
    let positions: &[(&str, Mutate)] = &[
        ("backbox.front_y", |g, v| g.backbox.front_y = v),
        ("backbox.bottom_z", |g, v| g.backbox.bottom_z = v),
    ];
    for (key, set) in positions {
        for v in [f32::NAN, f32::INFINITY, -1.0, big] {
            let mut g = base.clone();
            set(&mut g, v);
            assert_eq!(&generate_cabinet(&g).unwrap_err().element, key, "{v}");
        }
        let mut g = base.clone();
        set(&mut g, 0.0);
        assert!(generate_cabinet(&g).is_ok(), "{key} = 0");
    }
    for v in [f32::NAN, f32::INFINITY, -big] {
        let mut g = base.clone();
        g.backbox.grilles[0].u = v;
        assert_eq!(
            generate_cabinet(&g).unwrap_err().element,
            "backbox.grilles[0].u"
        );
    }
    for px in [[0, 32], [128, 0]] {
        let mut g = base.clone();
        g.backbox.score_display.pixels = px;
        assert_eq!(
            generate_cabinet(&g).unwrap_err().element,
            "backbox.score_display.pixels"
        );
    }
    let mut g = base.clone();
    let one = g.backbox.grilles[0].clone();
    g.backbox.grilles = vec![
        pin_core::schema::cabinet::Grille {
            diameter: 10.0,
            ..one
        };
        9
    ];
    assert_eq!(generate_cabinet(&g).unwrap_err().element, "backbox.grilles");
    g.backbox.grilles.truncate(8);
    assert!(generate_cabinet(&g).is_ok());
}

/// Section G.2: parts that do not fit the face name their key.
#[test]
fn every_backbox_fit_error_names_its_key() {
    let cases: &[(&str, Fit)] = &[
        ("backbox.translite_opening.width", |g| {
            g.backbox.translite_opening.width = 731.0
        }),
        ("backbox.translite_opening.bottom", |g| {
            g.backbox.translite_opening.bottom = 290.0
        }),
        ("backbox.score_display.width", |g| {
            g.backbox.score_display.width = 731.0
        }),
        ("backbox.score_display.centre_v", |g| {
            g.backbox.score_display.centre_v = 170.0
        }),
        ("backbox.score_display.centre_v", |g| {
            g.backbox.score_display.centre_v = 30.0
        }),
        ("backbox.grilles[1].u", |g| g.backbox.grilles[1].u = 300.0),
        ("backbox.grilles[0].u", |g| g.backbox.grilles[0].u = -300.0),
        ("backbox.grilles[0].v", |g| g.backbox.grilles[0].v = 70.0),
        ("backbox.grilles[1].v", |g| g.backbox.grilles[1].v = 700.0),
        ("backbox.backglass.width", |g| {
            g.backbox.backglass.width = 731.0
        }),
        ("backbox.backglass.height", |g| {
            g.backbox.backglass.height = 600.0
        }),
        ("backbox.speaker_panel_height", |g| {
            g.backbox.speaker_panel_height = 730.0
        }),
    ];
    for (key, set) in cases {
        let mut g = preset("classic_standard");
        set(&mut g);
        assert_eq!(&generate_cabinet(&g).unwrap_err().element, key);
    }
    // Exactly at the limits is fine.
    let mut g = preset("classic_standard");
    g.backbox.translite_opening.width = 730.0;
    g.backbox.backglass.width = 730.0;
    g.backbox.grilles[1].u = 365.0 - 75.0;
    assert!(generate_cabinet(&g).is_ok());
}

/// Section E: blade errors name `mount.inset`, `mount.height` and
/// `mount.pitch_deg`.
#[test]
fn blade_errors_name_the_mount_keys() {
    let cases: &[(&str, Mutate)] = &[
        ("mount.inset", |g, v| g.mount.inset = v),
        ("mount.height", |g, v| g.mount.height = v),
        ("mount.pitch_deg", |g, v| g.mount.pitch_deg = v),
    ];
    for (key, set) in cases {
        for v in [f32::NAN, f32::INFINITY] {
            let mut g = preset("modern_standard");
            set(&mut g, v);
            assert_eq!(&generate_cabinet(&g).unwrap_err().element, key, "{v}");
        }
    }
    for (key, set, v) in [
        ("mount.inset", (|g, v| g.mount.inset = v) as Mutate, -1.0),
        ("mount.inset", |g, v| g.mount.inset = v, 1296.0),
        ("mount.height", |g, v| g.mount.height = v, 0.0),
        ("mount.height", |g, v| g.mount.height = v, 395.0),
        ("mount.pitch_deg", |g, v| g.mount.pitch_deg = v, 89.0),
        ("mount.pitch_deg", |g, v| g.mount.pitch_deg = v, -90.0),
        ("mount.height", |g, v| g.mount.pitch_deg = v, 25.0),
    ] {
        let mut g = preset("classic_standard");
        set(&mut g, v);
        assert_eq!(generate_cabinet(&g).unwrap_err().element, key, "{v}");
    }
}

/// R3: the same geometry gives the same bytes, twice, for every preset and
/// every profile; different presets give different bytes.
#[test]
fn backbox_bytes_are_deterministic() {
    let mut seen = Vec::new();
    for id in PRESETS {
        for (front, top) in [("flat", "flat"), ("angled", "angled"), ("flat", "arched")] {
            let mut g = preset(id);
            g.backbox.front_profile = if front == "flat" {
                BackboxFrontProfile::Flat
            } else {
                BackboxFrontProfile::Angled
            };
            g.backbox.top_profile = match top {
                "flat" => BackboxTopProfile::Flat,
                "angled" => BackboxTopProfile::Angled,
                _ => BackboxTopProfile::Arched,
            };
            let Ok(a) = generate_cabinet(&g) else {
                assert_eq!((id, top), ("modern_standard", "arched"));
                continue;
            };
            let b = generate_cabinet(&g).unwrap();
            assert_eq!(a.glb, b.glb, "{id} {front} {top}");
            let h = fnv1a(&a.glb);
            assert!(!seen.contains(&h));
            seen.push(h);
        }
    }
}

/// Contract C bounds set on an angled front (regression, Independent
/// Tester finding): a score-display window whose bottom edge is closer to
/// the face bottom than `T * tan(10)` must not push the speaker panel's
/// hole wall below the backbox bottom; the bounds stay within 1 mm and the
/// hole stays exact.
#[test]
fn angled_front_window_near_the_bottom_stays_above_zb() {
    for centre_v in [9.5f32, 11.0, 12.0, 30.0] {
        let mut g = preset("classic_standard");
        let bb = &mut g.backbox;
        bb.width = 500.0;
        bb.height = 500.0;
        bb.depth = 150.0;
        bb.front_y = 915.0;
        bb.bottom_z = 605.0;
        bb.front_profile = BackboxFrontProfile::Angled;
        bb.speaker_panel_height = 100.0;
        bb.score_display.width = 57.6;
        bb.score_display.height = 18.0;
        bb.score_display.centre_v = centre_v;
        bb.translite_opening = pin_core::schema::cabinet::TransliteOpening {
            width: 400.0,
            height: 300.0,
            bottom: 120.0,
        };
        bb.backglass.width = 420.0;
        bb.backglass.height = 320.0;
        bb.grilles = vec![pin_core::schema::cabinet::Grille {
            u: -150.0,
            v: 50.0,
            diameter: 60.0,
        }];
        let scene = load(&generate(&g).glb);
        let f = Face::of(&g);
        for n in BOUNDS_SET {
            let (lo, _) = extent(&verts(&scene, n));
            assert!(
                lo[2] >= f.zb - 1e-3,
                "{centre_v}: {n} min z {} < {}",
                lo[2],
                f.zb
            );
        }
        let ps: Vec<[f64; 3]> = BOUNDS_SET.iter().flat_map(|n| verts(&scene, n)).collect();
        let (lo, hi) = extent(&ps);
        near(lo[2], f.zb, 1.0, "z min");
        near(hi[2], g.backbox_top_height_mm(), 1.0, "z max");
        let (cv, dh) = (f64::from(centre_v), 9.0);
        if cv - dh >= 1.5 {
            assert_hole(
                &scene,
                &f,
                "cabinet.backbox.speaker_panel",
                28.8,
                cv - dh,
                cv + dh,
            );
        }
    }
}
