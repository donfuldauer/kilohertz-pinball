//! Subphase 2.6, criterion 4 on the Independent Tester's rig (analysis of
//! Tester findings D1 and D2, DECISIONS "2.6 — criterion 4 on an oblique
//! face"): a ball straight down the table at 3.5 m/s onto the raised bat,
//! rebound = its speed when it has cleared the bat by 15 mm.
//!
//! On a curved bat that ball does not arrive square: at 70 % of a 120 mm
//! arc it meets the face 38° off the normal. A dead flip cancels the
//! normal part of its velocity, as on the straight bat; the part along the
//! face is no business of the bat's momentum and rolls on. These tests pin
//! that down: the normal part dies alike on both bats, what is left is the
//! rolling tangential part, and a straight capsule turned to the same
//! contact normal gives the curved bat's curve.
//! (`cargo test -p pin_physics --test banana_dead_spot -- --nocapture`)

#[path = "banana_util.rs"]
mod butil;

use butil::*;
use glam::Vec3;
use pin_core::FlipperConfig;
use pin_physics::ball::MovingBodies;

/// The Tester's bend: 120 mm arc radius (arc angle 36.9°).
const TESTER_RADIUS_MM: f32 = 120.0;
const SPEED: f32 = 3.5;

/// One shot of the Tester's rig.
#[derive(Clone, Debug)]
struct Shot {
    /// Speed when the ball has cleared the bat by 15 mm (its speed after
    /// 400 ticks if it never does: caught, rolling along the bat).
    rebound: f32,
    /// Velocity then.
    exit: Vec3,
    /// Tick of the first impact.
    first: i64,
    /// Outward normal of the bat at the first impact.
    normal: Vec3,
    /// Ticks with a ball–bat impact.
    impacts: Vec<i64>,
    /// How many impacts fell within a tick of the bat reaching a stop.
    impacts_at_a_stop: usize,
    /// Ticks in which the bat reached a stop while within 1 mm of the
    /// ball without touching it (the stop ended a chase).
    stopped_short: usize,
}

/// The ball starts 200 mm up the table from the centreline point at
/// `fraction` of the raised bat and comes straight down; the flipper is
/// pressed at tick `flip` (`None`: held up from the start).
fn shot(config: &FlipperConfig, fraction: f32, flip: Option<i64>) -> Shot {
    let rest = config.rest_angle_deg;
    let end = rest + config.sweep_angle_deg;
    let mut rig = Rig::new(std::slice::from_ref(config));
    let (face, n) = leading_face(config, fraction);
    let aim = bat_to_world(end, face - n * TUBE);
    let h = rig.add_ball(v(aim.x, aim.y + 0.2, BALL_R), v(0.0, -SPEED, 0.0));
    if flip.is_none() {
        rig.flippers.set_energised(LEFT, true);
        for _ in 0..200 {
            rig.flippers.advance(DT);
        }
    }
    let mut out = Shot {
        rebound: 0.0,
        exit: Vec3::ZERO,
        first: -1,
        normal: Vec3::ZERO,
        impacts: Vec::new(),
        impacts_at_a_stop: 0,
        stopped_short: 0,
    };
    let mut stop_ticks = Vec::new();
    let mut was_on_stop = true;
    let mut touched = false;
    for t in 0..400i64 {
        if flip == Some(t) {
            rig.flippers.set_energised(LEFT, true);
        }
        let before = rig
            .flippers
            .closest_point(0, 0.0, rig.ball(h).position)
            .unwrap();
        rig.step();
        let angle = rig.flippers.angle_deg(LEFT);
        let on_stop = angle == rest || angle == end;
        let arrived = on_stop && !was_on_stop;
        was_on_stop = on_stop;
        if arrived {
            stop_ticks.push(t);
        }
        let b = rig.ball(h);
        let gap = rig
            .flippers
            .closest_point(0, 0.0, b.position)
            .unwrap()
            .distance
            - BALL_R;
        if !rig.hits.is_empty() {
            if out.impacts.is_empty() {
                out.first = t;
                out.normal = before.normal;
            }
            out.impacts.push(t);
        } else if arrived && !out.impacts.is_empty() && before.distance - BALL_R < 1.0e-3 {
            out.stopped_short += 1;
        }
        if !out.impacts.is_empty() {
            touched |= gap < 2.0e-3;
            if touched && gap >= 0.015 {
                out.rebound = b.velocity.length();
                out.exit = b.velocity;
                break;
            }
        }
    }
    assert!(out.first >= 0, "the ball reaches the bat");
    if out.exit == Vec3::ZERO {
        out.exit = rig.ball(h).velocity;
        out.rebound = out.exit.length();
    }
    out.impacts_at_a_stop = out
        .impacts
        .iter()
        .filter(|&&t| stop_ticks.iter().any(|&s| (s - t).abs() <= 1))
        .count();
    out
}

