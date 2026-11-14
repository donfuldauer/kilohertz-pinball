//! Subphase 2.8 spec tests (Independent Tester, black box): the clearance
//! validator (deliverable 2, verification criterion 2) and the
//! `pin_tablegen validate` command.
//!
//! Every checked feature (lane width, flipper gap, wireform gauge, scoop
//! and VUK exit, upper-deck headroom) has a reject and an accept case; a
//! rejection must name the offending feature (file + key path + name).
//! Bundles are generated with the merged 4.3 generator from the text
//! fixtures under `fixtures/spec_2_8/` and pass the 4.1 loader first.

#[path = "fixtures/spec_2_8/support.rs"]
mod support;

use pin_assets::Vfs;
use pin_core::ids::FlipperId;
use pin_physics::ball::MovingBodies;
use pin_physics::flipper::FlipperSet;
use pin_tablegen::clearance::{
    ClearanceReport, Feature, run_validate_cli, validate_bundle, validate_clearance,
};
use std::path::Path;
use std::process::{Command, Output};
use support::*;

// ------------------------------------------------------------ fixtures

/// The upper deck of PLAN.md Appendix A.3: layer 1 at `z` mm over the
/// upper-right quarter of the playfield.
fn deck(z: f32) -> String {
    format!(
        r#"
[[decks]]
layer = 1
z = {z:?}
outline = [[260.0, 700.0], [514.0, 700.0], [514.0, 1070.0], [260.0, 1070.0]]
material = "acrylic"
"#
    )
}

/// The main deck's open area for an XL ball centre: everything at least
/// 45 mm (> its 39.7 mm radius) away from the walls and from the deck
/// footprint (x ≥ 260, y ≥ 700). An L-shaped polygon.
const OPEN_AREA: [[f32; 2]; 6] = [
    [45.0, 45.0],
    [469.0, 45.0],
    [469.0, 655.0],
    [215.0, 655.0],
    [215.0, 1025.0],
    [45.0, 1025.0],
];

/// The lower rectangle only (nothing beside the deck).
const LOWER_AREA: [[f32; 2]; 4] = [[45.0, 45.0], [469.0, 45.0], [469.0, 655.0], [45.0, 655.0]];

fn check_all(report: &ClearanceReport) {
    for e in report.errors.iter().chain(&report.warnings) {
        check_error_shape(e, None);
    }
    assert_sorted(&report.errors, "errors");
    assert_sorted(&report.warnings, "warnings");
    assert_eq!(report.is_ok(), report.errors.is_empty());
}

/// Builds, loads, validates; returns the loaded bundle too.
fn run(tag: &str, layout: &str, table: &str) -> (Loaded, ClearanceReport) {
    let dir = bundle(tag, layout, table);
    let l = load(&dir);
    let report = validate_clearance(&l.table, &l.scene);
    // The bundle entry point gives the same answer.
    let via_vfs = validate_bundle(&Vfs::open(&dir).unwrap())
        .unwrap_or_else(|e| panic!("{tag}: validate_bundle failed on a loadable bundle: {e}"));
    // (Compared as text: `required_mm` / `actual_mm` may be NaN.)
    let text = |r: &ClearanceReport| format!("{:?}\n{:?}", r.errors, r.warnings);
    assert_eq!(text(&report), text(&via_vfs), "{tag}: validate_bundle differs");
    // And it is a pure function of its inputs.
    let again = validate_clearance(&l.table, &l.scene);
    assert_eq!(text(&report), text(&again), "{tag}: not deterministic");
    check_all(&report);
    for e in report.errors.iter().chain(&report.warnings) {
        check_error_shape(e, Some(&l));
    }
    let _ = std::fs::remove_dir_all(&dir);
    (l, report)
}

fn close(a: f32, b: f32, tol: f32) -> bool {
    (a - b).abs() <= tol
}

// ------------------------------------------------------------ criterion 2

#[test]
fn c2_unrestricted_xl_ball_under_a_65_mm_deck_is_rejected() {
    let (l, r) = run(
        "c2-reject",
        &layout_toml(&deck(65.0)),
        &table_toml(&balls(&[STD, XL]), ""),
    );
    let errs = assert_rejects_only(&r, Feature::DeckHeadroom, XL, "XL under a 65 mm deck");
    assert_eq!(errs.len(), 1, "one error per ceiling:\n{}", describe(&r));
    let e = errs[0];
    // The offending feature is the deck: its glb node, by file and key path.
    assert_eq!(e.name, "decks[0]");
    assert_eq!(e.file, l.table.manifest.meta.playfield_mesh);
    assert_eq!(e.path, format!("meshes[{}]", mesh_index(&l, "decks[0]")));
    assert!(close(e.actual_mm, 65.0, 0.05), "actual {}", e.actual_mm);
    assert!(
        e.required_mm >= diameter_mm(XL) && e.required_mm <= diameter_mm(XL) + 2.0,
        "required {} for a 79.4 mm ball",
        e.required_mm
    );
    assert!(e.message.contains("65"), "{}", e.message);
    // As a bundle-validation error.
    let errs = r.into_result().expect_err("rejected");
    assert_eq!(errs.len(), 1);
    assert!(errs[0].path.starts_with("meshes[") && errs[0].message.contains(XL));
}

#[test]
fn c2_xl_ball_restricted_to_the_main_decks_open_area_is_accepted() {
    for (tag, area) in [("L", &OPEN_AREA[..]), ("lower", &LOWER_AREA[..])] {
        let (_, r) = run(
            &format!("c2-accept-{tag}"),
            &layout_toml(&deck(65.0)),
            &table_toml(&balls_restricted(&[STD, XL], XL, &[0], Some(area)), ""),
        );
        assert_accepts(&r, &format!("XL restricted to the open area ({tag})"));
        assert!(r.into_result().is_ok());
    }
}

