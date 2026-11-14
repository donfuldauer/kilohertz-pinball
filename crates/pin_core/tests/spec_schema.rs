//! Black-box spec tests for `pin_core::schema` (PLAN.md Appendix A, Subphase 1.1 brief).
//!
//! The Appendix A examples are inlined verbatim from docs/PLAN.md. Because the
//! field names of the Rust types are the implementer's choice, content checks are
//! done black-box: by mutating the TOML text and comparing parsed values.

use pin_core::schema::{BackglassManifest, LayoutManifest, PhysicsDefaults, TableManifest};
use proptest::prelude::*;

const TABLE_TOML: &str = r###"
schema_version = 1

[meta]
id = "apex_circuit"
name = "Apex Circuit"
author = "Reference"
version = "1.0.0"
script_entry = "rules.lua"            # or "rules.wasm"
playfield_mesh = "playfield.glb"
pitch_deg = 6.5
players_max = 4
palette = ["#0b1d3a", "#f2a900", "#e63946"]   # used by the placeholder asset kit

[balls]
profiles = ["standard_steel", "ceramic_powerball"]
restrict = [ { profile = "jumbo_xl_resin", layers = [0] } ]   # optional

[physics]                              # optional per-table overrides of physics_defaults.toml
# flipper_rubber_damping = 0.35

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
id = "right_orbit_opto"
number = 24
kind = "opto"        # opto | rollover | eddy | leaf | target | trough | eos | spinner | ring
layer = 0
position = [410.0, 720.0, 0.0]         # omit when bound through glTF extras
radius = 12.0

[[coils]]
id = "left_flipper_coil"
number = 1
kind = "flipper"     # flipper | bumper | sling | kicker | vuk | drop_reset | diverter | autoplunger | knocker | flasher

[[lamps]]
id = "overdrive_ready"
number = 40
kind = "insert"      # insert | gi | flasher
color = "#ff7a00"
response = "led"     # led | incandescent

[[magnets]]
id = "orbit_magnet"
number = 1
position = [400.0, 700.0, 0.0]
k_coil = 2.0e-6      # N·m³ at full power
d_min = 8.0
range = 60.0
tau_ms = 60.0

[[flippers]]                           # fields of FlipperConfig (§3.3)
id = "lower_left"
coil = "left_flipper_coil"
eos_switch = "lower_left_eos"
collision_layer = 0
base_position = [180.0, 120.0, 0.0]
surface_normal = [0.0, 0.0, 1.0]
rest_angle_deg = -30.0
sweep_angle_deg = 50.0
eos_angle_fraction = 0.92
shape = { type = "StraightCapsule", length = 76.0, base_radius = 11.0, tip_radius = 6.5 }
staging = { type = "DedicatedButton", input = "left_flipper" }
# torque, inertia, restitution and damping default from physics_defaults.toml

[[splines]]
id = "return_wireform"
kind = "wireform"    # wireform | guide
layer = 2
gauge = 19.0         # wireform only
points = [[60.0, 900.0, 40.0], [40.0, 600.0, 55.0], [150.0, 260.0, 20.0]]
roll_deg = [0.0, 8.0, 0.0]

[[layer_transitions]]
id = "ramp_entry"
from_layer = 0
to_layer = 2
zone = { min = [240.0, 640.0, 0.0], max = [290.0, 680.0, 30.0] }

[[dispensers]]
id = "power_chamber"
number = 1
exit_position = [300.0, 880.0, 10.0]
exit_velocity = [0.0, -0.6, 0.0]       # m/s
contents = ["standard_steel", "standard_steel", "standard_steel", "standard_steel", "ceramic_powerball"]

[[mechs]]
id = "bumper_1"
kind = "bumper"      # bumper | sling | drop_target | spinner | vuk | plunger | kickback | diverter | newton_ball | gate
coil = "bumper_1_coil"
switch = "bumper_1_ring"
position = [260.0, 820.0, 0.0]
# kind-specific parameters default from physics_defaults.toml

[[shots]]                              # used by the autoplay harness for coverage
id = "cross_shot_ramp"
sequence = ["ramp_entry_opto", "ramp_made_opto"]
window_ms = 2500

[[modes]]                              # rule modes the soak must see start at least once
id = "powerball_mania"
start_scene = "scene_powerball_mania"

[[sounds]]
id = "sfx_bumper"
file = "audio/bumper.ogg"
bus = "sfx"          # music | callout | sfx

[[scenes]]
id = "scene_powerball_mania"
file = "display/powerball_mania.toml"

[persist]
keys = ["high_score_1", "high_score_2", "games_played"]

[dof]                                  # optional (Subphase 3.5)
rom_name = "apexcircuit"
map = [ { coil = "left_flipper_coil", type = "S", number = 1 } ]
"###;

