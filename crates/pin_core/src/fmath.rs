//! Portable `min`/`max` for `f32` and `f64` with target-independent bits (D9, CR-010, REQ-WEB-002).
//!
//! Std `f32::min`/`max` lower to different instructions per ISA (`minss`/`maxss` on x86_64,
//! `fminnm`/`fmaxnm` on aarch64, `f32.min`/`f32.max` on wasm32), so the sign of a zero tie differs:
//! spike S7 measured `max(+0, -0)` as `+0` on aarch64 and `-0` on x86_64. These functions use only
//! comparisons, so every target gives the same bits, equal to std on x86_64. Sim crates must use
//! them instead of `min`/`max` (PROCESS R3). Arguments are `(self, other)` as in `a.min(b)`.
//!
//! Rules: a NaN operand yields the other operand; both NaN yields the first operand `a`, bits
//! unchanged (no quieting or canonicalising; hashing canonicalises NaNs elsewhere); a tie,
//! including +0 against -0 in either order, yields the second operand `b`.
//!
//! Measured std results on x86_64 (rustc 1.97.0, debug, release and `target-cpu=native` agree),
//! f32 bits (f64 is analogous):
//!
//! | call | min | max |
//! |---|---|---|
//! | (+0, -0) | `80000000` | `80000000` |
//! | (-0, +0) | `00000000` | `00000000` |
//! | (NaN, 1.0) or (1.0, NaN), any NaN payload/sign | `3f800000` | `3f800000` |
//! | (NaN_a, NaN_b), e.g. `7fa00001`, `ffc00000`, `7fc00005` | bits of `NaN_a` | bits of `NaN_a` |
//!
//! The brief assumed both-NaN returns the second operand; the measurement shows the first, so the
//! measurement wins.

/// Minimum of `a` and `b`, bit-identical on every target (D9, CR-010).
///
/// Equals `a.min(b)` of std on x86_64. Rules, using plain comparisons only:
/// a NaN operand yields the other operand; if both are NaN the FIRST operand `a` is returned with
/// its bits unchanged (measured on x86_64, see the module docs); on a tie (including +0 against
/// -0, in either order) the SECOND operand `b` is returned. Sim crates must use this instead of
/// `min` (PROCESS R3).
#[inline]
pub fn min_f32(a: f32, b: f32) -> f32 {
    if b.is_nan() {
        a
    } else if a.is_nan() {
        b
    } else if a < b {
        a
    } else {
        b
    }
}

/// Maximum of `a` and `b`, bit-identical on every target (D9, CR-010).
///
/// Equals `a.max(b)` of std on x86_64. Rules, using plain comparisons only:
/// a NaN operand yields the other operand; if both are NaN the FIRST operand `a` is returned with
/// its bits unchanged (measured on x86_64, see the module docs); on a tie (including +0 against
/// -0, in either order) the SECOND operand `b` is returned. Sim crates must use this instead of
/// `max` (PROCESS R3).
#[inline]
pub fn max_f32(a: f32, b: f32) -> f32 {
    if b.is_nan() {
        a
    } else if a.is_nan() {
        b
    } else if a > b {
        a
    } else {
        b
    }
}

/// Minimum of `a` and `b`, bit-identical on every target (D9, CR-010).
///
/// Equals `a.min(b)` of std on x86_64. Rules, using plain comparisons only:
/// a NaN operand yields the other operand; if both are NaN the FIRST operand `a` is returned with
/// its bits unchanged (measured on x86_64, see the module docs); on a tie (including +0 against
/// -0, in either order) the SECOND operand `b` is returned. Sim crates must use this instead of
/// `min` (PROCESS R3).
#[inline]
pub fn min_f64(a: f64, b: f64) -> f64 {
    if b.is_nan() {
        a
    } else if a.is_nan() {
        b
    } else if a < b {
        a
    } else {
        b
    }
}

