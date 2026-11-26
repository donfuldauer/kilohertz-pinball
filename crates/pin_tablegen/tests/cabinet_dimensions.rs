//! KP-029 (#110): the cabinet model's dimensions follow the geometry
//! (REQ-CAB-001, REQ-CAB-003, REQ-CAB-007): body, wall tops, lockdown bar,
//! glass, legs and levelers within 1 mm for each preset; legs stand on the
//! floor plane z = 0; nothing is hard-coded; legs are independent.

#[path = "cabinet_common.rs"]
mod common;

use common::*;
use pin_core::schema::cabinet::CabinetGeometry;

const BODY: [&str; 5] = [
    "cabinet.body.bottom",
    "cabinet.body.side_left",
    "cabinet.body.side_right",
    "cabinet.body.front",
    "cabinet.body.rear",
];

/// Checks every dimension guarantee of the module docs on `g`.
fn check_dimensions(g: &CabinetGeometry, what: &str) {
    let scene = load(&generate(g).glb);
    let b = &g.body;
    let w = f64::from(b.width) / 1000.0;
    let l = f64::from(b.length) / 1000.0;
    // Body width and length: the union of the body panels.
    let (lo, hi) = bounds(&scene, &BODY);
    close(lo[0], -w / 2.0, MM, &format!("{what} body -x"));
    close(hi[0], w / 2.0, MM, &format!("{what} body +x"));
    close(lo[1], 0.0, MM, &format!("{what} body front y"));
    close(hi[1], l, MM, &format!("{what} body rear y"));
    // Body bottom underside follows body_bottom_height_mm.
    close(
        lo[2],
        pin_core::fmath::min_f64(
            g.body_bottom_height_mm(0.0),
            g.body_bottom_height_mm(f64::from(b.length)),
        ) / 1000.0,
        MM,
        &format!("{what} body bottom"),
    );
    // Front and rear wall tops.
    let (_, fhi) = bounds(&scene, &["cabinet.body.front"]);
    close(
        fhi[2],
        g.floor_height_mm(0.0, f64::from(b.front_height)) / 1000.0,
        MM,
        &format!("{what} front wall top"),
    );
    let (_, rhi) = bounds(&scene, &["cabinet.body.rear"]);
    close(
        rhi[2],
        g.floor_height_mm(f64::from(b.length), f64::from(b.rear_height)) / 1000.0,
        MM,
        &format!("{what} rear wall top"),
    );
    let (_, shi) = bounds(
        &scene,
        &["cabinet.body.side_left", "cabinet.body.side_right"],
    );
    close(
        shi[2],
        f64::from(rhi[2]),
        MM,
        &format!("{what} side wall top"),
    );
    // Lockdown bar top.
    let (_, bhi) = bounds(&scene, &["cabinet.lockdown_bar"]);
    close(
        bhi[2],
        g.lockdown_bar_top_height_mm() / 1000.0,
        MM,
        &format!("{what} lockdown bar top"),
    );
    // Glass width, under the bar's top at the front.
    let (glo, ghi) = bounds(&scene, &["cabinet.glass"]);
    close(
        ghi[0] - glo[0],
        f64::from(g.glass_panel.width) / 1000.0,
        MM,
        &format!("{what} glass width"),
    );
    assert!(
        glo[2] < bhi[2],
        "{what}: glass front edge under the bar top"
    );
    // Legs: shaft z extent = length, from the extension; assemblies on the
    // floor.
    for (name, leg) in g.legs.all() {
        let [shaft, bracket, leveler] = leg_nodes(name);
        let (slo, shi) = bounds(&scene, &[&shaft]);
        close(
            shi[2] - slo[2],
            f64::from(leg.length) / 1000.0,
            MM,
            &format!("{what} {name} shaft length"),
        );
        close(
            slo[2],
            f64::from(leg.leveler.extension) / 1000.0,
            MM,
            &format!("{what} {name} shaft bottom"),
        );
        let (llo, lhi) = bounds(&scene, &[&leveler]);
        close(
            llo[2],
            0.0,
            1e-4,
            &format!("{what} {name} leveler on the floor"),
        );
        assert!(
            lhi[2] + 1e-4 >= slo[2],
            "{what} {name}: leveler reaches the shaft"
        );
        let (alo, _) = bounds(&scene, &[&shaft, &bracket, &leveler]);
        close(
            alo[2],
            0.0,
            1e-4,
            &format!("{what} {name} assembly on the floor"),
        );
    }
    let (all_lo, _) = scene_bounds(&scene);
    close(all_lo[2], 0.0, 1e-4, &format!("{what} global min z"));
}