/// The ±20 ms sweep around the held bat's impact tick.
struct Sweep {
    held: Shot,
    shots: Vec<Shot>,
}

impl Sweep {
    fn run(config: &FlipperConfig, fraction: f32) -> Sweep {
        let held = shot(config, fraction, None);
        let shots = (-20..=20)
            .map(|k| shot(config, fraction, Some(held.first + k)))
            .collect();
        Sweep { held, shots }
    }

    /// (flip time, shot) of the lowest rebound.
    fn minimum(&self) -> (i64, &Shot) {
        let (i, s) = self
            .shots
            .iter()
            .enumerate()
            .min_by(|a, b| a.1.rebound.total_cmp(&b.1.rebound))
            .unwrap();
        (i as i64 - 20, s)
    }

    fn print(&self, label: &str) {
        let curve: Vec<String> = self
            .shots
            .iter()
            .enumerate()
            .map(|(i, s)| format!("{:+}:{:.2}", i as i64 - 20, s.rebound))
            .collect();
        println!(
            "{label}: held {:.3} m/s; rebound by flip time (ms): {}",
            self.held.rebound,
            curve.join(" ")
        );
    }
}

/// Parts of a velocity along the contact normal and along the face.
fn parts(velocity: Vec3, normal: Vec3) -> (f32, f32) {
    (
        velocity.dot(normal),
        velocity.dot(Vec3::Z.cross(normal)).abs(),
    )
}

/// A straight bat (same length and thickness) whose face at the end of its
/// stroke has the normal the banana of `radius_mm` has at `fraction` of
/// its arc: the flipper turned by that much of the arc angle.
fn straight_turned_like(radius_mm: f32, fraction: f32) -> FlipperConfig {
    let arc = 2.0 * libm::asinf(0.5 * LENGTH_MM / radius_mm);
    let mut c = straight_config(0, [0.0; 3]);
    c.rest_angle_deg += (fraction * arc).to_degrees();
    c
}

/// Tester finding D1. The dead flip kills the normal part of the ball's
/// velocity on the curved bat as it does on the straight one; the exit
/// speed that remains is the part along the face, rolling on. A straight
/// capsule turned to the curved face's normal shows the same floor.
#[test]
fn an_oblique_face_leaves_the_tangential_part_of_a_dead_ball() {
    let banana = banana_config(0, [0.0; 3], TESTER_RADIUS_MM);
    let straight = straight_config(0, [0.0; 3]);
    let turned = straight_turned_like(TESTER_RADIUS_MM, 0.7);
    let mut floors = Vec::new();
    for (label, config) in [
        ("banana R 120 mm, 70 %", &banana),
        ("straight, 70 %", &straight),
        ("straight turned to the banana's 70 % normal", &turned),
    ] {
        let sweep = Sweep::run(config, 0.7);
        sweep.print(label);
        let (at, dead) = sweep.minimum();
        let incoming = v(0.0, -SPEED, 0.0);
        let (in_n, in_t) = parts(incoming, dead.normal);
        let (out_n, out_t) = parts(dead.exit, dead.normal);
        let incidence = libm::atan2f(in_t, -in_n).to_degrees();
        // A ball that slides in without spin and is gripped rolls on with
        // 5/7 of its tangential speed.
        let rolling = 5.0 / 7.0 * in_t;
        let ratio = dead.rebound / sweep.held.rebound;
        println!(
            "  minimum {:.3} m/s at {at:+} ms = {:.1} % of held; incidence {incidence:.1}° off the normal: in ({in_n:.2} normal, {in_t:.2} along the face) m/s, out ({out_n:.2}, {out_t:.2}); rolling floor 5/7·{in_t:.2} = {rolling:.2} m/s",
            dead.rebound,
            100.0 * ratio
        );
        // The normal part is dead: a fifth of what came in, on every bat.
        assert!(
            out_n.abs() < 0.25 * in_n.abs(),
            "{label}: normal part {out_n} of {in_n}"
        );
        // What is left is the tangential part, at about the rolling floor.
        assert!(
            (out_t - rolling).abs() < 0.2,
            "{label}: {out_t} vs {rolling}"
        );
        assert!(
            out_t > out_n.abs(),
            "{label}: the exit speed is mostly tangential"
        );
        floors.push((incidence, ratio, out_n.abs()));
    }
    let (banana, straight, turned) = (floors[0], floors[1], floors[2]);
    // The curved face is met far more obliquely than the straight bat ...
    assert!(banana.0 > straight.0 + 15.0, "{banana:?} vs {straight:?}");
    // ... its normal part dies as completely ...
    assert!(banana.2 < straight.2 + 0.1);
    // ... and a straight capsule at that incidence has the same shallow
    // dead spot by this measure: the depth follows the incidence, not the
    // bend.
    assert!(
        (turned.0 - banana.0).abs() < 6.0,
        "{turned:?} vs {banana:?}"
    );
    assert!(turned.1 > 0.5 && banana.1 > 0.5, "{turned:?} {banana:?}");
    assert!(
        (turned.1 - banana.1).abs() < 0.1,
        "{turned:?} vs {banana:?}"
    );
    assert!(straight.1 < 0.5);
}

