//! S7 mini probe: FNV-1a digests of glam 0.33.12 operation results over PCG inputs that are
//! built by scalar code (bit-identical on every target), plus the portable replica
//! (`quat_ops.rs`, the same file as the pin_physics patch) and its bitwise agreement with glam
//! on the running target. Self-contained (no pin_* crates) so it builds in seconds for any
//! target / feature / RUSTFLAGS combination.
use core::hint::black_box as bb;
use glam::{Mat4, Quat, Vec3, Vec3A, Vec4};
mod quat_ops;

struct Pcg(u64);
impl Pcg {
    fn next(&mut self) -> u32 {
        let old = self.0;
        self.0 = old.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        let x = (((old >> 18) ^ old) >> 27) as u32;
        x.rotate_right((old >> 59) as u32)
    }
    fn f(&mut self) -> f32 {
        let r = self.next();
        if r % 97 == 0 {
            if r & 1 == 0 { 0.0 } else { -0.0 }
        } else {
            (r >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
        }
    }
}

#[derive(Clone, Copy)]
struct H(u64);
impl H {
    fn u(&mut self, x: u32) {
        for b in x.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x100_0000_01b3);
        }
    }
    fn f(&mut self, x: f32) {
        self.u(x.to_bits());
    }
    fn s(&mut self, v: &[f32]) {
        v.iter().for_each(|x| self.f(*x));
    }
}

fn unit4(v: [f32; 4]) -> [f32; 4] {
    let n = ((v[0] * v[0] + v[1] * v[1]) + (v[2] * v[2] + v[3] * v[3])).sqrt();
    if n > 1e-3 { [v[0] / n, v[1] / n, v[2] / n, v[3] / n] } else { [0.0, 0.0, 0.0, 1.0] }
}
fn unit3(v: [f32; 3]) -> [f32; 3] {
    let n = ((v[0] * v[0] + v[1] * v[1]) + v[2] * v[2]).sqrt();
    if n > 1e-3 { [v[0] / n, v[1] / n, v[2] / n] } else { [0.0, 0.0, 1.0] }
}

/// The sim's `pin_physics::ball::contact::integrate_orientation`, verbatim, with glam ops.
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
/// The same with the portable replica (the patch).
fn integrate_portable(q: Quat, w: Vec3, t: f32) -> Quat {
    let rate = w.length();
    let half = 0.5 * rate * t;
    if half.is_nan() || half <= 0.0 || !half.is_finite() {
        return q;
    }
    let s = libm::sinf(half) / rate;
    let dq = Quat::from_xyzw(w.x * s, w.y * s, w.z * s, libm::cosf(half));
    quat_ops::normalize(quat_ops::mul(dq, q))
}

const N: usize = 1_000_000;

