//! Spec tests for `pin_core::fmath` portable `min`/`max` (issue #159, KP-065, CR-010).
//!
//! Covers D9 Determinism (SPEC section 0 and 2.4, `fmath`) and REQ-WEB-002 (SPEC section 4,
//! Phase 9 prerequisite): the four functions return, bit for bit, what std `min`/`max` return on
//! x86_64, and follow one portable reference rule on every target.
//!
//! The std comparison runs only under `cfg(target_arch = "x86_64")`; the portable reference rule
//! and the algebraic properties run on all targets.
//!
//! Portable rule (argument order of `a.min(b)`):
//! both NaN -> `a`; only `a` NaN -> `b`; only `b` NaN -> `a`;
//! min: `if a < b { a } else { b }`; max: `if a > b { a } else { b }` (ties return `b`).

use pin_core::fmath::{max_f32, max_f64, min_f32, min_f64};
use proptest::prelude::*;

#[allow(clippy::if_same_then_else)] // written as the documented rule on purpose
fn ref_min_f32(a: f32, b: f32) -> f32 {
    if a.is_nan() && b.is_nan() {
        a
    } else if a.is_nan() {
        b
    } else if b.is_nan() {
        a
    } else if a < b {
        a
    } else {
        b
    }
}
#[allow(clippy::if_same_then_else)] // written as the documented rule on purpose
fn ref_max_f32(a: f32, b: f32) -> f32 {
    if a.is_nan() && b.is_nan() {
        a
    } else if a.is_nan() {
        b
    } else if b.is_nan() {
        a
    } else if a > b {
        a
    } else {
        b
    }
}
#[allow(clippy::if_same_then_else)] // written as the documented rule on purpose
fn ref_min_f64(a: f64, b: f64) -> f64 {
    if a.is_nan() && b.is_nan() {
        a
    } else if a.is_nan() {
        b
    } else if b.is_nan() {
        a
    } else if a < b {
        a
    } else {
        b
    }
}
#[allow(clippy::if_same_then_else)] // written as the documented rule on purpose
fn ref_max_f64(a: f64, b: f64) -> f64 {
    if a.is_nan() && b.is_nan() {
        a
    } else if a.is_nan() {
        b
    } else if b.is_nan() {
        a
    } else if a > b {
        a
    } else {
        b
    }
}

const SPECIAL_F32_BITS: [u32; 17] = [
    0x0000_0000, // +0
    0x8000_0000, // -0
    0x7f80_0000, // +inf
    0xff80_0000, // -inf
    0x0000_0001, // smallest positive subnormal
    0x007f_ffff, // largest positive subnormal
    0x8000_0001, // smallest negative subnormal
    0x807f_ffff, // largest negative subnormal
    0x0080_0000, // MIN_POSITIVE
    0x7f7f_ffff, // MAX
    0xff7f_ffff, // MIN
    0x3f80_0000, // 1
    0xbf80_0000, // -1
    0x7fc0_0000, // quiet NaN
    0xffc0_0000, // negative NaN
    0x7fc1_2345, // NaN with payload
    0x7fa0_0001, // signalling NaN
];

const SPECIAL_F64_BITS: [u64; 17] = [
    0x0000_0000_0000_0000,
    0x8000_0000_0000_0000,
    0x7ff0_0000_0000_0000,
    0xfff0_0000_0000_0000,
    0x0000_0000_0000_0001,
    0x000f_ffff_ffff_ffff,
    0x8000_0000_0000_0001,
    0x800f_ffff_ffff_ffff,
    0x0010_0000_0000_0000,
    0x7fef_ffff_ffff_ffff,
    0xffef_ffff_ffff_ffff,
    0x3ff0_0000_0000_0000,
    0xbff0_0000_0000_0000,
    0x7ff8_0000_0000_0000,
    0xfff8_0000_0000_0000,
    0x7ff8_0000_1234_5678,
    0x7ff4_0000_0000_0001,
];

