//! PR #16 review finding C: `CubicSpline::closest_point` bisects a sample
//! interval that may hold several roots of `g = (C - p) · C'` only to a
//! fixed depth. At that depth a piece whose ends do not bracket a root is
//! dropped and one whose ends do has one root refined. This sweep checks,
//! on tight splines (near-cusps, loops and uneven zig-zags inside single
//! segments) with query points near centres of curvature, where intervals
//! hold several roots and the depth limit is reached, that the result is
//! still the global minimum within the documented tolerance, and that
//! seeded and unseeded queries agree.
//!
//! Tolerance: 1 µm (as in `spline_closest_point_roots.rs`), several f32
//! ulps at the 2 m bound on the query coordinates. The release-mode sweeps
//! recorded in docs/DECISIONS.md ("closest_point depth-limit bound") found
//! at most 0.46 µm, all f32 rounding.

use glam::Vec3;
use pin_physics::world::CubicSpline;

/// Distance within this of the brute-force minimum, and seeded within this
/// of unseeded (metres).
const TOL: f64 = 1.0e-6;
/// Queries per seed.
const CASES: usize = 7_000;
/// Fixed seeds (two sweeps; over 10^4 queries are checked in all).
const SEEDS: [u64; 2] = [0x5eed_c0de_0001, 0x5eed_c0de_0002];
/// Brute-force samples per segment.
const PER: usize = 256;

/// SplitMix64: a fixed, seeded test-input generator.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * ((self.next() >> 40) as f32 / (1u64 << 24) as f32)
    }
}

/// Dense brute-force closest distance (f64 arithmetic on the f32 curve):
/// every local minimum of the samples is refined by ternary search.
fn brute(spline: &CubicSpline, p: Vec3) -> f64 {
    let total = spline.segment_count() * PER;
    let dist = |t: f64| (spline.point(t as f32).as_dvec3() - p.as_dvec3()).length();
    let ds: Vec<f64> = (0..=total).map(|k| dist(k as f64 / PER as f64)).collect();
    let mut best = f64::INFINITY;
    for k in 0..=total {
        best = best.min(ds[k]);
        let left = if k > 0 { ds[k - 1] } else { f64::INFINITY };
        let right = if k < total { ds[k + 1] } else { f64::INFINITY };
        if ds[k] <= left && ds[k] <= right {
            let mut lo = (k as f64 - 1.0).max(0.0) / PER as f64;
            let mut hi = ((k + 1) as f64).min(total as f64) / PER as f64;
            for _ in 0..40 {
                let a = lo + (hi - lo) / 3.0;
                let b = hi - (hi - lo) / 3.0;
                if dist(a) < dist(b) {
                    hi = b;
                } else {
                    lo = a;
                }
            }
            best = best.min(dist(0.5 * (lo + hi)));
        }
    }
    best
}

/// A random tight spline: a near-cusp like the issue #8 review curve,
/// clustered points (loops and cusps inside segments), or an uneven
/// zig-zag; 5 mm to 0.3 m across.
fn random_spline(r: &mut Rng) -> CubicSpline {
    loop {
        let scale = r.range(0.005, 0.3);
        let pts: Vec<Vec3> = match r.next() % 3 {
            0 => {
                let a = r.range(0.005, 0.4);
                let (b, c) = (r.range(-1.0e-3, 1.0e-3), r.range(-1.0e-3, 1.0e-3));
                vec![
                    Vec3::ZERO,
                    Vec3::new(scale, 0.0, 0.0),
                    Vec3::new(scale * (1.0 + a), scale * b, scale * c),
                ]
            }
            1 => {
                let n = 3 + (r.next() % 4) as usize;
                let mut p = Vec3::ZERO;
                let mut v = Vec::with_capacity(n);
                for _ in 0..n {
                    v.push(p);
                    let step = scale * 10f32.powf(r.range(-3.0, 0.0));
                    p += Vec3::new(r.range(-1.0, 1.0), r.range(-1.0, 1.0), r.range(-0.2, 0.2))
                        * step;
                }
                v
            }
            _ => {
                let n = 4 + (r.next() % 5) as usize;
                (0..n)
                    .map(|i| {
                        let x = 0.1 * scale * (i as f32 + r.range(-0.45, 0.45));
                        let y = if i % 2 == 0 {
                            0.0
                        } else {
                            scale * r.range(0.02, 0.3)
                        };
                        Vec3::new(x, y, scale * r.range(-0.01, 0.01))
                    })
                    .collect()
            }
        };
        if let Ok(s) = CubicSpline::interpolate(&pts) {
            return s;
        }
    }
}