#[test]
fn c2_restrictions_that_still_reach_under_the_deck_are_rejected() {
    // Layer 0 everywhere: the floor under the deck is still layer 0.
    let (_, r) = run(
        "c2-layer-only",
        &layout_toml(&deck(65.0)),
        &table_toml(&balls_restricted(&[STD, XL], XL, &[0], None), ""),
    );
    assert_rejects_only(&r, Feature::DeckHeadroom, XL, "XL on all of layer 0");
    // An area that reaches 100 mm under the deck.
    let under: [[f32; 2]; 4] = [[45.0, 45.0], [469.0, 45.0], [469.0, 800.0], [45.0, 800.0]];
    let (_, r) = run(
        "c2-area-under",
        &layout_toml(&deck(65.0)),
        &table_toml(&balls_restricted(&[STD, XL], XL, &[0], Some(&under)), ""),
    );
    assert_rejects_only(&r, Feature::DeckHeadroom, XL, "XL area reaching under the deck");
    // An area only under the deck.
    let only: [[f32; 2]; 3] = [[300.0, 750.0], [470.0, 750.0], [380.0, 1000.0]];
    let (_, r) = run(
        "c2-area-only-under",
        &layout_toml(&deck(65.0)),
        &table_toml(&balls_restricted(&[STD, XL], XL, &[0], Some(&only)), ""),
    );
    assert_rejects_only(&r, Feature::DeckHeadroom, XL, "XL area under the deck only");
}

#[test]
fn c2_profiles_count_wherever_the_table_declares_them() {
    // Only in a restriction (as the Appendix A example writes it), without
    // an area: declared, so checked.
    let b = format!(
        "profiles = [{STD:?}]\nrestrict = [ {{ profile = {XL:?}, layers = [0] }} ]"
    );
    let (_, r) = run("c2-restrict-only", &layout_toml(&deck(65.0)), &table_toml(&b, ""));
    assert_rejects_only(&r, Feature::DeckHeadroom, XL, "XL declared by a restriction only");
    // The same table accepts once the restriction names the open area.
    let b = balls_restricted(&[STD], XL, &[0], Some(&OPEN_AREA));
    let (_, r) = run("c2-restrict-only-ok", &layout_toml(&deck(65.0)), &table_toml(&b, ""));
    assert_accepts(&r, "XL declared by a restriction with the open area");
    // A table that never mentions the XL ball is fine with the 65 mm deck.
    let (_, r) = run(
        "c2-no-xl",
        &layout_toml(&deck(65.0)),
        &table_toml(&balls(&[STD, "ceramic_powerball", MINI, JUMBO]), ""),
    );
    assert_accepts(&r, "27 mm, 15.88 mm and 31.75 mm balls under a 65 mm deck");
}

// ------------------------------------------------------------ deck headroom

#[test]
fn deck_headroom_accepts_a_deck_high_enough_and_rejects_one_too_low() {
    // 100 mm clears the 79.4 mm ball with no restriction at all.
    let (_, r) = run(
        "deck-100",
        &layout_toml(&deck(100.0)),
        &table_toml(&balls(&[STD, XL]), ""),
    );
    assert_accepts(&r, "XL under a 100 mm deck");
    // 25 mm does not clear the standard 27 mm ball; the mini ball fits.
    let (l, r) = run(
        "deck-25",
        &layout_toml(&deck(25.0)),
        &table_toml(&balls(&[STD, MINI]), ""),
    );
    let errs = assert_rejects_only(&r, Feature::DeckHeadroom, STD, "27 mm ball under a 25 mm deck");
    assert_eq!(errs[0].name, "decks[0]");
    assert_eq!(errs[0].path, format!("meshes[{}]", mesh_index(&l, "decks[0]")));
    assert!(close(errs[0].actual_mm, 25.0, 0.05));
    assert!(errs[0].required_mm >= 27.0 && errs[0].required_mm <= 29.0);
    assert!(errors_of(&r, Feature::DeckHeadroom, MINI).is_empty());
}

#[test]
fn deck_headroom_reports_each_profile_that_does_not_fit() {
    // 30 mm: the mini (15.88) and standard (27) fit, jumbo (31.75) and XL do not.
    let (_, r) = run(
        "deck-30",
        &layout_toml(&deck(30.0)),
        &table_toml(&balls(&[MINI, STD, JUMBO, XL]), ""),
    );
    assert!(!r.is_ok());
    assert_eq!(errors_of(&r, Feature::DeckHeadroom, JUMBO).len(), 1, "{}", describe(&r));
    assert_eq!(errors_of(&r, Feature::DeckHeadroom, XL).len(), 1, "{}", describe(&r));
    assert_eq!(r.errors.len(), 2, "{}", describe(&r));
}

#[test]
fn xl_ball_restricted_to_the_upper_deck_itself_is_accepted() {
    // On top of the deck nothing is above the ball.
    let (_, r) = run(
        "deck-on-top",
        &layout_toml(&deck(65.0)),
        &table_toml(&balls_restricted(&[STD, XL], XL, &[1], None), ""),
    );
    assert_accepts(&r, "XL restricted to layer 1 (on the deck)");
}

// ------------------------------------------------------------ lane width

fn lane(width: f32) -> String {
    format!(
        r#"
[[lanes]]
kind = "inlane"
points = [[140.0, 420.0], [160.0, 280.0]]
width = {width:?}
"#
    )
}

#[test]
fn lane_width_rejects_a_ball_wider_than_the_lane_and_accepts_a_wide_lane() {
    // 32 mm lane (Appendix A.3): fine for 27 mm, too narrow for 31.75 mm.
    let (l, r) = run(
        "lane-32",
        &layout_toml(&lane(32.0)),
        &table_toml(&balls(&[STD, JUMBO]), ""),
    );
    let errs = assert_rejects_only(&r, Feature::Lane, JUMBO, "31.75 mm ball in a 32 mm lane");
    assert_eq!(errs.len(), 1, "{}", describe(&r));
    let e = errs[0];
    assert_eq!(e.name, "tablegen:lanes[0]");
    assert_eq!(e.file, "table.toml");
    assert_eq!(e.path, format!("splines[{}]", spline_index(&l, "tablegen:lanes[0].left")));
    assert!(close(e.actual_mm, 32.0, 0.2), "lane width measured {}", e.actual_mm);
    assert!(e.required_mm >= 31.75 && e.required_mm <= 33.75, "required {}", e.required_mm);
    // 36 mm: both fit.
    let (_, r) = run(
        "lane-36",
        &layout_toml(&lane(36.0)),
        &table_toml(&balls(&[STD, JUMBO]), ""),
    );
    assert_accepts(&r, "31.75 mm ball in a 36 mm lane");
    // The XL ball needs far more; 90 mm is enough.
    let (_, r) = run(
        "lane-36-xl",
        &layout_toml(&lane(36.0)),
        &table_toml(&balls(&[STD, XL]), ""),
    );
    let errs = assert_rejects_only(&r, Feature::Lane, XL, "XL ball in a 36 mm lane");
    assert_eq!(errs[0].name, "tablegen:lanes[0]");
    let (_, r) = run(
        "lane-90-xl",
        &layout_toml(&lane(90.0)),
        &table_toml(&balls(&[STD, XL]), ""),
    );
    assert_accepts(&r, "XL ball in a 90 mm lane");
}

