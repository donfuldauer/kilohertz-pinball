//! Subphase 2.8 property tests of the clearance validator: it never panics
//! on generated tables (random layouts, profiles, restrictions, wireforms,
//! VUKs and ball diameters), and the width checks are monotonic in the ball
//! diameter over 15.88–79.4 mm.

mod support;

use pin_assets::{GltfScene, LoadedTable, Vfs, load_gltf, load_table};
use pin_core::Material;
use pin_core::schema::layout::{Deck, Lane, LaneKind, Playfield, Post, Wall};
use pin_core::schema::table::BallRestriction;
use pin_core::schema::{LayoutManifest, TableManifest};
use pin_tablegen::clearance::{
    ClearanceReport, Feature, validate_clearance, validate_clearance_diameters,
};
use pin_tablegen::layout::{generate, write_bundle};
use proptest::prelude::*;
use std::collections::BTreeSet;
use std::sync::OnceLock;
use support::*;

const W: f32 = 514.0;
const L: f32 = 1070.0;
const D_MIN: f32 = 0.01588;
const D_MAX: f32 = 0.0794;

fn point() -> impl Strategy<Value = [f32; 2]> {
    (0.0f32..W, 0.0f32..L).prop_map(|(x, y)| [x, y])
}

fn wall() -> impl Strategy<Value = Wall> {
    proptest::collection::vec(point(), 2..5).prop_map(|mut points| {
        for j in 1..points.len() {
            if points[j] == points[j - 1] {
                points[j][0] += 1.0;
            }
        }
        Wall {
            points,
            closed: false,
            material: Material::Wood,
        }
    })
}

fn post() -> impl Strategy<Value = Post> {
    (point(), 2.0f32..60.0, any::<bool>()).prop_map(|(position, radius, rubber)| Post {
        position,
        radius,
        rubber,
    })
}

fn lane() -> impl Strategy<Value = Lane> {
    (
        point(),
        0.0f32..core::f32::consts::TAU,
        30.0f32..400.0,
        15.0f32..100.0,
    )
        .prop_map(|(p, a, len, width)| Lane {
            kind: LaneKind::Orbit,
            points: vec![p, [p[0] + len * libm::cosf(a), p[1] + len * libm::sinf(a)]],
            width,
        })
}

fn deck() -> impl Strategy<Value = Deck> {
    (
        1u8..8,
        5.0f32..120.0,
        point(),
        20.0f32..300.0,
        20.0f32..300.0,
    )
        .prop_map(|(layer, z, p, w, h)| Deck {
            layer,
            z,
            outline: vec![p, [p[0] + w, p[1]], [p[0] + w, p[1] + h], [p[0], p[1] + h]],
            material: Material::Acrylic,
        })
}

fn arb_layout() -> impl Strategy<Value = LayoutManifest> {
    (
        proptest::collection::vec(wall(), 0..4),
        proptest::collection::vec(post(), 0..5),
        proptest::collection::vec(lane(), 0..3),
        proptest::collection::vec(deck(), 0..3),
    )
        .prop_map(|(walls, posts, lanes, decks)| LayoutManifest {
            schema_version: 1,
            playfield: Playfield {
                width: W,
                length: L,
                wall_height: 28.0,
            },
            walls,
            posts,
            lanes,
            ramps: vec![],
            decks,
            inserts: vec![],
        })
}

fn restriction() -> impl Strategy<Value = BallRestriction> {
    (
        prop_oneof![
            Just("jumbo_xl_resin"),
            Just("mini_steel"),
            Just("standard_steel")
        ],
        proptest::collection::vec(0u8..8, 0..3),
        proptest::collection::vec(point(), 0..6),
    )
        .prop_map(|(p, layers, area)| BallRestriction {
            profile: p.to_owned(),
            layers,
            area,
        })
}

