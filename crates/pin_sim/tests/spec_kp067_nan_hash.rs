//! KP-067 black-box checks that NaNs hash canonically (REQ-WEB-002 / D9, SPEC §2.4) and every
//! non-NaN float still hashes as its raw little-endian bits.

use pin_sim::hash::{Fnv64, fnv1a64};
use proptest::prelude::*;
use std::hint::black_box;

const CANON32: u32 = 0x7fc0_0000;
const CANON64: u64 = 0x7ff8_0000_0000_0000;

/// Independent FNV-1a 64 reference.
fn reference(bytes: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    h
}

fn h32(v: f32) -> u64 {
    let mut h = Fnv64::new();
    h.write_f32(v);
    h.finish()
}
fn h64(v: f64) -> u64 {
    let mut h = Fnv64::new();
    h.write_f64(v);
    h.finish()
}
fn ref32(bits: u32) -> u64 {
    reference(&bits.to_le_bytes())
}
fn ref64(bits: u64) -> u64 {
    reference(&bits.to_le_bytes())
}

struct Xs(u64);
impl Xs {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

/// D9: published FNV vectors (sanity of the reference and `fnv1a64`).
#[test]
fn fnv_published_vectors() {
    for (s, v) in [
        (&b""[..], 0xcbf29ce484222325u64),
        (b"a", 0xaf63dc4c8601ec8c),
        (b"foobar", 0x85944171f73967e8),
    ] {
        assert_eq!(reference(s), v);
        assert_eq!(fnv1a64(s), v);
    }
}

/// REQ-WEB-002 / D9: every f32 NaN pattern hashes as the canonical NaN.
#[test]
fn f32_nan_patterns_canonical() {
    let want = ref32(CANON32);
    let zero = black_box(0.0f32);
    let mut cases = vec![
        f32::NAN,
        -f32::NAN,
        f32::from_bits(0x7fc00000),
        f32::from_bits(0xffc00000),
        f32::from_bits(0x7f800001),
        f32::from_bits(0xff800001),
        f32::from_bits(0x7fffffff),
        f32::from_bits(0xffffffff),
        f32::from_bits(0x7fa00000),
        zero / zero,
    ];
    let mut r = Xs(0x9e37_79b9_7f4a_7c15);
    for m in [1u32, 0x40_0000, 0x7f_ffff] {
        cases.push(f32::from_bits(0x7f80_0000 | m));
        cases.push(f32::from_bits(0xff80_0000 | m));
    }
    for _ in 0..100_000 {
        let r = r.next();
        let m = (r as u32 & 0x7f_ffff).max(1);
        let s = ((r >> 40) as u32 & 1) << 31;
        cases.push(f32::from_bits(s | 0x7f80_0000 | m));
    }
    for v in cases {
        assert!(v.is_nan());
        assert_eq!(h32(v), want, "bits {:#010x}", v.to_bits());
    }
}

/// REQ-WEB-002 / D9: every f64 NaN pattern hashes as the canonical NaN.
#[test]
fn f64_nan_patterns_canonical() {
    let want = ref64(CANON64);
    let zero = black_box(0.0f64);
    let mut cases = vec![
        f64::NAN,
        -f64::NAN,
        f64::from_bits(0x7ff8000000000000),
        f64::from_bits(0xfff8000000000000),
        f64::from_bits(0x7ff0000000000001),
        f64::from_bits(0x7fffffffffffffff),
        f64::from_bits(0xffffffffffffffff),
        zero / zero,
    ];
    let mut r = Xs(0x1234_5678_9abc_def1);
    for _ in 0..100_000 {
        let x = r.next();
        let m = (x & 0x000f_ffff_ffff_ffff).max(1);
        let s = (x >> 63) << 63;
        cases.push(f64::from_bits(s | 0x7ff0_0000_0000_0000 | m));
    }
    for v in cases {
        assert!(v.is_nan());
        assert_eq!(h64(v), want, "bits {:#018x}", v.to_bits());
    }
}

/// REQ-WEB-002 / D9: a float sequence differing only in NaN payloads hashes identically.
#[test]
fn sequences_differing_only_in_nan_payload_match() {
    let a = [
        1.5f32,
        f32::from_bits(0x7fc00000),
        -0.0,
        f32::from_bits(0xff800001),
    ];
    let b = [
        1.5f32,
        f32::from_bits(0x7fffffff),
        -0.0,
        f32::from_bits(0x7fa00000),
    ];
    let run = |s: &[f32]| {
        let mut h = Fnv64::new();
        for &v in s {
            h.write_f32(v);
        }
        h.finish()
    };
    assert_eq!(run(&a), run(&b));
    let a = [1.5f64, f64::from_bits(0x7ff0000000000001), -0.0, f64::NAN];
    let b = [1.5f64, -f64::NAN, -0.0, f64::from_bits(0xffffffffffffffff)];
    let run = |s: &[f64]| {
        let mut h = Fnv64::new();
        for &v in s {
            h.write_f64(v);
        }
        h.finish()
    };
    assert_eq!(run(&a), run(&b));
}

/// REQ-WEB-002 / D9: non-NaN values still hash as their raw bits.
#[test]
fn non_nan_unchanged() {
    let mut f32s = vec![
        0.0f32,
        -0.0,
        1.0,
        -1.0,
        f32::MIN_POSITIVE,
        f32::from_bits(1),
        f32::from_bits(0x007fffff),
        f32::MAX,
        f32::MIN,
        f32::INFINITY,
        f32::NEG_INFINITY,
    ];
    let mut r = Xs(0xdead_beef_cafe_f00d);
    while f32s.len() < 100_000 {
        let v = f32::from_bits(r.next() as u32);
        if !v.is_nan() {
            f32s.push(v);
        }
    }
    for v in f32s {
        assert_eq!(h32(v), ref32(v.to_bits()));
        let mut h = Fnv64::new();
        h.write(&v.to_bits().to_le_bytes());
        assert_eq!(h.finish(), h32(v));
    }
    let mut f64s = vec![
        0.0f64,
        -0.0,
        1.0,
        -1.0,
        f64::MIN_POSITIVE,
        f64::from_bits(1),
        f64::from_bits(0x000f_ffff_ffff_ffff),
        f64::MAX,
        f64::MIN,
        f64::INFINITY,
        f64::NEG_INFINITY,
    ];
    while f64s.len() < 100_000 {
        let v = f64::from_bits(r.next());
        if !v.is_nan() {
            f64s.push(v);
        }
    }
    for v in f64s {
        assert_eq!(h64(v), ref64(v.to_bits()));
    }
    assert_ne!(h32(0.0), h32(-0.0));
    assert_ne!(h64(0.0), h64(-0.0));
    assert_ne!(h32(f32::INFINITY), h32(f32::NEG_INFINITY));
    assert_ne!(h32(f32::INFINITY), h32(f32::NAN));
    assert_ne!(h32(f32::NEG_INFINITY), h32(f32::NAN));
    assert_ne!(h64(f64::INFINITY), h64(f64::NEG_INFINITY));
    assert_ne!(h64(f64::INFINITY), h64(f64::NAN));
    assert_ne!(h64(f64::NEG_INFINITY), h64(f64::NAN));
}

/// REQ-WEB-002 / D9: f32 and f64 NaN digests are the 4- and 8-byte canonical references.
#[test]
fn f32_f64_nan_widths() {
    assert_eq!(h32(f32::NAN), reference(&CANON32.to_le_bytes()));
    assert_eq!(h64(f64::NAN), reference(&CANON64.to_le_bytes()));
    assert_ne!(h32(f32::NAN), h64(f64::NAN));
}

/// REQ-WEB-002 / D9: two hashers fed the same mixed sequence agree.
#[test]
fn independent_hashers_agree() {
    let seq = [
        1.0f32,
        f32::from_bits(0xffc00001),
        2.5,
        f32::from_bits(0x7f800001),
        -7.0,
    ];
    let (mut a, mut b) = (Fnv64::new(), Fnv64::new());
    for &v in &seq {
        a.write_f32(v);
        b.write_f32(v);
    }
    assert_eq!(a.finish(), b.finish());
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 4096, rng_algorithm: proptest::test_runner::RngAlgorithm::ChaCha, ..ProptestConfig::default() })]

    /// REQ-WEB-002 / D9: for any f32 bits the digest is the reference of the canonicalised bits.
    #[test]
    fn prop_f32_canonicalised(b in any::<u32>()) {
        let v = f32::from_bits(b);
        let c = if v.is_nan() { CANON32 } else { b };
        prop_assert_eq!(h32(v), ref32(c));
    }

    /// REQ-WEB-002 / D9: for any f64 bits the digest is the reference of the canonicalised bits.
    #[test]
    fn prop_f64_canonicalised(b in any::<u64>()) {
        let v = f64::from_bits(b);
        let c = if v.is_nan() { CANON64 } else { b };
        prop_assert_eq!(h64(v), ref64(c));
    }
}
