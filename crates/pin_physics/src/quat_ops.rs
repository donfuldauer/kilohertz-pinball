//! Target-independent quaternion arithmetic for the simulation (SPEC §2.4
//! "To stay equal", D9).
//!
//! glam 0.33.12 implements `Quat` with SSE2 on x86_64, `simd128` on wasm32
//! (op for op the same as SSE2), NEON on aarch64 and plain scalar code
//! elsewhere. The NEON code sums the 4-lane dot product in a different order
//! (`vaddvq_f32`: `(x + y) + (z + w)` where SSE2 has `(x + z) + (y + w)`),
//! normalises by multiplying with the reciprocal length instead of dividing,
//! and fuses the cross product inside `Quat * Vec3` (`vfmsq_f32`), so the same
//! ball spin gives different orientation bits on aarch64 than on x86_64.
//!
//! The functions below are plain `f32` code that reproduces glam's SSE2
//! results bit for bit (same products, same additions in the same grouping,
//! no fused operation; rustc never contracts `a * b + c`). They give the same
//! bits on every IEEE-754 target, so x86_64 output is unchanged and aarch64
//! and wasm32 (with or without `simd128`) match it. Every `Quat` operation on
//! a simulation path of `pin_physics` goes through this module; glam `Vec3`
//! is scalar on every target and needs no replica.
//!
//! NaN payloads are not part of the guarantee (SPEC §2.4 parity exception:
//! NaN bits are canonicalised before hashing).

use glam::{Quat, Vec3};

/// `a * b` (Hamilton product), as glam's SSE2 `Quat::mul_quat`
/// (glam-0.33.12 `src/f32/sse2/quat.rs:885-919`, with the non-FMA
/// `m128_mul_add` of `src/sse2.rs:128-141`).
///
/// The sign-control multiplies of the SSE2 code are by `±1.0` and therefore
/// exact; what remains is the lane-wise sum order reproduced here.
/// NaN payloads are not part of the guarantee.
#[inline]
#[must_use]
pub fn mul(a: Quat, b: Quat) -> Quat {
    Quat::from_xyzw(
        (a.x * b.w + a.w * b.x) + (a.y * b.z - a.z * b.y),
        (a.w * b.y - a.x * b.z) + (a.z * b.x + a.y * b.w),
        (a.x * b.y + a.w * b.z) + (a.z * b.w - a.y * b.x),
        (a.w * b.w - a.x * b.x) + (-(a.z * b.z) - a.y * b.y),
    )
}

/// Squared length with SSE2's grouping `(x² + z²) + (y² + w²)`, as glam's
/// SSE2 `Quat::length_squared` (glam-0.33.12 `src/sse2.rs:75-80`,
/// `dot4_into_m128`).
///
/// NaN payloads are not part of the guarantee.
#[inline]
#[must_use]
pub fn length_squared(q: Quat) -> f32 {
    (q.x * q.x + q.z * q.z) + (q.y * q.y + q.w * q.w)
}

/// `q / |q|` component-wise, as glam's SSE2 `Quat::normalize`
/// (glam-0.33.12 `src/f32/sse2/vec4.rs:654-662`: `_mm_sqrt_ps` of
/// [`length_squared`], then a division by the length, not a multiply by its
/// reciprocal).
///
/// A zero-length input gives non-finite components, as in glam; the
/// simulation never passes one. NaN payloads are not part of the guarantee.
#[inline]
#[must_use]
pub fn normalize(q: Quat) -> Quat {
    let len = length_squared(q).sqrt();
    Quat::from_xyzw(q.x / len, q.y / len, q.z / len, q.w / len)
}