const BACKGLASS_TOML: &str = r###"
schema_version = 1

[translite]
layers = [
  { file = "backglass/base.png", depth = 0 },
  { file = "backglass/logo.png", depth = 1, lamp_mask = "bg_logo" },
]

[[lamp_masks]]
id = "bg_logo"
lamp = "backglass_logo"
file = "backglass/logo_mask.png"

[score_display]
mode = "dmd"                   # dmd | lcd
dmd = { width = 128, height = 32, color = "orange" }   # orange | red | rgb
lcd = { width = 1920, height = 1080 }

[backbox]                      # optional (Subphase 5.5)
scene = "backbox.glb"
camera = { position = [0.0, -600.0, 300.0], look_at = [0.0, 0.0, 300.0], fov_deg = 35.0 }
collision_layer = 4
"###;

const LAYOUT_TOML: &str = r###"
schema_version = 1

[playfield]
width = 514.0
length = 1070.0
wall_height = 28.0

[[walls]]
points = [[0.0, 0.0], [0.0, 1070.0], [514.0, 1070.0], [514.0, 0.0]]
closed = false
material = "wood"

[[posts]]
position = [120.0, 300.0]
radius = 5.0
rubber = true

[[lanes]]
kind = "inlane"                # inlane | outlane | shooter | orbit
points = [[40.0, 320.0], [60.0, 180.0]]
width = 32.0

[[ramps]]
spline = "return_wireform"     # references table.toml [[splines]]
entry_width = 60.0

[[decks]]
layer = 1
z = 65.0
outline = [[260.0, 700.0], [514.0, 700.0], [514.0, 1070.0], [260.0, 1070.0]]
material = "acrylic"

[[inserts]]
lamp = "overdrive_ready"
position = [257.0, 240.0]
shape = "circle"               # circle | arrow | triangle | rect
size = 18.0
"###;

/// Replaces exactly one occurrence of `from` and fails loudly if the needle is
/// missing or ambiguous, so a typo in a test cannot silently turn into a no-op.
fn replace_once(src: &str, from: &str, to: &str) -> String {
    assert_eq!(
        src.matches(from).count(),
        1,
        "test bug: needle {from:?} must occur exactly once"
    );
    src.replacen(from, to, 1)
}

fn fixture(rel: &str) -> String {
    let path = format!("{}/{}", env!("CARGO_MANIFEST_DIR"), rel);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("cannot read {path}: {e}"))
}

fn table() -> TableManifest {
    TableManifest::from_toml_str(TABLE_TOML).expect("Appendix A.1 example must parse")
}

fn backglass() -> BackglassManifest {
    BackglassManifest::from_toml_str(BACKGLASS_TOML).expect("Appendix A.2 example must parse")
}

fn layout() -> LayoutManifest {
    LayoutManifest::from_toml_str(LAYOUT_TOML).expect("Appendix A.3 example must parse")
}

// ---------------------------------------------------------------- parsing

#[test]
fn appendix_a_examples_parse() {
    let _ = table();
    let _ = backglass();
    let _ = layout();
}

#[test]
fn committed_fixtures_parse_and_match_appendix_a() {
    let t = TableManifest::from_toml_str(&fixture("fixtures/table.toml"))
        .expect("fixtures/table.toml must parse");
    assert_eq!(t, table(), "fixtures/table.toml differs from Appendix A.1");

    let b = BackglassManifest::from_toml_str(&fixture("fixtures/backglass.toml"))
        .expect("fixtures/backglass.toml must parse");
    assert_eq!(
        b,
        backglass(),
        "fixtures/backglass.toml differs from Appendix A.2"
    );

    let l = LayoutManifest::from_toml_str(&fixture("fixtures/layout.toml"))
        .expect("fixtures/layout.toml must parse");
    assert_eq!(
        l,
        layout(),
        "fixtures/layout.toml differs from Appendix A.3"
    );
}

#[test]
fn root_physics_defaults_parses_and_round_trips() {
    let text = fixture("../../physics_defaults.toml");
    let p = PhysicsDefaults::from_toml_str(&text).expect("physics_defaults.toml must parse");
    let again = PhysicsDefaults::from_toml_str(&p.to_toml_string().expect("serialise"))
        .expect("serialised PhysicsDefaults must parse");
    assert_eq!(p, again);
    // Clone + Debug are part of the fixed API.
    let c = p.clone();
    assert_eq!(c, p);
    assert!(!format!("{p:?}").is_empty());
}

#[test]
fn physics_defaults_rejects_unknown_top_level_key() {
    let text = fixture("../../physics_defaults.toml");
    let bad = format!("zz_unknown_spec_key = 1\n{text}");
    assert!(PhysicsDefaults::from_toml_str(&bad).is_err());
}

