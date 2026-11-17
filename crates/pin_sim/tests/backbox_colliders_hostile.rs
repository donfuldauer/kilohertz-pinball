//! Work item w7pre-backbox-colliders, requirement 4: a hostile or malformed
//! `backbox_colliders.glb` is a load error of the same type as a bad
//! playfield mesh (`SimError::Asset`, the glTF text naming the file), never
//! a panic. Cases: malformed bytes, a node that is not a collider, a bad
//! mesh reference, a missing / 0 / > 7 collision layer, non-finite vertices
//! (in the buffer, or after the node transform), more nodes than
//! `MAX_BACKBOX_COLLIDERS`, and bounded random corruptions of a valid file.

mod sim_support;

use pin_assets::{AssetError, BACKBOX_COLLIDERS_FILE, Vfs};
use pin_sim::load::{MAX_BACKBOX_COLLIDERS, backbox_colliders};
use pin_sim::{SimConfig, SimError, Simulation};
use sim_support::*;

/// One triangle node: `extras` JSON object body, optional `translation` /
/// `scale`, and the mesh index (0 is the only mesh).
struct NodeSpec {
    extras: String,
    transform: String,
    mesh: usize,
}

fn node(extras: &str) -> NodeSpec {
    NodeSpec {
        extras: extras.to_owned(),
        transform: String::new(),
        mesh: 0,
    }
}

