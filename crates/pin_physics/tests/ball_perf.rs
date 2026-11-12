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