fn special_f32() -> impl Strategy<Value = f32> {
    proptest::sample::select(SPECIAL_F32_BITS.to_vec()).prop_map(f32::from_bits)
}
fn special_f64() -> impl Strategy<Value = f64> {
    proptest::sample::select(SPECIAL_F64_BITS.to_vec()).prop_map(f64::from_bits)
}
/// Arbitrary bit patterns mixed with the special values (prop_oneof).
fn any_f32() -> impl Strategy<Value = f32> {
    prop_oneof![any::<u32>().prop_map(f32::from_bits), special_f32()]
}
fn any_f64() -> impl Strategy<Value = f64> {
    prop_oneof![any::<u64>().prop_map(f64::from_bits), special_f64()]
}

fn non_nan_f32() -> impl Strategy<Value = f32> {
    any_f32().prop_map(|v| if v.is_nan() { 0.5 } else { v })
}
fn non_nan_f64() -> impl Strategy<Value = f64> {
    any_f64().prop_map(|v| if v.is_nan() { 0.5 } else { v })
}

fn all_f32() -> Vec<f32> {
    SPECIAL_F32_BITS
        .iter()
        .map(|&b| f32::from_bits(b))
        .collect()
}
fn all_f64() -> Vec<f64> {
    SPECIAL_F64_BITS
        .iter()
        .map(|&b| f64::from_bits(b))
        .collect()
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(8192))]

    /// D9, REQ-WEB-002: bit-equal to std on x86_64 (arbitrary bits and special values), f32.
    #[test]
    fn d9_req_web_002_f32_matches_std_x86_64(a in any_f32(), b in any_f32()) {
        #[cfg(target_arch = "x86_64")]
        {
            prop_assert_eq!(min_f32(a, b).to_bits(), a.min(b).to_bits());
            prop_assert_eq!(max_f32(a, b).to_bits(), a.max(b).to_bits());
        }
        // all targets: portable rule
        prop_assert_eq!(min_f32(a, b).to_bits(), ref_min_f32(a, b).to_bits());
        prop_assert_eq!(max_f32(a, b).to_bits(), ref_max_f32(a, b).to_bits());
    }

    /// D9, REQ-WEB-002: bit-equal to std on x86_64, f64.
    #[test]
    fn d9_req_web_002_f64_matches_std_x86_64(a in any_f64(), b in any_f64()) {
        #[cfg(target_arch = "x86_64")]
        {
            prop_assert_eq!(min_f64(a, b).to_bits(), a.min(b).to_bits());
            prop_assert_eq!(max_f64(a, b).to_bits(), a.max(b).to_bits());
        }
        prop_assert_eq!(min_f64(a, b).to_bits(), ref_min_f64(a, b).to_bits());
        prop_assert_eq!(max_f64(a, b).to_bits(), ref_max_f64(a, b).to_bits());
    }

    /// D9, REQ-WEB-002: purely random bit patterns only (NaN payloads, subnormals), f32.
    #[test]
    fn d9_req_web_002_f32_arbitrary_bits(x in any::<u32>(), y in any::<u32>()) {
        let (a, b) = (f32::from_bits(x), f32::from_bits(y));
        #[cfg(target_arch = "x86_64")]
        {
            prop_assert_eq!(min_f32(a, b).to_bits(), a.min(b).to_bits());
            prop_assert_eq!(max_f32(a, b).to_bits(), a.max(b).to_bits());
        }
        prop_assert_eq!(min_f32(a, b).to_bits(), ref_min_f32(a, b).to_bits());
        prop_assert_eq!(max_f32(a, b).to_bits(), ref_max_f32(a, b).to_bits());
    }

    /// D9, REQ-WEB-002: purely random bit patterns only, f64.
    #[test]
    fn d9_req_web_002_f64_arbitrary_bits(x in any::<u64>(), y in any::<u64>()) {
        let (a, b) = (f64::from_bits(x), f64::from_bits(y));
        #[cfg(target_arch = "x86_64")]
        {
            prop_assert_eq!(min_f64(a, b).to_bits(), a.min(b).to_bits());
            prop_assert_eq!(max_f64(a, b).to_bits(), a.max(b).to_bits());
        }
        prop_assert_eq!(min_f64(a, b).to_bits(), ref_min_f64(a, b).to_bits());
        prop_assert_eq!(max_f64(a, b).to_bits(), ref_max_f64(a, b).to_bits());
    }

    /// D9, issue #159 algebra (all targets), f32.
    #[test]
    fn d9_algebra_f32(a in non_nan_f32(), b in non_nan_f32()) {
        let (lo, hi) = (min_f32(a, b), max_f32(a, b));
        prop_assert!(lo.to_bits() == a.to_bits() || lo.to_bits() == b.to_bits());
        prop_assert!(hi.to_bits() == a.to_bits() || hi.to_bits() == b.to_bits());
        prop_assert!(lo <= hi);
        if a == b {
            prop_assert!(lo == hi);
        }
        prop_assert!(min_f32(a, b) == min_f32(b, a));
        prop_assert!(max_f32(a, b) == max_f32(b, a));
    }

    /// D9, issue #159 algebra (all targets), f64.
    #[test]
    fn d9_algebra_f64(a in non_nan_f64(), b in non_nan_f64()) {
        let (lo, hi) = (min_f64(a, b), max_f64(a, b));
        prop_assert!(lo.to_bits() == a.to_bits() || lo.to_bits() == b.to_bits());
        prop_assert!(hi.to_bits() == a.to_bits() || hi.to_bits() == b.to_bits());
        prop_assert!(lo <= hi);
        if a == b {
            prop_assert!(lo == hi);
        }
        prop_assert!(min_f64(a, b) == min_f64(b, a));
        prop_assert!(max_f64(a, b) == max_f64(b, a));
    }
}

