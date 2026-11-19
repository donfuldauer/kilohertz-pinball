//! Subphase 6.2 spec tests (Independent Tester): structure of the "Sky
//! Deck" bundle (brief C1, C2 and the static part of C6, "C6b").
//!
//! Black-box: the bundle is built in-process from `tables/sky_deck` the way
//! its `build.sh` does (`fixtures/spec_6_2/common.rs`), loaded through
//! `pin_app::orchestrator::load_validated` and inspected through
//! `pin_assets::load_table`, `pin_assets::load_gltf` and the bundle's
//! `layout.toml` (parsed by `pin_tablegen::parse_layout`).

#[path = "fixtures/spec_6_2/common.rs"]
mod common;

use common::{DECK_Z_M, big_stack, bundle, layout, table};
use pin_app::orchestrator::load_validated;
use pin_assets::{FlipperStageHint, Vfs, load_gltf};
use pin_core::input::CabinetInput;
use pin_core::materials::Material;
use pin_core::schema::table::{FlipperShapeDef, FlipperStagingDef, MechKind, SwitchKind};
use pin_sim::SimConfig;
use pin_sim::autoplay::TableInfo;

fn close(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

// ---------------------------------------------------------------------------
// C1: build and load
// ---------------------------------------------------------------------------

/// C1: `load_validated` (manifests, glTF, clearance validator) accepts the
/// bundle and the simulation loads.
#[test]
fn c1_bundle_loads_validated() {
    big_stack(|| {
        let sim = load_validated(bundle(), SimConfig::default());
        assert!(sim.is_ok(), "load_validated: {:?}", sim.err());
        let sim = sim.unwrap();
        assert!(
            !sim.script_faulted(),
            "on_init faulted: {:?}",
            sim.script_error()
        );
    });
}

/// C1: the bundle holds the copied sources and the generated files.
#[test]
fn c1_bundle_files() {
    let b = bundle();
    for f in [
        "table.toml",
        "playfield.glb",
        "rules.lua",
        "backglass.toml",
        "layout.toml",
        "glass.toml",
    ] {
        assert!(b.join(f).is_file(), "bundle has {f}");
    }
    let display: Vec<_> = std::fs::read_dir(common::sky_src().join("display"))
        .expect("tables/sky_deck/display")
        .map(|e| e.expect("entry").file_name())
        .collect();
    assert!(!display.is_empty(), "display/ has files");
    for f in display {
        assert!(
            b.join("display").join(&f).exists(),
            "bundle has display/{f:?}"
        );
    }
}

/// C1: table id, script entry, pitch and ball profiles.
#[test]
fn c1_meta_and_profiles() {
    let t = table();
    let m = &t.manifest.meta;
    assert_eq!(m.id, "sky_deck");
    assert_eq!(m.script_entry, "rules.lua");
    assert!(close(m.pitch_deg, 6.5, 1e-4), "pitch {} deg", m.pitch_deg);
    assert!(close(t.pitch_rad, 6.5f32.to_radians(), 1e-5));
    let mut p = t.manifest.balls.profiles.clone();
    p.sort();
    assert_eq!(
        p,
        vec!["standard_steel".to_owned(), "uv_globall".to_owned()]
    );
}

// ---------------------------------------------------------------------------
// C2: structure
// ---------------------------------------------------------------------------

/// C2: exactly three flippers with the contracted layers, shapes, heights
/// and staging.
#[test]
fn c2_flippers() {
    let t = table();
    let ids: Vec<&str> = t.manifest.flippers.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(
        t.manifest.flippers.len(),
        3,
        "exactly three flippers: {ids:?}"
    );
    let find = |id: &str| {
        t.manifest
            .flippers
            .iter()
            .find(|f| f.id == id)
            .unwrap_or_else(|| panic!("flipper {id}"))
    };
    for (id, layer, len) in [("lower_left", 0u8, 76.0f32), ("lower_right", 0, 76.0)] {
        let f = find(id);
        assert_eq!(f.collision_layer, layer, "{id} layer");
        match f.shape {
            FlipperShapeDef::StraightCapsule { length, .. } => {
                assert!(close(length, len, 1e-3), "{id} length {length}");
            }
            s => panic!("{id}: StraightCapsule expected, got {s:?}"),
        }
    }
    let u = find("upper_right");
    assert_eq!(u.collision_layer, 1, "upper_right on layer 1");
    match u.shape {
        FlipperShapeDef::StraightCapsule { length, .. } => {
            assert!(close(length, 52.0, 1e-3), "upper_right length {length}");
        }
        s => panic!("upper_right: StraightCapsule expected, got {s:?}"),
    }
    assert!(
        close(u.base_position[2], 65.0, 1e-3),
        "upper_right base z {} mm",
        u.base_position[2]
    );
    assert_eq!(
        u.staging,
        FlipperStagingDef::DedicatedButton {
            input: CabinetInput::RightUpper
        }
    );
    // Runtime configs agree, and the lower pair is staged lower.
    assert_eq!(t.flippers.len(), 3);
    for (i, f) in t.manifest.flippers.iter().enumerate() {
        assert_eq!(t.flippers[i].collision_layer, f.collision_layer);
        if f.id.starts_with("lower_") {
            assert_eq!(t.flipper_stages[i], FlipperStageHint::Lower, "{}", f.id);
        }
    }
}

/// C2: a `[[decks]]` entry on layer 1 at z = 65 mm, acrylic, in the
/// generated bundle (its `layout.toml` and its `playfield.glb` collider).
#[test]
fn c2_upper_deck() {
    let decks: Vec<_> = layout()
        .decks
        .iter()
        .enumerate()
        .filter(|(_, d)| d.layer == 1)
        .collect();
    assert!(!decks.is_empty(), "a layer-1 deck");
    let (i, d) = decks[0];
    assert!(close(d.z, 65.0, 1e-3), "deck z {} mm", d.z);
    assert_eq!(d.material, Material::Acrylic, "deck material");
    // The generated collider node.
    let vfs = Vfs::open(bundle()).expect("open bundle");
    let scene = load_gltf(&vfs, &table().manifest.meta.playfield_mesh).expect("playfield glTF");
    let name = format!("decks[{i}]");
    let node = scene
        .meshes
        .iter()
        .find(|m| m.name == name)
        .unwrap_or_else(|| panic!("glTF node {name}"));
    assert!(node.extras.collider, "{name} is a collider");
    assert_eq!(node.extras.collision_layer, Some(1), "{name} layer");
    let mat = node
        .material
        .and_then(|k| scene.materials.get(k))
        .map(|m| m.name.clone());
    assert_eq!(mat.as_deref(), Some("acrylic"), "{name} material");
    // Its top face is at 65 mm (table frame).
    let m = node.transform;
    let top = node
        .positions
        .iter()
        .map(|p| m[0][2] * p[0] + m[1][2] * p[1] + m[2][2] * p[2] + m[3][2])
        .fold(f32::NEG_INFINITY, f32::max);
    assert!(close(top, DECK_Z_M, 1.0e-3), "deck top z {top} m");
}

/// C2: the contracted mechs, with their kinds and layers.
#[test]
fn c2_mechs() {
    let t = table();
    let m = |id: &str| {
        t.manifest
            .mechs
            .iter()
            .find(|m| m.id == id)
            .unwrap_or_else(|| panic!("mech {id}"))
    };
    let vuk = m("vuk");
    assert_eq!(vuk.kind, MechKind::Vuk);
    assert_eq!(vuk.layer.unwrap_or(0), 0, "vuk capture layer");
    assert_eq!(vuk.exit_layer, Some(1), "vuk exit layer");
    assert_eq!(m("newton").kind, MechKind::NewtonBall);
    for d in ["drop_1", "drop_2", "drop_3"] {
        assert_eq!(m(d).kind, MechKind::DropTarget, "{d}");
        assert_eq!(m(d).layer, Some(1), "{d} on the upper deck");
    }
    assert_eq!(m("bridge_diverter").kind, MechKind::Diverter);
    assert_eq!(m("sling_left").kind, MechKind::Sling);
    assert_eq!(m("sling_right").kind, MechKind::Sling);
    assert_eq!(m("plunger").kind, MechKind::Plunger);
    // The runtime mech list carries them, the diverter with an id.
    let si = t
        .mechs
        .iter()
        .find(|s| s.name == "bridge_diverter")
        .expect("bridge_diverter in the loaded table");
    assert!(si.diverter.is_some(), "bridge_diverter has a diverter id");
}

/// C2: dispensers `feed` (>= 3 steel) and `glo_feed` (>= 3 Glo-Balls).
#[test]
fn c2_dispensers() {
    let t = table();
    let d = |id: &str| {
        t.manifest
            .dispensers
            .iter()
            .find(|d| d.id == id)
            .unwrap_or_else(|| panic!("dispenser {id}"))
    };
    let feed = d("feed");
    let steel = feed
        .contents
        .iter()
        .filter(|p| *p == "standard_steel")
        .count();
    assert!(steel >= 3, "feed holds {steel} standard_steel");
    let glo = d("glo_feed");
    let n = glo.contents.iter().filter(|p| *p == "uv_globall").count();
    assert!(n >= 3, "glo_feed holds {n} uv_globall");
}

/// C2: `[cabinet_switches]` numbers 1..=8 in the contracted order.
#[test]
fn c2_cabinet_switches() {
    let c = table().manifest.cabinet_switches;
    assert_eq!(
        [
            c.left_flipper,
            c.right_flipper,
            c.left_upper,
            c.right_upper,
            c.action,
            c.start,
            c.launch,
            c.tilt
        ],
        [1, 2, 3, 4, 5, 6, 7, 8]
    );
}

/// C2: the contracted switch names, trough kinds, the deck opto on layer 1
/// and unique switch numbers.
#[test]
fn c2_switches() {
    let t = table();
    let s = |id: &str| {
        t.manifest
            .switches
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("switch {id}"))
    };
    for id in [
        "trough_1",
        "trough_2",
        "vuk_sw",
        "newton_sw",
        "drop_1_sw",
        "drop_2_sw",
        "drop_3_sw",
        "deck_opto",
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
        s(id);
    }
    assert_eq!(s("trough_1").kind, SwitchKind::Trough);
    assert_eq!(s("trough_2").kind, SwitchKind::Trough);
    assert_eq!(s("deck_opto").layer, 1, "deck_opto on layer 1");
    let mut numbers: Vec<u16> = t.manifest.switches.iter().map(|s| s.number).collect();
    let n = numbers.len();
    numbers.sort_unstable();
    numbers.dedup();
    assert_eq!(numbers.len(), n, "switch numbers are unique");
    // Mech switch bindings of the contract.
    let mech_sw = |id: &str| {
        t.manifest
            .mechs
            .iter()
            .find(|m| m.id == id)
            .and_then(|m| m.switch.clone())
    };
    assert_eq!(mech_sw("vuk").as_deref(), Some("vuk_sw"));
    assert_eq!(mech_sw("newton").as_deref(), Some("newton_sw"));
    for i in 1..=3 {
        assert_eq!(
            mech_sw(&format!("drop_{i}")),
            Some(format!("drop_{i}_sw")),
            "drop_{i} switch"
        );
    }
}

