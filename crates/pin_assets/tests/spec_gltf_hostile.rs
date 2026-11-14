//! Subphase 4.1 spec tests (Independent Tester): hostile but well-formed JSON
//! glTF documents. Brief: D25 (tables are untrusted downloads), verification
//! criterion 4 / gate step 5 (parsers never panic). Each case is a valid JSON
//! document with one dangling or contradictory reference; `load_gltf` must
//! return (normally `Err(AssetError::Gltf)`), never panic, hang or overflow the
//! stack.

#[path = "spec_fixtures/mod.rs"]
mod fx;

use std::collections::BTreeMap;

use fx::{BufferRef, DECK_EXTRAS, INSERT_EXTRAS, scene_bin, scene_json};
use pin_assets::{Vfs, ZipLimits, load_gltf};

fn base() -> String {
    scene_json(BufferRef::Embedded, DECK_EXTRAS, INSERT_EXTRAS)
}

/// Loads the standard scene with one textual edit; returns whether it loaded.
fn run(from: &str, to: &str) -> bool {
    let json = fx::sub(&base(), from, to);
    let mut m = BTreeMap::new();
    m.insert("x.glb".to_owned(), fx::glb(&json, &scene_bin().0));
    let v = Vfs::from_zip_bytes(fx::zip_of(&m), ZipLimits::default()).unwrap();
    match load_gltf(&v, "x.glb") {
        Ok(s) => {
            for mesh in &s.meshes {
                assert!(
                    mesh.indices
                        .iter()
                        .all(|i| (*i as usize) < mesh.positions.len()),
                    "loaded mesh {:?} has an index past its vertices",
                    mesh.name
                );
                if let Some(mat) = mesh.material {
                    assert!(mat < s.materials.len());
                }
            }
            true
        }
        Err(e) => {
            assert!(!e.to_string().is_empty());
            false
        }
    }
}

fn must_fail(from: &str, to: &str) {
    assert!(!run(from, to), "dangling reference {to:?} was accepted");
}

