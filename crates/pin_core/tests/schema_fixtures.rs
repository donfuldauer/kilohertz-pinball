//! Appendix A/C fixtures parse, round-trip, reject unknown keys and wrong
//! schema versions; every parser survives random input.

use pin_core::schema::physics::DEFAULT_PHYSICS_TOML;
use pin_core::schema::table::{
    FlipperShapeDef, FlipperStagingDef, NameResolver, NumberResolver, ScriptBackendKind,
};
use pin_core::schema::{SCHEMA_VERSION, SchemaError};
use pin_core::{
    BackglassManifest, CabinetInput, CoilId, FlipperId, FlipperShape, FlipperStagingMode,
    LayoutManifest, PhysicsDefaults, SwitchId, TableManifest,
};
use proptest::prelude::*;

const TABLE: &str = include_str!("../fixtures/table.toml");
const BACKGLASS: &str = include_str!("../fixtures/backglass.toml");
const LAYOUT: &str = include_str!("../fixtures/layout.toml");
const PHYSICS: &str = DEFAULT_PHYSICS_TOML;

/// Parses every manifest kind from `s`, returning which succeeded.
fn parse_all(s: &str) -> [bool; 4] {
    [
        TableManifest::from_toml_str(s).is_ok(),
        BackglassManifest::from_toml_str(s).is_ok(),
        LayoutManifest::from_toml_str(s).is_ok(),
        PhysicsDefaults::from_toml_str(s).is_ok(),
    ]
}

#[test]
fn fixtures_are_the_appendix_examples_verbatim() {
    let plan = include_str!("../../../docs/PLAN.md");
    for f in [TABLE, BACKGLASS, LAYOUT] {
        assert!(plan.contains(f), "fixture differs from PLAN.md Appendix A");
    }
}

#[test]
fn table_fixture_parses_with_expected_content() {
    let t = TableManifest::from_toml_str(TABLE).unwrap();
    assert_eq!(t.schema_version, SCHEMA_VERSION);
    assert_eq!(t.meta.id, "apex_circuit");
    assert_eq!(t.meta.pitch_deg, 6.5);
    assert_eq!(t.meta.script_backend(), Some(ScriptBackendKind::Lua));
    assert_eq!(t.balls.restrict[0].profile, "jumbo_xl_resin");
    assert_eq!(t.physics, Some(Default::default()));
    assert_eq!(t.cabinet_switches.tilt, 8);
    assert_eq!(
        t.cabinet_switches.switch_id(CabinetInput::RightUpper),
        SwitchId(4)
    );
    for input in CabinetInput::ALL {
        assert_eq!(t.cabinet_switches.number(input), input.index() as u16 + 1);
    }
    assert_eq!(t.switches[0].number, 24);
    assert_eq!(t.magnets[0].k_coil, 2.0e-6);
    let f = &t.flippers[0];
    assert_eq!(
        f.shape,
        FlipperShapeDef::StraightCapsule {
            length: 76.0,
            base_radius: 11.0,
            tip_radius: 6.5
        }
    );
    assert_eq!(
        f.staging,
        FlipperStagingDef::DedicatedButton {
            input: CabinetInput::LeftFlipper
        }
    );
    assert_eq!(t.splines[0].roll_deg.as_deref(), Some(&[0.0, 8.0, 0.0][..]));
    assert_eq!(t.layer_transitions[0].zone.max, [290.0, 680.0, 30.0]);
    assert_eq!(t.dispensers[0].contents.len(), 5);
    assert_eq!(t.shots[0].window_ms, 2500);
    assert_eq!(t.persist.as_ref().unwrap().keys.len(), 3);
    let dof = t.dof.as_ref().unwrap();
    assert_eq!(dof.map[0].coil.as_deref(), Some("left_flipper_coil"));
}

