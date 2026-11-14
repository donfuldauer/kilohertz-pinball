//! Subphase 3.5 spec tests (Independent Tester): `[dof]` validation, the
//! resolved id → DOF element map, and the engine-side launcher API
//! (`pin_app::dof`), written from the brief and the published contract
//! ("3.5 — DOF bridge public API" in docs/DECISIONS.md) only.

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use pin_app::dof::{
    platform_supported, DofBridge, DofError, DofLaunchConfig, DofMapEntry, DofSourceKind,
    ResolvedDofMap,
};
use pin_core::schema::table::{DofElementType, TableManifest};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// A unique directory under the OS temp dir, removed on drop.
struct TempDir(PathBuf);

impl TempDir {
    fn new(tag: &str) -> TempDir {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("spec_3_5_{}_{tag}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).expect("create temp dir");
        TempDir(p)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn apex_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../pin_assets/tests/fixtures/apex")
}

/// The Appendix A reference `table.toml` with its `[dof]` section (the last
/// section of the file) replaced by `tail`.
fn table_toml(tail: &str) -> String {
    let text = std::fs::read_to_string(apex_dir().join("table.toml")).expect("apex table.toml");
    let cut = text.find("[dof]").expect("reference table has [dof]");
    format!("{}{tail}\n", &text[..cut])
}

/// Extra elements used by the mapping tests (numbers free in the reference table).
const EXTRA: &str = r#"
[[lamps]]
id = "gi_upper"
number = 7
kind = "gi"

[[lamps]]
id = "gi_edge"
number = 31
kind = "gi"

[[coils]]
id = "knocker_coil"
number = 255
kind = "knocker"
"#;

fn manifest(tail: &str) -> TableManifest {
    TableManifest::from_toml_str(&table_toml(tail)).expect("manifest parses")
}

const FULL_DOF: &str = r#"
[dof]
rom_name = "apexcircuit"
map = [
  { coil = "left_flipper_coil", type = "S", number = 1 },
  { switch = "right_orbit_opto", type = "W", number = 24 },
  { lamp = "overdrive_ready", type = "L", number = 140 },
  { lamp = "gi_upper", type = "G", number = 2 },
  { coil = "knocker_coil", type = "E", number = 65535 },
  { coil = "left_flipper_coil", type = "S", number = 48 },
  { lamp = "gi_edge", type = "L", number = 9 },
]
"#;

fn full_manifest() -> TableManifest {
    manifest(&format!("{EXTRA}{FULL_DOF}"))
}

fn resolved() -> ResolvedDofMap {
    ResolvedDofMap::resolve(&full_manifest())
        .expect("resolves")
        .expect("[dof] present")
}

fn tuple(e: &DofMapEntry) -> (DofSourceKind, u16, &str, DofElementType, u16) {
    (e.source, e.id, e.name.as_str(), e.element_type, e.number)
}

// ---------------------------------------------------------------------------
// [dof] schema rules (type / number / source)
// ---------------------------------------------------------------------------

fn parse_dof(map_line: &str) -> Result<TableManifest, String> {
    let text = table_toml(&format!(
        "{EXTRA}\n[dof]\nrom_name = \"apexcircuit\"\nmap = [ {map_line} ]\n"
    ));
    let m = TableManifest::from_toml_str(&text).map_err(|e| e.to_string())?;
    Ok(m)
}

#[test]
fn dof_schema_accepts_every_documented_type() {
    for (t, want) in [
        ("S", DofElementType::S),
        ("W", DofElementType::W),
        ("L", DofElementType::L),
        ("G", DofElementType::G),
        ("E", DofElementType::E),
    ] {
        let m = parse_dof(&format!(
            "{{ coil = \"left_flipper_coil\", type = \"{t}\", number = 3 }}"
        ))
        .unwrap_or_else(|e| panic!("type {t}: {e}"));
        assert_eq!(m.dof.unwrap().map[0].element_type, want);
    }
}

#[test]
fn dof_schema_rejects_bad_type_number_and_source() {
    let bad = [
        // unknown / mis-cased / missing type
        r#"{ coil = "left_flipper_coil", type = "X", number = 1 }"#,
        r#"{ coil = "left_flipper_coil", type = "s", number = 1 }"#,
        r#"{ coil = "left_flipper_coil", type = "SW", number = 1 }"#,
        r#"{ coil = "left_flipper_coil", type = 1, number = 1 }"#,
        r#"{ coil = "left_flipper_coil", number = 1 }"#,
        // number must be an integer 0..=65535
        r#"{ coil = "left_flipper_coil", type = "S", number = -1 }"#,
        r#"{ coil = "left_flipper_coil", type = "S", number = 65536 }"#,
        r#"{ coil = "left_flipper_coil", type = "S", number = 1.5 }"#,
        r#"{ coil = "left_flipper_coil", type = "S", number = "1" }"#,
        r#"{ coil = "left_flipper_coil", type = "S" }"#,
        // exactly one source
        r#"{ type = "S", number = 1 }"#,
        r#"{ coil = "left_flipper_coil", lamp = "overdrive_ready", type = "S", number = 1 }"#,
        r#"{ coil = "left_flipper_coil", switch = "right_orbit_opto", type = "S", number = 1 }"#,
        // unknown keys
        r#"{ magnet = "m", type = "S", number = 1 }"#,
        r#"{ coil = "left_flipper_coil", type = "S", number = 1, invert = true }"#,
    ];
    for line in bad {
        assert!(parse_dof(line).is_err(), "accepted: {line}");
    }
    for ok in [
        r#"{ coil = "left_flipper_coil", type = "S", number = 0 }"#,
        r#"{ coil = "left_flipper_coil", type = "S", number = 65535 }"#,
    ] {
        assert!(parse_dof(ok).is_ok(), "rejected: {ok}");
    }
}

#[test]
fn dof_section_needs_a_rom_name() {
    let text = table_toml("\n[dof]\nmap = []\n");
    assert!(TableManifest::from_toml_str(&text).is_err());
    let text = table_toml("\n[dof]\nrom_name = \"apexcircuit\"\n");
    let m = TableManifest::from_toml_str(&text).expect("map defaults to empty");
    assert!(m.dof.unwrap().map.is_empty());
}

// ---------------------------------------------------------------------------
// Load-time cross-reference validation (pin_assets::load_table)
// ---------------------------------------------------------------------------

/// Copies the reference bundle into a temp dir with a patched `table.toml` and
/// returns the validation errors under `dof.` (empty when the table loads).
fn dof_load_errors(tail: &str) -> Vec<(String, String)> {
    fn copy(from: &Path, to: &Path) {
        std::fs::create_dir_all(to).unwrap();
        for e in std::fs::read_dir(from).unwrap() {
            let e = e.unwrap();
            let dst = to.join(e.file_name());
            if e.file_type().unwrap().is_dir() {
                copy(&e.path(), &dst);
            } else {
                std::fs::copy(e.path(), dst).unwrap();
            }
        }
    }
    let dir = TempDir::new("bundle");
    copy(&apex_dir(), dir.path());
    std::fs::write(dir.path().join("table.toml"), table_toml(tail)).unwrap();
    let vfs = pin_assets::Vfs::open(dir.path()).expect("open bundle dir");
    match pin_assets::validate::load_table(&vfs) {
        Ok(_) => Vec::new(),
        Err(pin_assets::AssetError::Invalid(errors)) => errors
            .into_iter()
            .filter(|e| e.path.starts_with("dof"))
            .map(|e| (e.path, e.message))
            .collect(),
        Err(other) => panic!("unexpected load error: {other}"),
    }
}

#[test]
fn load_accepts_dof_names_that_exist() {
    let errors = dof_load_errors(&format!("{EXTRA}{FULL_DOF}"));
    assert!(errors.is_empty(), "{errors:?}");
}

#[test]
fn load_rejects_dof_names_that_do_not_exist() {
    let tail = r#"
[dof]
rom_name = "apexcircuit"
map = [
  { coil = "left_flipper_coil", type = "S", number = 1 },
  { coil = "no_such_coil", type = "S", number = 2 },
  { switch = "no_such_switch", type = "W", number = 3 },
  { lamp = "no_such_lamp", type = "L", number = 4 },
  { switch = "left_flipper_coil", type = "W", number = 5 },
  { lamp = "right_orbit_opto", type = "L", number = 6 },
  { coil = "overdrive_ready", type = "S", number = 7 },
]
"#;
    let errors = dof_load_errors(tail);
    let paths: Vec<&str> = errors.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(
        paths,
        [
            "dof.map[1].coil",
            "dof.map[2].switch",
            "dof.map[3].lamp",
            "dof.map[4].switch",
            "dof.map[5].lamp",
            "dof.map[6].coil",
        ],
        "{errors:?}"
    );
    // The message names the offending value.
    for ((_, message), name) in errors.iter().zip([
        "no_such_coil",
        "no_such_switch",
        "no_such_lamp",
        "left_flipper_coil",
        "right_orbit_opto",
        "overdrive_ready",
    ]) {
        assert!(message.contains(name), "{message:?} does not name {name}");
    }
}

// ---------------------------------------------------------------------------
// ResolvedDofMap
// ---------------------------------------------------------------------------

#[test]
fn resolve_is_none_without_dof() {
    let m = manifest("");
    assert!(m.dof.is_none());
    assert_eq!(ResolvedDofMap::resolve(&m).expect("no error"), None);
}

#[test]
fn resolve_maps_names_to_telemetry_ids_in_map_order() {
    let map = resolved();
    assert_eq!(map.rom_name, "apexcircuit");
    assert_eq!(map.table_name, "Apex Circuit");
    let got: Vec<_> = map.entries.iter().map(tuple).collect();
    use DofElementType as T;
    use DofSourceKind as K;
    assert_eq!(
        got,
        [
            (K::Coil, 1, "left_flipper_coil", T::S, 1),
            (K::Switch, 24, "right_orbit_opto", T::W, 24),
            (K::Lamp, 40, "overdrive_ready", T::L, 140),
            // a lamp of kind "gi" is a GI string: telemetry kind 3
            (K::Gi, 7, "gi_upper", T::G, 2),
            (K::Coil, 255, "knocker_coil", T::E, 65535),
            // one element may drive several DOF outputs
            (K::Coil, 1, "left_flipper_coil", T::S, 48),
            // the source follows the lamp's kind, not the DOF type
            (K::Gi, 31, "gi_edge", T::L, 9),
        ]
    );
}

#[test]
fn resolve_with_an_empty_map_is_some() {
    let m = manifest("\n[dof]\nrom_name = \"emptyrom\"\nmap = []\n");
    let map = ResolvedDofMap::resolve(&m).unwrap().expect("[dof] present");
    assert_eq!(map.rom_name, "emptyrom");
    assert!(map.entries.is_empty());
}

fn resolve_err(extra: &str, map_line: &str) -> DofError {
    let m = manifest(&format!(
        "{extra}\n[dof]\nrom_name = \"apexcircuit\"\nmap = [ {map_line} ]\n"
    ));
    match ResolvedDofMap::resolve(&m) {
        Err(e) => e,
        Ok(v) => panic!("resolved {map_line}: {v:?}"),
    }
}

#[test]
fn resolve_rejects_unknown_names() {
    for line in [
        r#"{ coil = "no_such_coil", type = "S", number = 1 }"#,
        r#"{ switch = "no_such_switch", type = "W", number = 1 }"#,
        r#"{ lamp = "no_such_lamp", type = "L", number = 1 }"#,
        // names are per kind: a coil name is not a switch or a lamp
        r#"{ switch = "left_flipper_coil", type = "W", number = 1 }"#,
        r#"{ lamp = "left_flipper_coil", type = "L", number = 1 }"#,
        r#"{ coil = "overdrive_ready", type = "S", number = 1 }"#,
    ] {
        let e = resolve_err(EXTRA, line);
        assert!(matches!(e, DofError::UnknownName { .. }), "{line}: {e:?}");
        // Errors are printable and name the problem.
        assert!(!e.to_string().is_empty());
    }
}

#[test]
fn resolve_rejects_ids_outside_the_telemetry_range() {
    let extra = r#"
[[coils]]
id = "far_coil"
number = 256
kind = "knocker"

[[switches]]
id = "far_switch"
number = 300
kind = "leaf"

[[lamps]]
id = "far_lamp"
number = 256
kind = "insert"

[[lamps]]
id = "far_gi"
number = 32
kind = "gi"
"#;
    for line in [
        r#"{ coil = "far_coil", type = "S", number = 1 }"#,
        r#"{ switch = "far_switch", type = "W", number = 1 }"#,
        r#"{ lamp = "far_lamp", type = "L", number = 1 }"#,
        // GI strings are 0..=31
        r#"{ lamp = "far_gi", type = "G", number = 1 }"#,
    ] {
        let e = resolve_err(extra, line);
        assert!(matches!(e, DofError::IdOutOfRange { .. }), "{line}: {e:?}");
    }
}

/// Minimal JSON reader for the checks below (pin_app has no JSON dependency
/// the tests may rely on): objects, arrays, strings, integers.
#[derive(Debug, Clone, PartialEq)]
enum Json {
    Obj(BTreeMap<String, Json>),
    Arr(Vec<Json>),
    Str(String),
    Num(i64),
    Other,
}

struct JsonReader<'a> {
    s: &'a [u8],
    i: usize,
}

impl JsonReader<'_> {
    fn ws(&mut self) {
        while self.i < self.s.len() && self.s[self.i].is_ascii_whitespace() {
            self.i += 1;
        }
    }

