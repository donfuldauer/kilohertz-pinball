//! Independent spec tests (Subphase 2.2, gate step 6 / D9): 10 runs of a 5 s
//! scenario give identical final state bits (balls, Newton chain, sensor
//! switches, event stream). Also an `#[ignore]`d timing probe for the
//! performance target (4 balls, 300 colliders, ≤ 0.1 ms per step in release),
//! which only prints (tests never assert on wall-clock time, rule 5).

#[path = "spec_ball_common.rs"]
mod common;

use common::*;
use glam::{Quat, Vec3};
use pin_physics::ball::{BallEvent, ImpactTarget};
use pin_physics::world::{StaticShape, StaticWorldBuilder, layers};
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

fn hash_event(h: &mut DefaultHasher, e: &BallEvent) {
    match *e {
        BallEvent::Impact {
            ball,
            other,
            normal_speed,
        } => {
            0u8.hash(h);
            ball.hash(h);
            match other {
                ImpactTarget::Collider(c) => (0u8, c.index()).hash(h),
                ImpactTarget::Guide(i) => (1u8, i).hash(h),
                ImpactTarget::Wireform(i) => (2u8, i).hash(h),
                ImpactTarget::Ball(b) => (3u8, b.0 as usize).hash(h),
                ImpactTarget::Newton(i) => (4u8, i).hash(h),
            }
            normal_speed.to_bits().hash(h);
        }
        BallEvent::SensorEdge { switch, closed } => {
            1u8.hash(h);
            switch.hash(h);
            closed.hash(h);
        }
    }
}

/// Runs the 5 s scenario and returns (final-state digest, event-stream digest,
/// number of impact events).
fn run_once() -> (Vec<u32>, u64, usize) {
    let mut sc = scenario(None);
    let g = geo_full(&sc.world, &sc.guides, &sc.wireforms);
    let mut ev = events();
    let mut h = DefaultHasher::new();
    let mut impacts = 0;
    let mut sw = [0u64; 4];
    for tick in 0..5000u32 {
        sc.solver.set_magnet_power(sc.magnet, scenario_power(tick));
        ev.clear();
        sc.solver.step(&g, DT, &mut ev);
        for e in ev.as_slice() {
            hash_event(&mut h, e);
            impacts += usize::from(matches!(e, BallEvent::Impact { .. }));
        }
        ev.dropped().hash(&mut h);
        sc.solver.sensor_switches(&mut sw);
        sw.hash(&mut h);
    }
    let mut bits = Vec::new();
    for (handle, b) in sc.solver.balls() {
        bits.push(u32::from(handle.0));
        bits.extend_from_slice(&state_bits(b));
        bits.push(u32::from(b.layer));
    }
    let c = sc.solver.newton_chain(0);
    for i in 0..c.len() {
        bits.push(c.ball_t(i).to_bits());
        bits.push(c.ball_speed(i).to_bits());
    }
    (bits, h.finish(), impacts)
}

#[test]
fn ten_runs_of_5s_scenario_are_bit_identical() {
    let first = run_once();
    assert!(first.2 > 0, "scenario produced no impacts");
    for run in 1..10 {
        let r = run_once();
        assert_eq!(r.0, first.0, "run {run}: final state differs");
        assert_eq!(r.1, first.1, "run {run}: event stream differs");
    }
}

#[test]
fn scenario_stays_physical() {
    // All balls end on (or above) the pitched playfield, finite, inside the walls.
    let mut sc = scenario(None);
    let g = geo_full(&sc.world, &sc.guides, &sc.wireforms);
    let mut ev = events();
    let rot = Quat::from_rotation_x(6.5f32.to_radians());
    let n = rot * Vec3::Z;
    for tick in 0..5000u32 {
        sc.solver.set_magnet_power(sc.magnet, scenario_power(tick));
        ev.clear();
        sc.solver.step(&g, DT, &mut ev);
        for (_, b) in sc.solver.balls() {
            assert!(b.position.is_finite() && b.velocity.is_finite());
            assert!(b.angular_velocity.is_finite() && b.orientation.is_finite());
            let r = profiles()[b.profile.index()].radius_m();
            let local = rot.inverse() * b.position;
            // Above the playfield top face (z_local = 0) minus a small tolerance.
            if local.x.abs() < 0.3 && local.y.abs() < 0.6 {
                assert!(
                    b.position.dot(n) >= r - 1e-3,
                    "tick {tick}: ball sank into the playfield: local {local:?}"
                );
            }
        }
    }
}

/// 300 colliders: a floor plus a grid of posts and wall segments.
#[test]
#[ignore = "timing probe: run with --release -- --ignored --nocapture"]
fn perf_probe_4_balls_300_colliders() {
    let mut b = StaticWorldBuilder::new();
    b.add_collider(floor_shape(), layers::ALL, true);
    let mut count = 1;
    'outer: for i in 0..20 {
        for j in 0..15 {
            if count == 300 {
                break 'outer;
            }
            let p = Vec3::new(-0.5 + i as f32 * 0.05, -0.4 + j as f32 * 0.06, 0.0);
            if (i + j) % 3 == 0 {
                b.add_collider(
                    StaticShape::Cuboid {
                        center: p + Vec3::new(0.0, 0.0, 0.02),
                        half_extents: Vec3::new(0.01, 0.005, 0.02),
                        rotation: Quat::from_rotation_z(0.3 * (i as f32)),
                    },
                    layers::ALL,
                    true,
                );
            } else {
                b.add_collider(
                    StaticShape::Cylinder {
                        base: p,
                        axis: Vec3::new(0.0, 0.0, 0.04),
                        radius: 0.004,
                    },
                    layers::ALL,
                    true,
                );
            }
            count += 1;
        }
    }
    let world = b.build();
    assert_eq!(world.collider_count(), 300);
    let g = geo(&world);
    let mut s = solver();
    for k in 0..4 {
        let mut bl = ball(
            "standard_steel",
            Vec3::new(-0.45 + 0.2 * k as f32, -0.37, 0.0135),
        );
        bl.velocity = Vec3::new(0.3 * k as f32 - 0.4, 2.5, 0.0);
        s.add_ball(bl).unwrap();
    }
    let mut ev = events();
    let n = 2000;
    let t0 = std::time::Instant::now();
    for _ in 0..n {
        ev.clear();
        s.step(&g, DT, &mut ev);
    }
    let per = t0.elapsed().as_secs_f64() / f64::from(n) * 1e3;
    println!("PERF: {per:.4} ms per step (4 balls, 300 colliders, target <= 0.1 ms)");
}
