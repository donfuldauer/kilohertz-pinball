//! Every validation rule has a failing fixture asserting the exact error path
//! (criterion 3).

#[path = "fixtures/support.rs"]
mod support;

use pin_assets::{
    AssetError, FlipperStageHint, LoadedTable, ValidationError, Vfs, ZipLimits, load_table,
};
use support::*;

fn load_files(files: &[(String, Vec<u8>)]) -> Result<LoadedTable, AssetError> {
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    let vfs = Vfs::from_zip_bytes(stored_zip(&refs), ZipLimits::default()).unwrap();
    load_table(&vfs)
}

fn load(table: &str, backglass: &str) -> Result<LoadedTable, AssetError> {
    let files = with_file(&bundle_files(), "table.toml", table.as_bytes());
    let files = with_file(&files, "backglass.toml", backglass.as_bytes());
    load_files(&files)
}

fn errors(table: &str, backglass: &str) -> Vec<ValidationError> {
    match load(table, backglass) {
        Err(AssetError::Invalid(e)) => e,
        other => panic!("expected Invalid, got {other:?}"),
    }
}

/// Edits table.toml (each `(from, to)` must match).
fn table_with(edits: &[(&str, &str)]) -> String {
    let mut t = table_text();
    for (from, to) in edits {
        assert!(t.contains(from), "fixture lacks {from:?}");
        t = t.replacen(from, to, 1);
    }
    t
}

fn bg_with(edits: &[(&str, &str)]) -> String {
    let mut t = backglass_text();
    for (from, to) in edits {
        assert!(t.contains(from), "fixture lacks {from:?}");
        t = t.replacen(from, to, 1);
    }
    t
}

/// Asserts exactly one error, at `file`/`path`, whose message contains `needle`.
fn expect_one(errs: &[ValidationError], file: &str, path: &str, needle: &str) {
    assert_eq!(errs.len(), 1, "{errs:#?}");
    assert_eq!(
        (errs[0].file.as_str(), errs[0].path.as_str()),
        (file, path),
        "{errs:#?}"
    );
    assert!(
        errs[0].message.contains(needle),
        "{:?} lacks {needle:?}",
        errs[0].message
    );
}

fn table_err(edits: &[(&str, &str)], path: &str, needle: &str) {
    let errs = errors(&table_with(edits), &backglass_text());
    expect_one(&errs, "table.toml", path, needle);
}

fn bg_err(edits: &[(&str, &str)], path: &str, needle: &str) {
    let errs = errors(&table_text(), &bg_with(edits));
    expect_one(&errs, "backglass.toml", path, needle);
}

#[test]
fn baseline_fixture_is_valid() {
    load(&table_text(), &backglass_text()).unwrap();
}

#[test]
fn unknown_coil_with_suggestion() {
    let errs = errors(
        &table_with(&[(
            "coil = \"left_flipper_coil\"\neos",
            "coil = \"left_flipper_coi\"\neos",
        )]),
        &backglass_text(),
    );
    assert_eq!(errs.len(), 1);
    assert_eq!(
        errs[0].to_string(),
        "table.toml: flippers[0].coil: unknown coil \"left_flipper_coi\" (did you mean \"left_flipper_coil\"?)"
    );
    // No suggestion beyond edit distance 2.
    table_err(
        &[("coil = \"left_flipper_coil\"\neos", "coil = \"zzz\"\neos")],
        "flippers[0].coil",
        "unknown coil \"zzz\"",
    );
    let errs = errors(
        &table_with(&[("coil = \"left_flipper_coil\"\neos", "coil = \"zzz\"\neos")]),
        &backglass_text(),
    );
    assert!(!errs[0].message.contains("did you mean"));
}

#[test]
fn flipper_references() {
    table_err(
        &[(
            "eos_switch = \"lower_left_eos\"",
            "eos_switch = \"lower_left_eo\"",
        )],
        "flippers[0].eos_switch",
        "unknown switch \"lower_left_eo\" (did you mean \"lower_left_eos\"?)",
    );
    table_err(
        &[(
            "staging = { type = \"DedicatedButton\", input = \"left_flipper\" }",
            "staging = { type = \"AutoSensorThingFlip\", trigger_switch = \"nope\", delay_ms = 10.0, hold_duration_ms = 100.0 }",
        )],
        "flippers[0].staging.trigger_switch",
        "unknown switch \"nope\"",
    );
    table_err(
        &[(
            "collision_layer = 0\nbase_position",
            "collision_layer = 8\nbase_position",
        )],
        "flippers[0].collision_layer",
        "layer 8",
    );
}