/// C2: at least three shots with the contracted sequences; exactly one
/// mode; the scenes; every reference resolves.
#[test]
fn c2_shots_modes_scenes() {
    let t = table();
    assert!(t.manifest.shots.len() >= 3, "[[shots]] >= 3");
    for (id, seq) in [
        ("loop", ["loop_entry", "loop_top"]),
        ("ramp", ["ramp_entry", "ramp_made"]),
        ("deck", ["vuk_sw", "deck_opto"]),
    ] {
        let s = t
            .manifest
            .shots
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("shot {id}"));
        assert_eq!(s.sequence, seq.map(str::to_owned).to_vec(), "shot {id}");
    }
    assert_eq!(t.manifest.modes.len(), 1, "exactly one mode");
    let mode = &t.manifest.modes[0];
    assert_eq!(mode.id, "night_overdrive");
    assert_eq!(mode.start_scene, "scene_night_overdrive");
    for sc in [
        "scene_attract",
        "scene_score",
        "scene_game_over",
        "scene_night_overdrive",
    ] {
        assert!(
            t.manifest.scenes.iter().any(|s| s.id == sc),
            "scene {sc} declared"
        );
        assert!(t.ids.scene(sc).is_some(), "scene {sc} interned");
    }
    // Every reference resolves (the loader interned them).
    assert_eq!(t.shots.len(), t.manifest.shots.len());
    for (def, si) in t.manifest.shots.iter().zip(&t.shots) {
        assert_eq!(si.sequence.len(), def.sequence.len(), "shot {}", def.id);
        for name in &def.sequence {
            assert!(
                t.manifest.switches.iter().any(|s| &s.id == name),
                "shot {}: {name} is a [[switches]] entry",
                def.id
            );
        }
    }
    assert_eq!(t.modes.len(), 1);
    assert_eq!(
        Some(t.modes[0].start_scene),
        t.ids.scene("scene_night_overdrive")
    );
}

