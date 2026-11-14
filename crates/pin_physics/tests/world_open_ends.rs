//! Subphase 2.1 fix task (2.2 final review, B7): open ends of a level guide
//! that face each other across a narrow opening.
//!
//! The reach estimate of a level guide must never exceed the true (Federer)
//! reach of its curve, in particular when the two open ends of one curve lie
//! behind each other: at an end the tangent cone is the half-line into the
//! curve, so the other end is at its full distance from it and the reach is
//! at most half the opening. An estimate above the true reach lets the
//! sphere cast take a step that is not a lower bound of the time to contact.
//!
//! The geometry is a "C ring": a circle of radius `r` through evenly spaced
//! points with an opening of a given chord centred on +X.

#[path = "ball_util.rs"]
mod util;

use glam::{DVec3, Vec3};
use pin_physics::ball::{BallEvents, Geometry};
use pin_physics::world::{CubicSpline, GuideCast, GuideCurve};

/// Deepest a free path may put the sphere into the wall: the sliding limit
/// of the cast (10 µm) plus f32 rounding of the closest-point query.
const MAX_SINK: f32 = 1.0e-5 + 1.0e-6;
const HEIGHT: f32 = 0.05;

fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

/// Through-points of a C ring: radius `r`, opening chord `opening` centred
/// on +X, points about `spacing` apart, at Z = 0.
fn c_ring_points(r: f32, opening: f32, spacing: f32) -> Vec<Vec3> {
    let half = f64::from(opening / (2.0 * r)).asin();
    let sweep = 2.0 * core::f64::consts::PI - 2.0 * half;
    let n = ((sweep * f64::from(r) / f64::from(spacing)).round() as usize).max(8);
    (0..=n)
        .map(|i| {
            let a = half + sweep * i as f64 / n as f64;
            v(
                (f64::from(r) * a.cos()) as f32,
                (f64::from(r) * a.sin()) as f32,
                0.0,
            )
        })
        .collect()
}

fn c_ring(r: f32, opening: f32, spacing: f32) -> GuideCurve {
    let pts = c_ring_points(r, opening, spacing);
    GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), HEIGHT).unwrap()
}

/// Through-point spacing used for a ring of radius `r`: 10 mm, coarser on
/// the large rings (well below the radius of curvature either way).
fn spacing_for(r: f32) -> f32 {
    (r / 10.0).max(0.01)
}

/// Centre of the opening of a C ring.
fn mouth(r: f32, opening: f32) -> Vec3 {
    v((r * r - 0.25 * opening * opening).sqrt(), 0.0, 0.0)
}

/// Federer's reach of an open curve by brute force over a dense sampling:
/// `inf |y − x|² / (2 dist(y − x, Tan(x)))`, the tangent cone being the
/// tangent line inside the curve and the half-line into it at an end.
/// (Over a finite sampling this is an upper bound of the true reach that
/// converges to it; the estimate under test must stay below it.)
fn brute_force_reach(spline: &CubicSpline, per_segment: usize) -> f64 {
    assert!(!spline.is_closed());
    let n = spline.segment_count() * per_segment;
    let max_t = f64::from(spline.max_t());
    let pts: Vec<(DVec3, DVec3)> = (0..=n)
        .map(|i| {
            let t = (max_t * i as f64 / n as f64) as f32;
            (
                spline.point(t).as_dvec3(),
                spline.tangent(t).as_dvec3().normalize(),
            )
        })
        .collect();
    let mut reach = f64::INFINITY;
    for (a, &(x, tangent)) in pts.iter().enumerate() {
        let inward = if a == 0 {
            Some(tangent)
        } else if a == n {
            Some(-tangent)
        } else {
            None
        };
        for (b, &(y, _)) in pts.iter().enumerate() {
            if a == b {
                continue;
            }
            let w = y - x;
            let w2 = w.length_squared();
            let off = match inward {
                Some(dir) if w.dot(dir) < 0.0 => w2.sqrt(),
                _ => (w - tangent * w.dot(tangent)).length(),
            };
            if off > 0.0 {
                reach = reach.min(w2 / (2.0 * off));
            }
        }
    }
    reach
}

/// (radius, opening) of every C ring of the sweeps, metres.
fn ring_cases() -> Vec<(f32, f32)> {
    let mut cases = Vec::new();
    for r in [0.04f32, 0.06, 0.1, 0.5] {
        for opening in [0.015f32, 0.022, 0.029, 0.03, 0.045, 0.06, 0.09, 0.12, 0.16] {
            // The opening is a chord: it cannot exceed the diameter, and a
            // C needs more than a half turn of wall to have facing ends.
            if opening <= 1.5 * r {
                cases.push((r, opening));
            }
        }
    }
    cases
}

/// C rings whose opening is narrower than a quarter of the through-point
/// spacing (radius, opening, spacing): the reach is shorter than the
/// coarsest tangent-ball table interval (2.1 fix task, review round 1).
const NARROW: [(f32, f32, f32); 9] = [
    // Gaps of a few percent of the spacing and less (review round 2).
    (0.5, 0.002, 0.05),
    (0.5, 0.0006, 0.05),
    (0.1, 0.0002, 0.01),
    (0.3, 0.02, 0.05),
    (0.5, 0.015, 0.05),
    (0.5, 0.02, 0.05),
    (0.5, 0.024, 0.05),
    (0.1, 0.009, 0.02),
    (0.5, 0.005, 0.05),
];

/// (radius, opening, spacing) of every ring of the sweeps.
fn all_cases() -> Vec<(f32, f32, f32)> {
    ring_cases()
        .into_iter()
        .map(|(r, opening)| (r, opening, spacing_for(r)))
        .chain(NARROW)
        .collect()
}

#[test]
fn reach_estimate_never_exceeds_the_true_reach_of_a_c_ring() {
    for (r, opening, spacing) in all_cases() {
        let g = c_ring(r, opening, spacing);
        let estimate = f64::from(g.reach());
        let truth = brute_force_reach(g.spline(), 24);
        // The two ends face each other: the reach is at most half the
        // opening, whatever the sampling (the ends are through-points).
        let ends = 0.5 * f64::from(opening);
        assert!(truth <= ends * (1.0 + 1e-6), "r {r} opening {opening}");
        assert!(
            estimate <= truth && estimate <= ends,
            "r {r} opening {opening}: estimate {estimate} above the true reach {truth} \
             (half opening {ends})"
        );
        // And not uselessly small: the 0.9 sampling margin, little more.
        assert!(
            estimate >= 0.8 * truth,
            "r {r} opening {opening}: estimate {estimate}, true {truth}"
        );
    }
}

#[test]
fn reach_of_guides_without_facing_ends_is_what_it_was() {
    // Nothing lies behind the ends of a straight wall or of an arc of up to
    // a half turn: there the half-line and the whole tangent line agree, and
    // the estimate is the arc's radius less the sampling margins.
    let straight: Vec<Vec3> = (0..=20).map(|i| v(0.05 * i as f32, 0.0, 0.0)).collect();
    let g = GuideCurve::new(CubicSpline::interpolate(&straight).unwrap(), 0.03).unwrap();
    assert!(g.reach() > 100.0, "{}", g.reach());
    for (radius, degrees) in [(0.15f32, 180usize), (0.5, 90), (0.03, 120)] {
        let pts: Vec<Vec3> = (0..=degrees / 5)
            .map(|i| {
                let a = (i as f32 * 5.0).to_radians();
                v(radius * libm::cosf(a), radius * libm::sinf(a), 0.0)
            })
            .collect();
        let g = GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), 0.03).unwrap();
        let truth = brute_force_reach(g.spline(), 24) as f32;
        assert!(
            g.reach() <= truth && g.reach() >= 0.8 * truth && g.reach() <= radius,
            "arc r {radius} {degrees}°: {} vs {truth}",
            g.reach()
        );
    }
}