#[test]
fn mech_shot_mode_dof_references() {
    table_err(
        &[(
            "coil = \"bumper_1_coil\"\nswitch",
            "coil = \"bumper_2_coil\"\nswitch",
        )],
        "mechs[0].coil",
        "unknown coil \"bumper_2_coil\" (did you mean \"bumper_1_coil\"?)",
    );
    table_err(
        &[("switch = \"bumper_1_ring\"", "switch = \"bumper_1_rin\"")],
        "mechs[0].switch",
        "unknown switch \"bumper_1_rin\"",
    );
    table_err(
        &[("\"ramp_made_opto\"]", "\"ramp_mad_opto\"]")],
        "shots[0].sequence[1]",
        "unknown switch \"ramp_mad_opto\" (did you mean \"ramp_made_opto\"?)",
    );
    table_err(
        &[(
            "sequence = [\"ramp_entry_opto\", \"ramp_made_opto\"]",
            "sequence = []",
        )],
        "shots[0].sequence",
        "empty",
    );
    table_err(
        &[(
            "start_scene = \"scene_powerball_mania\"",
            "start_scene = \"scene_x\"",
        )],
        "modes[0].start_scene",
        "unknown scene \"scene_x\"",
    );
    table_err(
        &[(
            "{ coil = \"left_flipper_coil\", type",
            "{ coil = \"right_flipper_coil\", type",
        )],
        "dof.map[0].coil",
        "unknown coil \"right_flipper_coil\"",
    );
    table_err(
        &[(
            "{ coil = \"left_flipper_coil\", type",
            "{ switch = \"nope\", type",
        )],
        "dof.map[0].switch",
        "unknown switch \"nope\"",
    );
    table_err(
        &[(
            "{ coil = \"left_flipper_coil\", type",
            "{ lamp = \"overdrive_read\", type",
        )],
        "dof.map[0].lamp",
        "unknown lamp \"overdrive_read\" (did you mean \"overdrive_ready\"?)",
    );
}

#[test]
fn profile_references() {
    table_err(
        &[(
            "profiles = [\"standard_steel\", \"ceramic_powerball\"]",
            "profiles = [\"standard_steel\", \"ceramic_powerbal\"]",
        )],
        "balls.profiles[1]",
        "unknown profile \"ceramic_powerbal\" (did you mean \"ceramic_powerball\"?)",
    );
    table_err(
        &[("profile = \"jumbo_xl_resin\"", "profile = \"jumbo\"")],
        "balls.restrict[0].profile",
        "unknown profile \"jumbo\"",
    );
    table_err(
        &[("layers = [0]", "layers = [9]")],
        "balls.restrict[0].layers[0]",
        "layer 9",
    );
    table_err(
        &[(
            "\"standard_steel\", \"ceramic_powerball\"]\n\n",
            "\"standard_steel\", \"glass\"]\n\n",
        )],
        "dispensers[0].contents[4]",
        "unknown profile \"glass\"",
    );
}

#[test]
fn backglass_references() {
    bg_err(
        &[("lamp = \"backglass_logo\"", "lamp = \"backglass_log\"")],
        "lamp_masks[0].lamp",
        "unknown lamp \"backglass_log\" (did you mean \"backglass_logo\"?)",
    );
    bg_err(
        &[("lamp_mask = \"bg_logo\"", "lamp_mask = \"bg_log\"")],
        "translite.layers[1].lamp_mask",
        "unknown lamp mask \"bg_log\" (did you mean \"bg_logo\"?)",
    );
    bg_err(
        &[("dmd = { width = 128, height = 32, color = \"orange\" }", "")],
        "score_display.dmd",
        "dmd",
    );
    bg_err(
        &[
            ("mode = \"dmd\"", "mode = \"lcd\""),
            ("lcd = { width = 1920, height = 1080 }", ""),
        ],
        "score_display.lcd",
        "lcd",
    );
    bg_err(
        &[("collision_layer = 4", "collision_layer = 12")],
        "backbox.collision_layer",
        "layer 12",
    );
    bg_err(
        &[(
            "[[lamp_masks]]\nid = \"bg_logo\"",
            "[[lamp_masks]]\nid = \"bg_logo\"\nlamp = \"overdrive_ready\"\nfile = \"backglass/logo_mask.png\"\n\n[[lamp_masks]]\nid = \"bg_logo\"",
        )],
        "lamp_masks[1].id",
        "duplicate lamp mask id \"bg_logo\"",
    );
}

