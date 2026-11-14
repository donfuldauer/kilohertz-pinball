//! Independent spec tests for Subphase 5.1: the fixed `MaterialKind` shader-name table and
//! `MAX_LIGHTS`. GPU-free.

use pin_render::pbr::{MAX_LIGHTS, MaterialKind};
use proptest::prelude::*;

const TABLE: [(&str, MaterialKind); 6] = [
    ("pbr_chrome_steel", MaterialKind::ChromeSteel),
    ("pbr_ceramic_pearl", MaterialKind::CeramicPearl),
    ("pbr_uv_emissive", MaterialKind::UvEmissive),
    ("pbr_playfield_wood", MaterialKind::PlayfieldWood),
    ("pbr_light_insert", MaterialKind::LightInsert),
    ("pbr_clear_acrylic", MaterialKind::ClearAcrylic),
];

#[test]
fn max_lights_is_128() {
    assert_eq!(MAX_LIGHTS, 128usize);
}

#[test]
fn every_fixed_shader_name_maps_to_its_kind() {
    for (name, kind) in TABLE {
        let got = MaterialKind::from_shader_name(name);
        assert!(
            matches!((got, kind), (Some(a), b) if std::mem::discriminant(&a) == std::mem::discriminant(&b)),
            "from_shader_name({name:?}) did not return the expected kind"
        );
    }
}

#[test]
fn the_six_kinds_are_distinct() {
    for (i, (_, a)) in TABLE.iter().enumerate() {
        for (j, (_, b)) in TABLE.iter().enumerate() {
            assert_eq!(
                i == j,
                std::mem::discriminant(a) == std::mem::discriminant(b),
                "kinds {i} and {j}"
            );
        }
    }
}

#[test]
fn unknown_names_map_to_none() {
    for name in [
        "",
        " ",
        "pbr",
        "pbr_",
        "pbr_chrome",
        "chrome_steel",
        "pbr_chrome_steel ",
        " pbr_chrome_steel",
        "PBR_CHROME_STEEL",
        "Pbr_Chrome_Steel",
        "pbr_chrome_steel2",
        "pbr_chrome-steel",
        "pbr_clear_acrylic\0",
        "pbr_wood",
        "pbr_insert",
        "ChromeSteel",
        "debug",
    ] {
        assert!(
            MaterialKind::from_shader_name(name).is_none(),
            "from_shader_name({name:?}) should be None"
        );
    }
}

/// Every shader name used by a built-in ball profile in `pin_core` resolves, and to the
/// kind the name says.
#[test]
fn every_builtin_ball_profile_shader_resolves() {
    let mut seen = 0;
    for p in pin_core::profiles::builtin_profiles() {
        seen += 1;
        let kind = MaterialKind::from_shader_name(&p.shader)
            .unwrap_or_else(|| panic!("profile {:?}: shader {:?} is unknown", p.name, p.shader));
        let want = TABLE
            .iter()
            .find(|(n, _)| *n == p.shader)
            .unwrap_or_else(|| panic!("profile shader {:?} is outside the brief's table", p.shader))
            .1;
        assert_eq!(std::mem::discriminant(&kind), std::mem::discriminant(&want));
    }
    assert!(seen > 0);
}

proptest! {
    /// Arbitrary strings never panic, and only the six exact names resolve.
    #[test]
    fn arbitrary_names_never_panic_and_only_exact_names_resolve(s in ".{0,40}") {
        let got = MaterialKind::from_shader_name(&s);
        let known = TABLE.iter().any(|(n, _)| *n == s);
        prop_assert_eq!(got.is_some(), known);
    }

    /// Any single-character edit of a known name is unknown.
    #[test]
    fn one_character_edits_are_unknown(which in 0usize..6, pos in 0usize..32, c in proptest::char::range('a', 'z')) {
        let name = TABLE[which].0;
        let pos = pos % name.len();
        let mut bytes = name.as_bytes().to_vec();
        prop_assume!(bytes[pos] != c as u8);
        bytes[pos] = c as u8;
        let edited = String::from_utf8(bytes).unwrap();
        prop_assume!(!TABLE.iter().any(|(n, _)| *n == edited));
        prop_assert!(MaterialKind::from_shader_name(&edited).is_none());
    }
}
