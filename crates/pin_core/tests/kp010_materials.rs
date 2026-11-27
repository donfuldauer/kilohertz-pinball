//! KP-010 (#90) acceptance C and F: the material library and physical-pair validation
//! (REQ-BUILD-004).

mod kp010_common;

use kp010_common::{PARTS_TABLE, invalid_table, layout, minimal_table, ok_table, table};
use pin_core::schema::SchemaError;
use pin_core::schema::parts::{
    MaterialDef, PartKind, PartLibrary, check_physical_pairs, parse_color,
};
use pin_core::schema::physics::DEFAULT_PHYSICS_TOML;
use pin_core::{Material, PhysicsDefaults, TableManifest};

fn physics() -> PhysicsDefaults {
    PhysicsDefaults::from_toml_str(DEFAULT_PHYSICS_TOML).unwrap()
}

fn part(id: &str, kind: &str, rest: &str) -> String {
    format!("[[parts]]\nid = \"{id}\"\nkind = \"{kind}\"\nposition = [10.0, 20.0, 0.0]\n{rest}\n")
}

fn mat(id: &str, physical: Option<&str>) -> String {
    let p = physical.map_or(String::new(), |p| format!("physical = \"{p}\"\n"));
    format!("[[materials]]\nid = \"{id}\"\nbase_color = \"#808080\"\n{p}\n")
}

// ---------------------------------------------------------------- F. built-in library

/// REQ-BUILD-004: every built-in material is valid; the seven physical names play as
/// themselves; the visual-only lenses have no physical material.
#[test]
fn req_build_004_builtin_library_is_valid() {
    let lib = PartLibrary::builtin().unwrap();
    let ids: Vec<&str> = lib.material_ids().collect();
    assert!(ids.windows(2).all(|w| w[0] < w[1]), "sorted, unique");
    for id in &ids {
        let m = lib.material(id).unwrap();
        m.validate(id).unwrap();
        assert!(m.base_rgb().is_some());
        assert!((0.0..=1.0).contains(&m.metallic) && (0.0..=1.0).contains(&m.roughness));
        assert!((0.0..=1.0).contains(&m.alpha));
        if let Some(e) = &m.emissive {
            assert!(parse_color(e).is_some());
        }
        if let Some(p) = &m.physical {
            assert!(Material::from_name(p).is_some(), "{id}: {p}");
        }
    }
    for m in Material::ALL {
        let r = lib.resolve_material(m.name(), "x").unwrap();
        assert_eq!(r.physical, Some(m));
        assert_eq!(r.def.physical.as_deref(), Some(m.name()));
    }
    for id in ["insert_lens", "lamp_lens"] {
        assert_eq!(lib.resolve_material(id, "x").unwrap().physical, None);
    }
    for id in [
        "chrome",
        "brushed_steel",
        "brass",
        "rubber_black",
        "rubber_white",
        "rubber_red",
        "acrylic_clear",
        "plastic_translucent",
        "wood_painted",
    ] {
        assert!(
            lib.resolve_material(id, "x").unwrap().physical.is_some(),
            "{id}"
        );
    }
}

/// REQ-BUILD-004: every ball-touchable built-in style's material has a pair with a ball
/// material in the shipped physics defaults.
#[test]
fn req_build_004_touchable_builtin_styles_have_material_pairs() {
    let lib = PartLibrary::builtin().unwrap();
    let phys = physics();
    let mut checked = 0;
    for id in lib.style_ids() {
        let s = lib.resolve_style(id).unwrap();
        if !s.kind.is_touchable() {
            continue;
        }
        let m = lib
            .resolve_material(s.fields.material.as_deref().unwrap(), id)
            .unwrap();
        let p = m.physical.unwrap_or_else(|| panic!("{id}: no physical"));
        assert!(
            Material::BALL
                .iter()
                .any(|&b| phys.material_pair(b, p).is_some()),
            "{id}: {p:?}"
        );
        checked += 1;
    }
    assert!(checked >= PartKind::ALL.iter().filter(|k| k.is_touchable()).count());
}