/// D9, REQ-WEB-002: exhaustive ordered-pair table over the special values, all four functions.
#[test]
fn d9_req_web_002_exhaustive_special_table() {
    let mut pairs = 0usize;
    for &a in &all_f32() {
        for &b in &all_f32() {
            pairs += 1;
            let ctx = format!("a={:#010x} b={:#010x}", a.to_bits(), b.to_bits());
            #[cfg(target_arch = "x86_64")]
            {
                assert_eq!(min_f32(a, b).to_bits(), a.min(b).to_bits(), "std min {ctx}");
                assert_eq!(max_f32(a, b).to_bits(), a.max(b).to_bits(), "std max {ctx}");
            }
            assert_eq!(
                min_f32(a, b).to_bits(),
                ref_min_f32(a, b).to_bits(),
                "ref min {ctx}"
            );
            assert_eq!(
                max_f32(a, b).to_bits(),
                ref_max_f32(a, b).to_bits(),
                "ref max {ctx}"
            );
        }
    }
    for &a in &all_f64() {
        for &b in &all_f64() {
            pairs += 1;
            let ctx = format!("a={:#018x} b={:#018x}", a.to_bits(), b.to_bits());
            #[cfg(target_arch = "x86_64")]
            {
                assert_eq!(min_f64(a, b).to_bits(), a.min(b).to_bits(), "std min {ctx}");
                assert_eq!(max_f64(a, b).to_bits(), a.max(b).to_bits(), "std max {ctx}");
            }
            assert_eq!(
                min_f64(a, b).to_bits(),
                ref_min_f64(a, b).to_bits(),
                "ref min {ctx}"
            );
            assert_eq!(
                max_f64(a, b).to_bits(),
                ref_max_f64(a, b).to_bits(),
                "ref max {ctx}"
            );
        }
    }
    assert_eq!(pairs, 2 * 17 * 17);
}

