//! Subphase 2.4 spec tests: drop targets (threshold, reset, no trapped ball;
//! verification 6), spinner pulse counts against the analytic spin-down
//! (verification 2) and VUK transfer from layer 0 to layer 1 (verification 3).

#[path = "spec_2_4_common.rs"]
mod common;

use common::*;
use glam::Vec3;
use pin_core::ids::{CoilId, SwitchId};
use pin_core::schema::table::MechKind;
use pin_core::script::SwitchEdge;
use pin_physics::ball::BallHandle;
use pin_physics::mech::MechSpec;
use pin_physics::world::{StaticWorldBuilder, layers};
use proptest::prelude::*;

// ------------------------------------------------------------ drop targets

const DT_SW: SwitchId = SwitchId(50);
const DT_SW2: SwitchId = SwitchId(51);
const DT_COIL: CoilId = CoilId(7);
const DT_POS: Vec3 = Vec3::new(0.05, 0.4, 0.0);

/// A drop target facing −y (balls come up the table at it).
fn target_spec(threshold: Option<f32>) -> MechSpec {
    let mut p = params();
    p.angle_deg = Some(270.0);
    p.threshold_speed_m_s = threshold;
    spec(MechKind::DropTarget, DT_POS)
        .with_coil(DT_COIL)
        .with_switch(DT_SW)
        .with_params(p)
}

/// Ball centre `gap` in front of the face (−y side), at lateral `x`.
fn in_front(gap: f32, x: f32) -> Vec3 {
    let t = defaults().mechs.drop_target.thickness;
    DT_POS + Vec3::new(x, -(0.5 * t + R + gap), R)
}

/// Flies a ball (no gravity) at the target with `velocity`; returns the rig
/// after 120 ms and the ball.
fn fly_at(threshold: Option<f32>, velocity: Vec3) -> (Rig, BallHandle) {
    let mut rig = Rig::in_space(&[target_spec(threshold)]);
    let n = velocity.normalize();
    // Start 5 mm before the contact point, backed up along the flight path.
    let h = rig.add(steel_ball(in_front(0.0, 0.0) - n * (0.005 / n.y), velocity));
    rig.run(120);
    (rig, h)
}

#[test]
fn drops_only_at_or_above_the_default_threshold_speed() {
    let thr = defaults().mechs.drop_target.threshold_speed;
    for i in 1..=40 {
        let speed = thr * 0.1 * i as f32;
        if (speed / thr - 1.0).abs() < 0.03 {
            continue;
        }
        let (rig, h) = fly_at(None, Vec3::new(0.0, speed, 0.0));
        let down = rig.mechs.drop_target_down(0).unwrap();
        assert_eq!(
            down,
            speed > thr,
            "impact at {speed} m/s, threshold {thr}: down = {down}"
        );
        assert_eq!(
            rig.mechs.mech_switch_closed(0),
            down,
            "switch follows the target"
        );
        assert_eq!(rig.switch_bit(DT_SW), down);
        let b = rig.ball(h);
        if !down {
            assert!(
                b.velocity.y < 0.0,
                "ball did not bounce off the standing target"
            );
        }
        assert!(is_finite_ball(&b));
    }
}

#[test]
fn threshold_uses_the_normal_impact_speed() {
    let thr = defaults().mechs.drop_target.threshold_speed;
    // 45° impacts: only the normal component counts.
    let d = Vec3::new(1.0, 1.0, 0.0).normalize();
    let (rig, _) = fly_at(None, d * (thr * 1.25)); // normal 0.88·thr
    assert_eq!(rig.mechs.drop_target_down(0), Some(false));
    let (rig, _) = fly_at(None, d * (thr * 1.6)); // normal 1.13·thr
    assert_eq!(rig.mechs.drop_target_down(0), Some(true));
}

#[test]
fn drop_edge_and_disabled_collider() {
    let mut rig = Rig::in_space(&[target_spec(None)]);
    let h = rig.add(steel_ball(in_front(0.005, 0.0), Vec3::new(0.0, 1.5, 0.0)));
    let mut edge_tick = None;
    for _ in 0..40 {
        rig.step();
        if rig.closed_edge(DT_SW) {
            assert!(edge_tick.is_none(), "two Closed edges");
            edge_tick = Some(rig.tick);
            assert_eq!(rig.mechs.drop_target_down(0), Some(true));
        }
    }
    // 5 mm at 1.5 m/s: contact during tick 4.
    let t = edge_tick.expect("no Closed edge");
    assert!((3..=5).contains(&t), "edge on tick {t}");
    rig.solver.remove_ball(h);
    // The dropped target has no collider: a slow second ball passes through
    // where it stood, and the switch stays closed.
    let h2 = rig.add(steel_ball(in_front(0.01, 0.0), Vec3::new(0.0, 0.3, 0.0)));
    rig.run(300);
    let b = rig.ball(h2);
    assert!(
        (b.velocity - Vec3::new(0.0, 0.3, 0.0)).length() < 1.0e-5,
        "{:?}",
        b.velocity
    );
    assert!(b.position.y > DT_POS.y + 0.03);
    assert!(rig.mechs.mech_switch_closed(0));
    assert_eq!(rig.mechs.drop_target_down(0), Some(true));
}

