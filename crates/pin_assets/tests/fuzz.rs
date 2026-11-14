//! Parsers survive bounded random input without panicking (gate 5, criterion 4).

#[path = "fixtures/support.rs"]
mod support;

use pin_assets::{Vfs, ZipLimits, load_gltf, load_table};
use proptest::prelude::*;
use support::*;

fn config() -> ProptestConfig {
    ProptestConfig {
        cases: 256,
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

/// Byte vectors seeded from a valid input with random overwrites.
fn mutated(base: Vec<u8>) -> impl Strategy<Value = Vec<u8>> {
    let len = base.len();
    proptest::collection::vec((0..len, any::<u8>()), 1..16).prop_map(move |edits| {
        let mut v = base.clone();
        for (i, b) in edits {
            v[i] = b;
        }
        v
    })
}

proptest! {
    #![proptest_config(config())]

    #[test]
    fn zip_random_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        if let Ok(vfs) = Vfs::from_zip_bytes(bytes, ZipLimits::default()) {
            for name in vfs.list() {
                let _ = vfs.read(&name);
            }
        }
    }

    #[test]
    fn zip_mutated_archives(bytes in mutated(deflated_zip(&[
        ("table.toml", b"schema_version = 1"),
        ("a/b.txt", &[7u8; 300]),
    ]))) {
        if let Ok(vfs) = Vfs::from_zip_bytes(bytes, ZipLimits::default()) {
            for name in vfs.list() {
                let _ = vfs.read(&name);
            }
        }
    }

    #[test]
    fn toml_random_text(table in "[ -~\n]{0,400}", backglass in proptest::collection::vec(any::<u8>(), 0..200)) {
        let files = with_file(&bundle_files(), "table.toml", table.as_bytes());
        let files = with_file(&files, "backglass.toml", &backglass);
        let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
        let vfs = Vfs::from_zip_bytes(stored_zip(&refs), ZipLimits::default()).unwrap();
        let _ = load_table(&vfs);
    }

    #[test]
    fn toml_mutated_manifest(bytes in mutated(table_text().into_bytes())) {
        let files = with_file(&bundle_files(), "table.toml", &bytes);
        let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, d)| (n.as_str(), d.as_slice())).collect();
        let vfs = Vfs::from_zip_bytes(stored_zip(&refs), ZipLimits::default()).unwrap();
        let _ = load_table(&vfs);
    }

    #[test]
    fn gltf_random_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..1024), glb_header in any::<bool>()) {
        let mut data = if glb_header { b"glTF".to_vec() } else { Vec::new() };
        data.extend(bytes);
        let vfs = Vfs::from_zip_bytes(stored_zip(&[("x.glb", &data)]), ZipLimits::default()).unwrap();
        let _ = load_gltf(&vfs, "x.glb");
    }

    #[test]
    fn gltf_mutated_glb(bytes in mutated(playfield_glb())) {
        let vfs = Vfs::from_zip_bytes(stored_zip(&[("x.glb", &bytes)]), ZipLimits::default()).unwrap();
        let _ = load_gltf(&vfs, "x.glb");
    }

    #[test]
    fn gltf_random_extras(key in prop::sample::select(vec!["collision_layer", "switch", "lamp", "collider", "other"]),
                          value in prop_oneof![
                              any::<i64>().prop_map(serde_json::Value::from),
                              any::<f64>().prop_map(|f| serde_json::json!(f)),
                              any::<bool>().prop_map(serde_json::Value::from),
                              "[a-z_]{0,12}".prop_map(serde_json::Value::from),
                              Just(serde_json::Value::Null),
                              Just(serde_json::json!([1, {"a": 2}])),
                          ]) {
        let glb = playfield_glb();
        // Rewrite node 1's extras through the JSON chunk.
        let json_len = u32::from_le_bytes([glb[12], glb[13], glb[14], glb[15]]) as usize;
        let mut doc: serde_json::Value = serde_json::from_slice(&glb[20..20 + json_len]).unwrap();
        doc["nodes"][1]["extras"] = serde_json::json!({ key: value });
        let bin_start = 20 + json_len + 8;
        let rebuilt = glb_with(&doc, &glb[bin_start..]);
        let vfs = Vfs::from_zip_bytes(stored_zip(&[("x.glb", &rebuilt)]), ZipLimits::default()).unwrap();
        let _ = load_gltf(&vfs, "x.glb");
    }
}

fn glb_with(doc: &serde_json::Value, bin: &[u8]) -> Vec<u8> {
    glb(doc, bin)
}