/// Follows a cast to its end, re-casting after every `Advance` as a solver
/// would: the time up to which the path is declared free, and whether it
/// ends in a contact.
fn free_time(g: &GuideCurve, start: Vec3, r: f32, vel: Vec3, window: f32) -> (f32, bool) {
    let mut elapsed = 0.0f32;
    for _ in 0..100_000 {
        let from = start + vel * elapsed;
        match g.cast_sphere(from, r, vel, window - elapsed, None) {
            None => return (window, false),
            Some(GuideCast::Contact(hit)) => {
                assert!(hit.time >= 0.0 && hit.time <= window - elapsed);
                return (elapsed + hit.time, true);
            }
            Some(GuideCast::Advance { time }) => {
                assert!(time > 0.0 && time < window - elapsed, "no progress");
                elapsed += time;
            }
        }
    }
    panic!("cast did not finish");
}

/// Smallest gap (distance to the wall less `r`) of the sphere along
/// `start + vel·τ`, `τ ∈ [0, time]`: sampled at most `step` metres apart,
/// then refined around the lowest sample.
fn min_gap_along(g: &GuideCurve, start: Vec3, r: f32, vel: Vec3, time: f32, step: f32) -> f32 {
    let gap = |tau: f32| g.closest_point(start + vel * tau, None).distance - r;
    let n = ((vel.length() * time / step).ceil() as usize).clamp(1, 4000);
    let mut best = (gap(0.0), 0usize);
    for i in 1..=n {
        let value = gap(time * i as f32 / n as f32);
        if value < best.0 {
            best = (value, i);
        }
    }
    // Ternary search between the neighbours of the lowest sample.
    let mut lo = time * best.1.saturating_sub(1) as f32 / n as f32;
    let mut hi = time * (best.1 + 1).min(n) as f32 / n as f32;
    let mut lowest = best.0;
    for _ in 0..24 {
        let (a, b) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
        let (ga, gb) = (gap(a), gap(b));
        lowest = lowest.min(ga).min(gb);
        if ga < gb {
            hi = b;
        } else {
            lo = a;
        }
    }
    lowest
}

#[test]
fn deterministic_repro_of_b7_is_a_contact() {
    // 2.2 final review, B7: r = 40 mm, 30 mm opening, 10 mm spacing. The
    // ball sits in the opening, 1.5 mm from the far end, moving at it.
    let g = c_ring(0.04, 0.03, 0.01);
    assert!(g.is_level());
    assert!(g.reach() <= 0.015, "{}", g.reach());
    let r = 0.0135;
    let start = v(0.036_934_227, -4.089_251e-5, 0.025);
    let vel = v(-0.510_006, 6.707_447_5, 0.0);
    assert!(g.closest_point(start, None).distance - r > 1.0e-3);
    for window in [0.25e-3f32, 0.5e-3, 1.0e-3, 2.0e-3] {
        // The very first cast must not return a free path into the wall.
        let first = g.cast_sphere(start, r, vel, window, None);
        assert!(first.is_some(), "window {window}: free path into the wall");
        let (time, contact) = free_time(&g, start, r, vel, window);
        assert!(contact, "window {window}: no contact");
        let at = g.closest_point(start + vel * time, None).distance - r;
        assert!(at.abs() <= MAX_SINK, "window {window}: gap at contact {at}");
        let sink = min_gap_along(&g, start, r, vel, time, 2.0e-5);
        assert!(sink >= -MAX_SINK, "window {window}: swept gap {sink}");
    }
    // The second ring of the review: r = 100 mm, 29 mm opening.
    let g = c_ring(0.1, 0.029, 0.01);
    assert!(g.reach() <= 0.0145, "{}", g.reach());
    let start = mouth(0.1, 0.029) + v(0.0, 0.0, 0.025);
    for (k, window) in [0.25e-3f32, 0.5e-3, 1.0e-3, 2.0e-3].into_iter().enumerate() {
        let vel = v(-0.4 + 0.2 * k as f32, 6.7, 0.0);
        let (time, contact) = free_time(&g, start, r, vel, window);
        assert!(contact, "window {window}");
        let sink = min_gap_along(&g, start, r, vel, time, 2.0e-5);
        assert!(sink >= -MAX_SINK, "window {window}: swept gap {sink}");
    }
}

/// Small deterministic generator (PCG-XSH-RR 64/32) for the sweeps.
struct Pcg(u64);

impl Pcg {
    fn next_u32(&mut self) -> u32 {
        let old = self.0;
        self.0 = old
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let xorshifted = (((old >> 18) ^ old) >> 27) as u32;
        xorshifted.rotate_right((old >> 59) as u32)
    }

    /// Uniform in [0, 1).
    fn unit(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
    }

    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.unit()
    }
}

const BALL_RADII: [f32; 5] = [0.0079, 0.0111, 0.0135, 0.016, 0.0397];
const SPEEDS: [f32; 7] = [0.2, 0.7, 2.0, 5.0, 8.0, 15.0, 30.0];

#[test]
fn casts_through_c_ring_openings_never_sweep_into_the_wall() {
    let mut rng = Pcg(0x21b7_c0ff);
    let mut casts = 0u32;
    let mut contacts = 0u32;
    let mut deepest = 0.0f32;
    for (r, opening, spacing) in all_cases() {
        let g = c_ring(r, opening, spacing);
        let centre = mouth(r, opening);
        let ends = [g.spline().point(0.0), g.spline().point(g.spline().max_t())];
        for &ball in &BALL_RADII {
            for &speed in &SPEEDS {
                for _ in 0..6 {
                    // A start near the opening that does not touch the wall.
                    let spread = 0.5 * opening + 1.5 * ball;
                    let mut start = None;
                    for _ in 0..64 {
                        let p = centre
                            + v(
                                rng.range(-spread, spread),
                                rng.range(-spread, spread),
                                rng.range(-ball, HEIGHT + ball),
                            );
                        if g.closest_point(p, None).distance > ball + 1.0e-4 {
                            start = Some(p);
                            break;
                        }
                    }
                    let Some(start) = start else { continue };
                    // Aimed at one of the ends, the opening or anywhere.
                    let target = match rng.next_u32() % 4 {
                        0 => ends[0],
                        1 => ends[1],
                        2 => centre,
                        _ => start + v(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), 0.0),
                    } + v(
                        rng.range(-ball, ball),
                        rng.range(-ball, ball),
                        rng.range(0.0, HEIGHT),
                    );
                    let dir = (target - start).normalize_or_zero();
                    if dir == Vec3::ZERO {
                        continue;
                    }
                    let vel = dir * speed;
                    // Windows of 0.25–2 ms, and long ones for the slow balls.
                    let window = match rng.next_u32() % 5 {
                        0 => 0.25e-3,
                        1 => 0.5e-3,
                        2 => 1.0e-3,
                        3 => 2.0e-3,
                        _ => (2.0 * spread / speed).min(0.05),
                    };
                    // The first cast alone (what `cast_sphere` promises)...
                    let first = match g.cast_sphere(start, ball, vel, window, None) {
                        None => window,
                        Some(GuideCast::Contact(hit)) => hit.time,
                        Some(GuideCast::Advance { time }) => time,
                    };
                    let sink = min_gap_along(&g, start, ball, vel, first, 1.0e-4);
                    // ... and the path followed to its end.
                    let (time, contact) = free_time(&g, start, ball, vel, window);
                    let sink = sink.min(min_gap_along(&g, start, ball, vel, time, 1.0e-4));
                    casts += 1;
                    contacts += u32::from(contact);
                    deepest = deepest.min(sink);
                    assert!(
                        sink >= -MAX_SINK,
                        "ring r {r} opening {opening}, ball {ball}, start {start:?}, vel {vel:?}, \
                         window {window}: free path {} mm into the wall",
                        -sink * 1.0e3
                    );
                    if contact {
                        let at = g.closest_point(start + vel * time, None).distance - ball;
                        assert!(at.abs() <= MAX_SINK, "gap at contact {at}");
                    }
                }
            }
        }
    }
    // The sweep must exercise both outcomes.
    assert!(
        casts > 4000 && contacts > casts / 10 && contacts < casts,
        "{contacts}/{casts}"
    );
    assert!(deepest >= -MAX_SINK);
}

