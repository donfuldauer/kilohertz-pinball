//! KP-011 / #91 Independent Tester spec tests (GPU-free): glTF `doubleSided` parsing
//! and hostile/malformed `doubleSided` fuzzing (REQ-VIS-001, docs/SPEC.md §7.4, R11).

#[path = "spec_fixtures/mod.rs"]
mod fx;

use std::collections::BTreeMap;

use fx::{BufferRef, DECK_EXTRAS, INSERT_EXTRAS, scene_bin, scene_json};
use pin_assets::{AssetError, GltfScene, Vfs, ZipLimits, load_gltf};
use proptest::prelude::*;

fn vfs(files: &[(&str, Vec<u8>)]) -> Vfs {
    let m: BTreeMap<String, Vec<u8>> = files
        .iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect();
    Vfs::from_zip_bytes(fx::zip_of(&m), ZipLimits::default()).unwrap()
}

fn load_single_glb(json: &str) -> Result<GltfScene, AssetError> {
    let glb = fx::glb(json, &scene_bin().0);
    let v = vfs(&[("scene.glb", glb)]);
    load_gltf(&v, "scene.glb")
}

/// REQ-VIS-001: glTF 2.0 `doubleSided` material property parsing:
/// - omitted -> `false` (glTF 2.0 default)
/// - `"doubleSided": false` -> `false`
/// - `"doubleSided": true` -> `true`
/// - mixed materials in a single scene preserve per-material `double_sided`.
#[test]
fn req_vis_001_gltf_double_sided_parsing() {
    let base = scene_json(BufferRef::Embedded, DECK_EXTRAS, INSERT_EXTRAS);

    // (a) Omitted in both materials ("wood" and "glow" in standard scene).
    let scene_omitted = load_single_glb(&base).expect("standard scene with doubleSided omitted");
    assert_eq!(scene_omitted.materials.len(), 2);
    assert!(
        !scene_omitted.materials[0].double_sided,
        "omitted doubleSided must default to false (wood)"
    );
    assert!(
        !scene_omitted.materials[1].double_sided,
        "omitted doubleSided must default to false (glow)"
    );

    // (b) Explicit `"doubleSided": false`.
    let json_false = fx::sub(
        &base,
        r#"{"name":"glow"}"#,
        r#"{"name":"glow","doubleSided":false}"#,
    );
    let scene_false = load_single_glb(&json_false).expect("explicit doubleSided: false");
    assert!(!scene_false.materials[0].double_sided);
    assert!(!scene_false.materials[1].double_sided);

    // (c) Explicit `"doubleSided": true`.
    let json_true = fx::sub(
        &base,
        r#"{"name":"glow"}"#,
        r#"{"name":"glow","doubleSided":true}"#,
    );
    let scene_true = load_single_glb(&json_true).expect("explicit doubleSided: true");
    assert!(
        !scene_true.materials[0].double_sided,
        "material 0 omitted doubleSided must remain false"
    );
    assert!(
        scene_true.materials[1].double_sided,
        "material 1 with doubleSided: true must be true"
    );

    // (d) Multiple materials mixing omitted, false, and true in one scene.
    let json_mixed = fx::sub(
        &base,
        r#""materials":[
 {"name":"wood","pbrMetallicRoughness":{"baseColorFactor":[0.8,0.6,0.4,0.5],"metallicFactor":0.25,"roughnessFactor":0.75,"baseColorTexture":{"index":0}},"emissiveFactor":[0.1,0.2,0.3],"emissiveTexture":{"index":1}},
 {"name":"glow"}
]"#,
        r#""materials":[
 {"name":"wood_true","doubleSided":true,"pbrMetallicRoughness":{"baseColorFactor":[0.8,0.6,0.4,0.5],"metallicFactor":0.25,"roughnessFactor":0.75,"baseColorTexture":{"index":0}},"emissiveFactor":[0.1,0.2,0.3],"emissiveTexture":{"index":1}},
 {"name":"glow_omitted"},
 {"name":"wall_false","doubleSided":false},
 {"name":"wall_true","doubleSided":true}
]"#,
    );
    let scene_mixed = load_single_glb(&json_mixed).expect("mixed materials scene");
    assert_eq!(scene_mixed.materials.len(), 4);
    assert_eq!(scene_mixed.materials[0].name, "wood_true");
    assert!(scene_mixed.materials[0].double_sided);
    assert_eq!(scene_mixed.materials[1].name, "glow_omitted");
    assert!(!scene_mixed.materials[1].double_sided);
    assert_eq!(scene_mixed.materials[2].name, "wall_false");
    assert!(!scene_mixed.materials[2].double_sided);
    assert_eq!(scene_mixed.materials[3].name, "wall_true");
    assert!(scene_mixed.materials[3].double_sided);
}

/// REQ-VIS-001 (also via text `.gltf` + external buffer): `doubleSided` parses identically
/// for `.gltf` text assets, not only `.glb`.
#[test]
fn req_vis_001_text_gltf_double_sided_parsing() {
    let base = scene_json(BufferRef::Uri("buf.bin"), DECK_EXTRAS, INSERT_EXTRAS);
    let json = fx::sub(
        &base,
        r#"{"name":"glow"}"#,
        r#"{"name":"glow","doubleSided":true}"#,
    );
    let v = vfs(&[
        ("models/wall.gltf", json.into_bytes()),
        ("models/buf.bin", scene_bin().0),
    ]);
    let scene = load_gltf(&v, "models/wall.gltf").expect("text glTF loads");
    assert!(!scene.materials[0].double_sided);
    assert!(scene.materials[1].double_sided);
}