/// REQ-BUILD-004: a table entry extends the library; an entry with a built-in id replaces it
/// whole; the order is fixed (sorted) whatever the file order.
#[test]
fn req_build_004_table_materials_extend_and_replace() {
    let t = ok_table(&format!(
        "{}[[materials]]\nid = \"chrome\"\nbase_color = \"#010101\"\nphysical = \"steel\"\n\n",
        mat("zz_new", Some("rubber"))
    ));
    let lib = t.part_library().unwrap();
    assert_eq!(
        lib.material("zz_new").unwrap().physical.as_deref(),
        Some("rubber")
    );
    let chrome = lib.material("chrome").unwrap();
    assert_eq!(chrome.base_color, "#010101");
    assert_eq!(chrome.metallic, 0.0, "replaced whole, not merged");
    assert_eq!(chrome.roughness, 0.5);
    let builtin = PartLibrary::builtin().unwrap();
    assert_eq!(
        lib.material_ids().count(),
        builtin.material_ids().count() + 1
    );
    let t2 = ok_table(&format!(
        "[[materials]]\nid = \"chrome\"\nbase_color = \"#010101\"\nphysical = \"steel\"\n\n{}",
        mat("zz_new", Some("rubber"))
    ));
    assert_eq!(t2.part_library().unwrap(), lib);
    let a: Vec<&str> = lib.material_ids().collect();
    assert!(a.windows(2).all(|w| w[0] < w[1]));
}

/// REQ-BUILD-004: duplicate ids in one table, bad values and too many entries are errors
/// naming the entry.
#[test]
fn req_build_004_material_errors_name_the_entry() {
    let m = invalid_table(&format!("{}{}", mat("twin", None), mat("twin", None)));
    assert!(m.contains("duplicate") && m.contains("twin"), "{m}");
    for (field, value, want) in [
        ("metallic", "1.5", "metallic"),
        ("roughness", "-0.1", "roughness"),
        ("clearcoat", "2.0", "clearcoat"),
        ("alpha", "1.01", "alpha"),
        ("emissive", "\"#12345\"", "emissive"),
        ("emissive_strength", "1001.0", "emissive_strength"),
        ("base_color", "\"blue\"", "base_color"),
        ("id", "\"Bad\"", "Bad"),
        ("physical", &format!("\"{}\"", "x".repeat(65)), "physical"),
    ] {
        let mut lines = vec![
            ("id", "\"m1\"".to_string()),
            ("base_color", "\"#808080\"".to_string()),
        ];
        match lines.iter_mut().find(|(k, _)| *k == field) {
            Some(l) => l.1 = value.to_string(),
            None => lines.push((field, value.to_string())),
        }
        let body: String = lines.iter().map(|(k, v)| format!("{k} = {v}\n")).collect();
        let src = format!("[[materials]]\n{body}");
        let m = invalid_table(&src);
        assert!(m.contains(want), "{field}: {m}");
    }
    let many: String = (0..257).map(|i| mat(&format!("m{i}"), None)).collect();
    let m = invalid_table(&many);
    assert!(m.contains("257"), "{m}");
}

// ---------------------------------------------------------------- C. physical-pair validation

/// REQ-BUILD-004 (a): a material whose `physical` is not a physical material, used by a part,
/// is an error naming the part, the material and the value; the positive twin passes.
#[test]
fn req_build_004_unknown_physical_names_the_part() {
    let m = invalid_table(&format!(
        "{}{}",
        mat("gummy", Some("gumdrop")),
        part("left_post", "post", "material = \"gummy\"")
    ));
    assert!(
        m.contains("left_post") && m.contains("gummy") && m.contains("gumdrop"),
        "{m}"
    );
    ok_table(&format!(
        "{}{}",
        mat("gummy", Some("rubber")),
        part("left_post", "post", "material = \"gummy\"")
    ));
    // Unused, it is not an error (checked when used).
    ok_table(&mat("gummy", Some("gumdrop")));
}

/// REQ-BUILD-004 (a): an id-less layout element is named by its array path.
#[test]
fn req_build_004_unknown_physical_names_layout_path() {
    // Replace the built-in `rubber` entry with a bad physical: layout rubber posts use it.
    let t = ok_table(&mat("rubber", Some("rubbery")));
    let l = layout();
    match t.all_parts(&l) {
        Err(SchemaError::Invalid(m)) => {
            assert!(
                m.contains("posts[0]") && m.contains("rubber") && m.contains("rubbery"),
                "{m}"
            )
        }
        other => panic!("{other:?}"),
    }
    assert!(l.check_parts(&t).is_err());
}

/// REQ-BUILD-004 (b): a ball-touchable part whose material has no `physical` is an error
/// naming the part; visual-only kinds accept it.
#[test]
fn req_build_004_touchable_without_physical_names_the_part() {
    for &k in PartKind::ALL {
        let src = format!(
            "{}{}",
            mat("paint", None),
            part("pp", k.name(), "material = \"paint\"")
        );
        if k.is_touchable() {
            let m = invalid_table(&src);
            assert!(m.contains("pp") && m.contains("paint"), "{k:?}: {m}");
        } else {
            ok_table(&src);
        }
    }
}

