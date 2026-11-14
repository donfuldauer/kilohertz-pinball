//! The top and bottom edges of rising and falling guide walls (Subphase 2.2
//! review blocker B6): balls riding the top edge or skimming over or under
//! either edge never sink into the wall, never pass through it, and move at
//! full speed; the closest point there is exact. Ground truth is a
//! brute-force distance to the wall. The timing report is
//! `cargo test -p pin_physics --release --test ball_guide_edges -- --ignored --nocapture`.

#[path = "ball_util.rs"]
mod util;

use glam::Vec3;
use pin_core::ids::ProfileId;
use pin_physics::ball::{BallEvents, BallSolver, Geometry};
use pin_physics::world::{CubicSpline, GuideCast, GuideCurve, StaticWorld};
use util::*;

/// Wall height: low enough that a ball reaches either edge.
const HEIGHT: f32 = 0.03;
const XL: ProfileId = ProfileId(6);

/// Deterministic pseudo-random numbers.
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

fn arc(radius: f32, angle: f32, rise: f32, count: usize) -> Vec<Vec3> {
    (0..=count)
        .map(|i| {
            let s = i as f32 / count as f32;
            let a = angle * s;
            v(
                radius * libm::sinf(a),
                radius - radius * libm::cosf(a),
                rise * s,
            )
        })
        .collect()
}

/// Accepted rising and falling guides, from the gentlest edge to the
/// tightest accepted plan view.
fn shapes() -> Vec<(&'static str, GuideCurve)> {
    let open = |pts: Vec<Vec3>| {
        let g = GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), HEIGHT).unwrap();
        assert!(!g.is_level());
        g
    };
    let ring: Vec<Vec3> = (0..=48)
        .map(|i| {
            let a = std::f32::consts::TAU * (i % 48) as f32 / 48.0;
            v(
                0.1 * libm::cosf(a),
                0.1 * libm::sinf(a),
                0.02 * libm::sinf(a),
            )
        })
        .collect();
    vec![
        (
            "straight 1:5 rising",
            open(
                (0..30)
                    .map(|i| v(0.03 * i as f32, 0.0, 0.006 * i as f32))
                    .collect(),
            ),
        ),
        ("arc r=50mm rise 3:1", open(arc(0.05, 3.0, 0.45, 60))),
        ("arc r=100mm rise 100mm", open(arc(0.1, 2.5, 0.1, 40))),
        ("arc r=30mm rise 1:1", open(arc(0.03, 4.0, 0.12, 60))),
        (
            "S-curve falling",
            open(
                (0..=60)
                    .map(|i| {
                        let s = i as f32 / 60.0;
                        v(
                            0.5 * s,
                            0.03 * libm::sinf(s * 14.0),
                            -0.15 * s + 0.005 * libm::sinf(s * 30.0),
                        )
                    })
                    .collect(),
            ),
        ),
        ("closed ring r=100mm rising and falling", {
            let g =
                GuideCurve::new(CubicSpline::interpolate_closed(&ring).unwrap(), HEIGHT).unwrap();
            assert!(!g.is_level());
            g
        }),
    ]
}

/// Brute-force distance to the wall: dense samples of the spline, then a
/// golden-section search around the best one.
struct Brute<'a> {
    guide: &'a GuideCurve,
    dense: Vec<Vec3>,
    per: usize,
}

impl<'a> Brute<'a> {
    fn new(guide: &'a GuideCurve, per: usize) -> Self {
        let spline = guide.spline();
        let n = spline.segment_count() * per;
        let dense = (0..=n)
            .map(|i| spline.point(i as f32 / per as f32))
            .collect();
        Self { guide, dense, per }
    }

    fn line(&self, c: Vec3, p: Vec3) -> f32 {
        let u = (p.z - c.z).clamp(0.0, self.guide.height());
        v(c.x, c.y, c.z + u).distance(p)
    }

