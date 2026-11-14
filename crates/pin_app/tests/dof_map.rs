//! Subphase 3.5: `[dof]` resolution and the bridge map file.

use pin_app::dof::{DofError, DofMapEntry, DofSourceKind, MAP_FORMAT, ResolvedDofMap};
use pin_core::schema::table::{DofElementType, DofMapping, TableManifest};
use proptest::prelude::*;

const BASE: &str = r#"
schema_version = 1

[meta]
id = "t"
name = "Test Table"
author = "a"
version = "1"
script_entry = "rules.lua"
playfield_mesh = "playfield.glb"
players_max = 1

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

[[switches]]
id = "sw"
number = 21
kind = "leaf"

[[switches]]
id = "sw_high"
number = 300
kind = "leaf"

[[coils]]
id = "flip"
number = 1
kind = "flipper"

[[lamps]]
id = "insert"
number = 40
kind = "insert"

[[lamps]]
id = "gi_a"
number = 3
kind = "gi"

[[lamps]]
id = "gi_high"
number = 40
kind = "gi"

[[lamps]]
id = "flash"
number = 255
kind = "flasher"
"#;

fn table(dof: &str) -> TableManifest {
    TableManifest::from_toml_str(&format!("{BASE}\n{dof}")).expect("test table parses")
}

#[test]
fn no_dof_section_resolves_to_none() {
    assert_eq!(ResolvedDofMap::resolve(&table("")).unwrap(), None);
}

#[test]
fn resolves_names_to_telemetry_ids() {
    let t = table(
        r#"[dof]
rom_name = "rom_1"
map = [
  { coil = "flip", type = "S", number = 1 },
  { switch = "sw", type = "W", number = 21 },
  { lamp = "insert", type = "L", number = 7 },
  { lamp = "gi_a", type = "G", number = 2 },
  { lamp = "flash", type = "E", number = 65535 },
  { coil = "flip", type = "E", number = 300 },
]"#,
    );
    let m = ResolvedDofMap::resolve(&t).unwrap().unwrap();
    assert_eq!(m.rom_name, "rom_1");
    assert_eq!(m.table_name, "Test Table");
    let e = |source, id, name: &str, element_type, number| DofMapEntry {
        source,
        id,
        name: name.into(),
        element_type,
        number,
    };
    assert_eq!(
        m.entries,
        vec![
            e(DofSourceKind::Coil, 1, "flip", DofElementType::S, 1),
            e(DofSourceKind::Switch, 21, "sw", DofElementType::W, 21),
            e(DofSourceKind::Lamp, 40, "insert", DofElementType::L, 7),
            e(DofSourceKind::Gi, 3, "gi_a", DofElementType::G, 2),
            e(DofSourceKind::Lamp, 255, "flash", DofElementType::E, 65535),
            e(DofSourceKind::Coil, 1, "flip", DofElementType::E, 300),
        ]
    );
}

#[test]
fn json_is_the_documented_format() {
    let t = table(
        r#"[dof]
rom_name = "r\"om"
map = [ { lamp = "gi_a", type = "G", number = 2 }, { switch = "sw", type = "W", number = 9 } ]"#,
    );
    let json = ResolvedDofMap::resolve(&t).unwrap().unwrap().to_json();
    let want = format!(
        "{{\n  \"format\": \"{MAP_FORMAT}\",\n  \"version\": 1,\n  \"rom_name\": \"r\\\"om\",\n  \"table_name\": \"Test Table\",\n  \"entries\": [\n    {{ \"source\": \"gi\", \"id\": 3, \"name\": \"gi_a\", \"type\": \"G\", \"number\": 2 }},\n    {{ \"source\": \"switch\", \"id\": 21, \"name\": \"sw\", \"type\": \"W\", \"number\": 9 }}\n  ]\n}}\n"
    );
    assert_eq!(json, want);
}

#[test]
fn unknown_names_are_errors() {
    for (src, field) in [
        ("coil = \"nope\"", "dof.map[0].coil"),
        ("switch = \"nope\"", "dof.map[0].switch"),
        ("lamp = \"nope\"", "dof.map[0].lamp"),
        ("lamp = \"flip\"", "dof.map[0].lamp"),
    ] {
        let t = table(&format!(
            "[dof]\nrom_name = \"r\"\nmap = [ {{ {src}, type = \"S\", number = 1 }} ]"
        ));
        match ResolvedDofMap::resolve(&t) {
            Err(DofError::UnknownName { field: f, name }) => {
                assert_eq!(f, field);
                assert!(name == "nope" || name == "flip");
            }
            other => panic!("{src}: {other:?}"),
        }
    }
}

