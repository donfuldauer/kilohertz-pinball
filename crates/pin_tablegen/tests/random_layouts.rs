//! PLAN 4.3 verification 1 and 6: random valid layouts generate bundles
//! that pass 4.1 validation; random bytes and random (often invalid)
//! layouts never panic.

mod support;

use pin_assets::{Vfs, load_gltf, load_table, validate_gltf_bindings};
use pin_core::Material;
use pin_core::schema::LayoutManifest;
use pin_core::schema::layout::{
    Deck, Insert, InsertShape, Lane, LaneKind, Playfield, Post, Ramp, Wall,
};
use pin_tablegen::layout::{GENERATED_PREFIX, generate, parse_layout_bytes, write_bundle};
use proptest::prelude::*;
use support::*;

const W: f32 = 514.0;
const L: f32 = 1070.0;

fn point() -> impl Strategy<Value = [f32; 2]> {
    (0.0f32..W, 0.0f32..L).prop_map(|(x, y)| [x, y])
}

fn open_wall() -> impl Strategy<Value = Wall> {
    proptest::collection::vec(point(), 2..7).prop_map(|mut points| {
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

/// A regular polygon (simple by construction).
fn closed_wall() -> impl Strategy<Value = Wall> {
    (
        3usize..9,
        60.0f32..450.0,
        60.0f32..1000.0,
        5.0f32..50.0,
        0.0f32..core::f32::consts::TAU,
    )
        .prop_map(|(k, cx, cy, r, rot)| Wall {
            points: (0..k)
                .map(|i| {
                    let a = rot + i as f32 / k as f32 * core::f32::consts::TAU;
                    [cx + r * libm::cosf(a), cy + r * libm::sinf(a)]
                })
                .collect(),
            closed: true,
            material: Material::Plastic,
        })
}

fn post() -> impl Strategy<Value = Post> {
    (point(), 2.0f32..30.0, any::<bool>()).prop_map(|(position, radius, rubber)| Post {
        position,
        radius,
        rubber,
    })
}

fn kind() -> impl Strategy<Value = LaneKind> {
    prop_oneof![
        Just(LaneKind::Inlane),
        Just(LaneKind::Outlane),
        Just(LaneKind::Shooter),
        Just(LaneKind::Orbit)
    ]
}

/// Straight lanes and circular-arc lanes whose radius exceeds the width.
fn lane() -> impl Strategy<Value = Lane> {
    let straight = (
        point(),
        0.0f32..core::f32::consts::TAU,
        30.0f32..500.0,
        15.0f32..50.0,
        kind(),
    )
        .prop_map(|(p, a, len, width, kind)| Lane {
            kind,
            points: vec![p, [p[0] + len * libm::cosf(a), p[1] + len * libm::sinf(a)]],
            width,
        });
    let arc = (
        point(),
        15.0f32..50.0,
        1.0f32..6.0,
        0.0f32..core::f32::consts::TAU,
        0.3f32..4.7,
        any::<bool>(),
        kind(),
    )
        .prop_map(|(c, width, scale, start, sweep, ccw, kind)| {
            let r = width * scale + 10.0;
            let k = (sweep / 0.35).ceil() as usize + 2;
            let dir = if ccw { 1.0 } else { -1.0 };
            Lane {
                kind,
                points: (0..k)
                    .map(|i| {
                        let a = start + dir * sweep * i as f32 / (k - 1) as f32;
                        [c[0] + r * libm::cosf(a), c[1] + r * libm::sinf(a)]
                    })
                    .collect(),
                width,
            }
        });
    prop_oneof![straight, arc]
}

fn insert() -> impl Strategy<Value = Insert> {
    (
        prop_oneof![Just("overdrive_ready"), Just("backglass_logo")],
        point(),
        prop_oneof![
            Just(InsertShape::Circle),
            Just(InsertShape::Arrow),
            Just(InsertShape::Triangle),
            Just(InsertShape::Rect)
        ],
        3.0f32..40.0,
    )
        .prop_map(|(lamp, position, shape, size)| Insert {
            lamp: lamp.to_owned(),
            position,
            shape,
            size,
        })
}

fn deck() -> impl Strategy<Value = Deck> {
    (
        1u8..8,
        10.0f32..100.0,
        point(),
        20.0f32..200.0,
        20.0f32..200.0,
    )
        .prop_map(|(layer, z, p, w, h)| Deck {
            layer,
            z,
            outline: vec![p, [p[0] + w, p[1]], [p[0] + w, p[1] + h], [p[0], p[1] + h]],
            material: Material::Acrylic,
        })
}

fn layout() -> impl Strategy<Value = LayoutManifest> {
    (
        proptest::collection::vec(open_wall(), 0..4),
        proptest::collection::vec(closed_wall(), 0..3),
        proptest::collection::vec(post(), 0..6),
        proptest::collection::vec(lane(), 0..4),
        proptest::collection::vec(insert(), 0..6),
        proptest::collection::vec(deck(), 0..3),
        proptest::option::of(20.0f32..100.0),
    )
        .prop_map(|(mut walls, closed, posts, lanes, inserts, decks, ramp)| {
            walls.extend(closed);
            LayoutManifest {
                schema_version: 1,
                playfield: Playfield {
                    width: W,
                    length: L,
                    wall_height: 28.0,
                },
                walls,
                posts,
                lanes,
                ramps: ramp
                    .map(|entry_width| Ramp {
                        spline: "return_wireform".into(),
                        entry_width,
                    })
                    .into_iter()
                    .collect(),
                decks,
                inserts,
            }
        })
}

/// A layout of one straight lane and nothing else.
fn one_lane(from: [f32; 2], to: [f32; 2], width: f32) -> LayoutManifest {
    LayoutManifest {
        schema_version: 1,
        playfield: Playfield {
            width: W,
            length: L,
            wall_height: 28.0,
        },
        walls: vec![],
        posts: vec![],
        lanes: vec![Lane {
            kind: LaneKind::Inlane,
            points: vec![from, to],
            width,
        }],
        ramps: vec![],
        decks: vec![],
        inserts: vec![],
    }
}

/// A straight lane near a diagonal once tripped the doubles-back lint
/// (f32 parallel determinant read as a crossing).
#[test]
fn straight_diagonal_lane_generates() {
    let layout = one_lane([364.8081, 278.6047], [274.15408, 188.0386], 15.889875);
    generate(&layout, &apex_base()).unwrap_or_else(|e| panic!("{e}"));
}

/// Fixed sweep of straight lanes within 0.1 degrees of each diagonal.
#[test]
fn straight_lanes_near_diagonals_generate() {
    let base = apex_base();
    let mut cases = 0;
    for quadrant in 0..4 {
        let centre = (45.0 + 90.0 * quadrant as f32).to_radians();
        for i in 0..=24 {
            let a = centre + (-0.1 + 0.2 * i as f32 / 24.0).to_radians();
            for j in 0..20 {
                let len = 60.0 + 23.0 * j as f32;
                let from = [200.0 + 1.37 * j as f32, 400.0 + 0.91 * i as f32];
                let to = [from[0] + len * libm::cosf(a), from[1] + len * libm::sinf(a)];
                let layout = one_lane(from, to, 15.0 + 1.7 * (j % 7) as f32);
                generate(&layout, &base).unwrap_or_else(|e| panic!("{e}\n{layout:#?}"));
                cases += 1;
            }
        }
    }
    assert!(cases >= 1900);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    #[test]
    fn random_valid_layouts_validate(layout in layout()) {
        let base = apex_base();
        let generated = generate(&layout, &base).unwrap_or_else(|e| panic!("{e}\n{layout:#?}"));
        let dir = bundle_dir("random");
        write_bundle(&generated, &dir).unwrap();
        let vfs = Vfs::open(&dir).unwrap();
        let table = load_table(&vfs).unwrap_or_else(|e| panic!("{e}\n{layout:#?}"));
        for (k, s) in table.manifest.splines.iter().enumerate() {
            if s.id.starts_with(GENERATED_PREFIX) {
                let p = format!("splines[{k}]");
                let w: Vec<_> = table.warnings.iter().filter(|w| w.path.starts_with(&p)).collect();
                prop_assert!(w.is_empty(), "{w:#?}\n{layout:#?}");
            }
        }
        let scene = load_gltf(&vfs, "playfield.glb").unwrap();
        prop_assert!(validate_gltf_bindings(&scene, &table.ids).is_empty());
        let lamps = scene.meshes.iter().filter(|m| m.extras.lamp.is_some()).count();
        prop_assert_eq!(lamps, layout.inserts.len());
        prop_assert!(scene.meshes.iter().filter(|m| m.extras.collider).all(|m| m.extras.collision_layer.is_some()));
        std::fs::remove_dir_all(dir).unwrap();
    }

    /// Random bytes through the CLI's parse path, then the generator.
    #[test]
    fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        if let Ok(layout) = parse_layout_bytes(&bytes) {
            let _ = generate(&layout, &apex_base());
        }
    }

    /// Random TOML-shaped text that often parses.
    #[test]
    fn random_layout_text_never_panics(
        w in -1.0e6f64..1.0e6, l in -1.0e6f64..1.0e6, h in -10.0f64..1.0e5,
        pts in proptest::collection::vec((-1.0e4f64..1.0e4, -1.0e4f64..1.0e4), 0..8),
        width in -10.0f64..2000.0, r in -10.0f64..2000.0, closed in any::<bool>(),
        layer in 0u8..10, z in -10.0f64..2000.0,
    ) {
        let list: Vec<String> = pts.iter().map(|(x, y)| format!("[{x:?}, {y:?}]")).collect();
        let list = list.join(", ");
        let text = format!(
            "schema_version = 1\n[playfield]\nwidth = {w:?}\nlength = {l:?}\nwall_height = {h:?}\n\
             [[walls]]\npoints = [{list}]\nclosed = {closed}\nmaterial = \"wood\"\n\
             [[lanes]]\nkind = \"orbit\"\npoints = [{list}]\nwidth = {width:?}\n\
             [[posts]]\nposition = [{w:?}, {l:?}]\nradius = {r:?}\n\
             [[decks]]\nlayer = {layer}\nz = {z:?}\noutline = [{list}]\nmaterial = \"acrylic\"\n\
             [[inserts]]\nlamp = \"overdrive_ready\"\nposition = [{w:?}, {l:?}]\nshape = \"arrow\"\nsize = {r:?}\n"
        );
        let layout = parse_layout_bytes(text.as_bytes()).unwrap();
        let _ = generate(&layout, &apex_base());
    }
}