#[test]
fn physics_defaults_text_carries_appendix_c_material_pairs() {
    // Appendix C values must be present in the committed file. Checked on the
    // TOML text because the Rust field names are not part of the fixed API.
    let text = fixture("../../physics_defaults.toml");
    for needle in ["0.65", "0.78", "0.84", "1.2", "0.15"] {
        assert!(
            text.contains(needle),
            "physics_defaults.toml lacks Appendix C value {needle}"
        );
    }
}

// ---------------------------------------------------------------- round trips

#[test]
fn table_round_trips() {
    let t = table();
    let s = t.to_toml_string().expect("serialise table");
    assert_eq!(TableManifest::from_toml_str(&s).expect("reparse"), t);
    assert!(s.contains("apex_circuit"), "serialised table lost meta.id");
    assert!(s.contains("power_chamber"));
}

#[test]
fn backglass_round_trips() {
    let b = backglass();
    let s = b.to_toml_string().expect("serialise backglass");
    assert_eq!(BackglassManifest::from_toml_str(&s).expect("reparse"), b);
    assert!(s.contains("backglass/logo_mask.png"));
}

#[test]
fn layout_round_trips() {
    let l = layout();
    let s = l.to_toml_string().expect("serialise layout");
    assert_eq!(LayoutManifest::from_toml_str(&s).expect("reparse"), l);
    assert!(s.contains("return_wireform"));
}

#[test]
fn round_trip_is_stable_without_optional_sections() {
    let t = TableManifest::from_toml_str(&table_without_optionals()).expect("parse");
    let s = t.to_toml_string().expect("serialise");
    assert_eq!(TableManifest::from_toml_str(&s).expect("reparse"), t);

    let b = BackglassManifest::from_toml_str(&backglass_without_backbox()).expect("parse");
    let s = b.to_toml_string().expect("serialise");
    assert_eq!(BackglassManifest::from_toml_str(&s).expect("reparse"), b);
}

#[test]
fn manifests_are_clone_and_debug() {
    let t = table();
    assert_eq!(t.clone(), t);
    assert!(format!("{t:?}").contains("apex_circuit"));
    let b = backglass();
    assert_eq!(b.clone(), b);
    assert!(!format!("{b:?}").is_empty());
    let l = layout();
    assert_eq!(l.clone(), l);
    assert!(!format!("{l:?}").is_empty());
}

// ---------------------------------------------------------------- values are really stored

/// Each edit changes one value in the document; the parsed manifest must differ
/// from the original, proving the field is modelled rather than ignored.
#[test]
fn table_value_edits_are_observable() {
    let base = table();
    let edits: &[(&str, &str)] = &[
        ("pitch_deg = 6.5", "pitch_deg = 7.5"),
        ("players_max = 4", "players_max = 2"),
        ("name = \"Apex Circuit\"", "name = \"Other\""),
        ("\"#e63946\"]", "\"#000000\"]"),
        ("layers = [0] }", "layers = [0, 1] }"),
        ("tilt = 8", "tilt = 9"),
        ("number = 24", "number = 25"),
        ("radius = 12.0", "radius = 13.0"),
        (
            "position = [410.0, 720.0, 0.0]",
            "position = [411.0, 720.0, 0.0]",
        ),
        ("color = \"#ff7a00\"", "color = \"#ff7a01\""),
        ("response = \"led\"", "response = \"incandescent\""),
        ("k_coil = 2.0e-6", "k_coil = 3.0e-6"),
        ("d_min = 8.0", "d_min = 9.0"),
        ("range = 60.0", "range = 61.0"),
        ("tau_ms = 60.0", "tau_ms = 61.0"),
        ("rest_angle_deg = -30.0", "rest_angle_deg = -31.0"),
        ("sweep_angle_deg = 50.0", "sweep_angle_deg = -50.0"),
        ("eos_angle_fraction = 0.92", "eos_angle_fraction = 0.9"),
        ("tip_radius = 6.5", "tip_radius = 6.0"),
        ("input = \"left_flipper\"", "input = \"right_flipper\""),
        ("gauge = 19.0", "gauge = 20.0"),
        ("roll_deg = [0.0, 8.0, 0.0]", "roll_deg = [0.0, 9.0, 0.0]"),
        ("to_layer = 2", "to_layer = 3"),
        ("max = [290.0, 680.0, 30.0]", "max = [290.0, 680.0, 31.0]"),
        (
            "exit_velocity = [0.0, -0.6, 0.0]",
            "exit_velocity = [0.0, -0.7, 0.0]",
        ),
        ("switch = \"bumper_1_ring\"", "switch = \"bumper_2_ring\""),
        ("window_ms = 2500", "window_ms = 2501"),
        (
            "start_scene = \"scene_powerball_mania\"",
            "start_scene = \"x\"",
        ),
        ("bus = \"sfx\"", "bus = \"music\""),
        (
            "file = \"display/powerball_mania.toml\"",
            "file = \"display/x.toml\"",
        ),
        ("\"games_played\"]", "\"games_played\", \"extra\"]"),
        ("rom_name = \"apexcircuit\"", "rom_name = \"other\""),
        ("type = \"S\", number = 1", "type = \"S\", number = 2"),
    ];
    for (from, to) in edits {
        let doc = replace_once(TABLE_TOML, from, to);
        let parsed = TableManifest::from_toml_str(&doc)
            .unwrap_or_else(|e| panic!("edit {from:?} -> {to:?} should still parse: {e:?}"));
        assert_ne!(parsed, base, "edit {from:?} -> {to:?} was not observable");
    }
}

