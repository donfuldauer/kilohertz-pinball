//! Seam task 2.2-S performance report (not a gate assertion: tests never
//! assert on wall-clock time). Run with
//! `cargo test -p pin_physics --release --test ball_seam_perf -- --ignored --nocapture`.

#[path = "ball_seam_ref.rs"]
mod reference;

use std::time::{Duration, Instant};

use pin_physics::ball::{BallEvents, Geometry};
use reference::*;

fn stats(mut samples: Vec<Duration>) -> (f64, f64, f64) {
    samples.sort();
    let us = |d: Duration| d.as_secs_f64() * 1e6;
    let mean = samples.iter().map(|d| us(*d)).sum::<f64>() / samples.len() as f64;
    let median = us(samples[samples.len() / 2]);
    let p99 = us(samples[samples.len() * 99 / 100]);
    (mean, median, p99)
}

/// Two rotating capsule bodies (flippers firing on a schedule), a kinematic
/// kicker and 4 balls on a walled table: µs per `step_with_bodies`
/// (target ≤ 50 µs mean). The same table stepped without bodies is printed
/// for comparison.
#[test]
#[ignore = "timing report, run in release"]
fn report_step_time_two_rotors_four_balls() {
    for with_bodies in [true, false] {
        let mut t = FlipperTable::new(4);
        let mut ev = BallEvents::new();
        let mut samples = Vec::with_capacity(20_000);
        for tick in 0..20_000u32 {
            t.drive(tick);
            ev.clear();
            let start = Instant::now();
            if with_bodies {
                t.step(&mut ev);
            } else {
                let geo = Geometry::new(&t.world);
                t.solver.step(&geo, 1.0e-3, &mut ev);
            }
            samples.push(start.elapsed());
        }
        let (mean, median, p99) = stats(samples);
        println!(
            "{} bodies, 4 balls: mean {mean:.2} µs, median {median:.2} µs, p99 {p99:.2} µs per step",
            if with_bodies {
                "2 rotors + kicker"
            } else {
                "no"
            }
        );
    }
}

/// A kinematic kicker slab (10–20 m/s strokes) pinching 1 or 3 resting
/// balls against a static wall `gap` behind them on a pitched floor
/// (Reviewer finding N1): µs per step, worst tick.
#[test]
#[ignore = "timing report, run in release"]
fn report_kicker_pinch() {
    use glam::Vec3;
    use pin_core::PhysicsDefaults;
    use pin_physics::ball::{BallSolver, BallSolverConfig};
    use pin_physics::world::{StaticShape, StaticWorldBuilder, layers};
    let cuboid = |c: Vec3, h: Vec3| StaticShape::Cuboid {
        center: c,
        half_extents: h,
        rotation: glam::Quat::IDENTITY,
    };
    for (speed, stroke, gap, balls) in [
        (5.0f32, 4u32, None, 1usize),
        (10.0, 2, Some(0.002f32), 1),
        (20.0, 1, Some(0.002), 1),
        (10.0, 2, Some(0.002), 3),
        (20.0, 1, Some(0.0), 3),
    ] {
        let pitch = 6.5f32.to_radians();
        let mut cfg = BallSolverConfig::from_defaults(&PhysicsDefaults::builtin());
        cfg.gravity = Vec3::new(0.0, -9.81 * libm::sinf(pitch), -9.81 * libm::cosf(pitch));
        let profiles: Vec<pin_core::BallProfile> = pin_core::profiles::builtin_profiles().collect();
        let r = profiles[0].radius_m();
        let mut s = BallSolver::new(cfg, &profiles);
        let mut wb = StaticWorldBuilder::new();
        wb.add_collider(
            cuboid(Vec3::new(0.0, 0.0, -0.05), Vec3::new(0.6, 0.6, 0.05)),
            layers::ALL,
            true,
        );
        if let Some(g) = gap {
            wb.add_collider(
                cuboid(
                    Vec3::new(2.0 * r + g + 0.01, 0.0, 0.03),
                    Vec3::new(0.01, 0.2, 0.03),
                ),
                layers::ALL,
                true,
            );
        }
        wb.add_collider(
            cuboid(Vec3::new(0.0, -0.045 - r, 0.03), Vec3::new(0.3, 0.03, 0.03)),
            layers::ALL,
            true,
        );
        let world = wb.build();
        let geo = Geometry::new(&world);
        let mut slab = Slab::one(SlabBody::new(
            Vec3::new(-0.01, 0.0, 0.02),
            Vec3::new(0.01, 0.05, 0.02),
            Vec3::ZERO,
        ));
        for k in 0..balls {
            let mut b = pin_physics::ball::Ball::new(
                pin_core::ids::ProfileId(0),
                Vec3::new(r + 0.0001, -0.015 + 0.0271 * k as f32, r),
            );
            b.velocity = Vec3::ZERO;
            s.add_ball(b).unwrap();
        }
        let mut ev = BallEvents::new();
        let mut samples = Vec::new();
        for tick in 0..500u32 {
            slab.bodies[0].velocity = if (200..200 + stroke).contains(&tick) {
                Vec3::new(speed, 0.0, 0.0)
            } else if (200 + stroke..200 + 3 * stroke).contains(&tick) {
                Vec3::new(-speed * 0.5, 0.0, 0.0)
            } else {
                Vec3::ZERO
            };
            ev.clear();
            let start = Instant::now();
            s.step_with_bodies(&geo, &mut [&mut slab], 1.0e-3, &mut ev);
            samples.push(start.elapsed());
        }
        let worst = samples.iter().max().unwrap().as_secs_f64() * 1e6;
        let (mean, _, p99) = stats(samples);
        println!(
            "kicker {speed} m/s x{stroke} wall gap {gap:?} balls {balls}: mean {mean:.1} µs, p99 {p99:.1} µs, worst {worst:.1} µs"
        );
    }
}
