//! 6.1 T1: the Apex Circuit table builds from `tables/apex_circuit`, passes
//! `pin_app::orchestrator::load_validated` (manifests, 2.8 clearance for
//! every declared ball profile) with no warnings, and declares what PLAN 6.1
//! and Gate A2 ask for (G1..G7, C1..C3).

mod apex_support;

use apex_support::{SCENES, apex_bundle, apex_src, on_big_stack, persist_key};
use pin_app::orchestrator::{load_validated, validate_table};
use pin_core::schema::table::{MechKind, SplineKind, SwitchKind, TableManifest};
use pin_sim::SimConfig;
use std::collections::BTreeSet;

fn manifest() -> TableManifest {
    let checked = validate_table(apex_bundle()).unwrap_or_else(|e| panic!("{e}"));
    checked.table.manifest.clone()
}

#[test]
fn apex_validates_with_no_warnings() {
    let checked = validate_table(apex_bundle()).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        checked.warnings.is_empty(),
        "warnings: {:#?}",
        checked.warnings
    );
}

#[test]
fn apex_loads_through_load_validated() {
    let dir = apex_bundle().to_path_buf();
    on_big_stack(move || {
        let sim = load_validated(&dir, SimConfig::default()).unwrap_or_else(|e| panic!("{e}"));
        assert!(!sim.script_faulted(), "{:?}", sim.script_error());
        assert_eq!(sim.balls_in_play(), 0, "attract mode: no ball");
        assert_eq!(sim.persist(persist_key(&sim, "game_state")), 0);
    });
}

/// G1: identity, pitch, playfield, backend, profiles, players.
#[test]
fn meta_and_balls() {
    let m = manifest();
    assert_eq!(m.meta.id, "apex_circuit");
    assert_eq!(m.meta.name, "Apex Circuit");
    assert_eq!(m.meta.pitch_deg, 6.5);
    assert_eq!(m.meta.script_entry, "rules.lua");
    assert_eq!(m.meta.players_max, 1);
    assert_eq!(
        m.balls.profiles,
        vec!["standard_steel".to_owned(), "ceramic_powerball".to_owned()]
    );
    let layout = std::fs::read_to_string(apex_src().join("layout.toml")).unwrap();
    let layout = pin_core::schema::LayoutManifest::from_toml_str(&layout).unwrap();
    assert_eq!(layout.playfield.width, 514.0);
    assert_eq!(layout.playfield.length, 1070.0);
}

/// G2: exactly three flippers with these ids; the lower pair are 76 mm
/// zippers sliding towards each other, the upper-left one is 55 mm on the
/// main playfield at 10 o'clock.
#[test]
fn flippers() {
    let m = manifest();
    let ids: Vec<&str> = m.flippers.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, ["left_flipper", "right_flipper", "left_upper"]);
    let len = |f: &pin_core::schema::table::FlipperDef| match f.shape {
        pin_core::schema::table::FlipperShapeDef::StraightCapsule { length, .. } => length,
        _ => panic!("{} is not a straight capsule", f.id),
    };
    let l = &m.flippers[0];
    let r = &m.flippers[1];
    let u = &m.flippers[2];
    assert_eq!(len(l), 76.0);
    assert_eq!(len(r), 76.0);
    assert_eq!(len(u), 55.0);
    let la = l.zipper_slide_axis.expect("left zipper axis");
    let ra = r.zipper_slide_axis.expect("right zipper axis");
    assert!(la[0] > 0.0 && ra[0] < 0.0, "slide towards each other");
    assert_eq!(la[0], -ra[0], "symmetric slide");
    assert!(l.zipper_slide_speed.unwrap() > 0.0 && r.zipper_slide_speed.unwrap() > 0.0);
    assert!(u.zipper_slide_axis.is_none());
    assert_eq!(u.collision_layer, 0);
    // 10 o'clock: left of centre, in the upper half.
    assert!(u.base_position[0] < 257.0 && u.base_position[1] > 535.0);
}

/// G3: the dispenser serves four steel balls and then the Powerball.
#[test]
fn dispenser_contents() {
    let m = manifest();
    assert_eq!(m.dispensers.len(), 1);
    let feed = &m.dispensers[0];
    assert_eq!(feed.id, "feed");
    assert_eq!(
        feed.contents,
        [
            "standard_steel",
            "standard_steel",
            "standard_steel",
            "standard_steel",
            "ceramic_powerball"
        ]
    );
    // Above the plunger in the shooter lane (x 470..514).
    assert!(feed.exit_position[0] > 470.0);
}

