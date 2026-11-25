//! Independent spec tests for KP-066 `pin_physics::quat_ops` (SPEC §0 D9, SPEC §2.4 "To stay
//! equal"; REQ-WEB-002 prerequisite). `quat_ops` must equal glam 0.33.12's SSE2 `Quat`
//! arithmetic bit for bit on every target, so the x86_64 goldens hold on aarch64 and wasm32.

use glam::{Quat, Vec3};
use pin_physics::ball::contact::integrate_orientation;
use pin_physics::quat_ops;
use proptest::prelude::*;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fnv(h: &mut u64, bits: u32) {
    for b in bits.to_le_bytes() {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(FNV_PRIME);
    }
}

/// PCG32 (XSH-RR) with explicit constants, independent of the sim's PCG.
struct Pcg(u64);
impl Pcg {
    fn next_u32(&mut self) -> u32 {
        let old = self.0;
        self.0 = old
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let xs = (((old >> 18) ^ old) >> 27) as u32;
        xs.rotate_right((old >> 59) as u32)
    }
    /// Uniform in [0,1) from 24 bits.
    fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / 16_777_216.0
    }
    /// Component: exact +-0 (1/16), else +-10^e with e in [-3,3] (finite, subnormal-free).
    fn comp(&mut self) -> f32 {
        let k = self.next_u32();
        match k & 15 {
            0 => 0.0,
            1 => -0.0,
            _ => {
                let e = -3.0 + 6.0 * self.unit();
                let m = libm::powf(10.0, e);
                if k & 16 != 0 { -m } else { m }
            }
        }
    }
    fn quat(&mut self) -> Quat {
        Quat::from_xyzw(self.comp(), self.comp(), self.comp(), self.comp())
    }
    fn vec(&mut self) -> Vec3 {
        Vec3::new(self.comp(), self.comp(), self.comp())
    }
    fn unit_vec(&mut self) -> Vec3 {
        loop {
            let v = Vec3::new(self.comp(), self.comp(), self.comp());
            let l = libm::sqrtf(v.x * v.x + v.y * v.y + v.z * v.z);
            if l > 1e-3 {
                return Vec3::new(v.x / l, v.y / l, v.z / l);
            }
        }
    }
}

fn bits4(q: Quat) -> [u32; 4] {
    [q.x.to_bits(), q.y.to_bits(), q.z.to_bits(), q.w.to_bits()]
}
fn bits3(v: Vec3) -> [u32; 3] {
    [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()]
}

// ---------------------------------------------------------------- strategies (own)
fn comp_strat() -> impl Strategy<Value = f32> {
    prop_oneof![
        1 => Just(0.0f32),
        1 => Just(-0.0f32),
        14 => (-3.0f32..3.0, any::<bool>()).prop_map(|(e, neg)| {
            let m = libm::powf(10.0, e);
            if neg { -m } else { m }
        }),
    ]
}
fn quat_strat() -> impl Strategy<Value = Quat> {
    prop_oneof![
        3 => (comp_strat(), comp_strat(), comp_strat(), comp_strat())
            .prop_map(|(x, y, z, w)| Quat::from_xyzw(x, y, z, w)),
        // unit quaternions via own normalisation
        3 => (comp_strat(), comp_strat(), comp_strat(), comp_strat()).prop_map(|(x, y, z, w)| {
            let l = libm::sqrtf(x * x + y * y + z * z + w * w);
            if l < 1e-3 { Quat::from_xyzw(0.0, 0.0, 0.0, 1.0) }
            else { Quat::from_xyzw(x / l, y / l, z / l, w / l) }
        }),
    ]
}
fn vec_strat() -> impl Strategy<Value = Vec3> {
    (comp_strat(), comp_strat(), comp_strat()).prop_map(|(x, y, z)| Vec3::new(x, y, z))
}
#[cfg(any(target_feature = "sse2", target_feature = "simd128"))]
fn unit_vec_strat() -> impl Strategy<Value = Vec3> {
    (comp_strat(), comp_strat(), comp_strat()).prop_map(|(x, y, z)| {
        let l = libm::sqrtf(x * x + y * y + z * z);
        if l < 1e-3 {
            Vec3::Z
        } else {
            Vec3::new(x / l, y / l, z / l)
        }
    })
}

