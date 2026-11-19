//! 6.2 T1: the Sky Deck table builds from `tables/sky_deck`, passes
//! `pin_app::orchestrator::load_validated` (manifests, 2.8 clearance for
//! every declared ball profile, `Simulation::load`) with no warnings, and
//! declares the structure of the brief (R1, R2, R4, R5, R9): flippers,
//! mechs, switches, dispensers, shots, the mode and the persist keys.

mod sky_deck_support;

use pin_app::orchestrator::{load_validated, validate_table};
use pin_core::CabinetInput;
use pin_core::schema::table::{FlipperShapeDef, FlipperStagingDef, MechKind, SwitchKind};
use pin_sim::SimConfig;
use sky_deck_support::{on_big_stack, scratch, sky_deck_bundle, sky_deck_src};
use std::collections::BTreeSet;
use std::process::Command;

#[test]
fn sky_deck_validates_with_no_warnings() {
    let checked = validate_table(sky_deck_bundle()).unwrap_or_else(|e| panic!("{e}"));
    assert!(
        checked.warnings.is_empty(),
        "warnings: {:#?}",
        checked.warnings
    );
    let m = &checked.table.manifest;
    assert_eq!(m.meta.id, "sky_deck");
    assert_eq!(m.meta.name, "Sky Deck");
    assert_eq!(m.meta.script_entry, "rules.lua");
    assert_eq!(m.meta.playfield_mesh, "playfield.glb");
    assert_eq!(m.meta.pitch_deg, 6.5);
    assert_eq!(m.meta.players_max, 1);
    assert!(!m.meta.palette.is_empty());
    assert_eq!(m.balls.profiles, ["standard_steel", "uv_globall"]);
}

#[test]
fn sky_deck_loads_through_load_validated() {
    let dir = sky_deck_bundle().to_path_buf();
    on_big_stack(move || {
        let sim = load_validated(&dir, SimConfig::default()).unwrap_or_else(|e| panic!("{e}"));
        assert!(!sim.script_faulted(), "{:?}", sim.script_error());
        assert_eq!(sim.balls_in_play(), 0, "attract mode: no ball");
        assert_eq!(sim.snapshot().gi, [1.0, 1.0, 1.0, 1.0], "default GI");
    });
}

#[test]
fn sky_deck_flippers_decks_and_layers() {
    let checked = validate_table(sky_deck_bundle()).unwrap();
    let m = &checked.table.manifest;
    let ids: Vec<&str> = m.flippers.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(ids, ["lower_left", "lower_right", "upper_right"]);
    for f in &m.flippers {
        assert!(
            m.switches
                .iter()
                .any(|s| s.id == f.eos_switch && s.kind == SwitchKind::Eos),
            "{} eos switch {}",
            f.id,
            f.eos_switch
        );
        let FlipperShapeDef::StraightCapsule { length, .. } = f.shape else {
            panic!("{} is not a StraightCapsule", f.id)
        };
        let (want_len, want_layer, want_z, want_input) = match f.id.as_str() {
            "lower_left" => (76.0, 0, 0.0, CabinetInput::LeftFlipper),
            "lower_right" => (76.0, 0, 0.0, CabinetInput::RightFlipper),
            _ => (52.0, 1, 65.0, CabinetInput::RightUpper),
        };
        assert_eq!(length, want_len, "{}", f.id);
        assert_eq!(f.collision_layer, want_layer, "{}", f.id);
        assert_eq!(f.base_position[2], want_z, "{}", f.id);
        let FlipperStagingDef::DedicatedButton { input } = f.staging else {
            panic!("{} staging", f.id)
        };
        assert_eq!(input, want_input, "{}", f.id);
    }
    // The upper deck: layer 1 at 65 mm, clear acrylic (layout.toml).
    let layout = std::fs::read_to_string(sky_deck_src().join("layout.toml")).unwrap();
    let layout = pin_tablegen::layout::parse_layout(&layout).unwrap();
    assert!(
        layout
            .decks
            .iter()
            .any(|d| d.layer == 1 && d.z == 65.0 && d.material == pin_core::Material::Acrylic)
    );
    // Layer changes: up onto the deck (VUK, bridge) and back (return
    // funnel, safety exits).
    let t = &checked.table.transitions;
    assert!(t.iter().any(|z| z.from_layer == 2 && z.to_layer == 1));
    let back: Vec<&str> = t
        .iter()
        .filter(|z| z.from_layer == 1 && z.to_layer == 0)
        .map(|z| z.name.as_str())
        .collect();
    for id in [
        "deck_return",
        "deck_exit_left",
        "deck_exit_right",
        "deck_exit_top",
    ] {
        assert!(back.contains(&id), "{id} missing: {back:?}");
    }
    assert!(
        m.splines
            .iter()
            .any(|s| s.id == "ramp_wire" && s.kind == pin_core::schema::table::SplineKind::Wireform)
    );
}

