//! Subphase 2.4 spec tests, verification 1: bumper ring response, exit speed
//! window, rotational symmetry.

#[path = "spec_2_4_common.rs"]
mod common;

use common::*;
use glam::Vec3;
use pin_core::ids::{CoilId, SwitchId};
use pin_core::schema::table::MechKind;
use pin_physics::ball::{BallEvent, BallHandle, ImpactTarget};

const SW: SwitchId = SwitchId(40);
const COIL: CoilId = CoilId(5);
const CENTER: Vec3 = Vec3::new(0.1, 0.3, 0.0);

fn bumper_rig() -> Rig {
    Rig::on_floor(&[spec(MechKind::Bumper, CENTER)
        .with_coil(COIL)
        .with_switch(SW)])
}

fn ring_radius() -> f32 {
    defaults().mechs.bumper.radius
}

/// Gap between the ball surface and the ring (default radius).
fn gap(rig: &Rig, h: BallHandle) -> f32 {
    horiz(rig.ball(h).position - CENTER).length() - ring_radius() - R
}

fn body_impact(rig: &Rig) -> bool {
    rig.events.as_slice().iter().any(|e| {
        matches!(
            e,
            BallEvent::Impact {
                other: ImpactTarget::Body { set: 0, body: 0 },
                ..
            }
        )
    })
}

struct Outcome {
    /// Tick of first contact.
    contact_tick: u64,
    /// Horizontal velocity 6 ticks after the coil fired.
    exit: Vec3,
    /// Position at that time.
    position: Vec3,
}

/// A ball starting `start_gap` from the ring on the ray at `angle_deg` from
/// the bumper, offset sideways by `offset`, moving toward the bumper at
/// `speed`. The "script" fires the coil for `pulse_ms` on the tick after the
/// ring switch closes (0: never).
fn approach(angle_deg: f32, offset: f32, speed: f32, pulse_ms: u16) -> Outcome {
    let mut rig = bumper_rig();
    let radial = dir_deg(angle_deg);
    let side = Vec3::new(-radial.y, radial.x, 0.0);
    let reach = ring_radius() + R;
    let along = (reach * reach - offset * offset).sqrt() + 0.004;
    let start = CENTER + radial * along + side * offset + Vec3::new(0.0, 0.0, R);
    let h = rig.add(steel_ball(start, -radial * speed));
    let mut contact_tick = None;
    let mut fired_at = None;
    for _ in 0..400 {
        if fired_at.is_none() && contact_tick == Some(rig.tick) && pulse_ms > 0 {
            rig.mechs.fire_coil(COIL, pulse_ms);
            fired_at = Some(rig.tick);
        }
        rig.step();
        if contact_tick.is_none() {
            if rig.closed_edge(SW) {
                contact_tick = Some(rig.tick);
            } else {
                assert!(
                    !rig.mechs.mech_switch_closed(0),
                    "switch state without a closing edge"
                );
            }
        }
        if let Some(t) = contact_tick.filter(|t| rig.tick == *t + 7) {
            let b = rig.ball(h);
            return Outcome {
                contact_tick: t,
                exit: horiz(b.velocity),
                position: b.position,
            };
        }
    }
    panic!("ball at {angle_deg}° offset {offset} speed {speed} never closed the ring switch");
}

