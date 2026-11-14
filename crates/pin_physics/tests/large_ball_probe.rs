//! Temporary probe (2.8 fix round): overlap with a fast bat vs radius.
#[path = "flipper_util.rs"]
mod flip;
use glam::Vec3;
use pin_core::BallProfile;
use pin_core::ids::ProfileId;
use pin_physics::ball::{Ball, BallSolver, BallSolverConfig, MovingBodies};
use pin_physics::flipper::{FlipperSet, FlipperTuning};
use pin_physics::world::StaticWorldBuilder;

fn profile(d: f32, m: f32) -> BallProfile {
    let mut p = pin_core::profiles::builtin_profile("standard_steel").unwrap();
    p.diameter_m = d;
    p.mass_kg = m;
    p
}

fn worst(d: f32, m: f32, bat_h: f32, torque: f32) -> (f32, String) {
    let r = 0.5 * d;
    let mut w = (0.0f32, String::new());
    for fi in 0..12 { let fraction = 0.15 + 0.07 * fi as f32;
        for speed in [0.0f32, 6.0, 12.0, 16.2, 20.0] {
            for flip_at in 0u32..16 {
                let mut c = flip::left_config(0, [0.0; 3]);
                c.power_torque_nm = torque;
                c.inertia_kg_m2 = 5e-5;
                let mut rig = flip::Rig::with_world(&[c.clone()], StaticWorldBuilder::new().build(), Vec3::ZERO);
                let mut cfg: BallSolverConfig = BallSolverConfig::from_defaults(&pin_core::PhysicsDefaults::builtin());
                cfg.gravity = Vec3::ZERO;
                rig.solver = BallSolver::new(cfg, &[profile(d, m)]);
                let tuning = FlipperTuning { bat_height_m: bat_h, ..FlipperTuning::default() };
                rig.flippers = FlipperSet::with_tuning(&[c], tuning).unwrap();
                let dir = flip::bat_dir(-30.0);
                let n = Vec3::Z.cross(dir);
                let bat_r = 0.011 + (0.0065 - 0.011) * fraction;
                let start = dir * (flip::BAT_LEN * fraction) + n * (bat_r + r + 0.002 + speed * 0.0025) + Vec3::new(0.0, 0.0, r);
                let mut b = Ball::new(ProfileId(0), start);
                b.velocity = -n * speed;
                let h = rig.solver.add_ball(b).unwrap();
                for t in 0..60 {
                    if t == flip_at { rig.flippers.set_energised(flip::LEFT, true); }
                    rig.step();
                    let cp = rig.flippers.closest_point(0, 0.0, rig.ball(h).position).unwrap();
                    if r - cp.distance > w.0 {
                        w = (r - cp.distance, format!("frac {fraction} v {speed} flip {flip_at} tick {t} solves {:?}", rig.solver.body_solves_last_step()[0]));
                    }
                }
            }
        }
    }
    w
}

#[test]
#[ignore]
fn probe() {
    for (d, m, h) in [(0.0794f32, 0.445f32, 0.03f32), (0.05, 0.445, 0.06), (0.0794, 0.445, 0.1)] {
        for torque in [3.0f32] {
            let (p, s) = worst(d, m, h, torque);
            println!("d {d} m {m} h {h} T {torque}: {:.3} mm  {s}", p * 1e3);
        }
    }
}

