//! Reviewer probes (final 2.2 review). TEMPORARY - deleted after review.
#![allow(dead_code, clippy::all)]

#[path = "ball_util.rs"]
mod util;

use glam::{DVec3, Vec3};
use pin_core::ids::ProfileId;
use pin_physics::ball::{BallEvents, BallSolver, Geometry};
use pin_physics::world::{CubicSpline, GuideCast, GuideCurve, StaticWorld};
use std::time::{Duration, Instant};
use util::*;

const XL: ProfileId = ProfileId(6);

struct Lcg(u64);
impl Lcg {
    fn unit(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }
    fn signed(&mut self) -> f32 {
        2.0 * self.unit() - 1.0
    }
    fn range(&mut self, a: f32, b: f32) -> f32 {
        a + (b - a) * self.unit()
    }
}

// ------------------------------------------------------------ shapes

fn arc(radius: f32, angle: f32, rise: f32, count: usize) -> Vec<Vec3> {
    (0..=count)
        .map(|i| {
            let s = i as f32 / count as f32;
            let a = angle * s;
            v(radius * a.sin(), radius - radius * a.cos(), rise * s)
        })
        .collect()
}

/// Resample a dense polyline at uniform arc-length spacing `sp`.
fn resample(dense: &[DVec3], sp: f64, offset: f64) -> Vec<DVec3> {
    let mut out = Vec::new();
    let mut total = 0.0;
    for w in dense.windows(2) {
        total += w[0].distance(w[1]);
    }
    let n = ((total - offset) / sp).floor() as usize;
    let mut targets: Vec<f64> = vec![0.0];
    for i in 0..=n {
        let s = offset + sp * i as f64;
        if s > 1e-9 && s < total - 0.3 * sp {
            targets.push(s);
        }
    }
    targets.push(total);
    let mut acc = 0.0;
    let mut k = 0;
    for w in dense.windows(2) {
        let l = w[0].distance(w[1]);
        while k < targets.len() && targets[k] <= acc + l + 1e-12 {
            let u = ((targets[k] - acc) / l).clamp(0.0, 1.0);
            out.push(w[0] + (w[1] - w[0]) * u);
            k += 1;
        }
        acc += l;
    }
    out
}

/// Teardrop / omega: two parallel legs `d` apart joined by fillets (radius rf)
/// and a loop (radius big). Plan view only (z = 0); dense polyline.
fn teardrop_dense(d: f64, rf: f64, big: f64, leg: f64) -> Vec<DVec3> {
    let mut pts = Vec::new();
    let step = 2.0e-4;
    let f1 = DVec3::new(-d / 2.0 - rf, 0.0, 0.0);
    let yc = ((big + rf).powi(2) - (d / 2.0 + rf).powi(2)).sqrt();
    let o = DVec3::new(0.0, yc, 0.0);
    // left leg up
    let n = (leg / step) as usize;
    for i in 0..n {
        pts.push(DVec3::new(-d / 2.0, -leg + step * i as f64, 0.0));
    }
    // fillet 1, CCW about f1 from angle 0 to a1
    let a1 = yc.atan2(d / 2.0 + rf);
    let n = (a1 * rf / step) as usize + 1;
    for i in 0..n {
        let a = a1 * i as f64 / n as f64;
        pts.push(f1 + DVec3::new(a.cos(), a.sin(), 0.0) * rf);
    }
    // loop clockwise about o from b1 to pi - b1
    let p1 = f1 + (o - f1).normalize() * rf;
    let mut b1 = (p1.y - o.y).atan2(p1.x - o.x);
    if b1 < 0.0 {
        b1 += std::f64::consts::TAU;
    }
    let b2 = std::f64::consts::PI - b1;
    let n = ((b1 - b2) * big / step) as usize + 1;
    for i in 0..n {
        let b = b1 + (b2 - b1) * i as f64 / n as f64;
        pts.push(o + DVec3::new(b.cos(), b.sin(), 0.0) * big);
    }
    // fillet 2 (mirror), from a1 down to 0
    let f2 = DVec3::new(d / 2.0 + rf, 0.0, 0.0);
    let n = (a1 * rf / step) as usize + 1;
    for i in 0..n {
        let a = a1 * (1.0 - i as f64 / n as f64);
        pts.push(f2 + DVec3::new(-a.cos(), a.sin(), 0.0) * rf);
    }
    let n = (leg / step) as usize;
    for i in 0..=n {
        pts.push(DVec3::new(d / 2.0, -step * i as f64, 0.0));
    }
    pts
}

fn with_slope(pts: &[DVec3], slope: f64) -> Vec<Vec3> {
    let mut out = Vec::new();
    let mut s = 0.0;
    for (i, p) in pts.iter().enumerate() {
        if i > 0 {
            s += pts[i - 1].distance(*p);
        }
        out.push(DVec3::new(p.x, p.y, slope * s).as_vec3());
    }
    out
}

fn try_guide(pts: &[Vec3], h: f32, closed: bool) -> Option<GuideCurve> {
    let sp = if closed {
        CubicSpline::interpolate_closed(pts).ok()?
    } else {
        CubicSpline::interpolate(pts).ok()?
    };
    GuideCurve::new(sp, h).ok()
}

const HEIGHT: f32 = 0.03;

