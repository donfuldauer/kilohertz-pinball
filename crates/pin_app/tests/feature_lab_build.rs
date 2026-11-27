//! 6.4 R1, R2, R5 (negative), R8 and the table contract C1..C7: the
//! Feature Lab sources build (through `pin_tablegen::backbox::build_from`,
//! which `tables/feature_lab/build.sh` runs as `pin_tablegen build`) into a
//! bundle that loads, validates for every declared ball with the XL ball
//! restricted to its area, and fails validation naming the offending
//! feature once the restriction is removed.

mod feature_lab_support;

use feature_lab_support::build::{VUK_LATERAL_M_S, VUK_RISE_M_S, parse_hex, vuk_eject};
use feature_lab_support::*;
use pin_app::orchestrator::load_validated;
use pin_assets::{Vfs, load_gltf, load_table};
use pin_core::schema::table::{FlipperShapeDef, FlipperStagingDef, MechKind, TableManifest};
use pin_sim::SimConfig;
use pin_tablegen::backbox::{BackboxFrame, generate_backbox, parse_backbox_layout};
use pin_tablegen::clearance::validate_bundle;
use pin_tablegen::layout::parse_layout;
use proptest::prelude::*;

fn manifest() -> TableManifest {
    TableManifest::from_toml_str(&std::fs::read_to_string(bundle().join("table.toml")).unwrap())
        .unwrap()
}

#[test]
fn sources_hold_no_build_output() {
    let src = feature_lab_src();
    for f in [
        "layout.toml",
        "backbox_layout.toml",
        "table.toml",
        "rules.lua",
        "backglass.toml",
        "glass.toml",
        "README.md",
        "build.sh",
    ] {
        assert!(src.join(f).is_file(), "{f}");
    }
    for f in [
        "playfield.glb",
        "backbox.glb",
        "backbox_colliders.glb",
        "assets.toml",
    ] {
        assert!(!src.join(f).exists(), "{f} is build output");
    }
    let scenes = std::fs::read_dir(src.join("display")).unwrap().count();
    assert_eq!(scenes, 8, "attract, score, game over and five mode scenes");
}

#[test]
fn bundle_has_every_generated_file() {
    for f in [
        "table.toml",
        "playfield.glb",
        "backbox.glb",
        "backbox_colliders.glb",
        "rules.lua",
        "backglass.toml",
        "layout.toml",
        "glass.toml",
        "assets.toml",
        "display/scene_glo_ball.toml",
    ] {
        assert!(bundle().join(f).is_file(), "{f}");
    }
}

#[test]
fn the_build_is_deterministic() {
    let a = scratch("fl_build_a");
    build_feature_lab_bundle(&a).unwrap();
    for f in [
        "table.toml",
        "playfield.glb",
        "backbox.glb",
        "backbox_colliders.glb",
    ] {
        assert_eq!(
            std::fs::read(a.join(f)).unwrap(),
            std::fs::read(bundle().join(f)).unwrap(),
            "{f}"
        );
    }
}

#[test]
fn a_missing_source_is_an_error_naming_it() {
    let src = scratch("fl_src_missing");
    let e = build::build_from(&src, &scratch("fl_out_missing")).unwrap_err();
    assert!(e.contains("layout.toml"), "{e}");
}

// --- Contract C1..C7 ------------------------------------------------------------

#[test]
fn meta_and_cabinet_switches() {
    let t = manifest();
    assert_eq!(t.meta.id, "feature_lab");
    assert_eq!(t.meta.script_entry, "rules.lua");
    assert_eq!(t.meta.pitch_deg, 6.5);
    assert_eq!(t.meta.players_max, 1);
    let c = t.cabinet_switches;
    let cab = [
        c.left_flipper,
        c.right_flipper,
        c.left_upper,
        c.right_upper,
        c.action,
        c.start,
        c.launch,
        c.tilt,
    ];
    let mut all: Vec<u16> = cab.to_vec();
    all.extend(t.switches.iter().map(|s| s.number));
    let n = all.len();
    all.sort_unstable();
    all.dedup();
    assert_eq!(all.len(), n, "switch numbers are unique");
}