#[test]
fn sky_deck_mechs_switches_and_ids() {
    let checked = validate_table(sky_deck_bundle()).unwrap();
    let m = &checked.table.manifest;
    let mech = |id: &str| {
        m.mechs
            .iter()
            .find(|x| x.id == id)
            .unwrap_or_else(|| panic!("{id}"))
    };
    for (id, kind) in [
        ("vuk", MechKind::Vuk),
        ("newton", MechKind::NewtonBall),
        ("drop_1", MechKind::DropTarget),
        ("drop_2", MechKind::DropTarget),
        ("drop_3", MechKind::DropTarget),
        ("bridge_diverter", MechKind::Diverter),
        ("sling_left", MechKind::Sling),
        ("sling_right", MechKind::Sling),
        ("plunger", MechKind::Plunger),
    ] {
        assert_eq!(mech(id).kind, kind, "{id}");
    }
    let vuk = mech("vuk");
    assert_eq!((vuk.layer, vuk.exit_layer), (Some(0), Some(1)));
    assert_eq!(vuk.switch.as_deref(), Some("vuk_sw"));
    assert_eq!(mech("newton").switch.as_deref(), Some("newton_sw"));
    for i in 1..=3 {
        let d = mech(&format!("drop_{i}"));
        assert_eq!(d.layer, Some(1), "drop_{i} on the upper deck");
        assert_eq!(d.switch.as_deref(), Some(format!("drop_{i}_sw").as_str()));
        assert_eq!(d.coil.as_deref(), Some("drop_reset_coil"));
    }
    // Every mech's coil and switch names resolve; driven mechs have a coil,
    // every mech but the diverter has a switch (a diverter has none: the
    // mech set would hold a declared one open, DECISIONS 6.2-diverter).
    for x in &m.mechs {
        if x.kind != MechKind::NewtonBall {
            let c = x.coil.as_deref().unwrap_or_else(|| panic!("{} coil", x.id));
            assert!(m.coils.iter().any(|k| k.id == c), "{} coil {c}", x.id);
        }
        match x.kind {
            MechKind::Diverter | MechKind::Kickback => {}
            _ => {
                let s = x
                    .switch
                    .as_deref()
                    .unwrap_or_else(|| panic!("{} switch", x.id));
                assert!(m.switches.iter().any(|w| w.id == s), "{} switch {s}", x.id);
            }
        }
    }
    // Fixed switch names and kinds.
    let sw = |id: &str| {
        m.switches
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("{id}"))
    };
    for id in ["trough_1", "trough_2"] {
        assert_eq!(sw(id).kind, SwitchKind::Trough);
    }
    assert_eq!(
        (sw("deck_opto").kind, sw("deck_opto").layer),
        (SwitchKind::Opto, 1)
    );
    for id in [
        "vuk_sw",
        "newton_sw",
        "drop_1_sw",
        "drop_2_sw",
        "drop_3_sw",
        "loop_entry",
        "loop_top",
        "ramp_entry",
        "ramp_made",
        "shooter_lane",
        "sling_left_sw",
        "sling_right_sw",
        "lower_left_eos",
        "lower_right_eos",
        "upper_right_eos",
    ] {
        sw(id);
    }
    // Unique switch numbers, none in the cabinet range 1..=8; the cabinet
    // keys as the brief numbers them.
    let cab = m.cabinet_switches;
    assert_eq!(
        [
            cab.left_flipper,
            cab.right_flipper,
            cab.left_upper,
            cab.right_upper,
            cab.action,
            cab.start,
            cab.launch,
            cab.tilt
        ],
        [1, 2, 3, 4, 5, 6, 7, 8]
    );
    let numbers: Vec<u16> = m.switches.iter().map(|s| s.number).collect();
    assert!(numbers.iter().all(|n| !(1..=8).contains(n)), "{numbers:?}");
    let unique: BTreeSet<u16> = numbers.iter().copied().collect();
    assert_eq!(unique.len(), numbers.len(), "duplicate switch numbers");
}

