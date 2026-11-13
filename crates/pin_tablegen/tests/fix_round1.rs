//! Regression tests for the 4.3 review, round 1 (B1, B2, N1, N2).

mod support;

use pin_assets::{Vfs, load_table};
use pin_core::Material;
use pin_core::schema::LayoutManifest;
use pin_core::schema::layout::{Insert, InsertShape, Lane, LaneKind, Playfield, Post};
use pin_tablegen::layout::{GENERATED_PREFIX, generate, write_bundle};
use proptest::prelude::*;
use support::*;

fn empty() -> LayoutManifest {
    LayoutManifest {
        schema_version: 1,
        playfield: Playfield {
            width: 514.0,
            length: 1070.0,
            wall_height: 28.0,
        },
        walls: vec![],
        posts: vec![],
        lanes: vec![],
        ramps: vec![],
        decks: vec![],
        inserts: vec![],
    }
}

fn with_lane(points: Vec<[f32; 2]>, width: f32) -> LayoutManifest {
    let mut l = empty();
    l.lanes.push(Lane {
        kind: LaneKind::Orbit,
        points,
        width,
    });
    l
}

/// B1: the reviewer's 71 m serpentine (7 legs of 8.8 m, 500 mm bends,
/// sampled every 80 mm) is refused, naming the lane.
#[test]
fn b1_lanes_longer_than_65_m_are_refused() {
    let mut pts: Vec<[f32; 2]> = Vec::new();
    let step = 80.0f32;
    for leg in 0..7 {
        let x = -3500.0 + 1000.0 * leg as f32;
        let up = leg % 2 == 0;
        let n = (8800.0 / step) as usize;
        for k in 0..n {
            let t = k as f32 * step;
            let y = if up { -4400.0 + t } else { 4400.0 - t };
            pts.push([x, y]);
        }
        if leg < 6 {
            let (cy, sign) = if up { (4400.0, 1.0) } else { (-4400.0, -1.0) };
            let arc_n = (core::f32::consts::PI * 500.0 / step) as usize;
            for k in 0..arc_n {
                let a = core::f32::consts::PI * (1.0 - k as f32 / arc_n as f32);
                pts.push([x + 500.0 + 500.0 * a.cos(), cy + sign * 500.0 * a.sin()]);
            }
        }
    }
    pts.dedup();
    let e = generate(&with_lane(pts, 40.0), &apex_base()).unwrap_err();
    assert_eq!(e.element, "lanes[0]", "{e}");
    assert!(e.message.contains("65 m"), "{e}");
}

/// B2: layouts beyond the loader's glTF budgets are refused, naming the
/// element that crosses the budget.
#[test]
fn b2_output_within_gltf_budgets() {
    let mut l = empty();
    l.posts = (0..16_400)
        .map(|i| Post {
            position: [(i % 500) as f32, (i / 500) as f32 * 10.0],
            radius: 0.1,
            rubber: false,
        })
        .collect();
    let e = generate(&l, &apex_base()).unwrap_err();
    // The floor is node 1: posts[16383] is mesh 16 385.
    assert_eq!(e.element, "posts[16383]", "{e}");
    assert!(e.message.contains("16384"), "{e}");

    let mut l = empty();
    l.inserts = (0..16_400)
        .map(|i| Insert {
            lamp: "overdrive_ready".into(),
            position: [(i % 500) as f32, (i / 500) as f32 * 10.0],
            shape: InsertShape::Circle,
            size: 1.0,
        })
        .collect();
    let e = generate(&l, &apex_base()).unwrap_err();
    assert!(e.element.starts_with("inserts["), "{e}");

    // Vertices: 6000 posts of 1 m radius (500 sides each).
    let mut l = empty();
    l.posts = (0..6000)
        .map(|i| Post {
            position: [0.0, i as f32 * 0.5],
            radius: 1000.0,
            rubber: true,
        })
        .collect();
    let e = generate(&l, &apex_base()).unwrap_err();
    assert!(e.element.starts_with("posts["), "{e}");
    assert!(e.message.contains("vertices"), "{e}");

    // Just under the mesh budget loads.
    let mut l = empty();
    l.posts = (0..2000)
        .map(|i| Post {
            position: [(i % 50) as f32 * 10.0, (i / 50) as f32 * 20.0],
            radius: 2.0,
            rubber: false,
        })
        .collect();
    l.walls.push(pin_core::schema::layout::Wall {
        points: vec![[0.0, 0.0], [514.0, 0.0]],
        closed: false,
        material: Material::Wood,
    });
    let g = generate(&l, &apex_base()).unwrap();
    let (dir, _, scene) = load(&g, "b2");
    assert_eq!(scene.meshes.len(), 2002);
    std::fs::remove_dir_all(dir).unwrap();
}

/// N1: a self-crossing lane is refused (its rails draw the loader's
/// "doubles back" warning).
#[test]
fn n1_self_crossing_lanes_are_refused() {
    let e = generate(
        &with_lane(
            vec![
                [0.0, 0.0],
                [300.0, 0.0],
                [300.0, 300.0],
                [150.0, 300.0],
                [150.0, -100.0],
            ],
            20.0,
        ),
        &apex_base(),
    )
    .unwrap_err();
    assert_eq!(e.element, "lanes[0]", "{e}");
}

/// N2: lanes about 1 mm long (hairline gap in the loader's f32 metres).
#[test]
fn n2_one_millimetre_lanes_are_refused() {
    for (pts, w) in [
        (vec![[100.0, 100.0], [100.0, 101.0]], 20.0),
        (vec![[5000.0, 5000.0], [5000.0, 4999.0]], 0.1),
    ] {
        let e = generate(&with_lane(pts, w), &apex_base()).unwrap_err();
        assert_eq!(e.element, "lanes[0]", "{e}");
    }
    // 2 mm is fine and loads without warnings.
    let g = generate(
        &with_lane(vec![[100.0, 100.0], [100.0, 102.0]], 20.0),
        &apex_base(),
    )
    .unwrap();
    let (dir, table, _) = load(&g, "n2");
    assert!(
        table
            .warnings
            .iter()
            .all(|w| !w.path.starts_with("splines[1]"))
    );
    std::fs::remove_dir_all(dir).unwrap();
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, ..ProptestConfig::default() })]

    /// N1 property: any random polyline lane is either refused (naming it)
    /// or loads with no error or warning on its rails.
    #[test]
    fn random_lanes_refused_or_clean(
        pts in proptest::collection::vec((0.0f32..514.0, 0.0f32..1070.0), 2..6),
        width in 5.0f32..60.0,
    ) {
        let mut points: Vec<[f32; 2]> = pts.into_iter().map(|(x, y)| [x, y]).collect();
        points.dedup();
        prop_assume!(points.len() >= 2);
        match generate(&with_lane(points.clone(), width), &apex_base()) {
            Err(e) => prop_assert_eq!(e.element, "lanes[0]"),
            Ok(g) => {
                let dir = bundle_dir("n1prop");
                write_bundle(&g, &dir).unwrap();
                let table = load_table(&Vfs::open(&dir).unwrap())
                    .unwrap_or_else(|e| panic!("{e}\n{points:?}"));
                for (k, s) in table.manifest.splines.iter().enumerate() {
                    if s.id.starts_with(GENERATED_PREFIX) {
                        let p = format!("splines[{k}]");
                        let w: Vec<_> = table.warnings.iter().filter(|w| w.path.starts_with(&p)).collect();
                        prop_assert!(w.is_empty(), "{w:#?}\n{points:?} {width}");
                    }
                }
                std::fs::remove_dir_all(dir).unwrap();
            }
        }
    }
}