#[test]
fn flippers_are_the_straight_left_the_banana_right_and_one_backbox_flipper() {
    let t = manifest();
    let ids: Vec<&str> = t.flippers.iter().map(|f| f.id.as_str()).collect();
    assert_eq!(
        ids,
        ["lower_left", "lower_right", "tablegen:backbox.flippers[0]"]
    );
    let button = |f: &pin_core::schema::table::FlipperDef| match &f.staging {
        FlipperStagingDef::DedicatedButton { input } => format!("{input:?}"),
        other => panic!("{other:?}"),
    };
    let l = &t.flippers[0];
    assert!(matches!(l.shape, FlipperShapeDef::StraightCapsule { .. }));
    assert_eq!(l.collision_layer, 0);
    assert!(button(l).contains("LeftFlipper"), "{}", button(l));
    let r = &t.flippers[1];
    assert_eq!(r.collision_layer, 0);
    assert!(button(r).contains("RightFlipper"), "{}", button(r));
    let FlipperShapeDef::CurvedBanana {
        length,
        arc_radius,
        arc_sweep_deg,
        tube_radius,
    } = r.shape
    else {
        panic!("the right flipper is a Banana");
    };
    // Tested envelope (w7pre-banana-pose) and the pin_assets sweep lint.
    assert!((60.0..=90.0).contains(&length));
    assert!((6.5..=11.0).contains(&tube_radius));
    assert!((20.0..=60.0).contains(&arc_sweep_deg) && arc_sweep_deg > 0.0);
    assert!(arc_radius >= 1.1 * length / 2.0);
    let implied = 2.0 * (length / (2.0 * arc_radius)).asin().to_degrees();
    assert!(
        (arc_sweep_deg - implied).abs() <= 1.0,
        "{arc_sweep_deg} vs {implied}"
    );
    for a in [r.rest_angle_deg, r.rest_angle_deg + r.sweep_angle_deg] {
        assert!((150.0..=240.0).contains(&a), "{a}");
    }
    let b = &t.flippers[2];
    assert_eq!(b.collision_layer, 4);
    assert!(button(b).contains("LeftUpper"), "{}", button(b));
    // The loader's lint: no warning about the Banana.
    let table = load_table(&Vfs::open(bundle()).unwrap()).unwrap();
    assert!(
        table
            .warnings
            .iter()
            .all(|w| !w.to_string().contains("flippers[1]")),
        "{:?}",
        table.warnings
    );
}

#[test]
fn balls_dispensers_magnet_and_backbox() {
    let t = manifest();
    assert_eq!(
        t.balls.profiles,
        [
            "standard_steel",
            "low_rem_steel",
            "uv_globall",
            "jumbo_xl_resin"
        ]
    );
    let r = &t.balls.restrict;
    assert_eq!(r.len(), 1);
    assert_eq!(r[0].profile, "jumbo_xl_resin");
    assert_eq!(r[0].layers, [0]);
    assert!(r[0].area.len() >= 3);
    let disp = |id: &str| {
        t.dispensers
            .iter()
            .find(|d| d.id == id)
            .unwrap_or_else(|| panic!("{id}"))
    };
    assert_eq!(disp("feed").contents, ["standard_steel"]);
    assert_eq!(
        disp("magnet_feed").contents,
        ["standard_steel", "low_rem_steel"]
    );
    assert_eq!(disp("glo_feed").contents, ["uv_globall"]);
    let xl = disp("xl_feed");
    assert_eq!(xl.contents, ["jumbo_xl_resin"]);
    assert!(inside_even_odd(
        &r[0].area,
        [xl.exit_position[0], xl.exit_position[1]]
    ));
    for d in &t.dispensers {
        for p in &d.contents {
            assert!(t.balls.profiles.contains(p), "{} serves {p}", d.id);
        }
    }
    assert_eq!(t.magnets.len(), 1);
    let m = &t.magnets[0];
    assert_eq!(m.id, "hold_magnet");
    let tau = m.tau_ms.expect("tau_ms tuned");
    assert!((20.0..=200.0).contains(&tau));
    let opto = t.switches.iter().find(|s| s.id == "magnet_opto").unwrap();
    let p = opto.position.unwrap();
    assert!((p[0] - m.position[0]).abs() < 1.0 && (p[1] - m.position[1]).abs() < 1.0);
    let vuk = t
        .mechs
        .iter()
        .find(|m| m.id == "tablegen:backbox.vuk")
        .expect("backbox VUK");
    assert_eq!(vuk.kind, MechKind::Vuk);
    let tr: Vec<&str> = t
        .layer_transitions
        .iter()
        .map(|t| t.id.as_str())
        .filter(|id| id.starts_with("tablegen:backbox."))
        .collect();
    assert_eq!(tr, ["tablegen:backbox.entry", "tablegen:backbox.drain"]);
    let bg = std::fs::read_to_string(bundle().join("backglass.toml")).unwrap();
    assert!(bg.contains("scene = \"backbox.glb\""));
}

