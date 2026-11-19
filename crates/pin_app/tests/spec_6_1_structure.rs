//! Subphase 6.1 spec tests (Independent Tester), S1: the "Apex Circuit"
//! bundle builds with the documented command and validates (brief item 1),
//! its identity and structure (item 2), its declared coverage data (item 3)
//! and the README spot-checks (item 8).
//!
//! Black-box: structure through `pin_assets::load_table` and the bundle's
//! own files (`layout.toml`, `glass.toml`, `playfield.glb`), never the
//! table sources beyond `README.md` / `build.sh`.

#[path = "fixtures/spec_6_1/common.rs"]
mod common;

use common::*;
use pin_assets::{FlipperStageHint, Vfs, load_table};
use pin_core::input::CabinetInput;
use pin_core::schema::table::{FlipperShapeDef, MechKind, SplineKind, SwitchKind};
use pin_sim::load::{BOUNDS_MARGIN_M, GLASS_THICKNESS_M};
use std::collections::{BTreeMap, BTreeSet};

// ---------------------------------------------------------------------------
// Item 1: build and validation
// ---------------------------------------------------------------------------

#[test]
fn build_script_builds_the_bundle() {
    // The README's one-command build (`tables/apex_circuit/build.sh
    // <out_dir>`) must succeed; the other tests fall back to the
    // tests/support-style build when it does not, so this test is the one
    // that reports it.
    if let Err(e) = script_result() {
        panic!("documented build failed: {e}");
    }
    for f in ["table.toml", "playfield.glb", "rules.lua", "glass.toml"] {
        assert!(bundle().join(f).is_file(), "bundle lacks {f}");
    }
}

#[test]
fn load_validated_is_ok_without_warnings() {
    big_stack(|| {
        let v = validated();
        assert!(
            v.warnings.is_empty(),
            "validate_table warnings: {:#?}",
            v.warnings
        );
        let sim = pin_app::orchestrator::load_validated(bundle(), pin_sim::SimConfig::default());
        assert!(sim.is_ok(), "load_validated: {:?}", sim.err());
    });
}

// ---------------------------------------------------------------------------
// Item 2: identity and structure
// ---------------------------------------------------------------------------

fn loaded() -> pin_assets::LoadedTable {
    let vfs = Vfs::open(bundle()).expect("open bundle");
    load_table(&vfs).expect("load_table on the bundle")
}

#[test]
fn identity_pitch_script_and_balls() {
    let t = loaded();
    let m = &t.manifest.meta;
    assert_eq!(m.id, TABLE_ID);
    assert!(
        (m.pitch_deg - 6.5).abs() < 1e-4,
        "pitch_deg {}",
        m.pitch_deg
    );
    assert_eq!(m.script_entry, "rules.lua");
    let balls: BTreeSet<&str> = t
        .manifest
        .balls
        .profiles
        .iter()
        .map(String::as_str)
        .collect();
    assert_eq!(
        balls,
        BTreeSet::from([STEEL, POWERBALL]),
        "[balls] profiles"
    );
    assert!(
        t.warnings.is_empty(),
        "load_table warnings: {:#?}",
        t.warnings
    );
}

fn flipper(id: &str) -> &'static pin_core::schema::table::FlipperDef {
    table()
        .manifest
        .flippers
        .iter()
        .find(|f| f.id == id)
        .unwrap_or_else(|| panic!("flipper {id}"))
}

fn length_mm(f: &pin_core::schema::table::FlipperDef) -> f32 {
    match f.shape {
        FlipperShapeDef::StraightCapsule { length, .. }
        | FlipperShapeDef::CurvedBanana { length, .. } => length,
    }
}

fn norm3(v: [f32; 3]) -> f32 {
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}