#[test]
fn numbers_beyond_telemetry_ids_are_errors() {
    let t = table(
        "[dof]\nrom_name = \"r\"\nmap = [ { switch = \"sw_high\", type = \"W\", number = 1 } ]",
    );
    let err = ResolvedDofMap::resolve(&t).unwrap_err();
    assert!(matches!(
        err,
        DofError::IdOutOfRange {
            number: 300,
            max: 255,
            ..
        }
    ));
    assert!(err.to_string().contains("300"));
    let t = table(
        "[dof]\nrom_name = \"r\"\nmap = [ { lamp = \"gi_high\", type = \"G\", number = 1 } ]",
    );
    assert!(matches!(
        ResolvedDofMap::resolve(&t),
        Err(DofError::IdOutOfRange {
            number: 40,
            max: 31,
            ..
        })
    ));
}

#[test]
fn entry_with_zero_or_two_sources_is_an_error() {
    let mut t = table("[dof]\nrom_name = \"r\"\nmap = []");
    let mapping = |coil: Option<&str>, switch: Option<&str>| DofMapping {
        coil: coil.map(Into::into),
        switch: switch.map(Into::into),
        lamp: None,
        element_type: DofElementType::S,
        number: 1,
    };
    for (m, n) in [
        (mapping(None, None), 0),
        (mapping(Some("flip"), Some("sw")), 2),
    ] {
        t.dof.as_mut().unwrap().map = vec![m];
        match ResolvedDofMap::resolve(&t) {
            Err(e @ DofError::InvalidEntry { sources, .. }) => {
                assert_eq!(sources, n);
                assert!(e.to_string().contains("exactly one"));
            }
            other => panic!("{other:?}"),
        }
    }
}

#[test]
fn write_file_creates_distinct_files() {
    let t =
        table("[dof]\nrom_name = \"r\"\nmap = [ { coil = \"flip\", type = \"S\", number = 1 } ]");
    let m = ResolvedDofMap::resolve(&t).unwrap().unwrap();
    let dir = std::env::temp_dir();
    let a = m.write_file(&dir).unwrap();
    let b = m.write_file(&dir).unwrap();
    assert_ne!(a, b);
    assert_eq!(std::fs::read_to_string(&a).unwrap(), m.to_json());
    std::fs::remove_file(a).unwrap();
    std::fs::remove_file(b).unwrap();
    let missing = dir.join("khz-dof-no-such-dir").join("deeper");
    let err = m.write_file(&missing).unwrap_err();
    assert!(matches!(err, DofError::MapFile(_)));
    assert!(std::error::Error::source(&err).is_some());
    assert!(err.to_string().contains("map file"));
}

#[test]
fn error_display() {
    let e = DofError::Spawn(std::io::Error::other("x"));
    assert!(e.to_string().contains("start"));
    assert!(std::error::Error::source(&e).is_some());
    let e = DofError::UnknownName {
        field: "f".into(),
        name: "n".into(),
    };
    assert_eq!(e.to_string(), "f: unknown name 'n'");
    assert!(std::error::Error::source(&e).is_none());
}

/// Undoes `push_json_string` for the subset of JSON the writer produces.
fn json_unescape(s: &str) -> String {
    let mut out = String::new();
    let mut it = s.chars();
    while let Some(c) = it.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match it.next().unwrap() {
            'n' => out.push('\n'),
            'r' => out.push('\r'),
            't' => out.push('\t'),
            'u' => {
                let hex: String = it.by_ref().take(4).collect();
                out.push(char::from_u32(u32::from_str_radix(&hex, 16).unwrap()).unwrap());
            }
            c => out.push(c),
        }
    }
    out
}

proptest! {
    /// Any ROM/table name is written as one JSON string that reads back unchanged.
    #[test]
    fn names_round_trip_through_json(rom in "\\PC{0,24}|[\\x00-\\x1f\"\\\\]{0,8}", name in "\\PC{0,24}") {
        let m = ResolvedDofMap { rom_name: rom.clone(), table_name: name.clone(), entries: Vec::new() };
        let json = m.to_json();
        let line = json.lines().find(|l| l.starts_with("  \"rom_name\": ")).unwrap();
        let lit = line.trim_start_matches("  \"rom_name\": ").trim_end_matches(',');
        prop_assert!(lit.starts_with('"') && lit.ends_with('"') && lit.len() >= 2);
        let inner = &lit[1..lit.len() - 1];
        // No raw control characters or unescaped quotes inside the literal.
        prop_assert!(!inner.chars().any(|c| (c as u32) < 0x20));
        let mut prev_backslashes = 0usize;
        for c in inner.chars() {
            if c == '"' { prop_assert!(prev_backslashes % 2 == 1); }
            prev_backslashes = if c == '\\' { prev_backslashes + 1 } else { 0 };
        }
        prop_assert_eq!(json_unescape(inner), rom);
        prop_assert_eq!(json.lines().count(), 7);
    }
}