/// Acceptance (REQ-CAB-001, REQ-CAB-007): width, length, wall tops, body
/// bottom, lockdown bar top, glass width and leg lengths match each preset
/// within 1 mm; legs touch the floor plane.
#[test]
fn presets_match_within_one_millimetre() {
    for id in PRESETS {
        check_dimensions(&preset(id), id);
    }
}

/// Acceptance: the Appendix E floor heights of the presets (body bottom,
/// lockdown bar top) and leg lengths, as numbers.
#[test]
fn appendix_e_heights() {
    let scene = load(&generate(&preset("classic_standard")).glb);
    let (_, hi) = bounds(&scene, &["cabinet.lockdown_bar"]);
    close(hi[2], 0.910, MM, "classic lockdown bar top 910 mm");
    let (lo, _) = bounds(&scene, &["cabinet.body.side_left"]);
    close(lo[2], 0.490, MM, "classic body bottom 490 mm");
    let (lo, hi) = bounds(&scene, &["cabinet.leg.front_left"]);
    close(hi[2] - lo[2], 0.724, MM, "classic leg 724 mm");
    let (lo, hi) = bounds(&scene, &["cabinet.glass"]);
    close(hi[0] - lo[0], 0.533, MM, "classic glass 533 mm");
    let scene = load(&generate(&preset("classic_widebody")).glb);
    let (lo, hi) = bounds(
        &scene,
        &["cabinet.body.side_left", "cabinet.body.side_right"],
    );
    close(hi[0] - lo[0], 0.630, MM, "widebody 630 mm");
    let scene = load(&generate(&preset("modern_standard")).glb);
    let (_, hi) = bounds(&scene, &["cabinet.lockdown_bar"]);
    close(hi[2], 0.914, MM, "modern lockdown bar top 914 mm");
    let (lo, _) = bounds(&scene, &["cabinet.body.side_left"]);
    close(lo[2], 0.481, MM, "modern body bottom 481 mm");
}

/// REQ-CAB-007: the levelers stand on z = 0 at extension_min, the preset
/// extension and extension_max, on every leg independently.
#[test]
fn levelers_touch_the_floor_over_their_range() {
    for id in PRESETS {
        let base = preset(id);
        let lv = &base.legs.front_left.leveler;
        for ext in [lv.extension_min, lv.extension, lv.extension_max] {
            let mut g = base.clone();
            for leg in [
                &mut g.legs.front_left,
                &mut g.legs.front_right,
                &mut g.legs.rear_left,
                &mut g.legs.rear_right,
            ] {
                leg.leveler.extension = ext;
            }
            check_dimensions(&g, &format!("{id} extension {ext}"));
        }
        // One leg out, the others in.
        let mut g = base.clone();
        g.legs.rear_right.leveler.extension = lv.extension_max;
        check_dimensions(&g, &format!("{id} rear_right out"));
    }
}