/// A query point near the centre of curvature of a random curve point
/// (relative offsets of 0.1 %, 5 % and 50 % along the normal), or anywhere
/// around the curve; with the curve parameter it was made from.
fn random_query(r: &mut Rng, s: &CubicSpline) -> (Vec3, f32) {
    let t = r.range(0.0, s.max_t());
    let c = s.point(t);
    let d1 = s.tangent(t);
    let d2 = s.second_derivative(t);
    let l = d1.length();
    let cross = d1.cross(d2).length();
    let mode = r.next() % 4;
    if mode < 3 && l > 0.0 && cross > 0.0 {
        let rho = l * l * l / cross;
        let normal = (d2 - d1 * (d1.dot(d2) / (l * l))).normalize_or_zero();
        let delta = match mode {
            0 => r.range(-1.0e-3, 1.0e-3),
            1 => r.range(-0.05, 0.05),
            _ => r.range(-0.5, 0.5),
        };
        let side = d1.cross(normal).normalize_or_zero() * rho * r.range(-0.02, 0.02);
        (c + normal * rho * (1.0 + delta) + side, t)
    } else {
        let mut lo = Vec3::splat(f32::MAX);
        let mut hi = Vec3::splat(f32::MIN);
        for k in 0..=64 {
            let q = s.point(k as f32 / 64.0 * s.max_t());
            lo = lo.min(q);
            hi = hi.max(q);
        }
        let pad = Vec3::splat(0.1 * (hi - lo).max_element());
        let (lo, hi) = (lo - pad, hi + pad);
        let p = Vec3::new(
            r.range(lo.x, hi.x),
            r.range(lo.y, hi.y),
            r.range(lo.z, hi.z),
        );
        (p, t)
    }
}

#[test]
fn tight_spline_sweep_finds_the_global_minimum() {
    let mut worst_brute = 0.0f64;
    let mut worst_seed = 0.0f64;
    let mut checked = 0usize;
    for seed in SEEDS {
        let mut r = Rng(seed);
        let mut s = random_spline(&mut r);
        for case in 0..CASES {
            if case % 16 == 0 {
                s = random_spline(&mut r);
            }
            let (p, t) = random_query(&mut r, &s);
            // Centres of nearly straight pieces lie far away, where f32
            // rounding of the coordinates alone exceeds the tolerance.
            if !p.is_finite() || p.length() >= 2.0 {
                continue;
            }
            checked += 1;
            let un = s.closest_point(p, None);
            let expected = brute(&s, p);
            let err = f64::from(un.distance) - expected;
            worst_brute = worst_brute.max(err);
            assert!(
                err <= TOL,
                "seed {seed:#x} case {case} p {p}: {} (t {}) vs brute force {expected}",
                un.distance,
                un.t
            );
            for seed_t in [t, 0.0, s.max_t(), r.range(0.0, s.max_t())] {
                let c = s.closest_point(p, Some(seed_t));
                let diff = f64::from((c.distance - un.distance).abs());
                worst_seed = worst_seed.max(diff);
                assert!(
                    diff <= TOL,
                    "seed {seed:#x} case {case} p {p} seed_t {seed_t}: {} vs unseeded {}",
                    c.distance,
                    un.distance
                );
            }
        }
    }
    assert!(checked >= 10_000, "only {checked} queries checked");
    println!(
        "{checked} queries: worst unseeded - brute {worst_brute:e} m, \
         worst |seeded - unseeded| {worst_seed:e} m"
    );
}
