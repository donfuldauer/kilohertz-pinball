//! Subphase 5.5: the generated backbox loads with the runtime loader
//! (`pin_assets::load_table`, `load_gltf`), and its bytes are deterministic
//! and pinned by a golden hash.

#[path = "backbox_common.rs"]
mod common;

use common::*;
use pin_assets::{Vfs, load_gltf};
use pin_core::FlipperStagingMode;
use pin_tablegen::backbox::{BACKBOX_COLLIDERS_FILE, BACKBOX_PREFIX, BACKBOX_SCENE_FILE};

/// FNV-1a of the example's three outputs. Update only with a reviewed
/// geometry change (the analytic tests below must pass first).
const GOLDEN_SCENE: u64 = 0xd8c5_225f_da3a_b29f;
const GOLDEN_COLLIDERS: u64 = 0xb253_758e_06b4_4152;
const GOLDEN_TABLE: u64 = 0x54cb_41e6_44b9_b517;

#[test]
fn golden_bytes_are_deterministic() {
    let a = generated();
    let b = generated();
    let hashes = |g: &pin_tablegen::backbox::GeneratedBackbox| {
        (
            fnv1a(&g.scene_glb),
            fnv1a(&g.colliders_glb),
            fnv1a(g.table_toml.as_bytes()),
        )
    };
    let (ha, hb) = (hashes(&a), hashes(&b));
    println!(
        "scene {:#018x} colliders {:#018x} table {:#018x}",
        ha.0, ha.1, ha.2
    );
    assert_eq!(ha, hb, "two runs give the same bytes");
    assert_eq!(
        ha,
        (GOLDEN_SCENE, GOLDEN_COLLIDERS, GOLDEN_TABLE),
        "golden hashes"
    );
}

#[test]
fn bundle_loads_with_the_runtime_loader() {
    let (dir, table, g) = load("load");
    // Flippers: the apex one plus three backbox flippers on layer 4.
    let bb: Vec<_> = table
        .manifest
        .flippers
        .iter()
        .enumerate()
        .filter(|(_, f)| f.id.starts_with(BACKBOX_PREFIX))
        .collect();
    assert_eq!(bb.len(), 3);
    for (i, _) in &bb {
        let c = &table.flippers[*i];
        assert_eq!(c.collision_layer, 4);
        let n = glam::Vec3::from(c.surface_normal);
        assert!(
            n.z > 0.0 && n.y < -0.99,
            "backbox normal towards the player: {n:?}"
        );
    }
    assert!(matches!(
        table.flippers[bb[1].0].staging,
        FlipperStagingMode::ParallelLinked(_)
    ));
    // The VUK and both transitions.
    assert!(
        table
            .manifest
            .mechs
            .iter()
            .any(|m| m.id == format!("{BACKBOX_PREFIX}vuk"))
    );
    let zones: Vec<_> = table
        .transitions
        .iter()
        .map(|t| (t.from_layer, t.to_layer))
        .collect();
    assert!(
        zones.contains(&(0, 4)) && zones.contains(&(4, 0)),
        "{zones:?}"
    );
    assert!(table.backbox.is_some(), "apex declares [backbox]");

    // Both glbs load with the runtime glTF loader.
    let vfs = Vfs::open(&dir).unwrap();
    let scene = load_gltf(&vfs, BACKBOX_SCENE_FILE).unwrap();
    let names: Vec<&str> = scene.meshes.iter().map(|m| m.name.as_str()).collect();
    for want in [
        "backbox.back",
        "backbox.walls[0]",
        "backbox.roof",
        "backbox.shelf[0]",
        "backbox.flippers[0]",
        "backbox.flippers[2]",
    ] {
        assert!(names.contains(&want), "{want} in {names:?}");
    }
    assert!(!names.contains(&"backbox.glass"), "the glass is not drawn");
    assert!(scene.meshes.iter().all(|m| !m.extras.collider));
    let colliders = load_gltf(&vfs, BACKBOX_COLLIDERS_FILE).unwrap();
    assert_eq!(colliders.meshes.len(), g.colliders.len());
    for m in &colliders.meshes {
        assert!(m.extras.collider);
        assert_eq!(m.extras.collision_layer, Some(4));
    }
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn backbox_runs_after_the_layout_generator() {
    // The documented order: layout.toml first, then the backbox; both sets
    // of generated entries survive, and a backbox re-run is idempotent.
    let layout_text = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/apex_layout.toml"),
    )
    .unwrap();
    let layout = pin_tablegen::layout::parse_layout(&layout_text).unwrap();
    let main = pin_tablegen::generate(&layout, &base()).unwrap();
    let bb_layout = pin_tablegen::backbox::parse_backbox_layout(LAYOUT).unwrap();
    let g = pin_tablegen::backbox::generate_backbox(&bb_layout.backbox, &main.table).unwrap();
    let main_ids: Vec<_> = main.table.splines.iter().map(|s| s.id.clone()).collect();
    let after: Vec<_> = g.table.splines.iter().map(|s| s.id.clone()).collect();
    assert_eq!(main_ids, after);
    let again = pin_tablegen::backbox::generate_backbox(&bb_layout.backbox, &g.table).unwrap();
    assert_eq!(again.table_toml, g.table_toml);
}
