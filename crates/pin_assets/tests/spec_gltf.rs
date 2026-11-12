//! Subphase 4.1 spec tests (Independent Tester): glTF loading and extras.
//! Brief: deliverable 3, FIXED API `load_gltf` / `GltfScene` / `MeshData` /
//! `NodeExtras` / `PbrMaterial` / `validate_gltf_bindings`, criterion 5.

#[path = "spec_fixtures/mod.rs"]
mod fx;

use std::collections::BTreeMap;

use fx::{BufferRef, DECK_EXTRAS, INSERT_EXTRAS, TempDir, scene_bin, scene_json};
use pin_assets::{
    AssetError, GltfScene, MeshData, Vfs, ZipLimits, load_gltf, load_table, validate_gltf_bindings,
};

fn vfs(files: &[(&str, Vec<u8>)]) -> Vfs {
    let m: BTreeMap<String, Vec<u8>> = files.iter().map(|(k, v)| ((*k).to_owned(), v.clone())).collect();
    Vfs::from_zip_bytes(fx::zip_of(&m), ZipLimits::default()).unwrap()
}

fn mesh<'a>(s: &'a GltfScene, name: &str) -> &'a MeshData {
    let found: Vec<&MeshData> = s.meshes.iter().filter(|m| m.name == name).collect();
    assert_eq!(
        found.len(),
        1,
        "expected exactly one mesh named {name:?}; names: {:?}",
        s.meshes.iter().map(|m| &m.name).collect::<Vec<_>>()
    );
    found[0]
}

fn assert_mat(got: [[f32; 4]; 4], want: [[f32; 4]; 4], what: &str) {
    for c in 0..4 {
        for r in 0..4 {
            assert!(
                (got[c][r] - want[c][r]).abs() < 1e-5,
                "{what}: transform[{c}][{r}] = {} want {} (column-major)\n got {got:?}\nwant {want:?}",
                got[c][r],
                want[c][r]
            );
        }
    }
}