/// D9: the portable reference rule itself equals std on x86_64 for the special table.
#[cfg(target_arch = "x86_64")]
#[test]
fn d9_reference_rule_matches_std_x86_64() {
    for &a in &all_f32() {
        for &b in &all_f32() {
            assert_eq!(ref_min_f32(a, b).to_bits(), a.min(b).to_bits());
            assert_eq!(ref_max_f32(a, b).to_bits(), a.max(b).to_bits());
        }
    }
    for &a in &all_f64() {
        for &b in &all_f64() {
            assert_eq!(ref_min_f64(a, b).to_bits(), a.min(b).to_bits());
            assert_eq!(ref_max_f64(a, b).to_bits(), a.max(b).to_bits());
        }
    }
}

/// D9, issue #159: signed-zero ties, second operand wins (f32). Also pins commutativity as
/// values only: bits differ for the +0/-0 ties.
#[test]
fn d9_signed_zero_ties_second_operand_wins_f32() {
    assert_eq!(min_f32(0.0, -0.0).to_bits(), (-0.0f32).to_bits());
    assert_eq!(min_f32(-0.0, 0.0).to_bits(), 0.0f32.to_bits());
    assert_eq!(max_f32(0.0, -0.0).to_bits(), (-0.0f32).to_bits());
    assert_eq!(max_f32(-0.0, 0.0).to_bits(), 0.0f32.to_bits());
    assert_ne!(min_f32(0.0, -0.0).to_bits(), min_f32(-0.0, 0.0).to_bits());
    assert_ne!(max_f32(0.0, -0.0).to_bits(), max_f32(-0.0, 0.0).to_bits());
    assert!(min_f32(0.0, -0.0) == min_f32(-0.0, 0.0));
    assert!(max_f32(0.0, -0.0) == max_f32(-0.0, 0.0));
}

/// D9, issue #159: signed-zero ties, second operand wins (f64).
#[test]
fn d9_signed_zero_ties_second_operand_wins_f64() {
    assert_eq!(min_f64(0.0, -0.0).to_bits(), (-0.0f64).to_bits());
    assert_eq!(min_f64(-0.0, 0.0).to_bits(), 0.0f64.to_bits());
    assert_eq!(max_f64(0.0, -0.0).to_bits(), (-0.0f64).to_bits());
    assert_eq!(max_f64(-0.0, 0.0).to_bits(), 0.0f64.to_bits());
    assert_ne!(min_f64(0.0, -0.0).to_bits(), min_f64(-0.0, 0.0).to_bits());
    assert_ne!(max_f64(0.0, -0.0).to_bits(), max_f64(-0.0, 0.0).to_bits());
    assert!(min_f64(0.0, -0.0) == min_f64(-0.0, 0.0));
    assert!(max_f64(0.0, -0.0) == max_f64(-0.0, 0.0));
}

/// D9, issue #159: a NaN operand yields the other operand (f32 and f64), both orders.
#[test]
fn d9_single_nan_yields_other_operand() {
    for nan in [0x7fc0_0000u32, 0xffc0_0000, 0x7fc1_2345, 0x7fa0_0001].map(f32::from_bits) {
        assert_eq!(min_f32(nan, 1.0).to_bits(), 1.0f32.to_bits());
        assert_eq!(min_f32(1.0, nan).to_bits(), 1.0f32.to_bits());
        assert_eq!(max_f32(nan, 1.0).to_bits(), 1.0f32.to_bits());
        assert_eq!(max_f32(1.0, nan).to_bits(), 1.0f32.to_bits());
    }
    for nan in [
        0x7ff8_0000_0000_0000u64,
        0xfff8_0000_0000_0000,
        0x7ff8_0000_1234_5678,
    ]
    .map(f64::from_bits)
    {
        assert_eq!(min_f64(nan, 1.0).to_bits(), 1.0f64.to_bits());
        assert_eq!(min_f64(1.0, nan).to_bits(), 1.0f64.to_bits());
        assert_eq!(max_f64(nan, 1.0).to_bits(), 1.0f64.to_bits());
        assert_eq!(max_f64(1.0, nan).to_bits(), 1.0f64.to_bits());
    }
}

