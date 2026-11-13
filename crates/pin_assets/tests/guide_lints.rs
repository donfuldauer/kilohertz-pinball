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
    let found = lints(&corner, &[]);
    expect_lint(
        &found,
        &[
            "through-point spacing 50.0 mm",
            "smallest radius of curvature",
            "add through-points or soften the bend",
        ],
    );
}

#[test]
fn c_xl_ball_on_a_sloped_guide_tighter_than_its_radius() {
    // A rising quarter turn of radius 30 mm (spacing 7.8 mm), XL ball named
    // in balls.restrict (Appendix A) and then in balls.profiles.
    let ramp = arc(
        300.0,
        300.0,
        30.0,
        0.0,
        core::f64::consts::FRAC_PI_2,
        6,
        10.0,
    );
    let found = lints(&ramp, &[]);
    expect_lint(
        &found,
        &[
            "\"jumbo_xl_resin\" (radius 39.7 mm)",
            "sloped guide",
            "soften the bend or build the ramp as a wireform",
        ],
    );
    let profiles = (
        "profiles = [\"standard_steel\", \"ceramic_powerball\"]",
        "profiles = [\"standard_steel\", \"jumbo_xl_resin\"]",
    );
    expect_lint(
        &lints(&ramp, &[profiles, (RESTRICT, "")]),
        &["\"jumbo_xl_resin\""],
    );
    // Without an XL ball anywhere: no warning.
    assert!(lints(&ramp, &[(RESTRICT, "")]).is_empty());
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
    assert!(lints(&level, &[]).is_empty(), "{:#?}", lints(&level, &[]));
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

#[test]
fn d_open_end_touching_its_own_wall() {
    // A J (25 mm spacing, 50 mm bends): the tip comes down to 0.05 mm above
    // the base's through-point at x = 100 mm.
    let mut j: Vec<[f64; 3]> = (0..8).map(|i| [25.0 * f64::from(i), 0.0, 0.0]).collect();
    let deg = core::f64::consts::PI / 180.0;
    j.extend(arc(200.0, 50.0, 50.0, -90.0 * deg, 90.0 * deg, 6, 0.0));
    j.push([175.0, 100.0, 0.0]);
    j.extend(arc(150.0, 50.0, 50.0, 90.0 * deg, 180.0 * deg, 3, 0.0));
    j.push([100.0, 25.0, 0.0]);
    j.push([100.0, 0.05, 0.0]);
    let found = lints(&j, &[]);
    expect_lint(
        &found,
        &["no clearance proven", "close the guide or open the gap"],
    );
}