#[test]
#[ignore]
fn trace() {
    let (d, m, bat_h, torque, fraction, speed, flip_at) = (
        std::env::var("D").map(|s| s.parse().unwrap()).unwrap_or(0.05f32), 0.445f32, 0.06f32, 3.0f32,
        std::env::var("F").map(|s| s.parse().unwrap()).unwrap_or(0.71f32),
        std::env::var("V").map(|s| s.parse().unwrap()).unwrap_or(20.0f32), 0u32);
    let r = 0.5 * d;
    let mut c = flip::left_config(0, [0.0; 3]);
    c.power_torque_nm = torque;
    c.inertia_kg_m2 = 5e-5;
    let mut rig = flip::Rig::with_world(&[c.clone()], StaticWorldBuilder::new().build(), Vec3::ZERO);
    let mut cfg: BallSolverConfig = BallSolverConfig::from_defaults(&pin_core::PhysicsDefaults::builtin());
    cfg.gravity = Vec3::ZERO;
    rig.solver = BallSolver::new(cfg, &[profile(d, m)]);
    let tuning = FlipperTuning { bat_height_m: bat_h.max(d + 0.01), ..FlipperTuning::default() };
    rig.flippers = FlipperSet::with_tuning(&[c], tuning).unwrap();
    let dir = flip::bat_dir(-30.0);
    let n = Vec3::Z.cross(dir);
    let bat_r = 0.011 + (0.0065 - 0.011) * fraction;
    let start = dir * (flip::BAT_LEN * fraction) + n * (bat_r + r + 0.002 + speed * 0.0025) + Vec3::new(0.0, 0.0, r);
    let mut b = Ball::new(ProfileId(0), start);
    b.velocity = -n * speed;
    let h = rig.solver.add_ball(b).unwrap();
    for t in 0..60 {
        if t == flip_at { rig.flippers.set_energised(flip::LEFT, true); }
        rig.step();
        let b = rig.ball(h);
        let cp = rig.flippers.closest_point(0, 0.0, b.position).unwrap();
        println!("t {t}: pen {:.3} angle {:.2} w {:.1} pos {} v {} solves {} hits {}", (r - cp.distance) * 1e3, rig.flippers.angle_deg(flip::LEFT), rig.flippers.angular_velocity(flip::LEFT), b.position, b.velocity, rig.solver.body_solves_last_step()[0], rig.hits.len());
    }
}

use pin_core::Material;
use pin_physics::ball::{BodyCast, BodyPoint, BallEvents, Geometry};
use std::cell::RefCell;

struct Log<'a> {
    inner: &'a mut FlipperSet,
    on: bool,
    r: f32,
    lines: RefCell<Vec<String>>,
}

impl MovingBodies for Log<'_> {
    fn body_count(&self) -> usize { self.inner.body_count() }
    fn begin_tick(&mut self, dt: f32) { self.inner.begin_tick(dt) }
    fn end_tick(&mut self, dt: f32) { self.inner.end_tick(dt) }
    fn layer_mask(&self, b: usize) -> u8 { self.inner.layer_mask(b) }
    fn material(&self, b: usize) -> Material { self.inner.material(b) }
    fn swept_bounds(&self, b: usize) -> (Vec3, Vec3) { self.inner.swept_bounds(b) }
    fn max_surface_speed(&self, b: usize) -> f32 { self.inner.max_surface_speed(b) }
    fn closest_point(&self, b: usize, t: f32, p: Vec3) -> Option<BodyPoint> {
        let c = self.inner.closest_point(b, t, p);
        if self.on { self.lines.borrow_mut().push(format!("cp t {t:.6} gap {:.4}mm", (c.unwrap().distance - self.r) * 1e3)); }
        c
    }
    fn velocity_at(&self, b: usize, t: f32, p: Vec3) -> Vec3 { let v = self.inner.velocity_at(b, t, p); if self.on { self.lines.borrow_mut().push(format!("vel t {t:.6} {v}")); } v }
    fn inverse_mass_at(&self, b: usize, t: f32, p: Vec3, d: Vec3) -> f32 { let w = self.inner.inverse_mass_at(b, t, p, d); if self.on { self.lines.borrow_mut().push(format!("invm t {t:.6} d {d} -> {w}")); } w }
    fn apply_impulse(&mut self, b: usize, t: f32, p: Vec3, j: Vec3) {
        if self.on { self.lines.borrow_mut().push(format!("imp t {t:.6} j {j}")); }
        self.inner.apply_impulse(b, t, p, j)
    }
    fn restitution(&self, b: usize, t: f32, p: Vec3, a: f32) -> Option<f32> { self.inner.restitution(b, t, p, a) }
    fn cast_sphere(&self, b: usize, t: f32, c: Vec3, r: f32, v: Vec3, w: f32) -> Option<BodyCast> {
        let res = self.inner.cast_sphere(b, t, c, r, v, w);
        if self.on {
            let gap = (self.inner.closest_point(b, t, c).unwrap().distance - r) * 1e3;
            self.lines.borrow_mut().push(format!("cast t {t:.6} win {w:.6} gap {gap:.4}mm -> {res:?}"));
        }
        res
    }
}

