//! Regressions for the CodeRabbit review of PR 4 (pin_assets hardening):
//! `data:` URIs charged before decoding, percent-encoded binary payloads,
//! buffer views bounded by their own buffer, per-instance name/extras bytes
//! budgeted, and manifests read to a bounded prefix.

#[path = "fixtures/support.rs"]
mod support;

use pin_assets::validate::MAX_MANIFEST_BYTES;
use pin_assets::{AssetError, GltfLimits, Vfs, ZipLimits, load_gltf, load_gltf_with_limits};
use serde_json::json;
use support::*;

fn stored(files: &[(String, Vec<u8>)]) -> Vec<u8> {
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    stored_zip(&refs)
}

fn open(bytes: Vec<u8>) -> Vfs {
    Vfs::from_zip_bytes(bytes, ZipLimits::default()).unwrap()
}

fn triangle() -> Vec<u8> {
    f32_bytes(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0])
}

fn gltf(nodes: serde_json::Value, buffers: serde_json::Value, view: serde_json::Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "asset": {"version": "2.0"},
        "nodes": nodes,
        "meshes": [{"primitives": [{"attributes": {"POSITION": 0}}]}],
        "accessors": [{"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
                        "min": [0.0, 0.0, 0.0], "max": [1.0, 1.0, 0.0]}],
        "bufferViews": [view],
        "buffers": buffers
    }))
    .unwrap()
}

fn one_node(buffer: serde_json::Value) -> Vec<u8> {
    gltf(
        json!([{"mesh": 0}]),
        json!([buffer]),
        json!({"buffer": 0, "byteOffset": 0, "byteLength": 36}),
    )
}