#[test]
fn modes_shots_and_persist_keys() {
    let t = manifest();
    let modes: Vec<(&str, &str)> = t
        .modes
        .iter()
        .map(|m| (m.id.as_str(), m.start_scene.as_str()))
        .collect();
    let want: Vec<(String, String)> = MODES
        .iter()
        .map(|m| ((*m).to_owned(), format!("scene_{m}")))
        .collect();
    assert_eq!(
        modes,
        want.iter()
            .map(|(a, b)| (a.as_str(), b.as_str()))
            .collect::<Vec<_>>()
    );
    assert!(t.shots.len() >= 5);
    let declared: Vec<&str> = t.switches.iter().map(|s| s.id.as_str()).collect();
    for s in &t.shots {
        for sw in &s.sequence {
            assert!(declared.contains(&sw.as_str()), "{}: {sw}", s.id);
        }
    }
    let keys = &t.persist.as_ref().unwrap().keys;
    for k in [
        "game_state",
        "ball",
        "mode_sel",
        "games_played",
        "high_score",
        "cmp_release_ms_standard",
        "cmp_release_ms_low_rem",
    ] {
        assert!(keys.iter().any(|x| x == k), "{k}");
    }
    for m in MODES {
        assert!(keys.iter().any(|x| *x == format!("mode:{m}")), "{m}");
    }
}

// --- R8 / R5 negative: clearance ---------------------------------------------------

#[test]
fn clearance_validates_every_declared_ball() {
    let report = validate_bundle(&Vfs::open(bundle()).unwrap()).unwrap();
    assert!(report.errors.is_empty(), "{:#?}", report.errors);
    let dir = bundle().to_path_buf();
    on_big_stack(move || {
        load_validated(&dir, SimConfig::default()).expect("load_validated");
    });
}

#[test]
fn without_the_xl_restriction_validation_names_the_feature() {
    let dir = bundle_copy("fl_unrestricted");
    let mut t = manifest();
    t.balls.restrict.clear();
    std::fs::write(dir.join("table.toml"), t.to_toml_string().unwrap()).unwrap();
    let report = validate_bundle(&Vfs::open(&dir).unwrap()).unwrap();
    assert!(!report.errors.is_empty());
    assert!(report.errors.iter().all(|e| e.profile == "jumbo_xl_resin"));
    assert!(
        report
            .errors
            .iter()
            .any(|e| e.name == "lower_left / lower_right"),
        "{:#?}",
        report.errors
    );
    let e = on_big_stack(move || match load_validated(&dir, SimConfig::default()) {
        Ok(_) => panic!("the unrestricted XL ball must be refused"),
        Err(e) => e,
    });
    assert!(e.contains("jumbo_xl_resin"), "{e}");
    assert!(e.contains("lower_left / lower_right"), "{e}");
}

#[test]
fn glass_clears_the_playfield_geometry() {
    let vfs = Vfs::open(bundle()).unwrap();
    let scene = load_gltf(&vfs, "playfield.glb").unwrap();
    let max_z = scene
        .meshes
        .iter()
        .flat_map(|m| m.positions.iter().map(|p| p[2]))
        .fold(f32::MIN, f32::max)
        * 1.0e3;
    let glass = std::fs::read_to_string(bundle().join("glass.toml")).unwrap();
    let h: f32 = glass
        .lines()
        .find_map(|l| l.strip_prefix("height = "))
        .unwrap()
        .parse()
        .unwrap();
    assert!(h >= max_z, "glass {h} mm under geometry {max_z} mm");
    // Seal rule: the gap over the walls is under one ball diameter.
    assert!(h - max_z < 27.0, "{h} - {max_z}");
}