fn main() {
    let names = [
        "quat_mul", "quat_dot", "quat_length_squared", "quat_normalize", "quat_mul_vec3",
        "quat_conj_mul_vec3", "quat_from_rotation_arc", "integrate_orientation", "vec4_dot",
        "vec4_length", "vec4_normalize", "vec4_min_max", "vec3a_cross", "vec3a_dot",
        "vec3a_length", "vec3_ops", "vec3_min_max", "f32_min_max", "mat4_mul_mat4",
        "mat4_transform_point3", "mat4_inverse", "libm", "f32_mul_add", "quat_lerp_slerp",
        "vec3a_lerp",
        // portable replica (patch) digests: must equal the glam digests of x86_64 SSE2
        "P:quat_mul", "P:quat_length_squared", "P:quat_normalize", "P:quat_mul_vec3",
        "P:quat_conj_mul_vec3", "P:quat_from_rotation_arc", "P:integrate_orientation",
    ];
    let mut h = vec![H(0xcbf2_9ce4_8422_2325); names.len()];
    let mut mism = [0u64; 7]; // portable != glam on this target
    let mut r = Pcg(0x0057_A7A7);
    for _ in 0..N {
        let ra = [r.f(), r.f(), r.f(), r.f()];
        let rb = [r.f(), r.f(), r.f(), r.f()];
        let qa = Quat::from_array(unit4(ra));
        let qb = Quat::from_array(unit4(rb));
        let v = Vec3::new(r.f(), r.f(), r.f());
        let u = Vec3::new(r.f(), r.f(), r.f());
        let w = Vec3::new(r.f() * 300.0, r.f() * 300.0, r.f() * 300.0);
        let a = Vec4::from_array(ra);
        let b = Vec4::from_array(rb);
        let vn = Vec3::from_array(unit3(v.to_array()));
        let raw = Quat::from_array(ra);
        let len_ok = (ra[0] * ra[0] + ra[1] * ra[1]) + (ra[2] * ra[2] + ra[3] * ra[3]) > 1e-6;

        let g_mul = bb(qa) * bb(qb);
        h[0].s(&g_mul.to_array());
        h[1].f(bb(raw).dot(bb(Quat::from_array(rb))));
        let g_l2 = bb(raw).length_squared();
        h[2].f(g_l2);
        let g_norm = if len_ok { Some(bb(raw).normalize()) } else { None };
        if let Some(q) = g_norm { h[3].s(&q.to_array()); }
        let g_mv = bb(qa) * bb(v);
        h[4].s(&g_mv.to_array());
        let g_cmv = bb(qa).conjugate() * bb(v);
        h[5].s(&g_cmv.to_array());
        let g_arc = Quat::from_rotation_arc(Vec3::Z, bb(vn));
        h[6].s(&g_arc.to_array());
        let g_int = integrate_glam(bb(qa), bb(w), 0.001);
        h[7].s(&g_int.to_array());
        h[8].f(bb(a).dot(bb(b)));
        h[9].f(bb(a).length());
        h[10].s(&bb(a).normalize_or_zero().to_array());
        h[11].s(&bb(a).min(bb(b)).to_array());
        h[11].s(&bb(a).max(bb(b)).to_array());
        let (va, ua) = (Vec3A::from(v), Vec3A::from(u));
        h[12].s(&bb(va).cross(bb(ua)).to_array());
        h[13].f(bb(va).dot(bb(ua)));
        h[14].f(bb(va).length());
        h[15].s(&bb(v).cross(bb(u)).to_array());
        h[15].f(bb(v).dot(bb(u)));
        h[15].f(bb(v).length());
        h[15].s(&bb(v).normalize_or_zero().to_array());
        h[16].s(&bb(v).min(bb(u)).to_array());
        h[16].s(&bb(v).max(bb(u)).to_array());
        h[17].f(bb(a.x).min(bb(b.x)));
        h[17].f(bb(a.x).max(bb(b.x)));
        h[17].f(bb(a.y).max(0.0));
        h[17].f(bb(a.y).min(0.0));
        let ma = Mat4::from_rotation_translation(bb(qa), bb(v));
        let mb = Mat4::from_scale_rotation_translation(Vec3::new(1.0 + a.x.abs(), 1.0, 2.0), bb(qb), bb(w));
        h[18].s(&(ma * mb).to_cols_array());
        h[19].s(&mb.transform_point3(bb(v)).to_array());
        h[20].s(&mb.inverse().to_cols_array());
        h[21].f(libm::sinf(bb(w.x)));
        h[21].f(libm::cosf(bb(w.y)));
        h[21].f(libm::atan2f(bb(w.y), bb(w.z)));
        h[21].f(libm::expf(bb(a.x) * 10.0));
        h[21].f(libm::sqrtf(bb(a.y).abs()));
        h[21].f(libm::acosf(bb(a.z)));
        h[21].f(libm::powf(bb(a.x).abs(), bb(b.y) * 4.0));
        h[22].f(bb(a.x).mul_add(bb(b.x), bb(a.y)));
        h[23].s(&bb(qa).lerp(bb(qb), 0.3).to_array());
        h[23].s(&bb(qa).slerp(bb(qb), 0.3).to_array());
        h[24].s(&bb(va).lerp(bb(ua), 0.3).to_array());

        let p_mul = quat_ops::mul(bb(qa), bb(qb));
        h[25].s(&p_mul.to_array());
        let p_l2 = quat_ops::length_squared(bb(raw));
        h[26].f(p_l2);
        let p_norm = if len_ok { Some(quat_ops::normalize(bb(raw))) } else { None };
        if let Some(q) = p_norm { h[27].s(&q.to_array()); }
        let p_mv = quat_ops::mul_vec3(bb(qa), bb(v));
        h[28].s(&p_mv.to_array());
        let p_cmv = quat_ops::mul_vec3(bb(qa).conjugate(), bb(v));
        h[29].s(&p_cmv.to_array());
        let p_arc = quat_ops::from_rotation_arc(Vec3::Z, bb(vn));
        h[30].s(&p_arc.to_array());
        let p_int = integrate_portable(bb(qa), bb(w), 0.001);
        h[31].s(&p_int.to_array());
        let ne4 = |x: Quat, y: Quat| x.to_array().map(f32::to_bits) != y.to_array().map(f32::to_bits);
        let ne3 = |x: Vec3, y: Vec3| x.to_array().map(f32::to_bits) != y.to_array().map(f32::to_bits);
        mism[0] += u64::from(ne4(g_mul, p_mul));
        mism[1] += u64::from(g_l2.to_bits() != p_l2.to_bits());
        mism[2] += u64::from(matches!((g_norm, p_norm), (Some(x), Some(y)) if ne4(x, y)));
        mism[3] += u64::from(ne3(g_mv, p_mv));
        mism[4] += u64::from(ne3(g_cmv, p_cmv));
        mism[5] += u64::from(ne4(g_arc, p_arc));
        mism[6] += u64::from(ne4(g_int, p_int));
    }
    println!("target_arch={} N={N}", std::env::consts::ARCH);
    for (n, h) in names.iter().zip(&h) {
        println!("{n}={:016x}", h.0);
    }
    let mn = ["quat_mul", "quat_length_squared", "quat_normalize", "quat_mul_vec3", "quat_conj_mul_vec3", "quat_from_rotation_arc", "integrate_orientation"];
    let s: Vec<String> = mn.iter().zip(mism).map(|(n, m)| format!("{n}:{m}")).collect();
    println!("portable_vs_glam_mismatches(of {N}) {}", s.join(" "));
}