#[test]
fn backglass_value_edits_are_observable() {
    let base = backglass();
    let edits: &[(&str, &str)] = &[
        ("depth = 0", "depth = 2"),
        ("lamp_mask = \"bg_logo\"", "lamp_mask = \"bg_other\""),
        ("lamp = \"backglass_logo\"", "lamp = \"other\""),
        ("mode = \"dmd\"", "mode = \"lcd\""),
        ("height = 32", "height = 64"),
        ("color = \"orange\"", "color = \"rgb\""),
        ("width = 1920", "width = 1280"),
        ("scene = \"backbox.glb\"", "scene = \"other.glb\""),
        ("fov_deg = 35.0", "fov_deg = 40.0"),
        ("collision_layer = 4", "collision_layer = 5"),
    ];
    for (from, to) in edits {
        let doc = replace_once(BACKGLASS_TOML, from, to);
        let parsed = BackglassManifest::from_toml_str(&doc)
            .unwrap_or_else(|e| panic!("edit {from:?} -> {to:?} should still parse: {e:?}"));
        assert_ne!(parsed, base, "edit {from:?} -> {to:?} was not observable");
    }
}

#[test]
fn layout_value_edits_are_observable() {
    let base = layout();
    let edits: &[(&str, &str)] = &[
        ("wall_height = 28.0", "wall_height = 30.0"),
        ("length = 1070.0", "length = 1071.0"),
        ("closed = false", "closed = true"),
        ("material = \"wood\"", "material = \"steel\""),
        ("radius = 5.0", "radius = 6.0"),
        ("rubber = true", "rubber = false"),
        ("kind = \"inlane\"", "kind = \"orbit\""),
        ("width = 32.0", "width = 33.0"),
        ("entry_width = 60.0", "entry_width = 61.0"),
        ("z = 65.0", "z = 66.0"),
        ("layer = 1", "layer = 2"),
        ("shape = \"circle\"", "shape = \"arrow\""),
        ("size = 18.0", "size = 19.0"),
    ];
    for (from, to) in edits {
        let doc = replace_once(LAYOUT_TOML, from, to);
        let parsed = LayoutManifest::from_toml_str(&doc)
            .unwrap_or_else(|e| panic!("edit {from:?} -> {to:?} should still parse: {e:?}"));
        assert_ne!(parsed, base, "edit {from:?} -> {to:?} was not observable");
    }
}

// ---------------------------------------------------------------- documented alternatives

#[test]
fn every_documented_enum_alternative_parses() {
    let cases: &[(&str, &str, &[&str])] = &[
        (
            "kind = \"opto\"",
            "kind = \"{}\"",
            &[
                "opto", "rollover", "eddy", "leaf", "target", "trough", "eos", "spinner", "ring",
            ],
        ),
        (
            "kind = \"flipper\"",
            "kind = \"{}\"",
            &[
                "flipper",
                "bumper",
                "sling",
                "kicker",
                "vuk",
                "drop_reset",
                "diverter",
                "autoplunger",
                "knocker",
                "flasher",
            ],
        ),
        (
            "kind = \"insert\"",
            "kind = \"{}\"",
            &["insert", "gi", "flasher"],
        ),
        (
            "response = \"led\"",
            "response = \"{}\"",
            &["led", "incandescent"],
        ),
        (
            "kind = \"bumper\"      #",
            "kind = \"{}\"      #",
            &[
                "bumper",
                "sling",
                "drop_target",
                "spinner",
                "vuk",
                "plunger",
                "kickback",
                "diverter",
                "newton_ball",
                "gate",
            ],
        ),
        (
            "bus = \"sfx\"",
            "bus = \"{}\"",
            &["music", "callout", "sfx"],
        ),
        (
            "script_entry = \"rules.lua\"",
            "script_entry = \"{}\"",
            &["rules.lua", "rules.wasm"],
        ),
    ];
    for (needle, template, values) in cases {
        for v in *values {
            let doc = replace_once(TABLE_TOML, needle, &template.replace("{}", v));
            assert!(
                TableManifest::from_toml_str(&doc).is_ok(),
                "documented value {v:?} for {needle:?} must parse"
            );
        }
    }

    for v in ["dmd", "lcd"] {
        let doc = replace_once(BACKGLASS_TOML, "mode = \"dmd\"", &format!("mode = \"{v}\""));
        assert!(BackglassManifest::from_toml_str(&doc).is_ok(), "mode {v}");
    }
    for v in ["orange", "red", "rgb"] {
        let doc = replace_once(
            BACKGLASS_TOML,
            "color = \"orange\"",
            &format!("color = \"{v}\""),
        );
        assert!(BackglassManifest::from_toml_str(&doc).is_ok(), "color {v}");
    }
    for v in ["inlane", "outlane", "shooter", "orbit"] {
        let doc = replace_once(LAYOUT_TOML, "kind = \"inlane\"", &format!("kind = \"{v}\""));
        assert!(LayoutManifest::from_toml_str(&doc).is_ok(), "lane kind {v}");
    }
    for v in ["circle", "arrow", "triangle", "rect"] {
        let doc = replace_once(
            LAYOUT_TOML,
            "shape = \"circle\"",
            &format!("shape = \"{v}\""),
        );
        assert!(LayoutManifest::from_toml_str(&doc).is_ok(), "shape {v}");
    }
}