    /// (distance, curve parameter).
    fn distance(&self, p: Vec3) -> (f32, f32) {
        let (mut best, mut at) = (f32::INFINITY, 0usize);
        for (i, c) in self.dense.iter().enumerate() {
            let d = self.line(*c, p);
            if d < best {
                (best, at) = (d, i);
            }
        }
        let spline = self.guide.spline();
        let per = self.per as f32;
        let (mut lo, mut hi) = (
            at.saturating_sub(1) as f32 / per,
            (at + 1).min(self.dense.len() - 1) as f32 / per,
        );
        for _ in 0..40 {
            let (a, b) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
            if self.line(spline.point(a), p) < self.line(spline.point(b), p) {
                hi = b;
            } else {
                lo = a;
            }
        }
        let t = 0.5 * (lo + hi);
        (best.min(self.line(spline.point(t), p)), t)
    }
}

/// The local frame of edge `top` (or the bottom edge) at parameter `t`:
/// edge point, unit edge tangent, the in-plane unit vector pointing away
/// from the wall across that edge, and the horizontal wall normal.
fn edge_frame(g: &GuideCurve, t: f32, top: bool) -> (Vec3, Vec3, Vec3, Vec3) {
    let spline = g.spline();
    let c = spline.point(t);
    let tangent = spline.tangent(t).normalize();
    let flat = v(tangent.x, tangent.y, 0.0).normalize();
    let normal = Vec3::Z.cross(flat);
    let up = (Vec3::Z - tangent * tangent.z).normalize();
    if top {
        (c + Vec3::Z * g.height(), tangent, up, normal)
    } else {
        (c, tangent, -up, normal)
    }
}

/// One edge scenario: a ball started `clear` beyond an edge at parameter
/// `t0`, in the direction `angle` (radians) from the edge's outward
/// direction toward the horizontal wall normal (0 = straight over the top
/// edge or under the bottom edge), moving along the edge and slightly into
/// it.
#[derive(Clone, Copy, Debug)]
struct Scenario {
    profile: ProfileId,
    top: bool,
    angle: f32,
    speed: f32,
    t0: f32,
    clear: f32,
}

impl Scenario {
    fn start(&self, g: &GuideCurve) -> (Vec3, Vec3) {
        let r = radius(self.profile);
        let (e, tangent, out, normal) = edge_frame(g, self.t0, self.top);
        let dir = out * libm::cosf(self.angle) + normal * libm::sinf(self.angle);
        let start = e + dir * (r + self.clear);
        let vel = (tangent - dir * 0.02).normalize() * self.speed;
        (start, vel)
    }
}

fn frictionless_solver() -> BallSolver {
    let mut cfg = config();
    cfg.gravity = Vec3::ZERO;
    for p in &mut cfg.pairs {
        p.friction = 0.0;
    }
    BallSolver::new(cfg, &profiles())
}

/// Every edge scenario of the sweep (deterministic).
fn sweep(g: &GuideCurve, profiles: &[ProfileId], speeds: &[f32]) -> Vec<Scenario> {
    let mut out = Vec::new();
    for &profile in profiles {
        for top in [true, false] {
            for angle_deg in [0.0f32, 35.0, -35.0, 60.0, -60.0] {
                for &speed in speeds {
                    out.push(Scenario {
                        profile,
                        top,
                        angle: angle_deg.to_radians(),
                        speed,
                        t0: 0.12 * g.spline().max_t(),
                        clear: 2.0e-4,
                    });
                }
            }
        }
    }
    out
}

/// Statistics of a run: (ticks in the edge region, ticks below 95 % of
/// v·dt, deepest sink into the wall in metres).
#[derive(Default, Clone, Copy)]
struct Tally {
    ticks: u32,
    slow: u32,
    sink: f32,
}

