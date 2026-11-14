//! Performance report (not a gate assertion: tests never assert on
//! wall-clock time). Run with
//! `cargo test -p pin_physics --release --test ball_perf -- --ignored --nocapture`.

#[path = "ball_util.rs"]
mod util;

use pin_core::ids::ProfileId;
use pin_physics::ball::{BallEvents, Geometry};
use pin_physics::world::{StaticShape, StaticWorldBuilder, layers};
use util::*;

/// One `step` with 4 balls in a 300-collider world (target ≤ 0.1 ms).
#[test]
#[ignore = "timing report, run in release"]
fn report_step_time_four_balls_300_colliders() {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(
        cuboid(v(0.0, 0.0, -0.02), v(0.3, 0.6, 0.02)),
        layers::ALL,
        true,
    );
    for (c, he) in [
        (v(0.31, 0.0, 0.05), v(0.01, 0.62, 0.1)),
        (v(-0.31, 0.0, 0.05), v(0.01, 0.62, 0.1)),
        (v(0.0, 0.61, 0.05), v(0.32, 0.01, 0.1)),
        (v(0.0, -0.61, 0.05), v(0.32, 0.01, 0.1)),
        (v(0.0, 0.0, 0.16), v(0.32, 0.62, 0.01)),
    ] {
        b.add_collider(cuboid(c, he), layers::ALL, true);
    }
    // 294 posts on a grid.
    for k in 0..294 {
        let (ix, iy) = (k % 14, k / 14);
        b.add_collider(
            StaticShape::Cylinder {
                base: v(-0.26 + 0.04 * ix as f32, -0.55 + 0.055 * iy as f32, 0.0),
                axis: v(0.0, 0.0, 0.03),
                radius: 0.004,
            },
            layers::ALL,
            true,
        );
    }
    let world = b.build();
    assert_eq!(world.collider_count(), 300);
    let geo = Geometry::new(&world);
    let mut s = solver();
    for k in 0..4u16 {
        s.add_ball(ball_at(
            ProfileId(k),
            v(-0.2 + 0.13 * f32::from(k), 0.02, 0.05),
            v(1.5 - f32::from(k), 2.0, 0.0),
        ))
        .unwrap();
    }
    let mut ev = BallEvents::new();
    let ticks = 20_000u32;
    let mut worst = std::time::Duration::ZERO;
    let start = std::time::Instant::now();
    for tick in 0..ticks {
        ev.clear();
        if tick % 500 == 0 {
            // Keep the balls lively.
            for (k, kick) in [
                v(0.2, 0.15, 0.0),
                v(-0.15, 0.2, 0.0),
                v(0.1, -0.2, 0.0),
                v(-0.05, -0.05, 0.0),
            ]
            .into_iter()
            .enumerate()
            {
                s.apply_impulse(pin_physics::ball::BallHandle(k as u8), kick);
            }
        }
        let t0 = std::time::Instant::now();
        s.step(&geo, DT, &mut ev);
        worst = worst.max(t0.elapsed());
    }
    let mean = start.elapsed() / ticks;
    println!("ball step, 4 balls / 300 colliders: mean {mean:?}, worst {worst:?}");
}

/// Resting clusters are the expensive case of the joint contact solve (up
/// to 64 sweeps): rows of touching balls on a pitched floor against a wall.
#[test]
#[ignore = "timing report, run in release"]
fn report_step_time_resting_rows() {
    for count in [4usize, 6, 16] {
        let mut b = StaticWorldBuilder::new();
        b.add_collider(
            cuboid(v(0.0, 0.0, -0.05), v(2.0, 2.0, 0.05)),
            layers::ALL,
            true,
        );
        b.add_collider(
            cuboid(v(-0.05, 0.0, 0.05), v(0.05, 2.0, 0.05)),
            layers::ALL,
            true,
        );
        let world = b.build();
        let geo = Geometry::new(&world);
        let mut cfg = config();
        cfg.gravity = v(-1.11, 0.0, -9.75);
        let mut s = pin_physics::ball::BallSolver::new(cfg, &profiles());
        let r = radius(STEEL);
        for k in 0..count {
            s.add_ball(ball_at(
                STEEL,
                v(r + 2.0 * r * k as f32, 0.0, r),
                glam::Vec3::ZERO,
            ))
            .unwrap();
        }
        let mut ev = BallEvents::new();
        let ticks = 5_000u32;
        let start = std::time::Instant::now();
        for _ in 0..ticks {
            ev.clear();
            s.step(&geo, DT, &mut ev);
        }
        println!(
            "ball step, row of {count} resting balls: mean {:?}",
            start.elapsed() / ticks
        );
    }
}

fn median(mut samples: Vec<std::time::Duration>) -> std::time::Duration {
    samples.sort_unstable();
    samples[samples.len() / 2]
}

/// The steep arc of the review: radius 50 mm, 3 rad, rising 450 mm
/// (rise 3:1), 60 segments; `rise = 0` gives its level twin.
fn arc_guide(rise: f32) -> pin_physics::world::GuideCurve {
    use pin_physics::world::{CubicSpline, GuideCurve};
    let pts: Vec<glam::Vec3> = (0..=60)
        .map(|i| {
            let s = i as f32 / 60.0;
            let a = 3.0 * s;
            v(0.05 * libm::sinf(a), 0.05 - 0.05 * libm::cosf(a), rise * s)
        })
        .collect();
    GuideCurve::new(CubicSpline::interpolate(&pts).unwrap(), 0.4).unwrap()
}

