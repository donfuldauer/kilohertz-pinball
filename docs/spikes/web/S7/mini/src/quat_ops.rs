//! Target-independent quaternion arithmetic for the simulation.
//!
//! glam 0.33.12 implements `Quat` with SSE2 on x86_64, `simd128` on wasm32
//! (op for op the same as SSE2), NEON on aarch64 and plain scalar code
//! elsewhere. The NEON code sums the 4-lane dot product in a different order
//! (`vaddvq_f32`: `(x + y) + (z + w)`), normalises by multiplying with the
//! reciprocal length instead of dividing, and fuses the cross product inside
//! `Quat * Vec3` (`vfmsq_f32`), so the same ball spin gives different
//! orientation bits on Apple Silicon than on x86_64.
//!
//! The functions below are plain `f32` code that reproduces glam's SSE2
//! results bit for bit (same products, same additions in the same grouping,
//! no fused operation; rustc never contracts `a * b + c`). They give the same
//! bits on every IEEE-754 target, so x86_64 output is unchanged and aarch64
//! and wasm32 (with or without `simd128`) match it (PLAN.md §2.4, D9).
//! NaN payloads are not part of the guarantee.

use glam::{Quat, Vec3};

/// `a * b` (Hamilton product), as glam's SSE2 `Quat::mul_quat`
/// (glam-0.33.12 `src/f32/sse2/quat.rs:885-919`).
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

/// Squared length with SSE2's grouping `(x² + z²) + (y² + w²)`
/// (glam-0.33.12 `src/sse2.rs:75-80`, `dot4_into_m128`).
#[inline]
#[must_use]
pub fn length_squared(q: Quat) -> f32 {
    (q.x * q.x + q.z * q.z) + (q.y * q.y + q.w * q.w)
}

/// `q / |q|` component-wise (a division, as SSE2 `Vec4::normalize`,
/// glam-0.33.12 `src/f32/sse2/vec4.rs:654-662`; not a reciprocal multiply).
#[inline]
#[must_use]
pub fn normalize(q: Quat) -> Quat {
    let len = length_squared(q).sqrt();
    Quat::from_xyzw(q.x / len, q.y / len, q.z / len, q.w / len)
}

/// `q * v` (rotation of `v`), as glam's SSE2 `Quat::mul_vec3a`
/// (glam-0.33.12 `src/f32/sse2/quat.rs:964-980`, cross product
/// `src/f32/sse2/vec3a.rs:279-291` without FMA).
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
/// `Quat::from_rotation_arc` with [`normalize`] for the final step.
#[inline]
#[must_use]
pub fn from_rotation_arc(from: Vec3, to: Vec3) -> Quat {
    const ONE_MINUS_EPS: f32 = 1.0 - 2.0 * f32::EPSILON;
    let dot = from.dot(to);
    if dot > ONE_MINUS_EPS {
        Quat::IDENTITY
    } else if dot < -ONE_MINUS_EPS {
        // Same as glam (axis-angle of a half turn); not reached by the
        // flipper frame, which handles -Z itself.
        Quat::from_axis_angle(from.any_orthonormal_vector(), core::f32::consts::PI)
    } else {
        let c = from.cross(to);
        normalize(Quat::from_xyzw(c.x, c.y, c.z, 1.0 + dot))
    }
}
