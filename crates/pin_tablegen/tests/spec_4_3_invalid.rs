//! Subphase 4.3 spec tests (Independent Tester, black box): invalid layouts
//! fail with an error naming the element (criterion 5), the generator never
//! emits an invalid bundle (deliverable 3) and never panics on arbitrary
//! layout text (criterion 6, library half; the CLI half is in spec_4_3_cli).

#[path = "fixtures/spec_4_3/support.rs"]
mod support;

use pin_core::Material;
use pin_core::schema::layout::{
    Deck, Insert, InsertShape, Lane, LaneKind, LayoutManifest, Post, Ramp, Wall,
};
use pin_tablegen::layout::generate;
use proptest::prelude::*;
use support::*;

fn plain() -> LayoutManifest {
    build_layout(500.0, 1000.0, 25.0, true, &[], &[])
}

fn insert(lamp: &str, x: f32, y: f32) -> Insert {
    Insert {
        lamp: lamp.to_owned(),
        position: [x, y],
        shape: InsertShape::Circle,
        size: 18.0,
    }
}

fn wall(points: &[[f32; 2]], closed: bool) -> Wall {
    Wall {
        points: points.to_vec(),
        closed,
        material: Material::Steel,
    }
}

fn assert_names(err: &str, element: &str) {
    assert!(
        err.contains(element),
        "the error must name {element}, got: {err}"
    );
}

// ------------------------------------------------- criterion 5, verbatim

#[test]
fn unknown_lamp_names_the_insert() {
    let mut layout = plain();
    layout.inserts.push(insert("lamp_a", 200.0, 300.0));
    layout.inserts.push(insert("no_such_lamp", 250.0, 400.0));
    layout.inserts.push(insert("lamp_b", 300.0, 500.0));
    let err = gen_err(&layout, &base_table());
    assert_names(&err, "inserts[1]");
}

#[test]
fn unknown_lamp_in_the_a3_example_names_the_insert() {
    let mut layout = a3_layout();
    layout.inserts[0].lamp = "overdrive_redy".to_owned();
    let err = gen_err(&layout, &base_table());
    assert_names(&err, "inserts[0]");
}

#[test]
fn self_intersecting_closed_wall_names_the_wall() {
    let mut layout = plain(); // walls[0] is the outer wall
    layout
        .walls
        .push(wall(&[[100.0, 600.0], [200.0, 650.0]], false));
    // A bow-tie: edges 0-1 and 2-3 cross.
    layout.walls.push(wall(
        &[[100.0, 100.0], [200.0, 200.0], [200.0, 100.0], [100.0, 200.0]],
        true,
    ));
    let err = gen_err(&layout, &base_table());
    assert_names(&err, "walls[2]");
}

#[test]
fn closed_wall_whose_closing_edge_crosses_names_the_wall() {
    let mut layout = plain();
    // Open, this polyline is simple; the closing edge 3-0 crosses edge 1-2.
    layout.walls.push(wall(
        &[[100.0, 100.0], [200.0, 100.0], [150.0, 180.0], [150.0, 40.0]],
        true,
    ));
    let err = gen_err(&layout, &base_table());
    assert_names(&err, "walls[1]");
}

#[test]
fn missing_spline_reference_names_the_ramp() {
    let mut layout = a3_layout();
    layout.ramps.push(Ramp {
        spline: "no_such_spline".to_owned(),
        entry_width: 60.0,
    });
    let err = gen_err(&layout, &base_table());
    assert_names(&err, "ramps[1]");
}

#[test]
fn missing_spline_reference_in_the_a3_example() {
    let mut base = base_table();
    base.splines.clear();
    let err = gen_err(&a3_layout(), &base);
    assert_names(&err, "ramps[0]");
}

// ------------------------------ other layouts that cannot be generated

#[test]
fn deck_layer_above_seven_names_the_deck() {
    let mut layout = plain();
    layout.decks.push(Deck {
        layer: 9,
        z: 60.0,
        outline: vec![[100.0, 500.0], [300.0, 500.0], [300.0, 700.0], [100.0, 700.0]],
        material: Material::Acrylic,
    });
    let err = gen_err(&layout, &base_table());
    assert_names(&err, "decks[0]");
}

