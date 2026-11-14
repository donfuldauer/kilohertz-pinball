//! Unit tests of the clearance validator's helpers (the table-level
//! behaviour is tested in `tests/clearance_*.rs`).

use super::geom::Polygon;
use super::*;

#[test]
fn feature_labels_are_distinct() {
    let all = [
        Feature::Lane,
        Feature::FlipperGap,
        Feature::Wireform,
        Feature::Scoop,
        Feature::VukExit,
        Feature::DeckHeadroom,
        Feature::Restriction,
    ];
    for (i, a) in all.iter().enumerate() {
        for b in &all[i + 1..] {
            assert_ne!(a.label(), b.label());
        }
    }
}

#[test]
fn allowed_regions() {
    let square = Polygon {
        points: vec![
            DVec2::new(0.0, 0.0),
            DVec2::new(1.0, 0.0),
            DVec2::new(1.0, 1.0),
            DVec2::new(0.0, 1.0),
        ],
    };
    let a = Allowed::Only(vec![(vec![0], Some(square)), (vec![2], None)]);
    assert!(a.restricted() && !Allowed::Anywhere.restricted());
    assert!(a.layer(0) && a.layer(2) && !a.layer(1));
    assert!(a.at(0, DVec2::new(0.5, 0.5)) && !a.at(0, DVec2::new(1.5, 0.5)));
    assert!(a.at(2, DVec2::new(9.0, 9.0)) && !a.at(1, DVec2::new(0.5, 0.5)));
    assert!(a.in_box(0, DVec2::new(0.9, 0.9), DVec2::new(2.0, 2.0)));
    assert!(!a.in_box(0, DVec2::new(1.5, 1.5), DVec2::new(2.0, 2.0)));
    assert!(
        Allowed::Anywhere.at(5, DVec2::ZERO)
            && Allowed::Anywhere.in_box(5, DVec2::ZERO, DVec2::ONE)
    );
    // A path whose vertices straddle the area without landing in it still
    // passes over it (a wireform sampled every 10 mm over a 1 mm area).
    let path = [DVec2::new(0.495, -1.0), DVec2::new(0.505, 2.0)];
    assert!(!path.iter().any(|&q| a.at(0, q)));
    assert!(a.on_path(0, &path));
    assert!(!a.on_path(1, &path));
    assert!(a.on_path(2, &path) && Allowed::Anywhere.on_path(1, &path));
    let beside = [DVec2::new(1.5, -1.0), DVec2::new(1.5, 2.0)];
    assert!(!a.on_path(0, &beside));
    assert!(a.on_path(0, &[DVec2::new(0.5, 0.5)]));
    // A path running along an area edge (collinear overlap, both ends
    // outside) passes over the area's boundary, which counts as inside
    // (`covers`), in either direction.
    let along = [DVec2::new(-1.0, 1.0), DVec2::new(2.0, 1.0)];
    assert!(a.on_path(0, &along));
    assert!(a.on_path(0, &[along[1], along[0]]));
    // Touching only a corner counts too; passing just beside it does not.
    assert!(a.on_path(0, &[DVec2::new(2.0, 0.0), DVec2::new(0.0, 2.0)]));
    assert!(!a.on_path(0, &[DVec2::new(2.0, 0.0), DVec2::new(0.0, 2.001)]));
    assert!(!a.on_path(0, &[DVec2::new(-1.0, 1.001), DVec2::new(2.0, 1.001)]));
}

#[test]
fn reports_convert_and_display() {
    let e = ClearanceError {
        feature: Feature::Lane,
        name: "l".into(),
        profile: "jumbo_xl_resin".into(),
        file: "table.toml".into(),
        path: "splines[0]".into(),
        required_mm: 80.4,
        actual_mm: 30.0,
        message: "too narrow".into(),
    };
    assert_eq!(e.to_string(), "table.toml: splines[0]: too narrow");
    let ok = ClearanceReport {
        errors: vec![],
        warnings: vec![e.clone()],
    };
    assert!(ok.is_ok());
    assert_eq!(ok.into_result().unwrap().len(), 1);
    let bad = ClearanceReport {
        errors: vec![e],
        warnings: vec![],
    };
    assert!(!bad.is_ok());
    assert_eq!(bad.into_result().unwrap_err()[0].path, "splines[0]");
    assert_eq!(mm(0.0794), "79.4 mm");
}

