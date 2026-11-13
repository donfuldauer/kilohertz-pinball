//! Guide authoring warnings (fix task 4.1-H, human ruling end of Wave 3,
//! item 4): each lint has a failing fixture asserting the exact path
//! `splines[i]`, the offending value and the advice; every lint is a
//! warning, never an error.

#[path = "fixtures/support.rs"]
mod support;

use pin_assets::{AssetError, LoadedTable, ValidationError, Vfs, ZipLimits, load_table};
use support::*;

const WIREFORM: &str = "points = [[60.0, 900.0, 40.0], [40.0, 600.0, 55.0], [150.0, 260.0, 20.0]]";
const RESTRICT: &str = "restrict = [ { profile = \"jumbo_xl_resin\", layers = [0] } ]";

fn load_text(table: &str) -> Result<LoadedTable, AssetError> {
    let files = with_file(&bundle_files(), "table.toml", table.as_bytes());
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    let vfs = Vfs::from_zip_bytes(stored_zip(&refs), ZipLimits::default()).unwrap();
    load_table(&vfs)
}

/// The Appendix A table with its wireform turned into a guide through
/// `points` (mm), plus further `(from, to)` edits.
fn guide_table(points: &[[f64; 3]], edits: &[(&str, &str)]) -> String {
    let list: Vec<String> = points
        .iter()
        .map(|p| format!("[{}, {}, {}]", p[0], p[1], p[2]))
        .collect();
    let mut t = table_text();
    let replacement = format!("points = [{}]", list.join(", "));
    for (from, to) in [
        ("kind = \"wireform\"", "kind = \"guide\""),
        ("gauge = 19.0", ""),
        ("roll_deg = [0.0, 8.0, 0.0]", ""),
        (WIREFORM, replacement.as_str()),
    ]
    .into_iter()
    .chain(edits.iter().copied())
    {
        assert!(t.contains(from), "fixture lacks {from:?}");
        t = t.replacen(from, to, 1);
    }
    t
}

/// Loads (must succeed: lints are warnings) and returns the warnings at
/// `splines[0]`.
fn lints(points: &[[f64; 3]], edits: &[(&str, &str)]) -> Vec<ValidationError> {
    let loaded = load_text(&guide_table(points, edits)).unwrap();
    loaded
        .warnings
        .into_iter()
        .filter(|w| w.path.starts_with("splines[0]"))
        .collect()
}

/// Exactly one lint, at `splines[0]`, whose message contains every needle.
fn expect_lint(found: &[ValidationError], needles: &[&str]) {
    assert_eq!(found.len(), 1, "{found:#?}");
    assert_eq!(found[0].file, "table.toml");
    assert_eq!(found[0].path, "splines[0]");
    for n in needles {
        assert!(found[0].message.contains(n), "{n:?} not in {found:#?}");
    }
}

/// `n + 1` points on an arc of radius `r` (mm) around `(cx, cy)` from `a0`
/// to `a1` (radians), rising linearly by `rise` mm.
fn arc(cx: f64, cy: f64, r: f64, a0: f64, a1: f64, n: usize, rise: f64) -> Vec<[f64; 3]> {
    (0..=n)
        .map(|i| {
            let f = i as f64 / n as f64;
            let a = a0 + (a1 - a0) * f;
            [cx + r * a.cos(), cy + r * a.sin(), rise * f]
        })
        .collect()
}

#[test]
fn clean_guides_do_not_warn() {
    // Half circle, radius 100 mm, spacing 52 mm.
    let half = arc(300.0, 300.0, 100.0, 0.0, core::f64::consts::PI, 6, 0.0);
    assert!(lints(&half, &[]).is_empty(), "{:#?}", lints(&half, &[]));
    // A C whose ends are 2 mm apart: not a hairline gap.
    let gap = 2.0 / 100.0;
    let c = arc(
        300.0,
        300.0,
        100.0,
        0.0,
        core::f64::consts::TAU - gap,
        12,
        0.0,
    );
    assert!(lints(&c, &[]).is_empty(), "{:#?}", lints(&c, &[]));
    // A sloped arc (radius 60 mm) with the XL ball (radius 39.7 mm): fine.
    let ramp = arc(
        300.0,
        300.0,
        60.0,
        0.0,
        core::f64::consts::FRAC_PI_2,
        4,
        20.0,
    );
    assert!(lints(&ramp, &[]).is_empty(), "{:#?}", lints(&ramp, &[]));
}