fn base_shapes() -> Vec<(String, GuideCurve)> {
    let mut out: Vec<(String, GuideCurve)> = Vec::new();
    let mut add = |name: &str, pts: Vec<Vec3>, closed: bool| match try_guide(&pts, HEIGHT, closed)
    {
        Some(g) => out.push((name.to_string(), g)),
        None => println!("   (rejected: {name})"),
    };
    add(
        "straight 1:5",
        (0..30).map(|i| v(0.03 * i as f32, 0.0, 0.006 * i as f32)).collect(),
        false,
    );
    add("arc r50 3:1", arc(0.05, 3.0, 0.45, 60), false);
    add("arc r100 rise100", arc(0.1, 2.5, 0.1, 40), false);
    add("arc r30 1:1", arc(0.03, 4.0, 0.12, 60), false);
    add("arc r30 flat-ish rise 5mm", arc(0.03, 4.0, 0.005, 60), false);
    add("arc r32 steep 6:1", arc(0.032, 3.0, 0.58, 60), false);
    add("arc r400 rise 150", arc(0.4, 1.5, 0.15, 40), false);
    add(
        "S falling ripple5",
        (0..=60)
            .map(|i| {
                let s = i as f32 / 60.0;
                v(0.5 * s, 0.03 * (s * 14.0).sin(), -0.15 * s + 0.005 * (s * 30.0).sin())
            })
            .collect(),
        false,
    );
    let ring: Vec<Vec3> = (0..=48)
        .map(|i| {
            let a = std::f32::consts::TAU * (i % 48) as f32 / 48.0;
            v(0.1 * a.cos(), 0.1 * a.sin(), 0.02 * a.sin())
        })
        .collect();
    add("ring r100 tilt", ring, true);
    // Tightest accepted S ripple amplitude (scan).
    let mut best = None;
    for k in 10..40 {
        let amp = 0.0005 * k as f32;
        let pts: Vec<Vec3> = (0..=60)
            .map(|i| {
                let s = i as f32 / 60.0;
                v(0.5 * s, 0.03 * (s * 14.0).sin(), -0.15 * s + amp * (s * 30.0).sin())
            })
            .collect();
        if try_guide(&pts, HEIGHT, false).is_some() {
            best = Some((amp, pts));
        }
    }
    if let Some((amp, pts)) = best {
        add(&format!("S ripple max accepted {:.1}mm", amp * 1e3), pts, false);
    }
    // Vertical wave on a straight plan: z = A sin(k x); tightest accepted.
    let mut best = None;
    for k in 1..60 {
        let amp = 0.0005 * k as f32;
        let pts: Vec<Vec3> = (0..=80)
            .map(|i| {
                let x = 0.005 * i as f32;
                v(x, 0.0, amp * (x * 60.0).sin())
            })
            .collect();
        if try_guide(&pts, HEIGHT, false).is_some() {
            best = Some((amp, pts));
        }
    }
    if let Some((amp, pts)) = best {
        add(&format!("vertical wave max accepted A={:.1}mm (k=60)", amp * 1e3), pts, false);
    }
    // Tightest accepted plan S (wiggle) amplitude with rise.
    let mut best = None;
    for k in 1..60 {
        let amp = 0.001 * k as f32;
        let pts: Vec<Vec3> = (0..=80)
            .map(|i| {
                let x = 0.005 * i as f32;
                v(x, amp * (x * 40.0).sin(), 0.3 * x)
            })
            .collect();
        if try_guide(&pts, HEIGHT, false).is_some() {
            best = Some((amp, pts));
        }
    }
    if let Some((amp, pts)) = best {
        add(&format!("plan wave max accepted A={:.0}mm (k=40)", amp * 1e3), pts, false);
    }
    // Teardrops: smallest accepted leg distance for several spacings.
    for sp in [0.012f64, 0.02, 0.04, 0.08, 0.15, 0.3] {
        let mut found: Option<(f64, f64, Vec<Vec3>)> = None;
        'scan: for dk in 0..60 {
            let d = 0.004 + 0.001 * dk as f64;
            for ok in 0..8 {
                let off = sp * ok as f64 / 8.0;
                let leg = if sp > 0.05 { 6.0 * sp } else { 0.25 };
                let dense = teardrop_dense(d, 0.03, 0.07, leg);
                let pts = with_slope(&resample(&dense, sp, off), 0.15);
                if try_guide(&pts, HEIGHT, false).is_some() {
                    found = Some((d, off, pts));
                    break 'scan;
                }
            }
        }
        match found {
            Some((d, off, pts)) => add(
                &format!("teardrop sp={:.0}mm min accepted legs D={:.0}mm (off {:.0}mm)", sp * 1e3, d * 1e3, off * 1e3),
                pts,
                false,
            ),
            None => println!("   teardrop sp={:.0}mm: nothing accepted up to D=63mm", sp * 1e3),
        }
    }
    // Rising spiral (one turn, plan radius growing): max accepted angle.
    for pitch in [0.03f32, 0.045, 0.06] {
        let mut best = None;
        for k in 0..40 {
            let ang = 4.5 + 0.1 * k as f32;
            let n = (ang / 0.12) as usize;
            let pts: Vec<Vec3> = (0..=n)
                .map(|i| {
                    let a = ang * i as f32 / n as f32;
                    let r = 0.05 + pitch * a / std::f32::consts::TAU;
                    v(r * a.cos(), r * a.sin(), 0.01 * a)
                })
                .collect();
            if try_guide(&pts, HEIGHT, false).is_some() {
                best = Some((ang, pts));
            }
        }
        if let Some((ang, pts)) = best {
            add(&format!("spiral pitch {:.0}mm max accepted {:.1} rad", pitch * 1e3, ang), pts, false);
        }
    }
    out
}

// ------------------------------------------------------------ brute force

struct Brute<'a> {
    g: &'a GuideCurve,
    dense: Vec<Vec3>,
    per: usize,
}
impl<'a> Brute<'a> {
    fn new(g: &'a GuideCurve, per: usize) -> Self {
        let sp = g.spline();
        let n = sp.segment_count() * per;
        let dense = (0..=n).map(|i| sp.point(i as f32 / per as f32)).collect();
        Self { g, dense, per }
    }
    fn line(&self, c: Vec3, p: Vec3) -> f32 {
        let u = (p.z - c.z).clamp(0.0, self.g.height());
        v(c.x, c.y, c.z + u).distance(p)
    }
    fn distance(&self, p: Vec3) -> (f32, f32) {
        let (mut best, mut at) = (f32::INFINITY, 0usize);
        for (i, c) in self.dense.iter().enumerate() {
            let d = self.line(*c, p);
            if d < best {
                (best, at) = (d, i);
            }
        }
        let sp = self.g.spline();
        let per = self.per as f32;
        let (mut lo, mut hi) = (
            at.saturating_sub(1) as f32 / per,
            (at + 1).min(self.dense.len() - 1) as f32 / per,
        );
        for _ in 0..40 {
            let (a, b) = (lo + (hi - lo) / 3.0, hi - (hi - lo) / 3.0);
            if self.line(sp.point(a), p) < self.line(sp.point(b), p) {
                hi = b;
            } else {
                lo = a;
            }
        }
        let t = 0.5 * (lo + hi);
        (best.min(self.line(sp.point(t), p)), t)
    }
    /// Does the straight centre path a->b pass through the ribbon?
    fn crosses(&self, a: Vec3, b: Vec3) -> bool {
        let sp = self.g.spline();
        let n = ((a.distance(b) / 2.0e-4) as usize).clamp(8, 400);
        let mut last: Option<(f32, f32)> = None; // (side, t)
        let max_t = sp.max_t();
        for k in 0..=n {
            let p = a + (b - a) * (k as f32 / n as f32);
            // nearest in plan among dense
            let (mut best, mut at) = (f32::INFINITY, 0usize);
            for (i, c) in self.dense.iter().enumerate() {
                let d = (c.x - p.x).powi(2) + (c.y - p.y).powi(2);
                if d < best {
                    (best, at) = (d, i);
                }
            }
            let t = at as f32 / self.per as f32;
            let c = self.dense[at];
            let tan = sp.tangent(t);
            let side = tan.x * (p.y - c.y) - tan.y * (p.x - c.x);
            let inside_z = p.z > c.z + 5.0e-4 && p.z < c.z + self.g.height() - 5.0e-4;
            let interior = sp.is_closed() || (t > 0.02 * max_t && t < 0.98 * max_t);
            if !(inside_z && interior && best.sqrt() < 0.01) {
                last = None;
                continue;
            }
            if let Some((s0, t0)) = last {
                if s0 * side < 0.0 && (t - t0).abs() < 0.5 {
                    return true;
                }
            }
            last = Some((side, t));
        }
        false
    }
}