/// Extra base entries: a wireform with a random gauge and a VUK.
fn extras() -> impl Strategy<Value = String> {
    (
        5.0f32..90.0,
        0u8..3,
        point(),
        (-100.0f32..100.0, -100.0f32..100.0, -50.0f32..150.0),
        0u8..3,
    )
        .prop_map(|(gauge, layer, at, off, exit_layer)| {
            format!(
                "[[splines]]\nid = \"rail\"\nkind = \"wireform\"\nlayer = {layer}\ngauge = {gauge:?}\n\
                 points = [[100.0, 300.0, 40.0], [200.0, 500.0, 60.0], [300.0, 700.0, 40.0]]\n\
                 [[mechs]]\nid = \"scoop\"\nkind = \"vuk\"\nposition = [{:?}, {:?}, 0.0]\n\
                 layer = {layer}\nexit_layer = {exit_layer}\nexit_offset = [{:?}, {:?}, {:?}]\n",
                at[0], at[1], off.0, off.1, off.2
            )
        })
}

fn sorted(r: &ClearanceReport) -> bool {
    let key = |e: &pin_tablegen::clearance::ClearanceError| {
        (
            e.file.clone(),
            e.path.clone(),
            e.profile.clone(),
            e.message.clone(),
        )
    };
    r.errors.windows(2).all(|w| key(&w[0]) <= key(&w[1]))
        && r.warnings.windows(2).all(|w| key(&w[0]) <= key(&w[1]))
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    /// Generated tables with random profiles and restrictions never make
    /// the validator panic; its findings are sorted, named and finite.
    #[test]
    fn validator_never_panics_on_generated_tables(
        layout in arb_layout(),
        profiles in proptest::sample::subsequence(
            vec!["standard_steel", "mini_steel", "jumbo_steel", "jumbo_xl_resin", "ceramic_powerball"], 0..5),
        restrict in proptest::collection::vec(restriction(), 0..3),
        extra in extras(),
        diameters in proptest::collection::vec(D_MIN..=D_MAX, 1..4),
    ) {
        let mut base = apex_base();
        base.balls.profiles = profiles.iter().map(|s| (*s).to_owned()).collect();
        base.balls.restrict = restrict.clone();
        base.splines.clear();
        base.layer_transitions.clear();
        base.mechs.clear();
        // Areas with < 3 points or NaN are validator errors, but valid TOML.
        let text = format!("{}\n{extra}", base.to_toml_string().unwrap());
        let base = TableManifest::from_toml_str(&text).unwrap();
        let generated = generate(&layout, &base).unwrap_or_else(|e| panic!("{e}\n{layout:#?}"));
        let dir = bundle_dir("clearance_props");
        write_bundle(&generated, &dir).unwrap();
        let vfs = Vfs::open(&dir).unwrap();
        // Restrictions name random layers: the loader may refuse some.
        if let Ok(table) = load_table(&vfs) {
            let scene = load_gltf(&vfs, "playfield.glb").unwrap();
            let r = validate_clearance(&table, &scene);
            prop_assert!(sorted(&r));
            for e in r.errors.iter().chain(&r.warnings) {
                prop_assert!(!e.name.is_empty() && !e.path.is_empty() && !e.message.is_empty());
                if e.feature != Feature::Restriction {
                    prop_assert!(e.required_mm.is_finite() && e.actual_mm.is_finite(), "{e:?}");
                }
            }
            for e in &r.errors {
                prop_assert!(e.feature == Feature::Restriction || !e.profile.is_empty());
            }
            // Deterministic (Debug: NaN fields compare unequal).
            prop_assert_eq!(format!("{:?}", validate_clearance(&table, &scene)), format!("{r:?}"));
            let rd = validate_clearance_diameters(&table, &scene, &diameters);
            prop_assert!(sorted(&rd));
            prop_assert!(rd.errors.iter().all(|e| e.feature != Feature::Restriction));
        }
        std::fs::remove_dir_all(dir).unwrap();
    }
}