fn check_standard_scene(s: &GltfScene) {
    assert_eq!(s.meshes.len(), 3);
    assert_eq!(s.materials.len(), 2);
    assert_eq!(s.images.len(), 2);

    // --- deck: triangle with normals, uvs and u16 indices -------------------
    let deck = mesh(s, "deck");
    assert_eq!(deck.positions, fx::TRI_POS.to_vec());
    assert_eq!(deck.normals, fx::TRI_NRM.to_vec());
    assert_eq!(deck.uvs, fx::TRI_UV.to_vec());
    assert_eq!(deck.indices, vec![0u32, 2, 1]);
    assert_eq!(deck.material, Some(0));
    assert_mat(
        deck.transform,
        [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [1.0, 2.0, 3.0, 1.0],
        ],
        "deck",
    );
    // Node extras take precedence over mesh extras (mesh says layer 5).
    assert_eq!(deck.extras.collision_layer, Some(2));
    assert_eq!(deck.extras.switch.as_deref(), Some("right_orbit_opto"));
    assert_eq!(deck.extras.lamp, None);
    assert!(deck.extras.collider);

    // --- insert: child of deck, T(10,0,0) * S(2), u32 indices ---------------
    let insert = mesh(s, "insert");
    assert_eq!(insert.positions, fx::QUAD_POS.to_vec());
    assert_eq!(insert.indices, fx::QUAD_IDX.to_vec());
    assert_eq!(insert.material, Some(1));
    assert_mat(
        insert.transform,
        [
            [2.0, 0.0, 0.0, 0.0],
            [0.0, 2.0, 0.0, 0.0],
            [0.0, 0.0, 2.0, 0.0],
            [11.0, 2.0, 3.0, 1.0],
        ],
        "insert (world = parent * local)",
    );
    assert_eq!(insert.extras.collision_layer, Some(0));
    assert_eq!(insert.extras.lamp.as_deref(), Some("overdrive_ready"));
    assert_eq!(insert.extras.switch, None);
    assert!(!insert.extras.collider, "collider defaults to false");

    // --- post: rotation 90 deg about +Z, extras only on the mesh ------------
    let post = mesh(s, "post");
    assert_eq!(post.positions, fx::QUAD_POS.to_vec());
    assert_eq!(post.indices, fx::QUAD_IDX.to_vec());
    assert_eq!(post.material, None);
    assert_mat(
        post.transform,
        [
            [0.0, 1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 5.0, 0.0, 1.0],
        ],
        "post",
    );
    assert_eq!(post.extras.collision_layer, Some(7), "mesh extras apply when the node has none");
    assert!(post.extras.collider);
    assert_eq!(post.extras.switch, None);
    assert_eq!(post.extras.lamp, None);

    // --- materials -----------------------------------------------------------
    let wood = &s.materials[0];
    assert_eq!(wood.name, "wood");
    assert_eq!(wood.base_color, [0.8, 0.6, 0.4, 0.5]);
    assert_eq!(wood.metallic, 0.25);
    assert_eq!(wood.roughness, 0.75);
    assert_eq!(wood.emissive, [0.1, 0.2, 0.3]);
    // texture 0 -> image 1, texture 1 -> image 0: indices are into `images`.
    assert_eq!(wood.base_color_image, Some(1));
    assert_eq!(wood.emissive_image, Some(0));
    assert_eq!(wood.metallic_roughness_image, None);
    assert_eq!(wood.normal_image, None);

    let glow = &s.materials[1];
    assert_eq!(glow.name, "glow");
    assert_eq!(glow.base_color, [1.0, 1.0, 1.0, 1.0], "glTF default");
    assert_eq!(glow.metallic, 1.0, "glTF default");
    assert_eq!(glow.roughness, 1.0, "glTF default");
    assert_eq!(glow.emissive, [0.0, 0.0, 0.0], "glTF default");
    assert_eq!(glow.base_color_image, None);
    assert_eq!(glow.metallic_roughness_image, None);
    assert_eq!(glow.normal_image, None);
    assert_eq!(glow.emissive_image, None);
}

// ============================ criterion 5 ===================================

#[test]
fn glb_with_extras_loads_meshes_transforms_and_materials() {
    let v = vfs(&[("playfield.glb", fx::playfield_glb())]);
    let s = load_gltf(&v, "playfield.glb").unwrap_or_else(|e| panic!("{e}"));
    check_standard_scene(&s);
}

#[test]
fn glb_loads_from_a_directory_vfs_and_a_subfolder() {
    let outer = TempDir::new("gltf");
    let mut files = BTreeMap::new();
    files.insert("models/deep/playfield.glb".to_owned(), fx::playfield_glb());
    fx::write_tree(outer.path(), &files);
    let v = Vfs::open(outer.path()).unwrap();
    check_standard_scene(&load_gltf(&v, "models/deep/playfield.glb").unwrap());
}

#[test]
fn gltf_with_external_buffer_resolves_relative_to_the_gltf_through_the_vfs() {
    let json = scene_json(BufferRef::Uri("pf.bin"), DECK_EXTRAS, INSERT_EXTRAS);
    let v = vfs(&[
        ("models/pf.gltf", json.clone().into_bytes()),
        ("models/pf.bin", scene_bin().0),
    ]);
    check_standard_scene(&load_gltf(&v, "models/pf.gltf").unwrap_or_else(|e| panic!("{e}")));

    // At the bundle root too.
    let v = vfs(&[("pf.gltf", json.into_bytes()), ("pf.bin", scene_bin().0)]);
    check_standard_scene(&load_gltf(&v, "pf.gltf").unwrap_or_else(|e| panic!("{e}")));
}

#[test]
fn gltf_with_data_uri_buffer() {
    let uri = format!(
        "data:application/octet-stream;base64,{}",
        fx::base64(&scene_bin().0)
    );
    let json = scene_json(BufferRef::Uri(&uri), DECK_EXTRAS, INSERT_EXTRAS);
    let v = vfs(&[("pf.gltf", json.into_bytes())]);
    check_standard_scene(&load_gltf(&v, "pf.gltf").unwrap_or_else(|e| panic!("{e}")));
}

