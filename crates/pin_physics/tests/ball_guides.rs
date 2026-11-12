//! Rising and falling guide walls (Subphase 2.2 fix round 2): the wall's
//! closest point is global, and balls never pass through such walls.
//! Ground truth is a brute-force distance to the wall.

#[path = "ball_util.rs"]
mod util;

use glam::Vec3;
use pin_physics::ball::{BallEvents, Geometry};
use pin_physics::world::{CubicSpline, GuideCurve};
use util::*;

const WALL_HEIGHT: f32 = 0.4;

/// Deterministic pseudo-random numbers (the tests are not proptests because
/// every case needs the same expensive guide).
struct Lcg(u64);

impl Lcg {
    /// Uniform in [0, 1).
    fn unit(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }

    /// Uniform in [-1, 1).
    fn signed(&mut self) -> f32 {
        2.0 * self.unit() - 1.0
    }
}

type Shape = (&'static str, fn(f32) -> Vec3);

/// Curves parametrised over s in [0, 1].
fn shapes() -> Vec<Shape> {
    vec![
        ("arc r=15mm rise 1:1", |s| {
            let a = s * 5.0;
            v(
                0.015 * libm::sinf(a),
                0.015 - 0.015 * libm::cosf(a),
                0.075 * s,
            )
        }),
        ("arc r=30mm rise 1:1", |s| {
            let a = s * 5.0;
            v(0.03 * libm::sinf(a), 0.03 - 0.03 * libm::cosf(a), 0.15 * s)
        }),
        ("arc r=30mm rise 3:1", |s| {
            let a = s * 4.0;
            v(0.03 * libm::sinf(a), 0.03 - 0.03 * libm::cosf(a), 0.36 * s)
        }),
        ("arc r=50mm rise 1:1", |s| {
            let a = s * 3.0;
            v(0.05 * libm::sinf(a), 0.05 - 0.05 * libm::cosf(a), 0.15 * s)
        }),
        ("arc r=50mm rise 3:1", |s| {
            let a = s * 3.0;
            v(0.05 * libm::sinf(a), 0.05 - 0.05 * libm::cosf(a), 0.45 * s)
        }),
        ("S-curve rising", |s| {
            v(0.6 * s, 0.06 * libm::sinf(s * 12.0), 0.2 * s)
        }),
        ("S-curve falling tight", |s| {
            v(
                0.3 * s,
                0.03 * libm::sinf(s * 20.0),
                -0.15 * s + 0.02 * libm::sinf(s * 30.0),
            )
        }),
        ("helix 1.5 turns", |s| {
            let a = s * 9.4;
            v(0.08 * libm::sinf(a), 0.08 - 0.08 * libm::cosf(a), 0.12 * s)
        }),
        ("helix 2.5 turns falling", |s| {
            let a = s * 15.7;
            v(0.06 * libm::sinf(a), 0.06 - 0.06 * libm::cosf(a), -0.5 * s)
        }),
        ("arc r=400mm rise 150mm", |s| {
            let a = s * 2.0;
            v(0.4 * libm::sinf(a), 0.4 - 0.4 * libm::cosf(a), 0.15 * s)
        }),
    ]
}

fn guide(f: fn(f32) -> Vec3) -> GuideCurve {
    let pts: Vec<Vec3> = (0..=60).map(|i| f(i as f32 / 60.0)).collect();
    let g = GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), WALL_HEIGHT).unwrap();
    assert!(!g.is_level());
    g
}

/// Distance from `p` to the wall's vertical line over the curve point `c`.
fn line_distance(c: Vec3, p: Vec3) -> f32 {
    let u = (p.z - c.z).clamp(0.0, WALL_HEIGHT);
    v(c.x, c.y, c.z + u).distance(p)
}

/// Dense samples of the guide's own spline (the wall as built).
fn dense(g: &GuideCurve) -> Vec<Vec3> {
    let spline = g.spline();
    let n = spline.segment_count() * 256;
    (0..=n).map(|i| spline.point(i as f32 / 256.0)).collect()
}

/// Brute-force distance to the wall: the dense samples, then a golden
/// section search around the best one. Returns the distance and its index.
fn brute_distance(g: &GuideCurve, dense: &[Vec3], p: Vec3) -> (f32, usize) {
    let (mut best, mut at) = (f32::INFINITY, 0usize);
    for (i, c) in dense.iter().enumerate() {
        let d = line_distance(*c, p);
        if d < best {
            (best, at) = (d, i);
        }
    }
    let spline = g.spline();
    let (mut lo, mut hi) = (
        (at.saturating_sub(1)) as f32 / 256.0,
        ((at + 1).min(dense.len() - 1)) as f32 / 256.0,
    );
    for _ in 0..40 {
        let (a, b) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
        if line_distance(spline.point(a), p) < line_distance(spline.point(b), p) {
            hi = b;
        } else {
            lo = a;
        }
    }
    (
        best.min(line_distance(spline.point(0.5 * (lo + hi)), p)),
        at,
    )
}