/// Runs a scenario until the ball leaves the edge region or nears the end
/// of the wall, checking every tick against the brute force.
fn run(
    world: &StaticWorld,
    guides: &[GuideCurve; 1],
    brute: &Brute<'_>,
    sc: Scenario,
    max_ticks: u32,
) -> Tally {
    let g = &guides[0];
    let geo = Geometry {
        world,
        guides,
        wireforms: &[],
    };
    let r = radius(sc.profile);
    let (start, vel) = sc.start(g);
    let mut s = frictionless_solver();
    let h = s.add_ball(ball_at(sc.profile, start, vel)).unwrap();
    let mut ev = BallEvents::new();
    let mut prev = start;
    let mut tally = Tally::default();
    let max_t = g.spline().max_t();
    let closed = g.spline().is_closed();
    for tick in 0..max_ticks {
        ev.clear();
        s.step(&geo, DT, &mut ev);
        let b = *s.ball(h).unwrap();
        let (d, t) = brute.distance(b.position);
        let what = format!("{sc:?} tick {tick}");
        assert!(b.position.is_finite(), "{what}");
        tally.sink = tally.sink.max(r - d);
        assert!(
            r - d <= 1.0e-4,
            "{what}: {} mm inside the wall",
            (r - d) * 1e3
        );
        // The edge region: within 3 mm of the wall, away from its ends.
        let inside = closed || (t > 0.05 * max_t && t < 0.92 * max_t);
        if d > r + 3.0e-3 || !inside {
            break;
        }
        tally.ticks += 1;
        if b.position.distance(prev) < 0.95 * b.velocity.length() * DT {
            tally.slow += 1;
        }
        // (The XL ball is wider than these walls' bends and really hits
        // them where they curve into its path.)
        assert!(
            sc.profile == XL || b.velocity.length() > 0.9 * sc.speed,
            "{what}: lost speed"
        );
        prev = b.position;
    }
    tally
}

/// Balls riding the top edge, and skimming over and under both edges, of
/// every accepted sloped guide: never more than 0.1 mm inside the wall, and
/// at least 95 % of v·dt on at least 99 % of the ticks (frictionless, no
/// gravity, 1–8 m/s, three ball sizes).
#[test]
fn balls_ride_and_skim_sloped_guide_edges_at_full_speed() {
    let world = empty_world();
    for (label, g) in shapes() {
        let guides = [g];
        let brute = Brute::new(&guides[0], 32);
        let mut total = Tally::default();
        for sc in sweep(&guides[0], &[STEEL, MINI, JUMBO], &[1.0, 4.0, 8.0]) {
            let t = run(&world, &guides, &brute, sc, 120);
            total.ticks += t.ticks;
            total.slow += t.slow;
            total.sink = total.sink.max(t.sink);
        }
        assert!(total.ticks > 500, "{label}: only {} ticks", total.ticks);
        assert!(
            total.slow * 100 <= total.ticks,
            "{label}: {} of {} ticks below 95 % of v·dt",
            total.slow,
            total.ticks
        );
    }
}

/// The XL ball (radius 39.7 mm, larger than these walls' reach) still never
/// sinks into the wall at an edge; it may be slow there.
#[test]
fn xl_ball_at_sloped_guide_edges_never_sinks() {
    let world = empty_world();
    for (_, g) in shapes().into_iter().take(4) {
        let guides = [g];
        let brute = Brute::new(&guides[0], 32);
        for sc in sweep(&guides[0], &[XL], &[1.0, 8.0]) {
            run(&world, &guides, &brute, sc, 40);
        }
    }
}

/// Closest point in the edge regions (above the top edge and below the
/// bottom edge, within a ball radius of the wall horizontally, up to 45 mm
/// away) against brute force: never more than 1e-6 m above the true
/// distance nor 2e-6 m below it, seeded or not.
#[test]
fn sloped_guide_closest_point_is_exact_at_the_edges() {
    let mut rng = Lcg(77);
    for (label, g) in shapes() {
        let brute = Brute::new(&g, 128);
        let max_t = g.spline().max_t();
        for case in 0..400 {
            let t = rng.unit() * max_t;
            let top = case % 2 == 0;
            let (e, _, out, normal) = edge_frame(&g, t, top);
            let p = e
                + out * (0.045 * rng.unit())
                + normal * (0.02 * rng.signed())
                + v(rng.signed(), rng.signed(), rng.signed()) * 0.002;
            let (truth, _) = brute.distance(p);
            for seed in [None, Some(rng.unit() * max_t)] {
                let cp = g.closest_point(p, seed);
                assert!(
                    cp.distance <= truth + 1.0e-6 && cp.distance >= truth - 2.0e-6,
                    "{label}: {} instead of {truth} at {p:?}",
                    cp.distance
                );
                assert!(
                    (cp.point.distance(p) - cp.distance).abs() < 1.0e-6,
                    "{label}"
                );
            }
        }
    }
}