#[test]
fn duplicate_ids_and_numbers() {
    let extra = |s: &str| format!("{}\n{s}\n", table_text());
    let errs = errors(
        &extra("[[coils]]\nid = \"left_flipper_coil\"\nnumber = 9\nkind = \"sling\""),
        &backglass_text(),
    );
    expect_one(
        &errs,
        "table.toml",
        "coils[2].id",
        "duplicate coil id \"left_flipper_coil\" (first declared at coils[0])",
    );
    let errs = errors(
        &extra("[[coils]]\nid = \"x\"\nnumber = 1\nkind = \"sling\""),
        &backglass_text(),
    );
    expect_one(
        &errs,
        "table.toml",
        "coils[2].number",
        "coil number 1 is already used by coils[0]",
    );
    let errs = errors(
        &extra("[[switches]]\nid = \"x\"\nnumber = 24\nkind = \"opto\""),
        &backglass_text(),
    );
    expect_one(
        &errs,
        "table.toml",
        "switches[5].number",
        "switch number 24",
    );
    let errs = errors(
        &extra("[[switches]]\nid = \"right_orbit_opto\"\nnumber = 99\nkind = \"opto\""),
        &backglass_text(),
    );
    expect_one(&errs, "table.toml", "switches[5].id", "duplicate switch id");
    let errs = errors(
        &extra("[[lamps]]\nid = \"l2\"\nnumber = 40\nkind = \"gi\""),
        &backglass_text(),
    );
    expect_one(&errs, "table.toml", "lamps[2].number", "lamp number 40");
    let errs = errors(
        &extra(
            "[[magnets]]\nid = \"orbit_magnet\"\nnumber = 2\nposition = [0.0, 0.0, 0.0]\nk_coil = 1.0\nd_min = 1.0\nrange = 2.0",
        ),
        &backglass_text(),
    );
    expect_one(&errs, "table.toml", "magnets[1].id", "duplicate magnet id");
    let errs = errors(
        &extra(
            "[[dispensers]]\nid = \"trough\"\nnumber = 1\nexit_position = [0.0, 0.0, 0.0]\nexit_velocity = [0.0, 0.0, 0.0]",
        ),
        &backglass_text(),
    );
    expect_one(
        &errs,
        "table.toml",
        "dispensers[1].number",
        "dispenser number 1",
    );
    let errs = errors(
        &extra("[[sounds]]\nid = \"sfx_bumper\"\nfile = \"audio/bumper.ogg\"\nbus = \"sfx\""),
        &backglass_text(),
    );
    expect_one(&errs, "table.toml", "sounds[1].id", "duplicate sound id");
    let errs = errors(
        &extra("[[mechs]]\nid = \"bumper_1\"\nkind = \"gate\""),
        &backglass_text(),
    );
    expect_one(&errs, "table.toml", "mechs[1].id", "duplicate mech id");
    let errs = errors(
        &table_with(&[("\"games_played\"]", "\"games_played\", \"high_score_1\"]")]),
        &backglass_text(),
    );
    expect_one(
        &errs,
        "table.toml",
        "persist.keys[3]",
        "duplicate persist key",
    );
}

