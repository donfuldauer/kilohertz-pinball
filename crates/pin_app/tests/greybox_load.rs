//! 6.0 C1/C3: the grey-box table builds from `tables/greybox`, passes
//! `pin_app::orchestrator::load_validated` (manifests, 2.8 clearance for
//! every declared ball profile) with no warnings, and declares what the 2.9
//! soak harness measures (`[[shots]]`, `[[modes]]`, unique switch numbers,
//! coil/switch ids on every mech).

mod support;

use pin_app::orchestrator::{load_validated, validate_table};
use pin_core::schema::table::MechKind;
use pin_sim::SimConfig;
use std::collections::BTreeSet;
use support::{greybox_bundle, on_big_stack, scratch};

#[test]
fn greybox_validates_with_no_warnings() {
    let checked = validate_table(greybox_bundle()).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        checked.warnings.is_empty(),
        "warnings: {:#?}",
        checked.warnings
    );
    let m = &checked.table.manifest;
    assert_eq!(m.meta.id, "greybox");
    assert!(m.meta.script_entry.ends_with(".lua"));
    assert!(m.balls.profiles.iter().any(|p| p == "ceramic_powerball"));
}

#[test]
fn greybox_loads_through_load_validated() {
    let dir = greybox_bundle().to_path_buf();
    on_big_stack(move || {
        let sim = load_validated(&dir, SimConfig::default()).unwrap_or_else(|e| panic!("{e}"));
        assert!(!sim.script_faulted(), "{:?}", sim.script_error());
        assert_eq!(sim.balls_in_play(), 0, "attract mode: no ball");
    });
}

#[test]
fn greybox_declares_shots_modes_and_unique_ids() {
    let checked = validate_table(greybox_bundle()).unwrap();
    let m = &checked.table.manifest;
    // Shots: an orbit and a ramp, each a switch sequence within a window.
    let shot = |id: &str| m.shots.iter().find(|s| s.id == id).expect(id);
    for id in ["orbit", "ramp"] {
        let s = shot(id);
        assert!(s.sequence.len() >= 2 && s.window_ms > 0, "{id}: {s:?}");
        for name in &s.sequence {
            assert!(
                m.switches.iter().any(|w| &w.id == name),
                "shot {id} names unknown switch {name}"
            );
        }
    }
    assert!(m.modes.iter().any(|md| md.id == "powerball"));
    for md in &m.modes {
        assert!(
            m.scenes.iter().any(|s| s.id == md.start_scene),
            "mode {} start scene {} not declared",
            md.id,
            md.start_scene
        );
    }
    // Unique switch numbers, cabinet switches included.
    let cab = m.cabinet_switches;
    let mut numbers: Vec<u16> = vec![
        cab.left_flipper,
        cab.right_flipper,
        cab.left_upper,
        cab.right_upper,
        cab.action,
        cab.start,
        cab.launch,
        cab.tilt,
    ];
    numbers.extend(m.switches.iter().map(|s| s.number));
    let unique: BTreeSet<u16> = numbers.iter().copied().collect();
    assert_eq!(unique.len(), numbers.len(), "duplicate switch numbers");
    // Every mech has its switch; every driven mech its coil.
    for mech in &m.mechs {
        assert!(mech.switch.is_some(), "mech {} has no switch", mech.id);
        if mech.kind != MechKind::NewtonBall {
            assert!(mech.coil.is_some(), "mech {} has no coil", mech.id);
        }
    }
    // Contents the brief asks for.
    let kinds = |k: MechKind| m.mechs.iter().filter(|x| x.kind == k).count();
    assert_eq!(kinds(MechKind::Bumper), 3);
    assert_eq!(kinds(MechKind::Sling), 2);
    assert_eq!(kinds(MechKind::Vuk), 1);
    assert_eq!(kinds(MechKind::NewtonBall), 1);
    assert_eq!(kinds(MechKind::Plunger), 1);
    assert_eq!(m.flippers.len(), 3);
    assert_eq!(m.magnets.len(), 1);
    assert!(
        m.switches
            .iter()
            .any(|s| s.kind == pin_core::schema::table::SwitchKind::Eddy)
    );
    assert!(
        m.switches
            .iter()
            .filter(|s| s.kind == pin_core::schema::table::SwitchKind::Trough)
            .count()
            >= 2
    );
    assert_eq!(m.dispensers.len(), 1);
    let feed = &m.dispensers[0];
    assert!(feed.contents.iter().any(|p| p == "standard_steel"));
    assert_eq!(
        feed.contents
            .iter()
            .filter(|p| *p == "ceramic_powerball")
            .count(),
        1
    );
    assert!(
        m.splines
            .iter()
            .any(|s| s.kind == pin_core::schema::table::SplineKind::Wireform)
    );
    assert!(checked.table.transitions.iter().any(|t| t.to_layer == 1));
    assert!(
        checked
            .table
            .transitions
            .iter()
            .any(|t| t.from_layer == 1 && t.to_layer == 0)
    );
}

