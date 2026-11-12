//! Subphase 4.1 spec tests (Independent Tester): manifest loading, interning
//! and validation. Brief: deliverable 2, FIXED API `load_table` / `LoadedTable`
//! / `TableIds` / `IdKind` / `ValidationError`, VALIDATION RULES, verification
//! criteria 1 and 3.

#[path = "spec_fixtures/mod.rs"]
mod fx;

use fx::{
    BACKGLASS_TOML, TABLE_TOML, TempDir, backglass_errors, bundle, bundle_with, expect_at,
    expect_under, invalid, sub, table_errors, vfs_from,
};
use pin_assets::{AssetError, FlipperStageHint, IdKind, LoadedTable, TableIds, Vfs, load_table};
use pin_core::schema::table::NameResolver;
use pin_core::schema::{PhysicsDefaults, TableManifest};
use pin_core::{
    BackglassManifest, CoilId, DispenserId, DiverterId, FlipperId, LampId, MagnetId, PersistKey,
    ProfileId, SceneId, SoundId, SwitchId,
};

const T: &str = "table.toml";
const B: &str = "backglass.toml";

fn load(table: &str, backglass: &str) -> LoadedTable {
    let vfs = vfs_from(&bundle_with(table, backglass));
    match load_table(&vfs) {
        Ok(t) => t,
        Err(e) => panic!("valid bundle failed to load: {e}"),
    }
}

fn plus(extra: &str) -> String {
    format!("{TABLE_TOML}\n{extra}\n")
}

struct Names<'a>(&'a TableIds);
impl NameResolver for Names<'_> {
    fn coil(&self, name: &str) -> Option<CoilId> {
        self.0.coil(name)
    }
    fn switch(&self, name: &str) -> Option<SwitchId> {
        self.0.switch(name)
    }
}

// ============================ criterion 1 ===================================

#[test]
fn mock_bundle_loads() {
    let t = load(TABLE_TOML, BACKGLASS_TOML);
    assert_eq!(t.manifest, TableManifest::from_toml_str(TABLE_TOML).unwrap());
    assert_eq!(
        t.backglass,
        BackglassManifest::from_toml_str(BACKGLASS_TOML).unwrap()
    );
    assert_eq!(t.physics, PhysicsDefaults::builtin().resolve(None));
    assert_eq!(t.flippers.len(), 1);
    assert_eq!(t.flipper_stages.len(), 1);
    assert_eq!(t.switches.len(), t.manifest.switches.len());
    assert_eq!(t.magnets.len(), 1);
    assert_eq!(t.splines.len(), 1);
    assert_eq!(t.transitions.len(), 1);
    assert_eq!(t.dispensers.len(), 1);
    assert!(
        !t.warnings.iter().any(|w| w.path.starts_with("splines")),
        "evenly spaced Appendix A wireform must not warn: {:?}",
        t.warnings
    );
}

#[test]
fn loads_identically_from_folder_zip_and_pinbundle() {
    let files = bundle();
    let outer = TempDir::new("bundle");
    let root = outer.path().join("apex_circuit");
    fx::write_tree(&root, &files);
    let zip_bytes = fx::zip_of(&files);
    let zip_path = outer.path().join("apex_circuit.zip");
    let pb_path = outer.path().join("apex_circuit.pinbundle");
    std::fs::write(&zip_path, &zip_bytes).unwrap();
    std::fs::write(&pb_path, &zip_bytes).unwrap();

    // A zip whose content sits in one wrapping folder, as "zip the folder" tools make.
    let wrapped: std::collections::BTreeMap<String, Vec<u8>> = files
        .iter()
        .map(|(k, v)| (format!("apex_circuit/{k}"), v.clone()))
        .collect();
    let wrapped_path = outer.path().join("wrapped.zip");
    std::fs::write(&wrapped_path, fx::zip_of(&wrapped)).unwrap();

    let dir = Vfs::open(&root).unwrap();
    let from_dir = load_table(&dir).unwrap_or_else(|e| panic!("folder: {e}"));
    let names: Vec<String> = files.keys().cloned().collect();
    assert_eq!(dir.list(), names);

    for path in [&zip_path, &pb_path, &wrapped_path] {
        let v = Vfs::open(path).unwrap();
        assert_eq!(v.list(), names, "{}", path.display());
        for (name, data) in &files {
            assert_eq!(&v.read(name).unwrap(), data, "{name} in {}", path.display());
            assert_eq!(&dir.read(name).unwrap(), data, "{name} in folder");
        }
        let from_zip = load_table(&v).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
        assert_eq!(from_zip, from_dir, "{}", path.display());
        assert_eq!(from_zip.clone(), from_zip);
    }
}

// ============================ interning =====================================

