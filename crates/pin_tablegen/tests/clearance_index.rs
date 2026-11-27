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
//!
//! KP-109 (issue #231): the headroom check's verdict-preserving prune in the
//! error case (a deck too low for the ball), tested from the public API alone
//! (SPEC §4 Subphase 2.8 deliverable 2; SPEC 6.0).
//!
//! * On too-low-deck scenes built to stress ties and late minima, `Fine`
//!   gives exactly `Legacy`'s report, including which support the headroom
//!   message names (the first support in triangle order that reaches the
//!   minimum), and repeated runs agree (R3).
//! * The pairs-examined counter grows at most linearly with the triangle
//!   count between about 8k and 75k triangles when the deck is too low for
//!   the 79.4 mm ball (counter, never wall clock, R6).

mod support;

use pin_assets::{GltfScene, LoadedTable, MeshData, NodeExtras, Vfs, load_gltf, load_table};
use pin_core::schema::TableManifest;
use pin_tablegen::clearance::{
    ClearanceError, ClearanceReport, Feature, IndexStats, SpatialIndex, validate_bundle,
    validate_clearance, validate_clearance_diameters, validate_clearance_indexed,
};
use pin_tablegen::layout::{generate, parse_layout, write_bundle};
use proptest::prelude::*;
use proptest::test_runner::RngSeed;
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

// ================================================== KP-109 (issue #231)

/// The 79.4 mm (XL) ball every too-low-deck scene below is too big for.
const XL: [f32; 1] = [0.0794];

/// The ball name an explicit 79.4 mm diameter reports under.
const XL_NAME: &str = "79.40 mm ball";

/// Vertical gap (m) of the too-low decks: the 79.4 mm ball needs 80.4 mm.
const LOW_GAP: f32 = 0.060;

fn scene_of(meshes: Vec<MeshData>) -> GltfScene {
    GltfScene {
        meshes,
        ..GltfScene::default()
    }
}

/// One collider mesh named `name` on `layer` made of the triangles of
/// `parts` (in order; their own names and layers are ignored).
fn merged(name: &str, layer: u8, parts: Vec<MeshData>) -> MeshData {
    let mut positions = Vec::new();
    let mut indices = Vec::new();
    for part in parts {
        let base = positions.len() as u32;
        positions.extend(part.positions);
        indices.extend(part.indices.iter().map(|i| i + base));
    }
    collider(name, layer, positions, indices)
}

/// `m` with every vertex height replaced by `f(x, y, z, vertex index)`.
fn reshaped(mut m: MeshData, f: impl Fn(f32, f32, f32, usize) -> f32) -> MeshData {
    for (k, p) in m.positions.iter_mut().enumerate() {
        p[2] = f(p[0], p[1], p[2], k);
    }
    m
}

/// A vertical wall quad on `layer` at `x` from `y0` to `y1`, 130 mm tall.
fn wall_x(name: &str, layer: u8, x: f32, y0: f32, y1: f32) -> MeshData {
    collider(
        name,
        layer,
        vec![[x, y0, -0.01], [x, y1, -0.01], [x, y1, 0.12], [x, y0, 0.12]],
        vec![0, 1, 2, 0, 2, 3],
    )
}

fn headroom_errors(r: &ClearanceReport) -> Vec<&ClearanceError> {
    r.errors
        .iter()
        .filter(|e| matches!(e.feature, Feature::DeckHeadroom))
        .collect()
}

/// What a too-low-deck scene must report for the 79.4 mm ball: the ceiling
/// mesh, the support the message names (node, layer) and, when known
/// analytically, the reported headroom (mm).
struct Expect<'a> {
    ceiling: &'a str,
    support: (&'a str, u8),
    headroom_mm: Option<f32>,
}