/// REQ-VIS-001 / R11: non-boolean `"doubleSided"` values never panic `load_gltf`
/// (return `Err(AssetError::Gltf(_))` or `Ok` cleanly).
#[test]
fn req_vis_001_hostile_non_boolean_double_sided_never_panics() {
    let base = scene_json(BufferRef::Embedded, DECK_EXTRAS, INSERT_EXTRAS);
    for bad_value in [
        "1",
        "0",
        "-1",
        "1.5",
        r#""true""#,
        r#""false""#,
        "null",
        "[]",
        "{}",
        "[true]",
        r#"{"doubleSided":true}"#,
    ] {
        let json = fx::sub(
            &base,
            r#"{"name":"glow"}"#,
            &format!(r#"{{"name":"glow","doubleSided":{bad_value}}}"#),
        );
        match load_single_glb(&json) {
            Ok(scene) => {
                // If tolerated, material count and indices must remain valid.
                assert_eq!(scene.materials.len(), 2);
            }
            Err(AssetError::Gltf(msg)) => {
                assert!(
                    !msg.is_empty(),
                    "AssetError::Gltf message must be non-empty"
                );
            }
            Err(other) => {
                panic!("unexpected error variant for doubleSided={bad_value}: {other:?}");
            }
        }
    }
}

fn json_value_strategy(depth: u32) -> BoxedStrategy<String> {
    let leaf = prop_oneof![
        Just("null".to_owned()),
        Just("true".to_owned()),
        Just("false".to_owned()),
        any::<i64>().prop_map(|n| n.to_string()),
        Just("1e400".to_owned()),
        "[a-zA-Z0-9_]{0,12}".prop_map(|s| format!("\"{s}\"")),
    ];
    if depth == 0 {
        return leaf.boxed();
    }
    prop_oneof![
        3 => leaf,
        1 => prop::collection::vec(json_value_strategy(depth - 1), 0..4)
            .prop_map(|v| format!("[{}]", v.join(","))),
        1 => prop::collection::btree_map("[a-z]{1,6}", json_value_strategy(depth - 1), 0..3)
            .prop_map(|m| {
                let items: Vec<String> = m.iter().map(|(k, v)| format!("\"{k}\":{v}")).collect();
                format!("{{{}}}", items.join(","))
            }),
    ]
    .boxed()
}

fn mutate_bytes(mut bytes: Vec<u8>, edits: &[(usize, u8)]) -> Vec<u8> {
    for (pos, val) in edits {
        if !bytes.is_empty() {
            let i = pos % bytes.len();
            bytes[i] = *val;
        }
    }
    bytes
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, .. ProptestConfig::default() })]

    /// REQ-VIS-001 / R11: arbitrary JSON values in `"doubleSided"` never panic `load_gltf`.
    #[test]
    fn req_vis_001_random_json_in_double_sided_never_panics(val in json_value_strategy(2)) {
        let base = scene_json(BufferRef::Embedded, DECK_EXTRAS, INSERT_EXTRAS);
        let json = fx::sub(
            &base,
            r#"{"name":"glow"}"#,
            &format!(r#"{{"name":"glow","doubleSided":{val}}}"#),
        );
        let _ = load_single_glb(&json);
    }

    /// REQ-VIS-001 / R11: bounded random-byte mutations around a glTF JSON containing
    /// `"doubleSided"` never panic `load_gltf`, and any accepted scene keeps finite materials.
    #[test]
    fn req_vis_001_mutated_double_sided_glb_never_panics(
        ds in any::<bool>(),
        edits in prop::collection::vec((any::<usize>(), any::<u8>()), 1..10),
    ) {
        let base = scene_json(BufferRef::Embedded, DECK_EXTRAS, INSERT_EXTRAS);
        let json = fx::sub(
            &base,
            r#"{"name":"glow"}"#,
            &format!(r#"{{"name":"glow","doubleSided":{ds}}}"#),
        );
        let glb = fx::glb(&json, &scene_bin().0);
        let mutated = mutate_bytes(glb, &edits);
        let v = vfs(&[("mutated.glb", mutated)]);
        if let Ok(scene) = load_gltf(&v, "mutated.glb") {
            for m in &scene.materials {
                prop_assert!(m.base_color.iter().all(|c| c.is_finite()));
                prop_assert!(m.metallic.is_finite() && m.roughness.is_finite());
            }
        }
    }

    /// REQ-VIS-001 / R11: bounded random-byte mutations on text `.gltf` JSON containing
    /// `"doubleSided"` never panic `load_gltf`.
    #[test]
    fn req_vis_001_mutated_double_sided_text_gltf_never_panics(
        ds in any::<bool>(),
        edits in prop::collection::vec((any::<usize>(), any::<u8>()), 1..10),
    ) {
        let base = scene_json(BufferRef::Uri("buf.bin"), DECK_EXTRAS, INSERT_EXTRAS);
        let json = fx::sub(
            &base,
            r#"{"name":"glow"}"#,
            &format!(r#"{{"name":"glow","doubleSided":{ds}}}"#),
        );
        let mutated = mutate_bytes(json.into_bytes(), &edits);
        let v = vfs(&[
            ("mutated.gltf", mutated),
            ("buf.bin", scene_bin().0),
        ]);
        let _ = load_gltf(&v, "mutated.gltf");
    }
}
