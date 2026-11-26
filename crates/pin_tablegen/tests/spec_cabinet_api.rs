//! Black-box API tests for the cabinet generator (issue #110, KP-029).
//!
//! Covers the deliverable "`cabinet.glb` with stable node names, UVs and material references"
//! (REQ-CAB-001, REQ-CAB-007) and the generator error path.
#![allow(dead_code)]

use pin_assets::{GltfScene, Vfs, load_gltf};
use pin_core::schema::cabinet::CabinetGeometry;
use pin_tablegen::cabinet::{CABINET_GLB_FILE, GeneratedCabinet, NODE_NAMES, generate_cabinet};
use std::sync::atomic::{AtomicUsize, Ordering};

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

type Edit = fn(&mut CabinetGeometry);

const PRESETS: [&str; 3] = ["classic_standard", "classic_widebody", "modern_standard"];

fn material_of(node: &str) -> String {
    let n = node.strip_prefix("cabinet.").unwrap();
    match n {
        "body.bottom" | "body.rear" => "cabinet.body".into(),
        "body.side_left" => "cabinet.side_left".into(),
        "body.side_right" => "cabinet.side_right".into(),
        "body.front" => "cabinet.front".into(),
        "rail.left" | "rail.right" | "lockdown_bar" => "cabinet.trim_metal".into(),
        "glass" => "cabinet.glass".into(),
        "coin_door" => "cabinet.coin_door".into(),
        "plunger.housing" | "plunger.rod" => "cabinet.plunger".into(),
        // #111 (KP-030) contract section F: backbox and blade nodes.
        "backbox.shell" => "cabinet.backbox".into(),
        "backbox.speaker_panel" => "cabinet.backbox.speaker_panel".into(),
        "backbox.translite_frame" | "backbox.hinge" => "cabinet.trim_metal".into(),
        "backbox.translite" => "cabinet.backbox.translite".into(),
        "backbox.display" => "cabinet.backbox.display".into(),
        "backbox.grilles" => "cabinet.backbox.grille".into(),
        "blade.left" => "cabinet.blade_left".into(),
        "blade.right" => "cabinet.blade_right".into(),
        _ => {
            if let Some(rest) = n.strip_prefix("leg.") {
                if rest.ends_with(".leveler") {
                    "cabinet.leveler".into()
                } else {
                    let leg = rest.strip_suffix(".bracket").unwrap_or(rest);
                    format!("cabinet.leg.{leg}")
                }
            } else {
                format!("cabinet.{n}")
            }
        }
    }
}