#[test]
fn reset_coil_raises_the_bank_and_reopens_the_switches() {
    let mut a = target_spec(None);
    a.position = DT_POS + Vec3::new(-0.03, 0.0, 0.0);
    let mut b = target_spec(None);
    b.position = DT_POS + Vec3::new(0.03, 0.0, 0.0);
    b.switch = Some(DT_SW2);
    let mut rig = Rig::in_space(&[a, b]);
    let front = |x: f32| in_front(0.005, x);
    // Reset with every target up: nothing happens.
    rig.mechs.fire_coil(DT_COIL, 100);
    for _ in 0..150 {
        rig.step();
        assert!(rig.mechs.edges().is_empty());
        assert_eq!(rig.mechs.drop_target_down(0), Some(false));
    }
    for cycle in 0..3 {
        let h1 = rig.add(steel_ball(front(-0.03), Vec3::new(0.0, 1.0, 0.0)));
        let h2 = rig.add(steel_ball(front(0.03), Vec3::new(0.0, 2.0, 0.0)));
        rig.run(100);
        assert_eq!(rig.mechs.drop_target_down(0), Some(true), "cycle {cycle}");
        assert_eq!(rig.mechs.drop_target_down(1), Some(true), "cycle {cycle}");
        assert!(rig.switch_bit(DT_SW) && rig.switch_bit(DT_SW2));
        rig.solver.remove_ball(h1);
        rig.solver.remove_ball(h2);
        rig.mechs.fire_coil(DT_COIL, 100);
        let mut opened = [false; 2];
        for _ in 0..200 {
            rig.step();
            for (s, e) in rig.mechs.edges() {
                assert_eq!(*e, SwitchEdge::Opened, "only opening edges during a reset");
                opened[usize::from(*s == DT_SW2)] = true;
            }
        }
        assert_eq!(opened, [true, true], "cycle {cycle}: bank did not reset");
        assert_eq!(rig.mechs.drop_target_down(0), Some(false));
        assert_eq!(rig.mechs.drop_target_down(1), Some(false));
        assert!(!rig.switch_bit(DT_SW) && !rig.switch_bit(DT_SW2));
        // Reset cleanly: a slow ball bounces off again, without dropping it.
        let slow = rig.add(steel_ball(front(-0.03), Vec3::new(0.0, 0.3, 0.0)));
        rig.run(100);
        assert_eq!(rig.mechs.drop_target_down(0), Some(false));
        assert!(
            rig.ball(slow).velocity.y < 0.0,
            "raised target is not solid"
        );
        rig.solver.remove_ball(slow);
    }
}

/// Signed gap between a ball centred at `c` and the raised target's box
/// (negative: the ball surface is inside the box).
fn target_gap(c: Vec3) -> f32 {
    let d = defaults().mechs.drop_target;
    // Faces −y: thickness along y, length along x.
    let half = Vec3::new(0.5 * d.length, 0.5 * d.thickness, 0.5 * d.height);
    let center = DT_POS + Vec3::new(0.0, 0.0, 0.5 * d.height);
    let q = (c - center).abs() - half;
    let outside = q.max(Vec3::ZERO).length();
    let inside = q.x.max(q.y).max(q.z).min(0.0);
    outside + inside - R
}