#[test]
fn three_flippers_with_the_declared_sizes() {
    let t = table();
    let ids: BTreeSet<&str> = t.manifest.flippers.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(
        ids,
        BTreeSet::from(["left_flipper", "right_flipper", "left_upper"]),
        "exactly the three flippers"
    );
    assert_eq!(t.manifest.flippers.len(), 3);
    for id in ["left_flipper", "right_flipper"] {
        let f = flipper(id);
        assert!(
            (length_mm(f) - 76.0).abs() < 0.01,
            "{id} length {}",
            length_mm(f)
        );
        let axis = f
            .zipper_slide_axis
            .unwrap_or_else(|| panic!("{id} has a zipper slide axis"));
        assert!(
            (norm3(axis) - 12.0).abs() < 0.01,
            "{id}: 12 mm inward slide, axis {axis:?}"
        );
        let speed = f.zipper_slide_speed.unwrap_or(0.0);
        assert!(speed > 0.0, "{id}: positive slide speed ({speed})");
    }
    // Inward: each lower flipper slides towards the other.
    let l = flipper("left_flipper");
    let r = flipper("right_flipper");
    assert!(l.base_position[0] < r.base_position[0], "left is left");
    assert!(
        l.zipper_slide_axis.unwrap()[0] > 0.0,
        "left slides right (inward)"
    );
    assert!(
        r.zipper_slide_axis.unwrap()[0] < 0.0,
        "right slides left (inward)"
    );
    let u = flipper("left_upper");
    assert!((length_mm(u) - 55.0).abs() < 0.01, "left_upper 55 mm");
    let width = layout().playfield.width;
    assert!(
        u.base_position[0] < width / 2.0,
        "left_upper is on the left ({} of {width} mm)",
        u.base_position[0]
    );
    assert!(
        u.base_position[1] > l.base_position[1].max(r.base_position[1]) + 100.0,
        "left_upper is up the table"
    );
    // Lower / upper staging as the loader classifies it.
    for (i, f) in t.manifest.flippers.iter().enumerate() {
        let want = if f.id == "left_upper" {
            FlipperStageHint::Upper
        } else {
            FlipperStageHint::Lower
        };
        assert_eq!(t.flipper_stages[i], want, "{} stage", f.id);
    }
}

fn mech_count(k: MechKind) -> usize {
    table()
        .manifest
        .mechs
        .iter()
        .filter(|m| m.kind == k)
        .count()
}

fn switch_kind(name: &str) -> SwitchKind {
    table()
        .manifest
        .switches
        .iter()
        .find(|s| s.id == name)
        .unwrap_or_else(|| panic!("switch {name}"))
        .kind
}

#[test]
fn slings_bumpers_drops_spinner_scoop() {
    assert_eq!(mech_count(MechKind::Sling), 2, "2 slings");
    assert_eq!(mech_count(MechKind::Bumper), 3, "exactly 3 bumpers");
    assert_eq!(mech_count(MechKind::DropTarget), 3, "3 drop-target mechs");
    let m = &table().manifest.mechs;
    let spinner = m
        .iter()
        .find(|m| m.kind == MechKind::Spinner)
        .expect("a spinner mech");
    let sw = spinner.switch.as_deref().expect("spinner has a switch");
    assert_eq!(switch_kind(sw), SwitchKind::Spinner, "spinner switch kind");
    let vuk = m
        .iter()
        .find(|m| m.kind == MechKind::Vuk)
        .expect("a vuk mech (lock scoop)");
    assert!(vuk.switch.is_some(), "the scoop has a switch");
    for k in [
        MechKind::Sling,
        MechKind::Bumper,
        MechKind::DropTarget,
        MechKind::Spinner,
    ] {
        for d in m.iter().filter(|d| d.kind == k) {
            assert!(d.switch.is_some(), "{} ({k:?}) has a switch", d.id);
        }
    }
    // The scoop is the lock_scoop shot's target.
    let shot = table()
        .manifest
        .shots
        .iter()
        .find(|s| s.id == "lock_scoop")
        .expect("lock_scoop shot");
    assert!(
        shot.sequence.iter().any(|s| Some(s) == vuk.switch.as_ref()),
        "lock_scoop sequence {:?} ends in the scoop switch {:?}",
        shot.sequence,
        vuk.switch
    );
}

#[test]
fn magnet_with_one_eddy_and_one_opto() {
    let t = table();
    assert!(!t.manifest.magnets.is_empty(), "a magnet");
    let eddies: Vec<_> = t
        .manifest
        .switches
        .iter()
        .filter(|s| s.kind == SwitchKind::Eddy)
        .collect();
    assert_eq!(eddies.len(), 1, "one eddy switch");
    assert_eq!(t.magnets.len(), 1, "one magnet");
    let mag = t.magnets[0].position_m;
    let (eddy, opto) = magnet_sensors();
    let pe = eddy.position_m.unwrap();
    let po = opto.position_m.unwrap();
    let (de, dop, deo) = (plan_dist(pe, mag), plan_dist(po, mag), plan_dist(pe, po));
    // Both sensors sit at the magnet (within 100 mm of its pole) and form
    // one detector (the opto within 60 mm of the eddy sensor); no second
    // opto is as close to the magnet.
    assert!(
        de < 0.100 && dop < 0.100 && deo < 0.060,
        "eddy {} {de} m and opto {} {dop} m from the magnet, {deo} m apart",
        eddy.name,
        opto.name
    );
    let rivals: Vec<&str> = t
        .switches
        .iter()
        .filter(|s| s.kind == SwitchKind::Opto && s.name != opto.name)
        .filter(|s| s.position_m.is_some_and(|p| plan_dist(p, mag) <= dop))
        .map(|s| s.name.as_str())
        .collect();
    assert!(
        rivals.is_empty(),
        "other optos as close to the magnet: {rivals:?}"
    );
}

