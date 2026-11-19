//! Subphase 6.4 spec tests (Independent Tester), brief S1: static checks of
//! the built Feature Lab bundle (TABLE CONTRACT C1–C7) through
//! `pin_assets::load_table`, plus static A2 readiness.

#[path = "fixtures/spec_6_4/common.rs"]
mod common;

use common::*;
use pin_core::flipper::FlipperShape;
use pin_core::input::CabinetInput;
use pin_core::schema::backglass::BackglassManifest;
use pin_core::schema::table::{FlipperShapeDef, FlipperStagingDef, MechKind, SwitchKind};

#[test]
fn c1_meta_and_cabinet_switches() {
    let m = &table().manifest;
    assert_eq!(m.meta.id, "feature_lab");
    assert_eq!(m.meta.script_entry, "rules.lua");
    assert!(
        (m.meta.pitch_deg - 6.5).abs() < 1e-6,
        "pitch_deg = {}",
        m.meta.pitch_deg
    );
    assert_eq!(m.meta.players_max, 1);
    let cab = m.cabinet_switches;
    let mut numbers: Vec<(u16, String)> = CabinetInput::ALL
        .iter()
        .map(|&i| (cab.number(i), format!("cabinet {i:?}")))
        .collect();
    numbers.extend(m.switches.iter().map(|s| (s.number, s.id.clone())));
    numbers.sort();
    for w in numbers.windows(2) {
        assert_ne!(
            w[0].0, w[1].0,
            "switch number {} used by {} and {}",
            w[0].0, w[0].1, w[1].1
        );
    }
    // Switch ids are unique too.
    let mut ids: Vec<&str> = m.switches.iter().map(|s| s.id.as_str()).collect();
    ids.sort_unstable();
    let n = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), n, "duplicate [[switches]] id");
}

fn flipper(id: &str) -> &'static pin_core::schema::table::FlipperDef {
    table()
        .manifest
        .flippers
        .iter()
        .find(|f| f.id == id)
        .unwrap_or_else(|| panic!("flipper {id} declared"))
}

fn backbox_layer() -> u8 {
    table()
        .backglass
        .backbox
        .as_ref()
        .expect("backglass.toml has a [backbox] block")
        .collision_layer
}

#[test]
fn c2_three_flippers_shapes_buttons_layers() {
    let m = &table().manifest;
    assert_eq!(m.flippers.len(), 3, "exactly 3 flippers");
    let mut ids: Vec<&str> = m.flippers.iter().map(|f| f.id.as_str()).collect();
    ids.sort_unstable();
    assert_eq!(ids, ["lower_left", "lower_right", BACKBOX_FLIPPER]);

    let ll = flipper("lower_left");
    assert!(
        matches!(ll.shape, FlipperShapeDef::StraightCapsule { .. }),
        "lower_left is a StraightCapsule: {:?}",
        ll.shape
    );
    assert_eq!(
        ll.staging,
        FlipperStagingDef::DedicatedButton {
            input: CabinetInput::LeftFlipper
        }
    );
    assert_eq!(ll.collision_layer, 0);

    let lr = flipper("lower_right");
    let FlipperShapeDef::CurvedBanana {
        length,
        arc_radius,
        arc_sweep_deg,
        ..
    } = lr.shape
    else {
        panic!("lower_right is a CurvedBanana: {:?}", lr.shape)
    };
    assert!(
        arc_sweep_deg > 0.0,
        "positive arc_sweep_deg: {arc_sweep_deg}"
    );
    let implied = 2.0 * (length / (2.0 * arc_radius)).asin().to_degrees();
    assert!(
        (arc_sweep_deg - implied).abs() <= 1.0,
        "1° sweep lint: arc_sweep_deg {arc_sweep_deg} vs 2·asin(L/2R) = {implied}"
    );
    assert_eq!(
        lr.staging,
        FlipperStagingDef::DedicatedButton {
            input: CabinetInput::RightFlipper
        }
    );
    assert_eq!(lr.collision_layer, 0);
    // The loaded config agrees (a Banana reaches the physics as one).
    let i = m
        .flippers
        .iter()
        .position(|f| f.id == "lower_right")
        .unwrap();
    assert!(matches!(
        table().flippers[i].shape,
        FlipperShape::CurvedBanana { .. }
    ));

    let bb = flipper(BACKBOX_FLIPPER);
    assert_eq!(bb.collision_layer, backbox_layer(), "on the backbox layer");
    assert_eq!(backbox_layer(), 4, "backbox collision layer 4 (default)");
    let n = norm3(bb.surface_normal);
    assert!(
        n[2] < 0.999,
        "backbox flipper surface_normal is not +Z: {:?}",
        bb.surface_normal
    );
    assert_eq!(
        bb.staging,
        FlipperStagingDef::DedicatedButton {
            input: CabinetInput::LeftUpper
        }
    );
}