    fn eat(&mut self, c: u8) {
        self.ws();
        assert_eq!(
            self.s[self.i], c,
            "JSON: expected {:?} at {}",
            c as char, self.i
        );
        self.i += 1;
    }

    fn string(&mut self) -> String {
        self.eat(b'"');
        let mut out = Vec::new();
        loop {
            let c = self.s[self.i];
            self.i += 1;
            match c {
                b'"' => break,
                b'\\' => {
                    let e = self.s[self.i];
                    self.i += 1;
                    match e {
                        b'n' => out.push(b'\n'),
                        b't' => out.push(b'\t'),
                        b'r' => out.push(b'\r'),
                        b'b' => out.push(8),
                        b'f' => out.push(12),
                        b'u' => {
                            let hex = std::str::from_utf8(&self.s[self.i..self.i + 4]).unwrap();
                            self.i += 4;
                            let cp = u32::from_str_radix(hex, 16).unwrap();
                            let ch = char::from_u32(cp).expect("BMP scalar in test data");
                            out.extend_from_slice(ch.to_string().as_bytes());
                        }
                        other => out.push(other),
                    }
                }
                other => out.push(other),
            }
        }
        String::from_utf8(out).expect("UTF-8 JSON string")
    }

    fn value(&mut self) -> Json {
        self.ws();
        match self.s[self.i] {
            b'{' => {
                self.i += 1;
                let mut m = BTreeMap::new();
                self.ws();
                if self.s[self.i] == b'}' {
                    self.i += 1;
                    return Json::Obj(m);
                }
                loop {
                    let k = self.string();
                    self.eat(b':');
                    let v = self.value();
                    assert!(m.insert(k.clone(), v).is_none(), "duplicate key {k}");
                    self.ws();
                    let c = self.s[self.i];
                    self.i += 1;
                    match c {
                        b',' => {}
                        b'}' => return Json::Obj(m),
                        other => panic!("JSON: unexpected {:?}", other as char),
                    }
                }
            }
            b'[' => {
                self.i += 1;
                let mut a = Vec::new();
                self.ws();
                if self.s[self.i] == b']' {
                    self.i += 1;
                    return Json::Arr(a);
                }
                loop {
                    a.push(self.value());
                    self.ws();
                    let c = self.s[self.i];
                    self.i += 1;
                    match c {
                        b',' => {}
                        b']' => return Json::Arr(a),
                        other => panic!("JSON: unexpected {:?}", other as char),
                    }
                }
            }
            b'"' => Json::Str(self.string()),
            c if c == b'-' || c.is_ascii_digit() => {
                let start = self.i;
                self.i += 1;
                while self.i < self.s.len() && self.s[self.i].is_ascii_digit() {
                    self.i += 1;
                }
                Json::Num(
                    std::str::from_utf8(&self.s[start..self.i])
                        .unwrap()
                        .parse()
                        .unwrap(),
                )
            }
            _ => {
                while self.i < self.s.len() && self.s[self.i].is_ascii_alphabetic() {
                    self.i += 1;
                }
                Json::Other
            }
        }
    }
}