/// A GLB with one shared triangle mesh (`positions`, material "acrylic";
/// mesh 0) placed by every node of `nodes`; mesh 1 is two primitives of the
/// same triangle (no material).
fn glb(nodes: &[NodeSpec], positions: [[f32; 3]; 3]) -> Vec<u8> {
    let mut bin = Vec::new();
    for p in positions {
        for c in p {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    let node_json: Vec<String> = nodes
        .iter()
        .enumerate()
        .map(|(i, n)| {
            format!(
                "{{\"name\":\"panel{i}\",\"mesh\":{}{},\"extras\":{{{}}}}}",
                n.mesh, n.transform, n.extras
            )
        })
        .collect();
    let ids: Vec<String> = (0..nodes.len()).map(|i| i.to_string()).collect();
    let json = format!(
        "{{\"asset\":{{\"version\":\"2.0\"}},\"scene\":0,\"scenes\":[{{\"nodes\":[{}]}}],\
         \"nodes\":[{}],\
         \"meshes\":[{{\"primitives\":[{{\"attributes\":{{\"POSITION\":0}},\"material\":0,\"mode\":4}}]}},\
         {{\"primitives\":[{{\"attributes\":{{\"POSITION\":0}},\"mode\":4}},\
         {{\"attributes\":{{\"POSITION\":0}},\"mode\":4}}]}}],\
         \"materials\":[{{\"name\":\"acrylic\"}}],\
         \"accessors\":[{{\"bufferView\":0,\"componentType\":5126,\"count\":3,\"type\":\"VEC3\",\
         \"min\":[0.0,0.0,0.0],\"max\":[1.0,1.0,1.0]}}],\
         \"bufferViews\":[{{\"buffer\":0,\"byteOffset\":0,\"byteLength\":{}}}],\
         \"buffers\":[{{\"byteLength\":{}}}]}}",
        ids.join(","),
        node_json.join(","),
        bin.len(),
        bin.len()
    );
    let mut json = json.into_bytes();
    while !json.len().is_multiple_of(4) {
        json.push(b' ');
    }
    let total = 12 + 8 + json.len() + 8 + bin.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(json.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    out.extend_from_slice(&json);
    out.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    out.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    out.extend_from_slice(&bin);
    out
}

/// A small triangle 1 m above the fixture's playfield.
const TRI: [[f32; 3]; 3] = [[0.1, 0.5, 1.0], [0.2, 0.5, 1.0], [0.1, 0.6, 1.0]];

const GOOD: &str = "\"collider\":true,\"collision_layer\":4";

/// The replay fixture plus `bytes` as its backbox colliders file.
fn bundle(tag: &str, bytes: &[u8]) -> ScratchDir {
    fixture_variant(tag, &[(BACKBOX_COLLIDERS_FILE, Some(bytes))])
}

/// Loads the bundle: `Ok(collider count)` or the error.
fn load(dir: &ScratchDir) -> Result<usize, SimError> {
    let path = dir.to_path_buf();
    on_sim_thread(move || {
        Simulation::load(&path, SimConfig::default()).map(|s| s.static_world().collider_count())
    })
}

/// The load fails with a glTF asset error naming the backbox file and
/// containing `needle`.
fn assert_gltf_error(tag: &str, bytes: &[u8], needle: &str) {
    let dir = bundle(tag, bytes);
    match load(&dir) {
        Err(SimError::Asset(AssetError::Gltf(m))) => {
            assert!(m.contains(BACKBOX_COLLIDERS_FILE), "{tag}: {m}");
            assert!(m.contains(needle), "{tag}: {m:?} lacks {needle:?}");
        }
        other => panic!("{tag}: expected a glTF asset error, got {other:?}"),
    }
}

fn fixture_count() -> usize {
    let path = fixture();
    on_sim_thread(move || {
        Simulation::load(&path, SimConfig::default())
            .unwrap()
            .static_world()
            .collider_count()
    })
}

#[test]
fn a_valid_hand_made_file_loads_and_appends_its_nodes() {
    let nodes: Vec<NodeSpec> = (0..3).map(|_| node(GOOD)).collect();
    let dir = bundle("bbh_ok", &glb(&nodes, TRI));
    assert_eq!(load(&dir).unwrap(), fixture_count() + 3);
    let vfs = Vfs::open(&dir).unwrap();
    let c = backbox_colliders(&vfs).unwrap();
    assert_eq!(c.len(), 3);
    for (i, k) in c.iter().enumerate() {
        assert_eq!(k.name, format!("panel{i}"), "node order");
        assert_eq!(k.layer_mask, 1 << 4);
        assert_eq!(k.material, pin_core::Material::Acrylic);
    }
}

#[test]
fn every_backbox_layer_1_to_7_is_accepted() {
    for layer in 1..=7u8 {
        let extras = format!("\"collider\":true,\"collision_layer\":{layer}");
        let dir = bundle("bbh_layers", &glb(&[node(&extras)], TRI));
        let vfs = Vfs::open(&dir).unwrap();
        let c = backbox_colliders(&vfs).unwrap();
        assert_eq!(c[0].layer_mask, 1 << layer);
    }
}

#[test]
fn malformed_bytes_are_a_gltf_error() {
    // Like a corrupt playfield.glb: the asset error, not a panic.
    let dir = bundle("bbh_junk", b"glTF\x02\0\0\0\xff\xff\xff\xff junk");
    assert!(matches!(load(&dir), Err(SimError::Asset(_))));
    let dir = bundle("bbh_text", b"not a gltf at all");
    assert!(matches!(load(&dir), Err(SimError::Asset(_))));
    let dir = bundle("bbh_empty", b"");
    assert!(matches!(load(&dir), Err(SimError::Asset(_))));
}

#[test]
fn a_node_that_is_not_a_collider_is_an_error() {
    let nodes = [node(GOOD), node("\"collider\":false,\"collision_layer\":4")];
    assert_gltf_error("bbh_nocollider", &glb(&nodes, TRI), "panel1");
    assert_gltf_error(
        "bbh_noextras",
        &glb(&[node("\"collision_layer\":4")], TRI),
        "not a collider",
    );
}

#[test]
fn a_bad_mesh_reference_is_an_error() {
    let mut bad = node(GOOD);
    bad.mesh = 7;
    let dir = bundle("bbh_badmesh", &glb(&[bad], TRI));
    assert!(matches!(
        load(&dir),
        Err(SimError::Asset(AssetError::Gltf(_)))
    ));
}

#[test]
fn layer_0_missing_or_above_7_is_an_error() {
    assert_gltf_error(
        "bbh_layer0",
        &glb(&[node("\"collider\":true,\"collision_layer\":0")], TRI),
        "1..=7",
    );
    assert_gltf_error(
        "bbh_nolayer",
        &glb(&[node("\"collider\":true")], TRI),
        "1..=7",
    );
    for bad in ["8", "255", "-1", "4.5", "\"4\""] {
        let extras = format!("\"collider\":true,\"collision_layer\":{bad}");
        let dir = bundle("bbh_layerbig", &glb(&[node(&extras)], TRI));
        assert!(
            matches!(load(&dir), Err(SimError::Asset(AssetError::Gltf(_)))),
            "collision_layer {bad}"
        );
    }
}

#[test]
fn non_finite_vertices_are_an_error() {
    // In the buffer.
    for v in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut tri = TRI;
        tri[1][2] = v;
        let dir = bundle("bbh_nan", &glb(&[node(GOOD)], tri));
        assert!(
            matches!(load(&dir), Err(SimError::Asset(AssetError::Gltf(_)))),
            "vertex {v}"
        );
    }
    // Finite in the file, infinite in the table frame (node scale).
    let mut big = node(GOOD);
    big.transform = ",\"scale\":[3.0e38,3.0e38,3.0e38]".to_owned();
    let tri = [[2.0, 2.0, 2.0], [3.0, 2.0, 2.0], [2.0, 3.0, 2.0]];
    assert_gltf_error("bbh_overflow", &glb(&[big], tri), "non-finite");
}

#[test]
fn more_than_the_node_limit_is_an_error() {
    let nodes: Vec<NodeSpec> = (0..=MAX_BACKBOX_COLLIDERS).map(|_| node(GOOD)).collect();
    let dir = bundle("bbh_many", &glb(&nodes, TRI));
    assert!(matches!(
        load(&dir),
        Err(SimError::Asset(AssetError::Gltf(_)))
    ));
    let vfs = Vfs::open(&dir).unwrap();
    let Err(SimError::Asset(AssetError::Gltf(m))) = backbox_colliders(&vfs) else {
        panic!("over the limit");
    };
    assert!(m.contains("budget"), "{m}");
}

#[test]
fn more_placed_primitives_than_the_limit_is_an_error() {
    // Within the node budget, but every node places two primitives.
    let n = MAX_BACKBOX_COLLIDERS / 2 + 1;
    let nodes: Vec<NodeSpec> = (0..n)
        .map(|_| {
            let mut k = node(GOOD);
            k.mesh = 1;
            k
        })
        .collect();
    assert_gltf_error("bbh_prims", &glb(&nodes, TRI), "exceed the limit");
    // Two primitives per node under the limit load, both wood (no material).
    let mut k = node(GOOD);
    k.mesh = 1;
    let dir = bundle("bbh_prims_ok", &glb(&[k], TRI));
    let vfs = Vfs::open(&dir).unwrap();
    let c = backbox_colliders(&vfs).unwrap();
    assert_eq!(c.len(), 2);
    assert!(c.iter().all(|k| k.material == pin_core::Material::Wood));
}

#[test]
fn the_node_limit_itself_is_accepted() {
    let nodes: Vec<NodeSpec> = (0..MAX_BACKBOX_COLLIDERS).map(|_| node(GOOD)).collect();
    let dir = bundle("bbh_limit", &glb(&nodes, TRI));
    let vfs = Vfs::open(&dir).unwrap();
    assert_eq!(
        backbox_colliders(&vfs).unwrap().len(),
        MAX_BACKBOX_COLLIDERS
    );
}

#[test]
fn bounded_random_corruptions_never_panic() {
    // A seeded xorshift; 300 files: 1..=8 random byte overwrites of a
    // valid file, or a random truncation.
    let good = glb(&[node(GOOD), node(GOOD)], TRI);
    let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    };
    let dir = scratch_dir("bbh_fuzz");
    std::fs::write(dir.join("x"), b"x").unwrap();
    for case in 0..300 {
        let mut bytes = good.clone();
        if case % 5 == 4 {
            bytes.truncate((next() % bytes.len() as u64) as usize);
        } else {
            for _ in 0..=(next() % 8) {
                let i = (next() % bytes.len() as u64) as usize;
                bytes[i] = next() as u8;
            }
        }
        std::fs::write(dir.join(BACKBOX_COLLIDERS_FILE), &bytes).unwrap();
        let vfs = Vfs::open(&dir).unwrap();
        let r = std::panic::catch_unwind(|| backbox_colliders(&vfs));
        let r = r.unwrap_or_else(|_| panic!("case {case}: panicked"));
        if let Ok(c) = r {
            for k in &c {
                let pin_physics::world::StaticShape::TriMesh { vertices, .. } = &k.shape else {
                    panic!("trimesh");
                };
                assert!(vertices.iter().all(|v| v.is_finite()), "case {case}");
                assert!(
                    k.layer_mask.count_ones() == 1 && k.layer_mask != 1,
                    "case {case}"
                );
            }
        }
    }
}
