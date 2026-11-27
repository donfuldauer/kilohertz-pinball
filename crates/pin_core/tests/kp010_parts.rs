//! KP-010 (#90) acceptance A, D, E: `[[part_styles]]` / `[[parts]]` round trip and unknown
//! keys, style precedence (REQ-BUILD-003), binds, and placement in the playfield frame
//! (REQ-BUILD-001, REQ-BUILD-002).

mod kp010_common;

use kp010_common::{PARTS_TABLE, invalid_table, layout, minimal_table, ok_table, table};
use pin_core::schema::parts::{
    MAX_STYLE_DEPTH, PartKind, PartLibrary, Placement, Shape, StyleField, StyleFields,
};
use pin_core::schema::{SCHEMA_VERSION, SchemaError};
use pin_core::{LayoutManifest, Material, TableManifest};

const A1_TABLE: &str = include_str!("../fixtures/table.toml");

fn a1_with(extra: &str) -> Result<TableManifest, SchemaError> {
    TableManifest::from_toml_str(&format!("{A1_TABLE}\n{extra}"))
}

fn a1_invalid(extra: &str) -> String {
    match a1_with(extra) {
        Err(SchemaError::Invalid(m)) => m,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// Builds `[[parts]]` text for one unbound part.
fn part(id: &str, kind: &str, rest: &str) -> String {
    format!("[[parts]]\nid = \"{id}\"\nkind = \"{kind}\"\nposition = [10.0, 20.0, 0.0]\n{rest}\n")
}

fn resolved(t: &TableManifest, id: &str) -> pin_core::schema::parts::ResolvedPart {
    let lib = t.part_library().unwrap();
    t.resolve_parts(&lib)
        .unwrap()
        .into_iter()
        .find(|p| p.name == id)
        .unwrap()
}

// ---------------------------------------------------------------- A. round trip, unknown keys

/// REQ-BUILD-005: the A.5 example parses and round-trips equal.
#[test]
fn req_build_005_parts_example_round_trips() {
    let t = table(PARTS_TABLE).unwrap();
    assert_eq!(t.parts.len(), 6);
    assert_eq!(t.part_styles.len(), 2);
    assert_eq!(t.materials.len(), 3);
    assert!(t.theme.is_some());
    let text = t.to_toml_string().unwrap();
    assert_eq!(table(&text).unwrap(), t);
}

/// REQ-BUILD-005: an unknown key at every nesting level of every new section is a parse error.
#[test]
fn req_build_005_unknown_keys_rejected_at_every_level() {
    let lines: Vec<&str> = PARTS_TABLE.lines().collect();
    let mut checked = 0;
    for (i, l) in lines.iter().enumerate() {
        let header = l.starts_with('[');
        let inline = l.contains("= {");
        if !header && !inline {
            continue;
        }
        let mut v: Vec<String> = lines.iter().map(|s| s.to_string()).collect();
        if header {
            v.insert(i + 1, "bogus_key = 1".into());
        } else {
            v[i] = l.replacen("= {", "= { bogus_key = 1,", 1);
        }
        let src = v.join("\n");
        match table(&src) {
            Err(SchemaError::Parse(m)) => assert!(m.contains("bogus_key"), "{m}"),
            other => panic!("line {i} `{l}`: expected Parse, got {other:?}"),
        }
        checked += 1;
    }
    // meta, balls, cabinet_switches, lamps, mechs, theme, 3 materials, 2 styles, 6 parts
    // (16 headers) + fonts and the 3 inline `overrides` and the decoration outline line.
    assert!(checked >= 20, "{checked}");
    // And at the top level.
    let src = PARTS_TABLE.replacen(
        "schema_version = 1\n",
        "schema_version = 1\nbogus_key = 1\n",
        1,
    );
    assert!(matches!(table(&src), Err(SchemaError::Parse(_))));
}

/// REQ-BUILD-005: a wrong `schema_version` is still `UnsupportedVersion`.
#[test]
fn req_build_005_wrong_schema_version() {
    let src = PARTS_TABLE.replacen("schema_version = 1", "schema_version = 2", 1);
    assert_eq!(table(&src), Err(SchemaError::UnsupportedVersion(2)));
    let mut t = table(PARTS_TABLE).unwrap();
    t.schema_version = SCHEMA_VERSION + 1;
    assert!(matches!(
        t.to_toml_string(),
        Err(SchemaError::UnsupportedVersion(_))
    ));
}

/// REQ-BUILD-005: non-finite floats in the new sections are rejected naming the key path.
#[test]
fn req_build_005_non_finite_rejected_naming_path() {
    for (from, to, path) in [
        (
            "roughness = 0.8",
            "roughness = inf",
            "materials[0].roughness",
        ),
        (
            "diameter = 11.0",
            "diameter = nan",
            "part_styles[0].diameter",
        ),
        (
            "position = [120.0, 300.0, 0.0]",
            "position = [-inf, 300.0, 0.0]",
            "parts[0].position[0]",
        ),
        (
            "section = 4.8",
            "section = nan",
            "parts[0].overrides.section",
        ),
        (
            "rotation_deg = [0.0, 0.0, 30.0]",
            "rotation_deg = [0.0, 0.0, nan]",
            "parts[1].rotation_deg[2]",
        ),
    ] {
        let src = PARTS_TABLE.replacen(from, to, 1);
        assert_ne!(src, PARTS_TABLE, "{from}");
        match table(&src) {
            Err(SchemaError::Invalid(m)) => assert!(m.contains(path), "{path}: {m}"),
            other => panic!("{path}: {other:?}"),
        }
    }
}

/// REQ-BUILD-005: `layout.toml` keeps its keys; style keys on its elements stay unknown
/// (styles come from `table.toml` `[[parts]]` with `bind`).
#[test]
fn req_build_005_layout_has_no_style_keys() {
    let base = include_str!("../fixtures/layout.toml");
    for (anchor, key) in [
        ("rubber = true", "style = \"post_rubber\""),
        ("material = \"wood\"", "surface = \"wood_painted\""),
        ("size = 18.0", "id = \"x\""),
        (
            "entry_width = 60.0",
            "overrides = { material = \"chrome\" }",
        ),
        ("width = 32.0", "rotation_deg = [0.0, 0.0, 1.0]"),
    ] {
        let src = base.replacen(anchor, &format!("{anchor}\n{key}"), 1);
        assert!(
            matches!(
                LayoutManifest::from_toml_str(&src),
                Err(SchemaError::Parse(_))
            ),
            "{key}"
        );
    }
    let src = format!("{base}\n[[parts]]\nid = \"p\"\nkind = \"plastic\"\n");
    assert!(matches!(
        LayoutManifest::from_toml_str(&src),
        Err(SchemaError::Parse(_))
    ));
}

// ---------------------------------------------------------------- D. kinds and precedence

/// REQ-BUILD-003: the kind list is the REQ-BUILD-003 part list plus `wall`, `lane_guide`,
/// `deck` (layout elements) and `decoration`.
#[test]
fn req_build_003_part_kind_list() {
    let names: Vec<&str> = PartKind::ALL.iter().map(|k| k.name()).collect();
    assert_eq!(
        names,
        [
            "post",
            "star_post",
            "rubber_ring",
            "pop_bumper",
            "slingshot",
            "flipper_bat",
            "flipper_rubber",
            "drop_target",
            "standup_target",
            "spinner",
            "gate",
            "lane_guide",
            "wall",
            "wireform",
            "plastic_ramp",
            "deck",
            "plastic",
            "gi_bulb",
            "flasher_dome",
            "insert",
            "decoration",
        ]
    );
    for &k in PartKind::ALL {
        assert_eq!(PartKind::from_name(k.name()), Some(k));
        assert!(k.allows(StyleField::Material), "{k:?}");
    }
    assert_eq!(PartKind::from_name("toaster"), None);
    let visual: Vec<PartKind> = PartKind::ALL
        .iter()
        .copied()
        .filter(|k| !k.is_touchable())
        .collect();
    assert_eq!(
        visual,
        [
            PartKind::Plastic,
            PartKind::GiBulb,
            PartKind::FlasherDome,
            PartKind::Insert,
            PartKind::Decoration
        ]
    );
}

/// REQ-BUILD-003: every kind has a built-in default style (id = kind name, no base), and every
/// built-in style resolves to its kind.
#[test]
fn req_build_003_every_kind_has_a_builtin_default_and_every_builtin_resolves() {
    let lib = PartLibrary::builtin().unwrap();
    for &k in PartKind::ALL {
        let s = lib.style(k.name()).unwrap();
        assert_eq!(s.kind, k);
        assert!(s.base.is_none());
        assert!(s.material.is_some(), "{k:?} default has a material");
    }
    let ids = lib.style_ids();
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "sorted");
    assert!(ids.len() > PartKind::ALL.len());
    for id in ids {
        let r = lib.resolve_style(id).unwrap();
        assert_eq!(r.kind, lib.style(id).unwrap().kind);
        r.fields.validate(r.kind, id).unwrap();
    }
    assert!(lib.resolve_style("nope").is_err());
}

/// REQ-BUILD-003: kind default layer (no style named).
#[test]
fn req_build_003_precedence_kind_default() {
    let t = ok_table(&part("b", "pop_bumper", ""));
    let r = resolved(&t, "b");
    assert_eq!(r.style, "pop_bumper");
    assert_eq!(r.fields.diameter, Some(76.0));
    assert_eq!(r.material.unwrap().def.id, "plastic_white");
}

/// REQ-BUILD-003: a built-in style over the kind default.
#[test]
fn req_build_003_precedence_builtin_style() {
    let t = ok_table(&part("p", "post", "style = \"post_chrome\""));
    let r = resolved(&t, "p");
    assert_eq!(r.material.as_ref().unwrap().def.id, "chrome");
    assert_eq!(r.physical(), Some(Material::Steel));
    assert_eq!(r.fields.diameter, Some(10.0), "from the kind default");
    assert_eq!(r.fields.shape, Some(Shape::Round));
}

/// REQ-BUILD-003: a table style (with a built-in base) over the built-in style.
#[test]
fn req_build_003_precedence_table_style() {
    let t = ok_table(&format!(
        "[[part_styles]]\nid = \"fat\"\nkind = \"post\"\nbase = \"post_chrome\"\ndiameter = 12.0\n\n{}",
        part("p", "post", "style = \"fat\"")
    ));
    let r = resolved(&t, "p");
    assert_eq!(r.fields.diameter, Some(12.0));
    assert_eq!(r.material.unwrap().def.id, "chrome");
}

/// REQ-BUILD-003: the part instance over the table style.
#[test]
fn req_build_003_precedence_instance_override() {
    let t = ok_table(&format!(
        "[[part_styles]]\nid = \"fat\"\nkind = \"post\"\nbase = \"post_chrome\"\ndiameter = 12.0\n\n{}",
        part(
            "p",
            "post",
            "style = \"fat\"\nmaterial = \"brass\"\noverrides = { diameter = 14.0 }"
        )
    ));
    let r = resolved(&t, "p");
    assert_eq!(r.fields.diameter, Some(14.0));
    assert_eq!(r.material.unwrap().def.id, "brass");
}

/// REQ-BUILD-003: a table style with a built-in id overrides it field by field, and parts
/// and styles based on it see the change.
#[test]
fn req_build_003_table_overrides_builtin_field_by_field() {
    let t = ok_table(&format!(
        "[[part_styles]]\nid = \"post_chrome\"\nkind = \"post\"\ndiameter = 9.0\n\n\
         [[part_styles]]\nid = \"post\"\nkind = \"post\"\nheight = 30.0\n\n{}",
        part("p", "post", "style = \"post_chrome\"")
    ));
    let r = resolved(&t, "p");
    assert_eq!(r.fields.diameter, Some(9.0));
    assert_eq!(
        r.fields.height,
        Some(30.0),
        "kind default overridden by the table"
    );
    assert_eq!(
        r.material.unwrap().def.id,
        "chrome",
        "kept from the built-in"
    );
    let lib = t.part_library().unwrap();
    assert_eq!(lib.style("post_chrome").unwrap().diameter, Some(9.0));
}

/// REQ-BUILD-003: the four layers in one sweep over every kind (material field, which every
/// kind accepts).
#[test]
fn req_build_003_precedence_sweep_every_kind() {
    let mut src = String::new();
    for m in ["m_kind", "m_table", "m_inst"] {
        src += &format!(
            "[[materials]]\nid = \"{m}\"\nbase_color = \"#102030\"\nphysical = \"plastic\"\n\n"
        );
    }
    for &k in PartKind::ALL {
        let n = k.name();
        src += &format!("[[part_styles]]\nid = \"{n}\"\nkind = \"{n}\"\nmaterial = \"m_kind\"\n\n");
        src +=
            &format!("[[part_styles]]\nid = \"t_{n}\"\nkind = \"{n}\"\nmaterial = \"m_table\"\n\n");
    }
    for &k in PartKind::ALL {
        let n = k.name();
        src += &part(&format!("a_{n}"), n, "");
        src += &part(&format!("b_{n}"), n, &format!("style = \"t_{n}\""));
        src += &part(
            &format!("c_{n}"),
            n,
            &format!("style = \"t_{n}\"\nmaterial = \"m_inst\""),
        );
    }
    let t = ok_table(&src);
    let lib = t.part_library().unwrap();
    let parts = t.resolve_parts(&lib).unwrap();
    let builtin = PartLibrary::builtin().unwrap();
    for &k in PartKind::ALL {
        let n = k.name();
        let get = |p: String| {
            parts
                .iter()
                .find(|r| r.name == p)
                .unwrap()
                .material
                .clone()
                .unwrap()
                .def
                .id
        };
        assert_eq!(get(format!("a_{n}")), "m_kind", "{n}");
        assert_eq!(get(format!("b_{n}")), "m_table", "{n}");
        assert_eq!(get(format!("c_{n}")), "m_inst", "{n}");
        // Without the table, the same part gets the built-in default.
        let bare = builtin.resolve_style(n).unwrap();
        assert_ne!(bare.fields.material.as_deref(), Some("m_kind"));
    }
}

/// REQ-BUILD-003: `base` cycles are errors naming the style.
#[test]
fn req_build_003_base_cycle_names_the_style() {
    let m = invalid_table(
        "[[part_styles]]\nid = \"aa\"\nkind = \"post\"\nbase = \"bb\"\n\n\
         [[part_styles]]\nid = \"bb\"\nkind = \"post\"\nbase = \"aa\"\n",
    );
    assert!(m.contains("cycle") && m.contains("`aa`"), "{m}");
    let m = invalid_table("[[part_styles]]\nid = \"self\"\nkind = \"post\"\nbase = \"self\"\n");
    assert!(m.contains("cycle") && m.contains("`self`"), "{m}");
}

/// REQ-BUILD-003, R11: chains are bounded at `MAX_STYLE_DEPTH` styles.
#[test]
fn req_build_003_chain_depth_bounded() {
    let chain = |n: usize| {
        let mut s = String::new();
        for i in 0..n {
            let base = if i + 1 < n {
                format!("base = \"s{}\"\n", i + 1)
            } else {
                String::new()
            };
            s += &format!("[[part_styles]]\nid = \"s{i}\"\nkind = \"post\"\n{base}\n");
        }
        s
    };
    // n table styles + the kind default.
    ok_table(&chain(MAX_STYLE_DEPTH - 1));
    let m = invalid_table(&chain(MAX_STYLE_DEPTH));
    assert!(m.contains("deeper") && m.contains("`s0`"), "{m}");
}

/// REQ-BUILD-003: style errors name the style.
#[test]
fn req_build_003_style_errors_name_the_style() {
    for (src, want) in [
        (
            "[[part_styles]]\nid = \"x1\"\nkind = \"post\"\nbase = \"nope\"\n",
            "nope",
        ),
        (
            "[[part_styles]]\nid = \"x2\"\nkind = \"post\"\nbase = \"wall\"\n",
            "x2",
        ),
        ("[[part_styles]]\nid = \"post\"\nkind = \"wall\"\n", "post"),
        (
            "[[part_styles]]\nid = \"post_chrome\"\nkind = \"post\"\nbase = \"post\"\n",
            "post_chrome",
        ),
        (
            "[[part_styles]]\nid = \"x3\"\nkind = \"post\"\n\n[[part_styles]]\nid = \"x3\"\nkind = \"post\"\n",
            "x3",
        ),
        (
            "[[part_styles]]\nid = \"x4\"\nkind = \"wall\"\ndiameter = 3.0\n",
            "diameter",
        ),
        (
            "[[part_styles]]\nid = \"x5\"\nkind = \"post\"\nmaterial = \"unobtainium\"\n",
            "unobtainium",
        ),
        (
            "[[part_styles]]\nid = \"x6\"\nkind = \"rubber_ring\"\nring_size = \"ring_999\"\n",
            "ring_999",
        ),
        (
            "[[part_styles]]\nid = \"Bad Id\"\nkind = \"post\"\n",
            "Bad Id",
        ),
    ] {
        let m = invalid_table(src);
        assert!(m.contains(want), "{want}: {m}");
    }
}

/// REQ-BUILD-003: a field that does not apply to the kind is an error naming part and field.
#[test]
fn req_build_003_field_not_applicable_names_part_and_field() {
    let m = invalid_table(&part("left_wall", "wall", "overrides = { diameter = 5.0 }"));
    assert!(
        m.contains("left_wall") && m.contains("`diameter`") && m.contains("wall"),
        "{m}"
    );
    // Every (kind, field) pair outside the kind's list is rejected; inside it is accepted.
    for &k in PartKind::ALL {
        for &f in StyleField::ALL {
            let mut s = StyleFields::default();
            match f {
                StyleField::Shape => s.shape = Some(Shape::Round),
                StyleField::StarPoints => s.star_points = Some(5),
                StyleField::TubeSides => s.tube_sides = Some(8),
                StyleField::Outline => s.outline = Some(vec![[0.0, 0.0]; 3]),
                StyleField::RingSize => s.ring_size = Some("ring_8".into()),
                StyleField::Material => s.material = Some("steel".into()),
                StyleField::TrimMaterial => s.trim_material = Some("steel".into()),
                StyleField::Color => s.color = Some("#000000".into()),
                StyleField::Size => s.size = Some(1.0),
                StyleField::Diameter => s.diameter = Some(1.0),
                StyleField::Width => s.width = Some(1.0),
                StyleField::Length => s.length = Some(1.0),
                StyleField::Height => s.height = Some(1.0),
                StyleField::Thickness => s.thickness = Some(1.0),
                StyleField::InnerDiameter => s.inner_diameter = Some(1.0),
                StyleField::Section => s.section = Some(1.0),
                StyleField::TubeRadius => s.tube_radius = Some(1.0),
                StyleField::Lift => s.lift = Some(1.0),
            }
            assert!(s.is_set(f));
            let r = s.validate(k, "p");
            assert_eq!(r.is_ok(), k.allows(f), "{k:?} {f:?} {r:?}");
            if let Err(SchemaError::Invalid(m)) = r {
                assert!(m.contains(f.name()));
            }
            s.clear(f);
            assert!(!s.is_set(f));
        }
    }
}

/// REQ-BUILD-003: value ranges of style fields.
#[test]
fn req_build_003_field_ranges() {
    for (kind, o, want) in [
        ("post", "diameter = 0.05", "diameter"),
        ("post", "diameter = 1001.0", "diameter"),
        ("star_post", "star_points = 2", "star_points"),
        ("star_post", "star_points = 25", "star_points"),
        ("wireform", "tube_sides = 2", "tube_sides"),
        ("insert", "lift = -1.0", "lift"),
        ("post", "color = \"red\"", "color"),
        ("post", "material = \"Steel\"", "material"),
        ("plastic", "shape = \"polygon\"", "polygon"),
        (
            "plastic",
            "shape = \"polygon\", outline = [[0.0, 0.0], [1.0, 0.0]]",
            "polygon",
        ),
        ("plastic", "outline = [[6000.0, 0.0]]", "outline[0]"),
        ("star_post", "diameter = 5.0", "__ok__"),
        // SPEC A.5: `star` (needs `star_points`), for every kind (gate r1 finding 4).
        ("decoration", "shape = \"star\"", "needs `star_points`"),
        ("post", "shape = \"star\"", "needs `star_points`"),
        ("decoration", "shape = \"star\", star_points = 5", "__ok__"),
    ] {
        let src = part("pp", kind, &format!("overrides = {{ {o} }}"));
        if want == "__ok__" {
            ok_table(&src);
        } else {
            let m = invalid_table(&src);
            assert!(m.contains(want) && m.contains("pp"), "{o}: {m}");
        }
    }
    let many: Vec<String> = (0..1025).map(|i| format!("[{i}.0, 0.0]")).collect();
    let m = invalid_table(&part(
        "pp",
        "plastic",
        &format!("overrides = {{ outline = [{}] }}", many.join(", ")),
    ));
    assert!(m.contains("1025"), "{m}");
}

/// REQ-BUILD-003: ring sizes supply inner diameter and section unless given.
#[test]
fn req_build_003_ring_sizes() {
    let lib = PartLibrary::builtin().unwrap();
    let ids: Vec<&str> = lib.ring_size_ids().collect();
    assert!(ids.len() >= 6);
    assert!(ids.windows(2).all(|w| w[0] < w[1]));
    let t = ok_table(&format!(
        "{}{}",
        part(
            "r1",
            "rubber_ring",
            "overrides = { ring_size = \"ring_51\" }"
        ),
        part(
            "r2",
            "rubber_ring",
            "style = \"rubber_ring_red\"\noverrides = { ring_size = \"ring_51\", section = 6.0 }"
        )
    ));
    let r1 = resolved(&t, "r1");
    let size = lib.ring_size("ring_51").unwrap();
    assert_eq!(r1.fields.inner_diameter, Some(size.inner_diameter));
    assert_eq!(r1.fields.section, Some(size.section));
    let r2 = resolved(&t, "r2");
    assert_eq!(r2.fields.section, Some(6.0));
    assert_eq!(r2.material.unwrap().def.id, "rubber_red");
    let m = invalid_table(&part(
        "r3",
        "rubber_ring",
        "overrides = { ring_size = \"ring_0\" }",
    ));
    assert!(m.contains("r3") && m.contains("ring_0"), "{m}");
}

/// REQ-BUILD-003: part-level errors name the part.
#[test]
fn req_build_003_part_errors_name_the_part() {
    for (src, want) in [
        (
            part("dup", "post", "") + &part("dup", "post", ""),
            "duplicate",
        ),
        (part("q1", "post", "style = \"wall\""), "q1"),
        (part("q2", "post", "style = \"nope\""), "nope"),
        (
            part(
                "q3",
                "post",
                "material = \"steel\"\noverrides = { material = \"steel\" }",
            ),
            "q3",
        ),
        (
            "[[parts]]\nid = \"q4\"\nkind = \"post\"\n".to_string(),
            "needs `position`",
        ),
        (part("Q5", "post", ""), "Q5"),
    ] {
        let m = invalid_table(&src);
        assert!(m.contains(want), "{want}: {m}");
    }
}

// ---------------------------------------------------------------- binds

/// REQ-BUILD-001: binds to table elements place the part at the element.
#[test]
fn req_build_001_bind_to_table_elements() {
    let t = a1_with(
        "[[parts]]\nid = \"cap\"\nkind = \"pop_bumper\"\nbind = \"bumper_1\"\n\n\
         [[parts]]\nid = \"bat\"\nkind = \"flipper_bat\"\nbind = \"flippers:lower_left\"\n\n\
         [[parts]]\nid = \"rub\"\nkind = \"flipper_rubber\"\nbind = \"lower_left\"\nstyle = \"flipper_rubber_red\"\n\n\
         [[parts]]\nid = \"wire\"\nkind = \"wireform\"\nbind = \"return_wireform\"\nstyle = \"wireform_chrome\"\n\n\
         [[parts]]\nid = \"lens\"\nkind = \"insert\"\nbind = \"overdrive_ready\"\nposition = [257.0, 240.0, 0.0]\n\n\
         [[parts]]\nid = \"opto\"\nkind = \"decoration\"\nbind = \"switches:right_orbit_opto\"\n",
    )
    .unwrap();
    let lib = t.part_library().unwrap();
    let parts = t.resolve_parts(&lib).unwrap();
    let get = |id: &str| parts.iter().find(|p| p.name == id).unwrap();
    assert_eq!(get("cap").placement.position, [260.0, 820.0, 0.0]);
    assert_eq!(get("cap").bind.as_deref(), Some("bumper_1"));
    assert_eq!(get("bat").placement.position, t.flippers[0].base_position);
    assert_eq!(get("rub").material.as_ref().unwrap().def.id, "rubber_red");
    assert_eq!(get("wire").placement.position, [0.0; 3]);
    assert_eq!(get("lens").placement.position, [257.0, 240.0, 0.0]);
    assert_eq!(get("opto").placement.position, [410.0, 720.0, 0.0]);
    let text = t.to_toml_string().unwrap();
    assert_eq!(TableManifest::from_toml_str(&text).unwrap(), t);
}

/// REQ-BUILD-001: bind errors name the part.
#[test]
fn req_build_001_bind_errors_name_the_part() {
    for (src, want) in [
        (
            "[[parts]]\nid = \"b1\"\nkind = \"pop_bumper\"\nbind = \"nothing_here\"\n",
            "matches no",
        ),
        (
            "[[parts]]\nid = \"b2\"\nkind = \"slingshot\"\nbind = \"bumper_1\"\n",
            "cannot bind",
        ),
        (
            "[[parts]]\nid = \"b3\"\nkind = \"pop_bumper\"\nbind = \"bumper_1\"\nposition = [1.0, 1.0, 0.0]\n",
            "has its own",
        ),
        (
            "[[parts]]\nid = \"b4\"\nkind = \"insert\"\nbind = \"overdrive_ready\"\n",
            "has none",
        ),
        (
            "[[parts]]\nid = \"b5\"\nkind = \"pop_bumper\"\nbind = \"gadgets:bumper_1\"\n",
            "gadgets",
        ),
        (
            "[[parts]]\nid = \"b6\"\nkind = \"pop_bumper\"\nbind = \"bumper_1\"\n\n[[parts]]\nid = \"b7\"\nkind = \"pop_bumper\"\nbind = \"mechs:bumper_1\"\n",
            "already bound",
        ),
        (
            "[[parts]]\nid = \"b8\"\nkind = \"post\"\nbind = \"posts[0]\"\nposition = [1.0, 1.0, 0.0]\n",
            "not allowed",
        ),
        (
            "[[parts]]\nid = \"b9\"\nkind = \"wall\"\nbind = \"posts[0]\"\n",
            "cannot bind",
        ),
        (
            "[[parts]]\nid = \"c1\"\nkind = \"post\"\nbind = \"posts[0]\"\n\n[[parts]]\nid = \"c2\"\nkind = \"post\"\nbind = \"posts[0]\"\n",
            "already bound",
        ),
        (
            "[[parts]]\nid = \"c3\"\nkind = \"post\"\nbind = \"\"\n",
            "c3",
        ),
    ] {
        let m = a1_invalid(src);
        assert!(m.contains(want), "{want}: {m}");
        let id = src.split('"').nth(1).unwrap();
        let last_id = src
            .rsplit("id = \"")
            .next()
            .unwrap()
            .split('"')
            .next()
            .unwrap();
        assert!(m.contains(id) || m.contains(last_id), "{id}: {m}");
    }
    // An id in two sections is ambiguous unless qualified.
    let t = minimal_table(
        "[[switches]]\nid = \"dual\"\nnumber = 9\nkind = \"ring\"\nposition = [1.0, 2.0, 0.0]\n\n\
         [[mechs]]\nid = \"dual\"\nkind = \"bumper\"\nposition = [1.0, 2.0, 0.0]\n\n",
    );
    let amb = format!("{t}[[parts]]\nid = \"amb\"\nkind = \"pop_bumper\"\nbind = \"dual\"\n");
    match table(&amb) {
        Err(SchemaError::Invalid(m)) => {
            assert!(m.contains("ambiguous") && m.contains("amb"), "{m}")
        }
        other => panic!("{other:?}"),
    }
    let qual = format!("{t}[[parts]]\nid = \"q\"\nkind = \"pop_bumper\"\nbind = \"mechs:dual\"\n");
    table(&qual).unwrap();
}

/// REQ-BUILD-001: a layout bind applies the part to the element and is checked against it.
#[test]
fn req_build_001_layout_bind() {
    let t = table(PARTS_TABLE).unwrap();
    let l = layout();
    let parts = t.all_parts(&l).unwrap();
    let post = parts.iter().find(|p| p.name == "posts[0]").unwrap();
    assert_eq!(post.material.as_ref().unwrap().def.id, "rubber_black");
    assert_eq!(post.physical(), Some(Material::Rubber));
    assert_eq!(
        post.fields.diameter,
        Some(10.0),
        "legacy radius 5 wins over the style's 11"
    );
    assert_eq!(post.style, "post_rubber");
    // Index out of range.
    let src = PARTS_TABLE.replacen("bind = \"posts[0]\"", "bind = \"posts[7]\"", 1);
    let t2 = table(&src).unwrap();
    match t2.all_parts(&l) {
        Err(SchemaError::Invalid(m)) => assert!(
            m.contains("left_post_look") && m.contains("posts[7]"),
            "{m}"
        ),
        other => panic!("{other:?}"),
    }
    assert!(l.check_parts(&t2).is_err());
    l.check_parts(&t).unwrap();
}

/// REQ-BUILD-001: every layout section accepts its bind kinds.
#[test]
fn req_build_001_layout_bind_every_section() {
    let t = a1_with(
        "[[parts]]\nid = \"w\"\nkind = \"wall\"\nbind = \"walls[0]\"\nmaterial = \"wood_painted\"\n\n\
         [[parts]]\nid = \"p\"\nkind = \"rubber_ring\"\nbind = \"posts[0]\"\n\n\
         [[parts]]\nid = \"l\"\nkind = \"lane_guide\"\nbind = \"lanes[0]\"\noverrides = { height = 20.0 }\n\n\
         [[parts]]\nid = \"r\"\nkind = \"plastic_ramp\"\nbind = \"ramps[0]\"\n\n\
         [[parts]]\nid = \"d\"\nkind = \"deck\"\nbind = \"decks[0]\"\nstyle = \"deck\"\nmaterial = \"acrylic_clear\"\n\n\
         [[parts]]\nid = \"i\"\nkind = \"insert\"\nbind = \"inserts[0]\"\nrotation_deg = [0.0, 0.0, 45.0]\n",
    )
    .unwrap();
    let parts = t.all_parts(&layout()).unwrap();
    let get = |n: &str| parts.iter().find(|p| p.name == n).unwrap();
    assert_eq!(
        get("walls[0]").material.as_ref().unwrap().def.id,
        "wood_painted"
    );
    assert_eq!(get("posts[0]").kind, PartKind::RubberRing);
    assert_eq!(
        get("posts[0]").fields.diameter,
        None,
        "rubber rings take no diameter"
    );
    assert_eq!(get("lanes[0]").fields.height, Some(20.0));
    assert_eq!(get("ramps[0]").kind, PartKind::PlasticRamp);
    assert_eq!(
        get("decks[0]").material.as_ref().unwrap().def.id,
        "acrylic_clear"
    );
    assert_eq!(get("decks[0]").placement.position, [0.0, 0.0, 65.0]);
    assert_eq!(get("inserts[0]").placement.rotation_deg, [0.0, 0.0, 45.0]);
}

// ---------------------------------------------------------------- E. placement

/// REQ-BUILD-001, REQ-BUILD-002: every kind, visual-only included, round-trips position and
/// rotation; the default rotation is zero.
#[test]
fn req_build_002_every_kind_round_trips_placement() {
    let mut src = String::new();
    for (i, &k) in PartKind::ALL.iter().enumerate() {
        let f = i as f32;
        src += &format!(
            "[[parts]]\nid = \"k{i}\"\nkind = \"{}\"\nposition = [{}, {}, {}]\nrotation_deg = [{}, {}, {}]\n\n",
            k.name(),
            f * 10.5,
            f * 20.25,
            f,
            f,
            -f,
            2.0 * f
        );
        src += &format!(
            "[[parts]]\nid = \"z{i}\"\nkind = \"{}\"\nposition = [1.0, 2.0, 3.0]\n\n",
            k.name()
        );
    }
    let t = ok_table(&src);
    let text = t.to_toml_string().unwrap();
    assert_eq!(table(&text).unwrap(), t);
    let lib = t.part_library().unwrap();
    let parts = t.resolve_parts(&lib).unwrap();
    for (i, &k) in PartKind::ALL.iter().enumerate() {
        let f = i as f32;
        let p = parts.iter().find(|p| p.name == format!("k{i}")).unwrap();
        assert_eq!(p.kind, k);
        assert_eq!(p.placement.position, [f * 10.5, f * 20.25, f]);
        assert_eq!(p.placement.rotation_deg, [f, -f, 2.0 * f]);
        let z = parts.iter().find(|p| p.name == format!("z{i}")).unwrap();
        assert_eq!(z.placement.rotation_deg, [0.0; 3]);
        assert_eq!(z.placement.position, [1.0, 2.0, 3.0]);
    }
}

fn close(a: [f64; 3], b: [f64; 3]) -> bool {
    a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-12)
}