/// Sphere casts from the edge regions, fired along, across and into the
/// edges at 0.2–30 m/s: up to the reported contact (or the end of the
/// window) the swept sphere is never more than the 10 µm sliding allowance
/// inside the wall (brute force along the path), and a contact is at the
/// wall. Mini to XL radius.
#[test]
fn sloped_guide_edge_casts_never_tunnel() {
    let mut rng = Lcg(4242);
    for (label, g) in shapes() {
        let brute = Brute::new(&g, 32);
        let max_t = g.spline().max_t();
        let mut cases = 0;
        while cases < 150 {
            let r = 0.008 + 0.032 * rng.unit();
            let t = (0.03 + 0.94 * rng.unit()) * max_t;
            let top = rng.unit() < 0.5;
            let (e, tangent, out, normal) = edge_frame(&g, t, top);
            let p0 = e + out * (r * (0.6 + 0.6 * rng.unit())) + normal * (r * 1.2 * rng.signed());
            if brute.distance(p0).0 < r + 1.0e-4 {
                continue;
            }
            cases += 1;
            let dir =
                (tangent * rng.signed() * 2.0 + out * rng.signed() + normal * rng.signed() * 0.5)
                    .normalize();
            let speed = 0.2 + 29.8 * rng.unit() * rng.unit();
            let vel = dir * speed;
            let window = 2.0e-3f32;
            let (mut time, mut end) = (0.0f32, window);
            for step in 0.. {
                assert!(step < 200, "{label}: too many casts");
                match g.cast_sphere(p0 + vel * time, r, vel, window - time, None) {
                    None => break,
                    Some(GuideCast::Advance { time: dt }) => {
                        assert!(dt > 0.0);
                        time += dt;
                    }
                    Some(GuideCast::Contact(hit)) => {
                        end = time + hit.time;
                        let gap = brute.distance(p0 + vel * end).0 - r;
                        assert!(gap.abs() <= 2.0e-5, "{label}: contact gap {gap}");
                        break;
                    }
                }
            }
            let samples = ((speed * end / 2.0e-4) as usize).clamp(20, 300);
            for k in 0..=samples {
                let p = p0 + vel * (end * k as f32 / samples as f32);
                let gap = brute.distance(p).0 - r;
                assert!(
                    gap > -2.0e-5,
                    "{label}: tunnelled, gap {gap} at {k}/{samples} (r {r}, v {vel:?}, p0 {p0:?})"
                );
            }
        }
    }
}

/// The acceptance rule covers the edges: a falling wall whose edges bend up
/// and down at about 14 mm radius (a 20 mm ripple) is rejected, the same
/// curve with a 5 mm ripple is accepted, and the rule does not depend on
/// the wall height.
#[test]
fn sloped_guide_edges_must_not_bend_tighter_than_a_ball() {
    use pin_physics::world::SplineError;
    let s_curve = |ripple: f32| -> CubicSpline {
        let pts: Vec<Vec3> = (0..=60)
            .map(|i| {
                let s = i as f32 / 60.0;
                v(
                    0.5 * s,
                    0.03 * libm::sinf(s * 14.0),
                    -0.15 * s + ripple * libm::sinf(s * 30.0),
                )
            })
            .collect();
        CubicSpline::interpolate(&pts).unwrap()
    };
    for height in [0.0f32, 0.03, 0.4] {
        assert_eq!(
            GuideCurve::new(s_curve(0.02), height).map(drop),
            Err(SplineError::SlopedGuidePlanView)
        );
        let g = GuideCurve::new(s_curve(0.005), height).unwrap();
        assert!(g.reach() >= 0.02 && g.reach() < 0.04, "{}", g.reach());
    }
    // A plain helical arc keeps its plan view's reach (its edges are
    // helices, whose curvature is below the plan view's).
    let level = GuideCurve::new(
        CubicSpline::interpolate(&arc(0.05, 3.0, 0.0, 60)).unwrap(),
        0.03,
    )
    .unwrap();
    let rising = GuideCurve::new(
        CubicSpline::interpolate(&arc(0.05, 3.0, 0.45, 60)).unwrap(),
        0.03,
    )
    .unwrap();
    assert!(
        rising.reach() >= 0.99 * level.reach(),
        "{} vs {}",
        rising.reach(),
        level.reach()
    );
}