/// Brute-force Federer reach of the wall (dense pairs), metres.
fn brute_wall_reach(g: &GuideCurve, heights: &[f64]) -> f64 {
    let sp = g.spline();
    let per = 24usize;
    let n = sp.segment_count() * per;
    let closed = sp.is_closed();
    let pts: Vec<(DVec3, DVec3)> = (0..=n)
        .map(|i| {
            let t = i as f32 / per as f32;
            (sp.point(t).as_dvec3(), sp.tangent(t).as_dvec3().normalize())
        })
        .collect();
    let h = g.height() as f64;
    let mut inv = 0.0f64;
    for (ia, (c, tan)) in pts.iter().enumerate() {
        let flat = (tan.x * tan.x + tan.y * tan.y).sqrt();
        let th = DVec3::new(tan.x, tan.y, 0.0) / flat;
        let nrm = DVec3::new(-th.y, th.x, 0.0);
        // in-plane coordinates (th, Z)
        let end = if closed {
            0
        } else if ia == 0 {
            1
        } else if ia == n {
            -1
        } else {
            0
        };
        for (xi, xu) in [0.0, 0.5 * h, h].into_iter().enumerate() {
            let x = *c + DVec3::Z * xu;
            for (ib, (c2, _)) in pts.iter().enumerate() {
                for &yu in heights {
                    let y = *c2 + DVec3::Z * (yu * h);
                    let w = y - x;
                    let w2 = w.length_squared();
                    if w2 < 1.0e-8 {
                        continue; // < 0.1 mm
                    }
                    let _ = ib;
                    let wn = w.dot(nrm);
                    let (a, b) = (w.dot(th), w.z); // in-plane
                    // cone constraints as half-planes: list of outward unit normals (in-plane)
                    let mut outs: Vec<(f64, f64)> = Vec::new();
                    if xi == 2 {
                        // top edge: away direction m = (Z - T Tz)/flat => in (th,Z): (-Tz, flat)
                        outs.push((-tan.z, flat));
                    }
                    if xi == 0 {
                        outs.push((tan.z, -flat));
                    }
                    if end == 1 {
                        outs.push((-1.0, 0.0));
                    }
                    if end == -1 {
                        outs.push((1.0, 0.0));
                    }
                    let dist_in = match outs.len() {
                        0 => 0.0,
                        1 => (a * outs[0].0 + b * outs[0].1).max(0.0),
                        _ => {
                            let d0 = a * outs[0].0 + b * outs[0].1;
                            let d1 = a * outs[1].0 + b * outs[1].1;
                            if d0 <= 0.0 && d1 <= 0.0 {
                                0.0
                            } else {
                                // distance to wedge: project on each boundary ray
                                // rays: boundary directions of the wedge
                                // boundary of half-plane i is perpendicular to outs[i];
                                // choose direction that satisfies the other constraint.
                                let ray = |o: (f64, f64), other: (f64, f64)| {
                                    let mut dir = (-o.1, o.0);
                                    if dir.0 * other.0 + dir.1 * other.1 > 0.0 {
                                        dir = (-dir.0, -dir.1);
                                    }
                                    let s = (a * dir.0 + b * dir.1).max(0.0);
                                    ((a - s * dir.0).powi(2) + (b - s * dir.1).powi(2)).sqrt()
                                };
                                ray(outs[0], outs[1]).min(ray(outs[1], outs[0]))
                            }
                        }
                    };
                    let dist = (wn * wn + dist_in * dist_in).sqrt();
                    let r = 2.0 * dist / w2;
                    if r > inv {
                        inv = r;
                    }
                }
            }
        }
    }
    if inv > 0.0 { 1.0 / inv } else { f64::INFINITY }
}

fn edge_frame(g: &GuideCurve, t: f32, top: bool) -> (Vec3, Vec3, Vec3, Vec3) {
    let sp = g.spline();
    let c = sp.point(t);
    let tangent = sp.tangent(t).normalize();
    let flat = v(tangent.x, tangent.y, 0.0).normalize();
    let normal = Vec3::Z.cross(flat);
    let up = (Vec3::Z - tangent * tangent.z).normalize();
    if top {
        (c + Vec3::Z * g.height(), tangent, up, normal)
    } else {
        (c, tangent, -up, normal)
    }
}

fn frictionless() -> BallSolver {
    let mut cfg = config();
    cfg.gravity = Vec3::ZERO;
    for p in &mut cfg.pairs {
        p.friction = 0.0;
    }
    BallSolver::new(cfg, &profiles())
}

// ------------------------------------------------------------ T1: reach

#[test]
fn t1_reach_estimate_vs_brute() {
    println!("| shape | est reach mm | brute reach mm (h=30) | brute (h=400) | est/brute |");
    for (name, g) in base_shapes() {
        let est = g.reach() as f64;
        let b30 = brute_wall_reach(&g, &[0.0, 0.25, 0.5, 0.75, 1.0]);
        let tall = GuideCurve::new(g.spline().clone(), 0.4).unwrap();
        let b400 = brute_wall_reach(&tall, &[0.0, 0.05, 0.1, 0.2, 0.35, 0.5, 0.75, 1.0]);
        let truth = b30.min(b400);
        println!(
            "| {name} | {:.2} | {:.2} | {:.2} | {:.3} {} |",
            est * 1e3,
            b30 * 1e3,
            b400 * 1e3,
            est / truth,
            if est > truth * 1.02 { "OVERESTIMATE" } else { "" }
        );
    }
}

// ------------------------------------------------------------ T2: closest/cast vs brute