/// The fuller fixture (lanes, posts, two decks, a closed wall) with two
/// flippers, a pocketed scoop and a wireform, loaded once.
fn rich() -> &'static (LoadedTable, GltfScene) {
    static RICH: OnceLock<(LoadedTable, GltfScene)> = OnceLock::new();
    RICH.get_or_init(|| {
        let mut base = apex_base();
        base.splines.clear();
        base.layer_transitions.clear();
        let mut lay = layout("rich_layout.toml");
        lay.ramps.clear();
        // Its 20 mm orbit is too narrow for any but the mini ball.
        lay.lanes[3].width = 30.0;
        lay.walls.push(Wall {
            points: vec![[135.0, 490.0], [135.0, 435.0], [165.0, 435.0], [165.0, 490.0]],
            closed: false,
            material: Material::Steel,
        });
        let text = format!(
            "{}\n[[mechs]]\nid = \"scoop\"\nkind = \"vuk\"\nposition = [150.0, 450.0, 0.0]\nexit_layer = 0\nexit_offset = [0.0, 0.0, 200.0]\n",
            base.to_toml_string().unwrap()
        );
        let base = TableManifest::from_toml_str(&text).unwrap();
        let g = generate(&lay, &base).unwrap_or_else(|e| panic!("{e}"));
        let (dir, table, scene) = load(&g, "clearance_rich");
        std::fs::remove_dir_all(dir).unwrap();
        (table, scene)
    })
}

/// (feature, path) of every error.
fn failing(table: &LoadedTable, scene: &GltfScene, d: f32) -> BTreeSet<(Feature, String)> {
    validate_clearance_diameters(table, scene, &[d])
        .errors
        .into_iter()
        .map(|e| (e.feature, e.path))
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    /// Lanes, flipper gaps, scoops and headroom only get harder for a
    /// bigger ball: whatever fails for `d1` fails for every `d2 > d1`.
    #[test]
    fn width_checks_are_monotonic_in_the_diameter(a in D_MIN..=D_MAX, b in D_MIN..=D_MAX) {
        let (d1, d2) = if a <= b { (a, b) } else { (b, a) };
        let (table, scene) = rich();
        let monotonic = |s: BTreeSet<(Feature, String)>| -> BTreeSet<(Feature, String)> {
            s.into_iter()
                .filter(|(f, _)| matches!(f, Feature::Lane | Feature::FlipperGap | Feature::Scoop | Feature::DeckHeadroom))
                .collect()
        };
        let f1 = monotonic(failing(table, scene, d1));
        let f2 = monotonic(failing(table, scene, d2));
        prop_assert!(f1.is_subset(&f2), "{d1}: {f1:?}\n{d2}: {f2:?}");
    }
}

#[test]
fn the_rich_table_carries_the_standard_ball_but_not_the_xl_ball() {
    let (table, scene) = rich();
    let steel = validate_clearance_diameters(table, scene, &[0.027]);
    assert!(steel.is_ok(), "{:#?}", steel.errors);
    let xl = failing(table, scene, D_MAX);
    for f in [Feature::Lane, Feature::Scoop, Feature::DeckHeadroom] {
        assert!(xl.iter().any(|(g, _)| *g == f), "{f:?}: {xl:?}");
    }
}

// ---------------------------------------------------------------- KP-054
// Issue #135: the finer spatial index gives the verdicts of the reference
// uniform grid on random hand-built scenes (SPEC §4 Subphase 2.8).

use pin_assets::{MeshData, NodeExtras};
use pin_tablegen::clearance::{SpatialIndex, validate_clearance_indexed};
use std::collections::BTreeMap;

/// The Appendix A.3 apex table (standard and powerball profiles, the XL
/// ball restricted to layer 0), loaded once, with its generated scene.
fn kp054_apex() -> &'static (LoadedTable, GltfScene) {
    static APEX: OnceLock<(LoadedTable, GltfScene)> = OnceLock::new();
    APEX.get_or_init(|| {
        let (dir, table, scene) = load(&gen_apex("apex_layout.toml"), "kp054_props");
        std::fs::remove_dir_all(dir).unwrap();
        (table, scene)
    })
}

/// Fixed-seed SplitMix64 (the case seed comes from proptest; R3).
struct Mix(u64);

impl Mix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `[a, b)`.
    fn f(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * ((self.next() >> 40) as f32 / (1u64 << 24) as f32)
    }
    fn below(&mut self, n: u32) -> u32 {
        (self.next() % u64::from(n.max(1))) as u32
    }
}