fn load(cab: &GeneratedCabinet) -> GltfScene {
    static N: AtomicUsize = AtomicUsize::new(0);
    let dir = std::env::temp_dir().join(format!(
        "spec_cabinet_api_{}_{}",
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

fn gen_ok(g: &CabinetGeometry) -> GeneratedCabinet {
    match generate_cabinet(g) {
        Ok(c) => c,
        Err(e) => panic!("generate_cabinet failed: {} / {}", e.element, e.message),
    }
}

fn preset(id: &str) -> CabinetGeometry {
    CabinetGeometry::preset(id).unwrap_or_else(|e| panic!("{e:?}"))
}

/// REQ-CAB-001: the file name constant and the stable 37-node list (the 28 of #110 plus the 9
/// backbox and blade nodes of #111 contract section C), in order, for every preset.
#[test]
fn req_cab_001_node_list_is_stable_and_ordered() {
    assert_eq!(CABINET_GLB_FILE, "cabinet.glb");
    assert_eq!(NODE_NAMES.len(), 37);
    for (i, n) in NODES.iter().enumerate() {
        assert_eq!(NODE_NAMES[i], *n, "NODE_NAMES[{i}]");
    }
    for id in PRESETS {
        let cab = gen_ok(&preset(id));
        let names: Vec<&str> = cab.nodes.iter().map(String::as_str).collect();
        assert_eq!(names, NODES, "{id}: GeneratedCabinet::nodes");
        let scene = load(&cab);
        let mesh_names: Vec<&str> = scene.meshes.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(
            mesh_names, NODES,
            "{id}: loader mesh names (one mesh per node)"
        );
    }
}

/// REQ-CAB-001: every node has one triangle primitive with finite positions, normals, UVs in
/// [0, 1], and in-range indices that form whole triangles.
#[test]
fn req_cab_001_meshes_have_uvs_normals_and_valid_indices() {
    for id in PRESETS {
        let scene = load(&gen_ok(&preset(id)));
        for m in &scene.meshes {
            let n = &m.name;
            assert!(!m.positions.is_empty(), "{id} {n}: no positions");
            assert_eq!(m.normals.len(), m.positions.len(), "{id} {n}: normals");
            assert_eq!(m.uvs.len(), m.positions.len(), "{id} {n}: uvs");
            assert!(
                !m.indices.is_empty() && m.indices.len() % 3 == 0,
                "{id} {n}: indices"
            );
            assert!(
                m.indices.iter().all(|&i| (i as usize) < m.positions.len()),
                "{id} {n}"
            );
            assert!(
                m.positions.iter().flatten().all(|v| v.is_finite()),
                "{id} {n}: pos"
            );
            assert!(
                m.normals.iter().flatten().all(|v| v.is_finite()),
                "{id} {n}: nrm"
            );
            assert!(
                m.uvs
                    .iter()
                    .flatten()
                    .all(|v| v.is_finite() && (0.0..=1.0).contains(v)),
                "{id} {n}: uv outside [0, 1]"
            );
            assert!(
                m.transform.iter().flatten().all(|v| v.is_finite()),
                "{id} {n}: xf"
            );
        }
    }
}

/// REQ-CAB-001 / REQ-CAB-007: the 24 materials (17 of #110 plus 7 of #111 contract section F),
/// their order, and the node-to-material mapping (including section F's backbox and blade nodes).
#[test]
fn req_cab_001_material_list_and_assignment() {
    for id in PRESETS {
        let scene = load(&gen_ok(&preset(id)));
        let names: Vec<&str> = scene.materials.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, MATERIALS, "{id}: material list");
        for m in &scene.meshes {
            let idx = m
                .material
                .unwrap_or_else(|| panic!("{id} {}: no material", m.name));
            assert_eq!(
                scene.materials[idx].name,
                material_of(&m.name),
                "{id} {}",
                m.name
            );
        }
        let glass = scene
            .materials
            .iter()
            .find(|m| m.name == "cabinet.glass")
            .unwrap();
        assert!(
            glass.base_color[3] < 1.0,
            "glass alpha {}",
            glass.base_color[3]
        );
        for m in &scene.materials {
            assert!(m.base_color.iter().all(|v| v.is_finite()), "{}", m.name);
        }
    }
}

fn srgb_to_linear(c: f64) -> f64 {
    if c > 0.04045 {
        libm::pow((c + 0.055) / 1.055, 2.4)
    } else {
        c / 12.92
    }
}

/// REQ-CAB-007: each leg material's base colour is the linear form of that leg's own colour,
/// alpha 1.
#[test]
fn req_cab_007_leg_material_colour_matches_leg_colour() {
    for id in PRESETS {
        let mut g = preset(id);
        g.legs.front_left.colour = "#ff8000".into();
        g.legs.front_right.colour = "#102030".into();
        g.legs.rear_left.colour = "#0a0a0a".into();
        g.legs.rear_right.colour = "#fafafa".into();
        let scene = load(&gen_ok(&g));
        for (leg, l) in g.legs.all() {
            let m = scene
                .materials
                .iter()
                .find(|m| m.name == format!("cabinet.leg.{leg}"))
                .unwrap();
            let hex = l.colour.trim_start_matches('#');
            for k in 0..3 {
                let c = f64::from(u8::from_str_radix(&hex[2 * k..2 * k + 2], 16).unwrap()) / 255.0;
                let want = srgb_to_linear(c);
                assert!(
                    (f64::from(m.base_color[k]) - want).abs() < 1e-3,
                    "{id} {leg} ch{k}: {} vs {want}",
                    m.base_color[k]
                );
            }
            assert!((m.base_color[3] - 1.0).abs() < 1e-6, "{id} {leg} alpha");
        }
    }
}

/// Deliverable "deterministic bytes": two runs are byte-equal; a changed geometry changes the
/// bytes but not the node or material lists.
#[test]
fn deterministic_bytes_and_stable_lists() {
    for id in PRESETS {
        let g = preset(id);
        let a = gen_ok(&g);
        let b = gen_ok(&g);
        assert_eq!(a.glb, b.glb, "{id}: bytes differ between runs");
        assert_eq!(a.nodes, b.nodes);
        let mut g2 = g.clone();
        g2.legs.rear_left.leveler.extension = g2.legs.rear_left.leveler.extension_min + 12.0;
        let c = gen_ok(&g2);
        assert_ne!(
            a.glb, c.glb,
            "{id}: geometry change left the bytes unchanged"
        );
        assert_eq!(a.nodes, c.nodes);
        let (sa, sc) = (load(&a), load(&c));
        let mat = |s: &GltfScene| {
            s.materials
                .iter()
                .map(|m| m.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(mat(&sa), mat(&sc));
    }
}

/// Deliverable: the file is a binary glTF 2.0 container whose declared length is the byte length.
#[test]
fn glb_header_is_gltf_v2_with_declared_length() {
    for id in PRESETS {
        let b = gen_ok(&preset(id)).glb;
        assert!(b.len() >= 12);
        assert_eq!(&b[0..4], b"glTF");
        assert_eq!(u32::from_le_bytes(b[4..8].try_into().unwrap()), 2);
        assert_eq!(
            u32::from_le_bytes(b[8..12].try_into().unwrap()) as usize,
            b.len()
        );
    }
}

/// REQ-CAB-003 / REQ-CAB-007: invalid dimensions return `GenError` naming the dotted key; no
/// panic.
#[test]
fn invalid_geometry_returns_gen_error_naming_the_key() {
    let cases: [(&str, Edit); 4] = [
        ("body.width", |g| g.body.width = f32::NAN),
        ("body.width", |g| g.body.width = -1.0),
        ("legs.rear_left.length", |g| g.legs.rear_left.length = 0.0),
        ("legs.rear_left.length", |g| {
            g.legs.rear_left.length = f32::NAN
        }),
    ];
    for id in PRESETS {
        for (key, edit) in &cases {
            let mut g = preset(id);
            edit(&mut g);
            match generate_cabinet(&g) {
                Ok(_) => panic!("{id}: invalid {key} accepted"),
                Err(e) => assert!(
                    e.element.contains(key),
                    "{id}: element {:?} does not name {key} ({})",
                    e.element,
                    e.message
                ),
            }
        }
    }
}