#[test]
fn t2_closest_and_cast_vs_brute() {
    println!("| shape | cp max over um | cp max under um | casts | worst swept gap um | bad contacts |");
    for (name, g) in base_shapes() {
        let brute = Brute::new(&g, 48);
        let max_t = g.spline().max_t();
        let mut rng = Lcg(991);
        let (mut over, mut under) = (0.0f32, 0.0f32);
        for case in 0..1500 {
            let t = rng.unit() * max_t;
            let top = case % 2 == 0;
            let (e, _, out, normal) = edge_frame(&g, t, top);
            let p = e
                + out * (0.05 * rng.unit() - 0.005)
                + normal * (0.045 * rng.signed())
                + v(rng.signed(), rng.signed(), rng.signed()) * 0.002;
            let (truth, _) = brute.distance(p);
            for seed in [None, Some(rng.unit() * max_t)] {
                let cp = g.closest_point(p, seed);
                over = over.max(cp.distance - truth);
                under = under.max(truth - cp.distance);
            }
        }
        // casts
        let mut worst = f32::INFINITY;
        let mut bad_contacts = 0;
        let mut cases = 0;
        let mut tries = 0;
        while cases < 1200 && tries < 100000 {
            tries += 1;
            let r = if rng.unit() < 0.8 {
                0.0079 + 0.008 * rng.unit()
            } else {
                0.0079 + 0.032 * rng.unit()
            };
            let t = (0.02 + 0.96 * rng.unit()) * max_t;
            let top = rng.unit() < 0.5;
            let (e, tangent, out, normal) = edge_frame(&g, t, top);
            let p0 = e + out * (r * rng.range(-0.5, 1.6)) + normal * (r * 1.6 * rng.signed());
            if brute.distance(p0).0 < r + 2.0e-5 {
                continue;
            }
            cases += 1;
            let dir = (tangent * rng.signed() * 2.0 + out * rng.signed() + normal * rng.signed())
                .normalize();
            let speed = 0.2 + 29.8 * rng.unit() * rng.unit();
            let vel = dir * speed;
            let window = if cases % 3 == 0 { 2.0e-3f32 } else { 2.5e-4 };
            let (mut time, mut end) = (0.0f32, window);
            for step in 0.. {
                assert!(step < 400, "{name}: too many casts");
                match g.cast_sphere(p0 + vel * time, r, vel, window - time, None) {
                    None => break,
                    Some(GuideCast::Advance { time: dt }) => {
                        if dt <= 0.0 {
                            break;
                        }
                        time += dt;
                    }
                    Some(GuideCast::Contact(hit)) => {
                        end = time + hit.time;
                        let gap = brute.distance(p0 + vel * end).0 - r;
                        if gap.abs() > 2.0e-5 {
                            bad_contacts += 1;
                        }
                        break;
                    }
                }
            }
            let samples = ((speed * end / 1.0e-4) as usize).clamp(20, 600);
            for k in 0..=samples {
                let p = p0 + vel * (end * k as f32 / samples as f32);
                let gap = brute.distance(p).0 - r;
                if gap < worst {
                    worst = gap;
                }
            }
        }
        println!(
            "| {name} | {:.2} | {:.2} | {cases} | {:.1} | {bad_contacts} | {}",
            over * 1e6,
            under * 1e6,
            worst * 1e6,
            if over > 1.0e-6 || worst < -2.0e-5 { "FAIL" } else { "" }
        );
    }
}

// ------------------------------------------------------------ T3: solver at edges

#[derive(Clone, Copy, Debug)]
struct Sc {
    profile: ProfileId,
    top: bool,
    angle: f32,
    speed: f32,
    t0: f32,
    clear: f32,
    dirsign: f32,
    inward: f32,
}
impl Sc {
    fn start(&self, g: &GuideCurve) -> (Vec3, Vec3) {
        let r = radius(self.profile);
        let (e, tangent, out, normal) = edge_frame(g, self.t0, self.top);
        let dir = out * self.angle.cos() + normal * self.angle.sin();
        let start = e + dir * (r + self.clear);
        let vel = (tangent * self.dirsign - dir * self.inward).normalize() * self.speed;
        (start, vel)
    }
}

fn sweep(g: &GuideCurve, profiles: &[ProfileId], speeds: &[f32], t0s: &[f32]) -> Vec<Sc> {
    let mut out = Vec::new();
    let max_t = g.spline().max_t();
    for &profile in profiles {
        for top in [true, false] {
            for angle_deg in [0.0f32, 20.0, -20.0, 45.0, -45.0, 75.0, -75.0] {
                for &speed in speeds {
                    for &t0 in t0s {
                        for dirsign in [1.0f32, -1.0] {
                            out.push(Sc {
                                profile,
                                top,
                                angle: angle_deg.to_radians(),
                                speed,
                                t0: t0 * max_t,
                                clear: 2.0e-4,
                                dirsign,
                                inward: 0.02,
                            });
                        }
                    }
                }
            }
        }
    }
    out
}

#[derive(Default, Clone, Copy)]
struct Tally {
    ticks: u32,
    slow: u32,
    sink: f32,
    crossed: u32,
    minratio: f32,
}

fn run_sc(
    world: &StaticWorld,
    guides: &[GuideCurve],
    brute: &Brute<'_>,
    sc: Sc,
    max_ticks: u32,
    gravity: bool,
    times: Option<&mut Vec<Duration>>,
) -> Tally {
    let g = &guides[0];
    let geo = Geometry { world, guides, wireforms: &[] };
    let r = radius(sc.profile);
    let (start, vel) = sc.start(g);
    let mut s = if gravity { solver() } else { frictionless() };
    let h = s.add_ball(ball_at(sc.profile, start, vel)).unwrap();
    let mut ev = BallEvents::new();
    let mut prev = start;
    let mut tally = Tally { minratio: 1.0, ..Default::default() };
    let max_t = g.spline().max_t();
    let closed = g.spline().is_closed();
    let mut times = times;
    for _tick in 0..max_ticks {
        ev.clear();
        let t0 = Instant::now();
        s.step(&geo, DT, &mut ev);
        if let Some(ts) = times.as_deref_mut() {
            ts.push(t0.elapsed());
        }
        let b = *s.ball(h).unwrap();
        assert!(b.position.is_finite());
        let (d, t) = brute.distance(b.position);
        tally.sink = tally.sink.max(r - d);
        if brute.crosses(prev, b.position) {
            tally.crossed += 1;
        }
        let inside = closed || (t > 0.04 * max_t && t < 0.96 * max_t);
        if d > r + 3.0e-3 || !inside {
            break;
        }
        tally.ticks += 1;
        let ratio = b.position.distance(prev) / (b.velocity.length() * DT).max(1e-9);
        if ratio < 0.95 {
            tally.slow += 1;
        }
        tally.minratio = tally.minratio.min(ratio);
        prev = b.position;
    }
    tally
}

fn extra_edge_shapes() -> Vec<(String, GuideCurve)> {
    base_shapes()
}

#[test]
fn t3_edge_solver_sweep() {
    let world = empty_world();
    println!("| shape | mode | ticks | slow (<95%) | min ratio | max sink um | crossings |");
    for (name, g) in extra_edge_shapes() {
        let guides = [g];
        let brute = Brute::new(&guides[0], 32);
        for gravity in [false, true] {
            let mut total = Tally { minratio: 1.0, ..Default::default() };
            for sc in sweep(&guides[0], &[STEEL, MINI, JUMBO], &[1.0, 2.0, 4.0, 6.0, 8.0], &[0.12, 0.35, 0.6, 0.85]) {
                let t = run_sc(&world, &guides, &brute, sc, 120, gravity, None);
                total.ticks += t.ticks;
                total.slow += t.slow;
                total.sink = total.sink.max(t.sink);
                total.crossed += t.crossed;
                total.minratio = total.minratio.min(t.minratio);
            }
            println!(
                "| {name} | {} | {} | {} ({:.2}%) | {:.2} | {:.1} | {} | {}",
                if gravity { "gravity+friction" } else { "frictionless" },
                total.ticks,
                total.slow,
                100.0 * total.slow as f32 / total.ticks.max(1) as f32,
                total.minratio,
                total.sink * 1e6,
                total.crossed,
                if total.sink > 1.0e-4 || total.crossed > 0 || (!gravity && total.slow * 100 > total.ticks) { "FAIL" } else { "" }
            );
        }
    }
}