fn expect<'a>(ceiling: &'a str, support: (&'a str, u8), headroom_mm: Option<f32>) -> Expect<'a> {
    Expect {
        ceiling,
        support,
        headroom_mm,
    }
}

/// Asserts the deck headroom errors of `report` for ball `ball` are exactly
/// `expected` (one per ceiling mesh, in any order), each naming its support
/// in the SPEC 2.8 message form.
fn assert_headroom(what: &str, report: &ClearanceReport, ball: &str, expected: &[Expect]) {
    let errors: Vec<_> = headroom_errors(report)
        .into_iter()
        .filter(|e| e.profile == ball)
        .collect();
    assert_eq!(
        errors.len(),
        expected.len(),
        "{what}: deck headroom errors for {ball}: {errors:#?}"
    );
    for x in expected {
        let e = errors
            .iter()
            .find(|e| e.name == x.ceiling)
            .unwrap_or_else(|| panic!("{what}: no error for {:?}: {errors:#?}", x.ceiling));
        let over = format!(
            "over \"{}\" (layer {}) where ball \"{ball}\"",
            x.support.0, x.support.1
        );
        assert!(
            e.message
                .starts_with(&format!("deck headroom: \"{}\" (layer ", x.ceiling)),
            "{what}: {}",
            e.message
        );
        assert!(
            e.message.contains(&over),
            "{what}: expected {over:?} in {:?}",
            e.message
        );
        assert!(
            e.actual_mm < e.required_mm,
            "{what}: {} >= {}",
            e.actual_mm,
            e.required_mm
        );
        if let Some(h) = x.headroom_mm {
            assert!(
                (e.actual_mm - h).abs() < 0.05,
                "{what}: headroom {} mm, expected {h} mm: {}",
                e.actual_mm,
                e.message
            );
        }
    }
}

/// The full SPEC 2.8 (#231) verdict-identity check on one too-low-deck
/// scene: every entry point agrees (`assert_same_verdicts`); with the
/// explicit 79.4 mm ball `Legacy` and `Fine` give the same report (errors,
/// warnings, messages, order), `Fine` examines no more pairs, three runs of
/// each index give identical reports and counters (R3), and the deck
/// headroom errors are exactly `expected` (so the scene really is an error
/// case). Returns the `Fine` report.
fn check_error_scene(
    what: &str,
    table: &LoadedTable,
    scene: &GltfScene,
    expected: &[Expect],
) -> ClearanceReport {
    assert_same_verdicts(what, table, scene);
    let (legacy, ls) = run(table, scene, Some(&XL), SpatialIndex::Legacy);
    let (fine, fs) = run(table, scene, Some(&XL), SpatialIndex::Fine);
    assert_eq!(legacy, fine, "{what}: Legacy vs Fine (79.4 mm)");
    assert!(
        fs.pairs_examined <= ls.pairs_examined,
        "{what}: Fine {} > Legacy {}",
        fs.pairs_examined,
        ls.pairs_examined
    );
    for _ in 0..2 {
        assert_eq!(
            run(table, scene, Some(&XL), SpatialIndex::Fine),
            (fine.clone(), fs),
            "{what}: Fine repeat"
        );
        assert_eq!(
            run(table, scene, Some(&XL), SpatialIndex::Legacy),
            (legacy.clone(), ls),
            "{what}: Legacy repeat"
        );
    }
    assert_headroom(what, &fine, XL_NAME, expected);
    fine
}

// ----------------------------------------------- KP-109 linear growth

/// Two parallel decks of `nx` × `ny` cells of side `cell` (2 triangles per
/// cell): `main_deck` (layer 0) at z = 0 and `upper_deck` (layer 1)
/// `LOW_GAP` above it, flat or ridged as `two_decks` (±`SLOPE`).
fn too_low_decks(nx: u32, ny: u32, cell: f32, ridged: bool) -> GltfScene {
    if ridged {
        two_decks(nx, ny, cell, LOW_GAP, SLOPE)
    } else {
        scene_of(vec![
            grid_deck("main_deck", 0, 0.0, 0.0, nx, ny, cell, 0.0),
            grid_deck("upper_deck", 1, 0.0, 0.0, nx, ny, cell, LOW_GAP),
        ])
    }
}

/// Runs `Fine` with the 79.4 mm ball on each `(nx, ny, cell)` size, checks
/// each is the error case (exactly one deck headroom error, for
/// `upper_deck` over `main_deck`) with a positive counter (and, up to ~10k
/// triangles, the same report as `Legacy` with no more pairs), then checks
/// `pairs(n2) / pairs(n1) <= 1.25 · n2 / n1` for every pair of sizes.
fn assert_linear_pairs(what: &str, sizes: &[(u32, u32, f32)], ridged: bool) {
    let (table, _) = apex();
    let mut pairs: Vec<(usize, u64)> = Vec::new();
    for &(nx, ny, cell) in sizes {
        let scene = too_low_decks(nx, ny, cell, ridged);
        let n = triangles(&scene);
        assert_eq!(n, (4 * nx * ny) as usize);
        let (report, stats) = run(table, &scene, Some(&XL), SpatialIndex::Fine);
        let flat_headroom = (!ridged).then_some(LOW_GAP * 1000.0);
        assert_headroom(
            &format!("{what} {n}"),
            &report,
            XL_NAME,
            &[expect("upper_deck", ("main_deck", 0), flat_headroom)],
        );
        assert!(stats.pairs_examined > 0, "{what} {n}: no pairs examined");
        if n <= 10_000 {
            let (legacy, ls) = run(table, &scene, Some(&XL), SpatialIndex::Legacy);
            assert_eq!(legacy, report, "{what} {n}: Legacy vs Fine");
            assert!(stats.pairs_examined <= ls.pairs_examined, "{what} {n}");
            assert_eq!(
                run(table, &scene, Some(&XL), SpatialIndex::Fine),
                (report, stats),
                "{what} {n}: deterministic"
            );
            println!(
                "kp109 {what}: {n} triangles -> Fine {} pairs (Legacy {})",
                stats.pairs_examined, ls.pairs_examined
            );
        } else {
            println!(
                "kp109 {what}: {n} triangles -> Fine {} pairs",
                stats.pairs_examined
            );
        }
        pairs.push((n, stats.pairs_examined));
    }
    for (a, &(n1, p1)) in pairs.iter().enumerate() {
        for &(n2, p2) in &pairs[a + 1..] {
            let growth = p2 as f64 / p1 as f64;
            let linear = n2 as f64 / n1 as f64;
            println!("kp109 {what}: pairs({n2})/pairs({n1}) = {growth:.3} (linear {linear:.3})");
            assert!(
                growth <= 1.25 * linear,
                "{what}: pairs({n2})/pairs({n1}) = {growth:.3} > 1.25 x {linear:.3}: {pairs:?}"
            );
        }
    }
}

/// ~8k, ~30k and ~75k triangles at the constant 15 mm cell (the area grows
/// with the count).
const CONSTANT_DENSITY: [(u32, u32, f32); 3] = [(46, 46, CELL), (87, 87, CELL), (137, 137, CELL)];

/// ~8k, ~30k and ~75k triangles on the same 690 mm square plan (the
/// density grows with the count, as in the #135 measurement).
const FIXED_AREA: [(u32, u32, f32); 3] = [
    (46, 46, 0.690 / 46.0),
    (87, 87, 0.690 / 87.0),
    (137, 137, 0.690 / 137.0),
];

/// SPEC 2.8 and SPEC 6.0, R6 (#231 acceptance): two flat parallel decks
/// 60 mm apart (too low for the 79.4 mm ball) at a constant triangle
/// density; between ~8k and ~75k triangles the pairs examined by `Fine`
/// grow at most linearly (×1.25 slack), measured with the deterministic
/// counter, never wall clock.
#[test]
fn spec_2_8_6_0_kp109_too_low_flat_deck_pairs_grow_linearly() {
    assert_linear_pairs("flat constant density", &CONSTANT_DENSITY, false);
}

/// SPEC 2.8 and SPEC 6.0, R6 (#231 acceptance): the ridged variant (each
/// cell slopes ±0.5 across x) of the too-low decks, 60 mm apart, at a
/// constant triangle density: the pairs examined grow at most linearly.
#[test]
fn spec_2_8_6_0_kp109_too_low_ridged_deck_pairs_grow_linearly() {
    assert_linear_pairs("ridged constant density", &CONSTANT_DENSITY, true);
}

/// SPEC 2.8 and SPEC 6.0, R6 (#231 acceptance, the #135 "Known limit"
/// shape: a dense floor under a too-low deck, 60 mm apart): the flat decks
/// on one fixed 690 mm plan, the triangle density growing with the count
/// (~8k, ~30k, ~75k triangles), so every support has more ceiling triangles
/// within reach as the count grows; the pairs examined must still grow at
/// most linearly.
#[test]
fn spec_2_8_6_0_kp109_too_low_dense_deck_on_fixed_plan_pairs_grow_linearly() {
    assert_linear_pairs("flat fixed plan", &FIXED_AREA, false);
}

// ---------------------------------------------- KP-109 verdict identity

/// All permutations of `0..n` in lexicographic order.
fn permutations(n: usize) -> Vec<Vec<usize>> {
    if n == 0 {
        return vec![vec![]];
    }
    let mut out = Vec::new();
    for first in 0..n {
        for rest in permutations(n - 1) {
            let mut p = vec![first];
            p.extend(rest.into_iter().map(|r| if r >= first { r + 1 } else { r }));
            out.push(p);
        }
    }
    out
}

/// SPEC 2.8 (#231, scene a): a flat deck 60 mm over a flat floor, so every
/// support under it reaches the same minimum headroom (60 mm). With the
/// floor as one mesh, and split into four strips listed in every order (the
/// strips alternating between layers 0 and 2, the deck listed first, in the
/// middle or last), the message names the first strip in triangle order,
/// identically under `Legacy` and `Fine` and on repeated runs (R3).
#[test]
fn spec_2_8_kp109_flat_ties_name_the_first_support() {
    let (table, _) = apex();
    let single = scene_of(vec![
        grid_deck("main_deck", 0, 0.0, 0.0, 8, 6, 0.02, 0.0),
        grid_deck("upper_deck", 1, 0.0, 0.0, 8, 6, 0.02, LOW_GAP),
    ]);
    check_error_scene(
        "flat tie, one floor",
        table,
        &single,
        &[expect("upper_deck", ("main_deck", 0), Some(60.0))],
    );
    let strip_layer = |s: usize| if s.is_multiple_of(2) { 0u8 } else { 2 };
    for (k, order) in permutations(4).into_iter().enumerate() {
        let mut meshes: Vec<MeshData> = order
            .iter()
            .map(|&s| {
                grid_deck(
                    &format!("strip_{s}"),
                    strip_layer(s),
                    s as f32 * 0.04,
                    0.0,
                    2,
                    6,
                    0.02,
                    0.0,
                )
            })
            .collect();
        let deck = grid_deck("deck", 1, 0.0, 0.0, 8, 6, 0.02, LOW_GAP);
        meshes.insert(k % 5, deck);
        let first = order[0];
        let name = format!("strip_{first}");
        check_error_scene(
            &format!("flat tie, strips {order:?}"),
            table,
            &scene_of(meshes),
            &[expect("deck", (&name, strip_layer(first)), Some(60.0))],
        );
    }
}

/// SPEC 2.8 (#231, scene b): ridged too-low decks; a deck partially
/// overlapping a floor split in strips (the first strips are outside the
/// deck: the minimum is first reached by a later strip); a tilted deck over
/// strips listed out of order; a deck with a hole over a floor patch inside
/// the hole (listed first) and the rim floor.
#[test]
fn spec_2_8_kp109_ridged_partial_tilted_and_holed_decks() {
    let (table, _) = apex();
    check_error_scene(
        "ridged",
        table,
        &two_decks(10, 8, CELL, LOW_GAP, SLOPE),
        &[expect("upper_deck", ("main_deck", 0), None)],
    );
    // Partial overlap: five 2-cell strips (30 mm) along x, the deck from
    // x = 75 mm (cell 5) on, 55 mm up. Strips 0 and 1 stay outside the
    // deck's plan; strip 2 (cells 4-5) is the first under it.
    let mut meshes: Vec<MeshData> = (0..5)
        .map(|s| {
            grid_deck(
                &format!("strip_{s}"),
                0,
                s as f32 * 0.03,
                0.0,
                2,
                10,
                CELL,
                0.0,
            )
        })
        .collect();
    meshes.push(grid_deck("deck", 1, 0.075, 0.075, 10, 10, CELL, 0.055));
    check_error_scene(
        "partial overlap",
        table,
        &scene_of(meshes),
        &[expect("deck", ("strip_2", 0), Some(55.0))],
    );
    // Tilted deck: z = 45 mm + 0.05 x over four strips listed 3, 1, 0, 2;
    // the lowest ceiling is over strip 0 (x = 0).
    let mut meshes: Vec<MeshData> = [3usize, 1, 0, 2]
        .iter()
        .map(|&s| {
            grid_deck(
                &format!("strip_{s}"),
                0,
                s as f32 * 0.04,
                0.0,
                2,
                6,
                0.02,
                0.0,
            )
        })
        .collect();
    meshes.push(reshaped(
        grid_deck("deck", 1, 0.0, 0.0, 8, 6, 0.02, 0.0),
        |x, _, _, _| 0.045 + 0.05 * x,
    ));
    let report = check_error_scene(
        "tilted",
        table,
        &scene_of(meshes),
        &[expect("deck", ("strip_0", 0), None)],
    );
    let h = headroom_errors(&report)[0].actual_mm;
    assert!(h < 46.0, "tilted: {h} mm");
    // A deck with a hole: a 12 x 12 deck of 15 mm cells 60 mm up with the
    // central 6 x 6 cells (90 mm) removed; the hole floor (layer 0, listed
    // first) covers only the hole's central 2 x 2 cells, 30 mm from its rim,
    // so its best headroom is above 60 mm; the rim floor reaches 60 mm.
    let mut holed = grid_deck("deck", 1, 0.0, 0.0, 12, 12, CELL, LOW_GAP);
    holed.indices = holed
        .indices
        .chunks(6)
        .enumerate()
        .filter(|(c, _)| {
            let (i, j) = (c % 12, c / 12);
            !((3..9).contains(&i) && (3..9).contains(&j))
        })
        .flat_map(|(_, q)| q.to_vec())
        .collect();
    assert_eq!(holed.indices.len() / 3, 2 * (144 - 36));
    let scene = scene_of(vec![
        grid_deck("hole_floor", 0, 5.0 * CELL, 5.0 * CELL, 2, 2, CELL, 0.0),
        grid_deck("rim_floor", 0, 0.0, 0.0, 12, 12, CELL, 0.0),
        holed,
    ]);
    check_error_scene(
        "holed",
        table,
        &scene,
        &[expect("deck", ("rim_floor", 0), Some(60.0))],
    );
    // Without the rim floor the hole floor alone is still too low, at a
    // headroom above 60 mm.
    let mut only_hole = scene.clone();
    only_hole.meshes.remove(1);
    let report = check_error_scene(
        "holed, hole floor only",
        table,
        &only_hole,
        &[expect("deck", ("hole_floor", 0), None)],
    );
    let h = headroom_errors(&report)[0].actual_mm;
    assert!(h > 60.5, "hole floor: {h} mm");
}

/// Eight 60 mm floor patches (layer 0) 200 mm apart along x, one ceiling
/// mesh (layer 1) of patches over them at `heights[k]` (m).
fn patches(heights: &[f32]) -> GltfScene {
    let mut meshes: Vec<MeshData> = (0..heights.len())
        .map(|k| {
            grid_deck(
                &format!("patch_{k}"),
                0,
                k as f32 * 0.2,
                0.0,
                4,
                4,
                CELL,
                0.0,
            )
        })
        .collect();
    let ceiling = heights
        .iter()
        .enumerate()
        .map(|(k, &z)| grid_deck("c", 1, k as f32 * 0.2, 0.0, 4, 4, CELL, z))
        .collect();
    meshes.push(merged("deck", 1, ceiling));
    scene_of(meshes)
}

/// SPEC 2.8 (#231, scene c): the minimum is reached only by the last
/// support in triangle order (late), or by the first support with a late
/// support tying it exactly (the message names the first); also within one
/// floor mesh, with the ceiling's single low vertex at the far end (or at
/// both ends) of the triangle order.
#[test]
fn spec_2_8_kp109_minimum_reached_late_or_first_with_a_late_tie() {
    let (table, _) = apex();
    let mut late = [0.070f32; 8];
    late[7] = 0.055;
    check_error_scene(
        "late minimum",
        table,
        &patches(&late),
        &[expect("deck", ("patch_7", 0), Some(55.0))],
    );
    let mut tie = [0.070f32; 8];
    tie[0] = 0.055;
    tie[7] = 0.055;
    check_error_scene(
        "first minimum, late tie",
        table,
        &patches(&tie),
        &[expect("deck", ("patch_0", 0), Some(55.0))],
    );
    // Ties at every patch but a slightly lower middle one.
    let mut mid = [0.065f32; 8];
    mid[4] = 0.0649;
    check_error_scene(
        "middle minimum among near-ties",
        table,
        &patches(&mid),
        &[expect("deck", ("patch_4", 0), Some(64.9))],
    );
    // One floor mesh; the ceiling at 70 mm with its last vertex (far end of
    // the triangle order) lowered to 50 mm, then also its first vertex.
    let (nx, ny) = (30u32, 6u32);
    let last = ((nx + 1) * (ny + 1) - 1) as usize;
    for (what, low) in [("far end", vec![last]), ("both ends", vec![0, last])] {
        let scene = scene_of(vec![
            grid_deck("floor", 0, 0.0, 0.0, nx, ny, CELL, 0.0),
            reshaped(
                grid_deck("deck", 1, 0.0, 0.0, nx, ny, CELL, 0.0),
                |_, _, _, k| if low.contains(&k) { 0.050 } else { 0.070 },
            ),
        ]);
        let report = check_error_scene(what, table, &scene, &[expect("deck", ("floor", 0), None)]);
        let h = headroom_errors(&report)[0].actual_mm;
        assert!(h < 69.0, "{what}: {h} mm");
    }
}

/// SPEC 2.8 (#231, scene d): two ceiling meshes at different heights over
/// the same floor give one error each: side by side (layers 1 and 2, 50
/// and 70 mm, 60 mm apart in plan so neither reaches a ball on the other),
/// and stacked (the upper one's minimum is over the lower deck).
#[test]
fn spec_2_8_kp109_two_ceiling_meshes_one_error_each() {
    let (table, _) = apex();
    let side_by_side = scene_of(vec![
        grid_deck("floor", 0, 0.0, 0.0, 20, 6, CELL, 0.0),
        grid_deck("deck_lo", 1, 0.0, 0.0, 8, 6, CELL, 0.050),
        grid_deck("deck_hi", 2, 12.0 * CELL, 0.0, 8, 6, CELL, 0.070),
    ]);
    check_error_scene(
        "side by side",
        table,
        &side_by_side,
        &[
            expect("deck_lo", ("floor", 0), Some(50.0)),
            expect("deck_hi", ("floor", 0), Some(70.0)),
        ],
    );
    let stacked = scene_of(vec![
        grid_deck("floor", 0, 0.0, 0.0, 10, 6, CELL, 0.0),
        grid_deck("deck_hi", 2, 0.0, 0.0, 10, 6, CELL, 0.110),
        grid_deck("deck_lo", 1, 0.0, 0.0, 10, 6, CELL, 0.050),
    ]);
    check_error_scene(
        "stacked",
        table,
        &stacked,
        &[
            expect("deck_lo", ("floor", 0), Some(50.0)),
            expect("deck_hi", ("deck_lo", 1), Some(60.0)),
        ],
    );
}

/// SPEC 2.8 (#231, scene e): the vertical gap decreases monotonically along
/// the support order, so every later support improves on the earlier ones:
/// ten floor strips (layer 0) at 0, 2, ..., 18 mm under a flat 70 mm deck
/// (the last strip is named, 52 mm), and one floor mesh rising along its
/// triangle order under a flat 75 mm deck.
#[test]
fn spec_2_8_kp109_gap_decreasing_along_the_support_order() {
    let (table, _) = apex();
    let mut meshes: Vec<MeshData> = (0..10)
        .map(|s| {
            grid_deck(
                &format!("strip_{s}"),
                0,
                s as f32 * 2.0 * CELL,
                0.0,
                2,
                6,
                CELL,
                s as f32 * 0.002,
            )
        })
        .collect();
    meshes.push(grid_deck("deck", 1, 0.0, 0.0, 20, 6, CELL, 0.070));
    check_error_scene(
        "rising strips",
        table,
        &scene_of(meshes),
        &[expect("deck", ("strip_9", 0), Some(52.0))],
    );
    // Rising within one mesh: z = 2 mm per row + 0.2 mm per column, so each
    // vertex (and triangle) later in the order is higher.
    let (nx, ny) = (10u32, 10u32);
    let scene = scene_of(vec![
        reshaped(
            grid_deck("ramp_floor", 0, 0.0, 0.0, nx, ny, CELL, 0.0),
            move |_, _, _, k| {
                let (i, j) = (k as u32 % (nx + 1), k as u32 / (nx + 1));
                0.002 * j as f32 + 0.0002 * i as f32
            },
        ),
        grid_deck("deck", 1, 0.0, 0.0, nx, ny, CELL, 0.075),
    ]);
    let report = check_error_scene(
        "rising mesh",
        table,
        &scene,
        &[expect("deck", ("ramp_floor", 0), None)],
    );
    let h = headroom_errors(&report)[0].actual_mm;
    assert!(h < 55.0, "rising mesh: {h} mm");
}

/// SPEC 2.8 (#231, scene f): the minimum sample is rejected, so the true
/// answer comes from a later, worse support. On the apex manifest:
/// * the declared `jumbo_xl_resin` (79.4 mm) is restricted to layer 0, so
///   its minimum over the layer-2 floor (50 mm, listed first) does not
///   count and the layer-0 floor (70 mm) is named; the unrestricted
///   explicit 79.4 mm ball names the layer-2 floor;
/// * a floor inside the `ramp_entry` 0 -> 2 transition zone (listed first)
///   under a 40 mm layer-2 ceiling patch: the open floor (70 mm) is named;
/// * a floor in a 40 mm slot between two walls (the ball does not fit;
///   listed first) under a 30 mm ceiling patch: the open floor is named.
#[test]
fn spec_2_8_kp109_rejected_minimum_names_a_later_worse_support() {
    let (table, _) = apex();
    let two_patches = |name: &str, layer: u8, a: (f32, f32, u32, u32, f32), z_open: f32| {
        merged(
            name,
            layer,
            vec![
                grid_deck("a", layer, a.0, a.1, a.2, a.3, CELL, a.4),
                grid_deck("b", layer, 0.0, 0.0, 6, 6, CELL, z_open),
            ],
        )
    };
    // Restriction.
    let restricted = scene_of(vec![
        grid_deck("floor_l2", 2, 0.3, 0.0, 6, 6, CELL, 0.0),
        grid_deck("floor_l0", 0, 0.0, 0.0, 6, 6, CELL, 0.0),
        two_patches("deck", 1, (0.3, 0.0, 6, 6, 0.050), 0.070),
    ]);
    check_error_scene(
        "restriction",
        table,
        &restricted,
        &[expect("deck", ("floor_l2", 2), Some(50.0))],
    );
    let (legacy, ls) = run(table, &restricted, None, SpatialIndex::Legacy);
    let (fine, fs) = run(table, &restricted, None, SpatialIndex::Fine);
    assert_eq!(legacy, fine, "restriction, declared profiles");
    assert!(fs.pairs_examined <= ls.pairs_examined);
    assert_eq!(
        run(table, &restricted, None, SpatialIndex::Fine),
        (fine.clone(), fs)
    );
    assert_headroom(
        "restriction, declared profiles",
        &fine,
        "jumbo_xl_resin",
        &[expect("deck", ("floor_l0", 0), Some(70.0))],
    );
    // Transition zone (`ramp_entry`: x 240-290 mm, y 640-680 mm, z 0-30
    // mm, layer 0 -> 2).
    let zone = scene_of(vec![
        grid_deck("zone_floor", 0, 0.25, 0.645, 2, 1, CELL, 0.0),
        grid_deck("open_floor", 0, 0.0, 0.0, 6, 6, CELL, 0.0),
        two_patches("deck", 2, (0.25, 0.645, 2, 1, 0.040), 0.070),
    ]);
    check_error_scene(
        "transition zone",
        table,
        &zone,
        &[expect("deck", ("open_floor", 0), Some(70.0))],
    );
    // A slot 40 mm wide between two walls (layer 0).
    let slot = scene_of(vec![
        grid_deck("slot_floor", 0, 0.5, 0.0, 2, 8, 0.02, 0.0),
        wall_x("wall_l", 0, 0.5, -0.05, 0.21),
        wall_x("wall_r", 0, 0.54, -0.05, 0.21),
        grid_deck("open_floor", 0, 0.0, 0.0, 6, 6, CELL, 0.0),
        merged(
            "deck",
            1,
            vec![
                grid_deck("a", 1, 0.5, 0.0, 2, 8, 0.02, 0.030),
                grid_deck("b", 1, 0.0, 0.0, 6, 6, CELL, 0.070),
            ],
        ),
    ]);
    check_error_scene(
        "slot",
        table,
        &slot,
        &[expect("deck", ("open_floor", 0), Some(70.0))],
    );
}

// ---------------------------------------------- KP-109 property test

/// One random mesh of a small too-low-deck scene.
#[derive(Debug, Clone)]
struct ArbMesh {
    nx: u32,
    ny: u32,
    cell_mm: u32,
    /// Plan offset as a fraction (per mille) of the first mesh's extent.
    offset: (u32, u32),
    layer_step: u8,
    level: u8,
    jitter: Vec<i8>,
}

fn arb_mesh() -> impl Strategy<Value = ArbMesh> {
    (
        1u32..=12,
        1u32..=12,
        10u32..=30,
        (0u32..1000, 0u32..1000),
        0u8..3,
        0u8..3,
        proptest::collection::vec(-3i8..=3, 169),
    )
        .prop_map(
            |(nx, ny, cell_mm, offset, layer_step, level, jitter)| ArbMesh {
                nx,
                ny,
                cell_mm,
                offset,
                layer_step,
                level,
                jitter,
            },
        )
}

/// Builds the scene: mesh 0 (`m0`) at z = 0 on `layer0`, mesh 1 (`m1`) at
/// the deck gap on another layer, optional mesh 2 (`m2`) at 0, 1 or 2 gaps
/// on any layer; each vertex height jittered by k · `q_mm` (k in -3..=3).
/// Meshes 1 and 2 start inside mesh 0's plan, so the deck overlaps it.
fn arb_scene(layer0: u8, meshes: &[ArbMesh], gap_mm: f32, q_mm: f32) -> GltfScene {
    let extent = |m: &ArbMesh| (m.nx * m.cell_mm, m.ny * m.cell_mm);
    let (ex, ey) = extent(&meshes[0]);
    let mut layer = layer0;
    let out = meshes
        .iter()
        .enumerate()
        .map(|(k, m)| {
            let (ox, oy, level) = if k == 0 {
                (0, 0, 0)
            } else {
                (
                    m.offset.0 * ex / 1000,
                    m.offset.1 * ey / 1000,
                    if k == 1 { 1 } else { m.level },
                )
            };
            if k == 1 {
                layer = (layer0 + 1 + m.layer_step % 2) % 3;
            } else if k == 2 {
                layer = m.layer_step;
            }
            let base_mm = level as f32 * gap_mm;
            let mesh = grid_deck(
                &format!("mesh_{k}"),
                layer,
                ox as f32 / 1000.0,
                oy as f32 / 1000.0,
                m.nx,
                m.ny,
                m.cell_mm as f32 / 1000.0,
                0.0,
            );
            reshaped(mesh, |_, _, _, v| {
                (base_mm + m.jitter[v] as f32 * q_mm) / 1000.0
            })
        })
        .collect();
    scene_of(out)
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 128,
        rng_seed: RngSeed::Fixed(231),
        ..ProptestConfig::default()
    })]

    /// SPEC 2.8 (#231): on random small too-low-deck scenes (grids of 1-12
    /// cells, per-vertex jitter quantised at 0, 0.1 or 1 mm so ties are
    /// frequent, deck gap 20-90 mm, 2-3 meshes on random layers), `Legacy`
    /// and `Fine` give equal reports with `fine <= legacy` pairs, for the
    /// 79.4 mm ball, the three test diameters and the declared profiles; a
    /// repeated run gives an identical report and `IndexStats` (R3); and
    /// with two meshes and a gap below 78 mm the deck really is reported.
    #[test]
    fn spec_2_8_kp109_prop_legacy_equals_fine_on_too_low_decks(
        layer0 in 0u8..3,
        meshes in proptest::collection::vec(arb_mesh(), 2..=3),
        gap_tenths in 200u32..=900,
        q in proptest::sample::select(vec![0.0f32, 0.1, 1.0]),
    ) {
        let (table, _) = apex();
        let gap_mm = gap_tenths as f32 / 10.0;
        let scene = arb_scene(layer0, &meshes, gap_mm, q);
        for d in [Some(&XL[..]), Some(&DIAMETERS[..]), None] {
            let (legacy, ls) = run(table, &scene, d, SpatialIndex::Legacy);
            let (fine, fs) = run(table, &scene, d, SpatialIndex::Fine);
            prop_assert_eq!(&legacy, &fine, "{:?}", d);
            prop_assert!(fs.pairs_examined <= ls.pairs_examined,
                "{:?}: Fine {} > Legacy {}", d, fs.pairs_examined, ls.pairs_examined);
            prop_assert_eq!(run(table, &scene, d, SpatialIndex::Fine), (fine.clone(), fs));
            prop_assert_eq!(run(table, &scene, d, SpatialIndex::Legacy), (legacy, ls));
            if d == Some(&XL[..]) && meshes.len() == 2 && gap_mm + 6.0 * q < 78.0 {
                prop_assert!(
                    headroom_errors(&fine).iter().any(|e| e.name == "mesh_1"),
                    "gap {} mm, q {} mm: {:#?}", gap_mm, q, fine.errors
                );
            }
        }
    }
}