#[test]
fn lane_width_follows_the_restriction() {
    // The XL ball kept out of the lane's region: accepted.
    let away: [[f32; 2]; 4] = [[45.0, 600.0], [469.0, 600.0], [469.0, 1025.0], [45.0, 1025.0]];
    let (_, r) = run(
        "lane-xl-away",
        &layout_toml(&lane(36.0)),
        &table_toml(&balls_restricted(&[STD, XL], XL, &[0], Some(&away)), ""),
    );
    assert_accepts(&r, "XL ball restricted to an area away from the lane");
    // Restricted to an area containing the lane: rejected as before.
    let around: [[f32; 2]; 4] = [[45.0, 45.0], [469.0, 45.0], [469.0, 600.0], [45.0, 600.0]];
    let (_, r) = run(
        "lane-xl-around",
        &layout_toml(&lane(36.0)),
        &table_toml(&balls_restricted(&[STD, XL], XL, &[0], Some(&around)), ""),
    );
    assert_rejects_only(&r, Feature::Lane, XL, "XL ball restricted to an area with the lane");
    // Restricted to another layer: the layer-0 lane does not concern it.
    let (_, r) = run(
        "lane-xl-layer1",
        &layout_toml(&lane(36.0)),
        &table_toml(&balls_restricted(&[STD, XL], XL, &[1], None), ""),
    );
    assert_accepts(&r, "XL ball restricted to layer 1, lane on layer 0");
}

fn hand_lane(gap: f32) -> String {
    format!(
        r#"
[[splines]]
id = "feed.left"
kind = "guide"
layer = 0
points = [[200.0, 400.0, 0.0], [200.0, 480.0, 0.0], [200.0, 560.0, 0.0], [200.0, 640.0, 0.0]]

[[splines]]
id = "feed.right"
kind = "guide"
layer = 0
points = [[{x:?}, 400.0, 0.0], [{x:?}, 480.0, 0.0], [{x:?}, 560.0, 0.0], [{x:?}, 640.0, 0.0]]
"#,
        x = 200.0 + gap
    )
}

#[test]
fn hand_written_guide_pairs_named_left_and_right_are_lanes_too() {
    let (l, r) = run(
        "hand-lane-24",
        &layout_toml(""),
        &table_toml(&balls(&[STD, MINI]), &hand_lane(24.0)),
    );
    let errs = assert_rejects_only(&r, Feature::Lane, STD, "27 mm ball between guides 24 mm apart");
    assert_eq!(errs[0].name, "feed");
    assert_eq!(errs[0].file, "table.toml");
    assert_eq!(errs[0].path, format!("splines[{}]", spline_index(&l, "feed.left")));
    assert!(close(errs[0].actual_mm, 24.0, 0.2), "{}", errs[0].actual_mm);
    let (_, r) = run(
        "hand-lane-40",
        &layout_toml(""),
        &table_toml(&balls(&[STD, MINI]), &hand_lane(40.0)),
    );
    assert_accepts(&r, "27 mm ball between guides 40 mm apart");
}

// ------------------------------------------------------------ flipper gap

/// A right flipper mirroring the base table's `lower_left` (base at
/// x = 180, y = 120) with its base at `x`.
fn right_flipper(x: f32) -> String {
    format!(
        r#"
[[flippers]]
id = "lower_right"
coil = "right_flipper_coil"
eos_switch = "lower_right_eos"
collision_layer = 0
base_position = [{x:?}, 120.0, 0.0]
surface_normal = [0.0, 0.0, 1.0]
rest_angle_deg = 210.0
sweep_angle_deg = -50.0
eos_angle_fraction = 0.92
shape = {{ type = "StraightCapsule", length = 76.0, base_radius = 11.0, tip_radius = 6.5 }}
staging = {{ type = "DedicatedButton", input = "right_flipper" }}
"#
    )
}

/// Smallest distance (mm) between the surfaces of the first two bats at
/// rest, from the physics shapes (alternating projections between the two
/// convex bats, from several starts).
fn measured_flipper_gap_mm(l: &Loaded) -> f32 {
    let set = FlipperSet::new(&l.table.flippers).expect("flippers build");
    assert!(set.body_count() >= 2);
    let (a, b) = (
        set.body_index(FlipperId(0)).unwrap(),
        set.body_index(FlipperId(1)).unwrap(),
    );
    let pa = set.pivot_m(FlipperId(0)).unwrap();
    let pb = set.pivot_m(FlipperId(1)).unwrap();
    let mut best = f32::INFINITY;
    for k in 0..=8 {
        let mut p = pa + (pb - pa) * (k as f32 / 8.0);
        let mut gap = f32::INFINITY;
        for _ in 0..200 {
            let q = set.closest_point(b, 0.0, p).unwrap().point;
            let hit = set.closest_point(a, 0.0, q).unwrap();
            p = hit.point;
            gap = hit.distance;
        }
        best = best.min(gap);
    }
    best * 1000.0
}

