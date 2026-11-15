//! `[playfield] glass_height` (CR-007).

use pin_core::schema::LayoutManifest;
use proptest::prelude::*;

const BASE: &str =
    "schema_version = 1\n[playfield]\nwidth = 514.0\nlength = 1070.0\nwall_height = 28.0\n";

#[test]
fn fixture_parses_to_none_and_round_trips() {
    let text = include_str!("../fixtures/layout.toml");
    let l = LayoutManifest::from_toml_str(text).unwrap();
    assert_eq!(l.playfield.glass_height, None);
    let out = l.to_toml_string().unwrap();
    assert!(!out.contains("glass_height"));
    assert_eq!(LayoutManifest::from_toml_str(&out).unwrap(), l);
}

#[test]
fn present_parses() {
    let l = LayoutManifest::from_toml_str(&format!("{BASE}glass_height = 90.0\n")).unwrap();
    assert_eq!(l.playfield.glass_height, Some(90.0));
    let out = l.to_toml_string().unwrap();
    assert!(out.contains("glass_height"));
    assert_eq!(LayoutManifest::from_toml_str(&out).unwrap(), l);
}

#[test]
fn unknown_playfield_key_still_rejected() {
    assert!(LayoutManifest::from_toml_str(&format!("{BASE}glass_hieght = 90.0\n")).is_err());
}

#[test]
fn zero_negative_nan_rejected() {
    for v in ["0.0", "-1.0", "nan", "inf"] {
        let t = format!("{BASE}glass_height = {v}\n");
        assert!(LayoutManifest::from_toml_str(&t).is_err(), "{v}");
    }
}

proptest! {
    #[test]
    fn positive_finite_round_trips(g in 0.001f32..10000.0f32) {
        let mut l = LayoutManifest::from_toml_str(BASE).unwrap();
        l.playfield.glass_height = Some(g);
        let back = LayoutManifest::from_toml_str(&l.to_toml_string().unwrap()).unwrap();
        prop_assert_eq!(back, l);
    }
}