#[test]
fn degenerate_deck_outline_names_the_deck() {
    let mut layout = plain();
    layout.decks.push(Deck {
        layer: 1,
        z: 60.0,
        outline: vec![[100.0, 500.0], [300.0, 500.0]],
        material: Material::Acrylic,
    });
    let err = gen_err(&layout, &base_table());
    assert_names(&err, "decks[0]");
}

#[test]
fn non_finite_coordinates_name_the_element() {
    let base = base_table();
    let mut layout = plain();
    layout
        .walls
        .push(wall(&[[100.0, 100.0], [f32::NAN, 200.0]], false));
    assert_names(&gen_err(&layout, &base), "walls[1]");

    let mut layout = plain();
    layout.posts.push(Post {
        position: [f32::INFINITY, 100.0],
        radius: 5.0,
        rubber: false,
    });
    assert_names(&gen_err(&layout, &base), "posts[0]");

    let mut layout = plain();
    layout.inserts.push(Insert {
        size: f32::NAN,
        ..insert("lamp_a", 100.0, 100.0)
    });
    assert_names(&gen_err(&layout, &base), "inserts[0]");
}

#[test]
fn non_positive_sizes_name_the_element() {
    let base = base_table();
    for radius in [0.0, -4.0] {
        let mut layout = plain();
        layout.posts.push(Post {
            position: [100.0, 100.0],
            radius: 6.0,
            rubber: true,
        });
        layout.posts.push(Post {
            position: [200.0, 100.0],
            radius,
            rubber: false,
        });
        assert_names(&gen_err(&layout, &base), "posts[1]");
    }
    for width in [0.0, -32.0] {
        let mut layout = plain();
        layout.lanes.push(Lane {
            kind: LaneKind::Inlane,
            points: vec![[100.0, 300.0], [120.0, 200.0]],
            width,
        });
        assert_names(&gen_err(&layout, &base), "lanes[0]");
    }
}

#[test]
fn too_few_points_name_the_element() {
    let base = base_table();
    let mut layout = plain();
    layout.walls.push(wall(&[[100.0, 100.0]], false));
    assert_names(&gen_err(&layout, &base), "walls[1]");

    let mut layout = plain();
    layout.walls.push(wall(&[], false));
    assert_names(&gen_err(&layout, &base), "walls[1]");

    let mut layout = plain();
    layout.lanes.push(Lane {
        kind: LaneKind::Shooter,
        points: vec![[100.0, 100.0]],
        width: 30.0,
    });
    assert_names(&gen_err(&layout, &base), "lanes[0]");
}

#[test]
fn coincident_wall_points_never_yield_an_invalid_bundle() {
    let base = base_table();
    let mut layout = plain();
    layout.walls.push(wall(
        &[[100.0, 100.0], [100.0, 100.0], [200.0, 150.0]],
        false,
    ));
    match generate(&layout, &base) {
        Ok(g) => {
            check_valid_bundle(&g, &base, "coincident").unwrap_or_else(|e| panic!("{e}"));
        }
        Err(e) => assert_names(&e.to_string(), "walls[1]"),
    }
}

#[test]
fn degenerate_playfield_is_refused() {
    let base = base_table();
    for (w, l, h) in [
        (0.0, 1000.0, 25.0),
        (500.0, -1.0, 25.0),
        (f32::NAN, 1000.0, 25.0),
        (500.0, 1000.0, f32::INFINITY),
    ] {
        let mut layout = plain();
        layout.walls.clear();
        layout.playfield.width = w;
        layout.playfield.length = l;
        layout.playfield.wall_height = h;
        let err = gen_err(&layout, &base);
        assert_names(&err, "playfield");
    }
}

// ------------------------------------------- never an invalid bundle

