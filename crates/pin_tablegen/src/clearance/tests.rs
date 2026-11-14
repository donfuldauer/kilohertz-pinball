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