#[test]
fn ids_follow_the_binding_interning_rule() {
    let t = load(
        &plus(
            r#"[[mechs]]
id = "second_diverter"
kind = "diverter"
coil = "ramp_diverter_coil"

[[sounds]]
id = "sfx_two"
file = "audio/bumper.ogg"
bus = "music"

[[scenes]]
id = "scene_two"
file = "display/powerball_mania.toml"

[[flippers]]
id = "lower_right"
coil = "left_flipper_coil"
eos_switch = "lower_left_eos"
base_position = [330.0, 120.0, 0.0]
surface_normal = [0.0, 0.0, 1.0]
rest_angle_deg = 210.0
sweep_angle_deg = -50.0
eos_angle_fraction = 0.92
shape = { type = "StraightCapsule", length = 76.0, base_radius = 11.0, tip_radius = 6.5 }
staging = { type = "DedicatedButton", input = "right_flipper" }
"#,
        ),
        BACKGLASS_TOML,
    );
    let ids = &t.ids;
    // number == id
    assert_eq!(ids.switch("right_orbit_opto"), Some(SwitchId(24)));
    assert_eq!(ids.switch("lower_left_eos"), Some(SwitchId(25)));
    assert_eq!(ids.switch("ramp_made_opto"), Some(SwitchId(28)));
    assert_eq!(ids.coil("left_flipper_coil"), Some(CoilId(1)));
    assert_eq!(ids.coil("ramp_diverter_coil"), Some(CoilId(3)));
    assert_eq!(ids.lamp("overdrive_ready"), Some(LampId(40)));
    assert_eq!(ids.lamp("backglass_logo"), Some(LampId(41)));
    assert_eq!(ids.magnet("orbit_magnet"), Some(MagnetId(1)));
    assert_eq!(ids.dispenser("power_chamber"), Some(DispenserId(1)));
    // declaration order from 0
    assert_eq!(ids.flipper("lower_left"), Some(FlipperId(0)));
    assert_eq!(ids.flipper("lower_right"), Some(FlipperId(1)));
    assert_eq!(ids.diverter("ramp_diverter"), Some(DiverterId(0)));
    assert_eq!(ids.diverter("second_diverter"), Some(DiverterId(1)));
    assert_eq!(ids.diverter("bumper_1"), None, "only mechs of kind diverter");
    assert_eq!(ids.sound("sfx_bumper"), Some(SoundId(0)));
    assert_eq!(ids.sound("sfx_two"), Some(SoundId(1)));
    assert_eq!(ids.scene("scene_powerball_mania"), Some(SceneId(0)));
    assert_eq!(ids.scene("scene_two"), Some(SceneId(1)));
    assert_eq!(ids.persist_key("high_score_1"), Some(PersistKey(0)));
    assert_eq!(ids.persist_key("high_score_2"), Some(PersistKey(1)));
    assert_eq!(ids.persist_key("games_played"), Some(PersistKey(2)));
    // profiles: index in BUILTIN_PROFILE_NAMES
    for (i, name) in pin_core::profiles::BUILTIN_PROFILE_NAMES.iter().enumerate() {
        assert_eq!(ids.profile(name), Some(ProfileId(i as u16)), "{name}");
    }
    // unknown names, and no cross-kind leakage
    assert_eq!(ids.switch("nope"), None);
    assert_eq!(ids.switch("left_flipper_coil"), None);
    assert_eq!(ids.coil("right_orbit_opto"), None);
    assert_eq!(ids.lamp(""), None);
    assert_eq!(ids.profile("unobtainium"), None);
    assert_eq!(ids.persist_key("high_score_9"), None);
}

#[test]
fn entries_give_reverse_lookup_per_kind() {
    let t = load(TABLE_TOML, BACKGLASS_TOML);
    let ids = &t.ids;
    let has = |kind: IdKind, name: &str, id: u16| {
        let e = ids.entries(kind);
        assert!(
            e.iter().any(|(n, i)| n == name && *i == id),
            "entries missing ({name:?}, {id}): {e:?}"
        );
    };
    has(IdKind::Switch, "right_orbit_opto", 24);
    has(IdKind::Coil, "bumper_1_coil", 2);
    has(IdKind::Lamp, "overdrive_ready", 40);
    has(IdKind::Magnet, "orbit_magnet", 1);
    has(IdKind::Diverter, "ramp_diverter", 0);
    has(IdKind::Flipper, "lower_left", 0);
    has(IdKind::Dispenser, "power_chamber", 1);
    has(IdKind::Profile, "ceramic_powerball", 2);
    has(IdKind::Sound, "sfx_bumper", 0);
    has(IdKind::Scene, "scene_powerball_mania", 0);
    has(IdKind::PersistKey, "games_played", 2);

    assert_eq!(ids.entries(IdKind::Coil).len(), 3);
    assert_eq!(ids.entries(IdKind::Lamp).len(), 2);
    assert_eq!(ids.entries(IdKind::Flipper).len(), 1);
    assert_eq!(ids.entries(IdKind::Diverter).len(), 1);
    assert_eq!(ids.entries(IdKind::PersistKey).len(), 3);
    assert_eq!(ids.entries(IdKind::Profile).len(), 7);

    // Cabinet switches are switches by number too (1..=8 in the fixture).
    let sw = ids.entries(IdKind::Switch);
    for n in 1..=8u16 {
        assert!(
            sw.iter().any(|(_, i)| *i == n),
            "cabinet switch number {n} is not interned as a switch: {sw:?}"
        );
    }
    // Every entry agrees with the forward lookup, and names are unique.
    for (name, id) in sw {
        assert_eq!(ids.switch(name), Some(SwitchId(*id)), "{name}");
    }
    let mut names: Vec<&String> = sw.iter().map(|(n, _)| n).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), sw.len());
}

// ============================ derived data ==================================

#[test]
fn physics_overrides_are_resolved() {
    let table = sub(TABLE_TOML, "[physics]\n", "[physics]\nflipper_rubber_damping = 0.35\n");
    let t = load(&table, BACKGLASS_TOML);
    assert_eq!(t.physics.flipper_rubber_damping, 0.35);
    assert_eq!(
        t.physics,
        PhysicsDefaults::builtin().resolve(t.manifest.physics.as_ref())
    );
    assert_ne!(t.physics, PhysicsDefaults::builtin().resolve(None));
    assert_eq!(t.flippers[0].rubber_damping, 0.35);
}

#[test]
fn flippers_are_built_via_to_config() {
    let t = load(TABLE_TOML, BACKGLASS_TOML);
    let want = t.manifest.flippers[0]
        .to_config(FlipperId(0), &Names(&t.ids), &t.physics)
        .unwrap();
    assert_eq!(t.flippers[0], want);
    assert_eq!(t.flippers[0].id, FlipperId(0));
    assert_eq!(t.flippers[0].coil, CoilId(1));
    assert_eq!(t.flippers[0].eos_switch, SwitchId(25));
    assert_eq!(t.flippers[0].power_torque_nm, t.physics.flipper_power_torque);
}