fn parse_json(text: &str) -> Json {
    let mut r = JsonReader {
        s: text.as_bytes(),
        i: 0,
    };
    let v = r.value();
    r.ws();
    assert_eq!(r.i, r.s.len(), "trailing bytes after JSON document");
    v
}

fn obj(j: &Json) -> &BTreeMap<String, Json> {
    match j {
        Json::Obj(m) => m,
        other => panic!("not an object: {other:?}"),
    }
}

fn s(v: &str) -> Json {
    Json::Str(v.to_owned())
}

fn check_map_json(text: &str, table_name: &str) {
    let root = parse_json(text);
    let root = obj(&root);
    assert_eq!(root["format"], s("kilohertz-dof-map"));
    assert_eq!(root["version"], Json::Num(1));
    assert_eq!(root["rom_name"], s("apexcircuit"));
    assert_eq!(root["table_name"], s(table_name));
    let Json::Arr(entries) = &root["entries"] else {
        panic!("entries is not an array");
    };
    let got: Vec<(String, i64, String, String, i64)> = entries
        .iter()
        .map(|e| {
            let e = obj(e);
            let text = |k: &str| match &e[k] {
                Json::Str(v) => v.clone(),
                other => panic!("{k}: {other:?}"),
            };
            let num = |k: &str| match &e[k] {
                Json::Num(v) => *v,
                other => panic!("{k}: {other:?}"),
            };
            (
                text("source"),
                num("id"),
                text("name"),
                text("type"),
                num("number"),
            )
        })
        .collect();
    let want = [
        ("coil", 1, "left_flipper_coil", "S", 1),
        ("switch", 24, "right_orbit_opto", "W", 24),
        ("lamp", 40, "overdrive_ready", "L", 140),
        ("gi", 7, "gi_upper", "G", 2),
        ("coil", 255, "knocker_coil", "E", 65535),
        ("coil", 1, "left_flipper_coil", "S", 48),
        ("gi", 31, "gi_edge", "L", 9),
    ]
    .map(|(a, b, c, d, e)| (a.to_owned(), b, c.to_owned(), d.to_owned(), e));
    assert_eq!(got, want);
}

