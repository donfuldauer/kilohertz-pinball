//! Unit conversions between on-disk units (mm, g, degrees) and SI (D17).
//!
//! All helpers are plain `f32` arithmetic with no transcendental functions, so
//! they are bit-identical on every platform (§2.4).

/// Millimetres per metre.
pub const MM_PER_M: f32 = 1000.0;
/// Grams per kilogram.
pub const G_PER_KG: f32 = 1000.0;
/// Radians per degree, `π / 180`.
pub const RAD_PER_DEG: f32 = core::f32::consts::PI / 180.0;
/// Degrees per radian, `180 / π`.
pub const DEG_PER_RAD: f32 = 180.0 / core::f32::consts::PI;

/// Millimetres to metres.
#[inline]
pub fn mm_to_m(mm: f32) -> f32 {
    mm / MM_PER_M
}

/// Metres to millimetres.
#[inline]
pub fn m_to_mm(m: f32) -> f32 {
    m * MM_PER_M
}

/// Grams to kilograms.
#[inline]
pub fn g_to_kg(g: f32) -> f32 {
    g / G_PER_KG
}

/// Kilograms to grams.
#[inline]
pub fn kg_to_g(kg: f32) -> f32 {
    kg * G_PER_KG
}

/// Degrees to radians.
#[inline]
pub fn deg_to_rad(deg: f32) -> f32 {
    deg * RAD_PER_DEG
}

/// Radians to degrees.
#[inline]
pub fn rad_to_deg(rad: f32) -> f32 {
    rad * DEG_PER_RAD
}

/// Converts an mm vector to metres component-wise.
#[inline]
pub fn mm3_to_m(v: [f32; 3]) -> [f32; 3] {
    [mm_to_m(v[0]), mm_to_m(v[1]), mm_to_m(v[2])]
}

/// Converts a metre vector to mm component-wise.
#[inline]
pub fn m3_to_mm(v: [f32; 3]) -> [f32; 3] {
    [m_to_mm(v[0]), m_to_mm(v[1]), m_to_mm(v[2])]
}

/// Milliseconds to seconds.
#[inline]
pub fn ms_to_s(ms: f32) -> f32 {
    ms / 1000.0
}

/// Seconds to milliseconds.
#[inline]
pub fn s_to_ms(s: f32) -> f32 {
    s * 1000.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn exact_reference_values() {
        assert_eq!(mm_to_m(1000.0), 1.0);
        assert_eq!(mm_to_m(27.0), 0.027);
        assert_eq!(m_to_mm(1.0), 1000.0);
        assert_eq!(g_to_kg(80.0), 0.08);
        assert_eq!(kg_to_g(1.0), 1000.0);
        assert_eq!(deg_to_rad(180.0), core::f32::consts::PI);
        assert!((rad_to_deg(core::f32::consts::PI) - 180.0).abs() < 1e-4);
        assert_eq!(deg_to_rad(0.0), 0.0);
        assert_eq!(mm3_to_m([1000.0, -2000.0, 0.0]), [1.0, -2.0, 0.0]);
        assert_eq!(m3_to_mm([1.0, -2.0, 0.5]), [1000.0, -2000.0, 500.0]);
        assert_eq!(ms_to_s(60.0), 0.06);
        assert_eq!(s_to_ms(0.5), 500.0);
    }

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() <= 1e-5 * b.abs().max(1.0)
    }

    proptest! {
        #[test]
        fn mm_round_trip(x in -1.0e6f32..1.0e6) {
            prop_assert!(close(m_to_mm(mm_to_m(x)), x));
            prop_assert!(close(mm_to_m(m_to_mm(x)), x));
        }

        #[test]
        fn mass_round_trip(x in -1.0e6f32..1.0e6) {
            prop_assert!(close(kg_to_g(g_to_kg(x)), x));
            prop_assert!(close(g_to_kg(kg_to_g(x)), x));
        }

        #[test]
        fn angle_round_trip(x in -1.0e4f32..1.0e4) {
            prop_assert!(close(rad_to_deg(deg_to_rad(x)), x));
            prop_assert!(close(deg_to_rad(rad_to_deg(x)), x));
        }

        #[test]
        fn conversions_are_monotonic_and_odd(a in -1.0e6f32..1.0e6, b in -1.0e6f32..1.0e6) {
            if a <= b {
                prop_assert!(mm_to_m(a) <= mm_to_m(b));
                prop_assert!(g_to_kg(a) <= g_to_kg(b));
                prop_assert!(deg_to_rad(a) <= deg_to_rad(b));
                prop_assert!(m_to_mm(a) <= m_to_mm(b));
                prop_assert!(kg_to_g(a) <= kg_to_g(b));
                prop_assert!(rad_to_deg(a) <= rad_to_deg(b));
            }
            prop_assert_eq!(mm_to_m(-a), -mm_to_m(a));
            prop_assert_eq!(deg_to_rad(-a), -deg_to_rad(a));
        }

        #[test]
        fn time_round_trip(x in -1.0e6f32..1.0e6) {
            prop_assert!(close(s_to_ms(ms_to_s(x)), x));
        }

        #[test]
        fn vec_helpers_match_scalar(x in -1.0e5f32..1.0e5, y in -1.0e5f32..1.0e5, z in -1.0e5f32..1.0e5) {
            prop_assert_eq!(mm3_to_m([x, y, z]), [mm_to_m(x), mm_to_m(y), mm_to_m(z)]);
            prop_assert_eq!(m3_to_mm([x, y, z]), [m_to_mm(x), m_to_mm(y), m_to_mm(z)]);
        }
    }
}
