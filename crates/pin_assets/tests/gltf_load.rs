//! glTF loading through the VFS (criterion 5).

#[path = "fixtures/support.rs"]
mod support;

use pin_assets::{
    AssetError, IdKind, NodeExtras, Vfs, ZipLimits, load_gltf, load_table, validate_gltf_bindings,
    validate_gltf_bindings_in,
};
use serde_json::json;
use support::*;

fn vfs_of(files: &[(&str, &[u8])]) -> Vfs {
    Vfs::from_zip_bytes(stored_zip(files), ZipLimits::default()).unwrap()
}

#[test]
fn generated_glb_with_two_nodes() {
    let glb = playfield_glb();
    let vfs = vfs_of(&[("models/playfield.glb", &glb)]);
    let scene = load_gltf(&vfs, "models/playfield.glb").unwrap();
    assert_eq!(scene.meshes.len(), 2);
    let (ramp, opto) = (&scene.meshes[0], &scene.meshes[1]);
    assert_eq!(ramp.name, "ramp");
    assert_eq!(opto.name, "orbit_opto");
    for m in [ramp, opto] {
        assert_eq!(
            m.positions,
            vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]
        );
        assert_eq!(m.indices, vec![0, 1, 2]);
        assert_eq!(m.normals, vec![[0.0, 0.0, 1.0]; 3]);
        assert!(m.uvs.is_empty());
        assert_eq!(m.material, Some(0));
    }
    // Parent translation; child = parent * scale(2).
    assert_eq!(ramp.transform[3], [1.0, 2.0, 3.0, 1.0]);
    assert_eq!(ramp.transform[0], [1.0, 0.0, 0.0, 0.0]);
    assert_eq!(opto.transform[0], [2.0, 0.0, 0.0, 0.0]);
    assert_eq!(opto.transform[2], [0.0, 0.0, 2.0, 0.0]);
    assert_eq!(opto.transform[3], [1.0, 2.0, 3.0, 1.0]);
    // Node extras override mesh extras key by key.
    assert_eq!(
        ramp.extras,
        NodeExtras {
            collision_layer: Some(2),
            switch: Some("mesh_level".into()),
            lamp: Some("overdrive_ready".into()),
            collider: true,
        }
    );
    assert_eq!(
        opto.extras,
        NodeExtras {
            collision_layer: Some(5),
            switch: Some("right_orbit_opto".into()),
            lamp: None,
            collider: false,
        }
    );
    let mat = &scene.materials[0];
    assert_eq!(mat.name, "chrome");
    assert_eq!(mat.base_color, [0.5, 0.25, 1.0, 1.0]);
    assert_eq!(mat.metallic, 0.75);
    assert_eq!(mat.roughness, 0.125);
    assert_eq!(mat.emissive, [0.1, 0.2, 0.3]);
    assert_eq!(mat.base_color_image, Some(0));
    assert_eq!(mat.metallic_roughness_image, None);
    assert_eq!(mat.normal_image, None);
    assert_eq!(mat.emissive_image, None);
    let img = &scene.images[0];
    assert_eq!((img.width, img.height), (2, 1));
    assert_eq!(img.rgba8, vec![255, 0, 0, 255, 0, 255, 0, 255]);
}

#[test]
fn bindings_validate_against_the_table() {
    let vfs = Vfs::from_zip_bytes(zip_of(&bundle_files()), ZipLimits::default()).unwrap();
    let table = load_table(&vfs).unwrap();
    let scene = load_gltf(&vfs, "playfield.glb").unwrap();
    let errs = validate_gltf_bindings(&scene, &table.ids);
    // "mesh_level" is not a switch; the lamp and the other switch exist.
    assert_eq!(errs.len(), 1, "{errs:?}");
    assert_eq!(errs[0].path, "meshes[0].extras.switch");
    assert!(errs[0].message.contains("unknown switch \"mesh_level\""));
    let mut scene = scene;
    scene.meshes[0].extras.switch = Some("right_orbit_opt".into());
    scene.meshes[1].extras.lamp = Some("nope".into());
    let errs = validate_gltf_bindings_in(&scene, &table.ids, "playfield.glb");
    assert_eq!(errs.len(), 2);
    assert_eq!(errs[0].file, "playfield.glb");
    assert!(
        errs[0]
            .message
            .contains("did you mean \"right_orbit_opto\"")
    );
    assert_eq!(errs[1].path, "meshes[1].extras.lamp");
    assert!(table.ids.entries(IdKind::Lamp).len() >= 2);
}

