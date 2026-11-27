//! KP-010 (#90) gate review round 1 (PR #269): regression tests for the eight CodeRabbit
//! findings on the part resolver (SPEC Appendix A.5; REQ-BUILD-003, REQ-BUILD-004,
//! REQ-BUILD-005). One test per finding, `r1_f<N>_…`.

mod kp010_common;

use kp010_common::{invalid_table, layout, ok_table};
use pin_core::schema::SchemaError;
use pin_core::schema::parts::{
    PartDef, PartKind, PartLibrary, ResolvedPart, Shape, StyleField, StyleFields,
};
use pin_core::{LayoutManifest, Material, TableManifest};

fn part_def(id: &str, kind: PartKind) -> PartDef {
    PartDef {
        id: id.into(),
        kind,
        style: None,
        bind: None,
        position: Some([10.0, 20.0, 0.0]),
        rotation_deg: [0.0; 3],
        material: None,
        overrides: None,
    }
}

fn bound(id: &str, kind: PartKind, bind: &str) -> PartDef {
    PartDef {
        bind: Some(bind.into()),
        position: None,
        ..part_def(id, kind)
    }
}

fn get<'a>(parts: &'a [ResolvedPart], name: &str) -> &'a ResolvedPart {
    parts
        .iter()
        .find(|p| p.name == name)
        .unwrap_or_else(|| panic!("no part `{name}`"))
}

