//! KP-029 (#110): `cabinet.glb` loads with the runtime loader, has the
//! stable node list, named materials, UVs and moving nodes, and its bytes
//! are deterministic and pinned by golden hashes (REQ-CAB-001,
//! REQ-CAB-003, REQ-CAB-007).

#[path = "cabinet_common.rs"]
mod common;

use common::*;
use pin_tablegen::cabinet::{CABINET_GLB_FILE, MATERIAL_NAMES, NODE_NAMES, generate_cabinet};

/// FNV-1a of `generate_cabinet` for each preset (in [`PRESETS`] order).
/// Update only with a reviewed geometry change (the analytic tests in
/// `cabinet_dimensions.rs` must pass first).
const GOLDEN: [u64; 3] = [
    0x727c_42ed_7f77_e9d9,
    0x7e5f_ec6f_89f8_ddfc,
    0xd9e0_84af_da1f_4489,
];

/// Acceptance (determinism, R3): two runs give identical bytes and the
/// bytes match the golden hashes for the three presets.
#[test]
fn golden_bytes_are_deterministic() {
    let mut got = [0u64; 3];
    for (i, id) in PRESETS.iter().enumerate() {
        let g = preset(id);
        let a = generate(&g);
        let b = generate(&g);
        assert_eq!(a, b, "{id}: two runs give the same output");
        got[i] = fnv1a(&a.glb);
        println!("{id}: {:#018x}", got[i]);
    }
    assert_eq!(got, GOLDEN, "golden hashes");
}

/// Acceptance: a changed leveler extension changes the bytes but not the
/// node list.
#[test]
fn leveler_change_changes_bytes_not_nodes() {
    let a = generate(&preset("classic_standard"));
    let b = generate(&from_toml("[legs.front_right.leveler]\nextension = 12.5\n"));
    assert_ne!(a.glb, b.glb);
    assert_eq!(a.nodes, b.nodes);
}

/// The override used by the node-list test: a non-default leveler, finish,
/// shape, bracket and bolt on every leg.
const STYLED: &str = r##"
[legs.front_left]
finish = "brushed_steel"
bracket = "flush_plate"
bolt = "carriage"
section = { shape = "round" }
leveler = { extension = 10.0, foot_shape = "swivel" }

[legs.front_right]
finish = "painted"
material = "wood"
bracket = "flush_plate"
bolt = "button_head"
section = { shape = "square" }
leveler = { extension = 44.0, foot_shape = "flat" }

[legs.rear_left]
finish = "wrinkle"
colour = "#204080"
bracket = "flush_plate"
bolt = "hex"
surface = "plain"
section = { shape = "square", top = 70.0 }
leveler = { extension = 5.0, foot_shape = "swivel" }

[legs.rear_right]
finish = "powder_coat"
material = "aluminium"
bracket = "corner_plate"
bolt = "carriage"
section = { shape = "round", top = 60.0 }
leveler = { extension = 30.0, foot_shape = "flat" }
"##;

/// REQ-CAB-001: the glb loads through `pin_assets::load_gltf` (default
/// limits) and its nodes are exactly `NODE_NAMES` and
/// `GeneratedCabinet::nodes`, in order and unique, for each preset and for a
/// fully restyled cabinet.
#[test]
fn node_list_is_stable_and_loads() {
    let mut geoms: Vec<_> = PRESETS.iter().map(|id| preset(id)).collect();
    geoms.push(from_toml(STYLED));
    assert_eq!(CABINET_GLB_FILE, "cabinet.glb");
    for g in &geoms {
        let out = generate(g);
        assert_eq!(out.nodes, NODE_NAMES.map(String::from).to_vec());
        let scene = load(&out.glb);
        let names: Vec<&str> = scene.meshes.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, NODE_NAMES.to_vec());
        let mut sorted = names.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), 28, "unique");
    }
}

fn travel_axis(name: &str) -> Option<&'static str> {
    match name {
        "cabinet.button.left_flipper" => Some("+x"),
        "cabinet.button.right_flipper" => Some("-x"),
        "cabinet.button.start" | "cabinet.button.action" => Some("+y"),
        "cabinet.plunger.rod" => Some("-y"),
        _ => None,
    }
}

/// Moving nodes: the four buttons and the plunger rod carry a translation
/// equal to their control position above the floor (`floor_height_mm`,
/// metres) and `moving`/`travel_axis`/`travel_mm` extras, with local
/// vertices; every other node has the identity transform and no extras.
#[test]
fn buttons_and_plunger_rod_are_moving_nodes() {
    for g in [
        preset("classic_standard"),
        preset("modern_standard"),
        from_toml(STYLED),
    ] {
        let out = generate(&g);
        let scene = load(&out.glb);
        let json = json(&out.glb);
        let c = &g.controls;
        let pivots = [
            ("cabinet.button.left_flipper", c.left_flipper_button),
            ("cabinet.button.right_flipper", c.right_flipper_button),
            ("cabinet.button.start", c.start_button),
            ("cabinet.button.action", c.action_button),
            ("cabinet.plunger.rod", c.plunger),
        ];
        for m in &scene.meshes {
            let nj = node_json(&json, &m.name);
            match travel_axis(&m.name) {
                Some(axis) => {
                    let p = pivots.iter().find(|(n, _)| *n == m.name).unwrap().1;
                    let want = [
                        f64::from(p[0]) / 1000.0,
                        f64::from(p[1]) / 1000.0,
                        g.floor_height_mm(f64::from(p[1]), f64::from(p[2])) / 1000.0,
                    ];
                    for (k, w) in want.into_iter().enumerate() {
                        close(m.transform[3][k], w, 1e-5, &m.name);
                    }
                    assert!(nj.contains("\"moving\":\"true\""), "{nj}");
                    assert!(nj.contains(&format!("\"travel_axis\":\"{axis}\"")), "{nj}");
                    let travel = if m.name.contains("plunger") {
                        "80"
                    } else {
                        "4"
                    };
                    assert!(nj.contains(&format!("\"travel_mm\":\"{travel}\"")), "{nj}");
                    for p in &m.positions {
                        let r = (p[0] * p[0] + p[1] * p[1] + p[2] * p[2]).sqrt();
                        assert!(r < 0.2, "{} vertex {p:?} not local", m.name);
                    }
                }
                None => {
                    let identity = [
                        [1.0, 0.0, 0.0, 0.0],
                        [0.0, 1.0, 0.0, 0.0],
                        [0.0, 0.0, 1.0, 0.0],
                        [0.0, 0.0, 0.0, 1.0],
                    ];
                    assert_eq!(m.transform, identity, "{}", m.name);
                    assert!(!nj.contains("moving"), "{nj}");
                }
            }
        }
    }
}