#[test]
fn c3_balls_and_xl_restriction() {
    let b = &table().manifest.balls;
    let mut p: Vec<&str> = b.profiles.iter().map(String::as_str).collect();
    p.sort_unstable();
    assert_eq!(p, [XL, LOW_REM, STEEL, GLO], "[balls] profiles");
    let r: Vec<_> = b.restrict.iter().filter(|r| r.profile == XL).collect();
    assert_eq!(r.len(), 1, "one restrict entry for {XL}");
    assert_eq!(r[0].layers, vec![0u8], "XL restricted to layer 0");
    let area = &r[0].area;
    assert!(area.len() >= 3, "area polygon with >= 3 points: {area:?}");

    // Inscribed-circle estimate on a 2 mm grid: the largest clearance from
    // the boundary of a point inside the polygon (even-odd).
    let (mut lo, mut hi) = ([f32::INFINITY; 2], [f32::NEG_INFINITY; 2]);
    for q in area {
        for k in 0..2 {
            lo[k] = lo[k].min(q[k]);
            hi[k] = hi[k].max(q[k]);
        }
    }
    let step = 2.0f32;
    let mut best = 0.0f32;
    let mut x = lo[0];
    while x <= hi[0] {
        let mut y = lo[1];
        while y <= hi[1] {
            if in_polygon(area, [x, y]) {
                best = best.max(boundary_dist(area, [x, y]));
            }
            y += step;
        }
        x += step;
    }
    assert!(
        best >= 40.0,
        "the area has a point >= 40 mm clear of every edge (best {best:.1} mm)"
    );
    let diameter_mm = pin_core::builtin_profile(XL).unwrap().diameter_m * 1e3;
    assert!((diameter_mm - 79.4).abs() < 1e-3);
    assert!(
        2.0 * best >= 80.0 && 2.0 * best >= diameter_mm,
        "inscribed width {:.1} mm carries the {diameter_mm} mm ball",
        2.0 * best
    );
}

#[test]
fn c4_dispensers() {
    let m = &table().manifest;
    let d = |id: &str| {
        m.dispensers
            .iter()
            .find(|d| d.id == id)
            .unwrap_or_else(|| panic!("dispenser {id} declared"))
    };
    let feed = d("feed");
    assert!(
        feed.contents.iter().all(|c| c == STEEL),
        "feed serves standard_steel: {:?}",
        feed.contents
    );
    let mf = d("magnet_feed");
    assert!(
        mf.contents.iter().all(|c| c == STEEL || c == LOW_REM),
        "magnet_feed holds only steel profiles: {:?}",
        mf.contents
    );
    let xl = d("xl_feed");
    let e = xl.exit_position;
    assert!(
        in_polygon(&xl_area_mm(), [e[0], e[1]]),
        "xl_feed exit {e:?} inside the XL area"
    );
    assert!(
        xl.contents.iter().all(|c| c == XL),
        "xl_feed holds only XL balls: {:?}",
        xl.contents
    );
    let glo = d("glo_feed");
    assert!(
        glo.contents.iter().all(|c| c == GLO),
        "glo_feed holds only Glo-Balls: {:?}",
        glo.contents
    );
    // Dispenser numbers are distinct.
    let mut n: Vec<u16> = m.dispensers.iter().map(|d| d.number).collect();
    n.sort_unstable();
    let len = n.len();
    n.dedup();
    assert_eq!(n.len(), len, "distinct dispenser numbers");
}

#[test]
fn c4_magnet_feed_serves_both_steel_profiles_by_script() {
    // The script chooses the profile with `spawn_ball` (C4); the sim side
    // of that must accept both profiles on the table.
    big_stack(|| {
        let mut sim = load();
        let mf = table()
            .dispensers
            .iter()
            .find(|d| d.name == "magnet_feed")
            .expect("magnet_feed");
        for p in [STEEL, LOW_REM] {
            let h = sim
                .spawn_ball(p, mf.exit_position_m, mf.exit_velocity_m_s)
                .unwrap_or_else(|e| panic!("spawn {p} at magnet_feed: {e}"));
            let b = sim.ball(h).expect("ball");
            assert_eq!(b.profile.0, profile_index(p));
            assert!(sim.remove_ball(h));
        }
    });
}