fn external_image_json(uri: &str) -> String {
    // One triangle, one material whose base colour texture is an external image.
    format!(
        r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],
"nodes":[{{"name":"n","mesh":0}}],
"meshes":[{{"name":"n","primitives":[{{"attributes":{{"POSITION":0}},"material":0}}]}}],
"materials":[{{"name":"m","pbrMetallicRoughness":{{"baseColorTexture":{{"index":0}}}}}}],
"textures":[{{"source":0}}],
"images":[{{"uri":"{uri}"}}],
"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3","min":[0.0,0.0,0.0],"max":[0.25,0.5,0.125]}}],
"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}}],
"buffers":[{{"byteLength":36,"uri":"tri.bin"}}]}}"#
    )
}

#[test]
fn gltf_external_image_resolves_through_the_vfs() {
    let tri = scene_bin().0[..36].to_vec();
    let v = vfs(&[
        ("m/scene.gltf", external_image_json("tex/wood.png").into_bytes()),
        ("m/tri.bin", tri.clone()),
        ("m/tex/wood.png", fx::png_1x1([1, 2, 3, 255])),
    ]);
    let s = load_gltf(&v, "m/scene.gltf").unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(s.images.len(), 1);
    assert_eq!(s.materials[0].base_color_image, Some(0));
    assert_eq!(s.meshes[0].positions, fx::TRI_POS.to_vec());

    // Missing image: an error, never a silent fallback or a panic.
    let v = vfs(&[
        ("m/scene.gltf", external_image_json("tex/wood.png").into_bytes()),
        ("m/tri.bin", tri),
    ]);
    assert!(load_gltf(&v, "m/scene.gltf").is_err());
}

#[test]
fn missing_gltf_and_missing_external_buffer_are_errors() {
    let v = vfs(&[("other.txt", b"x".to_vec())]);
    assert!(matches!(load_gltf(&v, "playfield.glb"), Err(AssetError::NotFound(p)) if p.contains("playfield.glb")));

    let json = scene_json(BufferRef::Uri("gone.bin"), DECK_EXTRAS, INSERT_EXTRAS);
    let v = vfs(&[("pf.gltf", json.into_bytes())]);
    let e = load_gltf(&v, "pf.gltf").err().expect("missing buffer must fail");
    assert!(
        matches!(e, AssetError::NotFound(_) | AssetError::Gltf(_)),
        "{e:?}"
    );
    assert!(e.to_string().contains("gone.bin"), "error must name the missing buffer: {e}");
}

/// External URIs are resolved through the VFS only - never the real filesystem.
#[test]
fn external_uris_never_reach_the_real_filesystem() {
    let outer = TempDir::new("gltf_escape");
    // A perfectly valid buffer on the real filesystem, outside any bundle.
    let real = outer.path().join("outside.bin");
    std::fs::write(&real, scene_bin().0).unwrap();
    let abs = real.to_str().unwrap().replace('\\', "/");
    let file_url = if abs.starts_with('/') {
        format!("file://{abs}")
    } else {
        format!("file:///{abs}")
    };

    // 1. In-memory zip bundle: absolute path, file: URL, traversal.
    for uri in [abs.as_str(), file_url.as_str(), "../outside.bin", "../../outside.bin", "a/../../outside.bin"] {
        let json = scene_json(BufferRef::Uri(uri), DECK_EXTRAS, INSERT_EXTRAS);
        let v = vfs(&[("models/pf.gltf", json.into_bytes())]);
        let r = load_gltf(&v, "models/pf.gltf");
        assert!(r.is_err(), "buffer uri {uri:?} was resolved outside the VFS");
    }

    // 2. Directory bundle sitting next to the real file: `..` must not reach it.
    let root = outer.path().join("bundle");
    for uri in [abs.as_str(), file_url.as_str(), "../outside.bin", "../../outside.bin"] {
        let mut files = BTreeMap::new();
        files.insert(
            "pf.gltf".to_owned(),
            scene_json(BufferRef::Uri(uri), DECK_EXTRAS, INSERT_EXTRAS).into_bytes(),
        );
        files.insert(
            "models/pf.gltf".to_owned(),
            scene_json(BufferRef::Uri(uri), DECK_EXTRAS, INSERT_EXTRAS).into_bytes(),
        );
        fx::write_tree(&root, &files);
        let v = Vfs::open(&root).unwrap();
        assert!(load_gltf(&v, "pf.gltf").is_err(), "root gltf, uri {uri:?}");
        assert!(load_gltf(&v, "models/pf.gltf").is_err(), "nested gltf, uri {uri:?}");
    }

    // 3. Remote schemes are not fetched.
    for uri in ["http://example.invalid/pf.bin", "https://example.invalid/pf.bin"] {
        let json = scene_json(BufferRef::Uri(uri), DECK_EXTRAS, INSERT_EXTRAS);
        let v = vfs(&[("pf.gltf", json.into_bytes())]);
        assert!(load_gltf(&v, "pf.gltf").is_err(), "{uri}");
    }
}

