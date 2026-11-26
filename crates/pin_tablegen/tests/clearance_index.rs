//! KP-054 (issue #135): the clearance validator's finer spatial index, tested
//! from the public API alone (SPEC §4 Subphase 2.8 deliverable 2; SPEC 6.0:
//! the app runs the validator on every table load).
//!
//! * `SpatialIndex::Fine` gives exactly the verdicts of the reference
//!   `SpatialIndex::Legacy` uniform grid on every generated table and on the
//!   shipped tables (report equality: errors, warnings, messages, order).
//! * The narrow-phase pairs-examined counter (`IndexStats`, never wall clock,
//!   R6) grows at most linearly with triangle count between 60k and 540k
//!   triangles at constant triangle density.
//! * Robustness (R11): empty, huge, coincident and zero-area inputs.

mod support;

use pin_assets::{GltfScene, LoadedTable, MeshData, NodeExtras, Vfs, load_gltf, load_table};
use pin_core::schema::TableManifest;
use pin_tablegen::clearance::{
    ClearanceReport, IndexStats, SpatialIndex, validate_bundle, validate_clearance,
    validate_clearance_diameters, validate_clearance_indexed,
};
use pin_tablegen::layout::{generate, parse_layout, write_bundle};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use support::*;

/// Ball diameters (m) used for the `diameters_m = Some(..)` runs: mini,
/// standard and XL.
const DIAMETERS: [f32; 3] = [0.01588, 0.0270, 0.0794];

const IDENTITY: [[f32; 4]; 4] = [
    [1.0, 0.0, 0.0, 0.0],
    [0.0, 1.0, 0.0, 0.0],
    [0.0, 0.0, 1.0, 0.0],
    [0.0, 0.0, 0.0, 1.0],
];

/// A collider mesh on `layer` with identity transform.
fn collider(name: &str, layer: u8, positions: Vec<[f32; 3]>, indices: Vec<u32>) -> MeshData {
    let normals = vec![[0.0, 0.0, 1.0]; positions.len()];
    MeshData {
        name: name.to_owned(),
        positions,
        normals,
        uvs: vec![],
        indices,
        material: None,
        transform: IDENTITY,
        extras: NodeExtras {
            collision_layer: Some(layer),
            collider: true,
            ..NodeExtras::default()
        },
    }
}

/// A flat deck at height `z` (m) on `layer`: `nx` × `ny` square cells of
/// side `cell` (m) from `(x0, y0)`, two triangles per cell.
#[allow(clippy::too_many_arguments)]
fn grid_deck(
    name: &str,
    layer: u8,
    x0: f32,
    y0: f32,
    nx: u32,
    ny: u32,
    cell: f32,
    z: f32,
) -> MeshData {
    let mut positions = Vec::with_capacity(((nx + 1) * (ny + 1)) as usize);
    for j in 0..=ny {
        for i in 0..=nx {
            positions.push([x0 + i as f32 * cell, y0 + j as f32 * cell, z]);
        }
    }
    let mut indices = Vec::with_capacity((6 * nx * ny) as usize);
    let row = nx + 1;
    for j in 0..ny {
        for i in 0..nx {
            let a = j * row + i;
            indices.extend([a, a + 1, a + row + 1, a, a + row + 1, a + row]);
        }
    }
    collider(name, layer, positions, indices)
}

fn triangles(scene: &GltfScene) -> usize {
    scene.meshes.iter().map(|m| m.indices.len() / 3).sum()
}

fn run(
    table: &LoadedTable,
    scene: &GltfScene,
    d: Option<&[f32]>,
    index: SpatialIndex,
) -> (ClearanceReport, IndexStats) {
    validate_clearance_indexed(table, scene, d, index)
}