/// High-speed attacks on the edges through the solver: 0.2-30 m/s, random
/// directions into the edge region; sink and crossing check.
#[test]
fn t3b_edge_solver_attack() {
    let world = empty_world();
    println!("| shape | runs | max sink um (tick ends) | crossings |");
    for (name, g) in base_shapes() {
        let guides = [g];
        let g = &guides[0];
        let brute = Brute::new(g, 32);
        let geo = Geometry { world: &world, guides: &guides, wireforms: &[] };
        let max_t = g.spline().max_t();
        let mut rng = Lcg(31337);
        let (mut sink, mut crossed, mut runs) = (0.0f32, 0u32, 0u32);
        let mut worst_desc = String::new();
        for case in 0..900 {
            let profile = [STEEL, MINI, JUMBO, XL][case % 4];
            let r = radius(profile);
            let t = (0.06 + 0.88 * rng.unit()) * max_t;
            let top = rng.unit() < 0.6;
            let (e, tangent, out, normal) = edge_frame(g, t, top);
            // target point: on/near the edge; start: 5-40 mm away in a random direction on the outer side
            let dir = (out * rng.range(-0.3, 1.0) + normal * rng.signed() + tangent * rng.signed()).normalize();
            let dist = r + rng.range(0.001, 0.04);
            let start = e + dir * dist;
            if brute.distance(start).0 < r + 1.0e-4 {
                continue;
            }
            let aim = e + (out * rng.signed() + normal * rng.signed() + tangent * rng.signed()) * (0.8 * r) - out * (0.3 * r);
            let speed = 0.2 + 29.8 * rng.unit() * rng.unit();
            let vel = (aim - start).normalize() * speed;
            let gravity = case % 3 == 0;
            let mut s = if gravity { solver() } else { frictionless() };
            let h = s.add_ball(ball_at(profile, start, vel)).unwrap();
            let mut ev = BallEvents::new();
            let mut prev = start;
            runs += 1;
            for _ in 0..60 {
                ev.clear();
                s.step(&geo, DT, &mut ev);
                let b = *s.ball(h).unwrap();
                let (d, _) = brute.distance(b.position);
                if r - d > sink {
                    sink = r - d;
                    worst_desc = format!("{profile:?} speed {speed:.1} top {top} gravity {gravity} start {start:?} vel {vel:?}");
                }
                if brute.crosses(prev, b.position) {
                    crossed += 1;
                }
                prev = b.position;
                if d > r + 0.06 {
                    break;
                }
            }
        }
        println!(
            "| {name} | {runs} | {:.1} | {crossed} | {} {}",
            sink * 1e6,
            if sink > 1.0e-4 || crossed > 0 { "FAIL" } else { "" },
            if sink > 1.0e-4 { worst_desc } else { String::new() }
        );
    }
}

// ------------------------------------------------------------ T4: perf at edges

fn median(mut s: Vec<Duration>) -> Duration {
    s.sort_unstable();
    s[s.len() / 2]
}

#[test]
fn t4_edge_perf() {
    use std::hint::black_box;
    let world = empty_world();
    println!("| shape | balls | cp median | cp p99 | mean/step | p99.9 | worst | slow ticks |");
    for (name, g) in base_shapes() {
        let guides = [g];
        let g = &guides[0];
        let geo = Geometry { world: &world, guides: &guides, wireforms: &[] };
        let max_t = g.spline().max_t();
        for profs in [vec![STEEL], vec![MINI], vec![JUMBO], vec![XL], vec![STEEL, MINI, JUMBO, STEEL]] {
            let mut rng = Lcg(5);
            let mut queries = Vec::new();
            for _ in 0..2000 {
                let r = radius(profs[0]);
                let (e, _, out, normal) = edge_frame(g, (0.05 + 0.9 * rng.unit()) * max_t, rng.unit() < 0.5);
                let a = rng.signed() * 1.3;
                queries.push(e + (out * a.cos() + normal * a.sin()) * (r + rng.range(-1.0e-5, 1.0e-3)));
            }
            let mut best = vec![Duration::MAX; queries.len()];
            for _ in 0..5 {
                for (i, p) in queries.iter().enumerate() {
                    let t0 = Instant::now();
                    black_box(g.closest_point(black_box(*p), None));
                    best[i] = best[i].min(t0.elapsed());
                }
            }
            let mut sorted = best.clone();
            sorted.sort_unstable();
            let cp_med = sorted[sorted.len() / 2];
            let cp_p99 = sorted[sorted.len() * 99 / 100];

            let cases = sweep(g, &profs[..1], &[1.0, 2.0, 4.0, 6.0, 8.0], &[0.12, 0.5]);
            let mut fastest: Vec<Duration> = Vec::new();
            let (mut ticks, mut slow) = (0u32, 0u32);
            for repeat in 0..5 {
                let mut steps = Vec::new();
                (ticks, slow) = (0, 0);
                for (ci, base) in cases.iter().enumerate() {
                    let mut s = frictionless();
                    for (k, &profile) in profs.iter().enumerate() {
                        let mut sc = *base;
                        sc.profile = profile;
                        if profs.len() > 1 {
                            sc.t0 = (0.08 + 0.2 * k as f32) * max_t;
                            sc.dirsign = 1.0;
                            let other = &cases[(ci + 7 * k) % cases.len()];
                            sc.top = other.top;
                            sc.angle = other.angle;
                        }
                        let (start, vel) = sc.start(g);
                        s.add_ball(ball_at(profile, start, vel)).unwrap();
                    }
                    let mut ev = BallEvents::new();
                    let mut prev: Vec<Vec3> = s.balls().map(|(_, b)| b.position).collect();
                    for _ in 0..150 {
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
                                || (!closed && (cp.t < 0.04 * max_t || cp.t > 0.96 * max_t));
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
                    assert_eq!(fastest.len(), steps.len());
                    for (b, t) in fastest.iter_mut().zip(steps) {
                        *b = (*b).min(t);
                    }
                }
            }
            let total: Duration = fastest.iter().sum();
            let mean = total / fastest.len().max(1) as u32;
            let mut sorted = fastest.clone();
            sorted.sort_unstable();
            let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q) as usize];
            let names: Vec<&str> = profs
                .iter()
                .map(|p| match p.0 {
                    0 => "steel",
                    4 => "mini",
                    5 => "jumbo",
                    6 => "XL",
                    _ => "?",
                })
                .collect();
            let four = profs.len() > 1;
            let xl = profs[0] == XL;
            let fail = !xl
                && (cp_med > Duration::from_micros(3)
                    || mean > Duration::from_micros(if four { 60 } else { 20 })
                    || at(1.0) > Duration::from_micros(if four { 350 } else { 100 })
                    || slow * 100 > ticks);
            println!(
                "| {name} | {} | {cp_med:?} | {cp_p99:?} | {mean:?} | {:?} | {:?} | {slow} of {ticks} | {}",
                names.join("+"),
                at(0.999),
                at(1.0),
                if fail { "FAIL" } else { "" }
            );
        }
    }
}

