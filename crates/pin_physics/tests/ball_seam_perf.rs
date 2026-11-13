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
            if with_bodies { "2 rotors + kicker" } else { "no" }
        );
    }
}