/// Cost of rising guides against level ones (fix round 3 targets: closest
/// point and cast ≤ 3 µs median on a 60-segment sloped guide; one ball
/// sliding along the 50 mm 3:1 arc ≤ 20 µs/step mean, ≤ 100 µs worst; four
/// balls ≤ 60 µs mean, ≤ 350 µs worst).
#[test]
#[ignore = "timing report, run in release"]
fn report_sloped_guide_costs() {
    use glam::Vec3;
    use std::hint::black_box;
    use std::time::Instant;
    let r = radius(STEEL);
    for (label, rise) in [
        ("level arc r=50mm", 0.0f32),
        ("rising arc r=50mm 3:1", 0.45),
    ] {
        let g = arc_guide(rise);
        assert_eq!(g.spline().segment_count(), 60);
        // Query points: a ball touching the wall on either side, at heights
        // inside, at the edges of and beyond the wall.
        let mut queries = Vec::new();
        for k in 0..4000 {
            let s = 0.05 + 0.9 * (k % 997) as f32 / 997.0;
            let a = 3.0 * s;
            let c = v(0.05 * libm::sinf(a), 0.05 - 0.05 * libm::cosf(a), rise * s);
            let nrm = v(-libm::sinf(a), libm::cosf(a), 0.0);
            let tan = v(libm::cosf(a), libm::sinf(a), rise / 0.15);
            let side = if k % 2 == 0 { 1.0 } else { -1.0 };
            let height = match k % 8 {
                0 => -0.01,
                1 => 0.41,
                _ => 0.02 + 0.36 * (k % 13) as f32 / 13.0,
            };
            queries.push((
                c + nrm * side * (r + 2.0e-5) + Vec3::Z * height,
                tan.normalize() * 5.0,
            ));
        }
        let mut closest = Vec::new();
        let mut closest_seeded = Vec::new();
        let mut cast = Vec::new();
        let mut seed = None;
        for _ in 0..5 {
            for (p, vel) in &queries {
                let t0 = Instant::now();
                let cp = black_box(g.closest_point(black_box(*p), None));
                closest.push(t0.elapsed());
                let t0 = Instant::now();
                black_box(g.closest_point(black_box(*p), seed));
                closest_seeded.push(t0.elapsed());
                seed = Some(cp.t);
                let t0 = Instant::now();
                black_box(g.cast_sphere(black_box(*p), r, *vel, 1.0e-3, None));
                cast.push(t0.elapsed());
            }
        }
        println!(
            "{label}: closest_point median {:?} (seeded {:?}), cast_sphere median {:?}",
            median(closest),
            median(closest_seeded),
            median(cast)
        );
    }

    // Balls sliding along the rising arc: frictionless, no gravity, started
    // beside the wall and following its slope; a run ends when the first
    // ball reaches the far end.
    let world = empty_world();
    let guides = [arc_guide(0.45)];
    let geo = Geometry {
        world: &world,
        guides: &guides,
        wireforms: &[],
    };
    for count in [1usize, 4] {
        // The scenario is deterministic, so it is timed three times and every
        // step keeps its fastest time: what remains of a spike is the solver's
        // own cost, not the machine's (other processes share the cores).
        let mut fastest: Vec<std::time::Duration> = Vec::new();
        let (mut ticks, mut slow) = (0u32, 0u32);
        for repeat in 0..3 {
            let mut steps = Vec::new();
            (ticks, slow) = (0, 0);
            for run in 0..400 {
                let mut cfg = config();
                cfg.gravity = Vec3::ZERO;
                for p in &mut cfg.pairs {
                    p.friction = 0.0;
                }
                let mut s = pin_physics::ball::BallSolver::new(cfg, &profiles());
                let speed = 2.0 + 6.0 * (run % 7) as f32 / 6.0;
                for k in 0..count {
                    // Two on each side of the wall, 40 mm apart along it.
                    let side = if k % 2 == 0 { 1.0 } else { -1.0 };
                    let a = 0.15 + 0.8 * (k / 2) as f32;
                    let c = v(0.05 * libm::sinf(a), 0.05 - 0.05 * libm::cosf(a), 0.15 * a);
                    let nrm = v(-libm::sinf(a), libm::cosf(a), 0.0);
                    let along = v(libm::cosf(a), libm::sinf(a), 3.0).normalize();
                    let start = c + nrm * side * (r + 1.0e-4) + Vec3::Z * 0.2;
                    let vel = (along - nrm * side * 0.02).normalize() * speed;
                    s.add_ball(ball_at(STEEL, start, vel)).unwrap();
                }
                let mut ev = BallEvents::new();
                let mut prev: Vec<Vec3> = s.balls().map(|(_, b)| b.position).collect();
                for _ in 0..400 {
                    ev.clear();
                    let t0 = Instant::now();
                    s.step(&geo, DT, &mut ev);
                    steps.push(t0.elapsed());
                    let mut done = false;
                    for ((_, b), last) in s.balls().zip(prev.iter_mut()) {
                        let beside = guides[0].closest_point(b.position, None);
                        done |= beside.t > 57.0 || beside.distance > 0.03;
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
                for (best, t) in fastest.iter_mut().zip(steps) {
                    *best = (*best).min(t);
                }
            }
        }
        let steps = fastest;
        let total: std::time::Duration = steps.iter().sum();
        let mean = total / steps.len() as u32;
        let mut sorted = steps.clone();
        sorted.sort_unstable();
        let at = |q: f64| sorted[((sorted.len() - 1) as f64 * q) as usize];
        println!(
            "{count} ball(s) sliding along the rising arc: mean {mean:?}, median {:?}, p99 {:?}, p99.9 {:?}, worst {:?} per step (fastest of 3 identical repeats) over {} steps; ball-ticks below 95 % of v·dt: {slow} of {ticks}",
            at(0.5),
            at(0.99),
            at(0.999),
            at(1.0),
            steps.len()
        );
    }
}
