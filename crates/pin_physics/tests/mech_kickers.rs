//! Slingshot and kickback kickers through the 2.2-S seam: kickback launch,
//! a ball resting on a held kicker (owner obligation 3), one-sided inverse
//! mass on the stops (obligation 1), and real velocity at a stop
//! (obligation 2).

#[path = "mech_util.rs"]
mod util;

use glam::Vec3;
use pin_core::ids::{CoilId, SwitchId};
use pin_core::schema::table::MechKind;
use pin_physics::ball::MovingBodies;
use pin_physics::mech::MechSpec;
use util::*;

const COIL: CoilId = CoilId(7);
const SW: u16 = 33;

fn kicker(kind: MechKind) -> MechSpec {
    // Face toward +y (default angle 90°), at the bottom of a pitched table.
    MechSpec::new(kind, v(0.0, -0.3, 0.0))
        .with_coil(COIL)
        .with_switch(SwitchId(SW))
}

fn face(kind: MechKind) -> f32 {
    let half = if kind == MechKind::Sling {
        0.005
    } else {
        0.006
    };
    -0.3 + half
}

#[test]
fn kickback_launches_a_resting_ball_up_the_lane() {
    let mut t = Table::new(&[kicker(MechKind::Kickback)], world(&[]), 6.5);
    let h = t.ball(
        0.0,
        face(MechKind::Kickback) + radius(STEEL) + 0.001,
        Vec3::ZERO,
    );
    for _ in 0..300 {
        t.tick();
    }
    assert!(
        t.closed(SW),
        "ball resting on the kickback closes its switch"
    );
    assert!(t.get(h).velocity.length() < 1e-3);
    t.mechs.fire_coil(COIL, 40);
    let mut peak = 0.0f32;
    for _ in 0..100 {
        t.tick();
        peak = peak.max(t.get(h).velocity.y);
    }
    println!("kickback launch: {peak:.3} m/s");
    assert!(peak > 1.5 && peak < 6.0);
    // The kicker returns to rest after the pulse.
    for _ in 0..300 {
        t.tick();
    }
    assert_eq!(t.mechs.slider_state(0), Some((0.0, 0.0)));
}

#[test]
fn ball_rests_on_a_held_kicker() {
    for kind in [MechKind::Sling, MechKind::Kickback] {
        let mut t = Table::new(&[kicker(kind)], world(&[]), 6.5);
        t.mechs.hold_coil(COIL, true);
        for _ in 0..100 {
            t.tick();
        }
        let (x, vel) = t.mechs.slider_state(0).unwrap();
        assert!(x > 0.0 && vel == 0.0, "held on the end stop");
        let h = t.ball(0.0, face(kind) + x + radius(STEEL) + 0.0005, Vec3::ZERO);
        for _ in 0..300 {
            t.tick();
        }
        let mut max_speed = 0.0f32;
        for _ in 0..2000 {
            t.tick();
            max_speed = max_speed.max(t.get(h).velocity.length());
        }
        println!("{kind:?}: ball on the held kicker, max speed {max_speed:.2e} m/s");
        assert!(max_speed < 2e-3, "no chatter on a held kicker: {max_speed}");
        assert!(t.closed(SW));
        t.mechs.hold_coil(COIL, false);
        for _ in 0..300 {
            t.tick();
        }
        assert_eq!(t.mechs.slider_state(0).unwrap().0, 0.0);
    }
}