/// REQ-CAB-003: nothing is hard-coded: body width, length and heights,
/// the lockdown bar height and every control position set in a
/// `cabinet.toml` move the model.
#[test]
fn model_follows_cabinet_toml() {
    let g = from_toml(
        r#"
[body]
width = 600.0
length = 1400.0
front_height = 420.0
rear_height = 640.0
lockdown_bar = { height = 35.0, profile = "flat" }
side_rail = { profile = "angled" }

[controls]
left_flipper_button = [-300.0, 150.0, 380.0]
right_flipper_button = [300.0, 160.0, 390.0]
start_button = [-150.0, 0.0, 330.0]
action_button = [40.0, 0.0, 300.0]
plunger = [250.0, 0.0, 250.0]
coin_door = { centre = [10.0, 0.0, 200.0], width = 280.0, height = 300.0 }
"#,
    );
    assert_eq!(g.body.width, 600.0);
    check_dimensions(&g, "override");
    let out = generate(&g);
    let scene = load(&out.glb);
    let base = load(&generate(&preset("classic_standard")).glb);
    // Every body-fixed node moved or changed.
    for name in [
        "cabinet.body.side_left",
        "cabinet.lockdown_bar",
        "cabinet.rail.left",
        "cabinet.coin_door",
        "cabinet.plunger.housing",
    ] {
        assert!(
            mesh(&scene, name).positions != mesh(&base, name).positions,
            "{name}"
        );
    }
    // The coin door is centred on its position, width x height.
    let cd = &g.controls.coin_door;
    let (lo, hi) = bounds(&scene, &["cabinet.coin_door"]);
    close(
        (lo[0] + hi[0]) * 0.5,
        f64::from(cd.centre[0]) / 1000.0,
        MM,
        "coin door x",
    );
    close(
        hi[0] - lo[0],
        f64::from(cd.width) / 1000.0,
        MM,
        "coin door width",
    );
    close(
        hi[2] - lo[2],
        f64::from(cd.height) / 1000.0,
        MM,
        "coin door height",
    );
    close(
        (lo[2] + hi[2]) * 0.5,
        g.floor_height_mm(0.0, f64::from(cd.centre[2])) / 1000.0,
        MM,
        "coin door z",
    );
    assert!(
        lo[1] < 0.0 && hi[1] <= 1e-6,
        "coin door proud of the front face"
    );
    // The plunger housing is centred on the plunger position and runs in.
    let p = g.controls.plunger;
    let (lo, hi) = bounds(&scene, &["cabinet.plunger.housing"]);
    close(
        (lo[0] + hi[0]) * 0.5,
        f64::from(p[0]) / 1000.0,
        MM,
        "housing x",
    );
    close(
        (lo[2] + hi[2]) * 0.5,
        g.floor_height_mm(f64::from(p[1]), f64::from(p[2])) / 1000.0,
        MM,
        "housing z",
    );
    assert!(lo[1] < 0.0 && hi[1] > 0.05, "housing runs into the body");
    // The buttons sit at their positions (checked as pivots in
    // cabinet_bundle.rs); the side buttons protrude outward, the front
    // ones toward the player, the plunger knob toward the player.
    let (lo, _) = bounds(&scene, &["cabinet.button.left_flipper"]);
    assert!(f64::from(lo[0]) < f64::from(g.controls.left_flipper_button[0]) / 1000.0 - 0.01);
    let (_, hi) = bounds(&scene, &["cabinet.button.right_flipper"]);
    assert!(f64::from(hi[0]) > f64::from(g.controls.right_flipper_button[0]) / 1000.0 + 0.01);
    for n in [
        "cabinet.button.start",
        "cabinet.button.action",
        "cabinet.plunger.rod",
    ] {
        let (lo, _) = bounds(&scene, &[n]);
        assert!(lo[1] < -0.01, "{n} protrudes toward the player");
    }
}

/// REQ-CAB-007: legs are set independently. Restyling only rear_left
/// (section shape, finish, colour, bracket, bolt) changes only rear_left's
/// nodes and material; changing its length as well changes the other
/// legs' nodes and materials not at all (the body follows the new rear
/// pair height through `body_bottom_height_mm`).
#[test]
fn legs_are_independent() {
    let base_g = preset("classic_standard");
    let base_out = generate(&base_g);
    let base = load(&base_out.glb);
    let style = r##"
[legs.rear_left]
finish = "powder_coat"
colour = "#3060a0"
bracket = "flush_plate"
bolt = "button_head"
section = { shape = "round" }
"##;
    let rl = leg_nodes("rear_left");
    for (extra, length_changed) in [
        (style.to_owned(), false),
        (format!("{style}length = 690.0\n"), true),
    ] {
        let g = from_toml(&extra);
        assert_eq!(g.legs.front_left, base_g.legs.front_left);
        let scene = load(&generate(&g).glb);
        for (a, b) in scene.meshes.iter().zip(&base.meshes) {
            assert_eq!(a.name, b.name);
            let own = rl.contains(&a.name);
            let is_leg = a.name.starts_with("cabinet.leg.");
            if a.name == rl[0] || a.name == rl[1] {
                // Shaft (shape, length) and bracket (style, bolt) change.
                assert!(a.positions != b.positions, "{} changes", a.name);
            } else if own {
                // The leveler is not restyled here; it may stay.
            } else if is_leg || !length_changed {
                assert!(a.positions == b.positions, "{} unchanged", a.name);
                assert_eq!(a.transform, b.transform, "{} unchanged", a.name);
            }
        }
        for (a, b) in scene.materials.iter().zip(&base.materials) {
            if a.name == "cabinet.leg.rear_left" {
                assert_ne!(a, b);
            } else {
                assert_eq!(a, b, "{}", a.name);
            }
        }
        if length_changed {
            let (lo, hi) = bounds(&scene, &[&rl[0]]);
            close(
                hi[2] - lo[2],
                0.690,
                MM,
                "rear_left shaft follows its length",
            );
        }
    }
}