#[test]
fn flipper_gap_rejects_a_ball_that_wedges_and_accepts_a_wide_gap() {
    // Bases 200 mm apart: a tip gap of roughly 55 mm. Fine for the 27 mm
    // ball, a wedge for the 79.4 mm ball.
    let (l, r) = run(
        "flip-200",
        &layout_toml(""),
        &table_toml(&balls(&[STD, XL]), &right_flipper(380.0)),
    );
    let gap = measured_flipper_gap_mm(&l);
    println!("flipper gap, bases 200 mm apart: {gap:.2} mm (physics shapes)");
    assert!(
        gap > 29.0 && gap < 79.0,
        "fixture: the gap {gap} mm must lie between the two balls' needs"
    );
    let errs = assert_rejects_only(&r, Feature::FlipperGap, XL, "XL ball over a ~55 mm flipper gap");
    assert_eq!(errs.len(), 1, "{}", describe(&r));
    let e = errs[0];
    assert_eq!(e.file, "table.toml");
    assert_eq!(e.path, "flippers[0]", "the lower index of the pair");
    assert!(close(e.actual_mm, gap, 1.0), "gap reported {} mm, physics shapes {gap} mm", e.actual_mm);
    assert!(e.required_mm >= 79.4 && e.required_mm <= 81.4, "required {}", e.required_mm);
    // The name identifies both flippers of the pair.
    assert!(e.name.contains('/'), "pair name {:?}", e.name);

    // Bases 260 mm apart: about 115 mm, the XL ball drains freely.
    let (l, r) = run(
        "flip-260",
        &layout_toml(""),
        &table_toml(&balls(&[STD, XL]), &right_flipper(440.0)),
    );
    let gap = measured_flipper_gap_mm(&l);
    println!("flipper gap, bases 260 mm apart: {gap:.2} mm (physics shapes)");
    assert!(gap > 82.0, "fixture: gap {gap} mm");
    assert_accepts(&r, "XL ball over a ~115 mm flipper gap");
}

#[test]
fn flipper_gap_standard_ball_and_restrictions() {
    // Bases 165 mm apart: about 20 mm between the tips, a wedge for the
    // 27 mm ball; the mini ball (15.88 mm) passes.
    let (l, r) = run(
        "flip-165",
        &layout_toml(""),
        &table_toml(&balls(&[STD, MINI]), &right_flipper(345.0)),
    );
    let gap = measured_flipper_gap_mm(&l);
    println!("flipper gap, bases 165 mm apart: {gap:.2} mm (physics shapes)");
    assert!(gap > 17.5 && gap < 27.0, "fixture: gap {gap} mm");
    let errs = assert_rejects_only(&r, Feature::FlipperGap, STD, "27 mm ball over a ~20 mm gap");
    assert_eq!(errs[0].path, "flippers[0]");
    assert!(close(errs[0].actual_mm, gap, 1.0));

    // The XL ball kept away from the flippers (layer 0, upper half only)
    // is not concerned by the ~55 mm gap; nor is it on another layer.
    let upper: [[f32; 2]; 4] = [[45.0, 500.0], [469.0, 500.0], [469.0, 1025.0], [45.0, 1025.0]];
    for (tag, b) in [
        ("area", balls_restricted(&[STD, XL], XL, &[0], Some(&upper))),
        ("layer", balls_restricted(&[STD, XL], XL, &[1], None)),
    ] {
        let (_, r) = run(
            &format!("flip-200-{tag}"),
            &layout_toml(""),
            &table_toml(&b, &right_flipper(380.0)),
        );
        assert_accepts(&r, &format!("XL ball restricted away from the flippers ({tag})"));
    }
}

// ------------------------------------------------------------ wireform gauge

fn wireform(gauge: f32) -> String {
    format!(
        r#"
[[splines]]
id = "return_wireform"
kind = "wireform"
layer = 2
gauge = {gauge:?}
points = [[60.0, 900.0, 140.0], [40.0, 600.0, 155.0], [150.0, 260.0, 120.0]]
roll_deg = [0.0, 8.0, 0.0]
"#
    )
}

#[test]
fn wireform_gauge_rejects_a_ball_the_rails_cannot_carry() {
    // 19 mm gauge (Appendix A): carries the 27 mm ball, not the 79.4 mm one.
    let (l, r) = run(
        "wire-19",
        &layout_toml(""),
        &table_toml(&balls(&[STD, XL]), &wireform(19.0)),
    );
    let errs = assert_rejects_only(&r, Feature::Wireform, XL, "XL ball on a 19 mm wireform");
    assert_eq!(errs.len(), 1, "{}", describe(&r));
    let e = errs[0];
    assert_eq!(e.name, "return_wireform");
    assert_eq!(e.file, "table.toml");
    assert_eq!(e.path, format!("splines[{}]", spline_index(&l, "return_wireform")));
    assert!(close(e.actual_mm, 19.0, 0.01), "gauge reported {}", e.actual_mm);
    assert!(e.message.contains("19"), "{}", e.message);

    // The mini ball (15.88 mm) drops between rails 19 mm apart.
    let (_, r) = run(
        "wire-19-mini",
        &layout_toml(""),
        &table_toml(&balls(&[STD, MINI]), &wireform(19.0)),
    );
    let errs = assert_rejects_only(&r, Feature::Wireform, MINI, "15.88 mm ball on a 19 mm wireform");
    assert_eq!(errs[0].name, "return_wireform");
}

#[test]
fn wireform_gauge_accepts_a_matching_gauge_or_a_restricted_ball() {
    // The 27 mm and 31.75 mm balls both ride a 19 mm gauge.
    let (_, r) = run(
        "wire-19-ok",
        &layout_toml(""),
        &table_toml(&balls(&[STD, JUMBO]), &wireform(19.0)),
    );
    assert_accepts(&r, "27 mm and 31.75 mm balls on a 19 mm wireform");
    // A 50 mm gauge carries the XL ball; the 27 mm ball must then be kept
    // off the wireform layer.
    let b = format!(
        "profiles = [{STD:?}, {XL:?}]\nrestrict = [ {{ profile = {STD:?}, layers = [0] }} ]"
    );
    let (_, r) = run("wire-50-xl", &layout_toml(""), &table_toml(&b, &wireform(50.0)));
    assert_accepts(&r, "XL ball on a 50 mm wireform, standard ball restricted to layer 0");
    // Without that restriction the 27 mm ball falls through the 50 mm gauge.
    let (_, r) = run(
        "wire-50-std",
        &layout_toml(""),
        &table_toml(&balls(&[STD, XL]), &wireform(50.0)),
    );
    assert_rejects_only(&r, Feature::Wireform, STD, "27 mm ball on a 50 mm wireform");
    // The XL ball restricted to the main deck never meets the 19 mm wireform.
    let (_, r) = run(
        "wire-19-xl-restricted",
        &layout_toml(""),
        &table_toml(&balls_restricted(&[STD, XL], XL, &[0], None), &wireform(19.0)),
    );
    assert_accepts(&r, "XL ball restricted to layer 0, wireform on layer 2");
}

// ------------------------------------------------------------ scoop and VUK