#[test]
fn b_spacing_at_or_above_the_smallest_radius() {
    // A right-angle corner with 50 mm spacing: the bend is far tighter.
    let corner = [
        [0.0, 0.0, 0.0],
        [50.0, 0.0, 0.0],
        [100.0, 0.0, 0.0],
        [100.0, 50.0, 0.0],
        [100.0, 100.0, 0.0],
    ];
    expect_lint(
        &lints(&corner, &[]),
        &[
            "through-point spacing 50.0 mm",
            "radius of curvature is",
            "add through-points or soften the bend",
        ],
    );
}

#[test]
fn b_is_judged_against_the_spacing_next_to_the_bend() {
    // A long straight lead-in (150 mm spacing) and then a 40 mm-radius bend
    // through points 10 mm apart: the bend is fine where it is; no warning.
    let deg = core::f64::consts::PI / 180.0;
    let mut pts: Vec<[f64; 3]> = vec![[-450.0, 0.0, 0.0], [-300.0, 0.0, 0.0], [-150.0, 0.0, 0.0]];
    // 150 → 10 mm is uneven spacing (lint a) but no (b).
    pts.extend((0..4).map(|i| [-40.0 + 10.0 * f64::from(i), 0.0, 0.0]));
    pts.extend(arc(0.0, 40.0, 40.0, -90.0 * deg, 0.0, 6, 0.0));
    let found: Vec<_> = lints(&pts, &[])
        .into_iter()
        .filter(|w| w.path == "splines[0]")
        .collect();
    assert!(
        !found
            .iter()
            .any(|w| w.message.contains("radius of curvature")),
        "{found:#?}"
    );
    // The hint names the spacing next to the bend: here points[2] to [3].
    let tight = [
        [0.0, 0.0, 0.0],
        [10.0, 0.0, 0.0],
        [20.0, 0.0, 0.0],
        [60.0, 40.0, 0.0],
        [100.0, 0.0, 0.0],
        [110.0, 0.0, 0.0],
    ];
    let found = lints(&tight, &[]);
    let b: Vec<_> = found
        .iter()
        .filter(|w| w.message.contains("add through-points"))
        .collect();
    assert_eq!(b.len(), 1, "{found:#?}");
    assert_eq!(b[0].path, "splines[0]");
    assert!(
        b[0].message.contains("points[2] to points[3]")
            || b[0].message.contains("points[3] to points[4]"),
        "{}",
        b[0].message
    );
}

#[test]
fn b_reports_collinear_reversals_as_cusps() {
    // Runs to 100 mm, back to 50 mm, on to 150 mm: no curvature anywhere,
    // but the tangent reverses.
    let back = [
        [0.0, 0.0, 0.0],
        [100.0, 0.0, 0.0],
        [50.0, 0.0, 0.0],
        [150.0, 0.0, 0.0],
    ];
    let found = lints(&back, &[]);
    assert!(
        found
            .iter()
            .any(|w| w.path == "splines[0]" && w.message.contains("reverses (a cusp")),
        "{found:#?}"
    );
}

/// The Appendix A spline sits on layer 2; these move it to layer 0.
const LAYER_0: (&str, &str) = ("layer = 2\n", "layer = 0\n");

#[test]
fn c_xl_ball_on_a_sloped_guide_tighter_than_its_radius() {
    // A rising quarter turn of radius 30 mm (spacing 7.8 mm), XL ball named
    // in balls.restrict (Appendix A: layer 0 only) and then in balls.profiles.
    let ramp = arc(
        300.0,
        300.0,
        30.0,
        0.0,
        core::f64::consts::FRAC_PI_2,
        6,
        10.0,
    );
    expect_lint(
        &lints(&ramp, &[LAYER_0]),
        &[
            "\"jumbo_xl_resin\" (radius 39.7 mm)",
            "sloped guide",
            "soften the bend or build the ramp as a wireform",
        ],
    );
    // balls.restrict keeps the XL ball off layer 2: no warning there.
    assert!(lints(&ramp, &[]).is_empty(), "{:#?}", lints(&ramp, &[]));
    // Restricted to layers 1 and 2: warns on layer 2, not on 0.
    let restrict_12 = (
        RESTRICT,
        "restrict = [ { profile = \"jumbo_xl_resin\", layers = [1, 2] } ]",
    );
    expect_lint(&lints(&ramp, &[restrict_12]), &["\"jumbo_xl_resin\""]);
    assert!(lints(&ramp, &[restrict_12, LAYER_0]).is_empty());
    // Listed in balls.profiles without a restriction: any layer.
    let profiles = (
        "profiles = [\"standard_steel\", \"ceramic_powerball\"]",
        "profiles = [\"standard_steel\", \"jumbo_xl_resin\"]",
    );
    expect_lint(
        &lints(&ramp, &[profiles, (RESTRICT, "")]),
        &["\"jumbo_xl_resin\""],
    );
    // Without an XL ball anywhere: no warning.
    assert!(lints(&ramp, &[(RESTRICT, ""), LAYER_0]).is_empty());
    // The same bend level: the lint is for sloped guides only.
    let level = arc(
        300.0,
        300.0,
        30.0,
        0.0,
        core::f64::consts::FRAC_PI_2,
        6,
        0.0,
    );
    assert!(
        lints(&level, &[LAYER_0]).is_empty(),
        "{:#?}",
        lints(&level, &[LAYER_0])
    );
}