#[test]
fn auto_sensor_staging_resolves_its_trigger_switch() {
    let table = sub(
        TABLE_TOML,
        r#"staging = { type = "DedicatedButton", input = "left_flipper" }"#,
        r#"staging = { type = "AutoSensorThingFlip", trigger_switch = "right_orbit_opto", delay_ms = 20.0, hold_duration_ms = 150.0 }"#,
    );
    let t = load(&table, BACKGLASS_TOML);
    assert_eq!(
        t.flippers[0].staging,
        pin_core::FlipperStagingMode::AutoSensorThingFlip {
            trigger_switch: SwitchId(24),
            delay_ms: 20.0,
            hold_duration_ms: 150.0
        }
    );
}

fn flipper(id: &str, layer: u8, staging: &str) -> String {
    format!(
        r#"[[flippers]]
id = "{id}"
coil = "left_flipper_coil"
eos_switch = "lower_left_eos"
collision_layer = {layer}
base_position = [330.0, 120.0, 0.0]
surface_normal = [0.0, 0.0, 1.0]
rest_angle_deg = 210.0
sweep_angle_deg = -50.0
eos_angle_fraction = 0.92
shape = {{ type = "StraightCapsule", length = 76.0, base_radius = 11.0, tip_radius = 6.5 }}
staging = {staging}
"#
    )
}