fn err_text<T: std::fmt::Debug>(r: Result<T, SchemaError>) -> String {
    match r {
        Err(SchemaError::Invalid(m)) => m,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// A standalone `[[parts]]` entry at a fixed position.
fn part(id: &str, kind: &str, rest: &str) -> String {
    format!("[[parts]]\nid = \"{id}\"\nkind = \"{kind}\"\nposition = [10.0, 20.0, 0.0]\n{rest}\n")
}

/// A `[[parts]]` entry bound to `bind`.
fn bind(id: &str, kind: &str, bind: &str, rest: &str) -> String {
    format!("[[parts]]\nid = \"{id}\"\nkind = \"{kind}\"\nbind = \"{bind}\"\n{rest}\n")
}

/// The A.3 layout with its post made a steel post (`rubber = false`).
fn steel_post_layout() -> LayoutManifest {
    let mut l = layout();
    l.posts[0].rubber = false;
    l
}

/// Finding 1 (thread 4719600976), A.5 `[[parts]]`: giving both `material` and
/// `overrides.material` is an error in every entrypoint, not only at table parse.
#[test]
fn r1_f1_material_keys_exclusive_in_every_entrypoint() {
    let lib = PartLibrary::builtin().unwrap();
    let both = PartDef {
        material: Some("steel".into()),
        overrides: Some(StyleFields {
            material: Some("chrome".into()),
            ..StyleFields::default()
        }),
        ..part_def("p", PartKind::Post)
    };
    let m = err_text(lib.resolve(&both, [10.0, 20.0, 0.0], "part `p`"));
    assert!(m.contains("not both") && m.contains("part `p`"), "{m}");
    // Either spelling alone stays valid.
    let alone = PartDef {
        material: Some("steel".into()),
        ..part_def("p", PartKind::Post)
    };
    let r = lib.resolve(&alone, [0.0; 3], "part `p`").unwrap();
    assert_eq!(r.fields.material.as_deref(), Some("steel"));
    let alone = PartDef {
        overrides: Some(StyleFields {
            material: Some("chrome".into()),
            ..StyleFields::default()
        }),
        ..part_def("p", PartKind::Post)
    };
    let r = lib.resolve(&alone, [0.0; 3], "part `p`").unwrap();
    assert_eq!(r.fields.material.as_deref(), Some("chrome"));
    // A layout-bound part with both keys (both rubber, so they agree with the rubber post).
    let mut t = ok_table("");
    t.parts.push(PartDef {
        material: Some("rubber_red".into()),
        overrides: Some(StyleFields {
            material: Some("rubber_black".into()),
            ..StyleFields::default()
        }),
        ..bound("look", PartKind::Post, "posts[0]")
    });
    let l = layout();
    let lib = t.part_library().unwrap();
    let m = err_text(l.resolve_parts(&t, &lib));
    assert!(m.contains("not both") && m.contains("look"), "{m}");
    assert!(l.check_parts(&t).is_err());
}

/// Finding 2 (thread 4719600988), A.5 "the kind default's material gives way to the
/// element's": a named style that sets no material inherits the kind default's, which gives
/// way to the layout post's rubber; a material the style or instance sets still must agree.
#[test]
fn r1_f2_inherited_default_material_gives_way() {
    let l = layout();
    assert!(l.posts[0].rubber);
    let style = "[[part_styles]]\nid = \"tall_post\"\nkind = \"post\"\nheight = 20.0\n\n";
    let t = ok_table(&format!(
        "{style}{}",
        bind("look", "post", "posts[0]", "style = \"tall_post\"")
    ));
    let parts = t.all_parts(&l).unwrap();
    let p = get(&parts, "posts[0]");
    assert_eq!(p.physical(), Some(Material::Rubber));
    assert_eq!(p.fields.material.as_deref(), Some("rubber"));
    assert_eq!(p.fields.height, Some(20.0));
    // A named style that sets a steel material on the rubber post is an error.
    let t = ok_table(&format!(
        "[[part_styles]]\nid = \"steel_post\"\nkind = \"post\"\nmaterial = \"steel\"\n\n{}",
        bind("look", "post", "posts[0]", "style = \"steel_post\"")
    ));
    let m = err_text(t.all_parts(&l));
    assert!(
        m.contains("look") && m.contains("looks and plays must agree"),
        "{m}"
    );
    // A style whose base sets the material counts as named too.
    let t = ok_table(&format!(
        "[[part_styles]]\nid = \"tall_chrome\"\nkind = \"post\"\nbase = \"post_chrome\"\nheight = 20.0\n\n{}",
        bind("look", "post", "posts[0]", "style = \"tall_chrome\"")
    ));
    let m = err_text(t.all_parts(&l));
    assert!(m.contains("chrome") && m.contains("looks and plays"), "{m}");
    // So is an instance material.
    let t = ok_table(&format!(
        "{style}{}",
        bind(
            "look",
            "post",
            "posts[0]",
            "style = \"tall_post\"\nmaterial = \"steel\""
        )
    ));
    let m = err_text(t.all_parts(&l));
    assert!(
        m.contains("look") && m.contains("looks and plays must agree"),
        "{m}"
    );
}

/// Finding 3 (thread 4719601001), A.5 "a post's `rubber` (physical `rubber`, else `steel`)":
/// the element's physical material survives a table replacing that library entry with one
/// that plays differently; a replacement that plays the same supplies the finish.
#[test]
fn r1_f3_fallback_survives_catalog_replacement() {
    let l = steel_post_layout();
    let builtin_steel = PartLibrary::builtin()
        .unwrap()
        .material("steel")
        .unwrap()
        .clone();
    let t = ok_table(
        "[[materials]]\nid = \"steel\"\nbase_color = \"#123456\"\nphysical = \"rubber\"\n",
    );
    let parts = t.all_parts(&l).unwrap();
    let p = get(&parts, "posts[0]");
    assert_eq!(p.physical(), Some(Material::Steel));
    assert_eq!(p.fields.material.as_deref(), Some("steel"));
    assert_eq!(p.material.as_ref().unwrap().def, builtin_steel);
    let lib = t.part_library().unwrap();
    let direct = l.resolve_parts(&t, &lib).unwrap();
    assert_eq!(get(&direct, "posts[0]"), p);
    // A replacement that still plays as steel gives its finish.
    let t =
        ok_table("[[materials]]\nid = \"steel\"\nbase_color = \"#123456\"\nphysical = \"steel\"\n");
    let parts = t.all_parts(&l).unwrap();
    let p = get(&parts, "posts[0]");
    assert_eq!(p.physical(), Some(Material::Steel));
    assert_eq!(p.material.as_ref().unwrap().def.base_color, "#123456");
    // An explicitly chosen material that plays differently is still an error.
    let t = ok_table(&bind(
        "look",
        "post",
        "posts[0]",
        "material = \"rubber_red\"",
    ));
    let m = err_text(t.all_parts(&l));
    assert!(m.contains("look") && m.contains("rubber_red"), "{m}");
}

/// Finding 4 (thread 4719601012), A.5 "`star` (needs `star_points`)": a star shape without
/// a point count is rejected for every kind; `decoration` accepts `star_points`, so a star
/// decoration states its points; `star_post` keeps its 8-point default.
#[test]
fn r1_f4_star_needs_star_points_for_every_kind() {
    let m = invalid_table(&part("d", "decoration", "overrides = { shape = \"star\" }"));
    assert!(
        m.contains("needs `star_points`") && m.contains("`d`"),
        "{m}"
    );
    let m = invalid_table(&part("q", "post", "overrides = { shape = \"star\" }"));
    assert!(
        m.contains("needs `star_points`") && m.contains("`q`"),
        "{m}"
    );
    let lib = PartLibrary::builtin().unwrap();
    let star = PartDef {
        overrides: Some(StyleFields {
            shape: Some(Shape::Star),
            ..StyleFields::default()
        }),
        ..part_def("d", PartKind::Decoration)
    };
    let m = err_text(lib.resolve(&star, [0.0; 3], "part `d`"));
    assert!(m.contains("needs `star_points`"), "{m}");
    // The allowed combinations.
    let t = ok_table(&format!(
        "{}{}",
        part("sp", "star_post", ""),
        part(
            "d",
            "decoration",
            "overrides = { shape = \"star\", star_points = 5 }"
        )
    ));
    let parts = t.resolve_parts(&t.part_library().unwrap()).unwrap();
    assert_eq!(get(&parts, "sp").fields.star_points, Some(8));
    let d = get(&parts, "d");
    assert_eq!(
        (d.fields.shape, d.fields.star_points),
        (Some(Shape::Star), Some(5))
    );
    assert!(PartKind::Decoration.allows(StyleField::StarPoints));
}

/// Finding 5 (thread 4719601024), A.5 Validation: a part bound to a wall, post or deck gets
/// its physical material from the element, so table parsing does not require one from its
/// style; standalone parts and other binds still need one at parse.
#[test]
fn r1_f5_layout_bound_physical_checked_after_binding() {
    let l = layout();
    let lib_extra = "[[materials]]\nid = \"decal_gold\"\nbase_color = \"#d4af37\"\n\n\
                     [[part_styles]]\nid = \"post\"\nkind = \"post\"\nmaterial = \"decal_gold\"\n\n";
    let t = ok_table(&format!(
        "{lib_extra}{}",
        bind("look", "post", "posts[0]", "")
    ));
    let parts = t.all_parts(&l).unwrap();
    let p = get(&parts, "posts[0]");
    assert_eq!(p.physical(), Some(Material::Rubber));
    assert_eq!(p.fields.material.as_deref(), Some("rubber"));
    // Unbound, the layout post resolves the same.
    let t = ok_table(lib_extra);
    let parts = t.all_parts(&l).unwrap();
    assert_eq!(get(&parts, "posts[0]").physical(), Some(Material::Rubber));
    // A standalone post with the visual-only default is still rejected at parse.
    let m = invalid_table(&format!("{lib_extra}{}", part("sp", "post", "")));
    assert!(m.contains("sp") && m.contains("decal_gold"), "{m}");
    // A part bound to a mech, or to a lane (no element material), still needs a physical.
    let mech = "[[mechs]]\nid = \"b\"\nkind = \"bumper\"\nposition = [1.0, 2.0, 0.0]\n\n";
    let m = invalid_table(&format!(
        "{lib_extra}{mech}{}",
        bind("cap", "pop_bumper", "b", "material = \"decal_gold\"")
    ));
    assert!(m.contains("cap") && m.contains("decal_gold"), "{m}");
    let m = invalid_table(&format!(
        "{lib_extra}{}",
        bind("g", "lane_guide", "lanes[0]", "material = \"decal_gold\"")
    ));
    assert!(m.contains("`g`") && m.contains("decal_gold"), "{m}");
    // An unknown material on a layout-bound part is still a parse error.
    let m = invalid_table(&bind(
        "look",
        "post",
        "posts[0]",
        "material = \"unobtainium\"",
    ));
    assert!(m.contains("look") && m.contains("unobtainium"), "{m}");
}

/// Finding 6 (thread 4719601034), A.5 "`height` … unset on a `post`, `wall` or `lane_guide`
/// means the playfield's `wall_height`": applied to every part `all_parts` returns.
#[test]
fn r1_f6_height_fallback_for_table_parts() {
    let l = layout();
    assert_eq!(l.playfield.wall_height, 28.0);
    let t = ok_table(&format!(
        "{}{}{}{}{}",
        part("sp", "post", ""),
        part("sw", "wall", ""),
        part("sl", "lane_guide", ""),
        part("hp", "post", "overrides = { height = 40.0 }"),
        part("dec", "decoration", ""),
    ));
    let parts = t.all_parts(&l).unwrap();
    for id in ["sp", "sw", "sl"] {
        assert_eq!(get(&parts, id).fields.height, Some(28.0), "{id}");
    }
    assert_eq!(get(&parts, "hp").fields.height, Some(40.0));
    assert_eq!(get(&parts, "dec").fields.height, None);
    // Without the layout the height stays unset.
    let alone = t.resolve_parts(&t.part_library().unwrap()).unwrap();
    for id in ["sp", "sw", "sl", "dec"] {
        assert_eq!(get(&alone, id).fields.height, None, "{id}");
    }
    assert_eq!(get(&alone, "hp").fields.height, Some(40.0));
}

/// Finding 7 (thread 4719601042), A.5 `bind` "one part per element" and allowed kinds: the
/// direct layout entrypoints reject what `all_parts` rejects.
#[test]
fn r1_f7_direct_layout_entrypoints_check_binds() {
    let l = layout();
    let lib = PartLibrary::builtin().unwrap();
    let mut t = ok_table("");
    t.parts.push(bound("a", PartKind::Post, "posts[0]"));
    t.parts.push(bound("b", PartKind::Post, "posts[00]"));
    for m in [
        err_text(l.resolve_parts(&t, &lib)),
        err_text(l.check_parts(&t)),
        err_text(t.all_parts(&l)),
    ] {
        assert!(m.contains("already bound") && m.contains("parts[0]"), "{m}");
    }
    let mut t = ok_table("");
    t.parts.push(bound("w", PartKind::Wall, "posts[0]"));
    for m in [
        err_text(l.resolve_parts(&t, &lib)),
        err_text(l.check_parts(&t)),
        err_text(t.all_parts(&l)),
    ] {
        assert!(
            m.contains("`w`") && m.contains("kind `wall` cannot bind to `posts`"),
            "{m}"
        );
    }
    let mut t = ok_table("");
    t.parts.push(bound("a", PartKind::Post, "posts[0]"));
    let parts = l.resolve_parts(&t, &lib).unwrap();
    assert_eq!(get(&parts, "posts[0]").physical(), Some(Material::Rubber));
    l.check_parts(&t).unwrap();
    t.all_parts(&l).unwrap();
}

/// Finding 8 (thread 4719601051), A.3 lane `width`, A.5 "the element's own keys
/// … win": the lane's width is carried into its `lane_guide` part and wins over styles and
/// overrides; `lane_guide` accepts `width`.
#[test]
fn r1_f8_lane_width_carried_and_wins() {
    let l = layout();
    assert_eq!(l.lanes[0].width, 32.0);
    let parts = ok_table("").all_parts(&l).unwrap();
    let lane = get(&parts, "lanes[0]");
    assert_eq!(lane.kind, PartKind::LaneGuide);
    assert_eq!(lane.fields.width, Some(32.0));
    // A style or override width on a bound part does not change it.
    let t = ok_table(&format!(
        "[[part_styles]]\nid = \"wide_guide\"\nkind = \"lane_guide\"\nwidth = 5.0\n\n{}",
        bind(
            "g",
            "lane_guide",
            "lanes[0]",
            "style = \"wide_guide\"\noverrides = { width = 10.0 }"
        )
    ));
    let parts = t.all_parts(&l).unwrap();
    assert_eq!(get(&parts, "lanes[0]").fields.width, Some(32.0));
    let lib = t.part_library().unwrap();
    let direct = l.resolve_parts(&t, &lib).unwrap();
    assert_eq!(get(&direct, "lanes[0]").fields.width, Some(32.0));
    // `lane_guide` accepts `width`; `wall` is unchanged.
    let w = StyleFields {
        width: Some(10.0),
        ..StyleFields::default()
    };
    w.validate(PartKind::LaneGuide, "p").unwrap();
    assert!(w.validate(PartKind::Wall, "p").is_err());
    use StyleField as F;
    assert_eq!(
        PartKind::LaneGuide.fields(),
        &[F::Width, F::Height, F::Thickness, F::Material, F::Color]
    );
    assert_eq!(
        PartKind::Wall.fields(),
        &[F::Height, F::Thickness, F::Material, F::Color]
    );
    // A standalone lane guide may state a width.
    let t = ok_table(&part("sg", "lane_guide", "overrides = { width = 12.0 }"));
    let parts = t.resolve_parts(&t.part_library().unwrap()).unwrap();
    assert_eq!(get(&parts, "sg").fields.width, Some(12.0));
}

/// Tables without the new sections resolve exactly as before (REQ-BUILD-005): the A.1 table
/// with the A.3 layout gives the generator's values.
#[test]
fn r1_req_build_005_plain_table_unchanged() {
    let t = TableManifest::from_toml_str(include_str!("../fixtures/table.toml")).unwrap();
    let parts = t.all_parts(&layout()).unwrap();
    assert_eq!(parts.len(), 6);
    assert_eq!(get(&parts, "posts[0]").physical(), Some(Material::Rubber));
    assert_eq!(get(&parts, "walls[0]").physical(), Some(Material::Wood));
    assert_eq!(get(&parts, "lanes[0]").physical(), Some(Material::Steel));
}
