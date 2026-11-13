//! 4.1-H follow-ups done in Subphase 4.3: (a) spline coordinate and length
//! bounds, (b) no location on a "doubles back" warning that only the reach
//! raised, (c) lint text covering both a near-touching tip and a coil whose
//! clearance table ran out of budget.

#[path = "fixtures/support.rs"]
mod support;

use pin_assets::{AssetError, LoadedTable, ValidationError, Vfs, ZipLimits, load_table};
use support::*;

const WIREFORM: &str = "points = [[60.0, 900.0, 40.0], [40.0, 600.0, 55.0], [150.0, 260.0, 20.0]]";

fn load_text(table: &str) -> Result<LoadedTable, AssetError> {
    let files = with_file(&bundle_files(), "table.toml", table.as_bytes());
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    let vfs = Vfs::from_zip_bytes(stored_zip(&refs), ZipLimits::default()).unwrap();
    load_table(&vfs)
}

/// The Appendix A table with its wireform replaced by a spline of `kind`
/// through `points` (mm).
fn table_with(kind: &str, points: &[[f64; 3]]) -> String {
    let list: Vec<String> = points
        .iter()
        .map(|p| format!("[{:?}, {:?}, {:?}]", p[0], p[1], p[2]))
        .collect();
    let mut t = table_text();
    let replacement = format!("points = [{}]", list.join(", "));
    let roll = if kind == "wireform" {
        format!("roll_deg = [{}]", vec!["0.0"; points.len()].join(", "))
    } else {
        String::new()
    };
    for (from, to) in [
        ("kind = \"wireform\"", format!("kind = \"{kind}\"")),
        (
            "gauge = 19.0",
            if kind == "wireform" { "gauge = 19.0".into() } else { String::new() },
        ),
        ("roll_deg = [0.0, 8.0, 0.0]", roll),
        (WIREFORM, replacement),
    ] {
        assert!(t.contains(from), "fixture lacks {from:?}");
        t = t.replacen(from, &to, 1);
    }
    t
}

fn errors(kind: &str, points: &[[f64; 3]]) -> Vec<ValidationError> {
    match load_text(&table_with(kind, points)) {
        Ok(_) => Vec::new(),
        Err(AssetError::Invalid(e)) => e,
        Err(e) => panic!("{e}"),
    }
}

fn warnings(points: &[[f64; 3]]) -> Vec<ValidationError> {
    load_text(&table_with("guide", points))
        .unwrap_or_else(|e| panic!("{e}"))
        .warnings
        .into_iter()
        .filter(|w| w.path.starts_with("splines[0]"))
        .collect()
}

#[test]
fn a_points_beyond_10_m_are_refused() {
    for kind in ["guide", "wireform"] {
        let e = errors(
            kind,
            &[[0.0, 0.0, 0.0], [100.0, 0.0, 0.0], [10_000.5, 0.0, 0.0]],
        );
        assert_eq!(e.len(), 1, "{e:#?}");
        assert_eq!(e[0].path, "splines[0].points[2]");
        assert!(e[0].message.contains("within 10 m"), "{}", e[0]);
        // Euclidean distance, not per axis.
        let e = errors(kind, &[[0.0, 0.0, 0.0], [7100.0, 7100.0, 0.0]]);
        assert_eq!(e[0].path, "splines[0].points[1]", "{e:#?}");
    }
    // Just inside is fine.
    assert!(errors("guide", &[[0.0, 0.0, 0.0], [9999.0, 0.0, 0.0]]).is_empty());
}

#[test]
fn a_splines_longer_than_65_m_are_refused() {
    // Through-point chords alone already exceed 65 m.
    let zigzag: Vec<[f64; 3]> = (0..5)
        .map(|i| [if i % 2 == 0 { -9000.0 } else { 9000.0 }, i as f64, 0.0])
        .collect();
    let e = errors("guide", &zigzag);
    assert_eq!(e.len(), 1, "{e:#?}");
    assert_eq!(e[0].path, "splines[0].points");
    assert!(e[0].message.contains("at most 65 m"), "{}", e[0]);
    // Chords of 54 m, but the curve overshoots to 66 m between them.
    let overshoot = [
        [-9000.0, 0.0, 0.0],
        [9000.0, 0.0, 0.0],
        [9000.0, 0.1, 0.0],
        [-9000.0, 0.1, 0.0],
        [-9000.0, 0.2, 0.0],
        [9000.0, 0.2, 0.0],
    ];
    let e = errors("wireform", &overshoot);
    assert_eq!(e.len(), 1, "{e:#?}");
    assert_eq!(e[0].path, "splines[0].points");
    assert!(e[0].message.contains("66."), "{}", e[0]);
    // 60 m of straight rail is accepted.
    assert!(errors("guide", &[[-9000.0, 0.0, 0.0], [9000.0, 0.0, 0.0]]).is_empty());
}

/// A level Archimedean spiral (mm) whose clearance table runs out of
/// budget: `GuideCurve::reach()` is 0 although no stretch comes near another.
fn coil() -> Vec<[f64; 3]> {
    let (r0, pitch, turns, n) = (100.0, 40.0, 12.0, 600);
    (0..n)
        .map(|i| {
            let u = i as f64 / (n - 1) as f64;
            let a = core::f64::consts::TAU * turns * u;
            let r = r0 + pitch * turns * u;
            [r * a.cos(), r * a.sin(), 0.0]
        })
        .collect()
}

#[test]
fn b_c_reach_only_warning_has_no_location_and_covers_both_causes() {
    let pts: Vec<glam::Vec3> = coil()
        .iter()
        .map(|p| glam::Vec3::new(p[0] as f32, p[1] as f32, 0.0) * 1.0e-3)
        .collect();
    let guide = pin_physics::world::GuideCurve::new(
        pin_physics::world::CubicSpline::interpolate(&pts).unwrap(),
        0.0,
    )
    .unwrap();
    assert_eq!(guide.reach(), 0.0, "the fixture must exhaust the lane table");
    let w = warnings(&coil());
    let reach: Vec<_> = w
        .iter()
        .filter(|w| w.message.contains("no clearance could be proven"))
        .collect();
    assert_eq!(reach.len(), 1, "{w:#?}");
    let m = &reach[0].message;
    assert_eq!(reach[0].path, "splines[0]");
    assert!(!m.contains("points["), "no location: {m}");
    assert!(m.contains("close the guide or open the gap"), "{m}");
    assert!(m.contains("ran out of budget"), "{m}");
    assert!(!w.iter().any(|w| w.message.contains("doubles back")), "{w:#?}");
}

#[test]
fn b_geometric_doubling_back_keeps_its_location() {
    // An open Z whose diagonals cross: found geometrically, located.
    let w = warnings(&[
        [0.0, 0.0, 0.0],
        [100.0, 100.0, 0.0],
        [100.0, 0.0, 0.0],
        [0.0, 100.0, 0.0],
    ]);
    assert!(
        w.iter()
            .any(|w| w.message.contains("doubles back or touches itself near points[")),
        "{w:#?}"
    );
    assert!(!w.iter().any(|w| w.message.contains("no clearance could be proven")));
}