#[test]
fn ring_switch_closes_on_the_tick_of_contact_and_not_before() {
    for speed in [0.3f32, 1.0, 2.5] {
        let mut rig = bumper_rig();
        let start = CENTER + Vec3::new(0.0, -(ring_radius() + R + 0.01), R);
        let h = rig.add(steel_ball(start, Vec3::new(0.0, speed, 0.0)));
        let mut closed_tick = None;
        for _ in 0..200 {
            let gap_before = gap(&rig, h);
            rig.step();
            let touched = body_impact(&rig);
            let closed = rig.mechs.mech_switch_closed(0);
            if rig.closed_edge(SW) && closed_tick.is_none() {
                closed_tick = Some(rig.tick);
            }
            if touched {
                assert!(
                    closed,
                    "speed {speed}: ball hit the ring on tick {} but the switch is open",
                    rig.tick
                );
                assert!(rig.switch_bit(SW), "switch_states bit not set on contact");
                // The edge is reported on the contact tick (or the tick
                // before, when the ball ended that tick within the contact
                // tolerance of the ring).
                let t = closed_tick.expect("no Closed edge up to the contact tick");
                assert!(rig.tick - t <= 1, "edge at {t}, contact at {}", rig.tick);
                break;
            }
            // No contact this tick: the ball was farther than it can travel
            // in a tick, so the switch must be open.
            if gap_before > speed * DT + 5.0e-4 {
                assert!(
                    !closed && !rig.switch_bit(SW),
                    "speed {speed}: switch closed with the ball {gap_before} m away"
                );
                assert!(rig.edge(SW).is_none());
            }
        }
        assert!(closed_tick.is_some(), "speed {speed}: ball never touched");
    }
}

#[test]
fn ring_switch_opens_after_the_ball_leaves() {
    let mut rig = bumper_rig();
    let start = CENTER + Vec3::new(0.0, -(ring_radius() + R + 0.005), R);
    let h = rig.add(steel_ball(start, Vec3::new(0.0, 1.0, 0.0)));
    let mut seen_closed = false;
    let mut seen_opened = false;
    for _ in 0..500 {
        rig.step();
        seen_closed |= rig.closed_edge(SW);
        if seen_closed && rig.opened_edge(SW) {
            seen_opened = true;
        }
        if gap(&rig, h) > 0.02 && seen_closed {
            assert!(!rig.mechs.mech_switch_closed(0), "switch stuck closed");
            assert!(!rig.switch_bit(SW));
        }
    }
    assert!(seen_closed && seen_opened);
}

#[test]
fn kick_exit_speed_is_in_the_feel_window_and_radial() {
    for speed in [0.3f32, 0.6, 1.0, 1.5, 2.0] {
        let o = approach(-90.0, 0.0, speed, 25);
        let v = o.exit.length();
        eprintln!("bumper: approach {speed:.2} m/s -> exit {v:.3} m/s");
        assert!(
            (EXIT_MIN..=EXIT_MAX).contains(&v),
            "approach {speed}: exit speed {v} outside {EXIT_MIN}..{EXIT_MAX}"
        );
        let radial = horiz(o.position - CENTER).normalize();
        let cos = o.exit.normalize().dot(radial);
        assert!(
            cos > 0.999,
            "approach {speed}: exit direction not radial (cos {cos})"
        );
    }
}

#[test]
fn resting_ball_on_the_ring_is_kicked_radially_in_the_window() {
    for angle in [0.0f32, 37.0, 180.0, 291.0] {
        let mut rig = bumper_rig();
        let radial = dir_deg(angle);
        let start = CENTER + radial * (ring_radius() + R + 2.0e-5) + Vec3::new(0.0, 0.0, R);
        let h = rig.add(steel_ball(start, Vec3::ZERO));
        rig.run(3);
        assert!(
            rig.mechs.mech_switch_closed(0),
            "a ball resting on the ring must close the switch"
        );
        assert!(rig.ball(h).velocity.length() < 0.05, "no kick without coil");
        rig.mechs.fire_coil(COIL, 25);
        rig.run(6);
        let v = horiz(rig.ball(h).velocity);
        eprintln!(
            "bumper: resting ball at {angle}° -> exit {:.3} m/s",
            v.length()
        );
        assert!(
            (EXIT_MIN..=EXIT_MAX).contains(&v.length()),
            "angle {angle}: exit speed {} outside the window",
            v.length()
        );
        assert!(
            v.normalize().dot(radial) > 0.999,
            "angle {angle}: not radial"
        );
    }
}