#[derive(Clone, Debug)]
enum Damage {
    UnknownLamp,
    BowTie,
    MissingSpline,
    NanPoint,
    HugePoint(f32),
    ZeroRadius,
    HugeRadius,
    NegativeWidth,
    TinyWidth,
    OnePointWall,
    DuplicatePoint,
    BadDeckLayer(u8),
    ThinDeck,
    FlatPlayfield,
    ZeroSizeInsert,
    DoubledBackWall,
    SharpLane,
    OutsidePost,
    None,
}

fn damage() -> impl Strategy<Value = Damage> {
    prop_oneof![
        Just(Damage::UnknownLamp),
        Just(Damage::BowTie),
        Just(Damage::MissingSpline),
        Just(Damage::NanPoint),
        prop_oneof![Just(1.0e5f32), Just(-1.0e5), Just(2.0e7), Just(3.0e38)]
            .prop_map(Damage::HugePoint),
        Just(Damage::ZeroRadius),
        Just(Damage::HugeRadius),
        Just(Damage::NegativeWidth),
        Just(Damage::TinyWidth),
        Just(Damage::OnePointWall),
        Just(Damage::DuplicatePoint),
        (0u8..=255).prop_map(Damage::BadDeckLayer),
        Just(Damage::ThinDeck),
        Just(Damage::FlatPlayfield),
        Just(Damage::ZeroSizeInsert),
        Just(Damage::DoubledBackWall),
        Just(Damage::SharpLane),
        Just(Damage::OutsidePost),
        Just(Damage::None),
    ]
}

fn apply(layout: &mut LayoutManifest, d: &Damage) {
    let (w, l) = (layout.playfield.width, layout.playfield.length);
    let (cx, cy) = (w / 2.0, l / 2.0);
    match d {
        Damage::UnknownLamp => layout.inserts.push(insert("ghost_lamp", cx, cy)),
        Damage::BowTie => layout.walls.push(wall(
            &[[cx, cy], [cx + 60.0, cy + 60.0], [cx + 60.0, cy], [cx, cy + 60.0]],
            true,
        )),
        Damage::MissingSpline => layout.ramps.push(Ramp {
            spline: "ghost_spline".into(),
            entry_width: 50.0,
        }),
        Damage::NanPoint => layout.walls.push(wall(&[[cx, cy], [f32::NAN, f32::NAN]], false)),
        Damage::HugePoint(v) => layout.walls.push(wall(&[[cx, cy], [*v, cy]], false)),
        Damage::ZeroRadius => layout.posts.push(Post {
            position: [cx, cy],
            radius: 0.0,
            rubber: false,
        }),
        Damage::HugeRadius => layout.posts.push(Post {
            position: [cx, cy],
            radius: 1.0e6,
            rubber: true,
        }),
        Damage::NegativeWidth => layout.lanes.push(Lane {
            kind: LaneKind::Outlane,
            points: vec![[cx, cy], [cx + 10.0, cy + 80.0]],
            width: -30.0,
        }),
        Damage::TinyWidth => layout.lanes.push(Lane {
            kind: LaneKind::Outlane,
            points: vec![[cx, cy], [cx + 10.0, cy + 80.0]],
            width: 1.0e-4,
        }),
        Damage::OnePointWall => layout.walls.push(wall(&[[cx, cy]], true)),
        Damage::DuplicatePoint => layout
            .walls
            .push(wall(&[[cx, cy], [cx, cy], [cx, cy], [cx + 50.0, cy]], false)),
        Damage::BadDeckLayer(layer) => layout.decks.push(Deck {
            layer: *layer,
            z: 60.0,
            outline: vec![[cx, cy], [cx + 90.0, cy], [cx + 90.0, cy + 90.0], [cx, cy + 90.0]],
            material: Material::Acrylic,
        }),
        Damage::ThinDeck => layout.decks.push(Deck {
            layer: 1,
            z: 60.0,
            outline: vec![[cx, cy], [cx + 90.0, cy], [cx + 45.0, cy]],
            material: Material::Plastic,
        }),
        Damage::FlatPlayfield => layout.playfield.length = 0.0,
        Damage::ZeroSizeInsert => layout.inserts.push(Insert {
            size: 0.0,
            ..insert("lamp_h", cx, cy)
        }),
        // A hairpin: the wall returns along itself 0.5 mm away.
        Damage::DoubledBackWall => layout.walls.push(wall(
            &[[cx, cy], [cx + 80.0, cy], [cx, cy + 0.5]],
            false,
        )),
        Damage::SharpLane => layout.lanes.push(Lane {
            kind: LaneKind::Orbit,
            points: vec![[cx, cy], [cx + 60.0, cy], [cx + 2.0, cy + 6.0]],
            width: 36.0,
        }),
        Damage::OutsidePost => layout.posts.push(Post {
            position: [w + 500.0, -300.0],
            radius: 5.0,
            rubber: false,
        }),
        Damage::None => {}
    }
}