/// REQ-BUILD-002: the documented rotation convention (A.5): about fixed x, then y, then z,
/// counter-clockwise from the positive axis.
#[test]
fn req_build_002_rotation_convention_analytic() {
    let at = |r: [f32; 3]| Placement {
        position: [0.0; 3],
        rotation_deg: r,
    };
    assert!(close(
        at([0.0, 0.0, 90.0]).to_playfield([1.0, 0.0, 0.0]),
        [0.0, 1.0, 0.0]
    ));
    assert!(close(
        at([90.0, 0.0, 0.0]).to_playfield([0.0, 1.0, 0.0]),
        [0.0, 0.0, 1.0]
    ));
    assert!(close(
        at([0.0, 90.0, 0.0]).to_playfield([0.0, 0.0, 1.0]),
        [1.0, 0.0, 0.0]
    ));
    assert!(close(
        at([0.0, 90.0, 0.0]).to_playfield([1.0, 0.0, 0.0]),
        [0.0, 0.0, -1.0]
    ));
    // Order: x first, then z. Local +y -> (x 90) +z -> (z 90) +z; local +x -> +x -> +y.
    let p = Placement {
        position: [100.0, 200.0, 5.0],
        rotation_deg: [90.0, 0.0, 90.0],
    };
    assert!(close(p.to_playfield([0.0, 1.0, 0.0]), [100.0, 200.0, 6.0]));
    assert!(close(p.to_playfield([1.0, 0.0, 0.0]), [100.0, 201.0, 5.0]));
    // z first would have sent local +x to +y and then (x 90) to +z: the order matters.
    assert!(!close(p.to_playfield([1.0, 0.0, 0.0]), [100.0, 200.0, 6.0]));
    // Identity and orthonormality.
    let r = at([0.0; 3]).rotation_matrix();
    assert_eq!(r, [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]);
    let r = at([12.0, -33.0, 271.0]).rotation_matrix();
    for i in 0..3 {
        for j in 0..3 {
            let d: f64 = (0..3).map(|k| r[i][k] * r[j][k]).sum();
            assert!((d - if i == j { 1.0 } else { 0.0 }).abs() < 1e-12);
        }
    }
}

