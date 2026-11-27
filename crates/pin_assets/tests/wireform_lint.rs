//! KP-056 (issue #137), SPEC §4 Subphase 4.1 (validation) and §2.1 (wireform
//! frames): `load_table` warns, never errors, about a wireform whose
//! uniformly parametrized natural cubic centerline nearly stops (or reverses)
//! within a segment, and stays quiet about normal wireforms and guides.

#[path = "fixtures/support.rs"]
mod support;

use pin_assets::{LoadedTable, ValidationError, Vfs, ZipLimits, load_table};
use support::*;

const WIREFORM_POINTS: &str =
    "points = [[60.0, 900.0, 40.0], [40.0, 600.0, 55.0], [150.0, 260.0, 20.0]]";
const ROLL: &str = "roll_deg = [0.0, 8.0, 0.0]";

fn load_text(table: &str) -> LoadedTable {
    let files = with_file(&bundle_files(), "table.toml", table.as_bytes());
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    let vfs = Vfs::from_zip_bytes(stored_zip(&refs), ZipLimits::default()).unwrap();
    load_table(&vfs).unwrap_or_else(|e| panic!("lint must be a warning, not an error: {e}"))
}

/// The Appendix A table with its wireform replaced by one through `points`
/// (mm); `kind` is `wireform` or `guide`.
fn table_with(points: &[[f64; 3]], kind: &str) -> String {
    let list: Vec<String> = points
        .iter()
        .map(|p| format!("[{:?}, {:?}, {:?}]", p[0], p[1], p[2]))
        .collect();
    let rolls = vec!["0.0"; points.len()].join(", ");
    let mut t = table_text();
    let edits: Vec<(&str, String)> = if kind == "wireform" {
        vec![
            (WIREFORM_POINTS, format!("points = [{}]", list.join(", "))),
            (ROLL, format!("roll_deg = [{rolls}]")),
        ]
    } else {
        vec![
            ("kind = \"wireform\"", "kind = \"guide\"".to_owned()),
            ("gauge = 19.0", String::new()),
            (ROLL, String::new()),
            (WIREFORM_POINTS, format!("points = [{}]", list.join(", "))),
        ]
    };
    for (from, to) in edits {
        assert!(t.contains(from), "fixture lacks {from:?}");
        t = t.replacen(from, &to, 1);
    }
    t
}

fn cusp_warnings(loaded: &LoadedTable) -> Vec<&ValidationError> {
    loaded
        .warnings
        .iter()
        .filter(|w| w.message.contains("nearly vanishes"))
        .collect()
}

/// `n` points on an arc of radius `r` mm with chord spacing about `step` mm.
fn arc(n: usize, r: f64, step: f64) -> Vec<[f64; 3]> {
    let da = step / r;
    (0..n)
        .map(|i| {
            let a = da * i as f64;
            [300.0 + r * libm::cos(a), 300.0 + r * libm::sin(a), 40.0]
        })
        .collect()
}

/// SPEC 4.1 / 2.1 (kp056 acceptance: a near-cusp wireform fixture warns).
#[test]
fn kp056_near_cusp_wireforms_warn_at_the_spline() {
    let fixtures: [&[[f64; 3]]; 2] = [
        &[
            [100.0, 100.0, 40.0],
            [100.0, 161.0, 40.0],
            [100.0, 461.0, 40.0],
        ],
        // The curve reverses: ratio 0.
        &[
            [100.0, 100.0, 40.0],
            [100.0, 140.0, 40.0],
            [100.0, 440.0, 40.0],
        ],
    ];
    for pts in fixtures {
        let loaded = load_text(&table_with(pts, "wireform"));
        let found = cusp_warnings(&loaded);
        assert_eq!(found.len(), 1, "{pts:?}: {:#?}", loaded.warnings);
        let w = found[0];
        assert_eq!(w.file, "table.toml");
        // The wireform is splines[i]; find which index the fixture has.
        assert!(w.path.starts_with("splines["), "{w:?}");
        assert!(w.message.contains("wireform"), "{w:?}");
        assert!(
            w.message.contains("points[0]") && w.message.contains("points[1]"),
            "segment not named: {w:?}"
        );
    }
}

/// SPEC 4.1 / 2.1: the warning's path indexes the wireform in `[[splines]]`.
#[test]
fn kp056_near_cusp_warning_path_is_the_wireform_index() {
    let text = table_with(
        &[
            [100.0, 100.0, 40.0],
            [100.0, 140.0, 40.0],
            [100.0, 440.0, 40.0],
        ],
        "wireform",
    );
    let loaded = load_text(&text);
    let w = cusp_warnings(&loaded);
    assert_eq!(w.len(), 1, "{:#?}", loaded.warnings);
    let table = pin_core::schema::TableManifest::from_toml_str(&text).unwrap();
    let idx = table
        .splines
        .iter()
        .position(|s| s.id == "return_wireform")
        .unwrap();
    assert_eq!(w[0].path, format!("splines[{idx}]"));
}

/// SPEC 4.1 / 2.1 (kp056 acceptance): normal wireforms stay quiet, across a
/// sweep of arcs of 4..8 points and spacings.
#[test]
fn kp056_normal_wireforms_do_not_warn() {
    let mut fixtures: Vec<Vec<[f64; 3]>> = vec![
        vec![
            [100.0, 100.0, 40.0],
            [100.0, 160.0, 40.0],
            [100.0, 220.0, 40.0],
        ],
        vec![
            [60.0, 900.0, 40.0],
            [40.0, 600.0, 55.0],
            [150.0, 260.0, 20.0],
        ],
    ];
    for n in 4..=8 {
        for step in [40.0, 60.0, 100.0] {
            fixtures.push(arc(n, 400.0, step));
        }
    }
    for pts in &fixtures {
        let loaded = load_text(&table_with(pts, "wireform"));
        assert!(
            cusp_warnings(&loaded).is_empty(),
            "{pts:?}: {:#?}",
            loaded.warnings
        );
    }
    // The untouched Appendix A table.
    let loaded = load_text(&table_text());
    assert!(cusp_warnings(&loaded).is_empty(), "{:#?}", loaded.warnings);
}

/// SPEC 4.1 (human ruling: guide lints are warnings only): the same points
/// declared as a guide never draw the near-cusp warning.
#[test]
fn kp056_guides_never_draw_the_near_cusp_warning() {
    for pts in [
        [
            [100.0, 100.0, 40.0],
            [100.0, 161.0, 40.0],
            [100.0, 461.0, 40.0],
        ],
        [
            [100.0, 100.0, 40.0],
            [100.0, 140.0, 40.0],
            [100.0, 440.0, 40.0],
        ],
    ] {
        let loaded = load_text(&table_with(&pts, "guide"));
        assert!(cusp_warnings(&loaded).is_empty(), "{:#?}", loaded.warnings);
    }
}