/// Deck gaps (mm) around the thresholds of the 27 mm and 79.4 mm balls
/// (`r + 1 mm` = 14.5 / 40.7 mm, `d + 1 mm` = 28 / 80.4 mm).
const KP054_GAPS: [f32; 14] = [
    10.0, 14.5, 20.0, 27.0, 27.9, 28.1, 40.7, 41.0, 65.0, 79.4, 80.3, 80.5, 100.0, 150.0,
];

/// A random scene: what to put where. Expanded by [`kp054_scene`].
#[derive(Clone, Debug)]
struct Kp054Scene {
    seed: u64,
    /// Everything within 120 mm (clustered) or 1 m (spread).
    clustered: bool,
    /// Walkable main-deck patches (layer 0): cells in x, y, cell size (mm),
    /// slopes dz/dx, dz/dy (flat or slightly tilted).
    floors: Vec<(u32, u32, f32, f32, f32)>,
    /// Upper decks: layer, gap index (`KP054_GAPS`), gap jitter (mm), cells
    /// in x, y, cell size (mm).
    decks: Vec<(u8, usize, f32, u32, u32, f32)>,
    walls: u32,
    randoms: u32,
    coincident: u32,
    degenerate: u32,
    /// Also keep the generated apex playfield meshes.
    with_base: bool,
}

fn kp054_spec() -> impl Strategy<Value = Kp054Scene> {
    let slope = prop_oneof![Just(0.0f32), -0.25f32..0.25];
    let floor = (1u32..=16, 1u32..=16, 2.0f32..400.0, slope.clone(), slope);
    let deck = (
        1u8..=7,
        0..KP054_GAPS.len(),
        -0.5f32..0.5,
        1u32..=16,
        1u32..=16,
        2.0f32..400.0,
    );
    (
        any::<u64>(),
        any::<bool>(),
        proptest::collection::vec(floor, 1..=2),
        proptest::collection::vec(deck, 1..=2),
        0u32..40,
        0u32..300,
        0u32..60,
        0u32..30,
        proptest::bool::weighted(0.25),
    )
        .prop_map(
            |(
                seed,
                clustered,
                floors,
                decks,
                walls,
                randoms,
                coincident,
                degenerate,
                with_base,
            )| {
                Kp054Scene {
                    seed,
                    clustered,
                    floors,
                    decks,
                    walls,
                    randoms,
                    coincident,
                    degenerate,
                    with_base,
                }
            },
        )
}

fn kp054_mesh(name: String, layer: u8, positions: Vec<[f32; 3]>, indices: Vec<u32>) -> MeshData {
    MeshData {
        name,
        normals: vec![[0.0, 0.0, 1.0]; positions.len()],
        positions,
        uvs: vec![],
        indices,
        material: None,
        transform: [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ],
        extras: NodeExtras {
            collision_layer: Some(layer),
            collider: true,
            ..NodeExtras::default()
        },
    }
}

/// A grid patch from `o` (m): `nx` × `ny` cells of `cell` m, z = z0 +
/// sx·dx + sy·dy, two triangles per cell.
#[allow(clippy::too_many_arguments)]
fn kp054_grid(
    name: String,
    layer: u8,
    o: [f32; 2],
    nx: u32,
    ny: u32,
    cell: f32,
    z0: f32,
    (sx, sy): (f32, f32),
) -> MeshData {
    let mut positions = Vec::new();
    for j in 0..=ny {
        for i in 0..=nx {
            let (dx, dy) = (i as f32 * cell, j as f32 * cell);
            positions.push([o[0] + dx, o[1] + dy, z0 + sx * dx + sy * dy]);
        }
    }
    let row = nx + 1;
    let mut indices = Vec::new();
    for j in 0..ny {
        for i in 0..nx {
            let a = j * row + i;
            indices.extend([a, a + 1, a + row + 1, a, a + row + 1, a + row]);
        }
    }
    kp054_mesh(name, layer, positions, indices)
}