/// Mutations of the example layout text: (kind, position, replacement).
fn mutate(text: &str, kind: u8, pos: usize, pick: usize, byte: u8) -> String {
    const NUMBERS: [&str; 12] = [
        "nan", "inf", "-inf", "-1.0", "0.0", "1e30", "1e-30", "99999.0", "-0.0", "1e308", "7",
        "\"x\"",
    ];
    let lines: Vec<&str> = text.lines().collect();
    if lines.is_empty() || !text.bytes().any(|b| b.is_ascii_digit()) {
        return text.to_owned();
    }
    match kind % 5 {
        // Replace one byte.
        0 => {
            let mut bytes = text.as_bytes().to_vec();
            let i = pos % bytes.len();
            bytes[i] = byte;
            String::from_utf8_lossy(&bytes).into_owned()
        }
        // Replace one numeric literal.
        1 => {
            let spans: Vec<(usize, usize)> = {
                let b = text.as_bytes();
                let mut out = Vec::new();
                let mut i = 0;
                while i < b.len() {
                    if b[i].is_ascii_digit() {
                        let start = i;
                        while i < b.len() && (b[i].is_ascii_digit() || b[i] == b'.') {
                            i += 1;
                        }
                        out.push((start, i));
                    } else {
                        i += 1;
                    }
                }
                out
            };
            let (a, b) = spans[pos % spans.len()];
            format!("{}{}{}", &text[..a], NUMBERS[pick % NUMBERS.len()], &text[b..])
        }
        // Delete a line.
        2 => {
            let i = pos % lines.len();
            lines
                .iter()
                .enumerate()
                .filter(|(k, _)| *k != i)
                .map(|(_, l)| *l)
                .collect::<Vec<_>>()
                .join("\n")
        }
        // Duplicate a line.
        3 => {
            let i = pos % lines.len();
            let mut out = lines.clone();
            out.insert(i, lines[i]);
            out.join("\n")
        }
        // Truncate.
        _ => {
            let mut i = pos % text.len();
            while !text.is_char_boundary(i) {
                i -= 1;
            }
            text[..i].to_owned()
        }
    }
}