/// `q * v` (rotation of `v` by `q`), as glam's SSE2 `Quat::mul_vec3a`
/// (glam-0.33.12 `src/f32/sse2/quat.rs:964-980`, with the 3-lane dot of
/// `src/sse2.rs:40-48` and the cross product of
/// `src/f32/sse2/vec3a.rs:279-291` without FMA).
///
/// Computes `v (w² - |b|²) + b (2 v·b) + (b × v) 2w` with `b = (x, y, z)`;
/// valid for non-unit `q` as well (the result is then scaled by `|q|²`).
/// NaN payloads are not part of the guarantee.
#[inline]
#[must_use]
pub fn mul_vec3(q: Quat, v: Vec3) -> Vec3 {
    let (bx, by, bz, w) = (q.x, q.y, q.z, q.w);
    let b2 = (bx * bx + by * by) + bz * bz;
    let s1 = w * w - b2;
    let s2 = ((v.x * bx + v.y * by) + v.z * bz) * 2.0;
    let s3 = w * 2.0;
    let cx = by * v.z - v.y * bz;
    let cy = bz * v.x - v.z * bx;
    let cz = bx * v.y - v.x * by;
    Vec3::new(
        (v.x * s1 + bx * s2) + cx * s3,
        (v.y * s1 + by * s2) + cy * s3,
        (v.z * s1 + bz * s2) + cz * s3,
    )
}