#[test]
fn other_fixtures_parse() {
    let b = BackglassManifest::from_toml_str(BACKGLASS).unwrap();
    assert_eq!(b.translite.layers.len(), 2);
    assert_eq!(b.translite.layers[1].lamp_mask.as_deref(), Some("bg_logo"));
    assert_eq!(b.score_display.dmd.unwrap().width, 128);
    assert_eq!(b.backbox.as_ref().unwrap().collision_layer, 4);
    let l = LayoutManifest::from_toml_str(LAYOUT).unwrap();
    assert_eq!(l.playfield.length, 1070.0);
    assert_eq!(l.decks[0].material, pin_core::Material::Acrylic);
    assert_eq!(l.walls[0].material, pin_core::Material::Wood);
    assert_eq!(l.inserts[0].size, 18.0);
    let p = PhysicsDefaults::from_toml_str(PHYSICS).unwrap();
    assert_eq!(p, PhysicsDefaults::builtin());
    let on_disk = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../physics_defaults.toml"
    ))
    .unwrap();
    assert_eq!(on_disk, PHYSICS);
}

#[test]
fn round_trip_every_schema() {
    fn check<T: PartialEq + std::fmt::Debug>(
        src: &str,
        parse: fn(&str) -> Result<T, SchemaError>,
        ser: fn(&T) -> Result<String, SchemaError>,
    ) {
        let a = parse(src).unwrap();
        let text = ser(&a).unwrap();
        let b = parse(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        assert_eq!(a, b);
        assert_eq!(ser(&b).unwrap(), text, "serialisation is not stable");
    }
    check(
        TABLE,
        TableManifest::from_toml_str,
        TableManifest::to_toml_string,
    );
    check(
        BACKGLASS,
        BackglassManifest::from_toml_str,
        BackglassManifest::to_toml_string,
    );
    check(
        LAYOUT,
        LayoutManifest::from_toml_str,
        LayoutManifest::to_toml_string,
    );
    check(
        PHYSICS,
        PhysicsDefaults::from_toml_str,
        PhysicsDefaults::to_toml_string,
    );
}

#[test]
fn minimal_table_uses_defaults_for_optional_sections() {
    let src = r#"
schema_version = 1
[meta]
id = "t"
name = "T"
author = "a"
version = "0"
script_entry = "rules.wasm"
playfield_mesh = "p.glb"
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
"#;
    let t = TableManifest::from_toml_str(src).unwrap();
    assert_eq!(t.meta.pitch_deg, 6.5);
    assert_eq!(t.meta.script_backend(), Some(ScriptBackendKind::Wasm));
    assert!(t.physics.is_none() && t.dof.is_none() && t.persist.is_none());
    assert!(t.switches.is_empty() && t.balls.restrict.is_empty());
    let back = TableManifest::from_toml_str(&t.to_toml_string().unwrap()).unwrap();
    assert_eq!(back, t);
    let mut odd = t.clone();
    odd.meta.script_entry = "rules.py".into();
    assert_eq!(odd.meta.script_backend(), None);
    odd.meta.script_entry = "rules".into();
    assert_eq!(odd.meta.script_backend(), None);
}

/// Walks every table in `v` (including tables inside arrays) and calls `f` with
/// a copy of the root where an unknown key was inserted into that table.
fn for_each_table_with_unknown_key(root: &toml::Table, mut f: impl FnMut(String, toml::Table)) {
    fn paths(v: &toml::Value, path: Vec<String>, out: &mut Vec<Vec<String>>) {
        match v {
            toml::Value::Table(t) => {
                out.push(path.clone());
                for (k, child) in t {
                    let mut p = path.clone();
                    p.push(k.clone());
                    paths(child, p, out);
                }
            }
            toml::Value::Array(a) => {
                for (i, child) in a.iter().enumerate() {
                    let mut p = path.clone();
                    p.push(format!("#{i}"));
                    paths(child, p, out);
                }
            }
            _ => {}
        }
    }
    fn get_mut<'a>(v: &'a mut toml::Value, path: &[String]) -> &'a mut toml::Value {
        path.iter().fold(v, |v, seg| match seg.strip_prefix('#') {
            Some(i) => &mut v.as_array_mut().unwrap()[i.parse::<usize>().unwrap()],
            None => v.as_table_mut().unwrap().get_mut(seg).unwrap(),
        })
    }
    let root_v = toml::Value::Table(root.clone());
    let mut all = Vec::new();
    paths(&root_v, Vec::new(), &mut all);
    for p in all {
        let mut copy = root_v.clone();
        get_mut(&mut copy, &p)
            .as_table_mut()
            .unwrap()
            .insert("zz_unknown_key".into(), toml::Value::Integer(1));
        let toml::Value::Table(t) = copy else {
            unreachable!()
        };
        f(p.join("."), t);
    }
}