#[test]
fn greybox_bundle_build_is_deterministic() {
    let a = scratch("greybox_a");
    let b = scratch("greybox_b");
    support::build_greybox_bundle(&a);
    support::build_greybox_bundle(&b);
    for f in ["playfield.glb", "table.toml", "assets.toml", "rules.lua"] {
        assert_eq!(
            std::fs::read(a.join(f)).unwrap(),
            std::fs::read(b.join(f)).unwrap(),
            "{f} differs between builds"
        );
    }
}

#[test]
fn load_validated_names_the_failing_feature() {
    // Narrow the orbit lane below a ball diameter: the clearance check
    // must reject the bundle and name the lane.
    let dir = scratch("greybox_narrow");
    support::build_greybox_bundle(&dir);
    let src = support::greybox_src();
    let layout = std::fs::read_to_string(src.join("layout.toml"))
        .unwrap()
        .replace("width = 40.0", "width = 24.0");
    let base = std::fs::read_to_string(src.join("table.toml")).unwrap();
    let layout = pin_tablegen::layout::parse_layout(&layout).unwrap();
    let base = pin_core::schema::table::TableManifest::from_toml_str(&base).unwrap();
    let generated = pin_tablegen::generate(&layout, &base).unwrap();
    pin_tablegen::write_bundle(&generated, &dir).unwrap();
    let err = match validate_table(&dir) {
        Ok(_) => panic!("narrow lane accepted"),
        Err(e) => e,
    };
    assert!(err.contains("lane"), "{err}");
    assert!(err.contains("standard_steel"), "{err}");
    let dir2 = dir.clone();
    let err = on_big_stack(move || load_validated(&dir2, SimConfig::default()).err());
    assert!(err.is_some_and(|e| e.contains("clearance")));
    // A missing bundle is an error naming the path.
    let missing = dir.join("nope");
    let e = validate_table(&missing).err().unwrap();
    assert!(e.contains("nope"), "{e}");
}

#[test]
fn greybox_bundle_carries_its_layout_and_glass() {
    // The bundle carries layout.toml (pin_sim reads wall_height, the guide
    // height) and glass.toml (the glass underside, 112 mm: 22 mm over the
    // 90 mm walls, DECISIONS "a1-glass-height — grey-box walls to the
    // glass"), exactly as tables/greybox/build.sh copies them.
    let dir = greybox_bundle().to_path_buf();
    let src = support::greybox_src();
    for f in ["layout.toml", "glass.toml"] {
        let bundled = std::fs::read(dir.join(f)).unwrap_or_else(|e| panic!("bundle {f}: {e}"));
        assert_eq!(bundled, std::fs::read(src.join(f)).unwrap(), "{f}");
    }
    let hi_z = on_big_stack(move || {
        let sim = load_validated(&dir, SimConfig::default()).unwrap_or_else(|e| panic!("{e}"));
        sim.escape_bounds().1[2]
    });
    let under = hi_z - pin_sim::load::GLASS_THICKNESS_M - pin_sim::load::BOUNDS_MARGIN_M;
    assert!((under - 0.112).abs() < 1.0e-6, "glass underside {under} m");
}