#[test]
fn sky_deck_dispensers_shots_mode_and_persist() {
    let checked = validate_table(sky_deck_bundle()).unwrap();
    let m = &checked.table.manifest;
    let disp = |id: &str| {
        m.dispensers
            .iter()
            .find(|d| d.id == id)
            .unwrap_or_else(|| panic!("{id}"))
    };
    let feed = disp("feed");
    assert_eq!(feed.number, 1);
    assert!(
        feed.contents
            .iter()
            .filter(|p| *p == "standard_steel")
            .count()
            >= 3
    );
    let glo = disp("glo_feed");
    assert_eq!(glo.number, 2);
    assert!(glo.contents.iter().filter(|p| *p == "uv_globall").count() >= 3);
    let shot = |id: &str| {
        m.shots
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("{id}"))
    };
    for (id, seq) in [
        ("loop", ["loop_entry", "loop_top"]),
        ("ramp", ["ramp_entry", "ramp_made"]),
        ("deck", ["vuk_sw", "deck_opto"]),
    ] {
        let s = shot(id);
        assert_eq!(s.sequence, seq, "{id}");
        assert!(s.window_ms >= 2000, "{id}: {}", s.window_ms);
    }
    assert_eq!(m.modes.len(), 1);
    assert_eq!(m.modes[0].id, "night_overdrive");
    assert_eq!(m.modes[0].start_scene, "scene_night_overdrive");
    for s in [
        "scene_attract",
        "scene_score",
        "scene_game_over",
        "scene_night_overdrive",
    ] {
        assert!(m.scenes.iter().any(|x| x.id == s), "{s}");
    }
    let keys = &m.persist.as_ref().expect("[persist]").keys;
    for k in [
        "games_played",
        "high_score",
        "ball",
        "game_state",
        "mode:night_overdrive",
        "bridge_open",
        "overdrive_ready",
    ] {
        assert!(keys.iter().any(|x| x == k), "persist key {k}");
    }
}

/// Every file under `dir`, as `/`-separated paths relative to it, sorted.
#[cfg(unix)]
fn file_set(dir: &std::path::Path) -> BTreeSet<String> {
    fn walk(base: &std::path::Path, dir: &std::path::Path, out: &mut BTreeSet<String>) {
        for e in std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display())) {
            let p = e.expect("dir entry").path();
            if p.is_dir() {
                walk(base, &p, out);
            } else {
                let rel = p.strip_prefix(base).expect("under base");
                let parts: Vec<String> = rel
                    .components()
                    .map(|c| c.as_os_str().to_string_lossy().into_owned())
                    .collect();
                out.insert(parts.join("/"));
            }
        }
    }
    let mut out = BTreeSet::new();
    walk(dir, dir, &mut out);
    out
}

#[cfg(unix)]
#[test]
fn build_sh_output_matches_the_helper_bundle() {
    let out = scratch("sky_deck_build_sh");
    let root = sky_deck_src().join("../..");
    let status = Command::new("sh")
        .arg(sky_deck_src().join("build.sh"))
        .arg(&out)
        .current_dir(&root)
        .status()
        .expect("run build.sh");
    assert!(status.success(), "build.sh failed: {status}");
    let helper = sky_deck_bundle();
    // The complete output: the same file names (none missing, none extra)
    // and byte-identical contents for every generated asset and every
    // copied source (table, mesh, rules, layout, glass, backglass, every
    // display scene, fonts, sounds and the rest of the placeholder kit).
    let (built, helped) = (file_set(&out), file_set(helper));
    assert_eq!(
        built, helped,
        "file sets differ between build.sh and the helper"
    );
    for f in [
        "table.toml",
        "playfield.glb",
        "rules.lua",
        "layout.toml",
        "glass.toml",
        "backglass.toml",
        "display/scene_score.toml",
        "display/scene_night_overdrive.toml",
    ] {
        assert!(built.contains(f), "{f} missing from the bundle");
    }
    for f in &built {
        assert_eq!(
            std::fs::read(out.join(f)).unwrap_or_else(|e| panic!("build.sh {f}: {e}")),
            std::fs::read(helper.join(f)).unwrap_or_else(|e| panic!("helper {f}: {e}")),
            "{f} differs between build.sh and the helper"
        );
    }
}

#[test]
fn sky_deck_bundle_build_is_deterministic() {
    let a = scratch("sky_deck_a");
    let b = scratch("sky_deck_b");
    sky_deck_support::build_sky_deck_bundle(&a);
    sky_deck_support::build_sky_deck_bundle(&b);
    for f in ["playfield.glb", "table.toml", "assets.toml", "rules.lua"] {
        assert_eq!(
            std::fs::read(a.join(f)).unwrap(),
            std::fs::read(b.join(f)).unwrap(),
            "{f} differs between builds"
        );
    }
}