/// Maximum of `a` and `b`, bit-identical on every target (D9, CR-010).
///
/// Equals `a.max(b)` of std on x86_64. Rules, using plain comparisons only:
/// a NaN operand yields the other operand; if both are NaN the FIRST operand `a` is returned with
/// its bits unchanged (measured on x86_64, see the module docs); on a tie (including +0 against
/// -0, in either order) the SECOND operand `b` is returned. Sim crates must use this instead of
/// `max` (PROCESS R3).
#[inline]
pub fn max_f64(a: f64, b: f64) -> f64 {
    if b.is_nan() {
        a
    } else if a.is_nan() {
        b
    } else if a > b {
        a
    } else {
        b
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    const F32_NANS: [u32; 5] = [
        0x7fc0_0000,
        0x7fa0_0001,
        0xffc0_0000,
        0x7fc0_0005,
        0xffa0_0002,
    ];
    const F64_NANS: [u64; 4] = [
        0x7ff8_0000_0000_0000,
        0x7ff4_0000_0000_0001,
        0xfff8_0000_0000_0000,
        0x7ff8_0000_0000_0005,
    ];

    fn f32_specials() -> Vec<f32> {
        let mut v = vec![
            0.0,
            -0.0,
            1.0,
            -1.0,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::MAX,
            f32::MIN,
            f32::MIN_POSITIVE,
            f32::from_bits(1),
            f32::from_bits(0x8000_0001),
            f32::from_bits(0x007f_ffff),
            f32::from_bits(0x807f_ffff),
        ];
        v.extend(F32_NANS.iter().map(|&b| f32::from_bits(b)));
        v
    }

    fn f64_specials() -> Vec<f64> {
        let mut v = vec![
            0.0,
            -0.0,
            1.0,
            -1.0,
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::MAX,
            f64::MIN,
            f64::MIN_POSITIVE,
            f64::from_bits(1),
            f64::from_bits(0x8000_0000_0000_0001),
            f64::from_bits(0x000f_ffff_ffff_ffff),
            f64::from_bits(0x800f_ffff_ffff_ffff),
        ];
        v.extend(F64_NANS.iter().map(|&b| f64::from_bits(b)));
        v
    }

    // Reference written with comparisons only, structured differently from the implementation.
    fn ref_min32(a: f32, b: f32) -> f32 {
        if a.is_nan() && b.is_nan() {
            return a;
        }
        if a.is_nan() {
            return b;
        }
        if b.is_nan() {
            return a;
        }
        if a < b { a } else { b }
    }
    fn ref_max32(a: f32, b: f32) -> f32 {
        if a.is_nan() && b.is_nan() {
            return a;
        }
        if a.is_nan() {
            return b;
        }
        if b.is_nan() {
            return a;
        }
        if a > b { a } else { b }
    }
    fn ref_min64(a: f64, b: f64) -> f64 {
        if a.is_nan() {
            return if b.is_nan() { a } else { b };
        }
        if b.is_nan() || a < b { a } else { b }
    }
    fn ref_max64(a: f64, b: f64) -> f64 {
        if a.is_nan() {
            return if b.is_nan() { a } else { b };
        }
        if b.is_nan() || a > b { a } else { b }
    }

    /// D9 / CR-010: zero ties return the second operand, bit-exact, for all four functions.
    #[test]
    fn zero_ties_return_second_operand() {
        assert_eq!(min_f32(0.0, -0.0).to_bits(), 0x8000_0000);
        assert_eq!(min_f32(-0.0, 0.0).to_bits(), 0);
        assert_eq!(max_f32(0.0, -0.0).to_bits(), 0x8000_0000);
        assert_eq!(max_f32(-0.0, 0.0).to_bits(), 0);
        assert_eq!(min_f64(0.0, -0.0).to_bits(), 1 << 63);
        assert_eq!(min_f64(-0.0, 0.0).to_bits(), 0);
        assert_eq!(max_f64(0.0, -0.0).to_bits(), 1 << 63);
        assert_eq!(max_f64(-0.0, 0.0).to_bits(), 0);
    }

    /// D9 / CR-010: infinities, extremes, subnormals and equal values.
    #[test]
    fn ordinary_and_extreme_values() {
        assert_eq!(min_f32(f32::NEG_INFINITY, f32::MIN), f32::NEG_INFINITY);
        assert_eq!(max_f32(f32::INFINITY, f32::MAX), f32::INFINITY);
        assert_eq!(min_f32(f32::MAX, f32::MIN), f32::MIN);
        assert_eq!(max_f32(f32::MAX, f32::MIN), f32::MAX);
        let (p, n, big) = (
            f32::from_bits(1),
            f32::from_bits(0x8000_0001),
            f32::from_bits(0x007f_ffff),
        );
        assert_eq!(min_f32(p, n).to_bits(), n.to_bits());
        assert_eq!(max_f32(n, p).to_bits(), p.to_bits());
        assert_eq!(max_f32(big, p).to_bits(), big.to_bits());
        assert_eq!(min_f32(2.5, 2.5).to_bits(), 2.5f32.to_bits());
        assert_eq!(min_f64(f64::NEG_INFINITY, 1.0), f64::NEG_INFINITY);
        assert_eq!(max_f64(f64::INFINITY, 1.0), f64::INFINITY);
        assert_eq!(min_f64(f64::MAX, f64::MIN), f64::MIN);
        assert_eq!(max_f64(f64::MAX, f64::MIN), f64::MAX);
        let (p, n) = (f64::from_bits(1), f64::from_bits(1 << 63 | 1));
        assert_eq!(min_f64(p, n).to_bits(), n.to_bits());
        assert_eq!(max_f64(n, p).to_bits(), p.to_bits());
        assert_eq!(max_f64(3.5, 3.5).to_bits(), 3.5f64.to_bits());
    }

    /// D9 / CR-010: a NaN operand yields the other; both NaN yields the first, bits unchanged.
    #[test]
    fn nan_handling() {
        for &nb in &F32_NANS {
            let n = f32::from_bits(nb);
            for x in [1.0f32, -0.0, f32::INFINITY] {
                assert_eq!(min_f32(n, x).to_bits(), x.to_bits());
                assert_eq!(min_f32(x, n).to_bits(), x.to_bits());
                assert_eq!(max_f32(n, x).to_bits(), x.to_bits());
                assert_eq!(max_f32(x, n).to_bits(), x.to_bits());
            }
            for &mb in &F32_NANS {
                let m = f32::from_bits(mb);
                assert_eq!(min_f32(n, m).to_bits(), nb);
                assert_eq!(max_f32(n, m).to_bits(), nb);
            }
        }
        for &nb in &F64_NANS {
            let n = f64::from_bits(nb);
            for x in [1.0f64, -0.0, f64::NEG_INFINITY] {
                assert_eq!(min_f64(n, x).to_bits(), x.to_bits());
                assert_eq!(min_f64(x, n).to_bits(), x.to_bits());
                assert_eq!(max_f64(n, x).to_bits(), x.to_bits());
                assert_eq!(max_f64(x, n).to_bits(), x.to_bits());
            }
            for &mb in &F64_NANS {
                let m = f64::from_bits(mb);
                assert_eq!(min_f64(n, m).to_bits(), nb);
                assert_eq!(max_f64(n, m).to_bits(), nb);
            }
        }
    }

    /// All special-value pairs match the comparison-only reference on every target.
    #[test]
    fn specials_match_reference() {
        for &a in &f32_specials() {
            for &b in &f32_specials() {
                assert_eq!(min_f32(a, b).to_bits(), ref_min32(a, b).to_bits());
                assert_eq!(max_f32(a, b).to_bits(), ref_max32(a, b).to_bits());
            }
        }
        for &a in &f64_specials() {
            for &b in &f64_specials() {
                assert_eq!(min_f64(a, b).to_bits(), ref_min64(a, b).to_bits());
                assert_eq!(max_f64(a, b).to_bits(), ref_max64(a, b).to_bits());
            }
        }
    }

    /// On x86_64 all special-value pairs equal std bit for bit (CR-010).
    #[cfg(target_arch = "x86_64")]
    #[allow(
        clippy::disallowed_methods,
        reason = "reference: compares fmath with std min/max on x86_64 (CR-010, KP-067)"
    )]
    #[test]
    fn specials_match_std_on_x86_64() {
        for &a in &f32_specials() {
            for &b in &f32_specials() {
                assert_eq!(min_f32(a, b).to_bits(), a.min(b).to_bits());
                assert_eq!(max_f32(a, b).to_bits(), a.max(b).to_bits());
            }
        }
        for &a in &f64_specials() {
            for &b in &f64_specials() {
                assert_eq!(min_f64(a, b).to_bits(), a.min(b).to_bits());
                assert_eq!(max_f64(a, b).to_bits(), a.max(b).to_bits());
            }
        }
    }

    proptest! {
        /// D9 / CR-010: arbitrary bit patterns match the comparison-only reference.
        #[test]
        fn f32_matches_reference(x in any::<u32>(), y in any::<u32>()) {
            let (a, b) = (f32::from_bits(x), f32::from_bits(y));
            prop_assert_eq!(min_f32(a, b).to_bits(), ref_min32(a, b).to_bits());
            prop_assert_eq!(max_f32(a, b).to_bits(), ref_max32(a, b).to_bits());
        }

        #[test]
        fn f64_matches_reference(x in any::<u64>(), y in any::<u64>()) {
            let (a, b) = (f64::from_bits(x), f64::from_bits(y));
            prop_assert_eq!(min_f64(a, b).to_bits(), ref_min64(a, b).to_bits());
            prop_assert_eq!(max_f64(a, b).to_bits(), ref_max64(a, b).to_bits());
        }

        /// On x86_64 arbitrary bit patterns equal std bit for bit.
        #[cfg(target_arch = "x86_64")]
        #[allow(
            clippy::disallowed_methods,
            reason = "reference: compares fmath with std min/max on x86_64 (CR-010, KP-067)"
        )]
        #[test]
        fn f32_matches_std(x in any::<u32>(), y in any::<u32>()) {
            let (a, b) = (f32::from_bits(x), f32::from_bits(y));
            prop_assert_eq!(min_f32(a, b).to_bits(), a.min(b).to_bits());
            prop_assert_eq!(max_f32(a, b).to_bits(), a.max(b).to_bits());
        }

        #[cfg(target_arch = "x86_64")]
        #[allow(
            clippy::disallowed_methods,
            reason = "reference: compares fmath with std min/max on x86_64 (CR-010, KP-067)"
        )]
        #[test]
        fn f64_matches_std(x in any::<u64>(), y in any::<u64>()) {
            let (a, b) = (f64::from_bits(x), f64::from_bits(y));
            prop_assert_eq!(min_f64(a, b).to_bits(), a.min(b).to_bits());
            prop_assert_eq!(max_f64(a, b).to_bits(), a.max(b).to_bits());
        }
    }
}