/// Two parallel steel walls `width` mm apart around x = 257, y ∈ [440, 560].
fn pocket(width: f32) -> String {
    let (a, b) = (257.0 - 0.5 * width, 257.0 + 0.5 * width);
    format!(
        r#"
[[walls]]
points = [[{a:?}, 440.0], [{a:?}, 560.0]]
closed = false
material = "steel"

[[walls]]
points = [[{b:?}, 440.0], [{b:?}, 560.0]]
closed = false
material = "steel"
"#
    )
}

fn vuk(exit_layer: u8, exit_offset: [f32; 3]) -> String {
    format!(
        r#"
[[mechs]]
id = "scoop"
kind = "vuk"
coil = "scoop_coil"
switch = "scoop_switch"
position = [257.0, 500.0, 0.0]
layer = 0
exit_layer = {exit_layer}
exit_offset = {exit_offset:?}
"#
    )
}

#[test]
fn scoop_diameter_rejects_a_ball_that_does_not_fit_the_pocket() {
    // A 40 mm pocket: fine for 27 mm and 31.75 mm, too small for 79.4 mm.
    let (l, r) = run(
        "scoop-40",
        &layout_toml(&pocket(40.0)),
        &table_toml(&balls(&[STD, JUMBO, XL]), &vuk(0, [0.0, 150.0, 60.0])),
    );
    let errs = assert_rejects_only(&r, Feature::Scoop, XL, "XL ball in a 40 mm scoop");
    assert_eq!(errs.len(), 1, "{}", describe(&r));
    let e = errs[0];
    assert_eq!(e.name, "scoop");
    assert_eq!(e.file, "table.toml");
    assert_eq!(e.path, format!("mechs[{}]", mech_index(&l, "scoop")));
    assert!(e.required_mm >= 79.4 && e.required_mm <= 81.4, "required {}", e.required_mm);
    // The 39.7 mm-high centre is 20 mm beside and 11.7 mm above the 28 mm
    // wall tops: an opening of 2·√(20² + 11.7²) = 46.3 mm, 40 mm if only
    // the plan view is measured.
    assert!(
        e.actual_mm >= 39.5 && e.actual_mm <= 47.0,
        "opening reported {} mm",
        e.actual_mm
    );
    // A 24 mm pocket does not take the 27 mm ball either; the mini fits.
    let (_, r) = run(
        "scoop-24",
        &layout_toml(&pocket(24.0)),
        &table_toml(&balls(&[STD, MINI]), &vuk(0, [0.0, 150.0, 60.0])),
    );
    let errs = assert_rejects_only(&r, Feature::Scoop, STD, "27 mm ball in a 24 mm scoop");
    assert_eq!(errs[0].name, "scoop");
    assert!(close(errs[0].actual_mm, 24.0, 0.5), "opening {}", errs[0].actual_mm);
}

#[test]
fn scoop_diameter_accepts_a_pocket_wide_enough_or_a_restricted_ball() {
    // 130 mm between the walls: the XL ball fits.
    let (_, r) = run(
        "scoop-130",
        &layout_toml(&pocket(130.0)),
        &table_toml(&balls(&[STD, XL]), &vuk(0, [0.0, 150.0, 60.0])),
    );
    assert_accepts(&r, "XL ball in a 130 mm scoop");
    // A scoop on the open floor has no opening to violate.
    let (_, r) = run(
        "scoop-open",
        &layout_toml(""),
        &table_toml(&balls(&[STD, XL]), &vuk(0, [0.0, 150.0, 60.0])),
    );
    assert_accepts(&r, "XL ball in a scoop on the open floor");
    // The XL ball kept in the lower third never reaches the 40 mm scoop.
    let lower: [[f32; 2]; 4] = [[45.0, 45.0], [469.0, 45.0], [469.0, 350.0], [45.0, 350.0]];
    let (_, r) = run(
        "scoop-40-restricted",
        &layout_toml(&pocket(40.0)),
        &table_toml(
            &balls_restricted(&[STD, XL], XL, &[0], Some(&lower)),
            &vuk(0, [0.0, 150.0, 60.0]),
        ),
    );
    assert_accepts(&r, "XL ball restricted away from the 40 mm scoop");
}

#[test]
fn vuk_exit_rejects_a_ball_ejected_into_geometry_and_accepts_a_clear_exit() {
    // Exit onto the 65 mm upper deck (layer 1), 300 mm up the table.
    // Centre 5 mm above the deck: the 27 mm ball is 8.5 mm inside it.
    let (l, r) = run(
        "vuk-exit-low",
        &layout_toml(&deck(65.0)),
        &table_toml(&balls(&[STD]), &vuk(1, [100.0, 300.0, 70.0])),
    );
    let errs = assert_rejects_only(&r, Feature::VukExit, STD, "VUK exit 5 mm above the deck");
    assert_eq!(errs.len(), 1, "{}", describe(&r));
    let e = errs[0];
    assert_eq!(e.name, "scoop");
    assert_eq!(e.file, "table.toml");
    assert_eq!(e.path, format!("mechs[{}].exit_offset", mech_index(&l, "scoop")));
    assert!(close(e.actual_mm, 5.0, 0.1), "distance to the deck {}", e.actual_mm);
    assert!(e.required_mm >= 12.5 && e.required_mm <= 13.5, "required {}", e.required_mm);
    // Centre 20 mm above the deck: clear.
    let (_, r) = run(
        "vuk-exit-ok",
        &layout_toml(&deck(65.0)),
        &table_toml(&balls(&[STD]), &vuk(1, [100.0, 300.0, 85.0])),
    );
    assert_accepts(&r, "VUK exit 20 mm above the deck");
    // The same exit is too low for the jumbo ball's 15.9 mm radius only
    // when it is lower still: 15 mm above.
    let (_, r) = run(
        "vuk-exit-jumbo",
        &layout_toml(&deck(65.0)),
        &table_toml(&balls(&[STD, JUMBO]), &vuk(1, [100.0, 300.0, 80.0])),
    );
    let errs = assert_rejects_only(&r, Feature::VukExit, JUMBO, "jumbo VUK exit 15 mm above the deck");
    assert!(errs[0].path.ends_with(".exit_offset"));
}

