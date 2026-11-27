//! Helpers shared by the KP-010 (#90) test files.
#![allow(dead_code)]

use pin_core::schema::SchemaError;
use pin_core::{LayoutManifest, TableManifest};

/// The SPEC Appendix A.5 example (committed verbatim).
pub const PARTS_TABLE: &str = include_str!("../fixtures/parts_table.toml");
/// The SPEC Appendix A.3 example layout.
pub const LAYOUT: &str = include_str!("../../fixtures/layout.toml");

/// The four shipped tables: (name, table.toml, layout.toml).
pub const TABLES: [(&str, &str, &str); 4] = [
    (
        "greybox",
        include_str!("../../../../tables/greybox/table.toml"),
        include_str!("../../../../tables/greybox/layout.toml"),
    ),
    (
        "apex_circuit",
        include_str!("../../../../tables/apex_circuit/table.toml"),
        include_str!("../../../../tables/apex_circuit/layout.toml"),
    ),
    (
        "sky_deck",
        include_str!("../../../../tables/sky_deck/table.toml"),
        include_str!("../../../../tables/sky_deck/layout.toml"),
    ),
    (
        "feature_lab",
        include_str!("../../../../tables/feature_lab/table.toml"),
        include_str!("../../../../tables/feature_lab/layout.toml"),
    ),
];

/// A table.toml with only the required sections, then `extra`.
pub fn minimal_table(extra: &str) -> String {
    format!(
        r##"schema_version = 1

[meta]
id = "t"
name = "T"
author = "a"
version = "1"
script_entry = "rules.lua"
playfield_mesh = "playfield.glb"
players_max = 1
palette = ["#010203", "nonsense"]

[balls]
profiles = ["standard_steel"]

[cabinet_switches]
left_flipper = 1
right_flipper = 2
left_upper = 3
right_upper = 4
action = 5
start = 6
launch = 7
tilt = 8

{extra}"##
    )
}

/// Parses a table.toml text.
pub fn table(src: &str) -> Result<TableManifest, SchemaError> {
    TableManifest::from_toml_str(src)
}

/// Parses `minimal_table(extra)` and panics with the error if it fails.
pub fn ok_table(extra: &str) -> TableManifest {
    match table(&minimal_table(extra)) {
        Ok(t) => t,
        Err(e) => panic!("expected the table to parse, got {e}\n{extra}"),
    }
}

/// The `Invalid` message of `minimal_table(extra)`, panicking on success or another error.
pub fn invalid_table(extra: &str) -> String {
    match table(&minimal_table(extra)) {
        Err(SchemaError::Invalid(m)) => m,
        other => panic!("expected Invalid, got {other:?}\n{extra}"),
    }
}

/// The A.3 example layout.
pub fn layout() -> LayoutManifest {
    LayoutManifest::from_toml_str(LAYOUT).expect("A.3 layout parses")
}