/// Balls dropped onto the top edge with gravity and friction.
#[test]
fn t4b_drop_on_top_edge() {
    let world = empty_world();
    println!("| shape | ball | drop h mm | off mm | v0 | worst step | mean step | max sink um | crossings |");
    for (name, g) in base_shapes() {
        let guides = [g];
        let g = &guides[0];
        let brute = Brute::new(g, 32);
        let geo = Geometry { world: &world, guides: &guides, wireforms: &[] };
        let mut worst_all = Duration::ZERO;
        let mut mean_max = Duration::ZERO;
        let (mut sink_all, mut cross_all) = (0.0f32, 0u32);
        let mut rows = Vec::new();
        for profile in [STEEL, MINI, JUMBO, XL] {
            for (drop_h, off, v0) in [
                (0.0f32, 0.0f32, 0.0f32),
                (0.0, 0.0, 1.0),
                (0.0, 0.0, -2.0),
                (0.0, 0.002, 0.5),
                (0.02, 0.0, 0.0),
                (0.05, 0.003, 3.0),
                (0.1, -0.006, 6.0),
                (0.005, 0.0, 8.0),
            ] {
                let r = radius(profile);
                for tf in [0.3f32, 0.6] {
                    let t = tf * g.spline().max_t();
                    let (e, tangent, _, normal) = edge_frame(g, t, true);
                    let start = e + Vec3::Z * (r + 1.0e-6 + drop_h) + normal * off;
                    let mut fastest: Vec<Duration> = Vec::new();
                    let (mut sink, mut crossed) = (0.0f32, 0u32);
                    for repeat in 0..3 {
                        let mut s = solver();
                        let h = s.add_ball(ball_at(profile, start, tangent * v0)).unwrap();
                        let mut ev = BallEvents::new();
                        let mut steps = Vec::new();
                        let mut prev = start;
                        for _ in 0..400 {
                            ev.clear();
                            let t0 = Instant::now();
                            s.step(&geo, DT, &mut ev);
                            steps.push(t0.elapsed());
                            if repeat == 0 {
                                let b = *s.ball(h).unwrap();
                                let (d, _) = brute.distance(b.position);
                                sink = sink.max(r - d);
                                if brute.crosses(prev, b.position) {
                                    crossed += 1;
                                }
                                prev = b.position;
                            }
                        }
                        if repeat == 0 {
                            fastest = steps;
                        } else {
                            for (b, t) in fastest.iter_mut().zip(steps) {
                                *b = (*b).min(t);
                            }
                        }
                    }
                    let worst = *fastest.iter().max().unwrap();
                    let mean = fastest.iter().sum::<Duration>() / fastest.len() as u32;
                    if profile != XL {
                        worst_all = worst_all.max(worst);
                        mean_max = mean_max.max(mean);
                    }
                    sink_all = sink_all.max(sink);
                    cross_all += crossed;
                    if worst > Duration::from_micros(100) || sink > 1.0e-4 || crossed > 0 || mean > Duration::from_micros(20) {
                        rows.push(format!(
                            "|   {name} | {profile:?} | {:.0} | {:.0} | {v0} | {worst:?} | {mean:?} | {:.1} | {crossed} |",
                            drop_h * 1e3,
                            off * 1e3,
                            sink * 1e6
                        ));
                    }
                }
            }
        }
        println!(
            "| {name} | steel/mini/jumbo summary | | | | worst {worst_all:?} | max mean {mean_max:?} | {:.1} (incl XL) | {cross_all} |",
            sink_all * 1e6
        );
        for r in rows.iter().take(12) {
            println!("{r}");
        }
    }
}

// ------------------------------------------------------------ T5: level guides, open ends

fn c_ring(radius: f32, opening: f32, sp: f32, z: f32) -> Vec<Vec3> {
    // opening = chord between the two ends
    let half = (opening / (2.0 * radius)).asin();
    let ang = std::f32::consts::TAU - 2.0 * half;
    let n = ((ang * radius / sp).round() as usize).max(8);
    (0..=n)
        .map(|i| {
            let a = half + ang * i as f32 / n as f32;
            v(radius * a.cos(), radius * a.sin(), z)
        })
        .collect()
}