/// A C ring whose second end turns inward on a small arc and runs on
/// straight for a few points: that end points across the ring, with the
/// first end beside and behind it.
fn hook(r: f32, opening: f32, spacing: f32) -> GuideCurve {
    let mut pts = c_ring_points(r, opening, spacing);
    let end = pts[pts.len() - 1];
    let dir = (end - pts[pts.len() - 2]).normalize();
    let left = v(-dir.y, dir.x, 0.0);
    let bend = 3.0 * spacing;
    let centre = end + left * bend;
    let quarter = (core::f32::consts::FRAC_PI_2 * bend / spacing).round() as usize;
    for i in 1..=quarter {
        let a = core::f32::consts::FRAC_PI_2 * i as f32 / quarter as f32;
        pts.push(centre - left * bend * libm::cosf(a) + dir * bend * libm::sinf(a));
    }
    let tip = pts[pts.len() - 1];
    for i in 1..=3 {
        pts.push(tip + left * spacing * i as f32);
    }
    GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), HEIGHT).unwrap()
}

#[test]
fn casts_anywhere_around_rings_and_hooks_never_sweep_into_the_wall() {
    // Not only at the opening: along the wall, inside and outside, where a
    // ball wider than the reach relies on the tangent balls of the wall.
    let mut rng = Pcg(0x00b7_0002);
    let shapes = [
        (c_ring(0.04, 0.015, 0.01), 0.04f32),
        (c_ring(0.04, 0.03, 0.01), 0.04),
        (c_ring(0.1, 0.029, 0.01), 0.1),
        (c_ring(0.1, 0.06, 0.01), 0.1),
        (c_ring(0.5, 0.03, 0.05), 0.5),
        (c_ring(0.3, 0.02, 0.05), 0.3),
        (c_ring(0.5, 0.015, 0.05), 0.5),
        (c_ring(0.5, 0.02, 0.05), 0.5),
        (c_ring(0.5, 0.024, 0.05), 0.5),
        (c_ring(0.1, 0.009, 0.02), 0.1),
        (c_ring(0.5, 0.005, 0.05), 0.5),
        (hook(0.06, 0.03, 0.005), 0.06),
        (hook(0.1, 0.05, 0.008), 0.1),
        (hook(0.12, 0.04, 0.008), 0.12),
    ];
    let mut casts = 0u32;
    let mut contacts = 0u32;
    for (g, r) in &shapes {
        for &ball in &BALL_RADII {
            for &speed in &SPEEDS {
                for _ in 0..24 {
                    // A start within two ball radii of the wall, anywhere.
                    let t = rng.range(0.0, g.spline().max_t());
                    let on_wall = g.spline().point(t);
                    let away = v(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), 0.0);
                    let start = on_wall
                        + away.normalize_or_zero() * rng.range(1.0, 2.0) * ball
                        + Vec3::Z * rng.range(-0.5 * ball, HEIGHT + 0.5 * ball);
                    if g.closest_point(start, None).distance <= ball + 1.0e-5 {
                        continue;
                    }
                    // Mostly along the wall or into it.
                    let tangent = g.spline().tangent(t).normalize();
                    let aim = match rng.next_u32() % 3 {
                        0 => tangent * if rng.unit() < 0.5 { 1.0 } else { -1.0 },
                        1 => (on_wall - start).normalize(),
                        _ => v(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), 0.0),
                    } + v(
                        rng.range(-0.2, 0.2),
                        rng.range(-0.2, 0.2),
                        rng.range(-0.2, 0.2),
                    );
                    let dir = aim.normalize_or_zero();
                    if dir == Vec3::ZERO {
                        continue;
                    }
                    let vel = dir * speed;
                    let window = [
                        0.25e-3f32,
                        0.5e-3,
                        1.0e-3,
                        2.0e-3,
                        (0.5 * r / speed).min(0.05),
                    ][(rng.next_u32() % 5) as usize];
                    let (time, contact) = free_time(g, start, ball, vel, window);
                    let sink = min_gap_along(g, start, ball, vel, time, 1.0e-4);
                    casts += 1;
                    contacts += u32::from(contact);
                    assert!(
                        sink >= -MAX_SINK,
                        "shape r {r}, ball {ball}, start {start:?}, vel {vel:?}, window {window}: \
                         free path {} mm into the wall",
                        -sink * 1.0e3
                    );
                }
            }
        }
    }
    assert!(
        casts > 2500 && contacts > casts / 10 && contacts < casts,
        "{contacts}/{casts}"
    );
}

/// What one run of the test integrator saw.
struct Run {
    /// Lowest gap along any path a cast declared free.
    swept: f32,
    /// Lowest gap at the end of a sub-step, before the push-out.
    end: f32,
    /// Largest speed gain over one tick, relative.
    gain: f32,
    contacts: u32,
}

/// A minimal frictionless point-mass integrator (as in `guide_ramp`): 1 kHz
/// ticks, casts for approaching contacts, partly elastic bounce, and
/// closest-point push-out after every sub-step. No gravity: the ball moves
/// in the plane of the ring, at mid height of the wall.
fn integrate(g: &GuideCurve, ball: f32, mut pos: Vec3, mut vel: Vec3, ticks: usize) -> Run {
    const DT: f32 = 1.0e-3;
    const RESTITUTION: f32 = 0.6;
    let mut run = Run {
        swept: f32::INFINITY,
        end: f32::INFINITY,
        gain: 0.0,
        contacts: 0,
    };
    let mut seed = None;
    for _ in 0..ticks {
        let before = vel.length();
        let mut remaining = DT;
        for _ in 0..48 {
            if remaining <= 0.0 || vel.length_squared() == 0.0 {
                break;
            }
            let cast = g.cast_sphere(pos, ball, vel, remaining, seed);
            let (time, hit) = match cast {
                None => (remaining, None),
                Some(GuideCast::Advance { time }) => (time, None),
                Some(GuideCast::Contact(hit)) => (hit.time, Some(hit)),
            };
            if time > 0.0 {
                run.swept = run
                    .swept
                    .min(min_gap_along(g, pos, ball, vel, time, 2.5e-4));
            }
            pos += vel * time;
            remaining -= time;
            if let Some(hit) = hit {
                seed = Some(hit.t);
                run.contacts += 1;
                let vn = vel.dot(hit.normal);
                if vn < 0.0 {
                    vel -= hit.normal * (1.0 + RESTITUTION) * vn;
                }
            }
            // Every sub-step, as `cast_sphere` asks of its caller: project
            // out of penetration and stop the normal velocity (a sphere that
            // is leaving may sink 10 µm per cast along a concave wall).
            let cp = g.closest_point(pos, seed);
            seed = Some(cp.t);
            let gap = cp.distance - ball;
            run.end = run.end.min(gap);
            if gap < 0.0 && cp.distance > 0.0 {
                let normal = (pos - cp.point) / cp.distance;
                let vn = vel.dot(normal);
                if vn < 0.0 {
                    vel -= normal * vn;
                }
                pos -= normal * gap;
            }
        }
        if before > 0.0 {
            run.gain = run.gain.max(vel.length() / before - 1.0);
        }
    }
    run
}