/// The CLI's parse path, in process: text -> LayoutManifest -> generate.
/// Whatever comes out is an error or a bundle that validates.
fn parse_and_generate(text: &str, tag: &str) -> Result<(), String> {
    let Ok(layout) = LayoutManifest::from_toml_str(text) else {
        return Ok(());
    };
    let base = base_table();
    match generate(&layout, &base) {
        Ok(g) => check_valid_bundle(&g, &base, tag).map(|_| ()),
        Err(e) => {
            let msg = e.to_string();
            if msg.trim().is_empty() {
                Err("empty error message".into())
            } else {
                Ok(())
            }
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 96, failure_persistence: None, ..ProptestConfig::default() })]

    /// Deliverable 3: a layout that cannot be generated validly is refused;
    /// whatever is generated passes the 4.1 validation.
    #[test]
    fn damaged_layouts_fail_or_validate(
        layout in valid_layout(),
        damages in prop::collection::vec(damage(), 1..=3),
    ) {
        let mut layout = layout;
        for d in &damages {
            apply(&mut layout, d);
        }
        let base = base_table();
        match generate(&layout, &base) {
            Ok(g) => {
                check_valid_bundle(&g, &base, "damaged").map_err(TestCaseError::fail)?;
                // The three criterion-5 damages can never be accepted.
                for d in &damages {
                    prop_assert!(
                        !matches!(d, Damage::UnknownLamp | Damage::BowTie | Damage::MissingSpline
                            | Damage::NanPoint),
                        "accepted a layout damaged by {:?}", d
                    );
                }
            }
            Err(e) => {
                let msg = e.to_string();
                prop_assert!(!msg.trim().is_empty());
                // With a single damage the culprit is known: the error names
                // its section.
                if damages.len() == 1 {
                    let section = match &damages[0] {
                        Damage::UnknownLamp | Damage::ZeroSizeInsert => "inserts[",
                        Damage::BowTie | Damage::NanPoint | Damage::HugePoint(_)
                        | Damage::OnePointWall | Damage::DuplicatePoint
                        | Damage::DoubledBackWall => "walls[",
                        Damage::MissingSpline => "ramps[",
                        Damage::ZeroRadius | Damage::HugeRadius | Damage::OutsidePost => "posts[",
                        Damage::NegativeWidth | Damage::TinyWidth | Damage::SharpLane => "lanes[",
                        Damage::BadDeckLayer(_) | Damage::ThinDeck => "decks[",
                        Damage::FlatPlayfield => "playfield",
                        Damage::None => {
                            return Err(TestCaseError::fail(format!(
                                "valid layout refused: {msg}"
                            )));
                        }
                    };
                    prop_assert!(msg.contains(section), "{:?}: error does not name {}: {}",
                        &damages[0], section, msg);
                }
            }
        }
    }

    /// Criterion 6: no panics on random layout.toml bytes.
    #[test]
    fn random_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..2048)) {
        let text = String::from_utf8_lossy(&bytes);
        parse_and_generate(&text, "bytes").map_err(TestCaseError::fail)?;
    }

    /// Criterion 6 with inputs that get past the TOML parser.
    #[test]
    fn mutated_example_never_panics_and_never_emits_an_invalid_bundle(
        steps in prop::collection::vec((any::<u8>(), any::<usize>(), any::<usize>(), any::<u8>()), 1..=4),
        geo in any::<bool>(),
    ) {
        let name = if geo { "geo_layout.toml" } else { "a3_layout.toml" };
        let mut text = std::fs::read_to_string(fixture_dir().join(name)).unwrap();
        for (kind, pos, pick, byte) in steps {
            if text.is_empty() {
                break;
            }
            text = mutate(&text, kind, pos, pick, byte);
        }
        parse_and_generate(&text, "mutated").map_err(TestCaseError::fail)?;
    }

    /// TOML-shaped noise: random keys and values under the layout's tables.
    #[test]
    fn toml_shaped_noise_never_panics(
        entries in prop::collection::vec(
            (0usize..8, 0usize..12, prop_oneof![
                any::<f32>().prop_map(|v| format!("{v:?}")),
                any::<i64>().prop_map(|v| v.to_string()),
                "[a-z_]{0,8}".prop_map(|s| format!("{s:?}")),
                prop::collection::vec(prop::collection::vec(-2000.0f32..2000.0, 0..4), 0..6)
                    .prop_map(|v| format!("{v:?}")),
                any::<bool>().prop_map(|v| v.to_string()),
            ]),
            0..24,
        ),
    ) {
        const TABLES: [&str; 8] = [
            "[playfield]", "[[walls]]", "[[posts]]", "[[lanes]]", "[[ramps]]", "[[decks]]",
            "[[inserts]]", "[[walls]]",
        ];
        const KEYS: [&str; 12] = [
            "width", "length", "wall_height", "points", "closed", "material", "position",
            "radius", "kind", "lamp", "size", "outline",
        ];
        let mut text = String::from("schema_version = 1\n[playfield]\nwidth = 500.0\nlength = 1000.0\nwall_height = 25.0\n");
        for (table, key, value) in entries {
            if table != 0 {
                text.push_str(TABLES[table]);
                text.push('\n');
            }
            text.push_str(&format!("{} = {}\n", KEYS[key], value));
        }
        parse_and_generate(&text, "noise").map_err(TestCaseError::fail)?;
    }
}