#[test]
fn dispenser_holds_four_steel_and_one_powerball() {
    let t = table();
    assert_eq!(t.manifest.dispensers.len(), 1, "one dispenser");
    let d = &t.manifest.dispensers[0];
    let mut counts: BTreeMap<&str, usize> = BTreeMap::new();
    for p in &d.contents {
        *counts.entry(p.as_str()).or_default() += 1;
    }
    assert_eq!(
        counts,
        BTreeMap::from([(STEEL, 4), (POWERBALL, 1)]),
        "contents {:?}",
        d.contents
    );
}

#[test]
fn wireform_ramp_with_layer_transition() {
    let t = table();
    let wires: Vec<_> = t
        .manifest
        .splines
        .iter()
        .filter(|s| s.kind == SplineKind::Wireform)
        .collect();
    assert!(!wires.is_empty(), "a wireform ramp spline");
    assert!(
        wires.iter().any(|w| {
            t.manifest
                .layer_transitions
                .iter()
                .any(|lt| lt.from_layer == 0 && lt.to_layer == w.layer && w.layer != 0)
        }),
        "a 0 -> wireform-layer transition"
    );
}

#[test]
fn trough_and_cabinet_switches() {
    let t = table();
    assert!(!trough_switches().is_empty(), "trough switches");
    let cab = t.manifest.cabinet_switches;
    let mut numbers: Vec<u16> = CabinetInput::ALL.iter().map(|&i| cab.number(i)).collect();
    numbers.sort_unstable();
    numbers.dedup();
    assert_eq!(numbers.len(), 8, "8 distinct cabinet switch numbers");
    big_stack(|| {
        let sim = fresh();
        for key in [
            "left_flipper",
            "right_flipper",
            "left_upper",
            "right_upper",
            "action",
            "start",
            "launch",
            "tilt",
        ] {
            assert!(sim.switch_id(key).is_some(), "cabinet switch {key}");
        }
    });
}

#[test]
fn switch_numbers_are_unique() {
    let mut seen: BTreeMap<u16, &str> = BTreeMap::new();
    for s in &table().manifest.switches {
        if let Some(other) = seen.insert(s.number, &s.id) {
            panic!("switch number {} used by {other} and {}", s.number, s.id);
        }
    }
    let mut ids: BTreeSet<&str> = BTreeSet::new();
    for s in &table().manifest.switches {
        assert!(ids.insert(&s.id), "duplicate switch id {}", s.id);
    }
}

#[test]
fn top_lane_rollovers() {
    let lanes = top_lanes();
    assert!(
        lanes.len() >= 3,
        "at least 3 top-lane rollovers, found {lanes:?}"
    );
    for l in &lanes {
        assert_eq!(switch_kind(l), SwitchKind::Rollover, "{l} is a rollover");
    }
}