#[test]
fn t5_level_open_end() {
    let world = empty_world();
    println!("| level shape | est reach mm | true reach <= mm | runs | max sink um | sink>0.1mm runs | crossings | cast: worst swept gap um (substep window) | (2 ms window) |");
    let mut shapes: Vec<(String, Vec<Vec3>, f32)> = Vec::new();
    for (rad, open) in [
        (0.04f32, 0.020f32),
        (0.04, 0.026),
        (0.04, 0.028),
        (0.04, 0.030),
        (0.04, 0.034),
        (0.04, 0.040),
        (0.06, 0.030),
        (0.06, 0.045),
        (0.1, 0.029),
        (0.1, 0.060),
        (0.5, 0.156),
    ] {
        shapes.push((
            format!("C ring r{:.0} opening {:.0}", rad * 1e3, open * 1e3),
            c_ring(rad, open, if rad > 0.3 { 0.04 } else { 0.01 }, 0.0),
            open / 2.0,
        ));
    }
    // "6" shape: a spiral whose end comes back to within `gap` of the body.
    for gap in [0.022f32, 0.029, 0.034, 0.05] {
        let n = 60;
        let pts: Vec<Vec3> = (0..=n)
            .map(|i| {
                let a = 7.5 * i as f32 / n as f32; // > one turn
                let r = 0.05 + gap * a / std::f32::consts::TAU;
                v(r * a.cos(), r * a.sin(), 0.0)
            })
            .collect();
        shapes.push((format!("level spiral lap gap {:.0}", gap * 1e3), pts, gap / 2.0));
    }
    // Teardrop with parallel legs (level).
    for d in [0.029f64, 0.034, 0.05] {
        let dense = teardrop_dense(d, 0.03, 0.07, 0.1);
        let pts: Vec<Vec3> = resample(&dense, 0.012, 0.0).iter().map(|p| p.as_vec3()).collect();
        shapes.push((format!("level teardrop legs {:.0}", d * 1e3), pts, d as f32 / 2.0));
    }
    for (name, pts, true_reach) in shapes {
        let Some(g) = try_guide(&pts, 0.05, false) else {
            println!("| {name} | rejected |");
            continue;
        };
        assert!(g.is_level());
        let guides = [g];
        let g = &guides[0];
        let brute = Brute::new(g, 48);
        let geo = Geometry { world: &world, guides: &guides, wireforms: &[] };
        let sp = g.spline();
        let max_t = sp.max_t();
        let ends = [sp.point(0.0), sp.point(max_t)];
        let mut rng = Lcg(2024);
        let (mut sink, mut deep_runs, mut crossed, mut runs) = (0.0f32, 0u32, 0u32, 0u32);
        let mut worst_desc = String::new();
        // interesting anchor points: the two ends and points of the curve near the other end
        let mut anchors: Vec<Vec3> = ends.to_vec();
        anchors.push(0.5 * (ends[0] + ends[1]));
        for e in ends {
            // nearest point of the far part of the curve to this end
            let mut bestd = f32::INFINITY;
            let mut bp = e;
            for (i, c) in brute.dense.iter().enumerate() {
                let t = i as f32 / brute.per as f32;
                let arc_far = (sp.point(t).distance(e) > 1e-9)
                    && ((t - 0.0).abs() > 0.25 * max_t || e != ends[0])
                    && ((t - max_t).abs() > 0.25 * max_t || e != ends[1]);
                if arc_far && c.distance(e) < bestd {
                    bestd = c.distance(e);
                    bp = *c;
                }
            }
            anchors.push(0.5 * (e + bp));
        }
        for case in 0..3000 {
            let profile = [STEEL, MINI, JUMBO, XL, STEEL][case % 5];
            let r = radius(profile);
            let anchor = anchors[case % anchors.len()];
            let a0 = rng.unit() * std::f32::consts::TAU;
            let start = anchor + v(a0.cos(), a0.sin(), 0.0) * rng.range(0.0, 0.07) + Vec3::Z * 0.025;
            if brute.distance(start).0 < r + 5.0e-5 {
                continue;
            }
            let a1 = rng.unit() * std::f32::consts::TAU;
            let aim = anchor + v(a1.cos(), a1.sin(), 0.0) * rng.range(0.0, 0.03) + Vec3::Z * 0.025;
            if aim.distance(start) < 1e-4 {
                continue;
            }
            let speed = 0.2 + 29.8 * rng.unit() * rng.unit();
            let vel = (aim - start).normalize() * speed;
            let mut s = if case % 2 == 0 { frictionless() } else { solver_no_gravity() };
            let h = s.add_ball(ball_at(profile, start, vel)).unwrap();
            let mut ev = BallEvents::new();
            let mut prev = start;
            runs += 1;
            let mut run_sink = 0.0f32;
            for _ in 0..80 {
                ev.clear();
                s.step(&geo, DT, &mut ev);
                let b = *s.ball(h).unwrap();
                let (d, _) = brute.distance(b.position);
                run_sink = run_sink.max(r - d);
                if brute.crosses(prev, b.position) {
                    crossed += 1;
                    if worst_desc.is_empty() {
                        worst_desc = format!("CROSS {profile:?} speed {speed:.2} start {start:?} vel {vel:?}");
                    }
                }
                prev = b.position;
                if d > r + 0.12 {
                    break;
                }
            }
            if run_sink > sink {
                sink = run_sink;
                if sink > 1.0e-4 {
                    worst_desc = format!("{profile:?} speed {speed:.2} start {start:?} vel {vel:?} friction {}", case % 2);
                }
            }
            if run_sink > 1.0e-4 {
                deep_runs += 1;
            }
        }
        // direct casts
        let mut worst = [f32::INFINITY; 2];
        for (wi, long) in [false, true].into_iter().enumerate() {
            let mut rng = Lcg(77);
            let mut cases = 0;
            let mut tries = 0;
            while cases < 3000 && tries < 200000 {
                tries += 1;
                let r = [0.0135f32, 0.0079, 0.0159, 0.0397][cases % 4];
                let anchor = anchors[tries % anchors.len()];
                let a0 = rng.unit() * std::f32::consts::TAU;
                let p0 = anchor + v(a0.cos(), a0.sin(), 0.0) * rng.range(0.0, 0.06) + Vec3::Z * 0.025;
                if brute.distance(p0).0 < r + 2.0e-5 {
                    continue;
                }
                cases += 1;
                let a1 = rng.unit() * std::f32::consts::TAU;
                let speed = 0.2 + 29.8 * rng.unit() * rng.unit();
                let vel = v(a1.cos(), a1.sin(), 0.0) * speed;
                // the solver's window: travel <= r/4 per sub-step
                let window = if long { 2.0e-3 } else { (0.25 * r / speed).min(1.0e-3) };
                let (mut time, mut end) = (0.0f32, window);
                for step in 0.. {
                    if step > 400 {
                        break;
                    }
                    match g.cast_sphere(p0 + vel * time, r, vel, window - time, None) {
                        None => break,
                        Some(GuideCast::Advance { time: dt }) => {
                            if dt <= 0.0 {
                                break;
                            }
                            time += dt;
                        }
                        Some(GuideCast::Contact(hit)) => {
                            end = time + hit.time;
                            break;
                        }
                    }
                }
                let samples = ((speed * end / 1.0e-4) as usize).clamp(20, 600);
                for k in 0..=samples {
                    let p = p0 + vel * (end * k as f32 / samples as f32);
                    worst[wi] = worst[wi].min(brute.distance(p).0 - r);
                }
            }
        }
        println!(
            "| {name} | {:.1} | {:.1} | {runs} | {:.1} | {deep_runs} | {crossed} | {:.1} | {:.1} | {} {}",
            g.reach() * 1e3,
            true_reach * 1e3,
            sink * 1e6,
            worst[0] * 1e6,
            worst[1] * 1e6,
            if sink > 1.0e-4 || crossed > 0 || worst[0] < -2.0e-5 { "FAIL" } else { "" },
            worst_desc
        );
    }
}

// ------------------------------------------------------------ T6: criteria

#[test]
fn t6_criterion1_sweep() {
    let world = floor_world();
    let geo = Geometry::new(&world);
    let mut worst = 0.0f32;
    for imp in [0.005f32, 0.02, 0.05, 0.1, 0.2, 0.4] {
        for ang in [0.0f32, 0.7, 1.9, 3.3, 5.0] {
            let impulse = v(ang.cos(), ang.sin(), 0.0) * imp;
            let mut speeds = [0.0f32; 2];
            for (k, p) in [POWERBALL, STEEL].into_iter().enumerate() {
                let mut s = solver();
                let r = radius(p);
                let h = s.add_ball(ball_at(p, v(0.0, 0.0, r), Vec3::ZERO)).unwrap();
                let mut ev = BallEvents::new();
                for _ in 0..50 {
                    s.step(&geo, DT, &mut ev);
                }
                s.apply_impulse(h, impulse);
                s.step(&geo, DT, &mut ev);
                speeds[k] = s.ball(h).unwrap().velocity.length();
            }
            let ratio = speeds[0] / speeds[1];
            println!("c1 imp={imp} ang={ang}: v_power={} v_steel={} ratio={ratio:.5}", speeds[0], speeds[1]);
            if imp >= 0.02 { worst = worst.max((ratio - 80.0 / 65.0).abs()); }
        }
    }
    println!("criterion 1: worst |ratio - 1.2308| = {worst:.5} (tol 0.01) {}", if worst > 0.01 { "FAIL" } else { "ok" });
    assert!(worst <= 0.01);
}