#[test]
fn test_integrator_through_c_ring_openings_stays_out_of_the_wall() {
    let mut rng = Pcg(0x00b7_0001);
    let mut runs = 0u32;
    let mut contacts = 0u32;
    for (r, opening, spacing) in all_cases() {
        let g = c_ring(r, opening, spacing);
        let centre = mouth(r, opening);
        let ends = [g.spline().point(0.0), g.spline().point(g.spline().max_t())];
        for &ball in &BALL_RADII {
            for &speed in &SPEEDS {
                // From outside toward the opening, from inside out through
                // it, and across the opening at one of its ends.
                for case in 0..3 {
                    let reach = 0.5 * opening + ball + 0.02;
                    let (from, to) = match case {
                        0 => (
                            centre + v(reach, rng.range(-reach, reach), 0.0),
                            ends[(rng.next_u32() % 2) as usize],
                        ),
                        1 => (
                            centre * rng.range(0.0, 0.3),
                            ends[(rng.next_u32() % 2) as usize],
                        ),
                        _ => (
                            centre + v(rng.range(-0.3, 0.3) * ball, -reach, 0.0),
                            centre + v(0.0, reach, 0.0),
                        ),
                    };
                    let from = v(from.x, from.y, 0.5 * HEIGHT);
                    if g.closest_point(from, None).distance <= ball + 1.0e-4 {
                        continue;
                    }
                    let aim = v(to.x, to.y, 0.5 * HEIGHT)
                        + v(rng.range(-ball, ball), rng.range(-ball, ball), 0.0);
                    let vel = (aim - from).normalize() * speed;
                    // Long enough to cross the opening and bounce around it.
                    let ticks = ((4.0 * reach / speed / 1.0e-3) as usize).clamp(8, 400);
                    let run = integrate(&g, ball, from, vel, ticks);
                    runs += 1;
                    contacts += run.contacts;
                    let label = format!(
                        "ring r {r} opening {opening}, ball {ball}, from {from:?}, vel {vel:?}"
                    );
                    assert!(
                        run.swept >= -MAX_SINK,
                        "{label}: free path {} mm into the wall",
                        -run.swept * 1.0e3
                    );
                    // A sub-step ends where a free path ended: within the
                    // sliding limit as well.
                    assert!(
                        run.end >= -MAX_SINK,
                        "{label}: {} mm deep at the end of a sub-step",
                        -run.end * 1.0e3
                    );
                    assert!(run.gain <= 1.0e-5, "{label}: speed gain {}", run.gain);
                }
            }
        }
    }
    assert!(
        runs > 2000 && contacts > runs,
        "{contacts} contacts in {runs} runs"
    );
}

#[test]
fn sliding_along_a_c_ring_costs_what_it_costs_without_the_opening() {
    // The reach of a C is half its opening, less than a ball radius here,
    // but away from the ends a ball slides along the wall with the tangent
    // balls of the ring itself: a few closest-point evaluations per 1 ms
    // cast, not one per 10 µm of travel.
    let ball = 0.0135f32;
    let wide = [
        (0.04f32, 0.03f32, 0.01f32),
        (0.1, 0.029, 0.01),
        (0.1, 0.06, 0.01),
        (0.5, 0.03, 0.05),
    ];
    for (r, opening, spacing) in wide.into_iter().chain(NARROW) {
        let g = c_ring(r, opening, spacing);
        assert!(g.reach() <= 0.5 * opening);
        let mut steps = 0u32;
        let mut casts = 0u32;
        for k in 0..64 {
            // Away from the opening (it spans less than ±0.4 rad).
            let a = 0.7 + 4.9 * k as f32 / 64.0;
            let radial = v(libm::cosf(a), libm::sinf(a), 0.0);
            let tangent = v(-radial.y, radial.x, 0.0);
            for (side, gap, inward) in [
                (-1.0f32, 0.0f32, 0.0f32),
                (-1.0, 1.0e-3, 0.0),
                (-1.0, 5.0e-3, 0.5),
                (1.0, 0.0, 0.0),
                (1.0, 1.0e-3, 0.0),
                (1.0, 5.0e-3, -0.5),
            ] {
                // At exactly `gap` from the wall (the spline through the
                // ring's points is not quite a circle).
                let near = radial * (r + side * (ball + gap)) + Vec3::Z * 0.02;
                let cp = g.closest_point(near, None);
                let start = cp.point + (near - cp.point) / cp.distance * (ball + gap);
                let offset = side * gap;
                let vel = (tangent + radial * inward).normalize() * 8.0;
                let (out, used) = g.cast_sphere_counted(start, ball, vel, 1.0e-3, None);
                assert!(
                    !matches!(out, Some(GuideCast::Advance { .. })) && used <= 8,
                    "ring r {r} opening {opening}, angle {a}, offset {offset}: {out:?} after \
                     {used} steps"
                );
                steps += used;
                casts += 1;
                let time = match out {
                    Some(GuideCast::Contact(hit)) => hit.time,
                    _ => 1.0e-3,
                };
                let sink = min_gap_along(&g, start, ball, vel, time, 1.0e-4);
                assert!(sink >= -MAX_SINK, "ring r {r} opening {opening}: {sink}");
            }
        }
        assert!(
            steps <= 3 * casts,
            "ring r {r} opening {opening}: {steps} steps in {casts} casts"
        );
    }
}

#[test]
fn ball_solver_at_c_ring_openings_resolves_contacts_on_time() {
    // The 2.2 ball solver on the review's rings: before the fix a cast could
    // call a path into the far end free, and the contact was resolved one
    // sub-step late, up to 2.5 mm deep at the end of a tick.
    let world = util::empty_world();
    let mut rng = Pcg(0x00b7_0003);
    let mut deepest = 0.0f32;
    let mut runs = 0u32;
    for (r, opening) in [
        (0.04f32, 0.012f32),
        (0.04, 0.02),
        (0.04, 0.024),
        (0.04, 0.026),
        (0.04, 0.03),
        (0.06, 0.026),
        (0.06, 0.03),
        (0.1, 0.026),
        (0.1, 0.029),
        (0.1, 0.031),
        (0.1, 0.06),
    ] {
        let guides = [c_ring(r, opening, 0.01)];
        let geo = Geometry {
            world: &world,
            guides: &guides,
            wireforms: &[],
        };
        let centre = mouth(r, opening);
        for profile in [util::STEEL, util::MINI, util::JUMBO] {
            let ball = util::radius(profile);
            for &speed in &[0.5f32, 2.0, 5.0, 6.7, 8.0, 15.0] {
                for _ in 0..12 {
                    // In or just outside the opening, aimed across it or at
                    // one of its ends.
                    let from = centre
                        + v(
                            rng.range(-0.2, 1.5) * ball,
                            rng.range(-1.0, 1.0) * (0.5 * opening + ball),
                            0.5 * HEIGHT,
                        );
                    if guides[0].closest_point(from, None).distance <= ball + 1.0e-4 {
                        continue;
                    }
                    let to = centre
                        + v(
                            rng.range(-1.0, 0.3) * ball,
                            rng.range(-1.0, 1.0) * (0.5 * opening + ball),
                            0.5 * HEIGHT,
                        );
                    let dir = (to - from).normalize_or_zero();
                    if dir == Vec3::ZERO {
                        continue;
                    }
                    let mut solver = util::solver_no_gravity();
                    let handle = solver
                        .add_ball(util::ball_at(profile, from, dir * speed))
                        .unwrap();
                    let mut events = BallEvents::new();
                    let mut energy = util::kinetic_energy(&solver);
                    runs += 1;
                    for tick in 0..40 {
                        solver.step(&geo, util::DT, &mut events);
                        let pos = solver.ball(handle).unwrap().position;
                        let gap = guides[0].closest_point(pos, None).distance - ball;
                        deepest = deepest.min(gap);
                        assert!(
                            gap >= -1.0e-4,
                            "ring r {r} opening {opening}, ball {ball}, from {from:?}, speed \
                             {speed}, tick {tick}: {} mm deep at the end of a tick",
                            -gap * 1.0e3
                        );
                        let now = util::kinetic_energy(&solver);
                        assert!(
                            now <= energy * (1.0 + 1.0e-6),
                            "energy gain {energy} -> {now}"
                        );
                        energy = now;
                    }
                }
            }
        }
    }
    assert!(runs > 800, "{runs}");
    println!(
        "deepest tick-end gap {:.1} um in {runs} runs",
        deepest * 1.0e6
    );
}