// SPEC §2.4 / D9: `quat_ops` equals glam SSE2 (or simd128) `Quat` arithmetic bit for bit.
#[cfg(any(target_feature = "sse2", target_feature = "simd128"))]
proptest! {
    #![proptest_config(ProptestConfig { cases: 25_000, ..ProptestConfig::default() })]
    #[test]
    fn kp066_matches_glam_sse2_bit_for_bit(
        a in quat_strat(), b in quat_strat(), v in vec_strat(),
        f in unit_vec_strat(), t in unit_vec_strat(),
    ) {
        prop_assert_eq!(bits4(quat_ops::mul(a, b)), bits4(a * b));
        prop_assert_eq!(bits3(quat_ops::mul_vec3(a, v)), bits3(a * v));
        prop_assert_eq!(
            bits3(quat_ops::mul_vec3(a.conjugate(), v)),
            bits3(a.conjugate() * v)
        );
        prop_assert_eq!(quat_ops::length_squared(a).to_bits(), a.length_squared().to_bits());
        if a.length_squared() > 0.0 {
            prop_assert_eq!(bits4(quat_ops::normalize(a)), bits4(a.normalize()));
        }
        prop_assert_eq!(
            bits4(quat_ops::from_rotation_arc(f, t)),
            bits4(Quat::from_rotation_arc(f, t))
        );
        // antiparallel branch
        prop_assert_eq!(
            bits4(quat_ops::from_rotation_arc(f, -f)),
            bits4(Quat::from_rotation_arc(f, -f))
        );
    }
}

/// SPEC §2.4 / D9: hand-computed known values.
#[test]
fn kp066_known_values() {
    let v = Vec3::new(0.3, -0.7, 1.9);
    let q = Quat::from_xyzw(0.1, 0.2, 0.3, 0.9).normalize();
    assert_eq!(bits4(quat_ops::mul(Quat::IDENTITY, q)), bits4(q));
    assert_eq!(bits4(quat_ops::mul(q, Quat::IDENTITY)), bits4(q));
    assert_eq!(bits3(quat_ops::mul_vec3(Quat::IDENTITY, v)), bits3(v));

    let h = core::f32::consts::FRAC_PI_4;
    let rz = Quat::from_xyzw(0.0, 0.0, libm::sinf(h), libm::cosf(h));
    let y = quat_ops::mul_vec3(rz, Vec3::X);
    assert!((y - Vec3::Y).length() < 1e-6, "{y:?}");

    assert_eq!(
        bits4(quat_ops::normalize(Quat::from_xyzw(0.0, 0.0, 0.0, 2.0))),
        bits4(Quat::IDENTITY)
    );
    assert_eq!(
        quat_ops::length_squared(Quat::from_xyzw(1.0, 2.0, 3.0, 4.0)),
        30.0
    );

    assert_eq!(
        bits4(quat_ops::from_rotation_arc(Vec3::Z, Vec3::Z)),
        bits4(Quat::IDENTITY)
    );
    let r = quat_ops::from_rotation_arc(Vec3::Z, Vec3::X);
    assert!((quat_ops::mul_vec3(r, Vec3::Z) - Vec3::X).length() < 1e-6);
    let r = quat_ops::from_rotation_arc(Vec3::Z, -Vec3::Z);
    assert!((quat_ops::length_squared(r) - 1.0).abs() < 1e-6);
    assert!((quat_ops::mul_vec3(r, Vec3::Z) + Vec3::Z).length() < 1e-6);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 5_000, ..ProptestConfig::default() })]
    /// SPEC §2.4 / D9: algebraic sanity of the replica.
    /// (No allocation check: the crate's allocation guard lives in test files, not a public item.)
    #[test]
    fn kp066_algebraic_properties(
        a in quat_strat(), b in quat_strat(), v in vec_strat(),
    ) {
        prop_assume!(quat_ops::length_squared(a) > 1e-6 && quat_ops::length_squared(b) > 1e-6);
        let ua = quat_ops::normalize(a);
        let ub = quat_ops::normalize(b);
        let l2 = quat_ops::length_squared(ua);
        prop_assert!((l2 - 1.0).abs() < 1e-6, "len2 {}", l2);
        let rv = quat_ops::mul_vec3(ua, v);
        let (lv, lr) = (v.length(), rv.length());
        prop_assert!((lv - lr).abs() <= 1e-4 * lv.max(1e-3), "{} vs {}", lv, lr);
        let lhs = quat_ops::mul_vec3(quat_ops::mul(ua, ub), v);
        let rhs = quat_ops::mul_vec3(ua, quat_ops::mul_vec3(ub, v));
        prop_assert!((lhs - rhs).length() <= 1e-4 * v.length().max(1.0), "{:?} vs {:?}", lhs, rhs);
    }
}