fn tri_gltf(buffer_uri: &str, extra: serde_json::Value) -> serde_json::Value {
    let mut j = json!({
        "asset": {"version": "2.0"},
        "nodes": [{"mesh": 0, "matrix": [1,0,0,0, 0,1,0,0, 0,0,1,0, 5,6,7,1]}],
        "meshes": [{"name": "m", "primitives": [{"attributes": {"POSITION": 0, "NORMAL": 1, "TEXCOORD_0": 2}}]}],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]},
            {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"},
            {"bufferView": 2, "componentType": 5121, "normalized": true, "count": 3, "type": "VEC2"}
        ],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": 36},
            {"buffer": 0, "byteOffset": 36, "byteLength": 36},
            {"buffer": 0, "byteOffset": 72, "byteLength": 6}
        ],
        "buffers": [{"byteLength": 78, "uri": buffer_uri}]
    });
    if let (Some(dst), Some(src)) = (j.as_object_mut(), extra.as_object()) {
        for (k, v) in src {
            dst.insert(k.clone(), v.clone());
        }
    }
    j
}

fn tri_bin() -> Vec<u8> {
    let mut b = f32_bytes(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]);
    b.extend(f32_bytes(&[0.0, 1.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 0.0]));
    b.extend([0, 255, 255, 0, 0, 0]);
    b
}

fn b64(data: &[u8]) -> String {
    const A: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut s = String::new();
    for c in data.chunks(3) {
        let n = (u32::from(c[0]) << 16)
            | (u32::from(*c.get(1).unwrap_or(&0)) << 8)
            | u32::from(*c.get(2).unwrap_or(&0));
        for i in 0..4 {
            if i <= c.len() {
                s.push(A[(n >> (18 - 6 * i) & 63) as usize] as char);
            } else {
                s.push('=');
            }
        }
    }
    s
}

#[test]
fn gltf_with_external_and_data_buffers() {
    let bin = tri_bin();
    let gltf = serde_json::to_vec(&tri_gltf("bufs/tri%20data.bin", json!({}))).unwrap();
    let vfs = vfs_of(&[("m/tri.gltf", &gltf), ("m/bufs/tri data.bin", &bin)]);
    let scene = load_gltf(&vfs, "m/tri.gltf").unwrap();
    let m = &scene.meshes[0];
    assert_eq!(m.name, "m");
    assert_eq!(m.normals, vec![[0.0, 1.0, 0.0]; 3]);
    assert_eq!(m.uvs, vec![[0.0, 1.0], [1.0, 0.0], [0.0, 0.0]]);
    assert_eq!(m.indices, vec![0, 1, 2]);
    assert_eq!(m.transform[3], [5.0, 6.0, 7.0, 1.0]);
    assert_eq!(m.material, None);

    let uri = format!("data:application/octet-stream;base64,{}", b64(&bin));
    let gltf = serde_json::to_vec(&tri_gltf(&uri, json!({}))).unwrap();
    let vfs = vfs_of(&[("tri.gltf", &gltf)]);
    assert_eq!(
        load_gltf(&vfs, "tri.gltf").unwrap().meshes[0]
            .positions
            .len(),
        3
    );
}

#[test]
fn uris_cannot_leave_the_bundle() {
    let bin = tri_bin();
    let cases = [
        ("../x.bin", AssetError::PathTraversal("../x.bin".into())),
        ("/etc/x.bin", AssetError::AbsolutePath("/etc/x.bin".into())),
        ("C:/x.bin", AssetError::AbsolutePath("C:/x.bin".into())),
        ("missing.bin", AssetError::NotFound("missing.bin".into())),
    ];
    for (uri, want) in cases {
        let gltf = serde_json::to_vec(&tri_gltf(uri, json!({}))).unwrap();
        let vfs = vfs_of(&[("tri.gltf", &gltf), ("x.bin", &bin)]);
        assert_eq!(load_gltf(&vfs, "tri.gltf").err(), Some(want), "{uri}");
    }
    for uri in [
        "file:///etc/passwd",
        "http://example.com/x.bin",
        "data:nocomma",
        "data:;base64,***",
    ] {
        let gltf = serde_json::to_vec(&tri_gltf(uri, json!({}))).unwrap();
        let vfs = vfs_of(&[("tri.gltf", &gltf)]);
        assert!(
            matches!(load_gltf(&vfs, "tri.gltf"), Err(AssetError::Gltf(_))),
            "{uri}"
        );
    }
}