#[test]
fn guide_spline_without_gauge_parses() {
    // "gauge = 19.0  # wireform only": a guide spline carries no gauge.
    let doc = replace_once(TABLE_TOML, "kind = \"wireform\"", "kind = \"guide\"");
    let doc = replace_once(&doc, "gauge = 19.0         # wireform only\n", "");
    assert!(TableManifest::from_toml_str(&doc).is_ok());
}

#[test]
fn switch_position_may_be_omitted() {
    // "omit when bound through glTF extras"
    let doc = replace_once(
        TABLE_TOML,
        "position = [410.0, 720.0, 0.0]         # omit when bound through glTF extras\n",
        "",
    );
    assert!(TableManifest::from_toml_str(&doc).is_ok());
}

#[test]
fn undocumented_enum_values_are_rejected() {
    let table_cases: &[(&str, &str)] = &[
        ("kind = \"opto\"", "kind = \"laser\""),
        ("kind = \"flipper\"", "kind = \"warp_coil\""),
        ("kind = \"insert\"", "kind = \"neon\""),
        ("response = \"led\"", "response = \"plasma\""),
        ("kind = \"wireform\"", "kind = \"tunnel\""),
        ("kind = \"bumper\"      #", "kind = \"trampoline\"      #"),
        ("bus = \"sfx\"", "bus = \"ultrasound\""),
    ];
    for (from, to) in table_cases {
        let doc = replace_once(TABLE_TOML, from, to);
        assert!(
            TableManifest::from_toml_str(&doc).is_err(),
            "{to:?} is not a documented value and must be rejected"
        );
    }
    let doc = replace_once(BACKGLASS_TOML, "mode = \"dmd\"", "mode = \"vfd\"");
    assert!(BackglassManifest::from_toml_str(&doc).is_err());
    let doc = replace_once(BACKGLASS_TOML, "color = \"orange\"", "color = \"mauve\"");
    assert!(BackglassManifest::from_toml_str(&doc).is_err());
    let doc = replace_once(LAYOUT_TOML, "kind = \"inlane\"", "kind = \"skyway\"");
    assert!(LayoutManifest::from_toml_str(&doc).is_err());
    let doc = replace_once(LAYOUT_TOML, "shape = \"circle\"", "shape = \"hexagon\"");
    assert!(LayoutManifest::from_toml_str(&doc).is_err());
}

// ---------------------------------------------------------------- optional sections

fn table_without_dof() -> String {
    let start = TABLE_TOML.find("[dof]").expect("[dof] present");
    TABLE_TOML[..start].to_string()
}

fn table_without_optionals() -> String {
    let doc = table_without_dof();
    let doc = replace_once(
        &doc,
        "restrict = [ { profile = \"jumbo_xl_resin\", layers = [0] } ]   # optional\n",
        "",
    );
    let start = doc.find("[physics]").expect("[physics] present");
    let end = doc.find("[cabinet_switches]").expect("[cabinet_switches]");
    format!("{}{}", &doc[..start], &doc[end..])
}

fn backglass_without_backbox() -> String {
    let start = BACKGLASS_TOML.find("[backbox]").expect("[backbox] present");
    BACKGLASS_TOML[..start].to_string()
}