/// Expands a [`Kp054Scene`] into collider meshes (metres).
fn kp054_scene(spec: &Kp054Scene, base: &GltfScene) -> GltfScene {
    let mut rng = Mix(spec.seed);
    let extent = if spec.clustered { 0.12 } else { 1.0 };
    let o = [0.05f32, 0.05];
    let at = |rng: &mut Mix| [o[0] + rng.f(0.0, extent), o[1] + rng.f(0.0, extent)];
    let mut scene = if spec.with_base {
        base.clone()
    } else {
        GltfScene::default()
    };
    for (k, &(nx, ny, cell, sx, sy)) in spec.floors.iter().enumerate() {
        let p = at(&mut rng);
        let z0 = rng.f(-0.002, 0.002);
        scene.meshes.push(kp054_grid(
            format!("floor{k}"),
            0,
            p,
            nx,
            ny,
            cell * 1e-3,
            z0,
            (sx, sy),
        ));
    }
    for (k, &(layer, gap, jitter, nx, ny, cell)) in spec.decks.iter().enumerate() {
        let p = at(&mut rng);
        let tilt = if rng.below(2) == 0 {
            (0.0, 0.0)
        } else {
            (rng.f(-0.05, 0.05), rng.f(-0.05, 0.05))
        };
        scene.meshes.push(kp054_grid(
            format!("deck{k}"),
            layer,
            p,
            nx,
            ny,
            cell * 1e-3,
            (KP054_GAPS[gap] + jitter) * 1e-3,
            tilt,
        ));
    }
    // Loose triangles, per (category, layer), in a fixed order.
    let mut soup: BTreeMap<(&str, u8), Vec<[f32; 3]>> = BTreeMap::new();
    for _ in 0..spec.walls {
        // Steep: a vertical triangle (|n·z| = 0) or one leaning a little.
        let p = at(&mut rng);
        let len = rng.f(0.002, 0.4);
        let a = rng.f(0.0, core::f32::consts::TAU);
        let h = rng.f(0.005, 0.1);
        let lean = rng.f(-0.3, 0.3) * h;
        let (c, s) = (libm::cosf(a), libm::sinf(a));
        let layer = rng.below(3) as u8;
        soup.entry(("wall", layer)).or_default().extend([
            [p[0], p[1], 0.0],
            [p[0] + len * c, p[1] + len * s, 0.0],
            [p[0] - lean * s, p[1] + lean * c, h],
        ]);
    }
    for _ in 0..spec.randoms {
        let p = at(&mut rng);
        let size = rng.f(0.002, 0.4);
        let z = rng.f(0.0, 0.12);
        let layer = rng.below(4) as u8;
        let flat = rng.below(2) == 0;
        let mut v = [[0.0f32; 3]; 3];
        for q in &mut v {
            let dz = if flat { 0.0 } else { rng.f(-size, size) };
            *q = [p[0] + rng.f(-size, size), p[1] + rng.f(-size, size), z + dz];
        }
        soup.entry(("random", layer)).or_default().extend(v);
    }
    // Coincident copies of triangles already in the scene, on the same or
    // another layer.
    let mut all: Vec<[[f32; 3]; 3]> = Vec::new();
    for m in &scene.meshes {
        if m.transform != kp054_mesh(String::new(), 0, vec![], vec![]).transform {
            continue;
        }
        for t in m.indices.chunks_exact(3) {
            all.push([0, 1, 2].map(|k| m.positions[t[k] as usize]));
        }
    }
    for tris in soup.values() {
        for t in tris.chunks_exact(3) {
            all.push([t[0], t[1], t[2]]);
        }
    }
    if !all.is_empty() {
        for _ in 0..spec.coincident {
            let t = all[rng.below(all.len() as u32) as usize];
            let layer = rng.below(3) as u8;
            let copies = 1 + rng.below(4);
            for _ in 0..copies {
                soup.entry(("coincident", layer)).or_default().extend(t);
            }
        }
    }
    for _ in 0..spec.degenerate {
        let p = at(&mut rng);
        let z = rng.f(0.0, 0.1);
        let d = rng.f(0.002, 0.4);
        let layer = rng.below(3) as u8;
        let tri = match rng.below(3) {
            0 => [[p[0], p[1], z], [p[0], p[1], z], [p[0] + d, p[1], z]],
            1 => [
                [p[0], p[1], z],
                [p[0] + d, p[1], z],
                [p[0] + 2.0 * d, p[1], z],
            ],
            _ => [[p[0], p[1], z]; 3],
        };
        soup.entry(("degenerate", layer)).or_default().extend(tri);
    }
    for ((kind, layer), positions) in soup {
        let indices = (0..positions.len() as u32).collect();
        scene.meshes.push(kp054_mesh(
            format!("{kind}_l{layer}"),
            layer,
            positions,
            indices,
        ));
    }
    scene
}