#[test]
fn t6_newton_sweep() {
    use pin_physics::ball::NewtonChain;
    let world = empty_world();
    let geo = Geometry::new(&world);
    let mut fails = 0;
    for n in 3..=8usize {
        for profile in [STEEL, MINI, JUMBO] {
            let r = radius(profile);
            let pts: Vec<Vec3> = (0..=10).map(|i| v(0.05 * i as f32, 0.0, r)).collect();
            let path = CubicSpline::interpolate(&pts).unwrap();
            // params so that arc gap is 0: spacing 2r along a straight path with 0.05 per unit t
            let params: Vec<f32> = (0..n).map(|i| 2.0 + (2.0 * r * i as f32) / 0.05).collect();
            for mode in 0..2 {
                let mut cfg = config();
                cfg.gravity = Vec3::ZERO;
                let mut s = BallSolver::new(cfg, &profiles());
                let chain = NewtonChain::new(path.clone(), profile, &params).unwrap();
                let ci = s.add_newton_chain(chain);
                let mut ev = BallEvents::new();
                s.step(&geo, DT, &mut ev);
                let mut hit_tick = None;
                if mode == 0 {
                    s.newton_chain_mut(ci).strike(0, 1.0);
                    hit_tick = Some(0);
                } else {
                    let p0 = s.newton_chain(ci).ball_position(0);
                    s.add_ball(ball_at(STEEL, p0 - v(r + radius(STEEL) + 0.0005, 0.0, 0.0), v(2.0, 0.0, 0.0)));
                }
                let mut reached = None;
                for tick in 0..10 {
                    ev.clear();
                    s.step(&geo, DT, &mut ev);
                    if hit_tick.is_none() && s.newton_chain(ci).ball_speed(0).abs() > 0.0
                        || ev.as_slice().len() > 0 && hit_tick.is_none()
                    {
                        hit_tick = Some(tick);
                    }
                    if s.newton_chain(ci).ball_speed(n - 1) > 0.0 {
                        reached = Some(tick);
                        break;
                    }
                }
                let ok = match (hit_tick, reached) {
                    (Some(a), Some(b)) => b <= a + 2,
                    _ => false,
                };
                if !ok {
                    fails += 1;
                    println!("newton n={n} {profile:?} mode {mode}: hit {hit_tick:?} reached {reached:?}");
                }
            }
        }
    }
    println!("criterion 3 sweep: {fails} failures of {}", 6 * 3 * 2);
    assert_eq!(fails, 0);
}

#[test]
fn t7_level_slit_detail() {
    use pin_physics::ball::BallEvent;
    let world = empty_world();
    // (1) minimal cast repro: sphere next to end A, cast over one solver sub-step window.
    for (rad, open) in [(0.04f32, 0.030f32), (0.04, 0.034), (0.06, 0.030), (0.1, 0.029)] {
        let g = try_guide(&c_ring(rad, open, 0.01, 0.0), 0.05, false).unwrap();
        let brute = Brute::new(&g, 64);
        let sp = g.spline();
        let a = sp.point(0.0);
        let r = 0.0135f32;
        let mut worst = 0.0f32;
        let mut desc = String::new();
        let mut rng = Lcg(1);
        for _ in 0..60000 {
            let ang = rng.unit() * std::f32::consts::TAU;
            let p0 = a + v(ang.cos(), ang.sin(), 0.0) * (r + rng.range(1.0e-4, 3.0e-3)) + Vec3::Z * 0.025;
            if brute.distance(p0).0 < r + 5.0e-5 {
                continue;
            }
            let a1 = rng.unit() * std::f32::consts::TAU;
            let speed = rng.range(0.5, 8.0);
            let vel = v(a1.cos(), a1.sin(), 0.0) * speed;
            let window = (0.25 * r / speed).min(1.0e-3);
            let res = g.cast_sphere(p0, r, vel, window, None);
            let end = match res {
                None => window,
                Some(GuideCast::Contact(h)) => h.time,
                Some(GuideCast::Advance { time }) => time,
            };
            let mut mingap = f32::INFINITY;
            for k in 0..=100 {
                let p = p0 + vel * (end * k as f32 / 100.0);
                mingap = mingap.min(brute.distance(p).0 - r);
            }
            if -mingap > worst {
                worst = -mingap;
                desc = format!("p0={p0:?} vel={vel:?} window={window:e} result={res:?} gap0={:e}", brute.distance(p0).0 - r);
            }
        }
        println!("ring r{:.0} opening {:.0}: est reach {:.1} mm; worst penetration during a 'free' sub-step cast = {:.3} mm; {desc}", rad * 1e3, open * 1e3, g.reach() * 1e3, worst * 1e3);
    }
    // (2) solver: balls fired at a slit narrower than the ball. Do they get through? KE gain?
    println!("| ring | opening mm | ball | runs | passed through slit | max sink mm | max rel KE gain | late (boundary) hits w/o Impact event |");
    for (rad, open) in [(0.04f32, 0.012f32), (0.04, 0.020), (0.04, 0.024), (0.04, 0.026), (0.06, 0.026), (0.1, 0.026), (0.1, 0.031)] {
        let g = try_guide(&c_ring(rad, open, 0.01, 0.0), 0.05, false).unwrap();
        let guides = [g];
        let g = &guides[0];
        let brute = Brute::new(g, 48);
        let geo = Geometry { world: &world, guides: &guides, wireforms: &[] };
        for profile in [STEEL, MINI, JUMBO] {
            let r = radius(profile);
            if 2.0 * r <= open {
                continue;
            }
            let mut rng = Lcg(99);
            let (mut passed, mut runs, mut sink, mut gain, mut silent) = (0u32, 0u32, 0.0f32, 0.0f64, 0u32);
            let mut example = String::new();
            for case in 0..1500 {
                // start outside the ring in front of the slit (slit is at +x), aim at the slit region
                let start = v(rad + r + rng.range(0.002, 0.05), rng.range(-0.03, 0.03), 0.025);
                if brute.distance(start).0 < r + 1.0e-4 {
                    continue;
                }
                let aim = v(rad * (1.0 - (open / (2.0 * rad)).powi(2)).sqrt(), rng.range(-0.6, 0.6) * open, 0.025);
                let speed = 0.2 + 29.8 * rng.unit() * rng.unit();
                let vel = (aim - start).normalize() * speed;
                let mut s = if case % 2 == 0 { frictionless() } else { solver_no_gravity() };
                let h = s.add_ball(ball_at(profile, start, vel)).unwrap();
                let mut ev = BallEvents::new();
                runs += 1;
                let mut ke_prev = kinetic_energy(&s);
                let mut through = false;
                for _ in 0..150 {
                    ev.clear();
                    s.step(&geo, DT, &mut ev);
                    let b = *s.ball(h).unwrap();
                    let (d, _) = brute.distance(b.position);
                    let impacts = ev.as_slice().iter().filter(|e| matches!(e, BallEvent::Impact { .. })).count();
                    if r - d > 1.0e-4 && impacts == 0 {
                        silent += 1;
                    }
                    sink = sink.max(r - d);
                    let ke = kinetic_energy(&s);
                    gain = gain.max((ke - ke_prev) / ke_prev.max(1e-12));
                    ke_prev = ke;
                    let rho = (b.position.x * b.position.x + b.position.y * b.position.y).sqrt();
                    if rho < rad - 0.5 * r {
                        through = true;
                    }
                }
                if through {
                    passed += 1;
                    if example.is_empty() {
                        example = format!("start {start:?} vel {vel:?} friction {}", case % 2);
                    }
                }
            }
            println!(
                "| r{:.0} | {:.0} | {profile:?} (d={:.1}mm) | {runs} | {passed} | {:.3} | {gain:.2e} | {silent} | {example}",
                rad * 1e3, open * 1e3, 2e3 * r, sink * 1e3
            );
        }
    }
}