#[test]
#[ignore]
fn trace_calls() {
    let e = |k: &str, d: f32| std::env::var(k).map(|s| s.parse().unwrap()).unwrap_or(d);
    let (d, m, bat_h, torque, fraction, speed) = (e("D", 0.05), 0.445f32, e("H", 0.06), 3.0f32, e("F", 0.71), e("V", 20.0));
    let flip_at = e("FLIP", 0.0) as u32;
    let r = 0.5 * d;
    let mut c = flip::left_config(0, [0.0; 3]);
    c.power_torque_nm = torque;
    c.inertia_kg_m2 = 5e-5;
    let world = StaticWorldBuilder::new().build();
    let mut cfg: BallSolverConfig = BallSolverConfig::from_defaults(&pin_core::PhysicsDefaults::builtin());
    cfg.gravity = Vec3::ZERO;
    let mut solver = BallSolver::new(cfg, &[profile(d, m)]);
    let tuning = FlipperTuning { bat_height_m: bat_h, ..FlipperTuning::default() };
    let mut fl = FlipperSet::with_tuning(&[c], tuning).unwrap();
    let dir = flip::bat_dir(-30.0);
    let n = Vec3::Z.cross(dir);
    let bat_r = 0.011 + (0.0065 - 0.011) * fraction;
    let start = dir * (flip::BAT_LEN * fraction) + n * (bat_r + r + 0.002 + speed * 0.0025) + Vec3::new(0.0, 0.0, r);
    let mut b = Ball::new(ProfileId(0), start);
    b.velocity = -n * speed;
    let h = solver.add_ball(b).unwrap();
    let mut ev = BallEvents::new();
    let target: u32 = std::env::var("T").map(|s| s.parse().unwrap()).unwrap_or(42);
    for t in 0..=target {
        if t == flip_at { fl.set_energised(flip::LEFT, true); }
        fl.step_tick(t as u64);
        let mut lg = Log { inner: &mut fl, on: t == target, r, lines: RefCell::new(Vec::new()) };
        ev.clear();
        let geo = Geometry::new(&world);
        solver.step_with_bodies(&geo, &mut [&mut lg], flip::DT, &mut ev);
        if t == target {
            let lines = lg.lines.into_inner();
            println!("{} calls", lines.len());
            let mut last = String::new(); let mut rep = 0;
            for l in lines.into_iter().take(60) { let key: String = l.chars().take(4).collect(); if key == last { rep += 1; if rep < 6 { println!("{l}"); } } else { if rep >= 6 { println!("  ... x{rep}"); } rep = 0; last = key; println!("{l}"); } }
            if rep >= 6 { println!("  ... x{rep}"); }
        }
    }
    let bb = solver.ball(h).unwrap();
    println!("end pen {:.3}", (r - fl.closest_point(0, 0.0, bb.position).unwrap().distance) * 1e3);
}