#[test]
fn position_accessor_out_of_range() {
    must_fail(r#""POSITION":0,"#, r#""POSITION":99,"#);
}

#[test]
fn normal_and_uv_accessors_out_of_range() {
    must_fail(r#""NORMAL":1"#, r#""NORMAL":77"#);
    must_fail(r#""TEXCOORD_0":2"#, r#""TEXCOORD_0":77"#);
}

#[test]
fn index_accessor_out_of_range() {
    must_fail(r#""indices":3"#, r#""indices":4000000000"#);
}

#[test]
fn image_buffer_view_out_of_range() {
    must_fail(
        r#"{"bufferView":6,"mimeType":"image/png"}"#,
        r#"{"bufferView":99,"mimeType":"image/png"}"#,
    );
}

#[test]
fn image_buffer_view_without_mime_type() {
    // Schema: mimeType is required when bufferView is used. Error or tolerate, never panic.
    run(
        r#"{"bufferView":6,"mimeType":"image/png"}"#,
        r#"{"bufferView":6}"#,
    );
}

#[test]
fn image_with_neither_uri_nor_buffer_view() {
    run(r#"{"bufferView":6,"mimeType":"image/png"}"#, r#"{}"#);
}

#[test]
fn image_that_is_not_an_image() {
    // bufferView 0 holds vertex floats, not a PNG.
    must_fail(
        r#"{"bufferView":6,"mimeType":"image/png"}"#,
        r#"{"bufferView":0,"mimeType":"image/png"}"#,
    );
}

#[test]
fn material_texture_and_source_out_of_range() {
    must_fail(r#""material":1}"#, r#""material":50}"#);
    must_fail(
        r#""baseColorTexture":{"index":0}"#,
        r#""baseColorTexture":{"index":9}"#,
    );
    must_fail(r#"[{"source":1},"#, r#"[{"source":12},"#);
}

#[test]
fn node_mesh_child_and_scene_references_out_of_range() {
    must_fail(
        r#""scale":[2.0,2.0,2.0],"mesh":1"#,
        r#""scale":[2.0,2.0,2.0],"mesh":31"#,
    );
    must_fail(r#""children":[1]"#, r#""children":[40]"#);
    must_fail(
        r#""scenes":[{"nodes":[0,2]}]"#,
        r#""scenes":[{"nodes":[0,200]}]"#,
    );
    must_fail(r#""scene":0"#, r#""scene":5"#);
}

#[test]
fn buffer_view_and_buffer_references_out_of_range() {
    must_fail(
        r#"{"bufferView":0,"componentType":5126,"count":3"#,
        r#"{"bufferView":90,"componentType":5126,"count":3"#,
    );
    must_fail(
        r#"{"buffer":0,"byteOffset":0,"byteLength":36}"#,
        r#"{"buffer":3,"byteOffset":0,"byteLength":36}"#,
    );
}

#[test]
fn views_and_accessors_past_the_end_of_the_buffer() {
    must_fail(
        r#"{"buffer":0,"byteOffset":0,"byteLength":36}"#,
        r#"{"buffer":0,"byteOffset":4294967000,"byteLength":36}"#,
    );
    must_fail(
        r#"{"buffer":0,"byteOffset":0,"byteLength":36}"#,
        r#"{"buffer":0,"byteOffset":0,"byteLength":18446744073709551615}"#,
    );
    // Accessor claims far more elements than its view holds.
    must_fail(
        r#"{"bufferView":0,"componentType":5126,"count":3"#,
        r#"{"bufferView":0,"componentType":5126,"count":4000000000"#,
    );
    must_fail(
        r#"{"bufferView":5,"componentType":5125,"count":6"#,
        r#"{"bufferView":5,"componentType":5125,"count":1000000"#,
    );
    // Accessor byteOffset pushes it past its view.
    must_fail(
        r#"{"bufferView":1,"componentType":5126,"count":3"#,
        r#"{"bufferView":1,"byteOffset":4294967295,"componentType":5126,"count":3"#,
    );
}

#[test]
fn vertex_index_past_the_vertex_count() {
    // The deck's index view reinterpreted as u32 quad indices pointing at 3 vertices
    // is fine; pointing the triangle at the quad's u32 index list (values up to 3)
    // gives an index past its 3 vertices.
    let ok = run(
        r#""TEXCOORD_0":2},"indices":3"#,
        r#""TEXCOORD_0":2},"indices":5"#,
    );
    // `run` already asserts that anything loaded has in-range indices.
    let _ = ok;
}

#[test]
fn mismatched_attribute_counts_and_types() {
    // NORMAL accessor with fewer elements than POSITION; wrong component types.
    run(
        r#"{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"}"#,
        r#"{"bufferView":1,"componentType":5126,"count":1,"type":"VEC3"}"#,
    );
    run(
        r#"{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"}"#,
        r#"{"bufferView":1,"componentType":5120,"count":3,"type":"MAT4"}"#,
    );
    run(
        r#"{"bufferView":3,"componentType":5123,"count":3,"type":"SCALAR"}"#,
        r#"{"bufferView":3,"componentType":5126,"count":1,"type":"VEC3"}"#,
    );
    run(
        r#"{"bufferView":2,"componentType":5126,"count":3,"type":"VEC2"}"#,
        r#"{"bufferView":2,"componentType":9999,"count":3,"type":"BOGUS"}"#,
    );
}

#[test]
fn node_cycles_terminate() {
    // insert lists deck as its child: deck -> insert -> deck -> ...
    run(
        r#"{"name":"insert","translation""#,
        r#"{"name":"insert","children":[0],"translation""#,
    );
    // A node that is its own child.
    run(r#""children":[1]"#, r#""children":[0]"#);
}

#[test]
fn node_listed_twice_terminates() {
    run(r#""children":[1]"#, r#""children":[1,1,1,2]"#);
    run(
        r#""scenes":[{"nodes":[0,2]}]"#,
        r#""scenes":[{"nodes":[0,0,2,1]}]"#,
    );
}

#[test]
fn degenerate_transforms_do_not_panic() {
    run(r#""scale":[2.0,2.0,2.0]"#, r#""scale":[0.0,0.0,0.0]"#);
    run(r#""scale":[2.0,2.0,2.0]"#, r#""scale":[1e38,1e38,1e38]"#);
    run(
        r#""rotation":[0.0,0.0,0.70710678,0.70710678]"#,
        r#""rotation":[0.0,0.0,0.0,0.0]"#,
    );
    run(
        r#""translation":[1.0,2.0,3.0]"#,
        r#""matrix":[1,0,0,0,0,1,0,0,0,0,1,0,1e39,2,3,1]"#,
    );
}

#[test]
fn sparse_accessors_and_missing_views_do_not_panic() {
    // Accessor without a bufferView (legal: all zeros) and a sparse accessor
    // pointing at dangling views.
    run(
        r#"{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"}"#,
        r#"{"componentType":5126,"count":3,"type":"VEC3"}"#,
    );
    run(
        r#"{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"}"#,
        r#"{"componentType":5126,"count":3,"type":"VEC3","sparse":{"count":2,"indices":{"bufferView":70,"componentType":5123},"values":{"bufferView":71}}}"#,
    );
    run(
        r#"{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"}"#,
        r#"{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3","sparse":{"count":3000000000,"indices":{"bufferView":3,"componentType":5123},"values":{"bufferView":0}}}"#,
    );
}

#[test]
fn zero_byte_stride_and_odd_strides_do_not_panic() {
    run(
        r#"{"buffer":0,"byteOffset":0,"byteLength":36}"#,
        r#"{"buffer":0,"byteOffset":0,"byteLength":36,"byteStride":0}"#,
    );
    run(
        r#"{"buffer":0,"byteOffset":0,"byteLength":36}"#,
        r#"{"buffer":0,"byteOffset":0,"byteLength":36,"byteStride":255}"#,
    );
    run(
        r#"{"buffer":0,"byteOffset":0,"byteLength":36}"#,
        r#"{"buffer":0,"byteOffset":0,"byteLength":36,"byteStride":4}"#,
    );
}

#[test]
fn deeply_nested_extras_do_not_overflow() {
    let deep = format!("{}1{}", "[".repeat(5000), "]".repeat(5000));
    let json = scene_json(BufferRef::Embedded, &format!(r#"{{"x":{deep}}}"#), "{}");
    let mut m = BTreeMap::new();
    m.insert("x.glb".to_owned(), fx::glb(&json, &scene_bin().0));
    let v = Vfs::from_zip_bytes(fx::zip_of(&m), ZipLimits::default()).unwrap();
    let _ = load_gltf(&v, "x.glb");
}