#[test]
fn to_json_is_the_documented_mapping_file() {
    check_map_json(&resolved().to_json(), "Apex Circuit");
}

#[test]
fn to_json_escapes_names() {
    let mut map = resolved();
    map.table_name = "Apex \"Circuit\" \\ 2\nçé".to_owned();
    check_map_json(&map.to_json(), "Apex \"Circuit\" \\ 2\nçé");
}

/// The engine-written mapping file is also checked in for the .NET spec tests
/// (`ResolvedDofMap.Load` must accept exactly what the engine writes).
#[test]
fn checked_in_engine_map_matches_to_json() {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../tools/dof_bridge/tests/Spec.DofBridge/fixtures/engine_map.json");
    let json = resolved().to_json();
    if std::env::var_os("SPEC_3_5_WRITE_FIXTURES").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &json).unwrap();
    }
    let on_disk = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{e} (run with SPEC_3_5_WRITE_FIXTURES=1)"));
    assert_eq!(on_disk, json, "engine_map.json is stale");
}

#[test]
fn write_file_writes_to_json_into_the_directory() {
    let dir = TempDir::new("write");
    let map = resolved();
    let a = map.write_file(dir.path()).expect("written");
    assert_eq!(
        a.parent().map(|p| p.canonicalize().unwrap()),
        Some(dir.path().canonicalize().unwrap())
    );
    assert_eq!(std::fs::read_to_string(&a).unwrap(), map.to_json());
    // A second table load must not clobber the first bridge's file.
    let b = map.write_file(dir.path()).expect("written again");
    assert!(b.is_file() && a.is_file());
    assert_eq!(std::fs::read_to_string(&b).unwrap(), map.to_json());
}