#[test]
fn optional_table_sections_may_be_missing() {
    let full = table();

    let no_dof = TableManifest::from_toml_str(&table_without_dof()).expect("no [dof]");
    assert_ne!(no_dof, full, "[dof] content must be modelled");

    let no_restrict = replace_once(
        TABLE_TOML,
        "restrict = [ { profile = \"jumbo_xl_resin\", layers = [0] } ]   # optional\n",
        "",
    );
    let no_restrict = TableManifest::from_toml_str(&no_restrict).expect("no restrict");
    assert_ne!(no_restrict, full, "restrict content must be modelled");

    let start = TABLE_TOML.find("[physics]").expect("[physics]");
    let end = TABLE_TOML
        .find("[cabinet_switches]")
        .expect("[cabinet_switches]");
    let no_physics = format!("{}{}", &TABLE_TOML[..start], &TABLE_TOML[end..]);
    TableManifest::from_toml_str(&no_physics).expect("no [physics]");

    TableManifest::from_toml_str(&table_without_optionals()).expect("no optional sections");
}

#[test]
fn optional_backbox_section_may_be_missing() {
    let b = BackglassManifest::from_toml_str(&backglass_without_backbox()).expect("no [backbox]");
    assert_ne!(b, backglass(), "[backbox] content must be modelled");
}

// ---------------------------------------------------------------- unknown keys

#[test]
fn table_rejects_unknown_keys_everywhere() {
    let edits: &[(&str, &str)] = &[
        // top level
        (
            "schema_version = 1\n",
            "schema_version = 1\nzz_unknown = 1\n",
        ),
        // nested table
        ("[meta]\n", "[meta]\nzz_unknown = 1\n"),
        ("[balls]\n", "[balls]\nzz_unknown = 1\n"),
        (
            "[cabinet_switches]\n",
            "[cabinet_switches]\nzz_unknown = 1\n",
        ),
        ("[persist]\n", "[persist]\nzz_unknown = 1\n"),
        (
            "rom_name = \"apexcircuit\"\n",
            "rom_name = \"apexcircuit\"\nzz_unknown = 1\n",
        ),
        // array-of-tables elements
        (
            "id = \"right_orbit_opto\"\n",
            "id = \"right_orbit_opto\"\nzz_unknown = 1\n",
        ),
        (
            "id = \"left_flipper_coil\"\n",
            "id = \"left_flipper_coil\"\nzz_unknown = 1\n",
        ),
        (
            "id = \"overdrive_ready\"\n",
            "id = \"overdrive_ready\"\nzz_unknown = 1\n",
        ),
        (
            "id = \"orbit_magnet\"\n",
            "id = \"orbit_magnet\"\nzz_unknown = 1\n",
        ),
        (
            "id = \"lower_left\"\n",
            "id = \"lower_left\"\nzz_unknown = 1\n",
        ),
        (
            "id = \"return_wireform\"\n",
            "id = \"return_wireform\"\nzz_unknown = 1\n",
        ),
        (
            "id = \"ramp_entry\"\n",
            "id = \"ramp_entry\"\nzz_unknown = 1\n",
        ),
        (
            "id = \"power_chamber\"\n",
            "id = \"power_chamber\"\nzz_unknown = 1\n",
        ),
        ("id = \"bumper_1\"\n", "id = \"bumper_1\"\nzz_unknown = 1\n"),
        (
            "id = \"cross_shot_ramp\"\n",
            "id = \"cross_shot_ramp\"\nzz_unknown = 1\n",
        ),
        (
            "id = \"powerball_mania\"\n",
            "id = \"powerball_mania\"\nzz_unknown = 1\n",
        ),
        (
            "id = \"sfx_bumper\"\n",
            "id = \"sfx_bumper\"\nzz_unknown = 1\n",
        ),
        (
            "id = \"scene_powerball_mania\"\n",
            "id = \"scene_powerball_mania\"\nzz_unknown = 1\n",
        ),
        // inline tables
        ("layers = [0] }", "layers = [0], zz_unknown = 1 }"),
        ("tip_radius = 6.5 }", "tip_radius = 6.5, zz_unknown = 1 }"),
        (
            "input = \"left_flipper\" }",
            "input = \"left_flipper\", zz_unknown = 1 }",
        ),
        (
            "max = [290.0, 680.0, 30.0] }",
            "max = [290.0, 680.0, 30.0], zz_unknown = 1 }",
        ),
        (
            "type = \"S\", number = 1 }",
            "type = \"S\", number = 1, zz_unknown = 1 }",
        ),
    ];
    for (from, to) in edits {
        let doc = replace_once(TABLE_TOML, from, to);
        assert!(
            TableManifest::from_toml_str(&doc).is_err(),
            "unknown key injected at {from:?} must be a validation error"
        );
    }
    // An unknown top-level table and array-of-tables.
    let doc = format!("{TABLE_TOML}\n[zz_unknown_section]\na = 1\n");
    assert!(TableManifest::from_toml_str(&doc).is_err());
    let doc = format!("{TABLE_TOML}\n[[zz_unknown_list]]\na = 1\n");
    assert!(TableManifest::from_toml_str(&doc).is_err());
}