#[test]
fn rotational_symmetry_within_five_percent() {
    // Head-on and glancing approaches, each from 16 directions (not aligned
    // with the axes).
    for (offset, speed) in [(0.0f32, 1.0f32), (0.012, 1.0), (-0.02, 1.8), (0.0, 0.4)] {
        let mut speeds = Vec::new();
        let mut radial_cos = Vec::new();
        let mut ticks = Vec::new();
        for k in 0..16 {
            let angle = 7.0 + 22.5 * k as f32;
            let o = approach(angle, offset, speed, 25);
            speeds.push(o.exit.length());
            let radial = horiz(o.position - CENTER).normalize();
            radial_cos.push(o.exit.normalize().dot(radial));
            ticks.push(o.contact_tick);
        }
        let mean = speeds.iter().sum::<f32>() / speeds.len() as f32;
        for (k, v) in speeds.iter().enumerate() {
            assert!(
                (v - mean).abs() <= 0.05 * mean,
                "offset {offset} speed {speed}: exit {v} at direction {k} vs mean {mean} ({speeds:?})"
            );
            assert!(
                (EXIT_MIN..=EXIT_MAX).contains(v),
                "offset {offset} speed {speed}: exit {v} outside the window"
            );
        }
        let cmin = radial_cos.iter().copied().fold(f32::INFINITY, f32::min);
        let cmax = radial_cos.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            cmax - cmin <= 0.05,
            "offset {offset}: exit direction differs between approach angles ({radial_cos:?})"
        );
        let tmin = *ticks.iter().min().unwrap();
        let tmax = *ticks.iter().max().unwrap();
        assert!(tmax - tmin <= 1, "contact tick differs: {ticks:?}");
        eprintln!(
            "bumper symmetry: offset {offset} speed {speed}: mean exit {mean:.3} m/s, spread {:.2} %",
            100.0 * speeds.iter().map(|v| (v - mean).abs()).fold(0.0, f32::max) / mean
        );
    }
}

#[test]
fn repeated_hits_behave_alike() {
    // The same bumper is hit five times in a row; every hit gives the same
    // exit speed.
    let mut rig = bumper_rig();
    let mut exits = Vec::new();
    for k in 0..5 {
        let radial = dir_deg(50.0 + 60.0 * k as f32);
        let start = CENTER + radial * (ring_radius() + R + 0.004) + Vec3::new(0.0, 0.0, R);
        let h = rig.add(steel_ball(start, -radial));
        let mut left = None;
        for _ in 0..300 {
            let fire = rig.closed_edge(SW);
            if fire {
                rig.mechs.fire_coil(COIL, 25);
                left = Some(rig.tick + 7);
            }
            rig.step();
            if left == Some(rig.tick) {
                break;
            }
        }
        assert!(left.is_some(), "hit {k} never closed the switch");
        exits.push(horiz(rig.ball(h).velocity).length());
        rig.solver.remove_ball(h);
        rig.run(60);
        assert!(!rig.mechs.mech_switch_closed(0));
    }
    let mean = exits.iter().sum::<f32>() / exits.len() as f32;
    for v in &exits {
        assert!((v - mean).abs() <= 0.05 * mean, "hits differ: {exits:?}");
    }
}

#[test]
fn no_kick_without_the_coil_and_no_kick_far_away() {
    // Without the coil the ring is a passive post: the ball loses speed.
    let o = approach(200.0, 0.0, 1.0, 0);
    assert!(
        o.exit.length() < 1.0,
        "passive ring sped the ball up: {}",
        o.exit.length()
    );
    // A coil pulse does nothing to a ball 10 cm away.
    let mut rig = bumper_rig();
    let far = CENTER + Vec3::new(ring_radius() + R + 0.10, 0.0, R);
    let h = rig.add(steel_ball(far, Vec3::ZERO));
    rig.run(5);
    rig.mechs.fire_coil(COIL, 30);
    rig.run(60);
    assert!(rig.ball(h).velocity.length() < 0.02);
    assert!(!rig.mechs.mech_switch_closed(0));
}

