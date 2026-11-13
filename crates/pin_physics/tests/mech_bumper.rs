//! Verification 1 (bumper ring response) and 6 (sling exit speed) of
//! Subphase 2.4.

#[path = "mech_util.rs"]
mod util;

use glam::Vec3;
use pin_core::SwitchEdge;
use pin_core::ids::{CoilId, SwitchId};
use pin_core::schema::table::MechKind;
use pin_physics::mech::MechSpec;
use util::*;

const RING: u16 = 40;
const COIL: CoilId = CoilId(3);

/// A ball sent at the bumper from `angle` (rad) at `speed`; the coil fires
/// on the tick after the ring switch closes. Returns (ticks to contact,
/// exit speed, exit radial speed, max angle between exit velocity and the
/// radial direction).
fn bumper_shot(angle: f32, speed: f32) -> (u64, f32, f32) {
    let spec = MechSpec::new(MechKind::Bumper, Vec3::ZERO)
        .with_coil(COIL)
        .with_switch(SwitchId(RING));
    let mut t = Table::new(&[spec], world(&[]), 0.0);
    let dir = Vec3::new(libm::cosf(angle), libm::sinf(angle), 0.0);
    let start = dir * 0.1;
    let h = t.ball(start.x, start.y, -dir * speed);
    let mut contact = None;
    for i in 0..400 {
        t.tick();
        if t.closed(RING) {
            assert!(
                t.mechs
                    .edges()
                    .contains(&(SwitchId(RING), SwitchEdge::Closed)),
                "closing edge on the contact tick"
            );
            contact = Some(i);
            break;
        }
    }
    let contact = contact.expect("ring switch closed");
    // The ball touches the ring on the closing tick.
    let b = t.get(h);
    let gap = (b.position.truncate().length()) - 0.030 - radius(STEEL);
    assert!(
        gap < 0.004,
        "ball near the ring when the switch closes: {gap}"
    );
    t.mechs.fire_coil(COIL, 20);
    for _ in 0..30 {
        t.tick();
    }
    let b = t.get(h);
    let n = b.position.truncate().normalize().extend(0.0);
    let out = b.velocity.truncate().length();
    (contact, out, b.velocity.dot(n))
}

#[test]
fn bumper_kick_is_in_the_feel_window_and_symmetric() {
    let mut speeds = Vec::new();
    for k in 0..16 {
        let a = k as f32 * core::f32::consts::TAU / 16.0 + 0.1;
        let (_, out, radial) = bumper_shot(a, 1.0);
        assert!((1.5..=4.0).contains(&out), "exit {out} at angle {a}");
        assert!(radial > 0.97 * out, "leaves radially: {radial} of {out}");
        speeds.push(out);
    }
    let lo = speeds.iter().copied().fold(f32::INFINITY, f32::min);
    let hi = speeds.iter().copied().fold(0.0, f32::max);
    println!("bumper exit speed (1.0 m/s approach, 16 angles): {lo:.4}..{hi:.4} m/s");
    assert!((hi - lo) / lo < 0.05, "rotational symmetry: {lo}..{hi}");
}

#[test]
fn bumper_exit_speed_over_approach_speeds() {
    for speed in [0.3f32, 0.6, 1.0, 1.5, 2.0] {
        let (_, out, _) = bumper_shot(0.7, speed);
        println!("bumper: approach {speed} m/s -> exit {out:.3} m/s");
        assert!((1.5..=4.0).contains(&out), "exit {out} at approach {speed}");
    }
}

#[test]
fn ring_switch_opens_after_contact_and_no_kick_without_coil() {
    let spec = MechSpec::new(MechKind::Bumper, Vec3::ZERO)
        .with_coil(COIL)
        .with_switch(SwitchId(RING));
    let mut t = Table::new(&[spec], world(&[]), 0.0);
    let h = t.ball(0.1, 0.0, Vec3::new(-1.0, 0.0, 0.0));
    let mut closed_ticks = 0;
    let mut opened = false;
    for _ in 0..400 {
        t.tick();
        if t.closed(RING) {
            closed_ticks += 1;
        } else if closed_ticks > 0 {
            opened = true;
        }
    }
    assert!(closed_ticks >= 1 && opened);
    // Passive rebound only: slower than the approach (no energy added).
    assert!(t.get(h).velocity.length() < 1.0);
    // A coil pulse with no ball nearby does nothing.
    t.mechs.fire_coil(COIL, 10);
    let before = t.get(h).velocity;
    t.tick();
    let after = t.get(h).velocity;
    assert!((after - before).length() < 0.01);
}