#[test]
fn d_hairline_gap_between_open_ends() {
    // A C of radius 100 mm whose ends are 0.5 mm apart (above the 1e-6 m snap).
    let gap = 0.5 / 100.0;
    let c = arc(
        300.0,
        300.0,
        100.0,
        0.0,
        core::f64::consts::TAU - gap,
        12,
        0.0,
    );
    let loaded = load_text(&guide_table(&c, &[])).unwrap();
    assert!(!loaded.splines[0].closed);
    let found: Vec<_> = loaded
        .warnings
        .into_iter()
        .filter(|w| w.path.starts_with("splines[0]"))
        .collect();
    expect_lint(
        &found,
        &["0.500 mm apart", "close the guide or open the gap"],
    );
}

/// A J (25 mm spacing, 50 mm bends) whose tip comes down to `gap` mm above
/// the base's through-point at x = 100 mm.
fn j_shape(gap: f64) -> Vec<[f64; 3]> {
    let mut j: Vec<[f64; 3]> = (0..8).map(|i| [25.0 * f64::from(i), 0.0, 0.0]).collect();
    let deg = core::f64::consts::PI / 180.0;
    j.extend(arc(200.0, 50.0, 50.0, -90.0 * deg, 90.0 * deg, 6, 0.0));
    j.push([175.0, 100.0, 0.0]);
    j.extend(arc(150.0, 50.0, 50.0, 90.0 * deg, 180.0 * deg, 3, 0.0));
    j.push([100.0, 25.0, 0.0]);
    j.push([100.0, gap, 0.0]);
    j
}

#[test]
fn d_open_end_near_its_own_wall() {
    // Measured directly: warns up to (not at) 1 mm, like the end-to-end gap.
    for (gap, text) in [
        (0.0, "0.000 mm"),
        (0.05, "0.050 mm"),
        (0.3, "0.300 mm"),
        (0.9, "0.900 mm"),
    ] {
        expect_lint(
            &lints(&j_shape(gap), &[]),
            &[
                text,
                "from the guide's own wall near points[4]",
                "close the guide or open the gap",
            ],
        );
    }
    for gap in [1.1, 5.0, 10.0] {
        let found = lints(&j_shape(gap), &[]);
        assert!(found.is_empty(), "{gap}: {found:#?}");
    }
}

#[test]
fn d_doubling_back_is_not_called_a_gap() {
    // The reviewer's shapes: no open end is near a wall, but the curve
    // doubles back (no reach). A separate warning, without the gap advice.
    let deg = core::f64::consts::PI / 180.0;
    let mut span: Vec<[f64; 3]> = vec![[0.0, 0.0, 0.0], [300.0, 0.0, 0.0]];
    span.extend((1..5).map(|i| [300.0 + 20.0 * f64::from(i), 0.0, 0.0]));
    span.extend(arc(400.0, 100.0, 100.0, -90.0 * deg, 90.0 * deg, 12, 0.0));
    let shapes: Vec<Vec<[f64; 3]>> = vec![
        vec![
            [0.0, 0.0, 0.0],
            [100.0, 0.0, 0.0],
            [50.0, 0.0, 0.0],
            [150.0, 0.0, 0.0],
        ],
        vec![[0.0, 0.0, 0.0], [100.0, 5.0, 0.0], [0.0, 10.0, 0.0]],
        vec![
            [0.0, 0.0, 0.0],
            [10.0, 0.0, 0.0],
            [100.0, 0.0, 0.0],
            [110.0, 50.0, 0.0],
        ],
        span,
    ];
    for pts in shapes {
        let found = lints(&pts, &[]);
        assert!(
            !found
                .iter()
                .any(|w| w.message.contains("close the guide or open the gap")),
            "{pts:?}: {found:#?}"
        );
        let back: Vec<_> = found
            .iter()
            .filter(|w| {
                w.message
                    .contains("doubles back or touches itself near points[")
            })
            .collect();
        assert_eq!(back.len(), 1, "{pts:?}: {found:#?}");
        assert_eq!(back[0].path, "splines[0]");
        assert!(
            back[0]
                .message
                .contains("even out the through-point spacing")
        );
    }
}