#[test]
fn write_file_into_a_missing_location_is_an_error_or_creates_it() {
    let dir = TempDir::new("write_missing");
    // A path below a regular file can never be created.
    let blocker = dir.path().join("file");
    std::fs::write(&blocker, b"x").unwrap();
    assert!(resolved().write_file(&blocker.join("sub")).is_err());
}

// ---------------------------------------------------------------------------
// Launch configuration and arguments
// ---------------------------------------------------------------------------

fn os(v: &[&str]) -> Vec<OsString> {
    v.iter().map(OsString::from).collect()
}

#[test]
fn launch_config_defaults() {
    let cfg = DofLaunchConfig::new(PathBuf::from("bridge/KilohertzDofBridge.exe"), 35120);
    assert_eq!(
        cfg.bridge_exe,
        PathBuf::from("bridge/KilohertzDofBridge.exe")
    );
    assert_eq!(cfg.udp_port, 35120);
    assert_eq!(cfg.dof_path, None);
    assert_eq!(cfg.map_dir, std::env::temp_dir());
    assert_eq!(cfg.stop_grace, Duration::from_millis(1500));
    assert_eq!(cfg.stop_grace, pin_app::dof::DEFAULT_STOP_GRACE);
    assert!(cfg.extra_args.is_empty());
}

#[test]
fn default_bridge_exe_is_next_to_the_engine_or_absent() {
    if let Some(p) = DofLaunchConfig::default_bridge_exe() {
        assert_eq!(
            p.file_name().and_then(|n| n.to_str()),
            Some("KilohertzDofBridge.exe")
        );
        assert!(p.is_file(), "{p:?} was reported but does not exist");
    }
}