/// Perimeter walls: `walls[i]` collider meshes with a vertex within 5 mm
/// of the playfield rectangle's edge.
#[test]
fn perimeter_walls_reach_within_a_ball_of_the_glass() {
    let glass = bundle().join("glass.toml");
    assert!(glass.is_file(), "bundle has glass.toml");
    let text = std::fs::read_to_string(&glass).expect("read glass.toml");
    let v: toml::Value = toml::from_str(&text).expect("glass.toml parses");
    let height_mm = v
        .get("height")
        .and_then(|h| h.as_float().or_else(|| h.as_integer().map(|i| i as f64)))
        .expect("glass.toml height") as f32;
    let underside = height_mm * 1.0e-3;
    // The simulation puts the glass where glass.toml says.
    let sim_under = big_stack(|| {
        let sim = fresh();
        sim.escape_bounds().1[2] - GLASS_THICKNESS_M - BOUNDS_MARGIN_M
    });
    assert!(
        (sim_under - underside).abs() < 1.0e-5,
        "glass underside {sim_under} m vs glass.toml {underside} m"
    );

    let pf = &layout().playfield;
    let (w, l) = (pf.width * 1.0e-3, pf.length * 1.0e-3);
    let edge = 0.005;
    let mut perimeter = 0;
    for mesh in &validated().scene.meshes {
        if !mesh.name.starts_with("walls[") || !mesh.extras.collider {
            continue;
        }
        let on_edge = mesh
            .positions
            .iter()
            .any(|p| p[0] <= edge || p[0] >= w - edge || p[1] <= edge || p[1] >= l - edge);
        if !on_edge {
            continue;
        }
        perimeter += 1;
        let top = mesh.positions.iter().map(|p| p[2]).fold(f32::MIN, f32::max);
        let gap = underside - top;
        assert!(
            (0.0..BALL_DIAMETER_M).contains(&gap),
            "{}: wall top {top} m, glass underside {underside} m, gap {gap} m (must be < 27 mm)",
            mesh.name
        );
    }
    assert!(perimeter > 0, "no perimeter wall meshes found");
    // Layout wall height (every wall and post rises to it).
    let gap = underside - pf.wall_height * 1.0e-3;
    assert!(
        (0.0..BALL_DIAMETER_M).contains(&gap),
        "wall_height {} mm vs glass {height_mm} mm",
        pf.wall_height
    );
}

// ---------------------------------------------------------------------------
// Item 3: declared coverage data
// ---------------------------------------------------------------------------

#[test]
fn declared_shots() {
    let shots = &table().manifest.shots;
    assert!(shots.len() >= 5, ">= 5 shots, found {}", shots.len());
    for id in [
        "left_orbit",
        "right_orbit",
        "cross_ramp",
        "drop_bank",
        "lock_scoop",
    ] {
        let s = shots
            .iter()
            .find(|s| s.id == id)
            .unwrap_or_else(|| panic!("shot {id}"));
        assert!(!s.sequence.is_empty(), "{id}: switch sequence");
        assert!(s.window_ms > 0, "{id}: window_ms");
    }
    for s in shots {
        assert!(!s.sequence.is_empty() && s.window_ms > 0, "{}", s.id);
    }
}

#[test]
fn declared_modes() {
    let m = &table().manifest;
    let ids: BTreeSet<&str> = m.modes.iter().map(|x| x.id.as_str()).collect();
    assert_eq!(
        ids,
        BTreeSet::from(["powerball_mania", "combo_chain"]),
        "exactly the two modes"
    );
    assert_eq!(m.modes.len(), 2);
    for mode in &m.modes {
        assert!(
            m.scenes.iter().any(|s| s.id == mode.start_scene),
            "{}: start_scene {} is a declared scene",
            mode.id,
            mode.start_scene
        );
    }
}

#[test]
fn declared_persist_keys() {
    let keys = &table().manifest.persist.as_ref().expect("[persist]").keys;
    for k in PERSIST_KEYS {
        assert!(keys.iter().any(|x| x == k), "[persist] declares {k}");
    }
    // The manifest order is the key index the simulation uses.
    big_stack(|| {
        let sim = fresh();
        for k in PERSIST_KEYS {
            assert_eq!(
                sim.ids().persist_key(k).map(|p| p.0),
                Some(persist_index(k)),
                "{k}: persist index = [persist] order"
            );
        }
    });
}

const PERSIST_KEYS: [&str; 9] = [
    "ball",
    "game_state",
    "high_score",
    "games_played",
    "skill_shot_count",
    "firewall_save_count",
    "apex_jackpot_count",
    "mode:powerball_mania",
    "mode:combo_chain",
];

// ---------------------------------------------------------------------------
// Item 8: README spot-checks
// ---------------------------------------------------------------------------

#[test]
fn readme_states_build_rules_and_overdrive_selection() {
    let text = readme();
    assert!(!text.is_empty(), "tables/apex_circuit/README.md exists");
    let lower = text.to_ascii_lowercase();
    assert!(
        text.contains("tables/apex_circuit/build.sh"),
        "README states the build command"
    );
    for word in [
        "rules",
        "skill shot",
        "combo",
        "powerball mania",
        "bonus",
        "overdrive",
        "apex jackpot",
    ] {
        assert!(lower.contains(word), "README mentions {word:?}");
    }
    assert!(
        lower.contains("zipper") || lower.contains("firewall"),
        "README names the Zipper / Firewall Save outcome"
    );
    // The selection rule between the two action outcomes is stated.
    assert!(
        lower.contains("selection rule") || lower.contains("selected") || lower.contains("chosen"),
        "README states the Overdrive selection rule"
    );
}