/// C2: the `[persist] keys` include the contracted keys.
#[test]
fn c2_persist_keys() {
    let keys = &table()
        .manifest
        .persist
        .as_ref()
        .expect("[persist] section")
        .keys;
    for k in [
        "games_played",
        "high_score",
        "ball",
        "game_state",
        "mode:night_overdrive",
        "bridge_open",
        "overdrive_ready",
    ] {
        assert!(keys.iter().any(|x| x == k), "[persist] keys has {k}");
    }
    assert!(
        keys.len() <= 64,
        "every key addressable by HostView::persist"
    );
}

// ---------------------------------------------------------------------------
// C6b: static Gate-A2 readiness
// ---------------------------------------------------------------------------

/// C6b: no `[[switches]]` entry uses a cabinet number; no coverage switch
/// is a cabinet input.
#[test]
fn c6b_no_switch_is_a_cabinet_number() {
    let t = table();
    let cab = t.manifest.cabinet_switches;
    let cab_numbers: Vec<u16> = CabinetInput::ALL.iter().map(|&i| cab.number(i)).collect();
    for s in &t.manifest.switches {
        assert!(
            !cab_numbers.contains(&s.number),
            "switch {} uses cabinet number {}",
            s.id,
            s.number
        );
    }
    let info = TableInfo::load(bundle()).expect("TableInfo loads");
    let cov: Vec<_> = info.coverage_switches().collect();
    assert_eq!(
        cov.len(),
        t.manifest.switches.len(),
        "every declared switch counts for coverage"
    );
    for s in cov {
        assert!(!s.cabinet, "{} is a cabinet input", s.name);
        assert!(
            !cab_numbers.contains(&s.id.0),
            "coverage switch {} has cabinet number {}",
            s.name,
            s.id.0
        );
        assert!(
            !CabinetInput::ALL.iter().any(|i| i.name() == s.name),
            "coverage switch named like a cabinet input: {}",
            s.name
        );
    }
}

/// C6b: every declared shot's switches and every flipper's `eos_switch`
/// exist as `[[switches]]` entries; the autoplay table info sees the shots
/// and the mode with its persist key.
#[test]
fn c6b_references_exist() {
    let t = table();
    let exists = |n: &str| t.manifest.switches.iter().any(|s| s.id == n);
    for s in &t.manifest.shots {
        for n in &s.sequence {
            assert!(exists(n), "shot {}: switch {n}", s.id);
        }
    }
    for f in &t.manifest.flippers {
        assert!(
            exists(&f.eos_switch),
            "flipper {}: eos {}",
            f.id,
            f.eos_switch
        );
    }
    let info = TableInfo::load(bundle()).expect("TableInfo loads");
    assert_eq!(info.table_id, "sky_deck");
    assert_eq!(info.shots.len(), t.manifest.shots.len());
    let mode = info
        .modes
        .iter()
        .find(|m| m.name == "night_overdrive")
        .expect("night_overdrive in TableInfo");
    assert_eq!(
        mode.persist_key,
        Some(common::persist_index("mode:night_overdrive")),
        "the mode's start is observable through its persist key"
    );
}