fn kp054_triangles(scene: &GltfScene) -> usize {
    scene.meshes.iter().map(|m| m.indices.len() / 3).sum()
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// SPEC 2.8 (kp054, issue #135): on random scenes (walkable flat and
    /// tilted main-deck patches, upper decks on other layers at gaps around
    /// the 27 mm and 79.4 mm balls' limits, steep walls, 2–400 mm random
    /// triangles, coincident copies, zero-area triangles, clustered or
    /// spread) the finer index gives exactly the reference grid's report,
    /// for the declared profiles and for explicit diameters, and the plain
    /// entry points use it.
    #[test]
    fn spec_2_8_kp054_fine_index_matches_legacy_on_random_scenes(spec in kp054_spec()) {
        let (table, base) = kp054_apex();
        let scene = kp054_scene(&spec, base);
        let extra = if spec.with_base { kp054_triangles(base) } else { 0 };
        prop_assert!(kp054_triangles(&scene) <= 3000 + extra, "{}", kp054_triangles(&scene));
        let diameters: &[f32] = &[0.01588, 0.0270, 0.0794];
        for d in [None, Some(diameters)] {
            let (legacy, _) = validate_clearance_indexed(table, &scene, d, SpatialIndex::Legacy);
            let (fine, stats) = validate_clearance_indexed(table, &scene, d, SpatialIndex::Fine);
            prop_assert_eq!(&legacy, &fine, "{:?}", d);
            let plain = match d {
                None => validate_clearance(table, &scene),
                Some(d) => validate_clearance_diameters(table, &scene, d),
            };
            prop_assert_eq!(&plain, &fine, "{:?}", d);
            let (_, again) = validate_clearance_indexed(table, &scene, d, SpatialIndex::Fine);
            prop_assert_eq!(stats, again);
        }
    }
}

/// SPEC 2.8 (kp054): the random-scene generator reaches the cases the
/// property is about (a deck too low for the XL ball and one high enough,
/// beside walls and degenerate triangles), so the property is not
/// vacuous.
#[test]
fn spec_2_8_kp054_random_scenes_reach_both_verdicts() {
    let (table, base) = kp054_apex();
    let spec = |gap: usize| Kp054Scene {
        seed: 7,
        clustered: true,
        floors: vec![(8, 8, 20.0, 0.0, 0.0)],
        decks: vec![(1, gap, 0.0, 8, 8, 20.0)],
        walls: 5,
        randoms: 0,
        coincident: 0,
        degenerate: 5,
        with_base: false,
    };
    // Floor and deck (160 mm squares) start in the same 120 mm square, so
    // they overlap by at least 40 mm in x and y.
    let xl: &[f32] = &[0.0794];
    let low = kp054_scene(&spec(8), base); // 65 mm
    let high = kp054_scene(&spec(13), base); // 150 mm
    for (scene, rejected) in [(&low, true), (&high, false)] {
        let (legacy, _) = validate_clearance_indexed(table, scene, Some(xl), SpatialIndex::Legacy);
        let (fine, _) = validate_clearance_indexed(table, scene, Some(xl), SpatialIndex::Fine);
        assert_eq!(legacy, fine);
        assert_eq!(
            fine.errors.iter().any(|e| e.name == "deck0"),
            rejected,
            "{:#?}",
            fine.errors
        );
    }
}
