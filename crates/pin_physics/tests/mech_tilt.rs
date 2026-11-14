//! Verification 4 (tilt closure above a nudge threshold, none below) and the
//! nudge on the balls, Subphase 2.4.

#[path = "mech_util.rs"]
mod util;

use glam::Vec3;
use pin_core::PhysicsDefaults;
use pin_core::SwitchEdge;
use pin_core::ids::SwitchId;
use pin_physics::mech::{MechSet, TiltBob, TiltBobParams};
use util::*;

/// Whether one nudge of `dv` from rest closes the tilt switch within 2 s.
fn closes(dv: Vec3) -> bool {
    let mut bob = TiltBob::new(TiltBobParams::default());
    bob.nudge(dv);
    (0..2000).any(|_| {
        bob.step(DT);
        bob.closed()
    })
}

#[test]
fn tilt_threshold_is_single_and_monotone() {
    let p = TiltBobParams::default();
    let analytic = p.gap * libm::sqrtf(9.81 / p.length);
    let mut thresholds = Vec::new();
    for k in 0..8 {
        let a = k as f32 * core::f32::consts::TAU / 8.0 + 0.2;
        let dir = Vec3::new(libm::cosf(a), libm::sinf(a), 0.0);
        let mut threshold = None;
        // 0.002 m/s steps from 0.002 to 0.4 m/s.
        for i in 1..=200 {
            let m = i as f32 * 0.002;
            let c = closes(dir * m);
            match (c, threshold) {
                (true, None) => threshold = Some(m),
                (false, Some(t)) => panic!("{m} m/s does not close after {t} m/s did"),
                _ => {}
            }
        }
        thresholds.push(threshold.expect("closes at some nudge"));
    }
    let lo = thresholds.iter().copied().fold(f32::INFINITY, f32::min);
    let hi = thresholds.iter().copied().fold(0.0, f32::max);
    println!(
        "tilt threshold over 8 directions: {lo:.3}..{hi:.3} m/s (undamped small-swing estimate gap·√(g/L) = {analytic:.4})"
    );
    assert!(hi - lo <= 0.0021, "same threshold in every direction");
    assert!(lo > 0.9 * analytic && hi < 1.2 * analytic);
    // A vertical nudge never tilts (the rod takes it).
    assert!(!closes(Vec3::new(0.0, 0.0, 1.0)));
}

#[test]
fn mech_set_tilt_switch_and_ball_nudge() {
    let mut t = Table::new(&[], world(&[]), 0.0);
    t.mechs.set_tilt_switch(Some(SwitchId(8)));
    let h = t.ball(0.0, 0.0, Vec3::ZERO);
    for _ in 0..10 {
        t.tick();
    }
    // Below the threshold: the ball moves, the bob does not reach the ring.
    t.mechs.nudge(Vec3::new(0.0, 0.05, 0.0));
    t.tick();
    let vel = t.get(h).velocity;
    assert!((vel.y + 0.05).abs() < 0.01, "ball sees −Δv: {vel:?}");
    let mut any = false;
    for _ in 0..1500 {
        t.tick();
        any |= t.closed(8);
    }
    assert!(!any);
    assert!(t.mechs.tilt_bob_offset().length() < PhysicsDefaults::builtin().mechs.tilt_bob.gap);
    // Above: the tilt switch closes and reports edges.
    t.mechs.nudge(Vec3::new(0.3, 0.0, 0.0));
    let mut closed_edge = false;
    let mut opened_edge = false;
    for _ in 0..1500 {
        t.tick();
        closed_edge |= t.mechs.edges().contains(&(SwitchId(8), SwitchEdge::Closed));
        opened_edge |= t.mechs.edges().contains(&(SwitchId(8), SwitchEdge::Opened));
        if t.mechs.tilt_closed() {
            assert!(t.closed(8));
        }
    }
    assert!(closed_edge && opened_edge);
    // Without a tilt switch nothing is reported.
    t.mechs.set_tilt_switch(None);
    t.mechs.nudge(Vec3::new(0.3, 0.0, 0.0));
    for _ in 0..200 {
        t.tick();
        assert!(!t.closed(8));
        assert!(t.mechs.edges().is_empty());
    }
}

#[test]
fn tilt_bob_period_in_mech_set_follows_solver_gravity() {
    let mut set = MechSet::new(&[], &PhysicsDefaults::builtin()).unwrap();
    let mut s = solver(6.5);
    set.nudge(Vec3::new(0.02, 0.0, 0.0));
    let mut crossings = Vec::new();
    let mut last = 0.0f32;
    for i in 0..3000 {
        set.pre_step(&mut s, DT);
        let x = set.tilt_bob_offset().x;
        if last > 0.0 && x <= 0.0 {
            crossings.push(i as f32 * DT);
        }
        last = x;
    }
    let period = (crossings[3] - crossings[0]) / 3.0;
    let analytic = set.tilt_bob().small_swing_period();
    println!("tilt bob period {period:.4} s, analytic {analytic:.4} s");
    assert!((period - analytic).abs() / analytic < 0.02);
}