/// The shortest rotation from unit `from` to unit `to`, as glam's
/// `Quat::from_rotation_arc` (glam-0.33.12 `src/f32/sse2/quat.rs:344-361`)
/// with [`normalize`] for the final step.
///
/// Near `from == to` the result is the identity; near `from == -to` it is a
/// half turn about `from.any_orthonormal_vector()` built from
/// `libm::sinf`/`libm::cosf` of the half angle (glam's `from_axis_angle`,
/// `src/f32/sse2/quat.rs:152-157`, would reach the platform libm). Both
/// inputs must be unit vectors. NaN payloads are not part of the guarantee.
#[inline]
#[must_use]
pub fn from_rotation_arc(from: Vec3, to: Vec3) -> Quat {
    const ONE_MINUS_EPS: f32 = 1.0 - 2.0 * f32::EPSILON;
    let dot = from.dot(to);
    if dot > ONE_MINUS_EPS {
        Quat::IDENTITY
    } else if dot < -ONE_MINUS_EPS {
        // Half turn: angle PI, half angle PI * 0.5 (as glam computes it).
        let half = core::f32::consts::PI * 0.5;
        let axis = from.any_orthonormal_vector() * libm::sinf(half);
        Quat::from_xyzw(axis.x, axis.y, axis.z, libm::cosf(half))
    } else {
        let c = from.cross(to);
        normalize(Quat::from_xyzw(c.x, c.y, c.z, 1.0 + dot))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bits4(q: Quat) -> [u32; 4] {
        q.to_array().map(f32::to_bits)
    }

    fn bits3(v: Vec3) -> [u32; 3] {
        v.to_array().map(f32::to_bits)
    }

    fn close4(a: Quat, b: Quat) -> bool {
        a.to_array()
            .iter()
            .zip(b.to_array())
            .all(|(x, y)| (x - y).abs() <= 1e-6)
    }

    /// SPEC §2.4 / D9: identities and known values of every function.
    #[test]
    fn identity_and_known_values() {
        let q = normalize(Quat::from_xyzw(0.1, -0.2, 0.3, 0.9));
        assert_eq!(bits4(mul(Quat::IDENTITY, q)), bits4(q));
        assert_eq!(bits4(mul(q, Quat::IDENTITY)), bits4(q));
        let v = Vec3::new(1.0, -2.0, 3.0);
        assert_eq!(bits3(mul_vec3(Quat::IDENTITY, v)), bits3(v));
        assert_eq!(length_squared(Quat::from_xyzw(1.0, 2.0, 3.0, 4.0)), 30.0);
        assert_eq!(from_rotation_arc(Vec3::Z, Vec3::Z), Quat::IDENTITY);
        // i * j = k (Hamilton).
        let k = mul(
            Quat::from_xyzw(1.0, 0.0, 0.0, 0.0),
            Quat::from_xyzw(0.0, 1.0, 0.0, 0.0),
        );
        assert_eq!(k, Quat::from_xyzw(0.0, 0.0, 1.0, 0.0));
        // A quarter turn about Z maps X to Y.
        let h = core::f32::consts::FRAC_1_SQRT_2;
        let r = mul_vec3(Quat::from_xyzw(0.0, 0.0, h, h), Vec3::X);
        assert!((r - Vec3::Y).length() < 1e-6, "{r:?}");
        // q * conj(q) = identity for unit q.
        assert!(close4(mul(q, q.conjugate()), Quat::IDENTITY));
    }

    /// SPEC §2.4 / D9: `normalize` of a non-unit quaternion divides by the
    /// length ((1, 2, 2, 4) has length 5).
    #[test]
    fn normalize_non_unit() {
        let n = normalize(Quat::from_xyzw(1.0, 2.0, 2.0, 4.0));
        assert_eq!(n, Quat::from_xyzw(0.2, 0.4, 0.4, 0.8));
        assert!((length_squared(n) - 1.0).abs() < 1e-6);
        let tiny = normalize(Quat::from_xyzw(0.0, -3e-3, 0.0, 4e-3));
        assert!(
            close4(tiny, Quat::from_xyzw(0.0, -0.6, 0.0, 0.8)),
            "{tiny:?}"
        );
    }

    /// SPEC §2.4 / D9: the 180-degree branch of `from_rotation_arc` is a
    /// half turn about an axis orthogonal to `from` and maps `from` to `to`.
    #[test]
    fn from_rotation_arc_half_turn() {
        for from in [
            Vec3::X,
            Vec3::Y,
            Vec3::Z,
            -Vec3::Z,
            Vec3::new(0.6, -0.8, 0.0),
        ] {
            let q = from_rotation_arc(from, -from);
            assert!(q.w.abs() < 1e-7, "{q:?}");
            assert!((length_squared(q) - 1.0).abs() < 1e-6);
            assert!(Vec3::new(q.x, q.y, q.z).dot(from).abs() < 1e-6);
            assert!((mul_vec3(q, from) + from).length() < 1e-5, "{from:?}");
        }
        // General branch: maps `from` to `to`.
        let to = Vec3::new(0.3, -0.4, 0.5).normalize();
        let q = from_rotation_arc(Vec3::Z, to);
        assert!((mul_vec3(q, Vec3::Z) - to).length() < 1e-6);
    }

    /// Seeded PCG (port of the S7 probe generator): inputs built by scalar
    /// code, so they are bit-identical on every target.
    struct Pcg(u64);

    impl Pcg {
        fn next(&mut self) -> u32 {
            let old = self.0;
            self.0 = old
                .wrapping_mul(6_364_136_223_846_793_005)
                .wrapping_add(1_442_695_040_888_963_407);
            let x = (((old >> 18) ^ old) >> 27) as u32;
            x.rotate_right((old >> 59) as u32)
        }

        /// A value in [-1, 1), with exact `0.0` / `-0.0` about 1 in 97.
        fn f(&mut self) -> f32 {
            let r = self.next();
            if r.is_multiple_of(97) {
                if r & 1 == 0 { 0.0 } else { -0.0 }
            } else {
                (r >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
            }
        }
    }

    /// FNV-1a over the bit patterns of every output.
    #[derive(Clone, Copy)]
    struct Fnv(u64);

    impl Fnv {
        const fn new() -> Self {
            Self(0xcbf2_9ce4_8422_2325)
        }

        fn u(&mut self, x: u32) {
            for b in x.to_le_bytes() {
                self.0 ^= u64::from(b);
                self.0 = self.0.wrapping_mul(0x100_0000_01b3);
            }
        }

        fn s(&mut self, v: &[f32]) {
            for x in v {
                self.u(x.to_bits());
            }
        }
    }

    fn unit4(v: [f32; 4]) -> [f32; 4] {
        let n = ((v[0] * v[0] + v[1] * v[1]) + (v[2] * v[2] + v[3] * v[3])).sqrt();
        if n > 1e-3 {
            [v[0] / n, v[1] / n, v[2] / n, v[3] / n]
        } else {
            [0.0, 0.0, 0.0, 1.0]
        }
    }

    fn unit3(v: [f32; 3]) -> [f32; 3] {
        let n = ((v[0] * v[0] + v[1] * v[1]) + v[2] * v[2]).sqrt();
        if n > 1e-3 {
            [v[0] / n, v[1] / n, v[2] / n]
        } else {
            [0.0, 0.0, 1.0]
        }
    }

    /// The sweep's input helpers fall back to the identity / +Z for
    /// near-zero inputs (which the fixed PCG sequence happens not to draw).
    #[test]
    fn sweep_input_fallbacks() {
        assert_eq!(unit4([0.0, -0.0, 1e-4, 0.0]), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(unit3([-0.0, 1e-4, 0.0]), [0.0, 0.0, 1.0]);
        assert_eq!(unit4([0.0, 0.0, 0.0, 2.0]), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(unit3([0.0, 3.0, 0.0]), [0.0, 1.0, 0.0]);
    }

    /// The operations the digest sweep hashes, one digest per entry.
    const DIGEST_NAMES: [&str; 9] = [
        "mul",
        "mul_non_unit",
        "length_squared",
        "normalize",
        "mul_vec3",
        "conj_mul_vec3",
        "mul_vec3_non_unit",
        "from_rotation_arc",
        "integrate_orientation",
    ];

    /// One implementation of the swept operations (the replica or glam).
    struct Ops {
        mul: fn(Quat, Quat) -> Quat,
        length_squared: fn(Quat) -> f32,
        normalize: fn(Quat) -> Quat,
        mul_vec3: fn(Quat, Vec3) -> Vec3,
        from_rotation_arc: fn(Vec3, Vec3) -> Quat,
        integrate: fn(Quat, Vec3, f32) -> Quat,
    }

    /// Inputs per sweep.
    const SWEEP: usize = 100_000;

    /// Runs the fixed PCG sweep through `ops` and returns one FNV digest per
    /// [`DIGEST_NAMES`] entry. Inputs are built with scalar code only.
    fn sweep(ops: &Ops) -> [u64; 9] {
        let mut h = [Fnv::new(); 9];
        let mut r = Pcg(0x0057_A7A7);
        for i in 0..SWEEP {
            let ra = [r.f(), r.f(), r.f(), r.f()];
            let rb = [r.f(), r.f(), r.f(), r.f()];
            let qa = Quat::from_array(unit4(ra));
            let qb = Quat::from_array(unit4(rb));
            let v = Vec3::new(r.f(), r.f(), r.f());
            let u = Vec3::new(r.f(), r.f(), r.f());
            let w = Vec3::new(r.f() * 300.0, r.f() * 300.0, r.f() * 300.0);
            // Magnitudes 1e-3 .. 1e3 for the non-unit inputs.
            let scale = [1e-3f32, 1e-1, 1.0, 10.0, 1e3][i % 5];
            let raw = Quat::from_array(ra.map(|c| c * scale));
            let big = Vec3::new(v.x * 1e3, v.y * 1e3, v.z * 1e3);
            let vn = Vec3::from_array(unit3(v.to_array()));
            let un = Vec3::from_array(unit3(u.to_array()));
            // Some arcs are half turns, some the identity case.
            let to = match i % 272 {
                0 => -un,
                17 => un,
                _ => vn,
            };

            h[0].s(&(ops.mul)(qa, qb).to_array());
            h[1].s(&(ops.mul)(raw, Quat::from_array(rb)).to_array());
            let l2 = (ops.length_squared)(raw);
            h[2].u(l2.to_bits());
            if l2 > 0.0 {
                h[3].s(&(ops.normalize)(raw).to_array());
            }
            h[4].s(&(ops.mul_vec3)(qa, big).to_array());
            h[5].s(&(ops.mul_vec3)(qa.conjugate(), v).to_array());
            h[6].s(&(ops.mul_vec3)(raw, v).to_array());
            h[7].s(&(ops.from_rotation_arc)(Vec3::Z, vn).to_array());
            h[7].s(&(ops.from_rotation_arc)(un, to).to_array());
            h[8].s(&(ops.integrate)(qa, w, 0.001).to_array());
        }
        h.map(|d| d.0)
    }

    const REPLICA: Ops = Ops {
        mul,
        length_squared,
        normalize,
        mul_vec3,
        from_rotation_arc,
        integrate: crate::ball::contact::integrate_orientation,
    };

    /// Digests of [`sweep`] over the replica, recorded on x86_64 Linux
    /// (where they also equal glam's SSE2 digests, see
    /// `against_glam_sse2::digest_equals_glam_sse2`).
    const EXPECTED_DIGESTS: [u64; 9] = [
        0xa529_e348_7223_c76f, // mul
        0x839d_79df_bf55_be2d, // mul_non_unit
        0x0ed0_b769_0500_50f3, // length_squared
        0x037d_c4a1_f921_6a15, // normalize
        0x5c9a_dff8_4e5d_5a74, // mul_vec3
        0x500e_e205_5070_4903, // conj_mul_vec3
        0x7df2_38c4_a9c1_97fc, // mul_vec3_non_unit
        0x4aec_173c_b610_e7b3, // from_rotation_arc
        0x73a7_0466_a375_eb86, // integrate_orientation
    ];

    /// SPEC §2.4 / D9, REQ-WEB-002 prerequisite: a fixed PCG sweep of
    /// 100 000 inputs through every `quat_ops` function and the sim's
    /// `integrate_orientation` hashes to the x86_64-recorded digests on every
    /// target (aarch64, wasm32, x86_64). This is the cross-target proof: the
    /// glam comparison cannot compile where glam has no SSE2/simd128 path.
    #[test]
    fn digest_is_target_independent() {
        let got = sweep(&REPLICA);
        // Printed on every target (visible with `--nocapture`) for re-recording.
        for (n, d) in DIGEST_NAMES.iter().zip(got) {
            println!("{n}: 0x{d:016x}");
        }
        for ((n, g), e) in DIGEST_NAMES.iter().zip(got).zip(EXPECTED_DIGESTS) {
            assert_eq!(
                g, e,
                "quat_ops digest {n}: got 0x{g:016x}, x86_64 0x{e:016x}"
            );
        }
    }

    // glam's SSE2 path (x86_64) and its simd128 path (wasm32) are the
    // reference: the replica must give the same bits for every input.
    #[cfg(any(target_feature = "sse2", target_feature = "simd128"))]
    mod against_glam_sse2 {
        use super::*;
        use proptest::prelude::*;

        /// The sim's `integrate_orientation` written with glam operators
        /// (the code before this module existed).
        fn integrate_glam(q: Quat, w: Vec3, t: f32) -> Quat {
            let rate = w.length();
            let half = 0.5 * rate * t;
            if half.is_nan() || half <= 0.0 || !half.is_finite() {
                return q;
            }
            let s = libm::sinf(half) / rate;
            let dq = Quat::from_xyzw(w.x * s, w.y * s, w.z * s, libm::cosf(half));
            (dq * q).normalize()
        }

        const GLAM: Ops = Ops {
            mul: |a, b| a * b,
            length_squared: Quat::length_squared,
            normalize: Quat::normalize,
            mul_vec3: |q, v| q * v,
            from_rotation_arc: Quat::from_rotation_arc,
            integrate: integrate_glam,
        };

        /// The glam reference of `integrate_orientation` returns `q`
        /// unchanged for zero spin, as the sim does.
        #[test]
        fn integrate_glam_zero_rate() {
            let q = Quat::from_xyzw(0.5, -0.5, 0.5, 0.5);
            for (w, t) in [(Vec3::ZERO, 0.001), (Vec3::X, 0.0), (Vec3::Y, f32::NAN)] {
                assert_eq!(bits4(integrate_glam(q, w, t)), bits4(q));
                assert_eq!(
                    bits4(integrate_glam(q, w, t)),
                    bits4(crate::ball::contact::integrate_orientation(q, w, t))
                );
            }
        }

        /// SPEC §2.4 / D9: on the SSE2/simd128 path the pinned sweep digests
        /// are exactly glam's.
        #[test]
        fn digest_equals_glam_sse2() {
            let glam = sweep(&GLAM);
            for ((n, g), e) in DIGEST_NAMES.iter().zip(glam).zip(EXPECTED_DIGESTS) {
                assert_eq!(g, e, "glam digest {n}: 0x{g:016x} vs pinned 0x{e:016x}");
            }
        }

        /// SPEC §2.4 / D9 (R3 libm): the libm half-turn branch equals glam's
        /// `from_axis_angle` (std `sin_cos`) bit for bit for several axes.
        #[test]
        fn half_turn_equals_glam() {
            for from in [
                Vec3::X,
                Vec3::Y,
                Vec3::Z,
                -Vec3::X,
                -Vec3::Y,
                -Vec3::Z,
                Vec3::new(0.6, -0.8, 0.0),
                Vec3::new(0.0, 0.6, 0.8),
                Vec3::new(-0.48, 0.6, 0.64),
                Vec3::new(1.0, 2.0, -3.0).normalize(),
            ] {
                assert_eq!(
                    bits4(from_rotation_arc(from, -from)),
                    bits4(Quat::from_rotation_arc(from, -from)),
                    "{from:?}"
                );
            }
        }

        /// A component in [-1, 1), of magnitude 1e-3 .. 1e3 (log-uniform),
        /// or an exact `0.0` / `-0.0`.
        fn comp() -> impl Strategy<Value = f32> {
            prop_oneof![
                1 => Just(0.0f32),
                1 => Just(-0.0f32),
                4 => -1.0f32..1.0,
                4 => (-3.0f32..3.0, any::<bool>()).prop_map(|(e, neg)| {
                    let m = libm::powf(10.0, e);
                    if neg { -m } else { m }
                }),
            ]
        }

        fn quat() -> impl Strategy<Value = Quat> {
            (comp(), comp(), comp(), comp()).prop_map(|(x, y, z, w)| Quat::from_xyzw(x, y, z, w))
        }

        fn vec3() -> impl Strategy<Value = Vec3> {
            (comp(), comp(), comp()).prop_map(|(x, y, z)| Vec3::new(x, y, z))
        }

        proptest! {
            #![proptest_config(ProptestConfig::with_cases(20_000))]

            /// SPEC §2.4 / D9: every replica output equals glam's SSE2
            /// (simd128) result bit for bit, for unit and non-unit
            /// quaternions, magnitudes 1e-3 .. 1e3, zeros and -0.0.
            #[test]
            fn matches_glam_sse2_bit_for_bit(raw in quat(), b in quat(), v in vec3(), u in vec3()) {
                let len2 = raw.length_squared();
                // The sim normalises only finite, non-zero quaternions.
                let ok = len2 > 0.0 && len2.is_finite();
                let unit = if ok { raw.normalize() } else { Quat::IDENTITY };
                prop_assert_eq!(length_squared(raw).to_bits(), len2.to_bits());
                prop_assert_eq!(length_squared(b).to_bits(), b.length_squared().to_bits());
                if ok {
                    prop_assert_eq!(bits4(normalize(raw)), bits4(raw.normalize()));
                }
                for (p, q) in [(unit, b), (raw, b), (b, raw), (unit, unit.conjugate())] {
                    prop_assert_eq!(bits4(mul(p, q)), bits4(p * q));
                }
                for q in [unit, raw] {
                    prop_assert_eq!(bits3(mul_vec3(q, v)), bits3(q * v));
                    prop_assert_eq!(bits3(mul_vec3(q.conjugate(), v)), bits3(q.conjugate() * v));
                    prop_assert_eq!(bits3(mul_vec3(q, u)), bits3(q * u));
                }
                // Unit vectors only, as the sim feeds (flipper frame normal).
                if v.length_squared() > 1e-12 && u.length_squared() > 1e-12 {
                    let (vn, un) = (v.normalize(), u.normalize());
                    for (f, t) in [(Vec3::Z, vn), (un, vn), (vn, vn), (vn, -vn), (un, -un)] {
                        prop_assert_eq!(
                            bits4(from_rotation_arc(f, t)),
                            bits4(Quat::from_rotation_arc(f, t)),
                            "from {:?} to {:?}", f, t
                        );
                    }
                }
            }
        }
    }
}