#[test]
fn unknown_keys_are_rejected_everywhere() {
    type Accepts = fn(&str) -> bool;
    let cases: [(&str, Accepts); 4] = [
        (TABLE, |s| TableManifest::from_toml_str(s).is_ok()),
        (BACKGLASS, |s| BackglassManifest::from_toml_str(s).is_ok()),
        (LAYOUT, |s| LayoutManifest::from_toml_str(s).is_ok()),
        (PHYSICS, |s| PhysicsDefaults::from_toml_str(s).is_ok()),
    ];
    for (src, ok) in cases {
        let root: toml::Table = toml::from_str(src).unwrap();
        let mut checked = 0;
        for_each_table_with_unknown_key(&root, |path, t| {
            let text = toml::to_string(&t).unwrap();
            assert!(!ok(&text), "unknown key accepted at `{path}`");
            checked += 1;
        });
        assert!(checked > 3);
        // Sanity: the re-serialised original still parses.
        assert!(ok(&toml::to_string(&root).unwrap()));
    }
}

#[test]
fn wrong_or_missing_schema_version_is_rejected() {
    for src in [TABLE, BACKGLASS, LAYOUT, PHYSICS] {
        for v in [0u32, 2, 99] {
            let s = src.replacen("schema_version = 1", &format!("schema_version = {v}"), 1);
            assert!(s != src);
            let results = [
                TableManifest::from_toml_str(&s).err(),
                BackglassManifest::from_toml_str(&s).err(),
                LayoutManifest::from_toml_str(&s).err(),
                PhysicsDefaults::from_toml_str(&s).err(),
            ];
            assert!(results.iter().all(Option::is_some));
            assert!(
                results.contains(&Some(SchemaError::UnsupportedVersion(v))),
                "{results:?}"
            );
        }
        let s = src.replacen("schema_version = 1", "", 1);
        assert_eq!(parse_all(&s), [false; 4]);
        let s = src.replacen("schema_version = 1", "schema_version = -1", 1);
        assert_eq!(parse_all(&s), [false; 4]);
    }
}

#[test]
fn each_fixture_parses_only_as_its_own_schema() {
    assert_eq!(parse_all(TABLE), [true, false, false, false]);
    assert_eq!(parse_all(BACKGLASS), [false, true, false, false]);
    assert_eq!(parse_all(LAYOUT), [false, false, true, false]);
    assert_eq!(parse_all(PHYSICS), [false, false, false, true]);
}

#[test]
fn invalid_enum_values_and_dof_sources_are_rejected() {
    let bad_kind = TABLE.replacen("kind = \"opto\"", "kind = \"laser\"", 1);
    assert!(TableManifest::from_toml_str(&bad_kind).is_err());
    let bad_shape = TABLE.replacen("\"StraightCapsule\"", "\"Spoon\"", 1);
    assert!(TableManifest::from_toml_str(&bad_shape).is_err());
    let bad_input = TABLE.replacen("input = \"left_flipper\"", "input = \"coin\"", 1);
    assert!(TableManifest::from_toml_str(&bad_input).is_err());
    let two_sources = TABLE.replacen(
        "{ coil = \"left_flipper_coil\", type",
        "{ coil = \"left_flipper_coil\", lamp = \"x\", type",
        1,
    );
    assert!(matches!(
        TableManifest::from_toml_str(&two_sources),
        Err(SchemaError::Invalid(_))
    ));
    let no_source = TABLE.replacen("{ coil = \"left_flipper_coil\", type", "{ type", 1);
    assert!(matches!(
        TableManifest::from_toml_str(&no_source),
        Err(SchemaError::Invalid(_))
    ));
    let bad_mode = BACKGLASS.replacen("mode = \"dmd\"", "mode = \"crt\"", 1);
    assert!(BackglassManifest::from_toml_str(&bad_mode).is_err());
    let bad_material = PHYSICS.replacen("\"resin\"", "\"granite\"", 1);
    assert!(PhysicsDefaults::from_toml_str(&bad_material).is_err());
}