#[test]
fn c5_magnet_backbox_vuk_transitions_files() {
    let t = table();
    let m = &t.manifest;
    assert_eq!(m.magnets.len(), 1, "one magnet");
    let mag = &m.magnets[0];
    assert_eq!(mag.id, "hold_magnet");
    let opto_at_pole = m.switches.iter().any(|s| {
        s.kind == SwitchKind::Opto
            && s.position
                .is_some_and(|p| (p[0] - mag.position[0]).hypot(p[1] - mag.position[1]) <= 15.0)
    });
    assert!(
        opto_at_pole,
        "an opto switch within 15 mm (plan) of the pole {:?}",
        mag.position
    );

    let vuk = m
        .mechs
        .iter()
        .find(|x| x.id == BACKBOX_VUK)
        .expect("backbox VUK mech");
    assert_eq!(vuk.kind, MechKind::Vuk);
    assert_eq!(
        vuk.layer.unwrap_or(0),
        0,
        "the VUK is on the main playfield"
    );
    assert_eq!(vuk.exit_layer, Some(4), "the VUK feeds layer 4");
    assert!(
        m.layer_transitions
            .iter()
            .any(|l| l.from_layer == 0 && l.to_layer == 4),
        "a layer transition 0 -> 4 (backbox entry)"
    );
    assert!(
        m.layer_transitions
            .iter()
            .any(|l| l.from_layer == 4 && l.to_layer == 0),
        "a layer transition 4 -> 0 (backbox drain)"
    );

    let dir = bundle();
    for f in ["backbox.glb", "backbox_colliders.glb"] {
        assert!(dir.join(f).is_file(), "bundle has {f}");
    }
    let text = std::fs::read_to_string(dir.join("backglass.toml")).expect("backglass.toml");
    let bg = BackglassManifest::from_toml_str(&text).expect("backglass.toml parses");
    let bb = bg.backbox.expect("[backbox] block");
    assert_eq!(bb.scene, "backbox.glb");
    assert_eq!(bb.collision_layer, 4);
    let si = t.backbox.as_ref().expect("loaded backbox");
    assert_eq!(si.collision_layer, 4);
}

#[test]
fn c6_modes_persist_shots() {
    let t = table();
    let m = &t.manifest;
    let ids: Vec<&str> = m.modes.iter().map(|x| x.id.as_str()).collect();
    assert_eq!(ids, MODES, "[[modes]] ids in selector order");
    for (i, mode) in m.modes.iter().enumerate() {
        assert!(
            !mode.start_scene.is_empty(),
            "{} has a start_scene",
            mode.id
        );
        let scene = m
            .scenes
            .iter()
            .find(|s| s.id == mode.start_scene)
            .unwrap_or_else(|| panic!("scene {} of {} declared", mode.start_scene, mode.id));
        assert!(
            bundle().join(&scene.file).is_file(),
            "scene file {} exists",
            scene.file
        );
        assert_eq!(t.modes[i].name, mode.id);
    }
    let keys = &m.persist.as_ref().expect("[persist]").keys;
    for k in [
        "game_state",
        "ball",
        "mode_sel",
        "games_played",
        "high_score",
        "cmp_release_ms_standard",
        "cmp_release_ms_low_rem",
    ] {
        assert!(keys.iter().any(|x| x == k), "[persist] key {k}");
    }
    for mode in MODES {
        let k = format!("mode:{mode}");
        assert!(keys.contains(&k), "[persist] key {k}");
    }
    assert!(m.shots.len() >= 5, ">= 5 shots, got {}", m.shots.len());
    for s in &m.shots {
        assert!(!s.sequence.is_empty(), "shot {} nonempty", s.id);
        for sw in &s.sequence {
            assert!(
                m.switches.iter().any(|d| &d.id == sw),
                "shot {}: {sw} is a declared [[switches]] id",
                s.id
            );
        }
    }
}

/// Static A2 readiness: every declared (non-cabinet) switch can be closed
/// by a soak: it belongs to a mech, a flipper EOS, the trough, or it has a
/// position and radius (a zone a ball can enter).
#[test]
fn a2_every_switch_is_closable() {
    let m = &table().manifest;
    for s in &m.switches {
        let mech = m.mechs.iter().any(|x| x.switch.as_deref() == Some(&s.id));
        let eos = m.flippers.iter().any(|f| f.eos_switch == s.id);
        let trough = s.kind == SwitchKind::Trough;
        let zone = s.position.is_some() && s.radius.is_some();
        assert!(
            mech || eos || trough || zone,
            "switch {} ({:?}) is referenced by nothing and has no zone",
            s.id,
            s.kind
        );
    }
}

#[test]
fn readme_has_the_mode_table() {
    let rows = readme_mode_rows();
    for (i, mode) in MODES.iter().enumerate() {
        let r = rows
            .iter()
            .find(|r| r.mode == *mode)
            .unwrap_or_else(|| panic!("README mode table row for {mode}: {rows:?}"));
        assert_eq!(
            r.index.trim(),
            i.to_string(),
            "{mode}: selector index in the README"
        );
        assert!(!r.condition.is_empty(), "{mode}: start condition");
        assert!(
            !switches_in(&r.shot_switches).is_empty(),
            "{mode}: shot switches name declared switches: {:?}",
            r.shot_switches
        );
        assert!(!r.scene.is_empty(), "{mode}: scene");
    }
}