// ---------------------------------------------- KP-109 robustness

/// R11 (SPEC 2.8, #231): too-low-deck scenes with coincident ceiling (and
/// floor) triangles, zero-area triangles, a single triangle each, and
/// extents of 1e6 m cells do not panic or hang, and `Legacy` equals `Fine`.
#[test]
fn spec_2_8_kp109_robustness_of_too_low_decks() {
    let (table, _) = apex();
    let copies = |name: &str, layer: u8, tri: [[f32; 3]; 3], n: u32| {
        let indices = (0..n).flat_map(|_| [0u32, 1, 2]).collect();
        collider(name, layer, tri.to_vec(), indices)
    };
    // 1000 coincident ceiling copies 50 mm over 1000 coincident floor
    // copies and a floor grid.
    let coincident = scene_of(vec![
        copies(
            "floor_copies",
            0,
            [[0.1, 0.1, 0.0], [0.16, 0.1, 0.0], [0.1, 0.16, 0.0]],
            1000,
        ),
        grid_deck("floor", 0, 0.0, 0.0, 10, 10, 0.02, 0.0),
        copies(
            "ceiling_copies",
            1,
            [[0.0, 0.0, 0.05], [0.2, 0.0, 0.05], [0.0, 0.2, 0.05]],
            1000,
        ),
    ]);
    check_error_scene(
        "coincident",
        table,
        &coincident,
        &[expect("ceiling_copies", ("floor_copies", 0), Some(50.0))],
    );
    // Zero-area triangles in both meshes, mixed with real ones.
    let with_degenerates = |m: MeshData| {
        let n = m.positions.len() as u32;
        let mut m = m;
        let z = m.positions[0][2];
        m.positions.extend([
            [0.05, 0.05, z],
            [0.09, 0.05, z],
            [0.13, 0.05, z],
            [0.07, 0.07, z + 0.01],
        ]);
        // Repeated vertex, collinear, a point, a vertical needle; first.
        let mut indices = vec![
            n,
            n,
            n + 1,
            n,
            n + 1,
            n + 2,
            n + 3,
            n + 3,
            n + 3,
            n,
            n,
            n + 3,
        ];
        indices.extend(m.indices);
        m.indices = indices;
        m.normals = vec![[0.0, 0.0, 1.0]; m.positions.len()];
        m
    };
    let degenerate = scene_of(vec![
        with_degenerates(grid_deck("floor", 0, 0.0, 0.0, 8, 8, 0.02, 0.0)),
        with_degenerates(grid_deck("deck", 1, 0.0, 0.0, 8, 8, 0.02, 0.055)),
    ]);
    check_error_scene(
        "zero area",
        table,
        &degenerate,
        &[expect("deck", ("floor", 0), Some(55.0))],
    );
    // A single triangle each.
    let single = scene_of(vec![
        collider(
            "floor",
            0,
            vec![[0.0, 0.0, 0.0], [0.2, 0.0, 0.0], [0.0, 0.2, 0.0]],
            vec![0, 1, 2],
        ),
        collider(
            "deck",
            1,
            vec![[0.0, 0.0, 0.05], [0.2, 0.0, 0.05], [0.0, 0.2, 0.05]],
            vec![0, 1, 2],
        ),
    ]);
    check_error_scene(
        "single",
        table,
        &single,
        &[expect("deck", ("floor", 0), Some(50.0))],
    );
    // 1e6 m cells: a 2 x 2 floor and a 2 x 2 deck 50 mm up (NaN-free, f32
    // positions up to 2e6 m), plus a small too-low pair at the origin.
    let huge = scene_of(vec![
        grid_deck("huge_floor", 0, -1.0e6, -1.0e6, 2, 2, 1.0e6, 0.0),
        grid_deck("huge_deck", 1, -1.0e6, -1.0e6, 2, 2, 1.0e6, 0.05),
        grid_deck("small_floor", 2, 0.0, 0.0, 4, 4, CELL, 0.0),
        grid_deck("small_deck", 3, 0.0, 0.0, 4, 4, CELL, 0.06),
    ]);
    assert_same_verdicts("huge", table, &huge);
    let (legacy, ls) = run(table, &huge, Some(&XL), SpatialIndex::Legacy);
    let (fine, fs) = run(table, &huge, Some(&XL), SpatialIndex::Fine);
    assert_eq!(legacy, fine, "huge");
    assert!(fs.pairs_examined <= ls.pairs_examined, "huge");
    assert_eq!(
        run(table, &huge, Some(&XL), SpatialIndex::Fine),
        (fine.clone(), fs)
    );
    println!(
        "kp109 huge: Legacy {} pairs, Fine {} pairs: {:#?}",
        ls.pairs_examined,
        fs.pairs_examined,
        headroom_errors(&fine)
    );
    assert!(
        headroom_errors(&fine)
            .iter()
            .any(|e| e.name == "small_deck"),
        "huge: {:#?}",
        fine.errors
    );
}