/// G4, G5: the mechanisms, the magnet with its detector, and unique ids.
#[test]
fn mechanisms_switches_and_coils() {
    let m = manifest();
    let kinds = |k: MechKind| m.mechs.iter().filter(|x| x.kind == k).count();
    assert_eq!(kinds(MechKind::Bumper), 3);
    assert_eq!(kinds(MechKind::Sling), 2);
    assert_eq!(kinds(MechKind::DropTarget), 3);
    assert_eq!(kinds(MechKind::Spinner), 1);
    assert_eq!(kinds(MechKind::Vuk), 1);
    assert_eq!(kinds(MechKind::Plunger), 1);
    // One drop bank: one reset coil for the three targets.
    let drop_coils: BTreeSet<&str> = m
        .mechs
        .iter()
        .filter(|x| x.kind == MechKind::DropTarget)
        .map(|x| x.coil.as_deref().expect("drop target coil"))
        .collect();
    assert_eq!(drop_coils.len(), 1);
    // The scoop ejects back into play on the playfield layer.
    let scoop = m.mechs.iter().find(|x| x.kind == MechKind::Vuk).unwrap();
    assert_eq!(scoop.id, "lock_scoop");
    assert_eq!(scoop.layer.unwrap_or(0), 0);
    assert_eq!(scoop.exit_layer, Some(0));
    // The spinner's switch is a spinner switch.
    let spinner = m
        .mechs
        .iter()
        .find(|x| x.kind == MechKind::Spinner)
        .unwrap();
    let sw = |n: &str| m.switches.iter().find(|s| s.id == n).expect(n);
    assert_eq!(
        sw(spinner.switch.as_deref().unwrap()).kind,
        SwitchKind::Spinner
    );
    // Every mech has its switch and its coil (the spinner and the Newton
    // ball are passive).
    for mech in &m.mechs {
        let s = mech
            .switch
            .as_deref()
            .unwrap_or_else(|| panic!("{}", mech.id));
        assert!(m.switches.iter().any(|w| w.id == s), "{}: {s}", mech.id);
        if !matches!(mech.kind, MechKind::Spinner | MechKind::NewtonBall) {
            let c = mech
                .coil
                .as_deref()
                .unwrap_or_else(|| panic!("{}", mech.id));
            assert!(m.coils.iter().any(|k| k.id == c), "{}: {c}", mech.id);
        }
    }
    // Magnet with an eddy and an opto switch on the same spot.
    assert_eq!(m.magnets.len(), 1);
    let mag = &m.magnets[0];
    assert_eq!(mag.id, "apex_magnet");
    let eddy = sw("magnet_eddy");
    let opto = sw("magnet_opto");
    assert_eq!(eddy.kind, SwitchKind::Eddy);
    assert_eq!(opto.kind, SwitchKind::Opto);
    assert_eq!(eddy.position, opto.position);
    let p = eddy.position.unwrap();
    let d = ((p[0] - mag.position[0]).powi(2) + (p[1] - mag.position[1]).powi(2)).sqrt();
    assert!(d < mag.range, "detector within the magnet's range");
    // Wireform ramp with a made switch on its layer.
    assert!(m.splines.iter().any(|s| s.kind == SplineKind::Wireform));
    assert_eq!(sw("ramp_made").layer, 2);
    assert_eq!(sw("ramp_entry").layer, 0);
    // Trough, top lanes, tilt.
    let count = |k: SwitchKind| m.switches.iter().filter(|s| s.kind == k).count();
    assert_eq!(count(SwitchKind::Trough), 2);
    for lane in ["lane_1", "lane_2", "lane_3"] {
        assert_eq!(sw(lane).kind, SwitchKind::Rollover);
    }
    // Cabinet switches 1..8 and unique switch numbers overall.
    let cab = m.cabinet_switches;
    let cabinet = [
        cab.left_flipper,
        cab.right_flipper,
        cab.left_upper,
        cab.right_upper,
        cab.action,
        cab.start,
        cab.launch,
        cab.tilt,
    ];
    assert_eq!(cabinet, [1, 2, 3, 4, 5, 6, 7, 8]);
    let mut numbers: Vec<u16> = cabinet.to_vec();
    numbers.extend(m.switches.iter().map(|s| s.number));
    let unique: BTreeSet<u16> = numbers.iter().copied().collect();
    assert_eq!(unique.len(), numbers.len(), "duplicate switch numbers");
    let coil_numbers: BTreeSet<u16> = m.coils.iter().map(|c| c.number).collect();
    assert_eq!(coil_numbers.len(), m.coils.len(), "duplicate coil numbers");
}