#[test]
fn number_ranges() {
    table_err(
        &[("number = 24", "number = 256")],
        "switches[0].number",
        "switch number 256 exceeds 255",
    );
    table_err(
        &[("number = 40", "number = 300")],
        "lamps[0].number",
        "lamp number 300 exceeds 255",
    );
    table_err(
        &[("tilt = 8", "tilt = 1")],
        "cabinet_switches.tilt",
        "already used by cabinet_switches.left_flipper",
    );
    table_err(
        &[("tilt = 8", "tilt = 999")],
        "cabinet_switches.tilt",
        "999 exceeds 255",
    );
    // A declared switch may share a cabinet number (it *is* that switch).
    let t = format!(
        "{}\n[[switches]]\nid = \"start_button\"\nnumber = 6\nkind = \"leaf\"\n",
        table_text()
    );
    let loaded = load(&t, &backglass_text()).unwrap();
    assert_eq!(loaded.ids.switch("start"), Some(pin_core::SwitchId(6)));
    assert_eq!(
        loaded.ids.switch("start_button"),
        Some(pin_core::SwitchId(6))
    );
    // ...but cabinet names are reserved for cabinet numbers.
    let t = format!(
        "{}\n[[switches]]\nid = \"start\"\nnumber = 60\nkind = \"leaf\"\n",
        table_text()
    );
    expect_one(
        &errors(&t, &backglass_text()),
        "table.toml",
        "switches[5].id",
        "reserved for cabinet switch",
    );
}

#[test]
fn layers() {
    table_err(
        &[("layer = 0\nposition", "layer = 8\nposition")],
        "switches[0].layer",
        "layer 8 is out of range",
    );
    table_err(
        &[("layer = 2\ngauge", "layer = 10\ngauge")],
        "splines[0].layer",
        "layer 10",
    );
    table_err(
        &[("from_layer = 0", "from_layer = 9")],
        "layer_transitions[0].from_layer",
        "layer 9",
    );
    table_err(
        &[("to_layer = 2", "to_layer = 200")],
        "layer_transitions[0].to_layer",
        "layer 200",
    );
    table_err(
        &[("max = [290.0, 680.0, 30.0]", "max = [290.0, 640.0, 0.0]")],
        "layer_transitions[0].zone",
        "fails on y, z",
    );
}

#[test]
fn referenced_files_exist() {
    let base = bundle_files();
    let drop = |name: &str| -> Vec<ValidationError> {
        let files: Vec<_> = base.iter().filter(|(n, _)| n != name).cloned().collect();
        match load_files(&files) {
            Err(AssetError::Invalid(e)) => e,
            other => panic!("{other:?}"),
        }
    };
    for (file, manifest, path) in [
        ("playfield.glb", "table.toml", "meta.playfield_mesh"),
        ("rules.lua", "table.toml", "meta.script_entry"),
        ("audio/bumper.ogg", "table.toml", "sounds[0].file"),
        (
            "display/powerball_mania.toml",
            "table.toml",
            "scenes[0].file",
        ),
        (
            "backglass/base.png",
            "backglass.toml",
            "translite.layers[0].file",
        ),
        (
            "backglass/logo_mask.png",
            "backglass.toml",
            "lamp_masks[0].file",
        ),
        ("backbox.glb", "backglass.toml", "backbox.scene"),
    ] {
        let errs = drop(file);
        expect_one(&errs, manifest, path, &format!("{file:?} does not exist"));
    }
    table_err(
        &[("file = \"audio/bumper.ogg\"", "file = \"../outside.ogg\"")],
        "sounds[0].file",
        "invalid file path \"../outside.ogg\"",
    );
    table_err(
        &[("file = \"audio/bumper.ogg\"", "file = \"\"")],
        "sounds[0].file",
        "empty",
    );
    let errs = errors(
        &table_with(&[(
            "script_entry = \"rules.lua\"",
            "script_entry = \"rules.py\"",
        )]),
        &backglass_text(),
    );
    assert_eq!(errs.len(), 2);
    assert!(errs.iter().all(|e| e.path == "meta.script_entry"));
    assert!(errs.iter().any(|e| e.message.contains(".lua or .wasm")));
}

const WIREFORM: &str = "points = [[60.0, 900.0, 40.0], [40.0, 600.0, 55.0], [150.0, 260.0, 20.0]]";

#[test]
fn splines() {
    table_err(
        &[
            (WIREFORM, "points = [[60.0, 900.0, 40.0]]"),
            ("roll_deg = [0.0, 8.0, 0.0]", ""),
        ],
        "splines[0].points",
        "at least 2 points, got 1",
    );
    table_err(
        &[(
            WIREFORM,
            "points = [[60.0, 900.0, 40.0], [60.0, 900.0, 40.0], [150.0, 260.0, 20.0]]",
        )],
        "splines[0].points[1]",
        "repeats the previous point",
    );
    table_err(
        &[("roll_deg = [0.0, 8.0, 0.0]", "roll_deg = [0.0, 8.0]")],
        "splines[0].roll_deg",
        "2 roll angles given for 3 points",
    );
    table_err(&[("gauge = 19.0", "")], "splines[0].gauge", "needs a gauge");
    table_err(
        &[("gauge = 19.0", "gauge = -1.0")],
        "splines[0].gauge",
        "positive",
    );
}