#[test]
fn ball_solver_slides_along_narrow_c_rings_at_full_speed() {
    // Far from the mouth a ball sliding along the inside of a C moves the
    // distance its speed says, every tick, however narrow the opening
    // (before the tables adapted to the reach it crawled at about 15 %).
    let world = util::empty_world();
    let ball = util::radius(util::STEEL);
    let wide = [(0.04f32, 0.03f32, 0.01f32), (0.5, 0.03, 0.05)];
    for (r, opening, spacing) in wide.into_iter().chain(NARROW) {
        let guides = [c_ring(r, opening, spacing)];
        let geo = Geometry {
            world: &world,
            guides: &guides,
            wireforms: &[],
        };
        for speed in [1.0f32, 5.0, 8.0] {
            let a = 0.9f32;
            let radial = v(libm::cosf(a), libm::sinf(a), 0.0);
            let near = radial * (r - ball - 1.0e-4) + Vec3::Z * 0.5 * HEIGHT;
            let cp = guides[0].closest_point(near, None);
            let start = cp.point + (near - cp.point) / cp.distance * (ball + 1.0e-4);
            let mut solver = util::solver_no_gravity();
            let handle = solver
                .add_ball(util::ball_at(
                    util::STEEL,
                    start,
                    v(-radial.y, radial.x, 0.0) * speed,
                ))
                .unwrap();
            let mut events = BallEvents::new();
            let (mut ticks, mut path, mut expected) = (0u32, 0.0f32, 0.0f32);
            for tick in 0..300 {
                let before = *solver.ball(handle).unwrap();
                solver.step(&geo, util::DT, &mut events);
                let after = solver.ball(handle).unwrap();
                // Stop before the mouth (it is on +X).
                let angle = libm::atan2f(after.position.y, after.position.x);
                if angle.abs() < 0.6 {
                    break;
                }
                let moved = after.position.distance(before.position);
                let slower = before.velocity.length().min(after.velocity.length());
                assert!(
                    moved >= 0.95 * slower * util::DT,
                    "ring r {r} opening {opening} spacing {spacing}, {speed} m/s, tick {tick}: \
                     moved {moved} m at {slower} m/s"
                );
                let gap = guides[0].closest_point(after.position, None).distance - ball;
                assert!(gap >= -1.0e-4, "{} mm deep", -gap * 1.0e3);
                ticks += 1;
                path += moved;
                expected += slower * util::DT;
            }
            assert!(
                ticks >= 20 && path >= 0.95 * expected,
                "ring r {r} opening {opening}: {path} m of {expected} m in {ticks} ticks"
            );
        }
    }
}

#[test]
fn hairline_gaps_are_accepted_safe_and_no_obstacle() {
    // However close the two open ends of a level guide come, the guide is
    // built, a ball sliding along it passes the gap at speed (the gap is far
    // too narrow to enter), and never sinks into the wall.
    let world = util::empty_world();
    let ball = util::radius(util::STEEL);
    for (r, opening, spacing) in [
        (0.5f32, 0.002f32, 0.05f32),
        (0.5, 0.0006, 0.05),
        (0.5, 0.00005, 0.05),
        (0.1, 0.0002, 0.01),
        (0.1, 0.00002, 0.01),
    ] {
        let guides = [c_ring(r, opening, spacing)];
        assert!(guides[0].reach() <= 0.5 * opening && guides[0].reach() > 0.0);
        let geo = Geometry {
            world: &world,
            guides: &guides,
            wireforms: &[],
        };
        for speed in [1.0f32, 5.0] {
            // Start 60 mm of arc before the gap, inside the ring.
            let a = -0.06 / r;
            let radial = v(libm::cosf(a), libm::sinf(a), 0.0);
            let near = radial * (r - ball - 1.0e-4) + Vec3::Z * 0.5 * HEIGHT;
            let cp = guides[0].closest_point(near, None);
            let start = cp.point + (near - cp.point) / cp.distance * (ball + 1.0e-4);
            let mut solver = util::solver_no_gravity();
            let handle = solver
                .add_ball(util::ball_at(
                    util::STEEL,
                    start,
                    v(-radial.y, radial.x, 0.0) * speed,
                ))
                .unwrap();
            let mut events = BallEvents::new();
            let ticks = (0.12 / speed / util::DT) as usize;
            let (mut path, mut expected) = (0.0f32, 0.0f32);
            for tick in 0..ticks {
                let before = *solver.ball(handle).unwrap();
                solver.step(&geo, util::DT, &mut events);
                let after = solver.ball(handle).unwrap();
                path += after.position.distance(before.position);
                expected += before.velocity.length().min(after.velocity.length()) * util::DT;
                let gap = guides[0].closest_point(after.position, None).distance - ball;
                assert!(
                    gap >= -1.0e-4,
                    "ring r {r} gap {opening}, {speed} m/s, tick {tick}: {} mm deep",
                    -gap * 1.0e3
                );
            }
            let end = solver.ball(handle).unwrap().position;
            assert!(
                end.y > 0.03 && path >= 0.9 * expected,
                "ring r {r} gap {opening}, {speed} m/s: ended at {end:?} after {path} m of \
                 {expected} m"
            );
        }
    }
}

/// Through-points of the J of the 2.1 B7 re-review (round 3), `s` apart: a
/// straight base along +X from x = −0.6 m (shifted by `shift · s`), a
/// U-turn of radius 0.3 m, a 0.6 m return leg, a quarter turn of radius
/// 0.25 m (plus `tilt` radians) heading down, and a straight stub ending
/// `gap` above the base. The end points at the base, between two of its
/// through-points.
fn j_points(gap: f64, s: f64, tilt: f64, shift: f64) -> Vec<Vec3> {
    struct Path {
        pts: Vec<(f64, f64)>,
        x: f64,
        y: f64,
        heading: f64,
        s: f64,
    }
    impl Path {
        fn line(&mut self, length: f64) {
            let n = ((length / self.s).round() as usize).max(1);
            for _ in 0..n {
                self.x += length / n as f64 * self.heading.cos();
                self.y += length / n as f64 * self.heading.sin();
                self.pts.push((self.x, self.y));
            }
        }
        fn arc(&mut self, radius: f64, turn: f64) {
            let n = ((radius * turn.abs() / self.s).round() as usize).max(2);
            let sign = turn.signum();
            let cx = self.x - sign * radius * self.heading.sin();
            let cy = self.y + sign * radius * self.heading.cos();
            let a0 = self.heading - sign * core::f64::consts::FRAC_PI_2;
            for i in 1..=n {
                let a = a0 + turn * i as f64 / n as f64;
                self.x = cx + radius * a.cos();
                self.y = cy + radius * a.sin();
                self.pts.push((self.x, self.y));
            }
            self.heading += turn;
        }
    }
    let x0 = -0.6 + shift * s;
    let mut p = Path {
        pts: vec![(x0, 0.0)],
        x: x0,
        y: 0.0,
        heading: 0.0,
        s,
    };
    p.line(1.2);
    p.arc(0.3, core::f64::consts::PI);
    p.line(0.6);
    p.arc(0.25, core::f64::consts::FRAC_PI_2 + tilt);
    let length = (p.y - gap) / p.heading.sin().abs().max(0.2);
    p.line(length);
    p.pts
        .iter()
        .map(|&(x, y)| v(x as f32, y as f32, 0.0))
        .collect()
}

