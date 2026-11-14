//! Subphase 2.8: the clearance validator on generated bundles (PLAN 2.8
//! deliverable 2 and verification 2), its CLI, and a never-panics sweep.

mod support;

use pin_assets::{GltfScene, LoadedTable, Vfs};
use pin_core::schema::LayoutManifest;
use pin_core::schema::table::BallRestriction;
use pin_tablegen::clearance::{
    ClearanceError, ClearanceReport, Feature, run_validate_cli, validate_bundle,
    validate_clearance, validate_clearance_diameters,
};
use pin_tablegen::layout::{generate, write_bundle};
use std::path::PathBuf;
use support::*;

const XL: &str = "jumbo_xl_resin";
const STEEL: &str = "standard_steel";
const MINI: &str = "mini_steel";

/// A layout with just the playfield (514 × 1070 mm, 28 mm walls) and `extra`.
fn layout_with(extra: &str) -> LayoutManifest {
    let text = format!(
        "schema_version = 1\n[playfield]\nwidth = 514.0\nlength = 1070.0\nwall_height = 28.0\n{extra}"
    );
    LayoutManifest::from_toml_str(&text).unwrap_or_else(|e| panic!("{e}\n{text}"))
}

/// The apex base without its flipper, splines, transitions and mechs, with
/// `profiles` / `restrict`, plus the `extra` TOML arrays.
fn base_with(profiles: &[&str], restrict: Vec<BallRestriction>, extra: &str) -> String {
    let mut base = apex_base();
    base.balls.profiles = profiles.iter().map(|s| (*s).to_owned()).collect();
    base.balls.restrict = restrict;
    base.flippers.clear();
    base.splines.clear();
    base.layer_transitions.clear();
    base.mechs.clear();
    format!("{}\n{extra}", base.to_toml_string().unwrap())
}

fn restrict(profile: &str, layers: &[u8], area: &[[f32; 2]]) -> BallRestriction {
    BallRestriction {
        profile: profile.to_owned(),
        layers: layers.to_vec(),
        area: area.to_vec(),
    }
}

fn build(layout: &LayoutManifest, base_toml: &str, tag: &str) -> (PathBuf, LoadedTable, GltfScene) {
    let base = pin_core::schema::TableManifest::from_toml_str(base_toml)
        .unwrap_or_else(|e| panic!("{e}\n{base_toml}"));
    let generated = generate(layout, &base).unwrap_or_else(|e| panic!("{e}"));
    load(&generated, tag)
}

fn check(layout: &LayoutManifest, base_toml: &str, tag: &str) -> ClearanceReport {
    let (dir, table, scene) = build(layout, base_toml, tag);
    let report = validate_clearance(&table, &scene);
    std::fs::remove_dir_all(dir).unwrap();
    report
}

fn of<'a>(r: &'a ClearanceReport, f: Feature, profile: &str) -> Vec<&'a ClearanceError> {
    r.errors
        .iter()
        .filter(|e| e.feature == f && e.profile == profile)
        .collect()
}