fn guide(points: &str) -> String {
    table_with(&[
        ("kind = \"wireform\"", "kind = \"guide\""),
        ("gauge = 19.0", ""),
        ("roll_deg = [0.0, 8.0, 0.0]", ""),
        (WIREFORM, points),
    ])
}

#[test]
fn near_closed_guide_is_snapped_closed() {
    // A 100 mm circle through 12 points whose end point is 0.0005 mm
    // (5e-7 m) from the start (spacing 51.8 mm, below the radius: no lint).
    let mut pts: Vec<String> = (0..12)
        .map(|i| {
            let a = f64::from(i) * core::f64::consts::TAU / 12.0;
            format!(
                "[{}, {}, 0.0]",
                100.0 + 100.0 * a.cos(),
                100.0 + 100.0 * a.sin()
            )
        })
        .collect();
    pts.push("[200.0005, 100.0, 0.0]".to_owned());
    let t = guide(&format!("points = [{}]", pts.join(", ")));
    let loaded = load(&t, &backglass_text()).unwrap();
    let s = &loaded.splines[0];
    assert!(s.closed);
    assert_eq!(s.points_m[12], s.points_m[0]);
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    // Exactly closed with too few points.
    let t = guide("points = [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [0.0, 0.0, 0.0]]");
    expect_one(
        &errors(&t, &backglass_text()),
        "table.toml",
        "splines[0].points",
        "at least 4 points",
    );
    // Further than 1e-6 m: open.
    let t = guide(
        "points = [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [100.0, 100.0, 0.0], [0.0, 100.0, 0.0], [0.01, 0.0, 0.0]]",
    );
    assert!(!load(&t, &backglass_text()).unwrap().splines[0].closed);
    // Snapping that creates a repeated point.
    let t = guide(
        "points = [[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [100.0, 100.0, 0.0], [0.0, 0.0, 0.0], [0.0004, 0.0, 0.0]]",
    );
    expect_one(
        &errors(&t, &backglass_text()),
        "table.toml",
        "splines[0].points[4]",
        "repeats",
    );
}

#[test]
fn uneven_spacing_is_a_warning() {
    let t = guide("points = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [200.0, 0.0, 0.0]]");
    let loaded = load(&t, &backglass_text()).unwrap();
    // The spline doubles back on itself (overshoot), so the guide lints also
    // fire at `splines[0]`; the spacing warning is the one at `.points`.
    let spacing: Vec<_> = loaded
        .warnings
        .iter()
        .filter(|w| w.path == "splines[0].points")
        .collect();
    assert_eq!(spacing.len(), 1, "{:?}", loaded.warnings);
    assert!(
        spacing[0].message.contains("ratio 19.0"),
        "{}",
        spacing[0].message
    );
}

#[test]
fn parse_errors_are_reported_per_file() {
    let errs = errors(
        &table_text().replacen("schema_version = 1", "schema_version = 2", 1),
        &backglass_text(),
    );
    expect_one(
        &errs,
        "table.toml",
        "schema_version",
        "unsupported schema_version 2",
    );
    let errs = errors(&table_text(), "schema_version = 1\n[translite\n");
    assert_eq!(errs.len(), 1);
    assert_eq!(
        (errs[0].file.as_str(), errs[0].path.as_str()),
        ("backglass.toml", "")
    );
    let errs = errors(
        &table_with(&[("pitch_deg = 6.5", "pitch_deg = nan")]),
        &backglass_text(),
    );
    expect_one(&errs, "table.toml", "", "meta.pitch_deg");
    // Both files broken: both reported, sorted by file.
    let errs = errors("x = ", "y = ");
    assert_eq!(errs.len(), 2);
    assert_eq!(errs[0].file, "backglass.toml");
    assert_eq!(errs[1].file, "table.toml");
    // A broken table still lets backglass files be checked.
    let files = with_file(&bundle_files(), "table.toml", b"x = ");
    let files: Vec<_> = files
        .into_iter()
        .filter(|(n, _)| n != "backbox.glb")
        .collect();
    match load_files(&files) {
        Err(AssetError::Invalid(e)) => {
            assert_eq!(e.len(), 2);
            assert_eq!(e[0].path, "backbox.scene");
        }
        other => panic!("{other:?}"),
    }
    // Non-UTF-8 text.
    let files = with_file(&bundle_files(), "backglass.toml", &[0xff, 0xfe]);
    match load_files(&files) {
        Err(AssetError::Invalid(e)) => expect_one(&e, "backglass.toml", "", "UTF-8"),
        other => panic!("{other:?}"),
    }
}

