//! `CurvedBanana` arc_sweep_deg lint: a warning, never an error.

#[path = "fixtures/support.rs"]
mod support;

use pin_assets::{AssetError, LoadedTable, ValidationError, Vfs, ZipLimits, load_table};
use support::*;

const STRAIGHT: &str =
    "shape = { type = \"StraightCapsule\", length = 76.0, base_radius = 11.0, tip_radius = 6.5 }";

fn load_text(table: &str) -> Result<LoadedTable, AssetError> {
    let files = with_file(&bundle_files(), "table.toml", table.as_bytes());
    let refs: Vec<(&str, &[u8])> = files
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    let vfs = Vfs::from_zip_bytes(stored_zip(&refs), ZipLimits::default()).unwrap();
    load_table(&vfs)
}

fn banana(len: &str, r: &str, sweep: &str) -> String {
    format!(
        "shape = {{ type = \"CurvedBanana\", length = {len}, arc_radius = {r}, arc_sweep_deg = {sweep}, tube_radius = 8.0 }}"
    )
}

fn lints(shape: &str) -> Vec<ValidationError> {
    lints_opt(shape).expect("warnings still load")
}

/// The load result: `Err` when other checks reject the table, else the
/// `arc_sweep_deg` warnings of the loaded table.
fn lints_opt(shape: &str) -> Result<Vec<ValidationError>, AssetError> {
    let t = table_text();
    assert!(t.contains(STRAIGHT));
    let loaded = load_text(&t.replacen(STRAIGHT, shape, 1))?;
    Ok(loaded
        .warnings
        .into_iter()
        .filter(|w| w.path.contains("arc_sweep_deg"))
        .collect())
}

#[test]
fn warns_for_80_on_100_at_30() {
    let w = lints(&banana("80.0", "100.0", "30.0"));
    assert_eq!(w.len(), 1, "{w:?}");
    assert_eq!(w[0].path, "flippers[0].shape.arc_sweep_deg");
    for n in ["30.0", "47.2", "tolerance 1", "only the sign"] {
        assert!(w[0].message.contains(n), "{n}: {}", w[0].message);
    }
}

#[test]
fn matching_and_tolerance_band() {
    // 76 mm on R 100 implies 44.7 deg (44.70...).
    let implied: f64 = 2.0 * libm::asin(76.0f64 / 200.0_f64).to_degrees();
    assert!((implied - 44.7).abs() < 0.05);
    for d in [0.0, 0.9, -0.9] {
        for sign in ["", "-"] {
            let v = format!("{sign}{}", implied + d);
            assert!(lints(&banana("76.0", "100.0", &v)).is_empty(), "{v}");
        }
    }
    for d in [1.1, -1.1] {
        for sign in ["", "-"] {
            let v = format!("{sign}{}", implied + d);
            assert_eq!(lints(&banana("76.0", "100.0", &v)).len(), 1, "{v}");
        }
    }
}

#[test]
fn mirror_image_flippers_agree() {
    let a = lints(&banana("80.0", "100.0", "30.0"));
    let b = lints(&banana("80.0", "100.0", "-30.0"));
    assert_eq!((a.len(), b.len()), (1, 1));
}

#[test]
fn impossible_arcs_do_not_warn_or_panic() {
    // Geometry that loads but is impossible: loads, and the lint stays silent.
    for (l, r, s) in [
        ("250.0", "100.0", "30.0"),
        ("80.0", "0.0", "30.0"),
        ("80.0", "-5.0", "30.0"),
    ] {
        let w =
            lints_opt(&banana(l, r, s)).unwrap_or_else(|e| panic!("{l} {r} {s} must load: {e:?}"));
        assert!(w.is_empty(), "{l} {r} {s}: {w:?}");
    }
    // Non-finite numbers are rejected by the manifest check, never by this lint.
    for (l, r, s) in [
        ("nan", "100.0", "30.0"),
        ("80.0", "nan", "30.0"),
        ("80.0", "100.0", "nan"),
        ("80.0", "100.0", "inf"),
    ] {
        match lints_opt(&banana(l, r, s)) {
            Err(AssetError::Invalid(errs)) => {
                assert!(
                    errs.iter().all(|e| e.message.contains("finite number")),
                    "{l} {r} {s}: {errs:?}"
                );
                assert!(errs.iter().all(|e| !e.path.contains("arc_sweep_deg")));
            }
            other => panic!("{l} {r} {s} must be rejected as non-finite: {other:?}"),
        }
    }
}

#[test]
fn straight_capsule_does_not_warn() {
    assert!(lints(STRAIGHT).is_empty());
}