fn near(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

// ------------------------------------------------------------ verification 2

/// The open area of the Appendix A.3 main deck: right of the inlane and
/// below the upper deck (which starts at y = 700 mm), kept a ball radius
/// plus 1 mm clear of the deck edge (39.7 + 1 mm: y ≤ 659.3 mm), so the
/// XL ball never reaches under or into the deck sheet.
const OPEN_AREA: [[f32; 2]; 4] = [[100.0, 0.0], [514.0, 0.0], [514.0, 655.0], [100.0, 655.0]];

#[test]
fn v2_xl_rejected_under_a_65_mm_deck_and_accepted_in_the_open_area() {
    // The Appendix A.3 layout: a 65 mm upper deck (layer 1) over the main deck.
    let layout = layout("apex_layout.toml");
    let deck = &layout.decks[0];
    assert_eq!((deck.layer, deck.z), (1, 65.0));
    let mut base = apex_base();
    base.balls.profiles = vec![STEEL.into(), XL.into()];
    base.balls.restrict.clear();
    let unrestricted = base.to_toml_string().unwrap();

    let (dir, table, scene) = build(&layout, &unrestricted, "v2_open");
    let r = validate_clearance(&table, &scene);
    assert!(!r.is_ok());
    let deck_errors = of(&r, Feature::DeckHeadroom, XL);
    assert_eq!(deck_errors.len(), 1, "{:#?}", r.errors);
    let e = deck_errors[0];
    assert_eq!(e.file, "playfield.glb");
    assert_eq!(e.name, "decks[0]");
    let idx = scene
        .meshes
        .iter()
        .position(|m| m.name == "decks[0]")
        .unwrap();
    assert_eq!(e.path, format!("meshes[{idx}]"));
    assert!(near(e.actual_mm, 65.0, 1e-3), "{}", e.actual_mm);
    assert!(near(e.required_mm, 80.4, 1e-3), "{}", e.required_mm);
    assert!(
        e.message.contains("decks[0]") && e.message.contains(XL),
        "{}",
        e.message
    );
    // The standard ball fits everywhere.
    assert!(r.errors.iter().all(|e| e.profile == XL), "{:#?}", r.errors);
    // The same bundle through the bundle entry point.
    let vfs = Vfs::open(&dir).unwrap();
    assert_eq!(validate_bundle(&vfs).unwrap(), r);
    std::fs::remove_dir_all(dir).unwrap();

    // Layer 0 alone (Appendix A's restriction) still runs under the deck.
    base.balls.restrict = vec![restrict(XL, &[0], &[])];
    let r = check(&layout, &base.to_toml_string().unwrap(), "v2_layer0");
    assert_eq!(
        of(&r, Feature::DeckHeadroom, XL).len(),
        1,
        "{:#?}",
        r.errors
    );
    // Not on the layer-2 wireform any more.
    assert!(of(&r, Feature::Wireform, XL).is_empty());

    // Restricted to the main deck's open area: accepted.
    base.balls.restrict = vec![restrict(XL, &[0], &OPEN_AREA)];
    let r = check(&layout, &base.to_toml_string().unwrap(), "v2_area");
    assert!(r.is_ok(), "{:#?}", r.errors);
    // The ramp entry (0 -> 2) lies in that area: a warning, not an error.
    assert!(
        r.warnings
            .iter()
            .any(|w| w.feature == Feature::Restriction && w.path == "layer_transitions[0]"),
        "{:#?}",
        r.warnings
    );
}

/// Tester defect D1: a restriction area of any size lying under the deck
/// is rejected, wherever it is (the area itself is sampled, not only a
/// grid over the deck).
#[test]
fn small_areas_under_the_deck_are_rejected() {
    let layout = layout("apex_layout.toml");
    let g = |area: &[[f32; 2]]| {
        let mut base = apex_base();
        base.balls.restrict = vec![restrict(XL, &[0], area)];
        base.to_toml_string().unwrap()
    };
    let (dir, mut table, scene) = build(&layout, &g(&OPEN_AREA), "small_areas");
    for (cx, cy) in [
        (351.9f32, 803.3f32),
        (420.1, 955.5),
        (300.0, 750.0),
        (261.0, 701.0),
    ] {
        for side in [1.0f32, 3.0, 8.0, 20.0] {
            let h = 0.5 * side;
            table.manifest.balls.restrict[0].area = vec![
                [cx - h, cy - h],
                [cx + h, cy - h],
                [cx + h, cy + h],
                [cx - h, cy + h],
            ];
            let r = validate_clearance(&table, &scene);
            assert_eq!(
                of(&r, Feature::DeckHeadroom, XL).len(),
                1,
                "{side} mm square at ({cx}, {cy})"
            );
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

/// Reviewer B2: the ball is a sphere. An area ending short of the deck edge
/// still lets the XL ball reach into the deck sheet; walls along the edge
/// keep it out; a sloped ceiling is measured along its normal.
#[test]
fn deck_edges_and_slopes_are_tested_with_the_whole_ball() {
    let layout = layout("apex_layout.toml");
    let (dir, mut table, scene) = build(
        &layout,
        &{
            let mut base = apex_base();
            base.balls.restrict = vec![restrict(XL, &[0], &OPEN_AREA)];
            base.to_toml_string().unwrap()
        },
        "deck_edges",
    );
    for (top, rejected) in [(699.0f32, true), (690.0, true), (670.0, true), (659.0, false), (600.0, false)] {
        table.manifest.balls.restrict[0].area = vec![[100.0, 0.0], [514.0, 0.0], [514.0, top], [100.0, top]];
        let r = validate_clearance(&table, &scene);
        assert_eq!(!of(&r, Feature::DeckHeadroom, XL).is_empty(), rejected, "area to y = {top}: {:#?}", r.errors);
    }
    std::fs::remove_dir_all(dir).unwrap();
    // A wall along the deck edge (layer 0) keeps the ball's centre r away.
    let walled = layout_with(
        "[[decks]]\nlayer = 1\nz = 65.0\noutline = [[0.0, 700.0], [514.0, 700.0], [514.0, 1070.0], [0.0, 1070.0]]\nmaterial = \"acrylic\"\n\
         [[walls]]\npoints = [[0.0, 700.0], [514.0, 700.0]]\nmaterial = \"wood\"\n",
    );
    let area = [[0.0, 0.0], [514.0, 0.0], [514.0, 699.0], [0.0, 699.0]];
    let r = check(&walled, &base_with(&[STEEL], vec![restrict(XL, &[0], &area)], ""), "walled_edge");
    assert!(of(&r, Feature::DeckHeadroom, XL).is_empty(), "{:#?}", r.errors);
}

/// A hand-built scene: the generated floor plus one ceiling triangle on
/// layer 1, high (60 mm) along one side and dipping to `low_mm` at its
/// third vertex.
fn dipping_scene(base: &GltfScene, low_mm: f32) -> GltfScene {
    let mut scene = base.clone();
    let mut m = scene.meshes.iter().find(|m| m.name == "floor").unwrap().clone();
    m.name = "ceiling".into();
    m.positions = vec![[0.300, 0.400, 0.060], [0.400, 0.400, 0.060], [0.350, 0.500, low_mm * 1e-3]];
    m.normals = vec![[0.0, 0.0, 1.0]; 3];
    m.uvs.clear();
    m.indices = vec![0, 1, 2];
    m.extras.collision_layer = Some(1);
    scene.meshes.push(m);
    scene
}

/// Reviewer B2 (c): a ceiling whose low point is a vertex is found (the
/// ball is tested as a sphere against the triangle, including its corners).
#[test]
fn a_dipping_ceiling_vertex_is_found() {
    let (dir, table, scene) = build(&layout_with(""), &base_with(&[STEEL], vec![], ""), "dip");
    for (low, rejected) in [(22.0f32, true), (27.5, true), (29.0, false), (40.0, false)] {
        let s = dipping_scene(&scene, low);
        let r = validate_clearance_diameters(&table, &s, &[0.027]);
        let found: Vec<_> = r
            .errors
            .iter()
            .filter(|e| e.feature == Feature::DeckHeadroom && e.name == "ceiling")
            .collect();
        assert_eq!(!found.is_empty(), rejected, "low {low}: {:#?}", r.errors);
        if let Some(e) = found.first() {
            assert!(near(e.actual_mm, low, 0.3), "{} vs {low}", e.actual_mm);
        }
    }
    std::fs::remove_dir_all(dir).unwrap();
}

// ------------------------------------------------------------ feature checks

#[test]
fn lanes_are_measured_rail_to_rail() {
    for (width, xl_fits) in [(50.0f32, false), (85.0, true)] {
        let layout = layout_with(&format!(
            "[[lanes]]\nkind = \"orbit\"\npoints = [[200.0, 200.0], [260.0, 500.0], [200.0, 800.0]]\nwidth = {width}\n"
        ));
        let r = check(&layout, &base_with(&[STEEL, XL], vec![], ""), "lanes");
        assert!(of(&r, Feature::Lane, STEEL).is_empty());
        let lane = of(&r, Feature::Lane, XL);
        assert_eq!(lane.is_empty(), xl_fits, "{width}: {:#?}", r.errors);
        if let Some(e) = lane.first() {
            assert_eq!(e.name, "tablegen:lanes[0]");
            assert_eq!(
                (e.file.as_str(), e.path.as_str()),
                ("table.toml", "splines[0]")
            );
            assert!(near(e.actual_mm, width, 0.05), "{}", e.actual_mm);
            assert!(near(e.required_mm, 80.4, 1e-3));
        }
    }
}

#[test]
fn a_lane_outside_the_area_is_not_checked() {
    let layout = layout_with(
        "[[lanes]]\nkind = \"inlane\"\npoints = [[50.0, 200.0], [50.0, 400.0]]\nwidth = 40.0\n",
    );
    let area = [[100.0, 0.0], [514.0, 0.0], [514.0, 1070.0], [100.0, 1070.0]];
    let r = check(
        &layout,
        &base_with(&[STEEL], vec![restrict(XL, &[0], &area)], ""),
        "lane_area",
    );
    assert!(r.is_ok(), "{:#?}", r.errors);
    let r = check(
        &layout,
        &base_with(&[STEEL], vec![restrict(XL, &[1], &[])], ""),
        "lane_layer",
    );
    assert!(r.is_ok(), "{:#?}", r.errors);
    let r = check(
        &layout,
        &base_with(&[STEEL], vec![restrict(XL, &[0], &[])], ""),
        "lane_whole",
    );
    assert_eq!(of(&r, Feature::Lane, XL).len(), 1);
}

const FLIPPER_SWITCHES: &str = r#"
[[coils]]
id = "right_flipper_coil"
number = 61
kind = "flipper"

[[switches]]
id = "lower_right_eos"
number = 61
kind = "eos"
"#;

fn flipper_pair(right_x: f32, right_layer: u8) -> String {
    format!(
        r#"{FLIPPER_SWITCHES}
[[flippers]]
id = "lower_left"
coil = "left_flipper_coil"
eos_switch = "lower_left_eos"
collision_layer = 0
base_position = [180.0, 120.0, 0.0]
surface_normal = [0.0, 0.0, 1.0]
rest_angle_deg = -30.0
sweep_angle_deg = 50.0
eos_angle_fraction = 0.92
shape = {{ type = "StraightCapsule", length = 76.0, base_radius = 11.0, tip_radius = 6.5 }}
staging = {{ type = "DedicatedButton", input = "left_flipper" }}

[[flippers]]
id = "lower_right"
coil = "right_flipper_coil"
eos_switch = "lower_right_eos"
collision_layer = {right_layer}
base_position = [{right_x}, 120.0, 0.0]
surface_normal = [0.0, 0.0, 1.0]
rest_angle_deg = 210.0
sweep_angle_deg = -50.0
eos_angle_fraction = 0.92
shape = {{ type = "StraightCapsule", length = 76.0, base_radius = 11.0, tip_radius = 6.5 }}
staging = {{ type = "DedicatedButton", input = "right_flipper" }}
"#
    )
}

#[test]
fn flipper_gap_between_resting_tips() {
    // Tips 2 × 76 cos 30° apart from the pivots, radius 6.5 each.
    let tips = 2.0 * 76.0 * 30.0f32.to_radians().cos() + 13.0;
    let layout = layout_with("");
    for (gap, xl_fits) in [(50.0f32, false), (85.0, true)] {
        let r = check(
            &layout,
            &base_with(
                &[STEEL, XL, MINI],
                vec![],
                &flipper_pair(180.0 + tips + gap, 0),
            ),
            "flippers",
        );
        assert!(of(&r, Feature::FlipperGap, STEEL).is_empty());
        assert!(of(&r, Feature::FlipperGap, MINI).is_empty());
        let e = of(&r, Feature::FlipperGap, XL);
        assert_eq!(e.is_empty(), xl_fits, "{gap}: {:#?}", r.errors);
        if let Some(e) = e.first() {
            assert_eq!(e.name, "lower_left / lower_right");
            assert_eq!(e.path, "flippers[0]");
            assert!(near(e.actual_mm, gap, 0.05), "{} vs {gap}", e.actual_mm);
        }
    }
    // A 20 mm gap traps even the standard ball, but not the mini ball.
    let r = check(
        &layout,
        &base_with(
            &[STEEL, MINI],
            vec![],
            &flipper_pair(180.0 + tips + 20.0, 0),
        ),
        "flippers_20",
    );
    assert_eq!(of(&r, Feature::FlipperGap, STEEL).len(), 1);
    assert!(of(&r, Feature::FlipperGap, MINI).is_empty());
    // Overlapping bats are no gap; flippers on different layers never pair.
    for (x, layer) in [(180.0 + tips - 20.0, 0u8), (180.0 + tips + 20.0, 1)] {
        let r = check(
            &layout,
            &base_with(&[STEEL, XL], vec![], &flipper_pair(x, layer)),
            "flippers_none",
        );
        assert!(r.is_ok(), "{:#?}", r.errors);
    }
}

fn wireform(gauge: f32, layer: u8) -> String {
    format!(
        r#"
[[splines]]
id = "rail"
kind = "wireform"
layer = {layer}
gauge = {gauge}
points = [[100.0, 300.0, 40.0], [200.0, 500.0, 60.0], [300.0, 700.0, 40.0]]
"#
    )
}

#[test]
fn wireform_gauge_holds_the_ball_between_20_and_75_degrees() {
    let layout = layout_with("");
    // 19 mm: standard (44.7°) fine, mini drops through, XL perches (13.8°).
    let r = check(
        &layout,
        &base_with(&[STEEL, MINI, XL], vec![], &wireform(19.0, 2)),
        "wire19",
    );
    assert!(of(&r, Feature::Wireform, STEEL).is_empty());
    let mini = of(&r, Feature::Wireform, MINI);
    assert_eq!(mini.len(), 1);
    assert!(
        mini[0].message.contains("drops through"),
        "{}",
        mini[0].message
    );
    assert!(near(
        mini[0].required_mm,
        15.88 * 75.0f32.to_radians().sin(),
        0.01
    ));
    let xl = of(&r, Feature::Wireform, XL);
    assert_eq!(xl.len(), 1);
    assert_eq!(
        (xl[0].path.as_str(), xl[0].name.as_str()),
        ("splines[0]", "rail")
    );
    assert!(near(xl[0].actual_mm, 19.0, 1e-3));
    assert!(near(
        xl[0].required_mm,
        79.4 * 20.0f32.to_radians().sin(),
        0.01
    ));
    // 60 mm carries the XL ball only.
    let r = check(
        &layout,
        &base_with(&[STEEL, XL], vec![], &wireform(60.0, 2)),
        "wire60",
    );
    assert!(of(&r, Feature::Wireform, XL).is_empty());
    assert_eq!(of(&r, Feature::Wireform, STEEL).len(), 1);
    // Restricted off its layer, the XL ball is not checked on it.
    let r = check(
        &layout,
        &base_with(&[STEEL], vec![restrict(XL, &[0], &[])], &wireform(19.0, 2)),
        "wire_restricted",
    );
    assert!(r.is_ok(), "{:#?}", r.errors);
}

fn vuk(exit_offset: [f32; 3], exit_layer: u8) -> String {
    format!(
        r#"
[[mechs]]
id = "scoop"
kind = "vuk"
position = [300.0, 400.0, 0.0]
exit_layer = {exit_layer}
exit_offset = [{}, {}, {}]
"#,
        exit_offset[0], exit_offset[1], exit_offset[2]
    )
}

/// A U-shaped pocket 30 mm across around (300, 400), open toward +y.
const POCKET: &str = "[[walls]]\npoints = [[285.0, 440.0], [285.0, 385.0], [315.0, 385.0], [315.0, 440.0]]\nmaterial = \"steel\"\n";

#[test]
fn scoop_pocket_and_vuk_exit() {
    let deck = "[[decks]]\nlayer = 1\nz = 65.0\noutline = [[250.0, 350.0], [350.0, 350.0], [350.0, 450.0], [250.0, 450.0]]\nmaterial = \"acrylic\"\n";
    // Exit centre 80 mm up, on the deck's layer: 15 mm above the deck.
    let area = [
        [250.0, 360.0],
        [350.0, 360.0],
        [350.0, 470.0],
        [250.0, 470.0],
    ];
    let r = check(
        &layout_with(&format!("{POCKET}{deck}")),
        &base_with(
            &[STEEL, MINI],
            vec![restrict(XL, &[0, 1], &area)],
            &vuk([0.0, 0.0, 80.0], 1),
        ),
        "vuk",
    );
    assert!(of(&r, Feature::Scoop, STEEL).is_empty(), "{:#?}", r.errors);
    assert!(
        of(&r, Feature::VukExit, STEEL).is_empty(),
        "{:#?}",
        r.errors
    );
    let scoop = of(&r, Feature::Scoop, XL);
    assert_eq!(scoop.len(), 1, "{:#?}", r.errors);
    assert_eq!(
        (scoop[0].path.as_str(), scoop[0].name.as_str()),
        ("mechs[0]", "scoop")
    );
    assert!(
        scoop[0].actual_mm < 80.4 && scoop[0].actual_mm > 30.0,
        "{}",
        scoop[0].actual_mm
    );
    let exit = of(&r, Feature::VukExit, XL);
    assert_eq!(exit.len(), 1);
    assert_eq!(exit[0].path, "mechs[0].exit_offset");
    assert!(near(exit[0].actual_mm, 15.0, 0.01), "{}", exit[0].actual_mm);
    // The XL ball (layers 0 and 1) is under the deck in its area too.
    assert_eq!(of(&r, Feature::DeckHeadroom, XL).len(), 1);
    // A 60 mm exit puts even the standard ball into the deck.
    let r = check(
        &layout_with(&format!("{POCKET}{deck}")),
        &base_with(&[STEEL], vec![], &vuk([0.0, 0.0, 60.0], 1)),
        "vuk60",
    );
    assert_eq!(of(&r, Feature::VukExit, STEEL).len(), 1, "{:#?}", r.errors);
    // On open floor the scoop is wide open; an exit to a layer the
    // restriction does not allow is a warning.
    let r = check(
        &layout_with(""),
        &base_with(
            &[STEEL],
            vec![restrict(XL, &[0], &[])],
            &vuk([0.0, 300.0, 100.0], 1),
        ),
        "vuk_open",
    );
    assert!(r.is_ok(), "{:#?}", r.errors);
    assert!(
        r.warnings
            .iter()
            .any(|w| w.profile == XL && w.path == "mechs[0].exit_layer"),
        "{:#?}",
        r.warnings
    );
}

#[test]
fn bad_restriction_areas_are_errors() {
    let layout = layout_with("");
    for area in [
        vec![[0.0f32, 0.0], [10.0, 0.0]],
        vec![[0.0, 0.0], [10.0, 0.0], [20.0, 0.0]],
        vec![[0.0, 0.0], [10.0, 0.0], [f32::NAN, 5.0]],
    ] {
        let mut base = apex_base();
        base.balls.profiles = vec![STEEL.into()];
        base.balls.restrict = vec![restrict(XL, &[0], &area)];
        // NaN cannot be written to TOML: build the table by hand.
        let generated = generate(&layout, &base_no_nan(&base)).unwrap();
        let (dir, mut table, scene) = load(&generated, "bad_area");
        table.manifest.balls.restrict = base.balls.restrict.clone();
        let r = validate_clearance(&table, &scene);
        let e: Vec<_> = r
            .errors
            .iter()
            .filter(|e| e.feature == Feature::Restriction)
            .collect();
        assert_eq!(e.len(), 1, "{area:?}: {:#?}", r.errors);
        assert_eq!(e[0].path, "balls.restrict[0].area");
        assert_eq!(e[0].name, "balls.restrict[0]");
        std::fs::remove_dir_all(dir).unwrap();
    }
}

fn base_no_nan(b: &pin_core::schema::TableManifest) -> pin_core::schema::TableManifest {
    let mut b = b.clone();
    for r in &mut b.balls.restrict {
        r.area.clear();
    }
    b
}

#[test]
fn deck_headroom_follows_the_diameter() {
    let deck = "[[decks]]\nlayer = 1\nz = 40.0\noutline = [[200.0, 600.0], [400.0, 600.0], [400.0, 900.0], [200.0, 900.0]]\nmaterial = \"acrylic\"\n";
    let (dir, table, scene) = build(
        &layout_with(deck),
        &base_with(&[STEEL], vec![], ""),
        "sweep",
    );
    // jumbo 31.75 mm fits under 40 mm, the 39.5 mm ball does not.
    let ds = [0.01588f32, 0.027, 0.03175, 0.0395, 0.0794];
    let r = validate_clearance_diameters(&table, &scene, &ds);
    let failing: Vec<&str> = r.errors.iter().map(|e| e.profile.as_str()).collect();
    assert_eq!(
        failing,
        ["39.50 mm ball", "79.40 mm ball"],
        "{:#?}",
        r.errors
    );
    assert!(r.errors.iter().all(|e| near(e.actual_mm, 40.0, 1e-3)));
    // Deterministic.
    assert_eq!(validate_clearance_diameters(&table, &scene, &ds), r);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn walls_under_a_deck_keep_the_ball_out_of_a_narrow_channel() {
    // A 20 mm channel under a 40 mm deck: no 27 mm ball fits in it, so the
    // 40 mm headroom does not matter to it; a 50 mm channel does.
    for (channel, errors) in [(20.0f32, 0usize), (50.0, 1)] {
        let x0 = 300.0;
        let x1 = x0 + channel;
        let layout = layout_with(&format!(
            "[[decks]]\nlayer = 1\nz = 25.0\noutline = [[{x0}, 600.0], [{x1}, 600.0], [{x1}, 900.0], [{x0}, 900.0]]\nmaterial = \"acrylic\"\n\
             [[walls]]\npoints = [[{x0}, 600.0], [{x0}, 900.0]]\nmaterial = \"wood\"\n\
             [[walls]]\npoints = [[{x1}, 600.0], [{x1}, 900.0]]\nmaterial = \"wood\"\n"
        ));
        let r = check(&layout, &base_with(&[STEEL], vec![], ""), "channel");
        assert_eq!(
            of(&r, Feature::DeckHeadroom, STEEL).len(),
            errors,
            "{channel}: {:#?}",
            r.errors
        );
    }
}

// ------------------------------------------------------------ CLI

#[test]
fn validate_cli() {
    let layout = layout("apex_layout.toml");
    let s = |v: &[&str]| v.iter().map(|x| (*x).to_owned()).collect::<Vec<_>>();
    let ok_dir = {
        let mut base = apex_base();
        base.balls.restrict = vec![restrict(XL, &[0], &OPEN_AREA)];
        let g = generate(&layout, &base).unwrap();
        let dir = bundle_dir("cli_ok");
        write_bundle(&g, &dir).unwrap();
        dir
    };
    let out = run_validate_cli(&s(&[ok_dir.to_str().unwrap()])).unwrap();
    assert!(out.contains("clearance ok"), "{out}");
    assert!(
        out.contains("warning: table.toml: layer_transitions[0]"),
        "{out}"
    );
    let bad_dir = {
        let g = generate(&layout, &apex_base()).unwrap();
        let dir = bundle_dir("cli_bad");
        write_bundle(&g, &dir).unwrap();
        dir
    };
    let e = run_validate_cli(&s(&[bad_dir.to_str().unwrap()])).unwrap_err();
    assert_eq!(e.exit_code(), 1);
    assert!(e.to_string().contains("playfield.glb: meshes["), "{e}");
    assert!(e.to_string().contains("decks[0]"), "{e}");
    // A bundle that does not load lists the loader's errors.
    std::fs::write(bad_dir.join("table.toml"), "schema_version = 1\n").unwrap();
    let e = run_validate_cli(&s(&[bad_dir.to_str().unwrap()])).unwrap_err();
    assert_eq!(e.exit_code(), 1);
    assert!(e.to_string().contains("table.toml"), "{e}");
    let e = run_validate_cli(&s(&["/nonexistent/bundle"])).unwrap_err();
    assert_eq!(e.exit_code(), 1);
    for bad in [&[][..], &["a", "b"], &["--bogus"], &["-h"]] {
        let e = run_validate_cli(&s(bad)).unwrap_err();
        assert_eq!(e.exit_code(), 2, "{bad:?}");
        assert!(e.to_string().contains("pin_tablegen validate"));
    }
    // The binary dispatches the subcommand.
    let exe = env!("CARGO_BIN_EXE_pin_tablegen");
    let run = |dir: &PathBuf| {
        std::process::Command::new(exe)
            .arg("validate")
            .arg(dir)
            .output()
            .unwrap()
    };
    let ok = run(&ok_dir);
    assert!(
        ok.status.success(),
        "{}",
        String::from_utf8_lossy(&ok.stderr)
    );
    // Summary on stdout, warnings on stderr.
    let (stdout, stderr) = (
        String::from_utf8_lossy(&ok.stdout),
        String::from_utf8_lossy(&ok.stderr),
    );
    assert!(
        stdout.contains("clearance ok") && !stdout.contains("warning:"),
        "{stdout}"
    );
    assert!(
        stderr.contains("warning: table.toml: layer_transitions[0]"),
        "{stderr}"
    );
    let bad = run(&bad_dir);
    assert_eq!(bad.status.code(), Some(1));
    std::fs::remove_dir_all(ok_dir).unwrap();
    std::fs::remove_dir_all(bad_dir).unwrap();
}