fn base64(data: &[u8]) -> String {
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

/// Security (budget bypass): a buffer declaring 36 bytes whose `data:` URI
/// decodes to 1 MiB is refused by the buffer budget from the URI's decoded
/// length, before the payload is decoded.
#[test]
fn data_uri_over_the_buffer_budget_is_refused_before_decoding() {
    let mut payload = triangle();
    payload.resize(1 << 20, 0);
    let uri = format!("data:application/octet-stream;base64,{}", base64(&payload));
    let vfs = open(stored_zip(&[(
        "t.gltf",
        &one_node(json!({"byteLength": 36, "uri": uri})),
    )]));
    let limits = GltfLimits {
        max_buffer_bytes: 36,
        ..GltfLimits::default()
    };
    let r = load_gltf_with_limits(&vfs, "t.gltf", &limits);
    assert!(
        matches!(&r, Err(AssetError::Gltf(m)) if m.contains("buffers hold 1048576 bytes") && m.contains("budget")),
        "{r:?}"
    );
    let roomy = GltfLimits {
        max_buffer_bytes: 1 << 20,
        ..GltfLimits::default()
    };
    assert!(load_gltf_with_limits(&vfs, "t.gltf", &roomy).is_ok());
}

/// A non-base64 `data:` URI carries octets: percent-encoded bytes that are
/// not UTF-8 (0x80.., as in any float buffer or a PNG signature) decode.
#[test]
fn percent_encoded_binary_data_uris_load() {
    // 1.0f32 is 00 00 80 3F: 0x80 alone is not UTF-8.
    let bytes = triangle();
    assert!(String::from_utf8(bytes.clone()).is_err());
    let encoded: String = bytes.iter().map(|b| format!("%{b:02X}")).collect();
    let uri = format!("data:application/octet-stream,{encoded}");
    let vfs = open(stored_zip(&[(
        "t.gltf",
        &one_node(json!({"byteLength": 36, "uri": uri})),
    )]));
    let scene = load_gltf(&vfs, "t.gltf").unwrap();
    assert_eq!(scene.meshes[0].positions[1], [1.0, 0.0, 0.0]);
}

/// Security (out-of-bounds read): two buffers naming one file share its
/// longest prefix; a view of the shorter buffer may not read past that
/// buffer's own `byteLength`.
#[test]
fn views_are_bounded_by_their_own_buffer() {
    let mut file = triangle();
    file.extend(triangle());
    let buffers = json!([{"byteLength": 36, "uri": "b.bin"}, {"byteLength": 72, "uri": "b.bin"}]);
    let at = |buffer: u32| {
        gltf(
            json!([{"mesh": 0}]),
            buffers.clone(),
            json!({"buffer": buffer, "byteOffset": 36, "byteLength": 36}),
        )
    };
    let vfs = open(stored_zip(&[
        ("short.gltf", &at(0)),
        ("long.gltf", &at(1)),
        ("b.bin", &file),
    ]));
    let r = load_gltf(&vfs, "short.gltf");
    assert!(
        matches!(&r, Err(AssetError::Gltf(m)) if m.contains("runs past its buffer")),
        "{r:?}"
    );
    assert!(load_gltf(&vfs, "long.gltf").is_ok());
}

/// Security (memory amplification): one mesh with a long name and switch
/// extra placed by many nodes is charged per placed primitive against the
/// JSON byte cap, so the copies cannot multiply past it.
#[test]
fn mesh_instance_names_and_extras_are_budgeted() {
    let long = "s".repeat(1000);
    let bundle = |instances: usize| {
        let nodes: Vec<_> = (0..instances).map(|_| json!({"mesh": 0})).collect();
        let mut v: serde_json::Value = serde_json::from_slice(&gltf(
            json!(nodes),
            json!([{"byteLength": 36, "uri": "b.bin"}]),
            json!({"buffer": 0, "byteOffset": 0, "byteLength": 36}),
        ))
        .unwrap();
        v["meshes"][0]["name"] = json!(long);
        v["meshes"][0]["extras"] = json!({"switch": long});
        open(stored_zip(&[
            ("t.gltf", &serde_json::to_vec(&v).unwrap()),
            ("b.bin", &triangle()),
        ]))
    };
    let limits = GltfLimits {
        max_json_bytes: 30_000,
        ..GltfLimits::default()
    };
    // 10 placements copy ~20 kB: fine. 100 copy ~200 kB from a ~5 kB file.
    let scene = load_gltf_with_limits(&bundle(10), "t.gltf", &limits).unwrap();
    assert_eq!(scene.meshes.len(), 10);
    assert_eq!(
        scene.meshes[9].extras.switch.as_deref(),
        Some(long.as_str())
    );
    let r = load_gltf_with_limits(&bundle(100), "t.gltf", &limits);
    assert!(
        matches!(&r, Err(AssetError::Gltf(m)) if m.contains("mesh name and extras bytes") && m.contains("budget")),
        "{r:?}"
    );
}

/// Security (allocation before the size check): an oversized manifest is
/// refused with its size, read at most one byte past the cap.
#[test]
fn oversized_manifests_are_refused_with_their_size() {
    let mut big = table_text().into_bytes();
    big.resize(MAX_MANIFEST_BYTES + 1, b'\n');
    let files = with_file(&bundle_files(), "table.toml", &big);
    let dir = temp_dir("pr4_manifest");
    write_dir(&dir, &files);
    for vfs in [Vfs::open(&dir).unwrap(), open(stored(&files))] {
        match pin_assets::load_table(&vfs) {
            Err(AssetError::Invalid(errors)) => assert!(
                errors.iter().any(|e| e.file == "table.toml"
                    && e.message
                        == format!(
                            "manifest is {} bytes (max {MAX_MANIFEST_BYTES})",
                            MAX_MANIFEST_BYTES + 1
                        )),
                "{errors:?}"
            ),
            other => panic!("{other:?}"),
        }
    }
    // Exactly at the cap is not a size error.
    let mut at_cap = table_text().into_bytes();
    at_cap.resize(MAX_MANIFEST_BYTES, b'\n');
    let vfs = open(stored(&with_file(&bundle_files(), "table.toml", &at_cap)));
    if let Err(AssetError::Invalid(errors)) = pin_assets::load_table(&vfs) {
        assert!(
            !errors.iter().any(|e| e.message.starts_with("manifest is")),
            "{errors:?}"
        );
    }
    let _ = std::fs::remove_dir_all(&dir);
}