/// REQ-BUILD-002: placement out of range is an error naming the part.
#[test]
fn req_build_002_placement_out_of_range() {
    for (pos, rot, want) in [
        ("[5001.0, 0.0, 0.0]", "[0.0, 0.0, 0.0]", "position[0]"),
        ("[0.0, 0.0, -5001.0]", "[0.0, 0.0, 0.0]", "position[2]"),
        ("[0.0, 0.0, 0.0]", "[0.0, 361.0, 0.0]", "rotation_deg[1]"),
        ("[0.0, 0.0, 0.0]", "[-361.0, 0.0, 0.0]", "rotation_deg[0]"),
    ] {
        let m = invalid_table(&format!(
            "[[parts]]\nid = \"far\"\nkind = \"gi_bulb\"\nposition = {pos}\nrotation_deg = {rot}\n"
        ));
        assert!(m.contains("far") && m.contains(want), "{want}: {m}");
    }
    ok_table(
        "[[parts]]\nid = \"edge\"\nkind = \"gi_bulb\"\nposition = [5000.0, -5000.0, 0.0]\nrotation_deg = [360.0, -360.0, 0.0]\n",
    );
    // A scalar rotation is not the documented form.
    assert!(matches!(
        table(&minimal_table(
            "[[parts]]\nid = \"s\"\nkind = \"gi_bulb\"\nposition = [0.0, 0.0, 0.0]\nrotation_deg = 30.0\n"
        )),
        Err(SchemaError::Parse(_))
    ));
}