#[test]
fn backglass_rejects_unknown_keys_everywhere() {
    let edits: &[(&str, &str)] = &[
        (
            "schema_version = 1\n",
            "schema_version = 1\nzz_unknown = 1\n",
        ),
        ("[translite]\n", "[translite]\nzz_unknown = 1\n"),
        ("depth = 0 }", "depth = 0, zz_unknown = 1 }"),
        ("id = \"bg_logo\"\n", "id = \"bg_logo\"\nzz_unknown = 1\n"),
        ("[score_display]\n", "[score_display]\nzz_unknown = 1\n"),
        (
            "color = \"orange\" }",
            "color = \"orange\", zz_unknown = 1 }",
        ),
        ("height = 1080 }", "height = 1080, zz_unknown = 1 }"),
        (
            "scene = \"backbox.glb\"\n",
            "scene = \"backbox.glb\"\nzz_unknown = 1\n",
        ),
        ("fov_deg = 35.0 }", "fov_deg = 35.0, zz_unknown = 1 }"),
    ];
    for (from, to) in edits {
        let doc = replace_once(BACKGLASS_TOML, from, to);
        assert!(
            BackglassManifest::from_toml_str(&doc).is_err(),
            "unknown key injected at {from:?} must be a validation error"
        );
    }
    let doc = format!("{BACKGLASS_TOML}\n[zz_unknown_section]\na = 1\n");
    assert!(BackglassManifest::from_toml_str(&doc).is_err());
}

#[test]
fn layout_rejects_unknown_keys_everywhere() {
    let edits: &[(&str, &str)] = &[
        (
            "schema_version = 1\n",
            "schema_version = 1\nzz_unknown = 1\n",
        ),
        ("[playfield]\n", "[playfield]\nzz_unknown = 1\n"),
        ("closed = false\n", "closed = false\nzz_unknown = 1\n"),
        ("rubber = true\n", "rubber = true\nzz_unknown = 1\n"),
        ("width = 32.0\n", "width = 32.0\nzz_unknown = 1\n"),
        (
            "entry_width = 60.0\n",
            "entry_width = 60.0\nzz_unknown = 1\n",
        ),
        ("z = 65.0\n", "z = 65.0\nzz_unknown = 1\n"),
        ("size = 18.0\n", "size = 18.0\nzz_unknown = 1\n"),
    ];
    for (from, to) in edits {
        let doc = replace_once(LAYOUT_TOML, from, to);
        assert!(
            LayoutManifest::from_toml_str(&doc).is_err(),
            "unknown key injected at {from:?} must be a validation error"
        );
    }
    let doc = format!("{LAYOUT_TOML}\n[zz_unknown_section]\na = 1\n");
    assert!(LayoutManifest::from_toml_str(&doc).is_err());
}

// ---------------------------------------------------------------- schema_version

#[test]
fn wrong_schema_version_is_rejected() {
    for bad in ["0", "2", "3", "65535"] {
        let to = format!("schema_version = {bad}\n");
        let doc = replace_once(TABLE_TOML, "schema_version = 1\n", &to);
        assert!(TableManifest::from_toml_str(&doc).is_err(), "table v{bad}");
        let doc = replace_once(BACKGLASS_TOML, "schema_version = 1\n", &to);
        assert!(
            BackglassManifest::from_toml_str(&doc).is_err(),
            "backglass v{bad}"
        );
        let doc = replace_once(LAYOUT_TOML, "schema_version = 1\n", &to);
        assert!(
            LayoutManifest::from_toml_str(&doc).is_err(),
            "layout v{bad}"
        );
    }
}

#[test]
fn mistyped_schema_version_is_rejected() {
    let doc = replace_once(
        TABLE_TOML,
        "schema_version = 1\n",
        "schema_version = \"1\"\n",
    );
    assert!(TableManifest::from_toml_str(&doc).is_err());
    let doc = replace_once(
        BACKGLASS_TOML,
        "schema_version = 1\n",
        "schema_version = -1\n",
    );
    assert!(BackglassManifest::from_toml_str(&doc).is_err());
    let doc = replace_once(
        LAYOUT_TOML,
        "schema_version = 1\n",
        "schema_version = 1.5\n",
    );
    assert!(LayoutManifest::from_toml_str(&doc).is_err());
}

// ---------------------------------------------------------------- malformed input

