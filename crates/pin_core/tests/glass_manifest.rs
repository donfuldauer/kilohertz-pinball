//! `glass.toml` (`GlassManifest`, CR-007).

use pin_core::schema::{GLASS_FILE, GlassManifest, SchemaError};
use proptest::prelude::*;

#[test]
fn parses_and_round_trips() {
    assert_eq!(GLASS_FILE, "glass.toml");
    let g = GlassManifest::from_toml_str("schema_version = 1\nheight = 112.0\n").unwrap();
    assert_eq!(g.height, 112.0);
    let out = g.to_toml_string().unwrap();
    assert_eq!(GlassManifest::from_toml_str(&out).unwrap(), g);
}

#[test]
fn unknown_key_rejected() {
    let e = GlassManifest::from_toml_str("schema_version = 1\nheight = 1.0\nx = 2\n");
    assert!(matches!(e, Err(SchemaError::Parse(_))));
}

#[test]
fn missing_height_rejected() {
    assert!(GlassManifest::from_toml_str("schema_version = 1\n").is_err());
}

#[test]
fn wrong_version_rejected() {
    let e = GlassManifest::from_toml_str("schema_version = 2\nheight = 1.0\n");
    assert_eq!(e, Err(SchemaError::UnsupportedVersion(2)));
    let g = GlassManifest {
        schema_version: 2,
        height: 1.0,
    };
    assert_eq!(g.to_toml_string(), Err(SchemaError::UnsupportedVersion(2)));
}

#[test]
fn bad_heights_rejected_on_parse() {
    for v in ["0.0", "-5.0", "nan", "inf", "-inf"] {
        let t = format!("schema_version = 1\nheight = {v}\n");
        assert!(GlassManifest::from_toml_str(&t).is_err(), "{v}");
    }
}

#[test]
fn bad_heights_rejected_on_serialise() {
    for v in [0.0f32, -5.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let g = GlassManifest {
            schema_version: 1,
            height: v,
        };
        assert!(g.to_toml_string().is_err(), "{v}");
    }
}

proptest! {
    #[test]
    fn positive_finite_round_trips(h in 1e-3f32..1e6f32) {
        let g = GlassManifest { schema_version: 1, height: h };
        let out = g.to_toml_string().unwrap();
        prop_assert_eq!(GlassManifest::from_toml_str(&out).unwrap(), g);
    }
}