#[test]
fn kick_speed_comes_from_the_parameter() {
    // Exit speed grows with the kick_speed parameter (no named special case).
    let mut last = 0.0;
    for ks in [1.0f32, 1.5, 2.0, 2.4] {
        let mut p = params();
        p.kick_speed_m_s = Some(ks);
        let mut rig = Rig::on_floor(&[spec(MechKind::Bumper, CENTER)
            .with_coil(COIL)
            .with_switch(SW)
            .with_params(p)]);
        let start = CENTER + Vec3::new(ring_radius() + R + 2.0e-5, 0.0, R);
        let h = rig.add(steel_ball(start, Vec3::ZERO));
        rig.run(2);
        rig.mechs.fire_coil(COIL, 25);
        rig.run(6);
        let v = horiz(rig.ball(h).velocity).length();
        assert!(v > last + 0.2, "kick_speed {ks}: exit {v} not above {last}");
        assert!(
            v >= ks * 0.9,
            "kick_speed {ks}: exit {v} below the ring speed"
        );
        last = v;
    }
}

#[test]
fn bumper_only_acts_on_its_layer() {
    let mut p = params();
    p.layer = Some(1);
    let mut rig = Rig::in_space(&[spec(MechKind::Bumper, CENTER)
        .with_coil(COIL)
        .with_switch(SW)
        .with_params(p)]);
    // A layer-0 ball flies straight through a layer-1 bumper.
    let start = CENTER + Vec3::new(-(ring_radius() + R + 0.01), 0.0, R);
    let h = rig.add(steel_ball(start, Vec3::new(1.0, 0.0, 0.0)));
    rig.mechs.hold_coil(COIL, true);
    rig.run(150);
    let b = rig.ball(h);
    assert!(!rig.mechs.mech_switch_closed(0));
    assert!((b.velocity - Vec3::new(1.0, 0.0, 0.0)).length() < 1.0e-4);
    assert!(b.position.x > CENTER.x + 0.05);
}

#[test]
fn exit_speed_window_holds_across_an_approach_speed_sweep() {
    // Typical playfield speeds at a bumper: 0.2..3.0 m/s in 0.1 steps. The
    // exit speed must not depend on where in the tick the contact falls.
    let mut bad = Vec::new();
    for i in 2..=30 {
        let speed = 0.1 * i as f32;
        let o = approach(123.0, 0.0, speed, 25);
        let v = o.exit.length();
        if !(EXIT_MIN..=EXIT_MAX).contains(&v) {
            bad.push((speed, v));
        }
    }
    assert!(
        bad.is_empty(),
        "(approach, exit) pairs outside {EXIT_MIN}..{EXIT_MAX} m/s: {bad:?}"
    );
}

/// Verification 1 over the contact phase: the same approach speed must give
/// an in-window exit speed wherever within the tick the ball reaches the
/// ring (20 start gaps 0.17 mm apart, approach 0.2..3.0 m/s, coil fired on
/// the tick after the ring switch closes).
#[test]
fn exit_speed_window_holds_for_every_contact_phase() {
    let mut bad = Vec::new();
    let mut range = (f32::INFINITY, f32::NEG_INFINITY);
    for si in 2..=30 {
        let speed = 0.1 * si as f32;
        for gi in 0..20 {
            let gap = 0.003 + 0.00017 * gi as f32;
            let mut rig = bumper_rig();
            let start = CENTER + Vec3::new(ring_radius() + R + gap, 0.0, R);
            let h = rig.add(steel_ball(start, Vec3::new(-speed, 0.0, 0.0)));
            let mut contact = None;
            let mut exit = None;
            for _ in 0..300 {
                if contact == Some(rig.tick) {
                    rig.mechs.fire_coil(COIL, 25);
                }
                rig.step();
                if contact.is_none() && rig.closed_edge(SW) {
                    contact = Some(rig.tick);
                }
                if contact.is_some_and(|t| rig.tick == t + 12) {
                    exit = Some(horiz(rig.ball(h).velocity).length());
                    break;
                }
            }
            let v = exit.expect("ball never reached the ring");
            range = (range.0.min(v), range.1.max(v));
            if !(EXIT_MIN..=EXIT_MAX).contains(&v) {
                bad.push((speed, gap, v));
            }
        }
    }
    eprintln!("bumper exit speed over speeds × phases: {range:?} m/s");
    assert!(
        bad.is_empty(),
        "{} of 580 (approach m/s, start gap m, exit m/s) outside {EXIT_MIN}..{EXIT_MAX}: {bad:?}",
        bad.len()
    );
}