#[test]
fn sloped_wall_closest_point_is_global() {
    let mut rng = Lcg(2024);
    for (label, f) in shapes() {
        let g = guide(f);
        let dense = dense(&g);
        let (mut min, mut max) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
        for c in &dense {
            min = min.min(*c);
            max = max.max(*c);
        }
        max.z += WALL_HEIGHT;
        let mut worst_over = 0.0f32;
        for case in 0..600 {
            // Half the points anywhere around the wall, half close to it.
            let p = if case % 2 == 0 {
                let pad = Vec3::splat(0.08);
                let u = v(rng.unit(), rng.unit(), rng.unit());
                (min - pad) + (max - min + 2.0 * pad) * u
            } else {
                let c = dense[(rng.unit() * (dense.len() - 1) as f32) as usize];
                c + v(rng.signed(), rng.signed(), 0.0) * 0.02
                    + Vec3::Z * (WALL_HEIGHT * (1.4 * rng.unit() - 0.2))
            };
            let (truth, _) = brute_distance(&g, &dense, p);
            for seed in [None, Some(rng.unit() * g.spline().max_t())] {
                let cp = g.closest_point(p, seed);
                // Never an overestimate; and never below the true minimum
                // either (the brute force is itself exact to its polish).
                assert!(
                    cp.distance <= truth + 1.0e-6,
                    "{label}: {} instead of {truth} at {p:?} (seed {seed:?})",
                    cp.distance
                );
                assert!(
                    cp.distance >= truth - 2.0e-6,
                    "{label}: {} < {truth}",
                    cp.distance
                );
                worst_over = worst_over.max(cp.distance - truth);
                // The reported point is the wall point at the reported parameter.
                assert!((cp.point.distance(p) - cp.distance).abs() < 1.0e-6);
                let c = g.spline().point(cp.t);
                assert!(
                    v(c.x, c.y, cp.point.z).distance(cp.point) < 2.0e-6,
                    "{label}"
                );
                assert!(cp.point.z >= c.z - 1.0e-6 && cp.point.z <= c.z + WALL_HEIGHT + 1.0e-6);
            }
        }
        assert!(worst_over <= 1.0e-6, "{label}: {worst_over}");
    }
}

/// The reviewer's scenario: balls of three sizes fired at and along the wall
/// from either side at 1–30 m/s (no gravity). No ball may end a tick more
/// than 0.1 mm inside the wall, and none may change sides of the wall while
/// within its extent.
#[test]
fn balls_never_pass_through_sloped_guide_walls() {
    let world = empty_world();
    let mut rng = Lcg(31337);
    for (label, f) in shapes() {
        let g = guide(f);
        let dense = dense(&g);
        let guides = [g];
        let geo = Geometry {
            world: &world,
            guides: &guides,
            wireforms: &[],
        };
        // Side of the plan-view curve at dense index `i` (sign of the cross
        // product with the local direction).
        let side_of = |i: usize, q: Vec3| {
            let (a, b) = (
                dense[i.saturating_sub(1)],
                dense[(i + 1).min(dense.len() - 1)],
            );
            ((b.x - a.x) * (q.y - a.y) - (b.y - a.y) * (q.x - a.x)).signum()
        };
        let (mut runs, mut worst_sink) = (0, 0.0f32);
        let mut trial = 0;
        while runs < 320 {
            trial += 1;
            assert!(trial < 5000, "{label}: cannot place the balls");
            let profile = [STEEL, MINI, JUMBO][trial % 3];
            let r = radius(profile);
            let s0 = 0.15 + 0.7 * rng.unit();
            let (c, c2) = (f(s0), f(s0 + 1.0e-3));
            let tan = v(c2.x - c.x, c2.y - c.y, 0.0).normalize();
            let nrm = Vec3::Z.cross(tan);
            let side = if rng.signed() > 0.0 { 1.0 } else { -1.0 };
            let start = c
                + nrm * side * (r + 0.0005 + 0.02 * rng.unit())
                + Vec3::Z * (0.18 + 0.02 * rng.signed());
            let speed = 1.0 + 29.0 * rng.unit();
            let mix = rng.signed();
            let dir = (tan * mix + nrm * -side * (1.0 - mix.abs()) + Vec3::Z * 0.05 * rng.signed())
                .normalize();
            let spin = v(rng.signed(), rng.signed(), rng.signed()) * 500.0;
            // Start clear of every part of the wall (tight curves come back).
            if brute_distance(&guides[0], &dense, start).0 < r + 2.0e-4 {
                continue;
            }
            runs += 1;
            let mut s = solver_no_gravity();
            let mut ball = ball_at(profile, start, dir * speed);
            ball.angular_velocity = spin;
            let h = s.add_ball(ball).unwrap();
            let mut ev = BallEvents::new();
            let mut last = (start, brute_distance(&guides[0], &dense, start).1);
            for tick in 0..40 {
                ev.clear();
                s.step(&geo, DT, &mut ev);
                let q = s.ball(h).unwrap().position;
                let (d, i) = brute_distance(&guides[0], &dense, q);
                let what = format!("{label} {profile:?} run {runs} tick {tick} speed {speed:.1}");
                assert!(q.is_finite(), "{what}");
                worst_sink = worst_sink.max(r - d);
                assert!(
                    r - d <= 1.0e-4,
                    "{what}: {} mm inside the wall",
                    (r - d) * 1e3
                );
                // Crossing: both ends of the tick's motion are beside the
                // same stretch of wall, within its height, away from its
                // ends, and on opposite sides.
                let beside = |p: Vec3, i: usize| {
                    i > 300
                        && i + 300 < dense.len()
                        && p.z > dense[i].z + r
                        && p.z < dense[i].z + WALL_HEIGHT - r
                };
                let near = last.1.abs_diff(i) < 600;
                if near && beside(last.0, last.1) && beside(q, i) {
                    assert!(
                        side_of(last.1, last.0) == side_of(i, q),
                        "{what}: passed through the wall from {:?} to {q:?}",
                        last.0
                    );
                }
                last = (q, i);
            }
        }
        assert!(worst_sink <= 1.0e-4, "{label}: {worst_sink}");
    }
}