/// Asserts every entry point agrees on `scene`: `Legacy` vs `Fine` (with and
/// without explicit diameters), and `Fine` vs the plain entry points.
fn assert_same_verdicts(what: &str, table: &LoadedTable, scene: &GltfScene) {
    let (legacy, _) = run(table, scene, None, SpatialIndex::Legacy);
    let (fine, _) = run(table, scene, None, SpatialIndex::Fine);
    assert_eq!(legacy, fine, "{what}: Legacy vs Fine (declared profiles)");
    assert_eq!(
        validate_clearance(table, scene),
        fine,
        "{what}: validate_clearance vs Fine"
    );
    let (legacy, _) = run(table, scene, Some(&DIAMETERS), SpatialIndex::Legacy);
    let (fine, _) = run(table, scene, Some(&DIAMETERS), SpatialIndex::Fine);
    assert_eq!(legacy, fine, "{what}: Legacy vs Fine (diameters)");
    assert_eq!(
        validate_clearance_diameters(table, scene, &DIAMETERS),
        fine,
        "{what}: validate_clearance_diameters vs Fine"
    );
}

/// The Appendix A.3 apex table (a 65 mm deck over the main deck), loaded
/// once; the synthetic scenes below are validated against its manifest.
fn apex() -> &'static (LoadedTable, GltfScene) {
    static APEX: OnceLock<(LoadedTable, GltfScene)> = OnceLock::new();
    APEX.get_or_init(|| {
        let (dir, table, scene) = load(&gen_apex("apex_layout.toml"), "kp054_apex");
        std::fs::remove_dir_all(dir).unwrap();
        (table, scene)
    })
}