/// The wall of a level guide as a dense polyline (1024 points per segment,
/// evaluated in `f64`; chord error below 0.1 µm here), for distances that
/// do not depend on the closest-point search under test.
struct Oracle {
    pts: Vec<DVec3>,
}

impl Oracle {
    fn new(spline: &CubicSpline) -> Self {
        let per = 1024;
        let pts = spline
            .segments()
            .iter()
            .flat_map(|seg| {
                let p = seg.p.map(|q| q.as_dvec3());
                (0..per).map(move |j| {
                    let s = j as f64 / per as f64;
                    let a = 1.0 - s;
                    p[0] * (a * a * a)
                        + p[1] * (3.0 * a * a * s)
                        + p[2] * (3.0 * a * s * s)
                        + p[3] * (s * s * s)
                })
            })
            .chain(core::iter::once(
                spline.segments()[spline.segment_count() - 1].p[3].as_dvec3(),
            ))
            .collect();
        Self { pts }
    }

    /// The polyline pieces within `margin` (in plan) of the segment `a b`.
    fn near(&self, a: DVec3, b: DVec3, margin: f64) -> Vec<(DVec3, DVec3)> {
        let ab = DVec3::new(b.x - a.x, b.y - a.y, 0.0);
        let l2 = ab.length_squared();
        self.pts
            .windows(2)
            .filter(|w| {
                let ap = DVec3::new(w[0].x - a.x, w[0].y - a.y, 0.0);
                let t = if l2 > 0.0 {
                    (ap.dot(ab) / l2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                (ap - ab * t).length() <= margin
            })
            .map(|w| (w[0], w[1]))
            .collect()
    }

    /// Distance from `c` to the wall (the pieces × `[z, z + HEIGHT]`).
    fn distance(pieces: &[(DVec3, DVec3)], c: DVec3) -> f64 {
        pieces
            .iter()
            .map(|&(a, b)| {
                let ab = DVec3::new(b.x - a.x, b.y - a.y, 0.0);
                let ac = DVec3::new(c.x - a.x, c.y - a.y, 0.0);
                let l2 = ab.length_squared();
                let t = if l2 > 0.0 {
                    (ac.dot(ab) / l2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let h = (ac - ab * t).length();
                let top = a.z + f64::from(HEIGHT);
                let z = if c.z > top {
                    c.z - top
                } else if c.z < a.z {
                    a.z - c.z
                } else {
                    0.0
                };
                (h * h + z * z).sqrt()
            })
            .fold(f64::INFINITY, f64::min)
    }

    /// Smallest gap of the sphere along `start + vel·τ`, `τ ∈ [0, time]`,
    /// sampled every 4 µm of travel (the gap is 1-Lipschitz: at most 2 µm
    /// missed between samples).
    fn swept_gap(&self, start: Vec3, r: f32, vel: Vec3, time: f32) -> f64 {
        let (s, v) = (start.as_dvec3(), vel.as_dvec3());
        let end = s + v * f64::from(time);
        let pieces = self.near(s, end, f64::from(r) + 0.01);
        let n = (((end - s).length() / 4.0e-6).ceil() as usize).clamp(1, 20_000);
        (0..=n)
            .map(|i| {
                let c = s + v * (f64::from(time) * i as f64 / n as f64);
                Self::distance(&pieces, c) - f64::from(r)
            })
            .fold(f64::INFINITY, f64::min)
    }
}

/// Follows the casts of a sphere over `window` and returns the swept gap
/// up to the contact (or the end of the window), by the oracle.
fn cast_and_sweep(
    g: &GuideCurve,
    oracle: &Oracle,
    start: Vec3,
    r: f32,
    vel: Vec3,
    window: f32,
) -> f64 {
    let (time, _) = free_time(g, start, r, vel, window);
    oracle.swept_gap(start, r, vel, time)
}

#[test]
fn j_tips_pointing_at_the_wall_never_let_a_cast_sweep_into_it() {
    // 2.1 B7 re-review (round 3): the end of a J points at its own base,
    // 1 mm to 20 µm away, between two through-points 200 mm apart. With
    // the base seen only at sample points, the reach (9.4 mm for a 0.2 mm
    // gap) and the tangent balls near the tip were far too large, and a
    // 7.9 mm ball swept up to 0.39 mm into the base within one cast.
    let ball = 0.0079f32;
    for (gap, tilt, start, vel) in [
        (
            0.001,
            0.0,
            v(-0.235_695_42, 0.022_012_8, 0.025),
            v(-7.988_186_4, -18.335_455, 0.0),
        ),
        (
            0.0002,
            -0.4,
            v(-0.407_257_68, 0.008_014_502, 0.025),
            v(14.608_336, -13.660_032, 0.0),
        ),
        (
            0.0002,
            0.4,
            v(-0.138_100_34, 0.010_537_622_5, 0.025),
            v(9.893_417, -17.381_609, 0.0),
        ),
    ] {
        let spline = CubicSpline::interpolate(&j_points(gap, 0.2, tilt, 0.0)).unwrap();
        let oracle = Oracle::new(&spline);
        let g = GuideCurve::new(spline, HEIGHT).unwrap();
        for window in [1.0e-3f32, 2.0e-3] {
            let sink = cast_and_sweep(&g, &oracle, start, ball, vel, window);
            assert!(
                sink >= -f64::from(MAX_SINK),
                "J gap {gap} tilt {tilt}, window {window}: swept {} µm into the wall",
                -sink * 1.0e6
            );
        }
    }
    // Around the tip, every direction, real balls, a fixed pseudo-random
    // sweep (the J with 0.1 m and 0.2 m spacing, tips 20 µm and 0.2 mm
    // above the base, end leg tilted by ±0.4 rad).
    let mut rng = Pcg(0x00b7_0007);
    for (s, gap, tilt) in [
        (0.2f64, 2.0e-5f64, -0.4f64),
        (0.2, 2.0e-4, 0.4),
        (0.1, 2.0e-5, 0.0),
        (0.2, 2.0e-4, 0.0),
    ] {
        let spline = CubicSpline::interpolate(&j_points(gap, s, tilt, 0.0)).unwrap();
        let tip = spline.point(spline.max_t());
        let oracle = Oracle::new(&spline);
        let g = GuideCurve::new(spline, HEIGHT).unwrap();
        for k in 0..60 {
            let r = [0.0079f32, 0.0135, 0.0254][k % 3];
            let start = tip + v(rng.range(-3.0, 3.0) * r, rng.range(0.0, 3.0) * r, 0.025);
            if g.closest_point(start, None).distance <= r + 2.0e-5 {
                continue;
            }
            let a = rng.range(0.0, core::f32::consts::TAU);
            let vel = v(libm::cosf(a), libm::sinf(a), 0.0) * [0.5f32, 3.0, 8.0, 20.0][k % 4];
            let sink = cast_and_sweep(&g, &oracle, start, r, vel, 2.0e-3);
            assert!(
                sink >= -f64::from(MAX_SINK),
                "J s {s} gap {gap} tilt {tilt}: ball {r} from {start:?} at {vel:?} swept {} µm \
                 into the wall",
                -sink * 1.0e6
            );
        }
    }
}

#[test]
fn reach_of_an_end_pointing_at_the_wall_between_samples_is_bounded_by_its_gap() {
    // 2.1 B7 re-review (round 3), blocker 2: the half-line reach compared
    // the end with every fourth sample only; with through-points 200 mm
    // apart and the end 0.2 mm above the base between two of them, it came
    // out at 9.4 mm (the true reach is at most half the gap). Federer's
    // reach is at most half the distance from an end to any curve point
    // behind it, whatever the sampling.
    for s in [0.05f64, 0.1, 0.2] {
        for gap in [2.0e-5f64, 2.0e-4, 1.0e-3] {
            for tilt in [-0.4f64, 0.0, 0.4] {
                for shift in [0.0f64, 0.37] {
                    let spline = CubicSpline::interpolate(&j_points(gap, s, tilt, shift)).unwrap();
                    let oracle = Oracle::new(&spline);
                    let t = spline.max_t();
                    let tip = spline.point(t).as_dvec3();
                    let inward = -spline.tangent(t).as_dvec3().normalize();
                    // Distance from the tip to the base behind it.
                    let base: Vec<(DVec3, DVec3)> = oracle
                        .near(tip, tip, 0.05)
                        .into_iter()
                        .filter(|&(a, b)| {
                            (a - tip).dot(inward) < 0.0 && (b - tip).dot(inward) < 0.0
                        })
                        .collect();
                    let distance = Oracle::distance(&base, tip + DVec3::Z * 0.01);
                    // (The spline's end bends a little: not exactly `gap`.)
                    assert!(distance < gap + 1.0e-3, "s {s} gap {gap}: {distance}");
                    let g = GuideCurve::new(spline, HEIGHT).unwrap();
                    assert!(
                        f64::from(g.reach()) <= 0.5 * distance,
                        "s {s} gap {gap} tilt {tilt} shift {shift}: reach {} mm, tip {} mm \
                         from the wall",
                        g.reach() * 1.0e3,
                        distance * 1.0e3
                    );
                }
            }
        }
    }
}

/// Cost report (not a pass/fail test; run with `--release --ignored
/// --nocapture`): closest-point evaluations and time per level-guide cast.
#[test]
#[ignore = "cost report; prints, asserts nothing about time"]
fn level_guide_cast_cost_report() {
    use std::time::Instant;
    let ball = 0.0135f32;
    // A level arc as in `guide_ramp`: r = 150 mm, 180°, 5° spacing.
    let arc_pts: Vec<Vec3> = (0..=36)
        .map(|i| {
            let a = (i as f32 * 5.0).to_radians();
            v(0.15 * libm::cosf(a), 0.15 * libm::sinf(a), 0.0)
        })
        .collect();
    let arc = GuideCurve::new(CubicSpline::interpolate(&arc_pts).unwrap(), 0.03).unwrap();
    let rings = [
        ("C ring r40/30mm", c_ring(0.04, 0.03, 0.01), 0.04f32),
        ("C ring r100/29mm", c_ring(0.1, 0.029, 0.01), 0.1),
        ("C ring r100/60mm", c_ring(0.1, 0.06, 0.01), 0.1),
        ("C ring r500/160mm", c_ring(0.5, 0.16, 0.05), 0.5),
        ("C ring r500/30mm s50", c_ring(0.5, 0.03, 0.05), 0.5),
        ("C ring r300/20mm s50", c_ring(0.3, 0.02, 0.05), 0.3),
        ("C ring r500/15mm s50", c_ring(0.5, 0.015, 0.05), 0.5),
        ("C ring r500/20mm s50", c_ring(0.5, 0.02, 0.05), 0.5),
        ("C ring r500/24mm s50", c_ring(0.5, 0.024, 0.05), 0.5),
        ("C ring r100/9mm s20", c_ring(0.1, 0.009, 0.02), 0.1),
        ("C ring r500/5mm s50", c_ring(0.5, 0.005, 0.05), 0.5),
        ("C ring r500/0.6mm s50", c_ring(0.5, 0.0006, 0.05), 0.5),
    ];
    let mut shapes: Vec<(&str, &GuideCurve, f32)> = vec![("level arc r150 180°", &arc, 0.15)];
    for (name, g, r) in &rings {
        shapes.push((name, g, *r));
    }
    println!("| shape | reach mm | case | casts | steps/cast | ns/cast |");
    for (name, g, r) in shapes {
        for (case, gap, inward) in [
            ("sliding inside, 8 m/s", 0.0f32, 0.0f32),
            ("alongside 1 mm off, 8 m/s", 1.0e-3, 0.0),
            ("approaching at 30°, 8 m/s", 5.0e-3, 0.5),
        ] {
            let mut casts = Vec::new();
            for k in 0..64 {
                // Angles over the wall, away from the opening / the ends.
                let a = 0.6 + 2.0 * k as f32 / 64.0;
                let radial = v(libm::cosf(a), libm::sinf(a), 0.0);
                let tangent = v(-radial.y, radial.x, 0.0);
                let start = radial * (r - ball - gap) + Vec3::Z * 0.0135;
                let vel = (tangent + radial * inward).normalize() * 8.0;
                casts.push((start, vel));
            }
            let mut steps = 0u32;
            for &(start, vel) in &casts {
                steps += g.cast_sphere_counted(start, ball, vel, 1.0e-3, None).1;
            }
            let rounds = 200;
            let begin = Instant::now();
            let mut sum = 0.0f32;
            for _ in 0..rounds {
                for &(start, vel) in &casts {
                    let out = g.cast_sphere(
                        std::hint::black_box(start),
                        ball,
                        std::hint::black_box(vel),
                        1.0e-3,
                        None,
                    );
                    sum += match out {
                        Some(GuideCast::Contact(hit)) => hit.time,
                        Some(GuideCast::Advance { time }) => time,
                        None => 1.0,
                    };
                }
            }
            let ns = begin.elapsed().as_nanos() as f64 / (rounds * casts.len()) as f64;
            std::hint::black_box(sum);
            println!(
                "| {name} | {:.2} | {case} | {} | {:.2} | {ns:.0} |",
                g.reach() * 1.0e3,
                casts.len(),
                f64::from(steps) / casts.len() as f64
            );
        }
    }
    // Through the opening of the review's ring: the casts of the repro.
    for (name, g, r, opening) in [
        ("C ring r40/30mm", &rings[0].1, 0.04f32, 0.03f32),
        ("C ring r100/29mm", &rings[1].1, 0.1, 0.029),
        ("C ring r100/60mm", &rings[2].1, 0.1, 0.06),
        ("C ring r500/30mm s50", &rings[4].1, 0.5, 0.03),
    ] {
        let centre = mouth(r, opening) + Vec3::Z * 0.025;
        let mut casts = Vec::new();
        for k in 0..64 {
            let a = core::f32::consts::TAU * k as f32 / 64.0;
            casts.push((centre, v(libm::cosf(a), libm::sinf(a), 0.0) * 6.7));
        }
        let mut steps = 0u32;
        for &(start, vel) in &casts {
            steps += g.cast_sphere_counted(start, ball, vel, 1.0e-3, None).1;
        }
        let rounds = 200;
        let begin = Instant::now();
        for _ in 0..rounds {
            for &(start, vel) in &casts {
                std::hint::black_box(g.cast_sphere(
                    std::hint::black_box(start),
                    ball,
                    vel,
                    1.0e-3,
                    None,
                ));
            }
        }
        let ns = begin.elapsed().as_nanos() as f64 / (rounds * casts.len()) as f64;
        println!(
            "| {name} | {:.2} | from the opening, all directions, 6.7 m/s | {} | {:.2} | {ns:.0} |",
            g.reach() * 1.0e3,
            casts.len(),
            f64::from(steps) / casts.len() as f64
        );
    }
}

// ---------------------------------------------------------------------------
// 2.1 fix task B1: ball-wide lanes of end-limited level guides.

/// Through-points of a level Archimedean spiral `r(θ) = r0 + pitch · θ/2π`
/// over `turns` turns, `n` points evenly in angle.
fn spiral_points(r0: f64, pitch: f64, turns: f64, n: usize) -> Vec<Vec3> {
    (0..n)
        .map(|i| {
            let u = i as f64 / (n - 1) as f64;
            let a = core::f64::consts::TAU * turns * u;
            let r = r0 + pitch * turns * u;
            v((r * a.cos()) as f32, (r * a.sin()) as f32, 0.0)
        })
        .collect()
}

/// A ball of `profile` started in the middle turn of the spiral, its
/// surface 1 mm inside the outer wall of the lane, moving along the lane at
/// 5 m/s without gravity for 200 ticks: (ticks at the wall, ticks among
/// them below 95 % of speed · dt, worst fraction, deepest gap in metres).
fn slide_in_spiral_lane(
    pitch: f64,
    turns: f64,
    n: usize,
    profile: pin_core::ids::ProfileId,
) -> (u32, u32, f32, f32) {
    let world = util::empty_world();
    let guides = [GuideCurve::new(
        CubicSpline::interpolate(&spiral_points(0.1, pitch, turns, n)).unwrap(),
        HEIGHT,
    )
    .unwrap()];
    let geo = Geometry {
        world: &world,
        guides: &guides,
        wireforms: &[],
    };
    let ball = util::radius(profile);
    let turn = 0.5 * turns;
    let a = core::f64::consts::TAU * turn;
    let wall = (0.1 + pitch * turn) as f32;
    let radial = v(a.cos() as f32, a.sin() as f32, 0.0);
    let start = radial * (wall - ball - 1.0e-3) + Vec3::Z * 0.0135;
    let mut solver = util::solver_no_gravity();
    let handle = solver
        .add_ball(util::ball_at(
            profile,
            start,
            v(-radial.y, radial.x, 0.0) * 5.0,
        ))
        .unwrap();
    let mut events = BallEvents::new();
    let (mut at_wall, mut slow, mut worst, mut deepest) = (0u32, 0u32, f32::INFINITY, 0.0f32);
    for _ in 0..200 {
        let before = *solver.ball(handle).unwrap();
        solver.step(&geo, util::DT, &mut events);
        let after = solver.ball(handle).unwrap();
        let moved = after.position.distance(before.position);
        let speed = 0.5 * (before.velocity.length() + after.velocity.length());
        let fraction = moved / (speed * util::DT);
        let gap = guides[0].closest_point(after.position, None).distance - ball;
        deepest = deepest.min(gap);
        if gap < 1.0e-4 {
            at_wall += 1;
            worst = worst.min(fraction);
            if fraction < 0.95 {
                slow += 1;
            }
        }
    }
    (at_wall, slow, worst, deepest)
}

#[test]
fn a_steel_ball_slides_along_a_ball_wide_spiral_lane_at_full_speed() {
    // Review B1 of the other-model attempt: in a lane 40 mm wide (about
    // three steel-ball radii) between the turns of an end-limited level
    // spiral the ball crawled at ~5 % of speed · dt on 185 of 191 ticks at
    // the wall (reach and tangent balls both below the ball's radius).
    let (at_wall, slow, worst, deepest) = slide_in_spiral_lane(0.04, 3.0, 101, util::STEEL);
    assert!(at_wall >= 150, "only {at_wall} ticks at the wall");
    assert!(
        slow * 100 <= at_wall,
        "{slow} of {at_wall} ticks at the wall below 95 % of speed · dt (worst {worst})"
    );
    assert!(deepest >= -1.0e-5, "{} µm into the wall", -deepest * 1.0e6);
}

#[test]
fn balls_slide_along_spiral_lanes_of_34_to_44_mm_at_full_speed() {
    // The band of lanes that hold a pinball snugly: steel (27 mm), mini
    // (15.9 mm) and jumbo (31.75 mm) balls, spirals of three and four turns
    // (100 and 200 segments). At least 95 % of speed · dt on at least 99 %
    // of the ticks at the wall, never deeper than the 10 µm sliding limit.
    let mut report = Vec::new();
    for pitch in [0.034f64, 0.038, 0.044] {
        for (turns, n) in [(3.0f64, 101usize), (4.0, 201)] {
            for profile in [util::STEEL, util::MINI, util::JUMBO] {
                let (at_wall, slow, worst, deepest) =
                    slide_in_spiral_lane(pitch, turns, n, profile);
                let label = format!(
                    "pitch {} mm, {turns} turns, ball {} mm: {slow} of {at_wall} slow \
                     (worst {worst:.3}), deepest {:.2} µm",
                    pitch * 1.0e3,
                    util::radius(profile) * 1.0e3,
                    -deepest * 1.0e6
                );
                assert!(at_wall >= 150, "{label}");
                assert!(slow * 100 <= at_wall, "{label}");
                assert!(deepest >= -1.0e-5, "{label}");
                report.push(label);
            }
        }
    }
    println!("{}", report.join("\n"));
}

#[test]
fn casts_along_spiral_lanes_never_sweep_into_the_wall() {
    // The lane bound of the cast (2.1 fix B1) takes steps of millimetres
    // next to the wall: starts on or near either wall of the lanes of
    // spirals 34 to 44 mm apart, along the lane, across it or anywhere,
    // followed to the end of the window, never sweep deeper than the
    // sliding limit.
    let mut rng = Pcg(0x00b1_0001);
    let shapes: Vec<GuideCurve> = [(0.034, 3.0, 101), (0.04, 3.0, 101), (0.044, 4.0, 201)]
        .into_iter()
        .map(|(pitch, turns, n)| {
            GuideCurve::new(
                CubicSpline::interpolate(&spiral_points(0.1, pitch, turns, n)).unwrap(),
                HEIGHT,
            )
            .unwrap()
        })
        .collect();
    let (mut casts, mut contacts) = (0u32, 0u32);
    for g in &shapes {
        for &ball in &BALL_RADII {
            for &speed in &SPEEDS {
                for k in 0..24 {
                    let t = rng.range(1.0, g.spline().max_t() - 1.0);
                    let on_wall = g.spline().point(t);
                    let tangent = g.spline().tangent(t).normalize();
                    let side = v(-tangent.y, tangent.x, 0.0) * if k % 2 == 0 { 1.0 } else { -1.0 };
                    let off = if k % 3 == 0 {
                        2.0e-6
                    } else {
                        rng.range(0.0, 3.0e-3)
                    };
                    let start = on_wall + side * (ball + off) + Vec3::Z * rng.range(0.0, HEIGHT);
                    if g.closest_point(start, None).distance <= ball + 1.0e-6 {
                        continue;
                    }
                    let along = tangent * if rng.unit() < 0.5 { 1.0 } else { -1.0 };
                    let aim = match rng.next_u32() % 3 {
                        0 => along,
                        1 => along - side * rng.range(0.0, 0.3),
                        _ => v(rng.range(-1.0, 1.0), rng.range(-1.0, 1.0), 0.0),
                    };
                    let dir = aim.normalize_or_zero();
                    if dir == Vec3::ZERO {
                        continue;
                    }
                    let vel = dir * speed;
                    let window = [0.5e-3f32, 1.0e-3, 2.0e-3][(rng.next_u32() % 3) as usize];
                    let (time, contact) = free_time(g, start, ball, vel, window);
                    let sink = min_gap_along(g, start, ball, vel, time, 5.0e-5);
                    casts += 1;
                    contacts += u32::from(contact);
                    assert!(
                        sink >= -MAX_SINK,
                        "ball {ball}, start {start:?}, vel {vel:?}, window {window}: free path \
                         {} µm into the wall",
                        -sink * 1.0e6
                    );
                }
            }
        }
    }
    assert!(
        casts > 800 && contacts > casts / 10 && contacts < casts,
        "{contacts}/{casts}"
    );
}