fn digest_quat_ops() -> u64 {
    let mut p = Pcg(0x1234_5678_9abc_def0);
    let mut h = FNV_OFFSET;
    for i in 0..120_000u32 {
        let a = p.quat();
        let b = p.quat();
        let v = p.vec();
        let (f, mut t) = (p.unit_vec(), p.unit_vec());
        if i % 16 == 0 {
            t = -f;
        } else if i % 16 == 1 {
            t = f;
        }
        for x in bits4(quat_ops::mul(a, b)) {
            fnv(&mut h, x);
        }
        for x in bits3(quat_ops::mul_vec3(a, v)) {
            fnv(&mut h, x);
        }
        for x in bits3(quat_ops::mul_vec3(a.conjugate(), v)) {
            fnv(&mut h, x);
        }
        fnv(&mut h, quat_ops::length_squared(a).to_bits());
        // All-zero input gives NaN, whose sign bit is a parity exception (SPEC §2.4): skip it.
        if a.length_squared() > 0.0 {
            for x in bits4(quat_ops::normalize(a)) {
                fnv(&mut h, x);
            }
        }
        for x in bits4(quat_ops::from_rotation_arc(f, t)) {
            fnv(&mut h, x);
        }
    }
    h
}

#[cfg(any(target_feature = "sse2", target_feature = "simd128"))]
fn digest_glam() -> u64 {
    let mut p = Pcg(0x1234_5678_9abc_def0);
    let mut h = FNV_OFFSET;
    for i in 0..120_000u32 {
        let a = p.quat();
        let b = p.quat();
        let v = p.vec();
        let (f, mut t) = (p.unit_vec(), p.unit_vec());
        if i % 16 == 0 {
            t = -f;
        } else if i % 16 == 1 {
            t = f;
        }
        for x in bits4(a * b) {
            fnv(&mut h, x);
        }
        for x in bits3(a * v) {
            fnv(&mut h, x);
        }
        for x in bits3(a.conjugate() * v) {
            fnv(&mut h, x);
        }
        fnv(&mut h, a.length_squared().to_bits());
        // All-zero input gives NaN, whose sign bit is a parity exception (SPEC §2.4): skip it.
        if a.length_squared() > 0.0 {
            for x in bits4(a.normalize()) {
                fnv(&mut h, x);
            }
        }
        for x in bits4(Quat::from_rotation_arc(f, t)) {
            fnv(&mut h, x);
        }
    }
    h
}

const PINNED_OPS: u64 = 0x9b0f_6f36_9363_9d92;
const PINNED_INTEGRATE: u64 = 0x075c_3215_c4f1_5be2;

/// SPEC §2.4 / D9 / REQ-WEB-002 prerequisite: known-answer digest equal on every target.
#[test]
fn kp066_cross_target_digest() {
    let d = digest_quat_ops();
    println!("kp066 ops digest {d:#018x}");
    #[cfg(any(target_feature = "sse2", target_feature = "simd128"))]
    assert_eq!(d, digest_glam(), "quat_ops differs from glam SSE2");
    assert_eq!(d, PINNED_OPS);
}

fn integrate_inputs() -> impl Iterator<Item = Vec3> {
    let mut p = Pcg(0x0fed_cba9_8765_4321);
    (0..20_000).map(move |i| {
        if i % 97 == 0 {
            return Vec3::ZERO;
        }
        Vec3::new(
            (p.unit() * 2.0 - 1.0) * 200.0 / 1.75,
            (p.unit() * 2.0 - 1.0) * 200.0 / 1.75,
            (p.unit() * 2.0 - 1.0) * 200.0 / 1.75,
        )
    })
}

fn digest_integrate() -> u64 {
    let mut q = Quat::IDENTITY;
    let mut h = FNV_OFFSET;
    for w in integrate_inputs() {
        q = integrate_orientation(q, w, 0.001);
        for x in bits4(q) {
            fnv(&mut h, x);
        }
    }
    h
}

#[cfg(any(target_feature = "sse2", target_feature = "simd128"))]
fn digest_integrate_glam() -> u64 {
    let mut q = Quat::IDENTITY;
    let mut h = FNV_OFFSET;
    let t = 0.001f32;
    for w in integrate_inputs() {
        let rate = w.length();
        if rate > 0.0 {
            let half = 0.5 * rate * t;
            let s = libm::sinf(half) / rate;
            let dq = Quat::from_xyzw(w.x * s, w.y * s, w.z * s, libm::cosf(half));
            q = (dq * q).normalize();
        }
        for x in bits4(q) {
            fnv(&mut h, x);
        }
    }
    h
}

/// SPEC §2.4 / D9 / REQ-WEB-002 prerequisite: chained `integrate_orientation` digest equal on
/// every target and equal to the glam-operator reference on SSE2.
#[test]
fn kp066_integrate_orientation_cross_target() {
    let d = digest_integrate();
    println!("kp066 integrate digest {d:#018x}");
    #[cfg(any(target_feature = "sse2", target_feature = "simd128"))]
    assert_eq!(
        d,
        digest_integrate_glam(),
        "integrate_orientation differs from glam reference"
    );
    assert_eq!(d, PINNED_INTEGRATE);
}
