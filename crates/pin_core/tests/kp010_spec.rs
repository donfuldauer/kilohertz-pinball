//! KP-010 (#90) acceptance I: the SPEC Appendix A.5 example is a committed fixture that
//! appears verbatim and parses, and A.5 names every new key and every built-in
//! (REQ-BUILD-005).

mod kp010_common;

use kp010_common::{PARTS_TABLE, layout};
use pin_core::schema::parts::{
    MaterialDef, PartDef, PartKind, PartLibrary, PartStyleDef, Shape, StyleField, StyleFields,
    ThemeDef,
};
use pin_core::{PhysicsDefaults, TableManifest};
use std::collections::BTreeSet;

const SPEC: &str = include_str!("../../../docs/SPEC.md");

fn a5() -> &'static str {
    let start = SPEC
        .find("### A.5 Parts, part styles, materials and theme")
        .expect("A.5 heading");
    let rest = &SPEC[start..];
    let end = rest
        .find("\n---\n")
        .expect("A.5 ends before the Appendix A rule");
    &rest[..end]
}

/// REQ-BUILD-005: A.5 sits inside Appendix A, after A.4 and before Appendix B.
#[test]
fn req_build_005_a5_is_in_appendix_a() {
    let a4 = SPEC.find("### A.4 `cabinet.toml`").unwrap();
    let a5 = SPEC.find("### A.5 Parts").unwrap();
    let b = SPEC.find("## Appendix B").unwrap();
    assert!(a4 < a5 && a5 < b);
}

/// REQ-BUILD-005: the example is the fixture verbatim, parses, and resolves with the A.3
/// layout and the physics defaults.
#[test]
fn req_build_005_a5_example_is_the_fixture_and_parses() {
    assert!(
        a5().contains(&format!("```toml\n{PARTS_TABLE}```")),
        "fixture differs from SPEC A.5"
    );
    let t = TableManifest::from_toml_str(PARTS_TABLE).unwrap();
    let l = layout();
    let parts = t.all_parts(&l).unwrap();
    assert_eq!(
        parts.len(),
        t.parts.len() - 1 + 6,
        "one layout-bound part, six layout elements"
    );
    t.check_physical_pairs(&l, &PhysicsDefaults::builtin())
        .unwrap();
    let logo = parts.iter().find(|p| p.name == "apron_logo").unwrap();
    assert_eq!(logo.fields.shape, Some(Shape::Polygon));
    assert_eq!(logo.physical(), None);
}

fn keys_of<T: serde::Serialize>(v: &T, out: &mut BTreeSet<String>) {
    fn walk(v: &toml::Value, out: &mut BTreeSet<String>) {
        match v {
            toml::Value::Table(t) => {
                for (k, c) in t {
                    out.insert(k.clone());
                    walk(c, out);
                }
            }
            toml::Value::Array(a) => a.iter().for_each(|c| walk(c, out)),
            _ => {}
        }
    }
    walk(&toml::Value::try_from(v).unwrap(), out);
}

/// REQ-BUILD-005: every serde key of the new structs is documented in A.5 (as `` `key` ``),
/// as are the kinds, shapes, built-in style and material ids and ring sizes.
#[test]
fn req_build_005_a5_documents_every_key_and_builtin() {
    let full = StyleFields {
        shape: Some(Shape::Round),
        size: Some(1.0),
        diameter: Some(1.0),
        width: Some(1.0),
        length: Some(1.0),
        height: Some(1.0),
        thickness: Some(1.0),
        star_points: Some(5),
        outline: Some(vec![[0.0, 0.0]]),
        ring_size: Some("r".into()),
        inner_diameter: Some(1.0),
        section: Some(1.0),
        material: Some("m".into()),
        trim_material: Some("m".into()),
        color: Some("#000000".into()),
        tube_radius: Some(1.0),
        tube_sides: Some(8),
        lift: Some(1.0),
    };
    for &f in StyleField::ALL {
        assert!(full.is_set(f), "{f:?} not covered by this test");
    }
    let mut keys = BTreeSet::new();
    keys_of(&full, &mut keys);
    let mut style = PartStyleDef {
        id: "s".into(),
        kind: PartKind::Post,
        base: Some("b".into()),
        shape: None,
        size: None,
        diameter: None,
        width: None,
        length: None,
        height: None,
        thickness: None,
        star_points: None,
        outline: None,
        ring_size: None,
        inner_diameter: None,
        section: None,
        material: None,
        trim_material: None,
        color: None,
        tube_radius: None,
        tube_sides: None,
        lift: None,
    };
    style.material = Some("m".into());
    keys_of(&style, &mut keys);
    keys_of(
        &PartDef {
            id: "p".into(),
            kind: PartKind::Post,
            style: Some("s".into()),
            bind: Some("b".into()),
            position: Some([0.0; 3]),
            rotation_deg: [1.0; 3],
            material: Some("m".into()),
            overrides: Some(full.clone()),
        },
        &mut keys,
    );
    keys_of(
        &MaterialDef {
            id: "m".into(),
            base_color: "#000000".into(),
            metallic: 0.0,
            roughness: 0.5,
            clearcoat: Some(1.0),
            emissive: Some("#000000".into()),
            emissive_strength: Some(1.0),
            alpha: 1.0,
            translucent: true,
            uv_reactive: true,
            physical: Some("steel".into()),
        },
        &mut keys,
    );
    let mut theme = ThemeDef {
        title: Some("t".into()),
        palette: Some(vec!["#000000".into()]),
        motif: Some("m".into()),
        ..ThemeDef::default()
    };
    theme.fonts.insert("title".into(), "Sans".into());
    keys_of(&theme, &mut keys);
    keys.extend(["materials", "part_styles", "parts", "theme"].map(String::from));
    let section = a5();
    for k in &keys {
        assert!(
            section.contains(&format!("`{k}`"))
                || section.contains(&format!("[[{k}]]"))
                || section.contains(&format!("[{k}]")),
            "A.5 does not document `{k}`"
        );
    }
    for &k in PartKind::ALL {
        assert!(
            section.contains(&format!("`{}`", k.name())),
            "kind {}",
            k.name()
        );
    }
    for s in [
        "round",
        "rectangle",
        "polygon",
        "capsule",
        "star",
        "dome",
        "arrow",
        "triangle",
    ] {
        assert!(section.contains(&format!("`{s}`")), "shape {s}");
    }
    let lib = PartLibrary::builtin().unwrap();
    for id in lib.style_ids() {
        assert!(section.contains(&format!("`{id}`")), "style {id}");
    }
    for id in lib.material_ids() {
        assert!(section.contains(&format!("`{id}`")), "material {id}");
    }
    for id in lib.ring_size_ids() {
        let r = lib.ring_size(id).unwrap();
        let text = format!("`{id}` {:.1} / {:.1}", r.inner_diameter, r.section);
        assert!(section.contains(&text), "ring size {text}");
    }
}