#[test]
fn missing_manifest_is_not_found() {
    let files: Vec<_> = bundle_files()
        .into_iter()
        .filter(|(n, _)| n != "table.toml")
        .collect();
    assert_eq!(
        load_files(&files).err(),
        Some(AssetError::NotFound("table.toml".into()))
    );
    let files: Vec<_> = bundle_files()
        .into_iter()
        .filter(|(n, _)| n != "backglass.toml")
        .collect();
    assert_eq!(
        load_files(&files).err(),
        Some(AssetError::NotFound("backglass.toml".into()))
    );
}

#[test]
fn all_errors_collected_and_sorted() {
    let errs = errors(
        &table_with(&[
            (
                "start_scene = \"scene_powerball_mania\"",
                "start_scene = \"nope\"",
            ),
            ("coil = \"left_flipper_coil\"\neos", "coil = \"nope\"\neos"),
            ("number = 24", "number = 999"),
        ]),
        &bg_with(&[("lamp = \"backglass_logo\"", "lamp = \"nope\"")]),
    );
    let got: Vec<(&str, &str)> = errs
        .iter()
        .map(|e| (e.file.as_str(), e.path.as_str()))
        .collect();
    assert_eq!(
        got,
        vec![
            ("backglass.toml", "lamp_masks[0].lamp"),
            ("table.toml", "flippers[0].coil"),
            ("table.toml", "modes[0].start_scene"),
            ("table.toml", "switches[0].number"),
        ]
    );
}

#[test]
fn flipper_stage_hints() {
    let upper_input = table_with(&[("input = \"left_flipper\"", "input = \"right_upper\"")]);
    assert_eq!(
        load(&upper_input, &backglass_text())
            .unwrap()
            .flipper_stages,
        vec![FlipperStageHint::Upper]
    );
    let parallel = table_with(&[(
        "type = \"DedicatedButton\", input = \"left_flipper\"",
        "type = \"ParallelLinked\", input = \"left_upper\"",
    )]);
    assert_eq!(
        load(&parallel, &backglass_text()).unwrap().flipper_stages,
        vec![FlipperStageHint::Upper]
    );
    let layer = table_with(&[(
        "collision_layer = 0\nbase_position",
        "collision_layer = 3\nbase_position",
    )]);
    assert_eq!(
        load(&layer, &backglass_text()).unwrap().flipper_stages,
        vec![FlipperStageHint::Upper]
    );
}

#[test]
fn diverters_and_persist_overflow() {
    let mut t = table_text();
    t.push_str("\n[[mechs]]\nid = \"div_a\"\nkind = \"diverter\"\n\n[[mechs]]\nid = \"gate\"\nkind = \"gate\"\n\n[[mechs]]\nid = \"div_b\"\nkind = \"diverter\"\n");
    let loaded = load(&t, &backglass_text()).unwrap();
    assert_eq!(loaded.ids.diverter("div_a"), Some(pin_core::DiverterId(0)));
    assert_eq!(loaded.ids.diverter("div_b"), Some(pin_core::DiverterId(1)));
    assert_eq!(loaded.mechs[3].diverter, Some(pin_core::DiverterId(1)));
    assert_eq!(loaded.mechs[2].diverter, None);

    let keys: Vec<String> = (0..65).map(|i| format!("\"k{i}\"")).collect();
    let t = table_with(&[(
        "keys = [\"high_score_1\", \"high_score_2\", \"games_played\"]",
        &format!("keys = [{}]", keys.join(", ")),
    )]);
    let loaded = load(&t, &backglass_text()).unwrap();
    assert_eq!(loaded.warnings.len(), 1);
    assert_eq!(loaded.warnings[0].path, "persist.keys");
}