#[test]
fn flipper_stage_hints() {
    use FlipperStageHint::{Lower, Upper};
    let cases: &[(&str, u8, &str, FlipperStageHint)] = &[
        ("f1", 0, r#"{ type = "DedicatedButton", input = "right_flipper" }"#, Lower),
        ("f2", 0, r#"{ type = "DedicatedButton", input = "left_upper" }"#, Upper),
        ("f3", 0, r#"{ type = "DedicatedButton", input = "right_upper" }"#, Upper),
        ("f4", 0, r#"{ type = "ParallelLinked", input = "right_upper" }"#, Upper),
        ("f5", 0, r#"{ type = "ParallelLinked", input = "left_flipper" }"#, Lower),
        ("f6", 2, r#"{ type = "DedicatedButton", input = "left_flipper" }"#, Upper),
        ("f7", 1, r#"{ type = "ParallelLinked", input = "right_flipper" }"#, Upper),
        ("f8", 0, r#"{ type = "DedicatedButton", input = "action" }"#, Lower),
        (
            "f9",
            0,
            r#"{ type = "DualStageThreshold", axis = "left_flipper", lower_threshold = 0.2, upper_threshold = 0.8 }"#,
            Lower,
        ),
    ];
    let extra: String = cases
        .iter()
        .filter(|c| c.0 != "f9" || dual_stage_axis_name_ok())
        .map(|c| flipper(c.0, c.1, c.2))
        .collect::<Vec<_>>()
        .join("\n");
    let t = load(&plus(&extra), BACKGLASS_TOML);
    assert_eq!(t.flipper_stages.len(), t.flippers.len());
    assert_eq!(t.flipper_stages[0], Lower, "Appendix A lower_left");
    for c in cases {
        if let Some(id) = t.ids.flipper(c.0) {
            assert_eq!(
                t.flipper_stages[id.index()],
                c.3,
                "{} (layer {}, staging {})",
                c.0,
                c.1,
                c.2
            );
        } else {
            assert!(c.0 == "f9" && !dual_stage_axis_name_ok(), "{} missing", c.0);
        }
    }
}

/// `CabinetAxis` spelling is pin_core's business; only use the dual-stage case
/// when the fixture text parses there.
fn dual_stage_axis_name_ok() -> bool {
    let extra = flipper(
        "f9",
        0,
        r#"{ type = "DualStageThreshold", axis = "left_flipper", lower_threshold = 0.2, upper_threshold = 0.8 }"#,
    );
    TableManifest::from_toml_str(&plus(&extra)).is_ok()
}

// ============================ error plumbing ================================

#[test]
fn missing_manifests_are_reported() {
    for gone in [T, B] {
        let mut files = bundle();
        files.remove(gone);
        let vfs = vfs_from(&files);
        match load_table(&vfs) {
            Err(AssetError::NotFound(p)) => assert!(p.contains(gone), "{p}"),
            Err(AssetError::Invalid(errs)) => {
                assert!(errs.iter().any(|e| e.file == gone), "{errs:?}")
            }
            other => panic!("{gone} missing: got {:?}", other.map(|_| "Ok")),
        }
    }
}

#[test]
fn toml_syntax_and_schema_errors_name_the_file() {
    let cases: Vec<(String, String, &str)> = vec![
        ("this is = = not toml".into(), BACKGLASS_TOML.into(), T),
        (TABLE_TOML.into(), "[translite\nlayers = 3".into(), B),
        (sub(TABLE_TOML, "schema_version = 1", "schema_version = 2"), BACKGLASS_TOML.into(), T),
        (TABLE_TOML.into(), sub(BACKGLASS_TOML, "schema_version = 1", "schema_version = 7"), B),
        (sub(TABLE_TOML, "players_max = 4", "players_max = 4\nbogus_key = 1"), BACKGLASS_TOML.into(), T),
        // coil numbers must fit u16
        (sub(TABLE_TOML, "id = \"bumper_1_coil\"\nnumber = 2", "id = \"bumper_1_coil\"\nnumber = 70000"), BACKGLASS_TOML.into(), T),
        (sub(TABLE_TOML, "id = \"orbit_magnet\"\nnumber = 1", "id = \"orbit_magnet\"\nnumber = 65536"), BACKGLASS_TOML.into(), T),
        (sub(TABLE_TOML, "id = \"power_chamber\"\nnumber = 1", "id = \"power_chamber\"\nnumber = -1"), BACKGLASS_TOML.into(), T),
        (String::from_utf8_lossy(&[0xFF, 0xFE, 0x00]).into_owned(), BACKGLASS_TOML.into(), T),
    ];
    for (table, backglass, file) in cases {
        let errs = invalid(&bundle_with(&table, &backglass));
        assert!(
            errs.iter().any(|e| e.file == file && !e.message.is_empty()),
            "expected an error for {file}, got {errs:?}"
        );
    }
}

#[test]
fn non_utf8_manifest_is_an_error_not_a_panic() {
    let mut files = bundle();
    files.insert(T.to_owned(), vec![0xFF, 0xFE, 0x80, 0x00, b'a']);
    let vfs = vfs_from(&files);
    assert!(load_table(&vfs).is_err());
}

#[test]
fn all_errors_are_collected_and_sorted_by_file_then_path() {
    let mut table = sub(TABLE_TOML, "coil = \"left_flipper_coil\"\n", "coil = \"no_such_coil\"\n");
    table = sub(&table, r#"eos_switch = "lower_left_eos""#, r#"eos_switch = "no_such_switch""#);
    table = sub(&table, r#"start_scene = "scene_powerball_mania""#, r#"start_scene = "no_such_scene""#);
    table = sub(&table, r#"switch = "bumper_1_ring""#, r#"switch = "no_such_ring""#);
    table = sub(&table, r#"sequence = ["ramp_entry_opto", "ramp_made_opto"]"#, r#"sequence = ["zzz_a", "ramp_made_opto", "zzz_b"]"#);
    let backglass = sub(BACKGLASS_TOML, r#"lamp = "backglass_logo""#, r#"lamp = "no_such_lamp""#);
    let mut files = bundle_with(&table, &backglass);
    files.remove("audio/bumper.ogg");
    files.remove("backglass/base.png");
    let errs = invalid(&files);

    expect_at(&errs, T, &["flippers[0].coil"]);
    expect_at(&errs, T, &["flippers[0].eos_switch"]);
    expect_at(&errs, T, &["modes[0].start_scene"]);
    expect_at(&errs, T, &["mechs[0].switch"]);
    expect_at(&errs, T, &["shots[0].sequence[0]"]);
    expect_at(&errs, T, &["shots[0].sequence[2]"]);
    expect_at(&errs, T, &["sounds[0].file"]);
    expect_at(&errs, B, &["lamp_masks[0].lamp"]);
    expect_at(&errs, B, &["translite.layers[0].file"]);
    assert!(errs.len() >= 9, "{errs:?}");

    let keys: Vec<(&str, &str)> = errs.iter().map(|e| (e.file.as_str(), e.path.as_str())).collect();
    let mut sorted = keys.clone();
    sorted.sort();
    assert_eq!(keys, sorted, "errors must be sorted by (file, path)");
    // No spurious error for the valid reference.
    assert!(!errs.iter().any(|e| e.path == "shots[0].sequence[1]"), "{errs:?}");
}

// ============================ rule: name references =========================

#[test]
fn brief_example_message_for_unknown_flipper_coil() {
    let errs = table_errors(&sub(
        TABLE_TOML,
        r#"coil = "left_flipper_coil"
eos_switch"#,
        r#"coil = "left_flipper_coi"
eos_switch"#,
    ));
    let e = expect_at(&errs, T, &["flippers[0].coil"]);
    assert_eq!(
        e.to_string(),
        r#"table.toml: flippers[0].coil: unknown coil "left_flipper_coi" (did you mean "left_flipper_coil"?)"#
    );
    assert_eq!(errs.len(), 1, "exactly one violation: {errs:?}");
}

/// (mutation from, mutation to, file, path, offending value, expected suggestion)
type RefCase = (&'static str, &'static str, &'static str, &'static str, &'static str, Option<&'static str>);

const TABLE_REF_CASES: &[RefCase] = &[
    (r#"eos_switch = "lower_left_eos""#, r#"eos_switch = "lower_left_eoz""#, T, "flippers[0].eos_switch", "lower_left_eoz", Some("lower_left_eos")),
    (r#"coil = "bumper_1_coil""#, r#"coil = "bumper_coil""#, T, "mechs[0].coil", "bumper_coil", Some("bumper_1_coil")),
    (r#"switch = "bumper_1_ring""#, r#"switch = "bumper_1_rign""#, T, "mechs[0].switch", "bumper_1_rign", Some("bumper_1_ring")),
    (r#"coil = "ramp_diverter_coil""#, r#"coil = "completely_different""#, T, "mechs[1].coil", "completely_different", None),
    (r#"sequence = ["ramp_entry_opto", "ramp_made_opto"]"#, r#"sequence = ["ramp_entry_opto", "ramp_mad_opto"]"#, T, "shots[0].sequence[1]", "ramp_mad_opto", Some("ramp_made_opto")),
    (r#""standard_steel", "standard_steel", "ceramic_powerball"]"#, r#""standard_steel", "standard_steel", "ceramic_powerbal"]"#, T, "dispensers[0].contents[4]", "ceramic_powerbal", Some("ceramic_powerball")),
    (r#"profiles = ["standard_steel", "ceramic_powerball"]"#, r#"profiles = ["standard_steel", "ceramik_powerball"]"#, T, "balls.profiles[1]", "ceramik_powerball", Some("ceramic_powerball")),
    (r#"profiles = ["standard_steel", "ceramic_powerball"]"#, r#"profiles = ["unobtainium", "ceramic_powerball"]"#, T, "balls.profiles[0]", "unobtainium", None),
    (r#"profile = "jumbo_xl_resin""#, r#"profile = "jumbo_xl_rosin""#, T, "balls.restrict[0].profile", "jumbo_xl_rosin", Some("jumbo_xl_resin")),
    (r#"start_scene = "scene_powerball_mania""#, r#"start_scene = "scene_powerball_manic""#, T, "modes[0].start_scene", "scene_powerball_manic", Some("scene_powerball_mania")),
    (r#"map = [ { coil = "left_flipper_coil", type = "S", number = 1 } ]"#, r#"map = [ { coil = "left_flipper_coyl", type = "S", number = 1 } ]"#, T, "dof.map[0].coil", "left_flipper_coyl", Some("left_flipper_coil")),
    (r#"map = [ { coil = "left_flipper_coil", type = "S", number = 1 } ]"#, r#"map = [ { coil = "left_flipper_coil", type = "S", number = 1 }, { switch = "right_orbit_optoo", type = "W", number = 2 } ]"#, T, "dof.map[1].switch", "right_orbit_optoo", Some("right_orbit_opto")),
    (r#"map = [ { coil = "left_flipper_coil", type = "S", number = 1 } ]"#, r#"map = [ { lamp = "overdrive_redy", type = "L", number = 1 } ]"#, T, "dof.map[0].lamp", "overdrive_redy", Some("overdrive_ready")),
    (r#"staging = { type = "DedicatedButton", input = "left_flipper" }"#, r#"staging = { type = "AutoSensorThingFlip", trigger_switch = "right_orbit_opt", delay_ms = 20.0, hold_duration_ms = 150.0 }"#, T, "flippers[0].staging.trigger_switch", "right_orbit_opt", Some("right_orbit_opto")),
];

fn check_ref_case(errs: &[pin_assets::ValidationError], c: &RefCase) {
    let e = expect_at(errs, c.2, &[c.3]);
    assert!(
        e.message.contains(&format!("\"{}\"", c.4)) || e.message.contains(c.4),
        "message must name the offending value {:?}: {e}",
        c.4
    );
    match c.5 {
        Some(s) => assert!(
            e.message.contains("did you mean") && e.message.contains(&format!("\"{s}\"")),
            "expected a (did you mean \"{s}\"?) suggestion: {e}"
        ),
        None => assert!(
            !e.message.contains("did you mean"),
            "no name within edit distance 2, so no suggestion expected: {e}"
        ),
    }
    assert_eq!(
        errs.iter().filter(|x| x.file == c.2 && x.path == c.3).count(),
        1,
        "one violation = one ValidationError: {errs:?}"
    );
}

#[test]
fn unresolved_names_in_table_toml() {
    for c in TABLE_REF_CASES {
        let errs = table_errors(&sub(TABLE_TOML, c.0, c.1));
        check_ref_case(&errs, c);
    }
}

#[test]
fn unresolved_names_in_backglass_toml() {
    let cases: &[RefCase] = &[
        (r#"lamp = "backglass_logo""#, r#"lamp = "backglass_logos""#, B, "lamp_masks[0].lamp", "backglass_logos", Some("backglass_logo")),
        (r#"lamp = "backglass_logo""#, r#"lamp = "right_orbit_opto""#, B, "lamp_masks[0].lamp", "right_orbit_opto", None),
    ];
    for c in cases {
        let errs = backglass_errors(&sub(BACKGLASS_TOML, c.0, c.1));
        check_ref_case(&errs, c);
    }
}

#[test]
fn names_do_not_resolve_across_kinds() {
    // A switch name where a coil is wanted, and vice versa.
    let errs = table_errors(&sub(
        TABLE_TOML,
        r#"coil = "left_flipper_coil"
eos_switch = "lower_left_eos""#,
        r#"coil = "lower_left_eos"
eos_switch = "left_flipper_coil""#,
    ));
    expect_at(&errs, T, &["flippers[0].coil"]);
    expect_at(&errs, T, &["flippers[0].eos_switch"]);
}

// ============================ rule: uniqueness and ranges ===================

#[test]
fn duplicate_ids_per_kind() {
    let cases: &[(&str, &[&str])] = &[
        ("[[switches]]\nid = \"right_orbit_opto\"\nnumber = 99\nkind = \"opto\"", &["switches[5].id", "switches[0].id", "switches[5]"]),
        ("[[coils]]\nid = \"left_flipper_coil\"\nnumber = 99\nkind = \"knocker\"", &["coils[3].id", "coils[0].id", "coils[3]"]),
        ("[[lamps]]\nid = \"overdrive_ready\"\nnumber = 99\nkind = \"gi\"", &["lamps[2].id", "lamps[0].id", "lamps[2]"]),
        ("[[magnets]]\nid = \"orbit_magnet\"\nnumber = 9\nposition = [1.0, 2.0, 0.0]\nk_coil = 2.0e-6\nd_min = 8.0\nrange = 60.0", &["magnets[1].id", "magnets[0].id", "magnets[1]"]),
        ("[[dispensers]]\nid = \"power_chamber\"\nnumber = 9\nexit_position = [1.0, 2.0, 3.0]\nexit_velocity = [0.0, 0.0, 0.0]", &["dispensers[1].id", "dispensers[0].id", "dispensers[1]"]),
        ("[[mechs]]\nid = \"ramp_diverter\"\nkind = \"diverter\"", &["mechs[2].id", "mechs[1].id", "mechs[2]"]),
        ("[[sounds]]\nid = \"sfx_bumper\"\nfile = \"audio/bumper.ogg\"\nbus = \"sfx\"", &["sounds[1].id", "sounds[0].id", "sounds[1]"]),
        ("[[scenes]]\nid = \"scene_powerball_mania\"\nfile = \"display/powerball_mania.toml\"", &["scenes[1].id", "scenes[0].id", "scenes[1]"]),
    ];
    for (extra, paths) in cases {
        let errs = table_errors(&plus(extra));
        let e = expect_at(&errs, T, paths);
        let id = extra.split('"').nth(1).unwrap();
        assert!(e.message.contains(id), "message must name the duplicate id {id:?}: {e}");
    }
    // flippers
    let errs = table_errors(&plus(&flipper(
        "lower_left",
        0,
        r#"{ type = "DedicatedButton", input = "right_flipper" }"#,
    )));
    let e = expect_at(&errs, T, &["flippers[1].id", "flippers[0].id", "flippers[1]"]);
    assert!(e.message.contains("lower_left"), "{e}");
    // persist keys
    let errs = table_errors(&sub(
        TABLE_TOML,
        r#"keys = ["high_score_1", "high_score_2", "games_played"]"#,
        r#"keys = ["high_score_1", "high_score_2", "games_played", "high_score_1"]"#,
    ));
    let e = expect_at(&errs, T, &["persist.keys[3]", "persist.keys[0]"]);
    assert!(e.message.contains("high_score_1"), "{e}");
}

#[test]
fn duplicate_numbers_per_kind() {
    let cases: &[(&str, &[&str], &str)] = &[
        ("[[switches]]\nid = \"extra\"\nnumber = 24\nkind = \"opto\"", &["switches[5].number", "switches[0].number", "switches[5]"], "24"),
        ("[[coils]]\nid = \"extra\"\nnumber = 2\nkind = \"knocker\"", &["coils[3].number", "coils[1].number", "coils[3]"], "2"),
        ("[[lamps]]\nid = \"extra\"\nnumber = 40\nkind = \"gi\"", &["lamps[2].number", "lamps[0].number", "lamps[2]"], "40"),
        ("[[magnets]]\nid = \"extra\"\nnumber = 1\nposition = [1.0, 2.0, 0.0]\nk_coil = 2.0e-6\nd_min = 8.0\nrange = 60.0", &["magnets[1].number", "magnets[0].number", "magnets[1]"], "1"),
        ("[[dispensers]]\nid = \"extra\"\nnumber = 1\nexit_position = [1.0, 2.0, 3.0]\nexit_velocity = [0.0, 0.0, 0.0]", &["dispensers[1].number", "dispensers[0].number", "dispensers[1]"], "1"),
    ];
    for (extra, paths, number) in cases {
        let errs = table_errors(&plus(extra));
        let e = expect_at(&errs, T, paths);
        assert!(e.message.contains(number), "message must name the number {number}: {e}");
    }
}

#[test]
fn same_number_in_different_kinds_is_fine() {
    // coil 1, magnet 1, dispenser 1 and cabinet switch 1 coexist in Appendix A.
    let t = load(
        &plus("[[lamps]]\nid = \"lamp_24\"\nnumber = 24\nkind = \"gi\"\n\n[[coils]]\nid = \"coil_24\"\nnumber = 24\nkind = \"knocker\""),
        BACKGLASS_TOML,
    );
    assert_eq!(t.ids.lamp("lamp_24"), Some(LampId(24)));
    assert_eq!(t.ids.coil("coil_24"), Some(CoilId(24)));
    assert_eq!(t.ids.switch("right_orbit_opto"), Some(SwitchId(24)));
}

#[test]
fn switch_and_lamp_numbers_above_255_are_rejected() {
    let errs = table_errors(&sub(
        TABLE_TOML,
        "id = \"right_orbit_opto\"\nnumber = 24",
        "id = \"right_orbit_opto\"\nnumber = 256",
    ));
    let e = expect_at(&errs, T, &["switches[0].number"]);
    assert!(e.message.contains("256"), "{e}");

    let errs = table_errors(&sub(
        TABLE_TOML,
        "id = \"backglass_logo\"\nnumber = 41",
        "id = \"backglass_logo\"\nnumber = 1000",
    ));
    let e = expect_at(&errs, T, &["lamps[1].number"]);
    assert!(e.message.contains("1000"), "{e}");

    // 255 is the last valid number for both.
    let mut ok = sub(
        TABLE_TOML,
        "id = \"right_orbit_opto\"\nnumber = 24",
        "id = \"right_orbit_opto\"\nnumber = 255",
    );
    ok = sub(&ok, "id = \"backglass_logo\"\nnumber = 41", "id = \"backglass_logo\"\nnumber = 255");
    let t = load(&ok, BACKGLASS_TOML);
    assert_eq!(t.ids.switch("right_orbit_opto"), Some(SwitchId(255)));
    assert_eq!(t.ids.lamp("backglass_logo"), Some(LampId(255)));
}

#[test]
fn coil_magnet_dispenser_numbers_may_use_the_full_u16_range() {
    let mut ok = sub(TABLE_TOML, "id = \"bumper_1_coil\"\nnumber = 2", "id = \"bumper_1_coil\"\nnumber = 65535");
    ok = sub(&ok, "id = \"orbit_magnet\"\nnumber = 1", "id = \"orbit_magnet\"\nnumber = 300");
    ok = sub(&ok, "id = \"power_chamber\"\nnumber = 1", "id = \"power_chamber\"\nnumber = 4000");
    let t = load(&ok, BACKGLASS_TOML);
    assert_eq!(t.ids.coil("bumper_1_coil"), Some(CoilId(65535)));
    assert_eq!(t.ids.magnet("orbit_magnet"), Some(MagnetId(300)));
    assert_eq!(t.ids.dispenser("power_chamber"), Some(DispenserId(4000)));
}

#[test]
fn cabinet_switch_number_above_255_is_rejected() {
    let errs = table_errors(&sub(TABLE_TOML, "tilt = 8", "tilt = 300"));
    let e = expect_at(&errs, T, &["cabinet_switches.tilt"]);
    assert!(e.message.contains("300"), "{e}");
}

// ============================ rule: score display ===========================

#[test]
fn score_display_mode_needs_its_table() {
    let no_dmd = sub(
        BACKGLASS_TOML,
        "dmd = { width = 128, height = 32, color = \"orange\" }\n",
        "",
    );
    let errs = backglass_errors(&no_dmd);
    let e = expect_at(&errs, B, &["score_display.dmd", "score_display.mode", "score_display"]);
    assert!(e.message.contains("dmd"), "{e}");

    let lcd_mode = sub(BACKGLASS_TOML, "mode = \"dmd\"", "mode = \"lcd\"");
    let no_lcd = sub(&lcd_mode, "lcd = { width = 1920, height = 1080 }\n", "");
    let errs = backglass_errors(&no_lcd);
    let e = expect_at(&errs, B, &["score_display.lcd", "score_display.mode", "score_display"]);
    assert!(e.message.contains("lcd"), "{e}");

    // The table of the *other* mode is optional.
    load(TABLE_TOML, &sub(BACKGLASS_TOML, "lcd = { width = 1920, height = 1080 }\n", ""));
    load(TABLE_TOML, &sub(&lcd_mode, "dmd = { width = 128, height = 32, color = \"orange\" }\n", ""));
}

// ============================ rule: referenced files ========================

#[test]
fn every_referenced_file_must_exist() {
    let cases: &[(&str, &str, &str)] = &[
        ("playfield.glb", T, "meta.playfield_mesh"),
        ("rules.lua", T, "meta.script_entry"),
        ("audio/bumper.ogg", T, "sounds[0].file"),
        ("display/powerball_mania.toml", T, "scenes[0].file"),
        ("backglass/base.png", B, "translite.layers[0].file"),
        ("backglass/logo.png", B, "translite.layers[1].file"),
        ("backglass/logo_mask.png", B, "lamp_masks[0].file"),
        ("backbox.glb", B, "backbox.scene"),
    ];
    for (gone, file, path) in cases {
        let mut files = bundle();
        files.remove(*gone).expect("fixture has the file");
        let errs = invalid(&files);
        let e = expect_at(&errs, file, &[path]);
        assert!(e.message.contains(gone), "message must name the missing file {gone:?}: {e}");
        assert_eq!(errs.len(), 1, "removing {gone} is exactly one violation: {errs:?}");
    }
}

#[test]
fn referenced_file_paths_may_not_escape_the_bundle() {
    for bad in ["../rules.lua", "/etc/passwd", "C:\\rules.lua"] {
        let table = sub(
            TABLE_TOML,
            r#"script_entry = "rules.lua""#,
            &format!("script_entry = {bad:?}"),
        );
        let errs = table_errors(&table);
        expect_at(&errs, T, &["meta.script_entry"]);
    }
}

// ============================ rule: splines =================================

fn with_spline(replacement: &str) -> String {
    let start = TABLE_TOML.find("[[splines]]").unwrap();
    let end = TABLE_TOML.find("[[layer_transitions]]").unwrap();
    format!("{}{}\n\n{}", &TABLE_TOML[..start], replacement, &TABLE_TOML[end..])
}

fn guide(points: &str) -> String {
    with_spline(&format!(
        "[[splines]]\nid = \"g\"\nkind = \"guide\"\nlayer = 0\npoints = {points}"
    ))
}

#[test]
fn spline_needs_at_least_two_points() {
    for pts in ["[[60.0, 900.0, 40.0]]", "[]"] {
        let errs = table_errors(&guide(pts));
        expect_at(&errs, T, &["splines[0].points"]);
        let errs = table_errors(&with_spline(&format!(
            "[[splines]]\nid = \"w\"\nkind = \"wireform\"\nlayer = 2\ngauge = 19.0\npoints = {pts}"
        )));
        expect_at(&errs, T, &["splines[0].points"]);
    }
}

#[test]
fn spline_consecutive_points_must_be_distinct() {
    let errs = table_errors(&guide(
        "[[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [100.0, 0.0, 0.0], [200.0, 50.0, 0.0]]",
    ));
    expect_at(&errs, T, &["splines[0].points[2]", "splines[0].points[1]", "splines[0].points"]);

    let errs = table_errors(&with_spline(
        "[[splines]]\nid = \"w\"\nkind = \"wireform\"\nlayer = 2\ngauge = 19.0\npoints = [[60.0, 900.0, 40.0], [60.0, 900.0, 40.0], [150.0, 260.0, 20.0]]\nroll_deg = [0.0, 8.0, 0.0]",
    ));
    expect_at(&errs, T, &["splines[0].points[1]", "splines[0].points[0]", "splines[0].points"]);
}

#[test]
fn wireform_roll_count_must_match_points() {
    for roll in ["[0.0, 8.0]", "[0.0, 8.0, 0.0, 1.0]", "[]"] {
        let table = sub(TABLE_TOML, "roll_deg = [0.0, 8.0, 0.0]", &format!("roll_deg = {roll}"));
        let errs = table_errors(&table);
        expect_at(&errs, T, &["splines[0].roll_deg"]);
    }
}

#[test]
fn wireform_gauge_must_build() {
    for gauge in ["0.0", "-19.0"] {
        let table = sub(TABLE_TOML, "gauge = 19.0", &format!("gauge = {gauge}"));
        let errs = table_errors(&table);
        expect_at(&errs, T, &["splines[0].gauge"]);
    }
}

#[test]
fn valid_spline_shapes_load() {
    // Two-point guide, open guide, wireform without roll.
    load(&guide("[[0.0, 0.0, 0.0], [100.0, 0.0, 0.0]]"), BACKGLASS_TOML);
    load(
        &guide("[[0.0, 0.0, 0.0], [100.0, 10.0, 0.0], [200.0, 0.0, 0.0], [300.0, 40.0, 0.0]]"),
        BACKGLASS_TOML,
    );
    let t = load(&sub(TABLE_TOML, "roll_deg = [0.0, 8.0, 0.0]\n", ""), BACKGLASS_TOML);
    assert_eq!(t.splines.len(), 1);
}

#[test]
fn closed_and_near_closed_guides_load() {
    // Exactly closed loop: must be built periodic, not rejected as ClosedGuideNotPeriodic.
    let t = load(
        &guide("[[100.0, 100.0, 0.0], [200.0, 100.0, 0.0], [200.0, 200.0, 0.0], [100.0, 200.0, 0.0], [100.0, 100.0, 0.0]]"),
        BACKGLASS_TOML,
    );
    assert_eq!(t.splines.len(), 1);
    // End 0.0005 mm (5e-7 m) from the start: snapped closed, not an error.
    let t = load(
        &guide("[[100.0, 100.0, 0.0], [200.0, 100.0, 0.0], [200.0, 200.0, 0.0], [100.0, 200.0, 0.0], [100.0005, 100.0, 0.0]]"),
        BACKGLASS_TOML,
    );
    assert_eq!(t.splines.len(), 1);
    // The same shape snapped and exact must describe the same spline.
    let exact = load(
        &guide("[[100.0, 100.0, 0.0], [200.0, 100.0, 0.0], [200.0, 200.0, 0.0], [100.0, 200.0, 0.0], [100.0, 100.0, 0.0]]"),
        BACKGLASS_TOML,
    );
    assert_eq!(t.splines, exact.splines, "near-closed guide is snapped onto its start point");
    // An end point 1 mm away is simply an open guide.
    load(
        &guide("[[100.0, 100.0, 0.0], [200.0, 100.0, 0.0], [200.0, 200.0, 0.0], [100.0, 200.0, 0.0], [101.0, 100.0, 0.0]]"),
        BACKGLASS_TOML,
    );
}

#[test]
fn uneven_spline_spacing_is_a_warning_not_an_error() {
    // Spacings 10, 90, ~51 mm: longest/shortest = 9 > 3.
    let t = load(
        &guide("[[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [100.0, 0.0, 0.0], [110.0, 50.0, 0.0]]"),
        BACKGLASS_TOML,
    );
    let w: Vec<_> = t.warnings.iter().filter(|w| w.path.starts_with("splines[0]")).collect();
    assert_eq!(w.len(), 1, "one spacing warning expected: {:?}", t.warnings);
    assert_eq!(w[0].file, T);
    assert_eq!(w[0].to_string(), format!("{}: {}: {}", w[0].file, w[0].path, w[0].message));

    // Ratio 2.5: no warning.
    let t = load(
        &guide("[[0.0, 0.0, 0.0], [40.0, 0.0, 0.0], [140.0, 0.0, 0.0], [200.0, 0.0, 0.0]]"),
        BACKGLASS_TOML,
    );
    assert!(
        !t.warnings.iter().any(|w| w.path.starts_with("splines[0]")),
        "ratio 2.5 must not warn: {:?}",
        t.warnings
    );
}

// ============================ rule: layers and zones ========================

#[test]
fn layer_numbers_must_be_0_to_7() {
    let cases: &[(&str, &str, &[&str])] = &[
        ("kind = \"opto\"\nlayer = 0", "kind = \"opto\"\nlayer = 8", &["switches[0].layer"]),
        ("collision_layer = 0", "collision_layer = 9", &["flippers[0].collision_layer"]),
        ("kind = \"wireform\"\nlayer = 2", "kind = \"wireform\"\nlayer = 200", &["splines[0].layer"]),
        ("from_layer = 0", "from_layer = 8", &["layer_transitions[0].from_layer"]),
        ("to_layer = 2", "to_layer = 255", &["layer_transitions[0].to_layer"]),
        ("layers = [0] }", "layers = [0, 8] }", &["balls.restrict[0].layers[1]", "balls.restrict[0].layers"]),
    ];
    for (from, to, paths) in cases {
        let errs = table_errors(&sub(TABLE_TOML, from, to));
        let e = expect_at(&errs, T, paths);
        let bad = to.rsplit(|c: char| !c.is_ascii_digit()).find(|s| !s.is_empty()).unwrap();
        assert!(e.message.contains(bad), "message must name the layer {bad}: {e}");
    }
    // 7 is valid everywhere.
    let mut ok = sub(TABLE_TOML, "kind = \"opto\"\nlayer = 0", "kind = \"opto\"\nlayer = 7");
    ok = sub(&ok, "kind = \"wireform\"\nlayer = 2", "kind = \"wireform\"\nlayer = 7");
    ok = sub(&ok, "to_layer = 2", "to_layer = 7");
    ok = sub(&ok, "layers = [0] }", "layers = [0, 7] }");
    load(&ok, BACKGLASS_TOML);
}

#[test]
fn layer_transition_zone_needs_min_below_max_on_every_axis() {
    let zones = [
        // x reversed, y equal, z reversed
        "zone = { min = [290.0, 640.0, 0.0], max = [240.0, 680.0, 30.0] }",
        "zone = { min = [240.0, 680.0, 0.0], max = [290.0, 680.0, 30.0] }",
        "zone = { min = [240.0, 640.0, 30.0], max = [290.0, 680.0, 0.0] }",
    ];
    for z in zones {
        let table = sub(
            TABLE_TOML,
            "zone = { min = [240.0, 640.0, 0.0], max = [290.0, 680.0, 30.0] }",
            z,
        );
        let errs = table_errors(&table);
        expect_under(&errs, T, "layer_transitions[0].zone");
    }
}