/// G6, G7: glass sealing the walls, backglass and the display scenes.
#[test]
fn glass_backglass_and_scenes() {
    let dir = apex_bundle();
    let glass = std::fs::read_to_string(dir.join("glass.toml")).expect("glass.toml in bundle");
    let glass: toml::Table = toml::from_str(&glass).unwrap();
    let height = glass["height"].as_float().unwrap();
    let layout = std::fs::read_to_string(dir.join("layout.toml")).unwrap();
    let layout = pin_core::schema::LayoutManifest::from_toml_str(&layout).unwrap();
    let wall = f64::from(layout.playfield.wall_height);
    assert!(height - wall < 27.0, "walls seal: {height} - {wall}");
    assert!(height >= wall);
    assert!(dir.join("backglass.toml").is_file());
    let m = manifest();
    for s in SCENES {
        assert!(dir.join(format!("display/{s}.toml")).is_file(), "{s}");
        assert!(m.scenes.iter().any(|d| d.id == s), "scene {s} declared");
    }
}

/// C1, C2, C3: shots, modes and persist keys.
#[test]
fn shots_modes_and_persist_keys() {
    let m = manifest();
    let ids: Vec<&str> = m.shots.iter().map(|s| s.id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "left_orbit",
            "right_orbit",
            "cross_ramp",
            "drop_bank",
            "lock_scoop"
        ]
    );
    for s in &m.shots {
        assert!(s.sequence.len() >= 2 && s.window_ms > 0, "{s:?}");
        for name in &s.sequence {
            assert!(
                m.switches.iter().any(|w| &w.id == name),
                "shot {} names unknown switch {name}",
                s.id
            );
        }
    }
    let shot = |id: &str| m.shots.iter().find(|s| s.id == id).unwrap();
    assert_eq!(shot("left_orbit").sequence[0], "left_spinner");
    assert!(
        shot("right_orbit")
            .sequence
            .contains(&"magnet_opto".to_owned())
    );
    assert_eq!(
        shot("drop_bank").sequence,
        ["drop_1_sw", "drop_2_sw", "drop_3_sw"]
    );
    assert!(
        shot("lock_scoop")
            .sequence
            .contains(&"lock_scoop_sw".to_owned())
    );
    let modes: Vec<(&str, &str)> = m
        .modes
        .iter()
        .map(|d| (d.id.as_str(), d.start_scene.as_str()))
        .collect();
    assert_eq!(
        modes,
        [
            ("powerball_mania", "scene_powerball_mania"),
            ("combo_chain", "scene_combo_chain")
        ]
    );
    let keys = &m.persist.as_ref().expect("[persist]").keys;
    for k in [
        "ball",
        "game_state",
        "high_score",
        "games_played",
        "firewall_save_count",
        "apex_jackpot_count",
        "skill_shot_count",
        "mode:powerball_mania",
        "mode:combo_chain",
    ] {
        assert!(keys.iter().any(|x| x == k), "persist key {k}");
    }
}

/// C4 (static half): every declared non-cabinet switch is used by a
/// mechanism, a flipper, a shot or the rules (no decorative dead switch);
/// the soak covers reachability.
#[test]
fn no_dead_switches() {
    let m = manifest();
    let rules = std::fs::read_to_string(apex_src().join("rules.lua")).unwrap();
    for s in &m.switches {
        let used = m.mechs.iter().any(|x| x.switch.as_deref() == Some(&s.id))
            || m.flippers.iter().any(|f| f.eos_switch == s.id)
            || m.shots.iter().any(|x| x.sequence.contains(&s.id))
            || rules.contains(&s.id);
        assert!(used, "switch {} is used by nothing", s.id);
    }
}

/// The bundle carries layout.toml and glass.toml byte-identical to the
/// sources (pin_sim and the soak read them).
#[test]
fn bundle_carries_layout_glass_and_rules() {
    for f in ["layout.toml", "glass.toml", "rules.lua", "backglass.toml"] {
        let a = std::fs::read(apex_src().join(f)).unwrap();
        let b = std::fs::read(apex_bundle().join(f)).unwrap();
        assert_eq!(a, b, "{f}");
    }
    assert!(apex_bundle().join("playfield.glb").is_file());
}