#[test]
fn reset_never_traps_a_ball_inside_the_target() {
    let offsets = [
        (0.0f32, 0.0f32),
        (0.004, 0.0),
        (-0.009, 0.001),
        (0.0, 0.004),
        (0.0, -0.006),
        (0.011, -0.003),
        (-0.012, 0.007),
        (0.006, 0.012),
    ];
    for (ox, oy) in offsets {
        let mut rig = Rig::on_floor(&[target_spec(None)]);
        // Knock it down.
        let h = rig.add(steel_ball(in_front(0.005, 0.0), Vec3::new(0.0, 1.5, 0.0)));
        rig.run(30);
        assert_eq!(rig.mechs.drop_target_down(0), Some(true));
        rig.solver.remove_ball(h);
        // A ball rests on the playfield right over the dropped target.
        let h = rig.add(steel_ball(DT_POS + Vec3::new(ox, oy, R), Vec3::ZERO));
        rig.run(200);
        assert!(
            (rig.ball(h).position - (DT_POS + Vec3::new(ox, oy, R))).length() < 1.0e-3,
            "ball over a dropped target should rest on the playfield"
        );
        rig.mechs.fire_coil(DT_COIL, 150);
        for i in 0..1500 {
            rig.step();
            let b = rig.ball(h);
            assert!(is_finite_ball(&b), "offset ({ox},{oy}): non-finite ball");
            assert!(
                b.velocity.length() < 10.0,
                "offset ({ox},{oy}) tick {i}: ball thrown at {} m/s",
                b.velocity.length()
            );
            assert!(
                b.position.z > R - 1.0e-3,
                "ball pushed through the playfield"
            );
        }
        assert_eq!(
            rig.mechs.drop_target_down(0),
            Some(false),
            "offset ({ox},{oy}): target did not come up"
        );
        assert!(!rig.mechs.mech_switch_closed(0));
        let gap = target_gap(rig.ball(h).position);
        assert!(
            gap > -1.0e-3,
            "offset ({ox},{oy}): ball ends {} mm inside the raised target",
            -gap * 1.0e3
        );
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Drops iff the normal impact speed is at least the configured threshold.
    #[test]
    fn prop_drop_threshold(thr in 0.15f32..2.5, ratio in 0.2f32..3.0, x in -0.008f32..0.008) {
        prop_assume!((ratio - 1.0).abs() > 0.04);
        let speed = thr * ratio;
        let mut rig = Rig::in_space(&[target_spec(Some(thr))]);
        rig.add(steel_ball(in_front(0.004, x), Vec3::new(0.0, speed, 0.0)));
        rig.run(((0.02 / speed) / DT) as u32 + 20);
        prop_assert_eq!(rig.mechs.drop_target_down(0), Some(ratio > 1.0));
        prop_assert_eq!(rig.mechs.mech_switch_closed(0), ratio > 1.0);
    }
}

// ----------------------------------------------------------------- spinner
//
// Documented model (rustdoc of `mech::spinner`, DECISIONS "2.4 — fix round
// 1", B2): a uniform flap of length L hinged at its top edge,
//   dω/dt = −d·ω − (3g/(2L))·sin φ − c·sign ω,
// gravity from the solver, pivot friction c = `[mechs.spinner] friction`
// (60 rad/s²), stiction holding only within asin(c·2L/(3g)) of hanging.
// Without gravity and friction the total rotation is exactly ω0/d. A pulse
// is counted at every horizontal crossing (φ = π/2 + kπ).

const SP_SW: SwitchId = SwitchId(60);
const SP_POS: Vec3 = Vec3::new(0.0, 0.2, 0.0);
const G: f32 = 9.81;

fn spinner_spec(damping: Option<f32>) -> MechSpec {
    let mut p = params();
    p.damping_per_s = damping;
    spec(MechKind::Spinner, SP_POS)
        .with_switch(SP_SW)
        .with_params(p)
}

/// The default model on a level playfield (gravity straight down).
fn spinner_rig(damping: Option<f32>) -> Rig {
    Rig::on_floor(&[spinner_spec(damping)])
}

/// Pure viscous spin-down: no gravity, no pivot friction.
fn free_spinner_rig(damping: Option<f32>) -> Rig {
    let mut d = defaults();
    d.mechs.spinner.friction = 0.0;
    let mut rig = Rig::in_space(&[]);
    rig.mechs = pin_physics::mech::MechSet::new(&[spinner_spec(damping)], &d).unwrap();
    rig
}

struct SpinDown {
    total_angle: f32,
    pulses: u64,
    closed_edges: u64,
    ticks: u32,
}

/// Spins the flap of `rig` at `omega0` and runs until it has been at rest
/// for 50 ms (or `limit_s`).
fn spin_down(mut rig: Rig, omega0: f32, limit_s: f32) -> SpinDown {
    rig.run(3);
    let (a0, w0) = rig.mechs.spinner_state(0).unwrap();
    assert_eq!(w0, 0.0);
    assert_eq!(rig.mechs.spinner_pulses(0), 0);
    rig.mechs.set_spinner_speed(0, omega0);
    let mut closed_edges = 0;
    let mut ticks = 0;
    let mut prev_pulses = 0;
    let mut still = 0;
    let limit = (limit_s / DT) as u32;
    while ticks < limit && still < 50 {
        rig.step();
        ticks += 1;
        let pulses = rig.mechs.spinner_pulses(0);
        assert!(pulses >= prev_pulses, "pulse counter went backwards");
        let pulsed = pulses > prev_pulses;
        if rig.closed_edge(SP_SW) {
            closed_edges += 1;
            assert!(pulsed, "switch edge without a counted pulse");
        }
        if pulsed {
            assert!(
                rig.mechs.mech_switch_closed(0),
                "pulse tick without the switch closed"
            );
        }
        prev_pulses = pulses;
        let (a, w) = rig.mechs.spinner_state(0).unwrap();
        assert!(a.is_finite() && w.is_finite());
        still = if w == 0.0 { still + 1 } else { 0 };
    }
    let (a1, w1) = rig.mechs.spinner_state(0).unwrap();
    assert!(
        w1.abs() < 0.05,
        "flap still turning at {w1} rad/s after {ticks} ms"
    );
    SpinDown {
        total_angle: a1 - a0,
        pulses: prev_pulses,
        closed_edges,
        ticks,
    }
}

/// Independent f64 reference of the documented ODE (RK4, 10 µs steps, with
/// Coulomb friction and stiction in the lower half). Returns the number of
/// horizontal crossings and the final angle.
fn reference(d: f64, omega0: f64, length: f64, friction: f64, g: f64) -> (u64, f64) {
    let k = 1.5 * g / length;
    let h = 1.0e-5;
    let (mut phi, mut w) = (0.0f64, omega0);
    let half_turns =
        |phi: f64| ((phi - std::f64::consts::FRAC_PI_2) / std::f64::consts::PI).floor();
    let mut cell = half_turns(phi);
    let mut pulses = 0u64;
    for _ in 0..(120.0 / h) as u64 {
        let gamma = -k * phi.sin();
        if w == 0.0 {
            if gamma.abs() <= friction && phi.cos() > 0.0 {
                break; // held by static friction
            }
            if gamma == 0.0 {
                break; // exactly balanced (only reachable without gravity)
            }
        }
        let s = if w != 0.0 { w.signum() } else { gamma.signum() };
        let acc = |phi: f64, w: f64| -d * w - k * phi.sin() - friction * s;
        let (k1p, k1w) = (w, acc(phi, w));
        let (k2p, k2w) = (
            w + 0.5 * h * k1w,
            acc(phi + 0.5 * h * k1p, w + 0.5 * h * k1w),
        );
        let (k3p, k3w) = (
            w + 0.5 * h * k2w,
            acc(phi + 0.5 * h * k2p, w + 0.5 * h * k2w),
        );
        let (k4p, k4w) = (w + h * k3w, acc(phi + h * k3p, w + h * k3w));
        phi += h / 6.0 * (k1p + 2.0 * k2p + 2.0 * k3p + k4p);
        let w_new = w + h / 6.0 * (k1w + 2.0 * k2w + 2.0 * k3w + k4w);
        // Friction cannot reverse the motion within a step: stop at zero.
        w = if w != 0.0 && w_new * w <= 0.0 {
            0.0
        } else {
            w_new
        };
        if w.abs() < 1.0e-9 && k == 0.0 && friction == 0.0 {
            break;
        }
        let c = half_turns(phi);
        if c != cell {
            pulses += (c - cell).abs() as u64;
            cell = c;
        }
    }
    (pulses, phi)
}

/// Distance (rad) of `angle` from hanging (φ ≡ 0 mod 2π).
fn from_hanging(angle: f32) -> f32 {
    let two_pi = 2.0 * std::f32::consts::PI;
    let a = angle.rem_euclid(two_pi);
    a.min(two_pi - a)
}

/// Half-width of the stiction band, asin(c·2L/(3g)).
fn hold_angle() -> f32 {
    let sp = defaults().mechs.spinner;
    (sp.friction * 2.0 * sp.radius / (3.0 * G)).asin()
}

#[test]
fn free_spin_down_total_rotation_is_omega0_over_d() {
    // No gravity, no friction: exactly the exponential model of the brief.
    let d0 = defaults().mechs.spinner.damping;
    for d in [d0, 0.4, 2.0, 5.0] {
        for omega0 in [3.0f32, 10.0, 25.0, 60.0, 150.0, -40.0] {
            let s = spin_down(free_spinner_rig(Some(d)), omega0, 30.0 / d);
            let analytic = omega0 / d;
            assert!(
                (s.total_angle - analytic).abs() <= 0.01 * analytic.abs() + 0.08 / d,
                "d {d}, ω0 {omega0}: total rotation {} rad, analytic ω0/d = {analytic}",
                s.total_angle
            );
            let expected = (analytic.abs() / std::f32::consts::PI).floor() as i64;
            assert!(
                (s.pulses as i64 - expected).abs() <= 1,
                "d {d}, ω0 {omega0}: {} pulses, floor((ω0/d)/180°) = {expected}",
                s.pulses
            );
            assert_eq!(s.closed_edges, s.pulses, "one switch closure per pulse");
        }
    }
    // Default damping when not overridden.
    let s = spin_down(free_spinner_rig(None), 20.0, 30.0 / d0);
    assert!((s.total_angle - 20.0 / d0).abs() < 0.01 * 20.0 / d0 + 0.08 / d0);
}

#[test]
fn free_spin_down_is_exponential() {
    let d = 1.5f32;
    let mut rig = free_spinner_rig(Some(d));
    rig.mechs.set_spinner_speed(0, 40.0);
    for k in 1..=4 {
        rig.run(500);
        let (_, w) = rig.mechs.spinner_state(0).unwrap();
        let expected = 40.0 * (-d * 0.5 * k as f32).exp();
        assert!(
            (w - expected).abs() < 0.01 * expected,
            "after {} s: ω = {w}, expected {expected}",
            0.5 * k as f32
        );
    }
}

/// Tolerance against the f64 reference: the solver integrates the same ODE
/// with 1 ms semi-implicit steps, whose slightly different energy decides
/// whether the last decaying swings still reach horizontal. One swing more
/// or less is two crossings (up and back), so ±2; above 20 pulses 10 %
/// (the per-revolution energy error accumulates over a long spin).
fn pulse_tolerance(reference: u64) -> u64 {
    2.max(reference / 10)
}

#[test]
fn pulse_counts_match_the_documented_spin_down() {
    let sp = defaults().mechs.spinner;
    for d in [sp.damping, 0.4, 2.0, 5.0] {
        for omega0 in [20.0f32, 46.0, 60.0, 100.0, 150.0, 250.0, -80.0] {
            let s = spin_down(spinner_rig(Some(d)), omega0, 60.0);
            let (expected, _) = reference(
                f64::from(d),
                f64::from(omega0),
                f64::from(sp.radius),
                f64::from(sp.friction),
                f64::from(G),
            );
            eprintln!(
                "spinner d {d} ω0 {omega0}: {} pulses (reference {expected}), {:.2} rad, {} ms",
                s.pulses, s.total_angle, s.ticks
            );
            assert!(
                s.pulses.abs_diff(expected) <= pulse_tolerance(expected),
                "d {d}, ω0 {omega0}: {} pulses, documented model gives {expected}",
                s.pulses
            );
            // It settles hanging, inside the stiction band.
            assert!(
                from_hanging(s.total_angle) <= hold_angle() + 0.02,
                "d {d}, ω0 {omega0}: flap parked {} rad from hanging",
                from_hanging(s.total_angle)
            );
            // Gravity and friction only ever shorten the spin.
            assert!(s.total_angle.abs() <= (omega0 / d).abs() * 1.01 + 0.1);
        }
    }
}

#[test]
fn fast_spin_approaches_omega0_over_d() {
    // High-speed limit of the documented model: the total rotation is
    // ω0/d − (c/d²)·ln(1 + d·ω0/c) minus at most about ω_c/d (ω_c: the
    // over-the-top speed √(4·3g/(2L))).
    let sp = defaults().mechs.spinner;
    let (d, c) = (2.0f32, sp.friction);
    let omega_c = (4.0 * 1.5 * G / sp.radius).sqrt();
    for omega0 in [200.0f32, 300.0] {
        let s = spin_down(spinner_rig(Some(d)), omega0, 60.0);
        let upper = omega0 / d - c / (d * d) * (1.0 + d * omega0 / c).ln();
        assert!(
            s.total_angle <= upper * 1.02 + 0.5 && s.total_angle >= upper - 1.5 * omega_c / d,
            "ω0 {omega0}: total {} rad, expected within [{}, {}]",
            s.total_angle,
            upper - 1.5 * omega_c / d,
            upper
        );
    }
}

#[test]
fn one_switch_pulse_per_half_turn() {
    // A spin that goes over the top several times: every pulse is its own
    // switch closure.
    let s = spin_down(spinner_rig(None), 120.0, 60.0);
    assert!(s.pulses >= 8, "{} pulses", s.pulses);
    assert_eq!(
        s.closed_edges, s.pulses,
        "switch closures and counted pulses differ"
    );
}

#[test]
fn a_weak_push_swings_back_to_hanging_without_a_pulse() {
    // Below the speed that reaches horizontal (ω² < 2·3g/(2L): 32 rad/s)
    // the flap swings, gives no pulse, and settles hanging.
    for omega0 in [1.0f32, 5.0, 15.0, 25.0, -20.0] {
        let s = spin_down(spinner_rig(None), omega0, 30.0);
        assert_eq!(s.pulses, 0, "ω0 {omega0}: {} pulses", s.pulses);
        assert_eq!(s.closed_edges, 0);
        assert!(
            s.total_angle.abs() <= hold_angle() + 0.02,
            "ω0 {omega0}: flap rests at {} rad",
            s.total_angle
        );
    }
    // An untouched flap hangs still.
    let mut rig = spinner_rig(None);
    rig.run(2000);
    assert_eq!(rig.mechs.spinner_state(0), Some((0.0, 0.0)));
    assert_eq!(rig.mechs.spinner_pulses(0), 0);
}

#[test]
fn every_successive_shot_spins_the_flap() {
    for speed in [1.0f32, 1.5, 3.0] {
        let mut rig = spinner_rig(None);
        let mut counts = Vec::new();
        for shot in 0..8 {
            let before = rig.mechs.spinner_pulses(0);
            let h = rig.add(steel_ball(
                SP_POS + Vec3::new(0.0, -0.05, R),
                Vec3::new(0.0, speed, 0.0),
            ));
            rig.run(400);
            let b = rig.ball(h);
            assert!(
                b.position.y > SP_POS.y + 0.03 && b.velocity.y > 0.0,
                "shot {shot} at {speed} m/s did not pass: {:?} {:?}",
                b.position,
                b.velocity
            );
            assert!(b.velocity.y < speed, "the flap must take some energy");
            rig.solver.remove_ball(h);
            rig.run(2600);
            let (angle, w) = rig.mechs.spinner_state(0).unwrap();
            assert!(w.abs() < 0.05, "shot {shot}: flap still moving after 3 s");
            assert!(
                from_hanging(angle) <= hold_angle() + 0.02,
                "shot {shot} at {speed} m/s: flap parked {} rad from hanging",
                from_hanging(angle)
            );
            let n = rig.mechs.spinner_pulses(0) - before;
            assert!(
                n > 0,
                "shot {shot} at {speed} m/s gave no pulse ({counts:?})"
            );
            counts.push(n);
        }
        eprintln!("spinner: 8 shots at {speed} m/s -> pulses {counts:?}");
        // Repeated identical shots behave alike.
        let (lo, hi) = (*counts.iter().min().unwrap(), *counts.iter().max().unwrap());
        assert!(
            hi - lo <= 2.max(hi / 5),
            "shots at {speed} m/s differ: {counts:?}"
        );
    }
}

#[test]
fn faster_shots_give_more_pulses() {
    let mut last = 0;
    for speed in [1.0f32, 2.0, 3.5] {
        let mut rig = spinner_rig(None);
        rig.add(steel_ball(
            SP_POS + Vec3::new(0.0, -0.05, R),
            Vec3::new(0.0, speed, 0.0),
        ));
        rig.run(8000);
        let pulses = rig.mechs.spinner_pulses(0);
        assert!(
            pulses > last,
            "{speed} m/s: {pulses} pulses, slower shot {last}"
        );
        last = pulses;
    }
}

#[test]
fn a_slow_ball_lifts_the_flap_and_gets_through() {
    // Too slow to turn the flap over: no pulse count is asserted, only that
    // the ball is not stopped or trapped.
    for speed in [0.4f32, 0.6, 0.8] {
        let mut rig = spinner_rig(None);
        let h = rig.add(steel_ball(
            SP_POS + Vec3::new(0.0, -0.04, R),
            Vec3::new(0.0, speed, 0.0),
        ));
        let mut lifted = 0.0f32;
        for _ in 0..1500 {
            rig.step();
            lifted = lifted.max(rig.mechs.spinner_state(0).unwrap().0.abs());
            assert!(is_finite_ball(&rig.ball(h)));
        }
        let b = rig.ball(h);
        assert!(
            b.position.y > SP_POS.y + 0.03 && b.velocity.y >= 0.0,
            "ball at {speed} m/s did not get through: {:?} {:?}",
            b.position,
            b.velocity
        );
        assert!(
            lifted > 0.3,
            "ball at {speed} m/s did not lift the flap ({lifted} rad)"
        );
        rig.run(2000);
        let (angle, w) = rig.mechs.spinner_state(0).unwrap();
        assert!(w.abs() < 0.05 && from_hanging(angle) <= hold_angle() + 0.02);
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// Pure viscous configuration: total rotation ω0/d, pulses floor(·/180°) ± 1.
    #[test]
    fn prop_free_spinner_pulse_count(d in 0.5f32..6.0, omega0 in 2.0f32..120.0, sign in prop::bool::ANY) {
        let w0 = if sign { omega0 } else { -omega0 };
        let s = spin_down(free_spinner_rig(Some(d)), w0, 30.0 / d);
        let analytic = w0 / d;
        prop_assert!((s.total_angle - analytic).abs() <= 0.01 * analytic.abs() + 0.08 / d,
            "total {} vs {}", s.total_angle, analytic);
        let expected = (analytic.abs() / std::f32::consts::PI).floor() as i64;
        prop_assert!((s.pulses as i64 - expected).abs() <= 1, "{} vs {}", s.pulses, expected);
    }

    /// Default model against the f64 reference of the documented ODE. The
    /// random sweep meets initial speeds right at a go/no-go boundary of a
    /// swing, so the tolerance is ±4 (two marginal swings) or 10 %.
    #[test]
    fn prop_spinner_pulses_match_the_documented_model(d in 0.3f32..6.0, omega0 in 2.0f32..250.0, sign in prop::bool::ANY) {
        let sp = defaults().mechs.spinner;
        let w0 = if sign { omega0 } else { -omega0 };
        let s = spin_down(spinner_rig(Some(d)), w0, 60.0);
        let (expected, _) = reference(f64::from(d), f64::from(w0), f64::from(sp.radius), f64::from(sp.friction), f64::from(G));
        prop_assert!(s.pulses.abs_diff(expected) <= 4.max(expected / 10),
            "d {} ω0 {}: {} pulses, reference {}", d, w0, s.pulses, expected);
        prop_assert!(from_hanging(s.total_angle) <= hold_angle() + 0.02,
            "parked {} rad from hanging", from_hanging(s.total_angle));
    }
}

// --------------------------------------------------------------------- VUK

const VUK_SW: SwitchId = SwitchId(64);
const VUK_COIL: CoilId = CoilId(3);
const VUK_POS: Vec3 = Vec3::new(0.0, 0.0, 0.0);
/// Top of the upper deck (layer 1 only); below the default exit point.
const UPPER_TOP: f32 = 0.04;

fn vuk_rig(p: pin_core::schema::table::MechParams) -> Rig {
    let mut b = StaticWorldBuilder::new();
    slab(&mut b, 0.0, layers::MAIN_DECK);
    slab(&mut b, UPPER_TOP, layers::UPPER_DECK);
    Rig::new(
        b.build(),
        Vec3::new(0.0, 0.0, -9.81),
        &[spec(MechKind::Vuk, VUK_POS)
            .with_coil(VUK_COIL)
            .with_switch(VUK_SW)
            .with_params(p)],
    )
}

/// Rolls a ball into the scoop at `speed` from direction `angle`; returns the
/// rig once the ball is held.
fn capture(speed: f32, angle: f32) -> (Rig, BallHandle) {
    let mut rig = vuk_rig(params());
    let from = dir_deg(angle);
    let h = rig.add(steel_ball(
        VUK_POS + from * 0.06 + Vec3::new(0.0, 0.0, R),
        -from * speed,
    ));
    for _ in 0..1000 {
        rig.step();
        if rig.mechs.vuk_ball(0).is_some() {
            assert!(rig.closed_edge(VUK_SW), "capture without a Closed edge");
            return (rig, h);
        }
        assert!(!rig.mechs.mech_switch_closed(0));
    }
    panic!("ball at {speed} m/s from {angle}° was not captured");
}

#[test]
fn vuk_captures_and_holds_a_ball() {
    for (speed, angle) in [(0.3f32, 0.0f32), (0.8, 90.0), (1.5, 200.0), (2.5, 315.0)] {
        let (mut rig, h) = capture(speed, angle);
        assert_eq!(rig.mechs.vuk_ball(0), Some(h));
        // Held at rest in the scoop, switch closed, for as long as the coil is off.
        for _ in 0..1500 {
            rig.step();
            let b = rig.ball(h);
            assert!(
                b.velocity.length() < 1.0e-3,
                "held ball moves: {:?}",
                b.velocity
            );
            assert!(horiz(b.position - VUK_POS).length() <= defaults().mechs.vuk.radius + 1.0e-4);
            assert_eq!(b.layer, 0);
            assert!(rig.mechs.mech_switch_closed(0) && rig.switch_bit(VUK_SW));
        }
        assert_eq!(rig.mechs.vuk_ball(0), Some(h));
    }
}

#[test]
fn vuk_ejects_to_layer_one_and_never_sticks() {
    let d = defaults().mechs.vuk;
    assert_eq!(d.exit_layer, 1);
    for (speed, angle, pulse) in [(0.4f32, 30.0f32, 1u16), (1.0, 180.0, 30), (2.0, 270.0, 200)] {
        let (mut rig, h) = capture(speed, angle);
        rig.run(100);
        rig.mechs.fire_coil(VUK_COIL, pulse);
        let mut opened = false;
        let mut max_z = 0.0f32;
        let mut on_upper_since = None;
        for i in 0..1500 {
            rig.step();
            opened |= rig.opened_edge(VUK_SW);
            let b = rig.ball(h);
            assert!(is_finite_ball(&b));
            max_z = max_z.max(b.position.z);
            if i >= 1 {
                assert_eq!(
                    b.layer, 1,
                    "tick {i} after the eject: ball on layer {}",
                    b.layer
                );
                assert!(
                    b.position.z > UPPER_TOP + R - 1.0e-3,
                    "ejected ball fell back below the upper deck"
                );
            }
            if rig.mechs.vuk_ball(0).is_none() && on_upper_since.is_none() {
                on_upper_since = Some(i);
            }
        }
        assert!(opened, "scoop switch did not open after the eject");
        assert!(!rig.mechs.mech_switch_closed(0) && !rig.switch_bit(VUK_SW));
        assert_eq!(rig.mechs.vuk_ball(0), None, "ball still held: stuck");
        assert!(
            on_upper_since.unwrap() <= 2,
            "eject took {on_upper_since:?} ticks"
        );
        let b = rig.ball(h);
        assert_eq!(b.layer, 1, "ball must end on layer 1");
        // It rose out of the scoop and came to roll on the upper deck.
        assert!(
            max_z >= VUK_POS.z + d.exit_offset[2] - 1.0e-3,
            "max z {max_z}"
        );
        assert!(
            (b.position.z - (UPPER_TOP + R)).abs() < 1.0e-3,
            "ball ends at z {} instead of on the upper deck",
            b.position.z
        );
        // Kicked along the mech direction (default +y), out of the scoop.
        assert!(
            b.position.y > 0.02,
            "ball did not leave the scoop area: {:?}",
            b.position
        );
    }
}

#[test]
fn vuk_exit_zone_is_a_layer_zero_to_one_transition() {
    let d = defaults().mechs.vuk;
    let rig = vuk_rig(params());
    let ts = rig.mechs.transitions();
    assert_eq!(ts.zones().len(), 1, "one exit zone per VUK");
    let z = ts.zones()[0];
    assert_eq!((z.from_layer, z.to_layer), (0, 1));
    let exit = VUK_POS + Vec3::from(d.exit_offset);
    assert!(z.contains(exit), "exit point outside the exit zone");
    assert!(
        !z.contains(VUK_POS + Vec3::new(0.0, 0.0, R)),
        "scoop floor inside the exit zone"
    );
    // The eject move (scoop → exit point) is a layer-0 → layer-1 transition.
    assert_eq!(ts.update(0, VUK_POS + Vec3::new(0.0, 0.0, R), exit), 1);
    assert_eq!(ts.update(1, VUK_POS + Vec3::new(0.0, 0.0, R), exit), 1);
}

#[test]
fn vuk_eject_velocity_and_exit_layer_come_from_parameters() {
    let mut p = params();
    p.eject_speed_m_s = Some(0.6);
    p.kick_speed_m_s = Some(0.5);
    p.angle_deg = Some(180.0);
    p.exit_layer = Some(2);
    p.exit_offset = Some([10.0, 0.0, 80.0]);
    let mut rig = vuk_rig(p);
    let h = rig.add(steel_ball(
        VUK_POS + Vec3::new(0.0, 0.0, R + 1.0e-4),
        Vec3::ZERO,
    ));
    rig.run(50);
    assert_eq!(rig.mechs.vuk_ball(0), Some(h));
    rig.mechs.fire_coil(VUK_COIL, 20);
    rig.step();
    let b = rig.ball(h);
    assert_eq!(b.layer, 2);
    assert!(
        (b.position - Vec3::new(0.010, 0.0, 0.080)).length() < 2.0e-3,
        "{:?}",
        b.position
    );
    assert!(
        (b.velocity - Vec3::new(-0.5, 0.0, 0.6)).length() < 0.03,
        "{:?}",
        b.velocity
    );
    let z = rig.mechs.transitions().zones()[0];
    assert_eq!((z.from_layer, z.to_layer), (0, 2));
}

#[test]
fn vuk_ignores_balls_on_other_layers_and_passing_overhead() {
    let mut rig = vuk_rig(params());
    // A layer-1 ball rolling over the scoop on the upper deck.
    let mut over = steel_ball(
        Vec3::new(-0.05, 0.0, UPPER_TOP + R),
        Vec3::new(0.5, 0.0, 0.0),
    );
    over.layer = 1;
    let h = rig.add(over);
    rig.run(300);
    assert_eq!(rig.mechs.vuk_ball(0), None);
    assert!(!rig.mechs.mech_switch_closed(0));
    assert!(rig.ball(h).position.x > 0.03);
    // A layer-0 ball flying 3 cm above the scoop is not captured either.
    let mut rig = Rig::in_space(&[spec(MechKind::Vuk, VUK_POS).with_switch(VUK_SW)]);
    let h = rig.add(steel_ball(
        Vec3::new(-0.05, 0.0, R + 0.03),
        Vec3::new(0.5, 0.0, 0.0),
    ));
    rig.run(300);
    assert_eq!(rig.mechs.vuk_ball(0), None);
    assert!((rig.ball(h).velocity - Vec3::new(0.5, 0.0, 0.0)).length() < 1.0e-5);
}

#[test]
fn vuk_cycles_repeatedly_without_sticking() {
    let mut rig = vuk_rig(params());
    for cycle in 0..6 {
        let h = rig.add(steel_ball(
            VUK_POS + Vec3::new(0.05, 0.0, R),
            Vec3::new(-0.8, 0.0, 0.0),
        ));
        rig.run(300);
        assert_eq!(
            rig.mechs.vuk_ball(0),
            Some(h),
            "cycle {cycle}: not captured"
        );
        rig.mechs.fire_coil(VUK_COIL, 40);
        rig.run(600);
        assert_eq!(rig.mechs.vuk_ball(0), None, "cycle {cycle}: stuck");
        assert_eq!(rig.ball(h).layer, 1, "cycle {cycle}");
        assert!(!rig.mechs.mech_switch_closed(0));
        rig.solver.remove_ball(h);
    }
}