#[test]
fn hostile_documents_are_errors() {
    let bin = tri_bin();
    let run = |extra: serde_json::Value| {
        let gltf = serde_json::to_vec(&tri_gltf("b.bin", extra)).unwrap();
        let vfs = vfs_of(&[("t.gltf", &gltf), ("b.bin", &bin)]);
        load_gltf(&vfs, "t.gltf")
    };
    let bad = [
        // Buffer shorter than declared.
        json!({"buffers": [{"byteLength": 1000, "uri": "b.bin"}]}),
        // View past the buffer.
        json!({"bufferViews": [{"buffer": 0, "byteOffset": 70, "byteLength": 36},
            {"buffer": 0, "byteOffset": 36, "byteLength": 36},
            {"buffer": 0, "byteOffset": 72, "byteLength": 6}]}),
        // Accessor count past its view.
        json!({"accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 4, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]},
            {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"},
            {"bufferView": 2, "componentType": 5121, "normalized": true, "count": 3, "type": "VEC2"}]}),
        // Wrong component count.
        json!({"accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC2"},
            {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"},
            {"bufferView": 2, "componentType": 5121, "normalized": true, "count": 3, "type": "VEC2"}]}),
        // Non-normalized integer UVs.
        json!({"accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]},
            {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"},
            {"bufferView": 2, "componentType": 5121, "count": 3, "type": "VEC2"}]}),
        // Attribute counts differ.
        json!({"accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]},
            {"bufferView": 1, "componentType": 5126, "count": 2, "type": "VEC3"},
            {"bufferView": 2, "componentType": 5121, "normalized": true, "count": 3, "type": "VEC2"}]}),
        // Index out of range / not a multiple of 3 / float indices.
        json!({"meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 3}]}],
               "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]},
            {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"},
            {"bufferView": 2, "componentType": 5121, "normalized": true, "count": 3, "type": "VEC2"},
            {"bufferView": 2, "componentType": 5121, "count": 3, "type": "SCALAR"}]}),
        json!({"meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 3}]}],
               "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]},
            {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"},
            {"bufferView": 2, "componentType": 5121, "normalized": true, "count": 3, "type": "VEC2"},
            {"bufferView": 2, "componentType": 5121, "count": 4, "type": "SCALAR"}]}),
        json!({"meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "indices": 1}]}]}),
        // Missing POSITION.
        json!({"meshes": [{"primitives": [{"attributes": {"NORMAL": 1}}]}]}),
        // Cycle in the node graph.
        json!({"nodes": [{"mesh": 0, "children": [1]}, {"children": [0]}],
               "scenes": [{"nodes": [0]}], "scene": 0}),
        // Bad extras.
        json!({"nodes": [{"mesh": 0, "extras": {"collision_layer": 99}}]}),
        // Stride below element size.
        json!({"bufferViews": [{"buffer": 0, "byteOffset": 0, "byteLength": 36, "byteStride": 4},
            {"buffer": 0, "byteOffset": 36, "byteLength": 36},
            {"buffer": 0, "byteOffset": 72, "byteLength": 6}]}),
        // Dangling index (gltf validation).
        json!({"nodes": [{"mesh": 7}]}),
        // Undecodable image.
        json!({"images": [{"uri": "b.bin"}]}),
        // Non-finite matrix.
        json!({"nodes": [{"mesh": 0, "matrix": [1,0,0,0, 0,1,0,0, 0,0,1,0, 1e39,0,0,1]}]}),
    ];
    for (i, extra) in bad.into_iter().enumerate() {
        let r = run(extra.clone());
        assert!(
            matches!(r, Err(AssetError::Gltf(_))),
            "case {i}: {extra} -> {r:?}"
        );
    }
    // Non-triangle primitives are skipped; unindexed geometry gets generated indices.
    let r = run(json!({"meshes": [{"primitives": [{"attributes": {"POSITION": 0}, "mode": 1}]}]}))
        .unwrap();
    assert!(r.meshes.is_empty());
    // Without scenes, every root node is loaded.
    let r = run(json!({"nodes": [{"mesh": 0, "children": [1]}, {"mesh": 0}]})).unwrap();
    assert_eq!(r.meshes.len(), 2);
    assert_eq!(r.meshes[1].name, "m");
    // View-less, non-sparse accessors are rejected by validation.
    let r = run(json!({"accessors": [
        {"componentType": 5126, "count": 3, "type": "VEC3", "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]},
        {"bufferView": 1, "componentType": 5126, "count": 3, "type": "VEC3"},
        {"bufferView": 2, "componentType": 5121, "normalized": true, "count": 3, "type": "VEC2"}]}));
    assert!(matches!(r, Err(AssetError::Gltf(_))), "{r:?}");
}

#[test]
fn glb_container_errors() {
    let good = playfield_glb();
    let check = |bytes: Vec<u8>| {
        let vfs = vfs_of(&[("x.glb", &bytes)]);
        assert!(matches!(load_gltf(&vfs, "x.glb"), Err(AssetError::Gltf(_))));
    };
    check(b"glTF".to_vec());
    check(good[..good.len() - 10].to_vec()); // truncated: length mismatch
    let mut v = good.clone();
    v[4] = 1; // version 1
    check(v);
    let mut v = good.clone();
    v[16] = b'X'; // first chunk not JSON
    check(v);
    check(b"{not json".to_vec());
    // GLB without a BIN chunk but a buffer that needs one.
    let json = json!({"asset": {"version": "2.0"}, "buffers": [{"byteLength": 4}]});
    check(glb(&json, &[]));
    let vfs = vfs_of(&[("x.glb", &good)]);
    assert_eq!(
        load_gltf(&vfs, "nope.glb").err(),
        Some(AssetError::NotFound("nope.glb".into()))
    );
}