#[test]
fn wrong_value_types_are_rejected() {
    let doc = replace_once(TABLE_TOML, "pitch_deg = 6.5", "pitch_deg = \"steep\"");
    assert!(TableManifest::from_toml_str(&doc).is_err());
    let doc = replace_once(TABLE_TOML, "number = 24", "number = \"twenty-four\"");
    assert!(TableManifest::from_toml_str(&doc).is_err());
    let doc = replace_once(BACKGLASS_TOML, "height = 32", "height = \"tall\"");
    assert!(BackglassManifest::from_toml_str(&doc).is_err());
    let doc = replace_once(LAYOUT_TOML, "closed = false", "closed = 3");
    assert!(LayoutManifest::from_toml_str(&doc).is_err());
}

#[test]
fn syntactically_invalid_toml_is_an_error_not_a_panic() {
    for bad in [
        "",
        "=",
        "[",
        "[[switches]",
        "schema_version = ",
        "\u{0}",
        "a = [1, ",
    ] {
        assert!(TableManifest::from_toml_str(bad).is_err(), "{bad:?}");
        assert!(BackglassManifest::from_toml_str(bad).is_err(), "{bad:?}");
        assert!(LayoutManifest::from_toml_str(bad).is_err(), "{bad:?}");
    }
    for bad in ["=", "[", "a = [1, "] {
        assert!(PhysicsDefaults::from_toml_str(bad).is_err(), "{bad:?}");
    }
}

fn drop_line(src: &str, index: usize) -> String {
    let lines: Vec<&str> = src.lines().collect();
    let skip = index % lines.len();
    let mut out = String::with_capacity(src.len());
    for (i, line) in lines.iter().enumerate() {
        if i != skip {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

fn splice(src: &str, at: usize, junk: &str) -> String {
    let chars: Vec<char> = src.chars().collect();
    let at = at % (chars.len() + 1);
    let mut out: String = chars[..at].iter().collect();
    out.push_str(junk);
    out.extend(chars[at..].iter());
    out
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn parsers_never_panic_on_arbitrary_text(s in ".{0,400}") {
        let _ = TableManifest::from_toml_str(&s);
        let _ = BackglassManifest::from_toml_str(&s);
        let _ = LayoutManifest::from_toml_str(&s);
        let _ = PhysicsDefaults::from_toml_str(&s);
    }

    #[test]
    fn parsers_never_panic_on_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        let s = String::from_utf8_lossy(&bytes);
        let _ = TableManifest::from_toml_str(&s);
        let _ = BackglassManifest::from_toml_str(&s);
        let _ = LayoutManifest::from_toml_str(&s);
        let _ = PhysicsDefaults::from_toml_str(&s);
    }

    #[test]
    fn parsers_never_panic_on_toml_shaped_text(
        s in "(\\[{1,2}[a-z_]{1,12}\\]{1,2}\n|[a-z_]{1,14} = (-?[0-9]{1,6}|\"[a-z#.0-9]{0,8}\"|true|\\[[0-9., ]{0,12}\\]|\\{ [a-z]{1,5} = [0-9]{1,3} \\})\n){0,24}"
    ) {
        let _ = TableManifest::from_toml_str(&s);
        let _ = BackglassManifest::from_toml_str(&s);
        let _ = LayoutManifest::from_toml_str(&s);
        let _ = PhysicsDefaults::from_toml_str(&s);
    }

    #[test]
    fn parsers_never_panic_on_mutated_examples(
        line in any::<usize>(),
        at in any::<usize>(),
        junk in "[ -~\n]{0,6}",
    ) {
        for src in [TABLE_TOML, BACKGLASS_TOML, LAYOUT_TOML] {
            for doc in [drop_line(src, line), splice(src, at, &junk)] {
                let _ = TableManifest::from_toml_str(&doc);
                let _ = BackglassManifest::from_toml_str(&doc);
                let _ = LayoutManifest::from_toml_str(&doc);
                let _ = PhysicsDefaults::from_toml_str(&doc);
            }
        }
    }

    /// Any manifest that parses from a mutated example must survive a round trip.
    #[test]
    fn whatever_parses_round_trips(line in any::<usize>()) {
        if let Ok(t) = TableManifest::from_toml_str(&drop_line(TABLE_TOML, line)) {
            let s = t.to_toml_string().expect("serialise");
            prop_assert_eq!(TableManifest::from_toml_str(&s).expect("reparse"), t);
        }
        if let Ok(b) = BackglassManifest::from_toml_str(&drop_line(BACKGLASS_TOML, line)) {
            let s = b.to_toml_string().expect("serialise");
            prop_assert_eq!(BackglassManifest::from_toml_str(&s).expect("reparse"), b);
        }
        if let Ok(l) = LayoutManifest::from_toml_str(&drop_line(LAYOUT_TOML, line)) {
            let s = l.to_toml_string().expect("serialise");
            prop_assert_eq!(LayoutManifest::from_toml_str(&s).expect("reparse"), l);
        }
    }
}