/// D9, issue #159: both NaN returns the first operand's bits (measured x86_64 behaviour).
#[test]
fn d9_both_nan_returns_first_operand_bits() {
    let n32 = [0x7fc0_0000u32, 0xffc0_0000, 0x7fc1_2345, 0x7fa0_0001];
    for &x in &n32 {
        for &y in &n32 {
            let (a, b) = (f32::from_bits(x), f32::from_bits(y));
            assert_eq!(min_f32(a, b).to_bits(), x);
            assert_eq!(max_f32(a, b).to_bits(), x);
        }
    }
    let n64 = [
        0x7ff8_0000_0000_0000u64,
        0xfff8_0000_0000_0000,
        0x7ff8_0000_1234_5678,
        0x7ff4_0000_0000_0001,
    ];
    for &x in &n64 {
        for &y in &n64 {
            let (a, b) = (f64::from_bits(x), f64::from_bits(y));
            assert_eq!(min_f64(a, b).to_bits(), x);
            assert_eq!(max_f64(a, b).to_bits(), x);
        }
    }
}

/// D9, issue #159: infinities and subnormal ordering.
#[test]
fn d9_infinities_and_subnormals() {
    for x in [
        0.0f32,
        -0.0,
        1.0,
        -1.0,
        f32::MAX,
        f32::MIN,
        f32::from_bits(1),
    ] {
        assert_eq!(
            min_f32(f32::NEG_INFINITY, x).to_bits(),
            f32::NEG_INFINITY.to_bits()
        );
        assert_eq!(
            min_f32(x, f32::NEG_INFINITY).to_bits(),
            f32::NEG_INFINITY.to_bits()
        );
        assert_eq!(max_f32(f32::INFINITY, x).to_bits(), f32::INFINITY.to_bits());
        assert_eq!(max_f32(x, f32::INFINITY).to_bits(), f32::INFINITY.to_bits());
    }
    for x in [
        0.0f64,
        -0.0,
        1.0,
        -1.0,
        f64::MAX,
        f64::MIN,
        f64::from_bits(1),
    ] {
        assert_eq!(
            min_f64(f64::NEG_INFINITY, x).to_bits(),
            f64::NEG_INFINITY.to_bits()
        );
        assert_eq!(
            min_f64(x, f64::NEG_INFINITY).to_bits(),
            f64::NEG_INFINITY.to_bits()
        );
        assert_eq!(max_f64(f64::INFINITY, x).to_bits(), f64::INFINITY.to_bits());
        assert_eq!(max_f64(x, f64::INFINITY).to_bits(), f64::INFINITY.to_bits());
    }
    let (s1, smax, n1) = (
        f32::from_bits(1),
        f32::from_bits(0x007f_ffff),
        f32::from_bits(0x8000_0001),
    );
    assert_eq!(min_f32(s1, smax).to_bits(), s1.to_bits());
    assert_eq!(max_f32(s1, smax).to_bits(), smax.to_bits());
    assert_eq!(min_f32(n1, 0.0).to_bits(), n1.to_bits());
    assert_eq!(max_f32(n1, 0.0).to_bits(), 0.0f32.to_bits());
    assert_eq!(min_f32(s1, f32::MIN_POSITIVE).to_bits(), s1.to_bits());
    let (d1, dmax, dn1) = (
        f64::from_bits(1),
        f64::from_bits(0x000f_ffff_ffff_ffff),
        f64::from_bits(0x8000_0000_0000_0001),
    );
    assert_eq!(min_f64(d1, dmax).to_bits(), d1.to_bits());
    assert_eq!(max_f64(d1, dmax).to_bits(), dmax.to_bits());
    assert_eq!(min_f64(dn1, 0.0).to_bits(), dn1.to_bits());
    assert_eq!(max_f64(dn1, 0.0).to_bits(), 0.0f64.to_bits());
    assert_eq!(min_f64(d1, f64::MIN_POSITIVE).to_bits(), d1.to_bits());
}