struct Names;

impl NameResolver for Names {
    fn coil(&self, name: &str) -> Option<CoilId> {
        (name == "left_flipper_coil").then_some(CoilId(1))
    }
    fn switch(&self, name: &str) -> Option<SwitchId> {
        match name {
            "lower_left_eos" => Some(SwitchId(30)),
            "thing_opto" => Some(SwitchId(31)),
            _ => None,
        }
    }
}

#[test]
fn flipper_def_converts_to_runtime_config() {
    let t = TableManifest::from_toml_str(TABLE).unwrap();
    let defaults = PhysicsDefaults::builtin();
    let params = defaults.resolve(t.physics.as_ref());
    let def = &t.flippers[0];
    let c = def.to_config(FlipperId(0), &Names, &params).unwrap();
    assert_eq!(c.coil, CoilId(1));
    assert_eq!(c.eos_switch, SwitchId(30));
    assert_eq!(c.base_position_mm, [180.0, 120.0, 0.0]);
    assert_eq!(
        c.shape,
        FlipperShape::StraightCapsule {
            length_mm: 76.0,
            base_radius_mm: 11.0,
            tip_radius_mm: 6.5
        }
    );
    assert_eq!(
        c.staging,
        FlipperStagingMode::DedicatedButton(CabinetInput::LeftFlipper)
    );
    assert_eq!(c.inertia_kg_m2, 1.0e-4);
    assert_eq!(c.power_torque_nm, 1.2);
    assert_eq!(c.hold_torque_nm, 0.15);
    assert_eq!(c.return_spring_torque_nm, 0.03);
    assert_eq!(c.stop_restitution, 0.15);
    assert_eq!(c.rubber_restitution, 0.65);
    assert_eq!(c.rubber_damping, 0.35);
    assert_eq!(c.zipper_slide_axis_mm, None);
    assert_eq!(c.zipper_slide_speed_mm_s, 0.0);
    assert!((c.eos_angle_deg() - (-30.0 + 50.0 * 0.92)).abs() < 1e-5);
    assert_eq!(c.end_angle_deg(), 20.0);
    assert_eq!(c.shape.length_mm(), 76.0);
    // Shape conversion is lossless both ways.
    assert_eq!(FlipperShapeDef::from(c.shape), def.shape);

    // Explicit values override defaults; Thing Flip resolves its switch.
    let mut d2 = def.clone();
    d2.power_torque_nm = Some(2.0);
    d2.rubber_restitution = Some(0.7);
    d2.zipper_slide_axis = Some([0.0, 20.0, 0.0]);
    d2.zipper_slide_speed = Some(400.0);
    d2.shape = FlipperShapeDef::CurvedBanana {
        length: 80.0,
        arc_radius: 120.0,
        arc_sweep_deg: 30.0,
        tube_radius: 8.0,
    };
    d2.staging = FlipperStagingDef::AutoSensorThingFlip {
        trigger_switch: "thing_opto".into(),
        delay_ms: 15.0,
        hold_duration_ms: 200.0,
    };
    let c2 = d2.to_config(FlipperId(3), &Names, &params).unwrap();
    assert_eq!(c2.id, FlipperId(3));
    assert_eq!(c2.power_torque_nm, 2.0);
    assert_eq!(c2.rubber_restitution, 0.7);
    assert_eq!(c2.zipper_slide_axis_mm, Some([0.0, 20.0, 0.0]));
    assert_eq!(c2.zipper_slide_speed_mm_s, 400.0);
    assert_eq!(FlipperShapeDef::from(c2.shape), d2.shape);
    assert_eq!(
        c2.staging,
        FlipperStagingMode::AutoSensorThingFlip {
            trigger_switch: SwitchId(31),
            delay_ms: 15.0,
            hold_duration_ms: 200.0
        }
    );
    for (staging, expected) in [
        (
            FlipperStagingDef::ParallelLinked {
                input: CabinetInput::RightFlipper,
            },
            FlipperStagingMode::ParallelLinked(CabinetInput::RightFlipper),
        ),
        (
            FlipperStagingDef::DualStageThreshold {
                axis: pin_core::CabinetAxis::LeftFlipper,
                lower_threshold: 0.3,
                upper_threshold: 0.8,
            },
            FlipperStagingMode::DualStageThreshold {
                axis: pin_core::CabinetAxis::LeftFlipper,
                lower_threshold: 0.3,
                upper_threshold: 0.8,
            },
        ),
    ] {
        d2.staging = staging;
        assert_eq!(
            d2.to_config(FlipperId(0), &Names, &params).unwrap().staging,
            expected
        );
    }

    // Unknown names are reported.
    d2.staging = FlipperStagingDef::AutoSensorThingFlip {
        trigger_switch: "nope".into(),
        delay_ms: 0.0,
        hold_duration_ms: 0.0,
    };
    assert_eq!(
        d2.to_config(FlipperId(0), &Names, &params),
        Err(SchemaError::UnknownName {
            kind: "switch",
            name: "nope".into()
        })
    );
    let mut d3 = def.clone();
    d3.coil = "ghost".into();
    assert!(matches!(
        d3.to_config(FlipperId(0), &Names, &params),
        Err(SchemaError::UnknownName { kind: "coil", .. })
    ));
    // The fixture's EOS switch is not declared in [[switches]].
    assert!(matches!(
        def.to_config(FlipperId(0), &NumberResolver(&t), &params),
        Err(SchemaError::UnknownName { kind: "switch", .. })
    ));
    assert_eq!(
        NumberResolver(&t).coil("left_flipper_coil"),
        Some(CoilId(1))
    );
    assert_eq!(
        NumberResolver(&t).switch("right_orbit_opto"),
        Some(SwitchId(24))
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn random_text_never_panics(s in "\\PC{0,400}") {
        let _ = parse_all(&s);
    }

    #[test]
    fn random_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(), 0..600)) {
        let s = String::from_utf8_lossy(&bytes);
        let _ = parse_all(&s);
    }

    #[test]
    fn mutated_fixtures_never_panic(which in 0usize..4, pos in any::<prop::sample::Index>(),
                                     len in 0usize..40, insert in "\\PC{0,20}") {
        let src = [TABLE, BACKGLASS, LAYOUT, PHYSICS][which];
        let mut chars: Vec<char> = src.chars().collect();
        let at = pos.index(chars.len());
        let end = (at + len).min(chars.len());
        chars.splice(at..end, insert.chars());
        let s: String = chars.into_iter().collect();
        let _ = parse_all(&s);
    }

    #[test]
    fn truncated_fixtures_never_panic(which in 0usize..4, pos in any::<prop::sample::Index>()) {
        let src = [TABLE, BACKGLASS, LAYOUT, PHYSICS][which];
        let cut = pos.index(src.len() + 1);
        let s = String::from_utf8_lossy(&src.as_bytes()[..cut]);
        let _ = parse_all(&s);
    }

    #[test]
    fn numeric_edits_round_trip(pitch in -90.0f32..90.0, players in any::<u8>(),
                                 x in -1.0e4f32..1.0e4, layer in any::<u8>()) {
        let mut t = TableManifest::from_toml_str(TABLE).unwrap();
        t.meta.pitch_deg = pitch;
        t.meta.players_max = players;
        t.switches[0].position = Some([x, -x, x * 0.5]);
        t.switches[0].layer = layer;
        let back = TableManifest::from_toml_str(&t.to_toml_string().unwrap()).unwrap();
        prop_assert_eq!(back, t);
    }
}

