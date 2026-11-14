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
            points: vec![p, [p[0] + len * a.cos(), p[1] + len * a.sin()]],
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