#[test]
fn vuk_exit_into_a_wall_on_the_main_deck() {
    // Exit on layer 0 with the centre on a wall ribbon.
    let wall = r#"
[[walls]]
points = [[200.0, 650.0], [320.0, 650.0]]
closed = false
material = "steel"
"#;
    let (_, r) = run(
        "vuk-exit-wall",
        &layout_toml(wall),
        &table_toml(&balls(&[STD]), &vuk(0, [0.0, 150.0, 14.0])),
    );
    let errs = assert_rejects_only(&r, Feature::VukExit, STD, "VUK exit centred on a wall");
    assert!(errs[0].actual_mm < 1.0, "distance {}", errs[0].actual_mm);
    // 40 mm short of the wall: clear.
    let (_, r) = run(
        "vuk-exit-wall-ok",
        &layout_toml(wall),
        &table_toml(&balls(&[STD]), &vuk(0, [0.0, 110.0, 14.0])),
    );
    assert_accepts(&r, "VUK exit 40 mm short of the wall");
}

// ------------------------------------------------------------ report contract

#[test]
fn feature_labels_match_the_contract() {
    let all = [
        (Feature::Lane, "lane"),
        (Feature::FlipperGap, "flipper gap"),
        (Feature::Wireform, "wireform"),
        (Feature::Scoop, "scoop"),
        (Feature::VukExit, "vuk exit"),
        (Feature::DeckHeadroom, "deck headroom"),
        (Feature::Restriction, "restriction"),
    ];
    for (f, label) in all {
        assert_eq!(f.label(), label);
    }
}

#[test]
fn several_violations_are_all_reported_sorted_and_named() {
    // Deck 65 mm, 36 mm lane, 19 mm wireform, 40 mm scoop, ~55 mm flipper
    // gap: the unrestricted XL ball violates all five; the 27 mm ball none.
    let layout = layout_toml(&format!("{}{}{}", deck(65.0), lane(36.0), pocket(40.0)));
    let extra = format!(
        "{}{}{}",
        wireform(19.0),
        right_flipper(380.0),
        vuk(0, [0.0, 150.0, 60.0])
    );
    let (_, r) = run("many", &layout, &table_toml(&balls(&[STD, XL]), &extra));
    assert!(!r.is_ok());
    for f in [
        Feature::DeckHeadroom,
        Feature::Lane,
        Feature::Wireform,
        Feature::Scoop,
        Feature::FlipperGap,
    ] {
        assert_eq!(
            errors_of(&r, f, XL).len(),
            1,
            "one {f:?} error for the XL ball expected:\n{}",
            describe(&r)
        );
    }
    assert!(r.errors.iter().all(|e| e.profile == XL), "{}", describe(&r));
    assert_eq!(r.errors.len(), 5, "{}", describe(&r));
    let n = r.errors.len();
    let errs = r.into_result().expect_err("rejected");
    assert_eq!(errs.len(), n);

    // Restricted to the open main-deck area away from lane, scoop and
    // flippers, the same table is accepted.
    let safe: [[f32; 2]; 4] = [[45.0, 600.0], [215.0, 600.0], [215.0, 1025.0], [45.0, 1025.0]];
    let (_, r) = run(
        "many-restricted",
        &layout,
        &table_toml(&balls_restricted(&[STD, XL], XL, &[0], Some(&safe)), &extra),
    );
    assert_accepts(&r, "XL ball restricted to a clear part of the main deck");
}

#[test]
fn a_restriction_into_a_transition_is_a_warning_not_an_error() {
    // A layer transition 0 → 2 inside the XL ball's area, to a layer it
    // may not use: reported as a warning, validation still passes.
    let transition = r#"
[[layer_transitions]]
id = "ramp_entry"
from_layer = 0
to_layer = 2
zone = { min = [140.0, 340.0, 0.0], max = [190.0, 380.0, 30.0] }
"#;
    let (_, r) = run(
        "warn-transition",
        &layout_toml(""),
        &table_toml(
            &balls_restricted(&[STD, XL], XL, &[0], Some(&LOWER_AREA)),
            transition,
        ),
    );
    assert_accepts(&r, "restricted XL ball with a reachable transition");
    let w: Vec<_> = r
        .warnings
        .iter()
        .filter(|w| w.feature == Feature::Restriction && w.profile == XL)
        .collect();
    assert_eq!(w.len(), 1, "{}", describe(&r));
    assert_eq!(w[0].file, "table.toml");
    assert_eq!(w[0].path, "layer_transitions[0]");
    let n = r.warnings.len();
    let ok = r.into_result().expect("warnings do not fail validation");
    assert_eq!(ok.len(), n);
}