#[test]
#[ignore]
fn probe_random() {
    let d = std::env::var("D").map(|s| s.parse().unwrap()).unwrap_or(0.0794f32);
    let r = 0.5 * d;
    let mut seed = 12345u64;
    let mut rnd = || { seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407); ((seed >> 33) as f32) / (1u64 << 31) as f32 };
    let mut w = (0.0f32, String::new());
    for _ in 0..3000 {
        let (ang, frac, speed, flip_at) = (rnd() * std::f32::consts::TAU, 0.1 + 0.85 * rnd(), 0.5 + 19.5 * rnd(), (rnd() * 40.0) as u32);
        let mut c = flip::left_config(0, [0.0; 3]);
        c.power_torque_nm = 3.0;
        c.inertia_kg_m2 = 5e-5;
        let mut rig = flip::Rig::with_world(&[c.clone()], StaticWorldBuilder::new().build(), Vec3::ZERO);
        let mut cfg: BallSolverConfig = BallSolverConfig::from_defaults(&pin_core::PhysicsDefaults::builtin());
        cfg.gravity = Vec3::ZERO;
        rig.solver = BallSolver::new(cfg, &[profile(d, 0.445)]);
        let target = flip::bat_dir(-5.0) * (flip::BAT_LEN * frac);
        let from = target + Vec3::new(ang.cos(), ang.sin(), 0.0) * (0.12 + r);
        let mut b = Ball::new(ProfileId(0), from + Vec3::new(0.0, 0.0, r));
        b.velocity = (target - from).normalize() * speed;
        let h = rig.solver.add_ball(b).unwrap();
        for t in 0..60u32 {
            if t == flip_at { rig.flippers.set_energised(flip::LEFT, true); }
            rig.step();
            let cp = rig.flippers.closest_point(0, 0.0, rig.ball(h).position).unwrap();
            if r - cp.distance > w.0 {
                w = (r - cp.distance, format!("ang {ang} frac {frac} v {speed} flip {flip_at} tick {t} solves {}", rig.solver.body_solves_last_step()[0]));
            }
        }
    }
    println!("d {d}: worst {:.3} mm {}", w.0 * 1e3, w.1);
}

#[test]
#[ignore]
fn trace_random() {
    let e = |k: &str, d: f32| std::env::var(k).map(|s| s.parse().unwrap()).unwrap_or(d);
    let (d, ang, frac, speed, flip_at, target_t) = (e("D", 0.0794), e("ANG", 4.164538), e("F", 0.31285307), e("V", 19.123137), e("FLIP", 12.0) as u32, e("T", 5.0) as u32);
    let r = 0.5 * d;
    let mut c = flip::left_config(0, [0.0; 3]);
    c.power_torque_nm = 3.0;
    c.inertia_kg_m2 = 5e-5;
    let world = StaticWorldBuilder::new().build();
    let mut cfg: BallSolverConfig = BallSolverConfig::from_defaults(&pin_core::PhysicsDefaults::builtin());
    cfg.gravity = Vec3::ZERO;
    let mut solver = BallSolver::new(cfg, &[profile(d, 0.445)]);
    let mut fl = FlipperSet::new(&[c]).unwrap();
    let target = flip::bat_dir(-5.0) * (flip::BAT_LEN * frac);
    let from = target + Vec3::new(ang.cos(), ang.sin(), 0.0) * (0.12 + r);
    let mut b = Ball::new(ProfileId(0), from + Vec3::new(0.0, 0.0, r));
    b.velocity = (target - from).normalize() * speed;
    let h = solver.add_ball(b).unwrap();
    let mut ev = BallEvents::new();
    for t in 0..=target_t {
        if t == flip_at { fl.set_energised(flip::LEFT, true); }
        fl.step_tick(t as u64);
        let on = t + 1 >= target_t;
        let mut lg = Log { inner: &mut fl, on, r, lines: RefCell::new(Vec::new()) };
        ev.clear();
        let geo = Geometry::new(&world);
        solver.step_with_bodies(&geo, &mut [&mut lg], flip::DT, &mut ev);
        let lines = lg.lines.into_inner();
        let bb = *solver.ball(h).unwrap();
        println!("== tick {t}: pen {:.3} angle {:.2} w {:.1} v {} calls {}", (r - fl.closest_point(0, 0.0, bb.position).unwrap().distance) * 1e3, fl.angle_deg(flip::LEFT), fl.angular_velocity(flip::LEFT), bb.velocity, lines.len());
        if on { for l in lines.iter().filter(|l| !l.starts_with("invm") && !l.starts_with("vel")).take(80) { println!("{l}"); } }
    }
}