const SLING_SW: u16 = 41;
const SLING_COIL: CoilId = CoilId(4);

/// A ball at `speed` hitting a sling (face toward +x at x = 0) at `incidence`
/// from the normal; the coil fires when the sling switch closes.
fn sling_shot(incidence: f32, speed: f32) -> f32 {
    let mut spec = MechSpec::new(MechKind::Sling, Vec3::ZERO)
        .with_coil(SLING_COIL)
        .with_switch(SwitchId(SLING_SW));
    spec.params.angle_deg = Some(0.0);
    let mut t = Table::new(&[spec], world(&[]), 0.0);
    let d = Vec3::new(-libm::cosf(incidence), libm::sinf(incidence), 0.0);
    let start = Vec3::new(0.005 + radius(STEEL), 0.0, 0.0) - d * 0.06;
    let h = t.ball(start.x, start.y, d * speed);
    let mut fired = false;
    for _ in 0..300 {
        t.tick();
        if !fired && t.closed(SLING_SW) {
            t.mechs.fire_coil(SLING_COIL, 30);
            fired = true;
        }
    }
    assert!(fired, "sling switch closed");
    t.get(h).velocity.truncate().length()
}

#[test]
fn sling_exit_speed_is_in_the_feel_window() {
    for speed in [0.3f32, 0.6, 1.0, 1.5] {
        for inc in [0.0f32, 0.4, 0.8] {
            let out = sling_shot(inc, speed);
            println!("sling: approach {speed} m/s at {inc} rad -> exit {out:.3} m/s");
            assert!(
                (1.5..=4.0).contains(&out),
                "exit {out} (approach {speed}, {inc} rad)"
            );
        }
    }
}

/// Head-on sling shot from `gap` (m) at `speed`, coil fired `delay` ticks
/// after the switch closes; exit speed 40 ms after the fire.
fn sling_phase(speed: f32, gap: f32, delay: u32) -> f32 {
    let mut spec = MechSpec::new(MechKind::Sling, Vec3::ZERO)
        .with_coil(SLING_COIL)
        .with_switch(SwitchId(SLING_SW));
    spec.params.angle_deg = Some(0.0);
    let mut t = Table::new(&[spec], world(&[]), 0.0);
    let h = t.ball(0.005 + radius(STEEL) + gap, 0.0, v(-speed, 0.0, 0.0));
    let mut fire_at = None;
    for i in 0..600u32 {
        t.tick();
        if fire_at.is_none() && t.closed(SLING_SW) {
            fire_at = Some(i + delay);
        }
        if fire_at == Some(i) {
            t.mechs.fire_coil(SLING_COIL, 30);
        }
        if fire_at.is_some_and(|f| i == f + 40) {
            break;
        }
    }
    assert!(fire_at.is_some());
    t.get(h).velocity.truncate().length()
}

#[test]
fn sling_window_over_approach_speed_phase_and_latency() {
    let (mut lo, mut hi) = (f32::INFINITY, 0.0f32);
    for k in 1..=15 {
        let speed = k as f32 * 0.2;
        for g in 0..6 {
            for delay in 0..2 {
                let out = sling_phase(speed, 0.002 + g as f32 * 0.00037, delay);
                assert!((1.5..=4.0).contains(&out), "exit {out} (approach {speed}, gap {g}, delay {delay})");
                lo = lo.min(out);
                hi = hi.max(out);
            }
        }
    }
    println!("sling exit over 0.2–3 m/s, 6 phases, coil 0–1 ticks late: {lo:.3}..{hi:.3} m/s");
}