/// Every float literal in a fixture, as (byte offset, length) of the literal.
fn float_literals(src: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let b = src.as_bytes();
    let mut i = 0;
    let mut in_comment = false;
    let mut in_string = false;
    while i < b.len() {
        let c = b[i];
        if in_comment {
            in_comment = c != b'\n';
            i += 1;
            continue;
        }
        if c == b'"' {
            in_string = !in_string;
        }
        if !in_string && c == b'#' {
            in_comment = true;
            i += 1;
            continue;
        }
        let prev_ok = i == 0 || !(b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b'_');
        if !in_string && prev_ok && (c.is_ascii_digit() || c == b'-') {
            let start = i;
            let mut j = i + 1;
            while j < b.len() && (b[j].is_ascii_alphanumeric() || b"+-.".contains(&b[j])) {
                j += 1;
            }
            let lit = &src[start..j];
            if lit.contains('.') && lit.parse::<f64>().is_ok() {
                out.push((start, j - start));
            }
            i = j;
            continue;
        }
        i += 1;
    }
    out
}

#[test]
fn non_finite_floats_are_rejected_with_key_name() {
    let s = TABLE.replacen("pitch_deg = 6.5", "pitch_deg = nan", 1);
    match TableManifest::from_toml_str(&s) {
        Err(SchemaError::Invalid(m)) => assert!(m.contains("meta.pitch_deg"), "{m}"),
        other => panic!("{other:?}"),
    }
    let s = TABLE.replacen("length = 76.0", "length = inf", 1);
    match TableManifest::from_toml_str(&s) {
        Err(SchemaError::Invalid(m)) => assert!(m.contains("flippers[0].shape.length"), "{m}"),
        other => panic!("{other:?}"),
    }
    let s = LAYOUT.replacen("radius = 5.0", "radius = -inf", 1);
    assert!(
        matches!(LayoutManifest::from_toml_str(&s), Err(SchemaError::Invalid(m)) if m.contains("posts[0].radius"))
    );
    let s = BACKGLASS.replacen("fov_deg = 35.0", "fov_deg = +nan", 1);
    assert!(
        matches!(BackglassManifest::from_toml_str(&s), Err(SchemaError::Invalid(m)) if m.contains("backbox.camera.fov_deg"))
    );
    let s = PHYSICS.replacen("restitution = 0.65", "restitution = nan", 1);
    assert!(
        matches!(PhysicsDefaults::from_toml_str(&s), Err(SchemaError::Invalid(m)) if m.contains("material_pairs[0].restitution"))
    );
    // Every float literal of every fixture is checked.
    for src in [TABLE, BACKGLASS, LAYOUT, PHYSICS] {
        let lits = float_literals(src);
        assert!(!lits.is_empty());
        for (at, len) in lits {
            let mut s = src.to_owned();
            s.replace_range(at..at + len, "nan");
            assert!(
                parse_all(&s) == [false; 4],
                "nan accepted at offset {at}: {}",
                &src[at..at + len]
            );
        }
    }
}

proptest! {
    #[test]
    fn any_non_finite_float_is_rejected(which in 0usize..4, pick in any::<prop::sample::Index>(),
                                        bad in prop::sample::select(vec!["nan", "+nan", "-nan", "inf", "+inf", "-inf"])) {
        let src = [TABLE, BACKGLASS, LAYOUT, PHYSICS][which];
        let lits = float_literals(src);
        let (at, len) = lits[pick.index(lits.len())];
        let mut s = src.to_owned();
        s.replace_range(at..at + len, bad);
        prop_assert_eq!(parse_all(&s), [false; 4]);
    }
}