#[test]
fn malformed_gltf_is_a_gltf_error() {
    let good = fx::playfield_glb();
    let mut bad_magic = good.clone();
    bad_magic[0] = b'X';
    let short_buffer = fx::glb(
        &scene_json(BufferRef::Embedded, DECK_EXTRAS, INSERT_EXTRAS),
        &scene_bin().0[..100],
    );
    let out_of_range = fx::glb(
        &scene_json(BufferRef::Embedded, DECK_EXTRAS, INSERT_EXTRAS).replace(r#""indices":3"#, r#""indices":99"#),
        &scene_bin().0,
    );
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("empty", Vec::new()),
        ("text", b"not a gltf".to_vec()),
        ("bad magic", bad_magic),
        ("truncated", good[..good.len() / 2].to_vec()),
        ("header only", good[..12].to_vec()),
        ("json only junk", fx::glb("{", &[])),
        ("no asset", fx::glb("{}", &[])),
        ("buffer shorter than views", short_buffer),
        ("accessor index out of range", out_of_range),
    ];
    for (what, bytes) in cases {
        let v = vfs(&[("x.glb", bytes)]);
        match load_gltf(&v, "x.glb") {
            Err(AssetError::Gltf(m)) => assert!(!m.is_empty()),
            other => panic!("{what}: expected AssetError::Gltf, got {:?}", other.map(|s| s.meshes.len())),
        }
    }
}

// ============================ extras ========================================