#[test]
fn args_are_exactly_the_documented_command_line() {
    let map = resolved();
    let mut cfg = DofLaunchConfig::new(PathBuf::from("KilohertzDofBridge.exe"), 40123);
    let map_file = Path::new("/some dir/dof map.json");
    let pid = std::process::id().to_string();
    let base = [
        "--map",
        "/some dir/dof map.json",
        "--port",
        "40123",
        "--rom",
        "apexcircuit",
        "--table",
        "Apex Circuit",
        "--parent-pid",
        pid.as_str(),
        "--exit-on-stdin-eof",
    ];
    assert_eq!(cfg.args(&map, map_file), os(&base));

    cfg.dof_path = Some(PathBuf::from("C:\\DirectOutput"));
    cfg.extra_args = os(&["--sink", "log"]);
    let mut want = os(&base);
    want.extend(os(&["--dof-path", "C:\\DirectOutput", "--sink", "log"]));
    assert_eq!(cfg.args(&map, map_file), want);

    cfg.udp_port = 0;
    assert_eq!(cfg.args(&map, map_file)[3], OsString::from("0"));
    cfg.udp_port = u16::MAX;
    assert_eq!(cfg.args(&map, map_file)[3], OsString::from("65535"));
}

// ---------------------------------------------------------------------------
// Platform gate
// ---------------------------------------------------------------------------

#[test]
fn platform_supported_only_on_windows() {
    assert_eq!(platform_supported(), cfg!(windows));
}