/// REQ-BUILD-004 (c): every ball-touchable part's physical material needs a pair with a ball
/// material in the supplied physics defaults.
#[test]
fn req_build_004_missing_material_pair_names_the_part() {
    let t = ok_table(&format!(
        "{}{}",
        mat("glassy", Some("acrylic")),
        part("glass_post", "post", "material = \"glassy\"")
    ));
    let lib = t.part_library().unwrap();
    let parts = t.resolve_parts(&lib).unwrap();
    let mut phys = physics();
    check_physical_pairs(&parts, &phys).unwrap();
    phys.material_pairs
        .retain(|p| !p.materials.contains(&Material::Acrylic));
    match check_physical_pairs(&parts, &phys) {
        Err(SchemaError::Invalid(m)) => assert!(
            m.contains("glass_post") && m.contains("glassy") && m.contains("acrylic"),
            "{m}"
        ),
        other => panic!("{other:?}"),
    }
    // Through the table API, with the layout.
    let l = layout();
    t.check_physical_pairs(&l, &physics()).unwrap();
    match t.check_physical_pairs(&l, &phys) {
        Err(SchemaError::Invalid(m)) => {
            assert!(m.contains("decks[0]") || m.contains("glass_post"), "{m}")
        }
        other => panic!("{other:?}"),
    }
    // A resolved part edited to have no material or no physical is caught as well.
    let mut bare = parts.clone();
    bare[0].material.as_mut().unwrap().physical = None;
    assert!(
        check_physical_pairs(&bare, &physics())
            .unwrap_err()
            .to_string()
            .contains("glass_post")
    );
    bare[0].material = None;
    assert!(
        check_physical_pairs(&bare, &physics())
            .unwrap_err()
            .to_string()
            .contains("glass_post")
    );
}

/// REQ-BUILD-004 (d): an unknown material name is an error naming the part.
#[test]
fn req_build_004_unknown_material_names_the_part() {
    let m = invalid_table(&part("mystery", "decoration", "material = \"unobtainium\""));
    assert!(m.contains("mystery") && m.contains("unobtainium"), "{m}");
    let m = invalid_table(&part(
        "mystery2",
        "pop_bumper",
        "overrides = { trim_material = \"vapour\" }",
    ));
    assert!(m.contains("mystery2") && m.contains("vapour"), "{m}");
}

/// REQ-BUILD-004: a layout element's material decides how it plays; a look that plays
/// differently is an error naming the part, one that plays the same is accepted.
#[test]
fn req_build_004_layout_looks_and_plays_agree() {
    let l = layout();
    for (bind, kind, material, ok) in [
        ("walls[0]", "wall", "wood_painted", true),
        ("walls[0]", "wall", "rubber_black", false),
        ("decks[0]", "deck", "acrylic_clear", true),
        ("decks[0]", "deck", "plastic_white", false),
        ("posts[0]", "post", "rubber_red", true),
        ("posts[0]", "post", "chrome", false),
    ] {
        let src = minimal_table(&format!(
            "[[parts]]\nid = \"look\"\nkind = \"{kind}\"\nbind = \"{bind}\"\nmaterial = \"{material}\"\n"
        ));
        let t = TableManifest::from_toml_str(&src).unwrap();
        let r = t.all_parts(&l);
        assert_eq!(r.is_ok(), ok, "{bind} {material}: {r:?}");
        if let Err(SchemaError::Invalid(m)) = r {
            assert!(
                m.contains("look") && m.contains(bind) && m.contains(material),
                "{m}"
            );
        }
    }
    // A named style that plays differently is an error too; the kind default gives way.
    let t = ok_table(
        "[[parts]]\nid = \"s\"\nkind = \"post\"\nbind = \"posts[0]\"\nstyle = \"post_chrome\"\n",
    );
    assert!(t.all_parts(&l).is_err());
    let t = ok_table("[[parts]]\nid = \"s\"\nkind = \"post\"\nbind = \"posts[0]\"\n");
    let parts = t.all_parts(&l).unwrap();
    let p = parts.iter().find(|p| p.name == "posts[0]").unwrap();
    assert_eq!(p.material.as_ref().unwrap().def.id, "rubber");
}

/// REQ-BUILD-004: `MaterialDef` round-trips through TOML with defaults filled.
#[test]
fn req_build_004_material_def_defaults() {
    let t = ok_table(&mat("plain", None));
    let m: &MaterialDef = &t.materials[0];
    assert_eq!((m.metallic, m.roughness, m.alpha), (0.0, 0.5, 1.0));
    assert!(!m.translucent && !m.uv_reactive);
    assert_eq!(m.clearcoat, None);
    let text = t.to_toml_string().unwrap();
    assert_eq!(table(&text).unwrap(), t);
    let full = table(PARTS_TABLE).unwrap();
    assert!(
        full.materials
            .iter()
            .any(|m| m.uv_reactive && m.emissive.is_some())
    );
}