#[test]
fn every_collision_layer_0_to_7_is_read() {
    for layer in 0u8..=7 {
        let glb = fx::glb_with_extras(&format!(r#"{{"collision_layer":{layer}}}"#), "{}");
        let v = vfs(&[("x.glb", glb)]);
        let s = load_gltf(&v, "x.glb").unwrap();
        assert_eq!(mesh(&s, "deck").extras.collision_layer, Some(layer));
    }
}

#[test]
fn out_of_range_or_mistyped_extras_never_become_a_layer() {
    // Layers are 0..=7; anything else must be an error or be left unset, never
    // silently wrapped or clamped into a valid layer.
    for bad in ["8", "255", "256", "-1", "3.5", "\"2\"", "true", "[1]", "{}", "1e30", "null"] {
        let glb = fx::glb_with_extras(&format!(r#"{{"collision_layer":{bad}}}"#), "{}");
        let v = vfs(&[("x.glb", glb)]);
        if let Ok(s) = load_gltf(&v, "x.glb") {
            let got = mesh(&s, "deck").extras.collision_layer;
            // Falling back to the mesh's own extras (layer 5) is acceptable.
            assert!(
                got.is_none() || got == Some(5),
                "collision_layer {bad} became {got:?}"
            );
        }
    }
    // Mistyped switch / lamp / collider: no panic.
    for extras in [
        r#"{"switch":5}"#,
        r#"{"lamp":["a"]}"#,
        r#"{"collider":"yes"}"#,
        r#"{"collider":1}"#,
        r#"{"switch":null,"lamp":null,"collider":null}"#,
        r#"[1,2,3]"#,
        r#""just a string""#,
        r#"42"#,
        r#"null"#,
    ] {
        let v = vfs(&[("x.glb", fx::glb_with_extras(extras, extras))]);
        let _ = load_gltf(&v, "x.glb");
    }
}

#[test]
fn unknown_extras_keys_are_ignored() {
    let v = vfs(&[(
        "x.glb",
        fx::glb_with_extras(
            r#"{"foo":1,"bar":{"baz":[true,null]},"Collision_Layer":3,"switches":"x"}"#,
            r#"{"lamp":"l","zzz":"y"}"#,
        ),
    )]);
    let s = load_gltf(&v, "x.glb").unwrap();
    let deck = mesh(&s, "deck");
    assert_eq!(deck.extras.switch, None);
    assert_eq!(deck.extras.lamp, None);
    assert_eq!(mesh(&s, "insert").extras.lamp.as_deref(), Some("l"));
}

// ============================ bindings ======================================

fn table_ids() -> pin_assets::TableIds {
    load_table(&fx::vfs_from(&fx::bundle())).unwrap_or_else(|e| panic!("{e}")).ids
}

#[test]
fn bindings_to_declared_names_are_clean() {
    let ids = table_ids();
    let v = vfs(&[("playfield.glb", fx::playfield_glb())]);
    let s = load_gltf(&v, "playfield.glb").unwrap();
    assert_eq!(validate_gltf_bindings(&s, &ids), vec![]);
    // A scene with no extras at all is clean too.
    let v = vfs(&[("b.glb", fx::backbox_glb())]);
    assert_eq!(validate_gltf_bindings(&load_gltf(&v, "b.glb").unwrap(), &ids), vec![]);
}

#[test]
fn bindings_to_unknown_names_are_reported_with_suggestions() {
    let ids = table_ids();
    let v = vfs(&[(
        "playfield.glb",
        fx::glb_with_extras(
            r#"{"switch":"right_orbit_optx"}"#,
            r#"{"lamp":"totally_unrelated_lamp_name"}"#,
        ),
    )]);
    let s = load_gltf(&v, "playfield.glb").unwrap();
    let errs = validate_gltf_bindings(&s, &ids);
    assert_eq!(errs.len(), 2, "one error per unresolved binding: {errs:?}");
    let sw = errs
        .iter()
        .find(|e| e.message.contains("right_orbit_optx"))
        .unwrap_or_else(|| panic!("no error names the switch: {errs:?}"));
    assert!(
        sw.message.contains("did you mean") && sw.message.contains("\"right_orbit_opto\""),
        "{sw}"
    );
    let lamp = errs
        .iter()
        .find(|e| e.message.contains("totally_unrelated_lamp_name"))
        .unwrap_or_else(|| panic!("no error names the lamp: {errs:?}"));
    assert!(!lamp.message.contains("did you mean"), "{lamp}");
    for e in &errs {
        assert_eq!(e.to_string(), format!("{}: {}: {}", e.file, e.path, e.message));
        assert!(!e.path.is_empty(), "the error must say where the binding is: {e}");
    }
}

#[test]
fn bindings_do_not_resolve_across_kinds() {
    let ids = table_ids();
    // A lamp name used as a switch and a switch name used as a lamp.
    let v = vfs(&[(
        "playfield.glb",
        fx::glb_with_extras(r#"{"switch":"overdrive_ready"}"#, r#"{"lamp":"right_orbit_opto"}"#),
    )]);
    let s = load_gltf(&v, "playfield.glb").unwrap();
    assert_eq!(validate_gltf_bindings(&s, &ids).len(), 2);
}