#[test]
fn seam_obligations_on_the_stops() {
    let mut t = Table::new(&[kicker(MechKind::Kickback)], world(&[]), 0.0);
    let p = v(0.0, face(MechKind::Kickback), 0.01);
    // At rest: locked by the spring preload (kinematic).
    t.mechs.begin_tick(DT);
    for d in [Vec3::Y, -Vec3::Y, Vec3::X] {
        assert_eq!(t.mechs.inverse_mass_at(0, 0.0, p, d), 0.0);
    }
    assert_eq!(t.mechs.velocity_at(0, 0.0, p), Vec3::ZERO);
    t.mechs.end_tick(DT);
    // Driven: leaving the rest stop (moving away from it), so not engaged:
    // free along the axis in both directions, nothing across it
    // (obligation 1: engaged = at the stop and not moving away).
    t.mechs.hold_coil(COIL, true);
    t.mechs.begin_tick(DT);
    assert!(t.mechs.velocity_at(0, 0.0, p).y > 0.0);
    for d in [Vec3::Y, -Vec3::Y] {
        let w = t.mechs.inverse_mass_at(0, 0.0, p, d);
        assert!((w - 1.0 / 0.06).abs() < 1e-3, "free at kick-off: {w}");
    }
    assert!(t.mechs.inverse_mass_at(0, 0.0, p, Vec3::X) < 1e-9);
    // The reported inverse mass is the real response: a small push back
    // changes the velocity by J/m.
    let v0 = t.mechs.velocity_at(0, 0.0, p).y;
    t.mechs.apply_impulse(0, 0.0, p, -Vec3::Y * 0.01);
    let v1 = t.mechs.velocity_at(0, 0.0, p).y;
    assert!((v0 - v1 - 0.01 / 0.06).abs() < 1e-4, "{v0} -> {v1}");
    // A big push back while still on the rest stop: the stop takes it, the
    // velocity is 0 at that instant (obligation 2) and the stop is engaged.
    t.mechs.apply_impulse(0, 0.0, p, -Vec3::Y * 10.0);
    assert_eq!(t.mechs.velocity_at(0, 0.0, p), Vec3::ZERO);
    assert_eq!(t.mechs.velocity_at(0, 5e-4, p), Vec3::ZERO);
    assert_eq!(t.mechs.inverse_mass_at(0, 5e-4, p, -Vec3::Y), 0.0);
    assert!(t.mechs.inverse_mass_at(0, 5e-4, p, Vec3::Y) > 0.0);
    t.mechs.end_tick(DT);
    assert_eq!(t.mechs.slider_state(0).unwrap(), (0.0, 0.0));
    // Off the stop, a push back re-predicts to reach the stop by dt at most
    // (obligation 3 after an impulse): the slider ends exactly on the stop.
    t.mechs.begin_tick(DT);
    t.mechs.apply_impulse(0, 5e-4, p, -Vec3::Y * 10.0);
    let back = t.mechs.velocity_at(0, 5e-4, p).y;
    assert!(back < 0.0, "moving back toward the stop: {back}");
    let x_mid = -back * 5e-4;
    assert!(x_mid > 0.0 && x_mid < 0.002, "{x_mid}");
    t.mechs.end_tick(DT);
    assert_eq!(t.mechs.slider_state(0).unwrap(), (0.0, 0.0));
    // Driven to the end stop: engaged, no push into it, free away from it.
    for _ in 0..100 {
        t.mechs.begin_tick(DT);
        t.mechs.end_tick(DT);
    }
    t.mechs.begin_tick(DT);
    assert_eq!(t.mechs.velocity_at(0, 0.0, p), Vec3::ZERO);
    assert_eq!(t.mechs.inverse_mass_at(0, 0.0, p, Vec3::Y), 0.0);
    assert!(t.mechs.inverse_mass_at(0, 0.0, p, -Vec3::Y) > 0.0);
    // Obligation 3: no phantom speed after a small push off the stop.
    t.mechs.apply_impulse(0, 0.0, p, -Vec3::Y * 1e-5);
    t.mechs.end_tick(DT);
    t.mechs.begin_tick(DT);
    let s = t.mechs.velocity_at(0, 0.0, p).y;
    let (x, _) = t.mechs.slider_state(0).unwrap();
    assert!(
        s * DT <= 0.020 - x + 1e-7,
        "speculative stop: {s} at gap {}",
        0.020 - x
    );
    assert!(t.mechs.max_surface_speed(0) >= s.abs());
    t.mechs.end_tick(DT);
}

/// Regression (2.4 review B1): at kick-off the slider leaves its rest stop;
/// it must not act as an infinite mass. Over the first tick the ball can get
/// at most the impulse the drive supplies (`F·dt`, the kicker starting at
/// rest), and over the whole kick at most the drive's work as energy.
#[test]
fn kick_off_momentum_and_energy_are_bounded_by_the_drive() {
    let d = pin_core::PhysicsDefaults::builtin();
    let mb = profiles()[STEEL.index()].mass_kg;
    for (kind, k) in [
        (MechKind::Sling, d.mechs.sling),
        (MechKind::Kickback, d.mechs.kickback),
    ] {
        let mut t = Table::new(&[kicker(kind)], world(&[]), 0.0);
        let h = t.ball(0.0, face(kind) + radius(STEEL) + 2e-5, Vec3::ZERO);
        t.tick();
        t.mechs.hold_coil(COIL, true);
        t.tick();
        let p1 = mb * t.get(h).velocity.y;
        println!("{kind:?}: first-tick ball impulse {p1:.4} N·s, drive F·dt {:.4}", k.force * DT);
        assert!(p1 <= k.force * DT * 1.02 + 1e-5, "{p1}");
        let mut ke = 0.0f32;
        for _ in 0..60 {
            t.tick();
            ke = ke.max(0.5 * mb * t.get(h).velocity.length_squared());
        }
        println!("{kind:?}: ball energy {ke:.4} J, drive work {:.4} J", k.force * k.stroke);
        assert!(ke <= k.force * k.stroke * 1.02, "{ke}");
    }
    // Plunger released from full pull with the ball on the tip.
    let p = d.mechs.plunger;
    let spec = MechSpec::new(MechKind::Plunger, v(0.0, -0.3, 0.0));
    let mut t = Table::new(&[spec], world(&[]), 0.0);
    t.mechs.set_plunger(1.0);
    for _ in 0..60 {
        t.tick();
    }
    let h = t.ball(0.0, -0.3 - p.stroke + 0.006 + radius(STEEL) + 2e-5, Vec3::ZERO);
    t.tick();
    t.mechs.set_plunger(0.0);
    t.tick();
    let p1 = mb * t.get(h).velocity.y;
    let drive = p.spring_k * p.stroke * DT;
    println!("plunger: first-tick ball impulse {p1:.4} N·s, spring impulse {drive:.4}");
    assert!(p1 <= drive * 1.02, "{p1}");
    let mut ke = 0.0f32;
    for _ in 0..60 {
        t.tick();
        ke = ke.max(0.5 * mb * t.get(h).velocity.length_squared());
    }
    let work = 0.5 * p.spring_k * p.stroke * p.stroke;
    println!("plunger: ball energy {ke:.4} J, spring energy {work:.4} J");
    assert!(ke <= work * 1.02);
}