/// REQ-BUILD-003: the public resolver is usable directly.
#[test]
fn req_build_003_library_resolve_direct() {
    let t = table(PARTS_TABLE).unwrap();
    let lib = t.part_library().unwrap();
    let p = &t.parts[0];
    let r = lib.resolve(p, [1.0, 2.0, 3.0], "x").unwrap();
    assert_eq!(r.placement.position, [1.0, 2.0, 3.0]);
    assert_eq!(r.material.as_ref().unwrap().def.id, "rubber_lime");
    assert_eq!(r.fields.section, Some(4.8));
    assert_eq!(r.fields.inner_diameter, Some(19.1));
    assert_eq!(p.instance_fields().section, Some(4.8));
    p.validate(0).unwrap();
}

/// REQ-BUILD-003: the public layering helpers, used directly.
#[test]
fn req_build_003_public_helpers_direct() {
    let mut lower = StyleFields {
        diameter: Some(10.0),
        material: Some("steel".into()),
        ..StyleFields::default()
    };
    let upper = StyleFields {
        diameter: Some(12.0),
        color: Some("#ffffff".into()),
        ..StyleFields::default()
    };
    lower.overlay(&upper);
    assert_eq!(lower.diameter, Some(12.0));
    assert_eq!(lower.material.as_deref(), Some("steel"));
    assert_eq!(lower.color.as_deref(), Some("#ffffff"));
    let t = table(PARTS_TABLE).unwrap();
    let style = &t.part_styles[1];
    let f = style.fields();
    assert_eq!(f.ring_size.as_deref(), Some("ring_19"));
    assert_eq!(f.material.as_deref(), Some("rubber_lime"));
    let lib = PartLibrary::with_table(&t.materials, &t.part_styles).unwrap();
    assert_eq!(lib, t.part_library().unwrap());
    assert_eq!(
        lib.style("lime_ring").unwrap().base.as_deref(),
        Some("rubber_ring_white")
    );
    assert_eq!(
        lib.style("post").unwrap().diameter,
        Some(11.0),
        "the table's override"
    );
    let r = lib.resolve_style("lime_ring").unwrap();
    assert_eq!(r.kind, PartKind::RubberRing);
    assert_eq!(r.fields.material.as_deref(), Some("rubber_lime"));
    let theme = t.theme.clone().unwrap();
    theme.validate().unwrap();
    let mut bad = theme;
    bad.palette = Some(vec!["#zzzzzz".into()]);
    assert!(
        bad.validate()
            .unwrap_err()
            .to_string()
            .contains("theme.palette[0]")
    );
}