/// Number of closest-point evaluations of one cast for a ball riding or
/// skimming an edge at 8 m/s over a 1 ms window: a handful, as beside the
/// wall (it used to exhaust the 32-step budget).
#[test]
fn edge_casts_take_few_steps() {
    for (label, g) in shapes() {
        for sc in sweep(&g, &[STEEL, JUMBO], &[8.0]) {
            let r = radius(sc.profile);
            let (start, vel) = sc.start(&g);
            // Along the edge, touching it (no push into it).
            let (_, tangent, _, _) = edge_frame(&g, sc.t0, sc.top);
            let along = tangent * vel.length();
            let (_, steps) = g.cast_sphere_counted(start, r, along, 1.0e-3, None);
            assert!(steps <= 24, "{label} {sc:?}: {steps} steps");
        }
    }
}

// ------------------------------------------------------------------ report

fn median(mut samples: Vec<std::time::Duration>) -> std::time::Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

/// Timing report of the edge regions (release): closest point there, and
/// one or four balls riding or skimming the edges (each step timed five
/// times, its fastest kept: what remains of a spike is the solver's own
/// cost, not the machine's), plus the reviewer's gravity scenario.
#[test]
#[ignore = "timing report, run in release"]
fn report_edge_region_costs() {
    use std::hint::black_box;
    use std::time::{Duration, Instant};
    let world = empty_world();
    println!(
        "| guide | ball(s) | closest_point median | mean/step | p99.9 | worst | ticks < 95 % v·dt |"
    );
    println!("|---|---|---|---|---|---|---|");
    for (label, g) in shapes() {
        let guides = [g];
        let g = &guides[0];
        let geo = Geometry {
            world: &world,
            guides: &guides,
            wireforms: &[],
        };
        let max_t = g.spline().max_t();
        for profiles in [
            vec![STEEL],
            vec![MINI],
            vec![JUMBO],
            vec![XL],
            vec![STEEL, MINI, JUMBO, STEEL],
        ] {
            // Closest point at edge-region positions of these balls.
            let mut rng = Lcg(5);
            let mut queries = Vec::new();
            for _ in 0..2000 {
                let r = radius(profiles[0]);
                let (e, _, out, normal) =
                    edge_frame(g, (0.05 + 0.9 * rng.unit()) * max_t, rng.unit() < 0.5);
                let a = rng.signed() * 1.1;
                queries.push(e + (out * libm::cosf(a) + normal * libm::sinf(a)) * (r + 1.0e-4));
            }
            let mut times = Vec::new();
            for _ in 0..5 {
                for p in &queries {
                    let t0 = Instant::now();
                    black_box(g.closest_point(black_box(*p), None));
                    times.push(t0.elapsed());
                }
            }
            let closest = median(times);
            // Scenarios: every sweep case, `profiles.len()` balls at once
            // (spread along the wall).
            let speeds = [1.0f32, 2.0, 4.0, 6.0, 8.0];
            let cases = sweep(g, &profiles[..1], &speeds);
            let mut fastest: Vec<Duration> = Vec::new();
            let (mut ticks, mut slow) = (0u32, 0u32);
            for repeat in 0..5 {
                let mut steps = Vec::new();
                (ticks, slow) = (0, 0);
                for (ci, base) in cases.iter().enumerate() {
                    let mut s = frictionless_solver();
                    for (k, &profile) in profiles.iter().enumerate() {
                        let mut sc = *base;
                        sc.profile = profile;
                        sc.t0 = (0.08 + 0.2 * k as f32) * max_t;
                        if profiles.len() > 1 {
                            // Mix the edges and sides among the four.
                            let other = &cases[(ci + 7 * k) % cases.len()];
                            sc.top = other.top;
                            sc.angle = other.angle;
                        }
                        let (start, vel) = sc.start(g);
                        s.add_ball(ball_at(profile, start, vel)).unwrap();
                    }
                    let mut ev = BallEvents::new();
                    let mut prev: Vec<Vec3> = s.balls().map(|(_, b)| b.position).collect();
                    for _ in 0..200 {
                        ev.clear();
                        let t0 = Instant::now();
                        s.step(&geo, DT, &mut ev);
                        steps.push(t0.elapsed());
                        let mut done = false;
                        for ((_, b), last) in s.balls().zip(prev.iter_mut()) {
                            let cp = g.closest_point(b.position, None);
                            let r = radius(b.profile);
                            let closed = g.spline().is_closed();
                            done |= cp.distance > r + 3.0e-3
                                || (!closed && (cp.t < 0.05 * max_t || cp.t > 0.92 * max_t));
                            ticks += 1;
                            if b.position.distance(*last) < 0.95 * b.velocity.length() * DT {
                                slow += 1;
                            }
                            *last = b.position;
                        }
                        if done {
                            break;
                        }
                    }
                }
                if repeat == 0 {
                    fastest = steps;
                } else {
                    assert_eq!(fastest.len(), steps.len(), "not deterministic");
                    for (best, t) in fastest.iter_mut().zip(steps) {
                        *best = (*best).min(t);
                    }
                }
            }
            let total: Duration = fastest.iter().sum();
            let mean = total / fastest.len().max(1) as u32;
            let mut sorted = fastest.clone();
            sorted.sort_unstable();
            let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q) as usize];
            let names: Vec<&str> = profiles
                .iter()
                .map(|p| match p.0 {
                    0 => "steel",
                    4 => "mini",
                    5 => "jumbo",
                    6 => "XL",
                    _ => "?",
                })
                .collect();
            println!(
                "| {label} | {} | {closest:?} | {mean:?} | {:?} | {:?} | {slow} of {ticks} |",
                names.join("+"),
                at(0.999),
                at(1.0)
            );
        }
    }

    // The reviewer's scenario: a ball dropped onto the top edge with
    // gravity and friction (it rides, then falls off), steel/mini/XL.
    println!("\n| gravity, on the top edge | ball | offset | v0 | worst step | mean step |");
    println!("|---|---|---|---|---|---|");
    for (label, g) in shapes().into_iter().take(3) {
        let guides = [g];
        let g = &guides[0];
        let geo = Geometry {
            world: &world,
            guides: &guides,
            wireforms: &[],
        };
        for profile in [STEEL, MINI, XL] {
            for (off, v0) in [(0.0f32, 0.0f32), (0.0, 1.0), (0.0, -2.0), (0.002, 0.5)] {
                let r = radius(profile);
                let t = 0.5 * g.spline().max_t();
                let (e, tangent, _, normal) = edge_frame(g, t, true);
                let start = e + Vec3::Z * (r + 1.0e-6) + normal * off;
                let mut fastest: Vec<Duration> = Vec::new();
                for repeat in 0..5 {
                    let mut s = solver();
                    let h = s.add_ball(ball_at(profile, start, tangent * v0)).unwrap();
                    let mut ev = BallEvents::new();
                    let mut steps = Vec::new();
                    for _ in 0..600 {
                        ev.clear();
                        let t0 = Instant::now();
                        s.step(&geo, DT, &mut ev);
                        steps.push(t0.elapsed());
                    }
                    let _ = s.ball(h);
                    if repeat == 0 {
                        fastest = steps;
                    } else {
                        for (best, t) in fastest.iter_mut().zip(steps) {
                            *best = (*best).min(t);
                        }
                    }
                }
                let worst = fastest.iter().max().unwrap();
                let mean = fastest.iter().sum::<Duration>() / fastest.len() as u32;
                println!("| {label} | {profile:?} | {off} | {v0} | {worst:?} | {mean:?} |");
            }
        }
    }
}
