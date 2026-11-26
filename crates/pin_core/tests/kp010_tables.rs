//! KP-010 (#90) acceptance H: today's four tables parse unchanged and their parts resolve,
//! from built-in defaults alone, to the values the generator uses (REQ-BUILD-005).

mod kp010_common;

use kp010_common::{TABLES, layout};
use pin_core::schema::parts::{PartKind, Shape};
use pin_core::schema::physics::DEFAULT_PHYSICS_TOML;
use pin_core::schema::table::SplineKind;
use pin_core::{LayoutManifest, Material, PhysicsDefaults, TableManifest};

fn load(i: usize) -> (TableManifest, LayoutManifest) {
    let (name, t, l) = TABLES[i];
    let t = TableManifest::from_toml_str(t).unwrap_or_else(|e| panic!("{name} table: {e}"));
    let l = LayoutManifest::from_toml_str(l).unwrap_or_else(|e| panic!("{name} layout: {e}"));
    (t, l)
}

/// REQ-BUILD-005: parses, round-trips equal, and none of the new keys is present.
#[test]
fn req_build_005_four_tables_parse_and_round_trip_without_new_keys() {
    for (i, (name, t_src, l_src)) in TABLES.iter().enumerate() {
        let (t, l) = load(i);
        assert!(t.theme.is_none(), "{name}");
        assert!(t.materials.is_empty(), "{name}");
        assert!(t.part_styles.is_empty(), "{name}");
        assert!(t.parts.is_empty(), "{name}");
        let t_text = t.to_toml_string().unwrap();
        assert_eq!(TableManifest::from_toml_str(&t_text).unwrap(), t, "{name}");
        let l_text = l.to_toml_string().unwrap();
        assert_eq!(LayoutManifest::from_toml_str(&l_text).unwrap(), l, "{name}");
        for key in ["[theme]", "[[materials]]", "[[part_styles]]", "[[parts]]"] {
            assert!(!t_text.contains(key), "{name}: {key} serialised");
            assert!(!t_src.contains(key), "{name}: {key} in source");
        }
        for key in ["style", "overrides", "surface", "rotation_deg", "[[parts]]"] {
            assert!(!l_src.contains(key), "{name}: {key} in layout");
        }
    }
}

/// REQ-BUILD-005: with no `[theme]` the effective theme is `meta.name` and `meta.palette`.
#[test]
fn req_build_005_four_tables_keep_title_and_palette() {
    for i in 0..TABLES.len() {
        let (t, _) = load(i);
        let th = t.effective_theme();
        assert_eq!(th.title, t.meta.name);
        let want: Vec<[u8; 3]> = t
            .meta
            .palette
            .iter()
            .map(|c| pin_core::schema::parts::parse_color(c).expect("shipped palettes are valid"))
            .collect();
        assert_eq!(th.palette, want);
        assert_eq!(th.motif, pin_core::schema::parts::DEFAULT_MOTIF);
        assert!(th.fonts.is_empty());
    }
}

