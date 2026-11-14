//! Black-box spec tests for `pin_core::units` (PLAN.md §3.1, Subphase 1.1 brief).

use pin_core::units::{deg_to_rad, g_to_kg, kg_to_g, m_to_mm, mm_to_m, rad_to_deg};
use proptest::prelude::*;

fn close(a: f32, b: f32, rel: f32) -> bool {
    let scale = a.abs().max(b.abs());
    (a - b).abs() <= rel * scale || (a - b).abs() <= f32::MIN_POSITIVE
}

#[test]
fn known_lengths() {
    assert!(close(mm_to_m(76.0), 0.076, 1e-6));
    assert!(close(mm_to_m(27.0), 0.027, 1e-6));
    assert!(close(mm_to_m(79.4), 0.0794, 1e-6));
    assert!(close(mm_to_m(1000.0), 1.0, 1e-6));
    assert!(close(m_to_mm(1.0), 1000.0, 1e-6));
    assert!(close(m_to_mm(0.514), 514.0, 1e-6));
    assert_eq!(mm_to_m(0.0), 0.0);
    assert_eq!(m_to_mm(0.0), 0.0);
}

#[test]
fn known_masses() {
    assert!(close(g_to_kg(80.0), 0.08, 1e-6));
    assert!(close(g_to_kg(445.0), 0.445, 1e-6));
    assert!(close(g_to_kg(1000.0), 1.0, 1e-6));
    assert!(close(kg_to_g(0.08), 80.0, 1e-6));
    assert!(close(kg_to_g(1.0), 1000.0, 1e-6));
    assert_eq!(g_to_kg(0.0), 0.0);
    assert_eq!(kg_to_g(0.0), 0.0);
}

#[test]
fn known_angles() {
    let pi = std::f32::consts::PI;
    assert!(close(deg_to_rad(180.0), pi, 1e-6));
    assert!(close(deg_to_rad(90.0), pi / 2.0, 1e-6));
    assert!(close(deg_to_rad(-30.0), -pi / 6.0, 1e-6));
    // Default playfield pitch, 6.5 degrees.
    assert!(close(deg_to_rad(6.5), 0.113_446_4, 1e-6));
    assert!(close(rad_to_deg(pi), 180.0, 1e-6));
    assert!(close(rad_to_deg(0.113_446_4), 6.5, 1e-6));
    assert_eq!(deg_to_rad(0.0), 0.0);
    assert_eq!(rad_to_deg(0.0), 0.0);
}

proptest! {
    #[test]
    fn mm_m_round_trip(x in -1.0e6f32..1.0e6f32) {
        prop_assert!(close(m_to_mm(mm_to_m(x)), x, 1e-5));
        prop_assert!(close(mm_to_m(m_to_mm(x)), x, 1e-5));
    }

    #[test]
    fn g_kg_round_trip(x in -1.0e6f32..1.0e6f32) {
        prop_assert!(close(kg_to_g(g_to_kg(x)), x, 1e-5));
        prop_assert!(close(g_to_kg(kg_to_g(x)), x, 1e-5));
    }

    #[test]
    fn deg_rad_round_trip(x in -1.0e5f32..1.0e5f32) {
        prop_assert!(close(rad_to_deg(deg_to_rad(x)), x, 1e-5));
        prop_assert!(close(deg_to_rad(rad_to_deg(x)), x, 1e-5));
    }

    #[test]
    fn scale_factors_are_exactly_a_thousand(x in -1.0e6f32..1.0e6f32) {
        prop_assert!(close(mm_to_m(x), x / 1000.0, 1e-6));
        prop_assert!(close(m_to_mm(x), x * 1000.0, 1e-6));
        prop_assert!(close(g_to_kg(x), x / 1000.0, 1e-6));
        prop_assert!(close(kg_to_g(x), x * 1000.0, 1e-6));
    }

    #[test]
    fn angle_factor_is_pi_over_180(x in -1.0e5f32..1.0e5f32) {
        let expect = (f64::from(x) * std::f64::consts::PI / 180.0) as f32;
        prop_assert!(close(deg_to_rad(x), expect, 1e-6));
        let expect = (f64::from(x) * 180.0 / std::f64::consts::PI) as f32;
        prop_assert!(close(rad_to_deg(x), expect, 1e-6));
    }

    #[test]
    fn conversions_preserve_sign_and_order(a in -1.0e6f32..1.0e6f32, b in -1.0e6f32..1.0e6f32) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        prop_assert!(mm_to_m(lo) <= mm_to_m(hi));
        prop_assert!(m_to_mm(lo) <= m_to_mm(hi));
        prop_assert!(g_to_kg(lo) <= g_to_kg(hi));
        prop_assert!(kg_to_g(lo) <= kg_to_g(hi));
        prop_assert!(deg_to_rad(lo) <= deg_to_rad(hi));
        prop_assert!(rad_to_deg(lo) <= rad_to_deg(hi));
        prop_assert!(mm_to_m(-a) == -mm_to_m(a));
        prop_assert!(deg_to_rad(-a) == -deg_to_rad(a));
    }

    #[test]
    fn finite_in_finite_out(x in -1.0e30f32..1.0e30f32) {
        prop_assert!(mm_to_m(x).is_finite());
        prop_assert!(m_to_mm(x).is_finite());
        prop_assert!(g_to_kg(x).is_finite());
        prop_assert!(kg_to_g(x).is_finite());
        prop_assert!(deg_to_rad(x).is_finite());
        prop_assert!(rad_to_deg(x).is_finite());
    }
}