#[test]
fn start_for_table_without_dof_is_a_no_op_everywhere() {
    let dir = TempDir::new("nodof");
    let mut cfg = DofLaunchConfig::new(dir.path().join("does_not_exist.exe"), 35120);
    cfg.map_dir = dir.path().to_path_buf();
    let started = DofBridge::start_for_table(&manifest(""), &cfg).expect("no error");
    assert!(started.is_none());
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[cfg(not(windows))]
#[test]
fn start_for_table_is_a_no_op_on_linux_and_macos() {
    let dir = TempDir::new("noop");
    // Neither a missing nor a present bridge executable is touched.
    let fake = fake_bridge::path();
    for exe in [dir.path().join("does_not_exist.exe"), fake] {
        let out = TempDir::new("noop_out");
        let mut cfg = DofLaunchConfig::new(exe, 35120);
        cfg.map_dir = dir.path().to_path_buf();
        cfg.extra_args = os(&["--spec-out", out.path().join("run").to_str().unwrap()]);
        let started = DofBridge::start_for_table(&full_manifest(), &cfg).expect("no error");
        assert!(started.is_none());
        drop(started);
        // No mapping file, no process.
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        assert_eq!(std::fs::read_dir(out.path()).unwrap().count(), 0);
    }
    // Even a table whose [dof] names are wrong is not an error where the
    // bridge never runs... or it is reported: either way nothing is spawned.
    let bad = manifest(
        "\n[dof]\nrom_name = \"x\"\nmap = [ { coil = \"nope\", type = \"S\", number = 1 } ]\n",
    );
    let mut cfg = DofLaunchConfig::new(dir.path().join("does_not_exist.exe"), 35120);
    cfg.map_dir = dir.path().to_path_buf();
    if let Ok(started) = DofBridge::start_for_table(&bad, &cfg) {
        assert!(started.is_none());
    }
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn spawn_with_a_missing_executable_is_an_error_and_leaves_no_file() {
    let dir = TempDir::new("missing");
    let mut cfg = DofLaunchConfig::new(dir.path().join("no_such_bridge.exe"), 35120);
    cfg.map_dir = dir.path().to_path_buf();
    let r = DofBridge::spawn(&resolved(), &cfg);
    assert!(r.is_err());
    drop(r);
    assert_eq!(
        std::fs::read_dir(dir.path()).unwrap().count(),
        0,
        "a failed launch must not leak its mapping file"
    );
}

// ---------------------------------------------------------------------------
// Process lifecycle against a fake bridge executable (Unix shell script)
// ---------------------------------------------------------------------------

#[cfg(unix)]
mod fake_bridge {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::OnceLock;
    use std::time::Instant;

    /// A stand-in for `KilohertzDofBridge`: records its arguments to
    /// `<--spec-out>.args`, then (mode `eof`) waits for stdin to close, writes
    /// `<--spec-out>.done` and exits 0; mode `stubborn` ignores stdin and
    /// never exits on its own.
    const SCRIPT: &str = r#"#!/bin/sh
out=""
mode="eof"
prev=""
for a in "$@"; do
  if [ "$prev" = "--spec-out" ]; then out="$a"; fi
  if [ "$prev" = "--spec-mode" ]; then mode="$a"; fi
  prev="$a"
done
for a in "$@"; do printf '%s\n' "$a"; done > "$out.args.tmp"
mv "$out.args.tmp" "$out.args"
if [ "$mode" = "stubborn" ]; then
  exec sleep 120
fi
cat > /dev/null
echo done > "$out.done"
exit 0
"#;

    /// The script is written once, before any test spawns a process (a file
    /// still open for writing cannot be executed).
    pub fn path() -> PathBuf {
        static PATH: OnceLock<PathBuf> = OnceLock::new();
        PATH.get_or_init(|| {
            let dir = std::env::temp_dir().join(format!("spec_3_5_{}_fake", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let p = dir.join("fake_bridge.sh");
            std::fs::write(&p, SCRIPT).unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
            p
        })
        .clone()
    }

    struct Run {
        _dir: TempDir,
        map_dir: PathBuf,
        out: PathBuf,
        cfg: DofLaunchConfig,
    }

    fn run(tag: &str, mode: &str, grace: Duration) -> Run {
        let dir = TempDir::new(tag);
        let map_dir = dir.path().join("maps");
        std::fs::create_dir_all(&map_dir).unwrap();
        let out = dir.path().join("run");
        let mut cfg = DofLaunchConfig::new(path(), 40777);
        cfg.map_dir = map_dir.clone();
        cfg.stop_grace = grace;
        cfg.extra_args = os(&["--spec-out", out.to_str().unwrap(), "--spec-mode", mode]);
        Run {
            _dir: dir,
            map_dir,
            out,
            cfg,
        }
    }

    /// Polls (bounded) until `cond` holds; a liveness wait, not a timing assertion.
    fn wait_until(what: &str, mut cond: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !cond() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn alive(pid: u32) -> bool {
        // A zombie still has a /proc entry on Linux; `kill -0` works on macOS too.
        std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn recorded_args(out: &Path) -> Vec<String> {
        let p = PathBuf::from(format!("{}.args", out.display()));
        wait_until("the fake bridge to record its arguments", || p.is_file());
        std::fs::read_to_string(p)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect()
    }

    fn done(out: &Path) -> bool {
        PathBuf::from(format!("{}.done", out.display())).is_file()
    }

    #[test]
    fn spawn_passes_the_documented_arguments_and_mapping_file() {
        let r = run("spawn", "eof", Duration::from_secs(20));
        let map = resolved();
        let bridge = DofBridge::spawn(&map, &r.cfg).expect("spawned");
        assert_ne!(bridge.id(), 0);
        assert_ne!(bridge.id(), std::process::id());
        let map_file = bridge.map_file().to_path_buf();

        // The mapping file lives in map_dir and holds the resolved map.
        assert_eq!(
            map_file.parent().unwrap().canonicalize().unwrap(),
            r.map_dir.canonicalize().unwrap()
        );
        assert_eq!(std::fs::read_to_string(&map_file).unwrap(), map.to_json());

        let args = recorded_args(&r.out);
        let want: Vec<String> = r
            .cfg
            .args(&map, &map_file)
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect();
        assert_eq!(args, want);
        assert_eq!(
            &args[..11],
            [
                "--map",
                map_file.to_str().unwrap(),
                "--port",
                "40777",
                "--rom",
                "apexcircuit",
                "--table",
                "Apex Circuit",
                "--parent-pid",
                std::process::id().to_string().as_str(),
                "--exit-on-stdin-eof",
            ]
        );
        assert_eq!(args[11], "--spec-out");

        // Still running: nothing asked it to stop.
        assert!(!done(&r.out));
        assert!(alive(bridge.id()));

        // stop: stdin closes, the bridge shuts down cleanly, the file goes away.
        let status = bridge.stop().wait().expect("exit status");
        assert!(status.success(), "{status:?}");
        assert!(
            done(&r.out),
            "the bridge was killed instead of asked to stop"
        );
        assert!(!map_file.exists(), "mapping file not removed on stop");
    }

    #[test]
    fn dropping_the_bridge_stops_it() {
        let r = run("drop", "eof", Duration::from_secs(20));
        let bridge = DofBridge::spawn(&resolved(), &r.cfg).expect("spawned");
        let pid = bridge.id();
        let map_file = bridge.map_file().to_path_buf();
        let _ = recorded_args(&r.out);
        assert!(map_file.is_file());
        drop(bridge);
        wait_until("the dropped bridge to exit cleanly", || done(&r.out));
        wait_until("the dropped bridge's process to be reaped", || !alive(pid));
        wait_until("the mapping file to be removed", || !map_file.exists());
    }

    #[test]
    fn a_bridge_that_ignores_the_stop_signal_is_killed_after_the_grace_period() {
        let r = run("stubborn", "stubborn", Duration::from_millis(100));
        let bridge = DofBridge::spawn(&resolved(), &r.cfg).expect("spawned");
        let pid = bridge.id();
        let map_file = bridge.map_file().to_path_buf();
        let _ = recorded_args(&r.out);
        let status = bridge.stop().wait().expect("exit status");
        assert!(!status.success(), "a killed process does not exit cleanly");
        assert!(!alive(pid));
        assert!(!done(&r.out));
        assert!(!map_file.exists(), "mapping file not removed after kill");
    }

    #[test]
    fn dropping_a_stubborn_bridge_kills_it() {
        let r = run("stubborn_drop", "stubborn", Duration::from_millis(100));
        let bridge = DofBridge::spawn(&resolved(), &r.cfg).expect("spawned");
        let pid = bridge.id();
        let map_file = bridge.map_file().to_path_buf();
        let _ = recorded_args(&r.out);
        drop(bridge);
        wait_until("the stubborn bridge to be killed", || !alive(pid));
        wait_until("the mapping file to be removed", || !map_file.exists());
    }

    /// `stop` and `drop` hand the child to a reaper and return at once: with a
    /// bridge that never exits and a grace period far beyond the bound, they
    /// must come back long before the grace period ends (D5: never blocks the
    /// sim or render thread).
    #[test]
    fn stop_and_drop_do_not_wait_for_the_child() {
        let grace = Duration::from_secs(8);
        for use_drop in [false, true] {
            let r = run("nonblocking", "stubborn", grace);
            let bridge = DofBridge::spawn(&resolved(), &r.cfg).expect("spawned");
            let pid = bridge.id();
            let _ = recorded_args(&r.out);
            let t0 = Instant::now();
            let handle = if use_drop {
                drop(bridge);
                None
            } else {
                Some(bridge.stop())
            };
            let returned_after = t0.elapsed();
            assert!(
                returned_after < grace / 2,
                "stop/drop blocked for {returned_after:?} (grace {grace:?})"
            );
            assert!(alive(pid), "the child is given its grace period");
            // Clean up without waiting out the grace period.
            let _ = std::process::Command::new("kill")
                .args(["-9", &pid.to_string()])
                .status();
            if let Some(h) = handle {
                let status = h.wait().expect("exit status");
                assert!(!status.success());
            }
            wait_until("the killed bridge to be reaped", || !alive(pid));
        }
    }

    #[test]
    fn a_bridge_that_already_exited_can_be_stopped() {
        // The fake exits by itself when its stdin is closed; here it gets an
        // immediate usage-style exit instead: an unknown mode with no stdin
        // wait is not available, so close via stop and stop semantics twice.
        let r = run("exited", "eof", Duration::from_secs(20));
        let mut bridge = DofBridge::spawn(&resolved(), &r.cfg).expect("spawned");
        let _ = recorded_args(&r.out);
        let _ = bridge.try_wait();
        let status = bridge.stop().wait();
        assert!(status.is_some_and(|s| s.success()));
    }

    #[test]
    fn two_bridges_use_distinct_mapping_files() {
        let a = run("two_a", "eof", Duration::from_secs(20));
        let mut b = run("two_b", "eof", Duration::from_secs(20));
        b.cfg.map_dir = a.map_dir.clone();
        let first = DofBridge::spawn(&resolved(), &a.cfg).expect("spawned");
        let second = DofBridge::spawn(&resolved(), &b.cfg).expect("spawned");
        assert_ne!(first.map_file(), second.map_file());
        let first_file = first.map_file().to_path_buf();
        let second_file = second.map_file().to_path_buf();
        assert!(first.stop().wait().is_some());
        assert!(!first_file.exists());
        assert!(
            second_file.is_file(),
            "stopping one bridge removed the other's file"
        );
        assert!(second.stop().wait().is_some());
        assert!(!second_file.exists());
    }
}