/// Tester finding D2 and the continuity rule on this rig: every step above
/// 0.75 m/s between neighbouring 1 ms timings has a named cause. Impacts
/// are counted as the solver reports them (a ball caught up again within
/// 3 mm is a new impact, not the old contact).
#[test]
fn every_step_of_the_down_table_sweep_has_a_cause() {
    let banana = banana_config(0, [0.0; 3], TESTER_RADIUS_MM);
    for fraction in [0.5f32, 0.7] {
        let sweep = Sweep::run(&banana, fraction);
        sweep.print(&format!("banana R 120 mm, {fraction}"));
        for (i, pair) in sweep.shots.windows(2).enumerate() {
            let step = (pair[1].rebound - pair[0].rebound).abs();
            if step <= 0.75 {
                continue;
            }
            let k = i as i64 - 20;
            let cause = if pair[0].impacts_at_a_stop != pair[1].impacts_at_a_stop
                || pair[0].stopped_short != pair[1].stopped_short
            {
                "rigid-stop event: the bat reaches a stop at or just short of a contact in one run only"
            } else if pair[0].impacts.len() != pair[1].impacts.len() {
                "hit or miss: the bat reaches the already rebounding ball once more in one run only"
            } else {
                panic!(
                    "unexplained step {step:.2} m/s between {k:+} and {:+} ms",
                    k + 1
                );
            };
            println!(
                "  step {step:.2} m/s between {k:+} and {:+} ms ({} → {} impacts): {cause}",
                k + 1,
                pair[0].impacts.len(),
                pair[1].impacts.len()
            );
        }
    }
    // D2 itself: flips 9 and 8 ms before the held bat's impact, at 70 %.
    // Both knock the bat back, are hit again by the returning bat, and are
    // then chased up the stroke. The earlier flip's bat runs into its end
    // stop a fraction of a millimetre short of the ball; the later one
    // catches it once more.
    let held = shot(&banana, 0.7, None);
    let (early, late) = (
        shot(&banana, 0.7, Some(held.first - 9)),
        shot(&banana, 0.7, Some(held.first - 8)),
    );
    println!(
        "D2: flip −9 ms: impacts at ticks {:?}, stop reached within 1 mm of the ball {}×, rebound {:.3} m/s; flip −8 ms: impacts at {:?}, rebound {:.3} m/s",
        early.impacts, early.stopped_short, early.rebound, late.impacts, late.rebound
    );
    assert_eq!((early.impacts.len(), late.impacts.len()), (2, 3));
    assert_eq!(early.stopped_short, 1);
    assert!(late.rebound - early.rebound > 0.75);
}
