//! Independent spec tests (Subphase 2.2): FIXED API surface — ball storage,
//! handles, dispensers, snapshot conversion, event list and ball–ball impacts.

#[path = "spec_ball_common.rs"]
mod common;

use common::*;
use glam::{Quat, Vec3};
use pin_core::ProfileId;
use pin_core::snapshot::BallState;
use pin_physics::ball::{
    Ball, BallEvent, BallHandle, BallSolver, Dispenser, ImpactTarget, MAX_BALLS,
};

#[test]
fn max_balls_matches_snapshot_capacity() {
    assert_eq!(MAX_BALLS, 16);
    assert_eq!(MAX_BALLS, pin_core::snapshot::MAX_BALLS);
}

#[test]
fn ball_new_defaults() {
    let b = Ball::new(ProfileId(3), Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(b.position, Vec3::new(1.0, 2.0, 3.0));
    assert_eq!(b.velocity, Vec3::ZERO);
    assert_eq!(b.angular_velocity, Vec3::ZERO);
    assert_eq!(b.orientation, Quat::IDENTITY);
    assert_eq!(b.profile, ProfileId(3));
    assert_eq!(b.layer, 0);
}

#[test]
fn add_remove_and_iterate_in_handle_order() {
    let mut s = solver();
    let mut hs = Vec::new();
    for i in 0..MAX_BALLS {
        let h = s
            .add_ball(ball("standard_steel", Vec3::new(i as f32, 0.0, 0.0)))
            .expect("room for 16 balls");
        hs.push(h);
    }
    assert!(s.add_ball(ball("standard_steel", Vec3::ZERO)).is_none(), "17th ball accepted");
    // Distinct handles.
    let mut sorted = hs.clone();
    sorted.sort();
    sorted.dedup();
    assert_eq!(sorted.len(), MAX_BALLS);
    // Iteration is in ascending handle order.
    let it: Vec<BallHandle> = s.balls().map(|(h, _)| h).collect();
    assert_eq!(it, sorted);
    // Remove one: returns the ball, lookups fail afterwards, room again.
    let victim = hs[5];
    let removed = s.remove_ball(victim).unwrap();
    assert_eq!(removed.position, Vec3::new(5.0, 0.0, 0.0));
    assert!(s.ball(victim).is_none());
    assert!(s.ball_mut(victim).is_none());
    assert!(s.remove_ball(victim).is_none());
    assert_eq!(s.balls().count(), MAX_BALLS - 1);
    let h = s.add_ball(ball("mini_steel", Vec3::splat(9.0))).unwrap();
    assert_eq!(s.ball(h).unwrap().position, Vec3::splat(9.0));
    let it: Vec<BallHandle> = s.balls().map(|(h, _)| h).collect();
    assert!(it.windows(2).all(|w| w[0] < w[1]));
    // ball_mut edits stick.
    s.ball_mut(hs[0]).unwrap().velocity = Vec3::X;
    assert_eq!(s.ball(hs[0]).unwrap().velocity, Vec3::X);
    // Unknown handle.
    assert!(s.ball(BallHandle(200)).is_none());
}

#[test]
fn ball_state_conversion() {
    let mut b = Ball::new(ProfileId(2), Vec3::new(0.1, 0.2, 0.3));
    b.velocity = Vec3::new(1.0, -2.0, 3.0);
    b.angular_velocity = Vec3::new(-4.0, 5.0, -6.0);
    b.orientation = Quat::from_xyzw(0.5, 0.5, 0.5, 0.5);
    b.layer = 3;
    let s = BallState::from(&b);
    assert_eq!(s.position_m, [0.1, 0.2, 0.3]);
    assert_eq!(s.velocity_m_s, [1.0, -2.0, 3.0]);
    assert_eq!(s.angular_velocity_rad_s, [-4.0, 5.0, -6.0]);
    assert_eq!(s.orientation, [0.5, 0.5, 0.5, 0.5]);
    let q = Quat::from_rotation_z(0.7);
    b.orientation = q;
    assert_eq!(BallState::from(&b).orientation, [q.x, q.y, q.z, q.w]);
    assert_eq!(s.profile, ProfileId(2));
    assert_eq!(s.layer, 3);
}

#[test]
fn dispenser_fifo_capacity_and_dispense() {
    let exit_p = Vec3::new(0.3, -0.5, 0.02);
    let exit_v = Vec3::new(0.0, 1.5, 0.0);
    let mut d = Dispenser::new(exit_p, exit_v);
    assert!(d.is_empty());
    assert_eq!(d.len(), 0);
    assert_eq!(d.pop(), None);
    for i in 0..16u16 {
        assert!(d.push(ProfileId(i % 7)), "push {i} refused");
    }
    assert!(!d.push(ProfileId(0)), "17th push accepted");
    assert_eq!(d.len(), 16);
    for i in 0..16u16 {
        assert_eq!(d.pop(), Some(ProfileId(i % 7)));
    }
    assert!(d.is_empty());

    let mut s = solver();
    let mut empty = Dispenser::new(exit_p, exit_v);
    assert!(empty.dispense(&mut s).is_none());
    d.push(pid("ceramic_powerball"));
    d.push(pid("mini_steel"));
    let h = d.dispense(&mut s).unwrap();
    let b = *s.ball(h).unwrap();
    assert_eq!(b.profile, pid("ceramic_powerball"));
    assert_eq!(b.position, exit_p);
    assert_eq!(b.velocity, exit_v);
    assert_eq!(d.len(), 1);
    let h2 = d.dispense(&mut s).unwrap();
    assert_ne!(h, h2);
    assert_eq!(s.ball(h2).unwrap().profile, pid("mini_steel"));
    assert!(d.is_empty());
}

#[test]
fn dispense_into_full_solver_returns_none() {
    let mut s = solver();
    for i in 0..MAX_BALLS {
        s.add_ball(ball("standard_steel", Vec3::new(i as f32, 0.0, 0.0)))
            .unwrap();
    }
    let mut d = Dispenser::new(Vec3::ZERO, Vec3::ZERO);
    d.push(ProfileId(0));
    assert!(d.dispense(&mut s).is_none());
    assert_eq!(s.balls().count(), MAX_BALLS);
}

#[test]
fn events_start_empty_and_clear() {
    let mut ev = events();
    assert!(ev.as_slice().is_empty());
    assert_eq!(ev.dropped(), 0);
    // Produce an impact, then clear.
    let world = floor_world();
    let g = geo(&world);
    let mut s = solver_with_gravity(Vec3::ZERO);
    let mut b = ball("standard_steel", Vec3::new(0.0, 0.0, 0.0140));
    b.velocity = Vec3::new(0.0, 0.0, -1.0);
    s.add_ball(b).unwrap();
    s.step(&g, DT, &mut ev);
    assert_eq!(ev.as_slice().len(), 1);
    ev.clear();
    assert!(ev.as_slice().is_empty());
    assert_eq!(ev.dropped(), 0);
}

#[test]
fn slow_impacts_are_not_reported() {
    let world = floor_world();
    let g = geo(&world);
    for (speed, reported) in [(0.03f32, false), (0.2, true)] {
        let mut s = solver_with_gravity(Vec3::ZERO);
        let mut b = ball("standard_steel", Vec3::new(0.0, 0.0, 0.0135 + 0.00001));
        b.velocity = Vec3::new(0.0, 0.0, -speed);
        s.add_ball(b).unwrap();
        let mut ev = events();
        for _ in 0..3 {
            s.step(&g, DT, &mut ev);
        }
        let n = ev
            .as_slice()
            .iter()
            .filter(|e| matches!(e, BallEvent::Impact { .. }))
            .count();
        assert_eq!(n > 0, reported, "impact at {speed} m/s: {:?}", ev.as_slice());
    }
}

#[test]
fn ball_ball_impact_event_and_no_overlap() {
    let world = empty_world();
    let g = geo(&world);
    let mut s: BallSolver = solver_with_gravity(Vec3::ZERO);
    let mut a = ball("standard_steel", Vec3::new(-0.0140, 0.0, 0.0));
    a.velocity = Vec3::new(1.0, 0.0, 0.0);
    let ha = s.add_ball(a).unwrap();
    let hb = s.add_ball(ball("standard_steel", Vec3::new(0.0140, 0.0, 0.0))).unwrap();
    let mut ev = events();
    let mut seen = false;
    for _ in 0..5 {
        ev.clear();
        s.step(&g, DT, &mut ev);
        for e in ev.as_slice() {
            if let BallEvent::Impact {
                ball,
                other: ImpactTarget::Ball(o),
                normal_speed,
            } = *e
            {
                assert!(
                    (ball == ha && o == hb) || (ball == hb && o == ha),
                    "bad pair {e:?}"
                );
                assert!((normal_speed - 1.0).abs() < 0.02, "normal speed {normal_speed}");
                seen = true;
            }
        }
    }
    assert!(seen, "no ball-ball impact event");
    // Equal masses, steel–steel restitution 0.6: v_a = 0.2, v_b = 0.8.
    let va = s.ball(ha).unwrap().velocity.x;
    let vb = s.ball(hb).unwrap().velocity.x;
    assert!((va - 0.2).abs() < 0.02 && (vb - 0.8).abs() < 0.02, "va {va} vb {vb}");
    let d = s.ball(hb).unwrap().position - s.ball(ha).unwrap().position;
    assert!(d.length() >= 0.027 - 1e-5);
}

#[test]
fn event_list_caps_at_64_and_counts_dropped() {
    // 16 balls each hit a wall in the same tick, 5 ticks in a row of
    // bounces between two close walls; more than 64 impacts may occur in one
    // tick only if the solver reports many — we only check the invariant.
    let world = floor_world();
    let g = geo(&world);
    let mut s = solver_with_gravity(Vec3::ZERO);
    for i in 0..MAX_BALLS {
        let mut b = ball("mini_steel", Vec3::new(i as f32 * 0.05, 0.0, 0.0080));
        b.velocity = Vec3::new(0.0, 0.0, -2.0);
        s.add_ball(b).unwrap();
    }
    let mut ev = events();
    s.step(&g, DT, &mut ev);
    assert!(ev.as_slice().len() <= 64);
    assert_eq!(ev.as_slice().len(), 16);
    assert_eq!(ev.dropped(), 0);
    // Without clearing, keep accumulating: length saturates at 64 and the
    // overflow is counted.
    for _ in 0..100 {
        for (_, b) in s.balls() {
            let _ = b;
        }
        let hs: Vec<_> = s.balls().map(|(h, _)| h).collect();
        for h in hs {
            let b = s.ball_mut(h).unwrap();
            b.position.z = 0.0080;
            b.velocity = Vec3::new(0.0, 0.0, -2.0);
        }
        s.step(&g, DT, &mut ev);
    }
    assert_eq!(ev.as_slice().len(), 64);
    assert!(ev.dropped() > 0);
}