/// REQ-BUILD-005, REQ-BUILD-004: every legacy element resolves to the generator's values:
/// posts rubber → rubber else steel at `wall_height` and diameter 2r; walls and decks their
/// own material; lane guides and ramps steel; posts, walls, lane guides at `wall_height`;
/// inserts lifted 0.1 mm; wireform tube 1.5 mm with 8 sides; and every touchable part has a
/// material pair with a ball material.
#[test]
fn req_build_005_four_tables_resolve_to_generator_values() {
    let physics = PhysicsDefaults::from_toml_str(DEFAULT_PHYSICS_TOML).unwrap();
    for (i, (name, _, _)) in TABLES.iter().enumerate() {
        let (t, l) = load(i);
        let parts = t.all_parts(&l).unwrap_or_else(|e| panic!("{name}: {e}"));
        let n = l.walls.len()
            + l.posts.len()
            + l.lanes.len()
            + l.ramps.len()
            + l.decks.len()
            + l.inserts.len();
        assert_eq!(parts.len(), n, "{name}");
        let wh = l.playfield.wall_height;
        let by = |p: &str| parts.iter().find(|r| r.name == p).unwrap();
        for (k, w) in l.walls.iter().enumerate() {
            let r = by(&format!("walls[{k}]"));
            assert_eq!(r.kind, PartKind::Wall);
            assert_eq!(r.physical(), Some(w.material), "{name} walls[{k}]");
            assert_eq!(r.material.as_ref().unwrap().def.id, w.material.name());
            assert_eq!(r.fields.height, Some(wh));
            assert_eq!(r.placement.position, [0.0; 3]);
        }
        for (k, p) in l.posts.iter().enumerate() {
            let r = by(&format!("posts[{k}]"));
            assert_eq!(r.kind, PartKind::Post);
            let want = if p.rubber {
                Material::Rubber
            } else {
                Material::Steel
            };
            assert_eq!(r.physical(), Some(want), "{name} posts[{k}]");
            assert_eq!(r.fields.diameter, Some(2.0 * p.radius));
            assert_eq!(r.fields.height, Some(wh));
            assert_eq!(r.placement.position, [p.position[0], p.position[1], 0.0]);
            assert_eq!(r.placement.rotation_deg, [0.0; 3]);
        }
        for k in 0..l.lanes.len() {
            let r = by(&format!("lanes[{k}]"));
            assert_eq!(r.kind, PartKind::LaneGuide);
            assert_eq!(r.physical(), Some(Material::Steel));
            assert_eq!(r.fields.height, Some(wh));
        }
        for (k, ramp) in l.ramps.iter().enumerate() {
            let r = by(&format!("ramps[{k}]"));
            assert_eq!(r.physical(), Some(Material::Steel), "{name} ramps[{k}]");
            let kind = t
                .splines
                .iter()
                .find(|s| s.id == ramp.spline)
                .map(|s| s.kind);
            if kind == Some(SplineKind::Guide) {
                assert_eq!(r.kind, PartKind::LaneGuide);
                assert_eq!(r.fields.height, Some(wh));
            } else {
                assert_eq!(r.kind, PartKind::Wireform);
                assert_eq!(r.fields.tube_radius, Some(1.5));
                assert_eq!(r.fields.tube_sides, Some(8));
            }
        }
        for (k, d) in l.decks.iter().enumerate() {
            let r = by(&format!("decks[{k}]"));
            assert_eq!(r.kind, PartKind::Deck);
            assert_eq!(r.physical(), Some(d.material));
            assert_eq!(r.placement.position, [0.0, 0.0, d.z]);
        }
        for (k, ins) in l.inserts.iter().enumerate() {
            let r = by(&format!("inserts[{k}]"));
            assert_eq!(r.kind, PartKind::Insert);
            assert_eq!(r.fields.lift, Some(0.1));
            assert_eq!(r.fields.size, Some(ins.size));
            assert_eq!(r.fields.shape, Some(Shape::from(ins.shape)));
            assert_eq!(r.physical(), None);
            assert_eq!(
                r.placement.position,
                [ins.position[0], ins.position[1], 0.0]
            );
        }
        t.check_physical_pairs(&l, &physics)
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        l.check_parts(&t).unwrap();
    }
}

/// REQ-BUILD-005: the A.1 / A.3 fixtures stay verbatim in SPEC.md and unchanged, and the A.3
/// layout resolves against the A.1 table.
#[test]
fn req_build_005_appendix_fixtures_unchanged() {
    let spec = include_str!("../../../docs/SPEC.md");
    for f in [
        include_str!("../fixtures/table.toml"),
        include_str!("../fixtures/layout.toml"),
        include_str!("../fixtures/backglass.toml"),
    ] {
        assert!(spec.contains(f));
    }
    let t = TableManifest::from_toml_str(include_str!("../fixtures/table.toml")).unwrap();
    let parts = t.all_parts(&layout()).unwrap();
    assert_eq!(parts.len(), 6);
}