/// C5: the backbox VUK keeps the generator's exit (layer 4, just above the
/// entry hole, inside the box); only its eject is set.
#[test]
fn the_vuk_keeps_its_generated_exit_on_layer_4() {
    let t = manifest();
    let src = feature_lab_src();
    let read = |f: &str| std::fs::read_to_string(src.join(f)).unwrap();
    let base = TableManifest::from_toml_str(&read("table.toml")).unwrap();
    let playfield =
        pin_tablegen::generate(&parse_layout(&read("layout.toml")).unwrap(), &base).unwrap();
    let bb = parse_backbox_layout(&read("backbox_layout.toml"))
        .unwrap()
        .backbox;
    let generated = generate_backbox(&bb, &playfield.table).unwrap();
    let find = |m: &TableManifest| {
        m.mechs
            .iter()
            .find(|x| x.id == "tablegen:backbox.vuk")
            .cloned()
            .unwrap()
    };
    let (got, gen_vuk) = (find(&t), find(&generated.table));
    assert_eq!(got.layer.unwrap_or(0), 0);
    assert_eq!(got.exit_layer, Some(4));
    assert_eq!(got.exit_layer, gen_vuk.exit_layer);
    assert_eq!(got.exit_offset, gen_vuk.exit_offset);
    assert_eq!(got.position, gen_vuk.position);
    for z in &generated.table.layer_transitions {
        assert!(t.layer_transitions.contains(z), "generated {} kept", z.id);
    }
    // The exit is over the entry hole, inside the box, above the shelf.
    let frame = BackboxFrame::new(bb.origin, t.meta.pitch_deg);
    let p = got.position.unwrap();
    let o = got.exit_offset.unwrap();
    let e = [
        (p[0] + o[0]) * 1.0e-3 - frame.origin_m.x,
        (p[1] + o[1]) * 1.0e-3 - frame.origin_m.y,
        (p[2] + o[2]) * 1.0e-3 - frame.origin_m.z,
    ];
    let (n, up) = (frame.normal(), frame.up());
    let u = e[0] * 1.0e3;
    let depth = -(e[0] * n.x + e[1] * n.y + e[2] * n.z) * 1.0e3;
    let v = (e[0] * up.x + e[1] * up.y + e[2] * up.z) * 1.0e3;
    assert!((u - bb.entry.at).abs() <= 0.5 * bb.entry.width, "u {u}");
    assert!(depth > 13.0 && depth < bb.depth - 13.0, "depth {depth}");
    assert!(v > 13.5 && v < 30.0, "v {v}");
    // The eject is set and points up the box.
    let (eject, angle, kick) = vuk_eject(&frame);
    assert_eq!(got.eject_speed_m_s, Some(eject));
    assert_eq!(got.angle_deg, Some(angle));
    assert_eq!(got.kick_speed_m_s, Some(kick));
    assert!(eject > 1.5);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, .. ProptestConfig::default() })]

    #[test]
    fn parse_hex_round_trips(r in any::<u8>(), g in any::<u8>(), b in any::<u8>()) {
        prop_assert_eq!(parse_hex(&format!("#{r:02x}{g:02x}{b:02X}")), Ok([r, g, b]));
    }

    #[test]
    fn parse_hex_rejects_other_strings(s in "\\PC{0,9}") {
        let ok = s.trim_start_matches('#').len() == 6
            && s.trim_start_matches('#').chars().all(|c| c.is_ascii_hexdigit());
        prop_assert_eq!(parse_hex(&s).is_ok(), ok, "{:?}", s);
    }

    /// The even-odd test against an axis-aligned rectangle.
    #[test]
    fn even_odd_matches_a_rectangle(
        x0 in -100.0f32..100.0, y0 in -100.0f32..100.0,
        w in 1.0f32..200.0, h in 1.0f32..200.0,
        px in -300.0f32..300.0, py in -300.0f32..300.0,
    ) {
        let poly = [[x0, y0], [x0 + w, y0], [x0 + w, y0 + h], [x0, y0 + h]];
        let inside = px > x0 && px < x0 + w && py > y0 && py < y0 + h;
        let on_edge = (px - x0).abs() < 1e-3 || (px - x0 - w).abs() < 1e-3
            || (py - y0).abs() < 1e-3 || (py - y0 - h).abs() < 1e-3;
        if !on_edge {
            prop_assert_eq!(inside_even_odd(&poly, [px, py]), inside);
        }
    }

    /// The VUK's eject velocity lies in the backbox plane (no component
    /// towards the glass) for any entry hole and pitch.
    #[test]
    fn vuk_eject_stays_in_the_backbox_plane(pitch in 0.0f32..15.0) {
        let frame = BackboxFrame::new([257.0, 900.0, 110.0], pitch);
        let (eject, angle, kick) = vuk_eject(&frame);
        let a = angle.to_radians();
        let v = [kick * a.cos(), kick * a.sin(), eject];
        let n = frame.normal();
        let towards_glass = v[0] * n.x + v[1] * n.y + v[2] * n.z;
        prop_assert!(towards_glass.abs() < 1.0e-4, "{}", towards_glass);
        let up = frame.up();
        let rise = v[0] * up.x + v[1] * up.y + v[2] * up.z;
        prop_assert!((rise - VUK_RISE_M_S).abs() < 1.0e-4, "{}", rise);
        prop_assert!((v[0] - VUK_LATERAL_M_S).abs() < 1.0e-4, "{}", v[0]);
        prop_assert!(eject > 0.0 && kick > 0.0);
    }
}

/// The VUK eject angle and kick are pinned to their exact f32 bits: the
/// builder uses `libm` (not the platform's atan2f/hypotf), so the generated
/// bundle is the same on every glibc.
#[test]
fn vuk_eject_bits_are_platform_independent() {
    let t = manifest();
    let read = |f: &str| std::fs::read_to_string(feature_lab_src().join(f)).unwrap();
    let bb = parse_backbox_layout(&read("backbox_layout.toml"))
        .unwrap()
        .backbox;
    let frame = BackboxFrame::new(bb.origin, t.meta.pitch_deg);
    let (_, angle, kick) = vuk_eject(&frame);
    assert_eq!(angle.to_bits(), 0x4208BD8C, "angle {angle}");
    assert_eq!(kick.to_bits(), 0x3ED8A09F, "kick {kick}");
}
