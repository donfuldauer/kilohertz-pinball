//! Render-kind selection for generated (pin_tablegen) tables, GPU-free: the
//! generated glTF names its materials `wood`, `acrylic`, `steel` and
//! `insert #rrggbb`, never `pbr_*`, so the visuals must come from the
//! physical material and the lamp tag (fix-15). Inserts must be able to light
//! (`lamp_emissive > 0`) and acrylic decks must be drawn blended.

mod support;

use pin_app::orchestrator::{TableVisuals, validate_table};
use pin_render::pbr::MaterialKind;
use support::{greybox_bundle, greybox_inserts_bundle, on_big_stack};

fn visuals(dir: &'static std::path::Path) -> TableVisuals {
    on_big_stack(move || TableVisuals::of(&validate_table(dir).unwrap()))
}

#[test]
fn acrylic_decks_are_transparent_with_transmission() {
    let v = visuals(greybox_bundle());
    let acrylic = MaterialKind::ClearAcrylic.params();
    let decks: Vec<_> = v.meshes.iter().filter(|m| m.transparent).collect();
    assert!(
        !decks.is_empty(),
        "no transparent mesh: acrylic decks opaque"
    );
    for d in &decks {
        assert!(d.material.transmission > 0.0, "{}", d.name);
        assert_eq!(d.material.transmission, acrylic.transmission, "{}", d.name);
        assert_eq!(d.lamp, None, "{}", d.name);
    }
    // The grey box has acrylic decks (3 of them) and no other transparent mesh.
    assert_eq!(
        decks.len(),
        3,
        "{:?}",
        decks.iter().map(|m| &m.name).collect::<Vec<_>>()
    );
}

#[test]
fn inserts_emit_with_their_lamp() {
    let v = visuals(greybox_inserts_bundle());
    let inserts: Vec<_> = v.meshes.iter().filter(|m| m.lamp.is_some()).collect();
    assert_eq!(
        inserts.len(),
        2,
        "{:?}",
        inserts.iter().map(|m| &m.name).collect::<Vec<_>>()
    );
    for i in inserts {
        assert!(i.material.lamp_emissive > 0.0, "{} does not light", i.name);
        assert!(!i.transparent, "{}", i.name);
    }
}

#[test]
fn a_greybox_without_inserts_has_no_lamp_meshes() {
    let v = visuals(greybox_bundle());
    assert!(v.meshes.iter().all(|m| m.lamp.is_none()));
}