fn material_of(node: &str) -> String {
    match node {
        "cabinet.body.bottom" | "cabinet.body.rear" => "cabinet.body".into(),
        "cabinet.body.side_left" => "cabinet.side_left".into(),
        "cabinet.body.side_right" => "cabinet.side_right".into(),
        "cabinet.body.front" => "cabinet.front".into(),
        "cabinet.rail.left" | "cabinet.rail.right" | "cabinet.lockdown_bar" => {
            "cabinet.trim_metal".into()
        }
        "cabinet.glass" => "cabinet.glass".into(),
        "cabinet.coin_door" => "cabinet.coin_door".into(),
        "cabinet.plunger.housing" | "cabinet.plunger.rod" => "cabinet.plunger".into(),
        n if n.ends_with(".leveler") => "cabinet.leveler".into(),
        n if n.ends_with(".bracket") => n.trim_end_matches(".bracket").into(),
        n => n.into(), // legs and buttons share their node's name
    }
}

/// UVs and materials (REQ-CAB-001, REQ-CAB-007): every node has one UV per
/// position, all in [0, 1]; the 17 materials are named in order and
/// assigned as documented; each leg material's colour is the linear
/// conversion of `leg.colour` and its metallic/roughness follow the finish
/// (wood never metallic); the glass is translucent.
#[test]
fn uvs_and_named_materials() {
    for g in [
        preset("classic_standard"),
        preset("classic_widebody"),
        from_toml(STYLED),
    ] {
        let out = generate(&g);
        let scene = load(&out.glb);
        let names: Vec<&str> = scene.materials.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, MATERIAL_NAMES.to_vec());
        for m in &scene.meshes {
            assert_eq!(m.uvs.len(), m.positions.len(), "{}", m.name);
            assert!(
                m.uvs.iter().flatten().all(|v| (0.0..=1.0).contains(v)),
                "{} UVs",
                m.name
            );
            let mat = &scene.materials[m.material.unwrap()];
            assert_eq!(mat.name, material_of(&m.name), "{}", m.name);
        }
        for (name, leg) in g.legs.all() {
            let mat = scene
                .materials
                .iter()
                .find(|m| m.name == format!("cabinet.leg.{name}"))
                .unwrap();
            let want = linear(&leg.colour);
            for (k, w) in want.into_iter().enumerate() {
                close(mat.base_color[k], f64::from(w), 1e-6, name);
            }
            assert_eq!(mat.base_color[3], 1.0);
            use pin_core::schema::cabinet::{Finish, LegMaterial};
            let (metal, rough) = match leg.finish {
                Finish::Chrome => (1.0, 0.15),
                Finish::BrushedSteel => (1.0, 0.4),
                Finish::PowderCoat => (0.0, 0.6),
                Finish::Wrinkle => (0.0, 0.8),
                Finish::Painted => (0.0, 0.5),
            };
            let metal = if leg.material == LegMaterial::Wood {
                0.0
            } else {
                metal
            };
            assert_eq!((mat.metallic, mat.roughness), (metal, rough), "{name}");
        }
        let trim = scene
            .materials
            .iter()
            .find(|m| m.name == "cabinet.trim_metal")
            .unwrap();
        let want = linear(&g.trim.rail_colour);
        for (k, w) in want.into_iter().enumerate() {
            close(trim.base_color[k], f64::from(w), 1e-6, "trim");
        }
        let glass = scene
            .materials
            .iter()
            .find(|m| m.name == "cabinet.glass")
            .unwrap();
        assert!(glass.base_color[3] < 1.0);
    }
}

/// Error path through the public API: a NaN size names its key.
#[test]
fn nan_size_is_an_error_naming_the_key() {
    let mut g = preset("modern_standard");
    g.legs.rear_left.length = f32::NAN;
    let e = generate_cabinet(&g).unwrap_err();
    assert_eq!(e.element, "legs.rear_left.length");
    let mut g = preset("modern_standard");
    g.body.width = -560.0;
    assert_eq!(generate_cabinet(&g).unwrap_err().element, "body.width");
    let mut g = preset("modern_standard");
    g.controls.plunger[1] = f32::NAN;
    assert_eq!(
        generate_cabinet(&g).unwrap_err().element,
        "controls.plunger"
    );
}
