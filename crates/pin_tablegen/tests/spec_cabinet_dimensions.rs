//! Dimension, moving-node, independence and data-not-code tests for the cabinet generator
//! (issue #110, KP-029): REQ-CAB-001, REQ-CAB-003, REQ-CAB-007.
//!
//! Frame: metres, x across (player's right positive), y from the body front toward the rear,
//! z = height above the floor plane. Placed vertex = node transform applied to positions.
#![allow(dead_code)]

use pin_assets::{GltfScene, MeshData, Vfs, load_gltf};
use pin_core::schema::cabinet::{CabinetGeometry, CabinetManifest, Leg, Legs};
use pin_tablegen::cabinet::{CABINET_GLB_FILE, GeneratedCabinet, generate_cabinet};
use std::sync::atomic::{AtomicUsize, Ordering};

const PRESETS: [&str; 3] = ["classic_standard", "classic_widebody", "modern_standard"];
const MM: f64 = 0.001;
const TOL: f64 = 0.001;

const BODY_NODES: [&str; 5] = [
    "cabinet.body.bottom",
    "cabinet.body.side_left",
    "cabinet.body.side_right",
    "cabinet.body.front",
    "cabinet.body.rear",
];

fn load(cab: &GeneratedCabinet) -> GltfScene {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "spec_cabinet_dim_{}_{}",
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

fn gen_scene(g: &CabinetGeometry) -> (GeneratedCabinet, GltfScene) {
    let cab = match generate_cabinet(g) {
        Ok(c) => c,
        Err(e) => panic!("generate_cabinet failed: {} / {}", e.element, e.message),
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

fn placed(m: &MeshData) -> Vec<[f64; 3]> {
    let t = &m.transform;
    m.positions
        .iter()
        .map(|p| {
            let (x, y, z) = (f64::from(p[0]), f64::from(p[1]), f64::from(p[2]));
            [0, 1, 2].map(|r| {
                f64::from(t[0][r]) * x
                    + f64::from(t[1][r]) * y
                    + f64::from(t[2][r]) * z
                    + f64::from(t[3][r])
            })
        })
        .collect()
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

fn node_bounds(s: &GltfScene, name: &str) -> Bounds {
    bounds(&placed(mesh(s, name)))
}

fn body_bounds(s: &GltfScene) -> Bounds {
    let mut all = Vec::new();
    for n in BODY_NODES {
        all.extend(placed(mesh(s, n)));
    }
    bounds(&all)
}

fn near(a: f64, b: f64, tol: f64, what: &str) {
    assert!(
        (a - b).abs() <= tol,
        "{what}: got {a}, want {b} (tol {tol})"
    );
}

fn translation(m: &MeshData) -> [f64; 3] {
    [0, 1, 2].map(|r| f64::from(m.transform[3][r]))
}

fn is_identity_rotation(m: &MeshData) -> bool {
    (0..3).all(|c| {
        (0..3).all(|r| {
            let want = if c == r { 1.0 } else { 0.0 };
            (f64::from(m.transform[c][r]) - want).abs() < 1e-6
        })
    })
}

fn legs_mut(l: &mut Legs) -> [&mut Leg; 4] {
    [
        &mut l.front_left,
        &mut l.front_right,
        &mut l.rear_left,
        &mut l.rear_right,
    ]
}

fn control_positions(g: &CabinetGeometry) -> [(&'static str, [f32; 3]); 5] {
    let c = &g.controls;
    [
        ("cabinet.button.left_flipper", c.left_flipper_button),
        ("cabinet.button.right_flipper", c.right_flipper_button),
        ("cabinet.button.start", c.start_button),
        ("cabinet.button.action", c.action_button),
        ("cabinet.plunger.rod", c.plunger),
    ]
}

/// Asserts every dimension, leg, glass, coin-door and moving-node rule for `g`.
fn check_geometry(g: &CabinetGeometry, label: &str) -> GltfScene {
    let (_, s) = gen_scene(g);
    let b = &g.body;

    // Body extents (REQ-CAB-001, REQ-CAB-003).
    let (lo, hi) = body_bounds(&s);
    near(
        hi[0] - lo[0],
        f64::from(b.width) * MM,
        TOL,
        &format!("{label} body width"),
    );
    near(lo[1], 0.0, TOL, &format!("{label} body y min"));
    near(
        hi[1],
        f64::from(b.length) * MM,
        TOL,
        &format!("{label} body y max"),
    );

    // Wall tops.
    let front = node_bounds(&s, "cabinet.body.front").1[2];
    near(
        front,
        g.floor_height_mm(0.0, f64::from(b.front_height)) * MM,
        TOL,
        "front top",
    );
    let rear = node_bounds(&s, "cabinet.body.rear").1[2];
    let rear_want = g.floor_height_mm(f64::from(b.length), f64::from(b.rear_height));
    near(rear, rear_want * MM, TOL, &format!("{label} rear top"));

    // Lockdown bar and glass.
    let bar = node_bounds(&s, "cabinet.lockdown_bar").1[2];
    near(
        bar,
        g.lockdown_bar_top_height_mm() * MM,
        TOL,
        &format!("{label} bar top"),
    );
    let (glo, ghi) = node_bounds(&s, "cabinet.glass");
    near(
        ghi[0] - glo[0],
        f64::from(g.glass_panel.width) * MM,
        TOL,
        "glass width",
    );
    // The glass passes under the lockdown bar where they overlap in y (front edge if none do).
    let (blo, bhi) = node_bounds(&s, "cabinet.lockdown_bar");
    let gpts = placed(mesh(&s, "cabinet.glass"));
    let mut under: Vec<f64> = gpts
        .iter()
        .filter(|p| p[1] >= blo[1] - 1e-6 && p[1] <= bhi[1] + 1e-6)
        .map(|p| p[2])
        .collect();
    if under.is_empty() {
        under = gpts
            .iter()
            .filter(|p| p[1] <= glo[1] + 1e-6)
            .map(|p| p[2])
            .collect();
    }
    let mut under_top = f64::NEG_INFINITY;
    for z in under {
        if z > under_top {
            under_top = z;
        }
    }
    assert!(
        under_top <= bar + TOL,
        "{label}: glass under the bar reaches {under_top}, bar top {bar}"
    );
    // The glass stays within the body: not above the rear wall top.
    assert!(
        ghi[2] <= rear_want * MM + TOL,
        "{label}: glass top {} above rear wall top {}",
        ghi[2],
        rear_want * MM
    );

    // Legs (REQ-CAB-007) and the floor plane.
    let mut global_min = f64::INFINITY;
    for m in &s.meshes {
        let z = bounds(&placed(m)).0[2];
        if z < global_min {
            global_min = z;
        }
    }
    assert!(
        global_min >= -1e-4,
        "{label}: geometry below the floor: {global_min}"
    );
    near(global_min, 0.0, 1e-4, &format!("{label} global min z"));
    for (name, leg) in g.legs.all() {
        let (lo, hi) = node_bounds(&s, &format!("cabinet.leg.{name}"));
        let len = f64::from(leg.length) * MM;
        let ext = f64::from(leg.leveler.extension) * MM;
        near(
            hi[2] - lo[2],
            len,
            TOL,
            &format!("{label} {name} shaft length"),
        );
        near(lo[2], ext, TOL, &format!("{label} {name} shaft z min"));
        near(
            hi[2],
            ext + len,
            TOL,
            &format!("{label} {name} shaft z max"),
        );
        let right = name.ends_with("right");
        let rear = name.starts_with("rear");
        let ix = f64::from(leg.corner_inset[0]);
        let iy = f64::from(leg.corner_inset[1]);
        let hw = f64::from(b.width) * 0.5;
        let cx = if right { hw - ix } else { ix - hw };
        let cy = if rear { f64::from(b.length) - iy } else { iy };
        near(
            (lo[0] + hi[0]) * 0.5,
            cx * MM,
            TOL,
            &format!("{label} {name} centre x"),
        );
        near(
            (lo[1] + hi[1]) * 0.5,
            cy * MM,
            TOL,
            &format!("{label} {name} centre y"),
        );
        let lev = node_bounds(&s, &format!("cabinet.leg.{name}.leveler"));
        near(lev.0[2], 0.0, TOL, &format!("{label} {name} leveler min z"));
    }

    // Coin door.
    let cd = &g.controls.coin_door;
    let (lo, hi) = node_bounds(&s, "cabinet.coin_door");
    near(
        hi[0] - lo[0],
        f64::from(cd.width) * MM,
        2.0 * MM,
        "coin door width",
    );
    near(
        (lo[0] + hi[0]) * 0.5,
        f64::from(cd.centre[0]) * MM,
        2.0 * MM,
        "coin door x",
    );
    near(
        hi[2] - lo[2],
        f64::from(cd.height) * MM,
        2.0 * MM,
        "coin door height",
    );
    let cz = g.floor_height_mm(f64::from(cd.centre[1]), f64::from(cd.centre[2]));
    near((lo[2] + hi[2]) * 0.5, cz * MM, 2.0 * MM, "coin door z");

    // Moving nodes: pure translation to the control position; local geometry.
    let moving = control_positions(g);
    for (name, p) in moving {
        let m = mesh(&s, name);
        assert!(
            is_identity_rotation(m),
            "{label} {name}: rotation is not identity"
        );
        let t = translation(m);
        let want = [
            f64::from(p[0]) * MM,
            f64::from(p[1]) * MM,
            g.floor_height_mm(f64::from(p[1]), f64::from(p[2])) * MM,
        ];
        for a in 0..3 {
            near(
                t[a],
                want[a],
                1e-5,
                &format!("{label} {name} translation[{a}]"),
            );
        }
        assert!(
            m.positions.iter().flatten().all(|v| v.abs() <= 0.2),
            "{label} {name}: mesh not local to its pivot"
        );
    }
    // Every other node has an identity transform.
    for m in &s.meshes {
        if moving.iter().any(|(n, _)| *n == m.name) {
            continue;
        }
        assert!(is_identity_rotation(m), "{label} {}: rotation", m.name);
        for (a, v) in translation(m).into_iter().enumerate() {
            assert!(v.abs() < 1e-6, "{label} {}: translation[{a}] = {v}", m.name);
        }
    }
    s
}

/// REQ-CAB-001 / REQ-CAB-003 / REQ-CAB-007, acceptance "width, length, body heights, leg lengths
/// and lockdown height match the preset within 1 mm; legs touch the floor plane", all presets.
#[test]
fn req_cab_001_dimensions_match_every_preset() {
    for id in PRESETS {
        check_geometry(&preset(id), id);
    }
}

/// REQ-CAB-001: absolute cross-check of classic_standard against the specification's numbers.
#[test]
fn req_cab_001_classic_standard_absolute_numbers() {
    let g = preset("classic_standard");
    let (_, s) = gen_scene(&g);
    let (lo, hi) = body_bounds(&s);
    near(hi[0] - lo[0], 0.560, TOL, "width");
    near(hi[1], 1.315, TOL, "length");
    near(
        node_bounds(&s, "cabinet.body.front").1[2],
        0.890,
        TOL,
        "front wall top",
    );
    near(
        node_bounds(&s, "cabinet.body.rear").1[2],
        1.095,
        TOL,
        "rear wall top",
    );
    near(
        node_bounds(&s, "cabinet.lockdown_bar").1[2],
        0.910,
        TOL,
        "lockdown bar top",
    );
    for leg in ["front_left", "front_right", "rear_left", "rear_right"] {
        let (lo, hi) = node_bounds(&s, &format!("cabinet.leg.{leg}"));
        near(hi[2] - lo[2], 0.724, TOL, leg);
    }
    let m = preset("modern_standard");
    let (_, s) = gen_scene(&m);
    near(
        node_bounds(&s, "cabinet.lockdown_bar").1[2],
        0.914,
        TOL,
        "modern lockdown bar top",
    );
}

/// REQ-CAB-007: legs touch the floor with leveler extension at min, default and max, all legs
/// together and mixed per leg.
#[test]
fn req_cab_007_legs_touch_floor_across_leveler_extension() {
    for id in PRESETS {
        let base = preset(id);
        for mode in 0..3 {
            let mut g = base.clone();
            for leg in legs_mut(&mut g.legs) {
                let l = &mut leg.leveler;
                l.extension = match mode {
                    0 => l.extension_min,
                    1 => l.extension,
                    _ => l.extension_max,
                };
            }
            check_geometry(&g, &format!("{id} uniform mode {mode}"));
        }
        let mut g = base.clone();
        let picks = [0, 2, 1, 0];
        for (leg, pick) in legs_mut(&mut g.legs).into_iter().zip(picks) {
            let l = &mut leg.leveler;
            l.extension = match pick {
                0 => l.extension_min,
                1 => l.extension,
                _ => l.extension_max,
            };
        }
        check_geometry(&g, &format!("{id} mixed"));
    }
}

/// REQ-CAB-007: legs are independent. Left and right levelers are set separately: the shaft z
/// positions differ by the difference of the extensions.
#[test]
fn req_cab_007_left_and_right_levelers_are_independent() {
    for id in PRESETS {
        let mut g = preset(id);
        let (rl_min, rr_min) = (
            g.legs.rear_left.leveler.extension_min,
            g.legs.rear_right.leveler.extension_min,
        );
        g.legs.rear_left.leveler.extension = rl_min + 4.0;
        g.legs.rear_right.leveler.extension = rr_min + 16.0;
        let s = check_geometry(&g, id);
        let rl = node_bounds(&s, "cabinet.leg.rear_left");
        let rr = node_bounds(&s, "cabinet.leg.rear_right");
        near(
            rr.0[2] - rl.0[2],
            (16.0 + f64::from(rr_min) - 4.0 - f64::from(rl_min)) * MM,
            TOL,
            id,
        );
        let lev_l = node_bounds(&s, "cabinet.leg.rear_left.leveler");
        let lev_r = node_bounds(&s, "cabinet.leg.rear_right.leveler");
        near(lev_l.0[2], 0.0, TOL, "rear_left leveler min z");
        near(lev_r.0[2], 0.0, TOL, "rear_right leveler min z");
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

/// Compares two scenes: returns the names of nodes and materials that differ.
fn diff(a: &GltfScene, b: &GltfScene) -> (Vec<String>, Vec<String>) {
    assert_eq!(a.meshes.len(), b.meshes.len());
    assert_eq!(a.materials.len(), b.materials.len());
    let nodes = a
        .meshes
        .iter()
        .zip(&b.meshes)
        .filter(|(x, y)| !same_mesh(x, y))
        .map(|(x, _)| x.name.clone())
        .collect();
    let mats = a
        .materials
        .iter()
        .zip(&b.materials)
        .filter(|(x, y)| x != y)
        .map(|(x, _)| x.name.clone())
        .collect();
    (nodes, mats)
}

/// REQ-CAB-007: changing only rear_left's section shape, finish, colour, bracket or bolt changes
/// only the `cabinet.leg.rear_left*` nodes and the `cabinet.leg.rear_left` material.
#[test]
fn req_cab_007_rear_left_attributes_do_not_touch_other_legs() {
    let variants: [(&str, &str, &str); 5] = [
        (
            "section",
            "[legs.rear_left.section]\nshape = \"square\"\ntop = 30.0\n",
            "[legs.rear_left.section]\nshape = \"round\"\ntop = 30.0\n",
        ),
        (
            "finish",
            "[legs.rear_left]\nfinish = \"powder_coat\"\n",
            "[legs.rear_left]\nfinish = \"brushed_steel\"\n",
        ),
        (
            "colour",
            "[legs.rear_left]\ncolour = \"#202020\"\n",
            "[legs.rear_left]\ncolour = \"#c0a030\"\n",
        ),
        (
            "bracket",
            "[legs.rear_left]\nbracket = \"corner_plate\"\n",
            "[legs.rear_left]\nbracket = \"flush_plate\"\n",
        ),
        (
            "bolt",
            "[legs.rear_left]\nbolt = \"hex\"\n",
            "[legs.rear_left]\nbolt = \"carriage\"\n",
        ),
    ];
    for id in PRESETS {
        for (what, a, b) in variants {
            let (ca, sa) = gen_scene(&from_toml(a, id));
            let (cb, sb) = gen_scene(&from_toml(b, id));
            assert_ne!(ca.glb, cb.glb, "{id} {what}: no effect on the file");
            let (nodes, mats) = diff(&sa, &sb);
            assert!(
                nodes.iter().all(|n| n.starts_with("cabinet.leg.rear_left")),
                "{id} {what}: other nodes changed: {nodes:?}"
            );
            assert!(
                mats.iter().all(|m| m == "cabinet.leg.rear_left"),
                "{id} {what}: other materials changed: {mats:?}"
            );
            assert!(
                !nodes.is_empty() || !mats.is_empty(),
                "{id} {what}: nothing changed"
            );
        }
    }
}

/// REQ-CAB-007: changing rear_left's length changes its own shaft by the difference and leaves
/// the other three legs' nodes and every leg material untouched.
#[test]
fn req_cab_007_rear_left_length_is_independent() {
    for id in PRESETS {
        let g0 = preset(id);
        let g1 = from_toml("[legs.rear_left]\nlength = 740.0\n", id);
        let (_, s0) = gen_scene(&g0);
        let s1 = check_geometry(&g1, id);
        let (nodes, mats) = diff(&s0, &s1);
        for n in &nodes {
            let other_leg =
                n.starts_with("cabinet.leg.") && !n.starts_with("cabinet.leg.rear_left");
            assert!(
                !other_leg,
                "{id}: another leg's node {n} changed with rear_left length"
            );
        }
        assert!(
            mats.iter().all(|m| m == "cabinet.leg.rear_left"),
            "{id}: {mats:?}"
        );
        let before = node_bounds(&s0, "cabinet.leg.rear_left");
        let after = node_bounds(&s1, "cabinet.leg.rear_left");
        let delta = (after.1[2] - after.0[2]) - (before.1[2] - before.0[2]);
        near(
            delta,
            (740.0 - f64::from(g0.legs.rear_left.length)) * MM,
            TOL,
            id,
        );
        let rr0 = node_bounds(&s0, "cabinet.leg.rear_right");
        let rr1 = node_bounds(&s1, "cabinet.leg.rear_right");
        assert_eq!(rr0, rr1, "{id}: rear_right shaft moved");
    }
}

struct Metrics {
    body_w: f64,
    body_l: f64,
    front_top: f64,
    rear_top: f64,
    bar_top: f64,
    glass_w: f64,
    plunger: [f64; 3],
    start: [f64; 3],
}

fn metrics(g: &CabinetGeometry) -> Metrics {
    let s = check_geometry(g, "metrics");
    let (lo, hi) = body_bounds(&s);
    let (glo, ghi) = node_bounds(&s, "cabinet.glass");
    Metrics {
        body_w: hi[0] - lo[0],
        body_l: hi[1],
        front_top: node_bounds(&s, "cabinet.body.front").1[2],
        rear_top: node_bounds(&s, "cabinet.body.rear").1[2],
        bar_top: node_bounds(&s, "cabinet.lockdown_bar").1[2],
        glass_w: ghi[0] - glo[0],
        plunger: translation(mesh(&s, "cabinet.plunger.rod")),
        start: translation(mesh(&s, "cabinet.button.start")),
    }
}

/// REQ-CAB-003: dimensions are data. Each override in the TOML moves the model by its amount.
#[test]
fn req_cab_003_overrides_move_the_model() {
    for id in PRESETS {
        let g0 = preset(id);
        let m0 = metrics(&g0);
        let b = &g0.body;

        let g = from_toml(&format!("[body]\nwidth = {:?}\n", b.width + 30.0), id);
        near(
            metrics(&g).body_w - m0.body_w,
            0.030,
            TOL,
            &format!("{id} body.width"),
        );

        let g = from_toml(&format!("[body]\nlength = {:?}\n", b.length + 60.0), id);
        near(
            metrics(&g).body_l - m0.body_l,
            0.060,
            TOL,
            &format!("{id} body.length"),
        );

        let g = from_toml(
            &format!("[body]\nfront_height = {:?}\n", b.front_height + 5.0),
            id,
        );
        let m = metrics(&g);
        near(
            m.front_top - m0.front_top,
            0.005,
            TOL,
            &format!("{id} body.front_height"),
        );

        let g = from_toml(
            &format!("[body]\nrear_height = {:?}\n", b.rear_height + 7.0),
            id,
        );
        let m = metrics(&g);
        near(
            m.rear_top - m0.rear_top,
            0.007,
            TOL,
            &format!("{id} body.rear_height"),
        );

        let h = b.lockdown_bar.height + 6.0;
        let g = from_toml(&format!("[body.lockdown_bar]\nheight = {h:?}\n"), id);
        near(
            metrics(&g).bar_top - m0.bar_top,
            0.006,
            TOL,
            &format!("{id} lockdown height"),
        );

        let w = g0.glass_panel.width - 6.0;
        let g = from_toml(&format!("[glass_panel]\nwidth = {w:?}\n"), id);
        near(
            metrics(&g).glass_w - m0.glass_w,
            -0.006,
            TOL,
            &format!("{id} glass width"),
        );

        let p = g0.controls.plunger;
        let g = from_toml(
            &format!(
                "[controls]\nplunger = [{:?}, {:?}, {:?}]\n",
                p[0] - 10.0,
                p[1],
                p[2] + 5.0
            ),
            id,
        );
        let m = metrics(&g);
        near(
            m.plunger[0] - m0.plunger[0],
            -0.010,
            1e-5,
            &format!("{id} plunger x"),
        );
        near(
            m.plunger[2] - m0.plunger[2],
            0.005,
            1e-5,
            &format!("{id} plunger z"),
        );

        let st = g0.controls.start_button;
        let g = from_toml(
            &format!(
                "[controls]\nstart_button = [{:?}, {:?}, {:?}]\n",
                st[0] + 10.0,
                st[1],
                st[2]
            ),
            id,
        );
        let m = metrics(&g);
        near(
            m.start[0] - m0.start[0],
            0.010,
            1e-5,
            &format!("{id} start x"),
        );
    }
}