/// A ball resting on an inclined support is tangent to it, so under a
/// parallel inclined ceiling its headroom (`r + distance`) is the normal
/// separation; centring it `r` straight above the support overstated it by
/// `r (1 - n·z)` and let a too-low ceiling pass.
#[test]
fn resting_centre_on_an_inclined_support() {
    use super::geom::Tri;
    use glam::DVec3;
    let tri = |p: [DVec3; 3]| {
        let n = (p[1] - p[0]).cross(p[2] - p[0]).normalize();
        Tri {
            p,
            n,
            layer: 0,
            mesh: 0,
            min: p[0].min(p[1]).min(p[2]),
            max: p[0].max(p[1]).max(p[2]),
        }
    };
    // 30° slope rising along +x; ceiling parallel, `gap` above along the normal.
    let slope = 30f64.to_radians().tan();
    let plane = |dz: f64| {
        tri([
            DVec3::new(-1.0, -1.0, -slope + dz),
            DVec3::new(1.0, -1.0, slope + dz),
            DVec3::new(0.0, 1.0, dz),
        ])
    };
    let support = plane(0.0);
    let (r, gap) = (0.0135, 0.0275);
    let ceiling = plane(gap / support.nz());
    let q = DVec2::new(0.1, 0.0);
    let c = super::headroom::resting_centre(&support, q, r);
    assert!((support.distance(c) - r).abs() < 1e-12);
    let headroom = r + ceiling.distance(c);
    assert!((headroom - gap).abs() < 1e-12, "{headroom}");
    // A 27 mm ball needs 28 mm with the clearance margin: the 27.5 mm
    // separation is too low, which the old vertical offset hid.
    let need = 2.0 * r + f64::from(MIN_CLEARANCE_MM) * 1.0e-3;
    assert!(headroom < need, "{headroom} vs {need}");
    let old = DVec3::new(q.x, q.y, support.z_at(q) + r);
    assert!(r + ceiling.distance(old) > need);
}

/// The VUK exit support test measures to the interpolated spline the
/// physics uses, not to the chords between its control points.
#[test]
fn exit_support_measures_to_the_spline_not_its_chords() {
    use glam::{DVec3, Vec3};
    let pts = [[0.0f32, 0.0, 0.0], [0.3, 0.3, 0.0], [0.6, 0.0, 0.0]];
    let v: Vec<Vec3> = pts.iter().map(|&p| Vec3::from_array(p)).collect();
    let sp = super::geom::build_spline(&v, false).unwrap();
    let on_curve = sp.point(0.5).as_dvec3();
    let chord_mid = DVec3::new(0.15, 0.15, 0.0);
    // Distance from a point to the chord (0,0)-(0.3,0.3) (the nearer one).
    let to_chord = |q: DVec3| (q.x - q.y).abs() / 2f64.sqrt();
    assert!(to_chord(on_curve) > 5.0e-3, "{on_curve}");
    assert!(sp.closest_point(chord_mid.as_vec3(), None).distance > 5.0e-3);
    let reach = 1.0e-3;
    assert!(super::checks::near_spline(&pts, false, on_curve, reach));
    assert!(!super::checks::near_spline(&pts, false, chord_mid, reach));
    // A spline that does not construct falls back to the chords.
    let bad = [[0.0f32, 0.0, 0.0], [0.0, 0.0, 0.0]];
    let bad_v: Vec<Vec3> = bad.iter().map(|&p| Vec3::from_array(p)).collect();
    assert!(super::geom::build_spline(&bad_v, false).is_none());
    assert!(super::checks::near_spline(&bad, false, DVec3::ZERO, reach));
    assert!(!super::checks::near_spline(&bad, false, DVec3::X, reach));
}