// ------------------------------------------------------ generated tables

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    let mut entries: Vec<_> = std::fs::read_dir(from)
        .unwrap()
        .map(|e| e.unwrap())
        .collect();
    entries.sort_by_key(|e| e.file_name());
    for entry in entries {
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// Checks every entry point on one written bundle.
fn check_bundle(what: &str, dir: &Path) {
    let vfs = Vfs::open(dir).unwrap();
    let table = load_table(&vfs).unwrap_or_else(|e| panic!("{what}: {e}"));
    let scene = load_gltf(&vfs, "playfield.glb").unwrap();
    assert!(triangles(&scene) > 0, "{what}");
    assert_same_verdicts(what, &table, &scene);
    assert_eq!(
        validate_bundle(&vfs).unwrap(),
        validate_clearance(&table, &scene),
        "{what}: validate_bundle"
    );
}

/// SPEC 2.8 (kp054): `Fine` and `Legacy` give identical reports on every
/// generated table the test fixtures describe (the apex-based layouts and
/// the 4.3 layouts on their own base bundle), and `Fine` is what the plain
/// entry points and `validate_bundle` use.
#[test]
fn spec_2_8_kp054_generated_tables_have_identical_verdicts() {
    for name in [
        "apex_layout.toml",
        "rich_layout.toml",
        "spec_2_8/open_layout.toml",
        "spec_4_3/a3_layout.toml",
    ] {
        let mut lay = layout(name);
        let mut base = apex_base();
        // As the existing clearance props do for the rich fixture: its
        // ramps need splines the apex base does not carry.
        if name == "rich_layout.toml" {
            base.splines.clear();
            base.layer_transitions.clear();
            lay.ramps.clear();
        }
        let generated = generate(&lay, &base).unwrap_or_else(|e| panic!("{name}: {e}"));
        let dir = bundle_dir("kp054_layouts");
        write_bundle(&generated, &dir).unwrap();
        check_bundle(name, &dir);
        std::fs::remove_dir_all(dir).unwrap();
    }
    // The 4.3 layouts on the 4.3 base bundle (the apex example plus lamps).
    let base_dir = fixtures().join("spec_4_3/base");
    let base_text = std::fs::read_to_string(base_dir.join("table.toml")).unwrap();
    let base = TableManifest::from_toml_str(&base_text).unwrap();
    for name in ["spec_4_3/a3_layout.toml", "spec_4_3/geo_layout.toml"] {
        let generated = generate(&layout(name), &base).unwrap_or_else(|e| panic!("{name}: {e}"));
        let dir = bundle_dir("kp054_layouts_43");
        std::fs::remove_dir_all(&dir).unwrap();
        copy_tree(&base_dir, &dir);
        write_bundle(&generated, &dir).unwrap();
        check_bundle(name, &dir);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// The shipped tables (`tables/<name>`).
const SHIPPED: [&str; 4] = ["greybox", "apex_circuit", "sky_deck", "feature_lab"];

fn shipped_src(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../tables")
        .join(name)
}

/// The clearance inputs of a shipped table, built in-process: `pin_tablegen`
/// generates its `playfield.glb` and merged `table.toml` from the table's
/// `layout.toml` and `table.toml` (the clearance-relevant step of its
/// `build.sh`) into a fresh copy of the apex fixture bundle, the table's
/// rules, backglass, glass, layout and display scenes are copied over it,
/// and the `sounds` entries are dropped: their files come from the
/// `pin_assetgen` placeholder kit, which this crate cannot depend on, and
/// sounds take no part in the clearance checks.
fn shipped(name: &str) -> (PathBuf, LoadedTable, GltfScene) {
    let src = shipped_src(name);
    let layout_text = std::fs::read_to_string(src.join("layout.toml")).unwrap();
    let table_text = std::fs::read_to_string(src.join("table.toml")).unwrap();
    let lay = parse_layout(&layout_text).unwrap_or_else(|e| panic!("{name}: {e}"));
    let base = TableManifest::from_toml_str(&table_text).unwrap_or_else(|e| panic!("{name}: {e}"));
    let generated = generate(&lay, &base).unwrap_or_else(|e| panic!("{name}: {e}"));
    let dir = bundle_dir(&format!("kp054_{name}"));
    write_bundle(&generated, &dir).unwrap();
    // The placeholder sounds come from the asset kit; they take no part in
    // the clearance checks.
    let written = std::fs::read_to_string(dir.join("table.toml")).unwrap();
    let mut manifest = TableManifest::from_toml_str(&written).unwrap();
    manifest.sounds.clear();
    std::fs::write(dir.join("table.toml"), manifest.to_toml_string().unwrap()).unwrap();
    for f in ["rules.lua", "backglass.toml", "glass.toml", "layout.toml"] {
        std::fs::copy(src.join(f), dir.join(f)).unwrap_or_else(|e| panic!("{name}: {f}: {e}"));
    }
    std::fs::create_dir_all(dir.join("display")).unwrap();
    let mut scenes: Vec<_> = std::fs::read_dir(src.join("display"))
        .unwrap()
        .map(|e| e.unwrap().path())
        .collect();
    scenes.sort();
    for s in scenes {
        std::fs::copy(&s, dir.join("display").join(s.file_name().unwrap())).unwrap();
    }
    let vfs = Vfs::open(&dir).unwrap();
    let table = load_table(&vfs).unwrap_or_else(|e| panic!("{name}: {e}"));
    let scene = load_gltf(&vfs, "playfield.glb").unwrap();
    (dir, table, scene)
}

/// SPEC 2.8 / issue #135 acceptance (kp054): verdicts on the four shipped
/// tables are identical under both indexes.
#[test]
fn spec_2_8_kp054_shipped_tables_have_identical_verdicts() {
    for name in SHIPPED {
        let (dir, table, scene) = shipped(name);
        assert!(triangles(&scene) > 0, "{name}");
        assert_same_verdicts(name, &table, &scene);
        let vfs = Vfs::open(&dir).unwrap();
        assert_eq!(
            validate_bundle(&vfs).unwrap(),
            validate_clearance(&table, &scene),
            "{name}: validate_bundle"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

// ------------------------------------------------------------- scaling

/// Two parallel ridged decks of `nx` × `ny` cells of side `cell` (2
/// triangles per cell): the main deck (layer 0) from z = 0 and an upper deck
/// (layer 1) `gap` right above it. Every other vertex column is raised by
/// `slope · cell`, so each cell slopes by ±`slope` across x (a ridge and
/// valley every cell, heights stay bounded). `4 nx ny` triangles at a
/// constant density: the area grows with the count.
///
/// Why ridged: over a flat support a validator may prove a clear ceiling
/// with a height bound alone and examine no pair at all (the gap and the
/// headroom are then the same number); over sloped supports it must
/// examine the support/ceiling pairs within reach.
fn two_decks(nx: u32, ny: u32, cell: f32, gap: f32, slope: f32) -> GltfScene {
    let ridged = |mut m: MeshData| {
        for (k, p) in m.positions.iter_mut().enumerate() {
            if (k as u32 % (nx + 1)) % 2 == 1 {
                p[2] += slope * cell;
            }
        }
        m
    };
    GltfScene {
        meshes: vec![
            ridged(grid_deck("main_deck", 0, 0.0, 0.0, nx, ny, cell, 0.0)),
            ridged(grid_deck("upper_deck", 1, 0.0, 0.0, nx, ny, cell, gap)),
        ],
        ..GltfScene::default()
    }
}

const CELL: f32 = 0.015;
/// Slope 0.5 (|n·z| = 0.894: walkable, and a ceiling) and a 33 mm vertical
/// gap: the 27 mm ball clears everywhere (perpendicular gap 29.5 mm on the
/// slopes, 33 − 13.5 / 0.894 = 17.9 mm over the centre in a valley, 28 mm
/// and 14.5 mm needed), yet the ceiling is within the reach of a ball on
/// a sloped support.
const SLOPE: f32 = 0.5;
const GAP: f32 = 0.033;

/// SPEC 2.8 and SPEC 6.0, R6 (kp054): at constant triangle density the
/// pairs examined by `Fine` grow at most linearly with the triangle count
/// between 60k and 540k triangles (×1.25 slack), measured with the
/// deterministic pairs-examined counter, never wall clock.
#[test]
fn spec_2_8_6_0_kp054_pairs_examined_grow_linearly() {
    let (table, _) = apex();
    let d: &[f32] = &[0.0270];
    let mut pairs = Vec::new();
    for (nx, ny, total) in [
        (150u32, 100u32, 60_000usize),
        (250, 180, 180_000),
        (450, 300, 540_000),
    ] {
        let scene = two_decks(nx, ny, CELL, GAP, SLOPE);
        assert_eq!(triangles(&scene), total);
        let (report, stats) = run(table, &scene, Some(d), SpatialIndex::Fine);
        assert!(stats.pairs_examined > 0, "{total}: no pairs examined");
        // The ridged decks clear the 27 mm ball everywhere.
        assert!(
            report
                .errors
                .iter()
                .all(|e| e.name != "upper_deck" && e.name != "main_deck"),
            "{total}: {:#?}",
            report.errors
        );
        if total == 60_000 {
            let (again, stats_again) = run(table, &scene, Some(d), SpatialIndex::Fine);
            assert_eq!(stats, stats_again, "the counter is deterministic");
            assert_eq!(report, again);
        }
        println!(
            "kp054 scaling: {total} triangles -> {} pairs examined",
            stats.pairs_examined
        );
        pairs.push((total, stats.pairs_examined));
    }
    let ratio = |a: usize, b: usize| pairs[b].1 as f64 / pairs[a].1 as f64;
    let r180 = ratio(0, 1);
    let r540 = ratio(0, 2);
    println!(
        "kp054 scaling: pairs(180k)/pairs(60k) = {r180:.3}, pairs(540k)/pairs(60k) = {r540:.3}"
    );
    assert!(
        r180 <= 1.25 * 3.0,
        "pairs(180k)/pairs(60k) = {r180} > 3.75: {pairs:?}"
    );
    assert!(
        r540 <= 1.25 * 9.0,
        "pairs(540k)/pairs(60k) = {r540} > 11.25: {pairs:?}"
    );
}

/// SPEC 2.8 (kp054): on a small dense scene (ridged decks of 10 mm cells,
/// five times finer than the reference grid's 50 mm minimum cell; 2912
/// triangles) `Fine` examines no more pairs than `Legacy` and reaches the
/// same verdicts.
#[test]
fn spec_2_8_kp054_fine_examines_no_more_pairs_than_legacy_on_a_dense_scene() {
    let (table, _) = apex();
    let scene = two_decks(26, 28, 0.010, GAP, SLOPE);
    assert_eq!(triangles(&scene), 2912);
    for d in [Some(&[0.0270f32][..]), Some(&DIAMETERS[..]), None] {
        let (legacy, ls) = run(table, &scene, d, SpatialIndex::Legacy);
        let (fine, fs) = run(table, &scene, d, SpatialIndex::Fine);
        println!(
            "kp054 dense scene {d:?}: Legacy {} pairs, Fine {} pairs",
            ls.pairs_examined, fs.pairs_examined
        );
        assert_eq!(legacy, fine, "{d:?}");
        assert!(fs.pairs_examined > 0, "{d:?}");
        assert!(
            fs.pairs_examined <= ls.pairs_examined,
            "{d:?}: Fine {} > Legacy {}",
            fs.pairs_examined,
            ls.pairs_examined
        );
    }
    // A gap the 27 mm ball does not clear is found by both, identically.
    let low = two_decks(26, 28, 0.010, 0.020, 0.0);
    let (legacy, _) = run(table, &low, Some(&[0.0270]), SpatialIndex::Legacy);
    let (fine, _) = run(table, &low, Some(&[0.0270]), SpatialIndex::Fine);
    assert_eq!(legacy, fine);
    assert!(
        fine.errors.iter().any(|e| e.name == "upper_deck"),
        "{:#?}",
        fine.errors
    );
}

// ---------------------------------------------------------- robustness

/// R11 (SPEC 2.8, kp054): an empty scene.
#[test]
fn spec_2_8_kp054_empty_scene() {
    let (table, _) = apex();
    assert_same_verdicts("empty scene", table, &GltfScene::default());
    // Empty collider meshes too.
    let scene = GltfScene {
        meshes: vec![
            collider("empty0", 0, vec![], vec![]),
            collider("empty1", 1, vec![], vec![]),
        ],
        ..GltfScene::default()
    };
    assert_same_verdicts("empty meshes", table, &scene);
    let (_, stats) = run(table, &scene, Some(&DIAMETERS), SpatialIndex::Fine);
    assert_eq!(
        stats,
        run(table, &scene, Some(&DIAMETERS), SpatialIndex::Fine).1
    );
}

/// R11 (SPEC 2.8, kp054): extreme extents, a 1000 m plan (two triangles on
/// the main deck and two ceiling triangles on layer 1, 40 mm up) plus a
/// cluster of 1 mm triangles on two layers; `Fine` must not panic, its
/// counter is deterministic and its report equals `Legacy`'s.
#[test]
fn spec_2_8_kp054_extreme_extents_do_not_panic() {
    let (table, _) = apex();
    let km = 1000.0f32;
    let mut scene = GltfScene {
        meshes: vec![
            collider(
                "huge_floor",
                0,
                vec![
                    [-km / 2.0, -km / 2.0, 0.0],
                    [km / 2.0, -km / 2.0, 0.0],
                    [km / 2.0, km / 2.0, 0.0],
                    [-km / 2.0, km / 2.0, 0.0],
                ],
                vec![0, 1, 2, 0, 2, 3],
            ),
            collider(
                "huge_ceiling",
                1,
                vec![
                    [-km / 2.0, -km / 2.0, 0.04],
                    [km / 2.0, -km / 2.0, 0.04],
                    [km / 2.0, km / 2.0, 0.04],
                    [-km / 2.0, km / 2.0, 0.04],
                ],
                vec![0, 1, 2, 0, 2, 3],
            ),
        ],
        ..GltfScene::default()
    };
    scene
        .meshes
        .push(grid_deck("cluster_floor", 0, 0.1, 0.1, 30, 30, 0.001, 0.0));
    scene.meshes.push(grid_deck(
        "cluster_ceiling",
        2,
        0.1,
        0.1,
        30,
        30,
        0.001,
        0.025,
    ));
    for d in [None, Some(&DIAMETERS[..])] {
        let (report, stats) = run(table, &scene, d, SpatialIndex::Fine);
        let (again, stats_again) = run(table, &scene, d, SpatialIndex::Fine);
        assert_eq!(report, again);
        assert_eq!(stats, stats_again);
        // The reference grid copes with this scene too (about a second).
        let (legacy, legacy_stats) = run(table, &scene, d, SpatialIndex::Legacy);
        assert_eq!(legacy, report, "{d:?}");
        println!(
            "kp054 extreme extents {d:?}: Legacy {} pairs, Fine {} pairs",
            legacy_stats.pairs_examined, stats.pairs_examined
        );
        // The 25 mm cluster ceiling stops every ball but the mini one.
        if d.is_some() {
            assert!(
                report.errors.iter().any(|e| e.name == "cluster_ceiling"),
                "{:#?}",
                report.errors
            );
        }
    }
}

/// R11 (SPEC 2.8, kp054): 10 000 coincident copies of one 20 mm walkable
/// triangle on the main deck under ten coincident ceiling copies (layer 1,
/// 25 mm up), plus 10 000 copies of a steep wall triangle; `Fine` equals
/// `Legacy`.
#[test]
fn spec_2_8_kp054_coincident_triangles() {
    let (table, _) = apex();
    let copies = |name: &str, layer: u8, tri: [[f32; 3]; 3], n: u32| {
        let positions = tri.to_vec();
        let indices = (0..n).flat_map(|_| [0u32, 1, 2]).collect();
        collider(name, layer, positions, indices)
    };
    let scene = GltfScene {
        meshes: vec![
            copies(
                "coincident_floor",
                0,
                [[0.2, 0.2, 0.0], [0.22, 0.2, 0.0], [0.2, 0.22, 0.0]],
                10_000,
            ),
            copies(
                "coincident_ceiling",
                1,
                [
                    [0.19, 0.19, 0.025],
                    [0.24, 0.19, 0.025],
                    [0.19, 0.24, 0.025],
                ],
                10,
            ),
            copies(
                "coincident_wall",
                0,
                [[0.3, 0.3, 0.0], [0.32, 0.3, 0.0], [0.3, 0.3, 0.03]],
                10_000,
            ),
        ],
        ..GltfScene::default()
    };
    assert_eq!(triangles(&scene), 20_010);
    assert_same_verdicts("coincident", table, &scene);
    let (fine, stats) = run(table, &scene, Some(&[0.0270]), SpatialIndex::Fine);
    println!("kp054 coincident: Fine {} pairs", stats.pairs_examined);
    assert!(
        fine.errors.iter().any(|e| e.name == "coincident_ceiling"),
        "{:#?}",
        fine.errors
    );
}

/// R11 (SPEC 2.8, kp054): zero-area triangles (a repeated vertex, three
/// collinear vertices, a point) on both layers, mixed with real ones;
/// `Fine` equals `Legacy`.
#[test]
fn spec_2_8_kp054_zero_area_triangles() {
    let (table, _) = apex();
    let degenerate = |name: &str, layer: u8, z: f32| {
        let positions = vec![
            [0.10, 0.10, z],
            [0.10, 0.10, z],
            [0.15, 0.10, z],
            [0.10, 0.20, z],
            [0.10, 0.25, z],
            [0.10, 0.30, z],
            [0.12, 0.12, z + 0.01],
        ];
        // Repeated vertex, collinear (in plan and in 3D), a point, a vertical
        // needle.
        let indices = vec![0, 1, 2, 3, 4, 5, 6, 6, 6, 0, 0, 6];
        collider(name, layer, positions, indices)
    };
    let scene = GltfScene {
        meshes: vec![
            degenerate("degenerate_floor", 0, 0.0),
            degenerate("degenerate_ceiling", 1, 0.02),
            grid_deck("floor", 0, 0.05, 0.05, 10, 10, 0.02, 0.0),
            grid_deck("ceiling", 1, 0.08, 0.08, 4, 4, 0.02, 0.035),
        ],
        ..GltfScene::default()
    };
    assert_same_verdicts("zero area", table, &scene);
    // Only degenerate triangles.
    let only = GltfScene {
        meshes: vec![degenerate("d0", 0, 0.0), degenerate("d1", 1, 0.01)],
        ..GltfScene::default()
    };
    assert_same_verdicts("zero area only", table, &only);
}