#[test]
fn a_malformed_restriction_area_is_not_silently_accepted() {
    // Two points are not a polygon. Either the loader refuses the bundle or
    // the validator reports the restriction; a pass would silently drop the
    // author's restriction.
    for (tag, area) in [
        ("two-points", "[[45.0, 45.0], [469.0, 45.0]]"),
        ("zero-area", "[[45.0, 45.0], [469.0, 45.0], [200.0, 45.0]]"),
    ] {
        let b = format!(
            "profiles = [{STD:?}, {XL:?}]\nrestrict = [ {{ profile = {XL:?}, layers = [0], area = {area} }} ]"
        );
        let dir = bundle(&format!("bad-area-{tag}"), &layout_toml(""), &table_toml(&b, ""));
        match try_load(&dir) {
            Err(e) => assert!(e.contains("restrict"), "{tag}: loader error names something else: {e}"),
            Ok(l) => {
                let r = validate_clearance(&l.table, &l.scene);
                check_all(&r);
                let hits = errors_of(&r, Feature::Restriction, XL);
                assert!(
                    !hits.is_empty() && hits[0].path == "balls.restrict[0].area",
                    "{tag}: expected a restriction error at balls.restrict[0].area, got\n{}",
                    describe(&r)
                );
                assert_eq!(hits[0].file, "table.toml");
            }
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[test]
fn validate_bundle_reports_load_failures_as_asset_errors() {
    let dir = bundle("unloadable", &layout_toml(""), &table_toml(&balls(&[STD]), ""));
    std::fs::write(dir.join("table.toml"), "schema_version = 1\n[meta\n").unwrap();
    let vfs = Vfs::open(&dir).unwrap();
    assert!(validate_bundle(&vfs).is_err(), "a broken table.toml must be an error");
    let _ = std::fs::remove_dir_all(&dir);
}

// ------------------------------------------------------------ CLI

fn cli(args: &[&std::ffi::OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pin_tablegen"))
        .args(args)
        .output()
        .expect("the pin_tablegen binary runs")
}

fn validate_cli(target: &Path) -> Output {
    cli(&["validate".as_ref(), target.as_os_str()])
}

fn out(o: &Output) -> (String, String) {
    (
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

fn assert_no_panic(o: &Output, what: &str) {
    let (_, err) = out(o);
    assert!(
        o.status.code().is_some_and(|c| c != 101) && !err.contains("panicked"),
        "{what}: exit status {:?} (panic or signal)\n{err}",
        o.status
    );
}

fn c2_reject_bundle(tag: &str) -> std::path::PathBuf {
    bundle(tag, &layout_toml(&deck(65.0)), &table_toml(&balls(&[STD, XL]), ""))
}

fn c2_accept_bundle(tag: &str) -> std::path::PathBuf {
    bundle(
        tag,
        &layout_toml(&deck(65.0)),
        &table_toml(&balls_restricted(&[STD, XL], XL, &[0], Some(&OPEN_AREA)), ""),
    )
}

#[test]
fn cli_validate_exits_0_with_a_summary_for_a_passing_bundle() {
    let dir = c2_accept_bundle("cli-pass");
    let o = validate_cli(&dir);
    let (stdout, stderr) = out(&o);
    assert_no_panic(&o, "passing bundle");
    assert_eq!(o.status.code(), Some(0), "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(
        !stdout.trim().is_empty(),
        "a summary line is expected on stdout; stderr:\n{stderr}"
    );
    // The library entry point agrees.
    let s = run_validate_cli(&[dir.to_string_lossy().into_owned()]);
    assert!(s.is_ok(), "run_validate_cli fails on a passing bundle");
    assert!(!s.ok().unwrap().trim().is_empty());
}

#[test]
fn cli_validate_exits_1_and_names_the_feature_for_a_failing_bundle() {
    let dir = c2_reject_bundle("cli-fail");
    let l = load(&dir);
    let report = validate_clearance(&l.table, &l.scene);
    assert_eq!(report.errors.len(), 1);
    let o = validate_cli(&dir);
    let (stdout, stderr) = out(&o);
    assert_no_panic(&o, "failing bundle");
    assert_eq!(o.status.code(), Some(1), "stdout:\n{stdout}\nstderr:\n{stderr}");
    // One line per error, `file: path: message`.
    let line = report.errors[0].to_string();
    let all = format!("{stdout}\n{stderr}");
    assert!(
        all.lines().any(|l| l.trim_end().ends_with(&line) || l.contains(&line)),
        "the error line {line:?} is missing from the output:\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(all.contains("decks[0]") && all.contains(XL) && all.contains("playfield.glb"));
    assert!(
        run_validate_cli(&[dir.to_string_lossy().into_owned()]).is_err(),
        "run_validate_cli passes a failing bundle"
    );
}

#[test]
fn cli_validate_prints_warnings_and_still_exits_0() {
    let transition = r#"
[[layer_transitions]]
id = "ramp_entry"
from_layer = 0
to_layer = 2
zone = { min = [140.0, 340.0, 0.0], max = [190.0, 380.0, 30.0] }
"#;
    let dir = bundle(
        "cli-warn",
        &layout_toml(""),
        &table_toml(
            &balls_restricted(&[STD, XL], XL, &[0], Some(&LOWER_AREA)),
            transition,
        ),
    );
    let o = validate_cli(&dir);
    let (stdout, stderr) = out(&o);
    assert_eq!(o.status.code(), Some(0), "stdout:\n{stdout}\nstderr:\n{stderr}");
    assert!(
        stderr.contains("layer_transitions[0]"),
        "the warning is expected on stderr:\n{stderr}"
    );
}

#[test]
fn cli_validate_reports_loader_errors_with_exit_1() {
    // A table.toml the 4.1 loader refuses (unknown coil on a flipper).
    let dir = c2_accept_bundle("cli-load-error");
    let text = std::fs::read_to_string(dir.join("table.toml")).unwrap();
    assert!(text.contains("left_flipper_coil"));
    let broken = text.replacen("coil = \"left_flipper_coil\"", "coil = \"no_such_coil\"", 1);
    assert_ne!(text, broken);
    std::fs::write(dir.join("table.toml"), broken).unwrap();
    let o = validate_cli(&dir);
    let (stdout, stderr) = out(&o);
    assert_no_panic(&o, "loader error");
    assert_eq!(o.status.code(), Some(1), "stdout:\n{stdout}\nstderr:\n{stderr}");
    let all = format!("{stdout}\n{stderr}");
    assert!(
        all.contains("table.toml") && all.contains("no_such_coil"),
        "the loader's error is expected in the output:\n{all}"
    );
    // Not TOML at all.
    std::fs::write(dir.join("table.toml"), b"\xff\xfe not toml [[[").unwrap();
    let o = validate_cli(&dir);
    assert_no_panic(&o, "garbage table.toml");
    assert_eq!(o.status.code(), Some(1));
    // No playfield.glb.
    let dir = c2_accept_bundle("cli-no-glb");
    std::fs::remove_file(dir.join("playfield.glb")).unwrap();
    let o = validate_cli(&dir);
    assert_no_panic(&o, "missing glb");
    assert_eq!(o.status.code(), Some(1), "{:?}", out(&o));
    // A garbage glb.
    std::fs::write(dir.join("playfield.glb"), b"glTF\x02\x00\x00\x00 garbage").unwrap();
    let o = validate_cli(&dir);
    assert_no_panic(&o, "garbage glb");
    assert_eq!(o.status.code(), Some(1), "{:?}", out(&o));
}

#[test]
fn cli_validate_usage_errors_exit_2_and_missing_bundles_fail_cleanly() {
    // No bundle argument.
    let o = cli(&["validate".as_ref()]);
    let (_, stderr) = out(&o);
    assert_no_panic(&o, "validate without a bundle");
    assert_eq!(o.status.code(), Some(2), "{stderr}");
    assert!(!stderr.trim().is_empty(), "a usage message is expected");
    // Too many arguments.
    let dir = c2_accept_bundle("cli-usage");
    let o = cli(&["validate".as_ref(), dir.as_os_str(), "extra".as_ref()]);
    assert_no_panic(&o, "validate with two arguments");
    assert_eq!(o.status.code(), Some(2), "{:?}", out(&o));
    // A bundle that does not exist: a clean failure with a message.
    let missing = dir.join("no-such-bundle");
    let o = validate_cli(&missing);
    let (stdout, stderr) = out(&o);
    assert_no_panic(&o, "missing bundle");
    assert!(
        matches!(o.status.code(), Some(1) | Some(2)),
        "exit {:?}\n{stdout}\n{stderr}",
        o.status
    );
    assert!(!format!("{stdout}{stderr}").trim().is_empty());
}

// --- a minimal stored (uncompressed) zip writer for the .pinbundle case

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &b in data {
        crc ^= u32::from(b);
        for _ in 0..8 {
            crc = if crc & 1 != 0 { (crc >> 1) ^ 0xedb8_8320 } else { crc >> 1 };
        }
    }
    !crc
}

fn collect_files(root: &Path, dir: &Path, out: &mut Vec<(String, Vec<u8>)>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir).unwrap().map(|e| e.unwrap()).collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let p = e.path();
        if e.file_type().unwrap().is_dir() {
            collect_files(root, &p, out);
        } else {
            let rel = p.strip_prefix(root).unwrap();
            let name = rel
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            out.push((name, std::fs::read(&p).unwrap()));
        }
    }
}

fn zip_dir(dir: &Path, archive: &Path) {
    let mut files = Vec::new();
    collect_files(dir, dir, &mut files);
    let mut bytes: Vec<u8> = Vec::new();
    let mut central: Vec<u8> = Vec::new();
    for (name, data) in &files {
        let offset = bytes.len() as u32;
        let crc = crc32(data);
        let (n, len) = (name.len() as u16, data.len() as u32);
        bytes.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        bytes.extend_from_slice(&20u16.to_le_bytes()); // version needed
        bytes.extend_from_slice(&0u16.to_le_bytes()); // flags
        bytes.extend_from_slice(&0u16.to_le_bytes()); // stored
        bytes.extend_from_slice(&0u16.to_le_bytes()); // time
        bytes.extend_from_slice(&0x21u16.to_le_bytes()); // date 1980-01-01
        bytes.extend_from_slice(&crc.to_le_bytes());
        bytes.extend_from_slice(&len.to_le_bytes());
        bytes.extend_from_slice(&len.to_le_bytes());
        bytes.extend_from_slice(&n.to_le_bytes());
        bytes.extend_from_slice(&0u16.to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(data);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes()); // made by
        central.extend_from_slice(&20u16.to_le_bytes()); // needed
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0u16.to_le_bytes());
        central.extend_from_slice(&0x21u16.to_le_bytes());
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&len.to_le_bytes());
        central.extend_from_slice(&n.to_le_bytes());
        central.extend_from_slice(&[0u8; 12]); // extra, comment, disk, int attrs (2+2+2+2) + ext attrs (4)
        central.extend_from_slice(&offset.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let cd_offset = bytes.len() as u32;
    let cd_len = central.len() as u32;
    bytes.extend_from_slice(&central);
    bytes.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    bytes.extend_from_slice(&(files.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&(files.len() as u16).to_le_bytes());
    bytes.extend_from_slice(&cd_len.to_le_bytes());
    bytes.extend_from_slice(&cd_offset.to_le_bytes());
    bytes.extend_from_slice(&0u16.to_le_bytes());
    std::fs::write(archive, bytes).unwrap();
}

#[test]
fn cli_validate_reads_a_pinbundle_archive() {
    for (tag, dir, code) in [
        ("pass", c2_accept_bundle("cli-zip-pass"), 0),
        ("fail", c2_reject_bundle("cli-zip-fail"), 1),
    ] {
        let archive = temp_dir(&format!("cli-zip-{tag}-out")).join("table.pinbundle");
        zip_dir(&dir, &archive);
        // The archive itself is a valid bundle for the 4.1 loader.
        let vfs = Vfs::open(&archive).unwrap_or_else(|e| panic!("test zip not readable: {e}"));
        let report = validate_bundle(&vfs).unwrap_or_else(|e| panic!("{tag}: {e}"));
        assert_eq!(report.is_ok(), code == 0);
        let o = validate_cli(&archive);
        let (stdout, stderr) = out(&o);
        assert_no_panic(&o, "pinbundle");
        assert_eq!(o.status.code(), Some(code), "{tag}:\n{stdout}\n{stderr}");
        if code == 1 {
            assert!(format!("{stdout}{stderr}").contains("decks[0]"));
        }
    }
}

#[test]
fn cli_generator_command_line_still_works_next_to_validate() {
    // Any other first argument keeps the 4.3 command line.
    let work = temp_dir("cli-gen");
    let layout = work.join("layout.toml");
    let table = work.join("table_in.toml");
    std::fs::write(&layout, layout_toml(&deck(65.0))).unwrap();
    std::fs::write(
        &table,
        table_toml(&balls_restricted(&[STD, XL], XL, &[0], Some(&OPEN_AREA)), ""),
    )
    .unwrap();
    let outdir = work.join("bundle");
    std::fs::create_dir_all(&outdir).unwrap();
    copy_base_extras(&outdir);
    let o = cli(&[
        layout.as_os_str(),
        "--table".as_ref(),
        table.as_os_str(),
        "--out".as_ref(),
        outdir.as_os_str(),
    ]);
    assert!(o.status.success(), "{:?}", out(&o));
    // Generate, then validate: the two commands chain.
    let o = validate_cli(&outdir);
    assert_eq!(o.status.code(), Some(0), "{:?}", out(&o));
}

#[test]
#[ignore = "diagnostic"]
fn diag_cli_warn() {
    let transition = r#"
[[layer_transitions]]
id = "ramp_entry"
from_layer = 0
to_layer = 2
zone = { min = [140.0, 340.0, 0.0], max = [190.0, 380.0, 30.0] }
"#;
    let dir = bundle(
        "cli-warn",
        &layout_toml(""),
        &table_toml(&balls_restricted(&[STD, XL], XL, &[0], Some(&LOWER_AREA)), transition),
    );
    println!("{:?}", out(&validate_cli(&dir)));
    println!("{:?}", run_validate_cli(&[dir.to_string_lossy().into_owned()]).ok());
    println!("{:?}", out(&validate_cli(&c2_reject_bundle("d"))));
    println!("{:?}", out(&cli(&["validate".as_ref()])));
}
