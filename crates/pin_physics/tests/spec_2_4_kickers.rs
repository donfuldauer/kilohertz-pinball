//! Subphase 2.4 spec tests: slingshot (exit 1.5–4 m/s, verification 6),
//! kickback, and the spring plunger (verification 5).

#[path = "spec_2_4_common.rs"]
mod common;

use common::*;
use glam::Vec3;
use pin_core::ids::{CoilId, SwitchId};
use pin_core::schema::table::{MechKind, MechParams};
use pin_physics::ball::BallHandle;

const SW: SwitchId = SwitchId(21);
const COIL: CoilId = CoilId(9);
const POS: Vec3 = Vec3::new(-0.05, 0.2, 0.0);

fn kicker_rig(kind: MechKind, angle: f32, p: MechParams) -> Rig {
    let mut p = p;
    p.angle_deg = Some(angle);
    Rig::on_floor(&[spec(kind, POS)
        .with_coil(COIL)
        .with_switch(SW)
        .with_params(p)])
}

/// Ball centre touching the kicker face (plus `gap`) at lateral `offset`.
fn at_face(angle: f32, thickness: f32, gap: f32, offset: f32) -> Vec3 {
    let u = dir_deg(angle);
    let side = Vec3::new(-u.y, u.x, 0.0);
    POS + u * (0.5 * thickness + R + gap) + side * offset + Vec3::new(0.0, 0.0, R)
}

/// A ball approaching the face along −u at `speed`; the coil is fired for
/// `pulse_ms` on the tick after the switch closes. Returns the largest ball
/// speed along u within 80 ms of the contact and the velocity at that time.
fn sling_hit(kind: MechKind, angle: f32, speed: f32, offset: f32, pulse_ms: u16) -> (f32, Vec3) {
    let d = defaults();
    let thickness = if kind == MechKind::Sling {
        d.mechs.sling.thickness
    } else {
        d.mechs.kickback.thickness
    };
    let mut rig = kicker_rig(kind, angle, params());
    let u = dir_deg(angle);
    let gap = if speed > 0.0 { 0.004 } else { 2.0e-5 };
    let h = rig.add(steel_ball(
        at_face(angle, thickness, gap, offset),
        -u * speed,
    ));
    let mut contact = None;
    let mut best = (f32::NEG_INFINITY, Vec3::ZERO);
    for _ in 0..400 {
        if rig.closed_edge(SW) && contact.is_none() {
            contact = Some(rig.tick);
            if pulse_ms > 0 {
                rig.mechs.fire_coil(COIL, pulse_ms);
            }
        }
        rig.step();
        if let Some(t) = contact {
            let v = horiz(rig.ball(h).velocity);
            if v.dot(u) > best.0 {
                best = (v.dot(u), v);
            }
            if rig.tick >= t + 80 {
                return best;
            }
        }
    }
    panic!("{kind:?} at {angle}°: ball at {speed} m/s never closed the switch");
}

#[test]
fn sling_exit_speed_is_in_the_feel_window() {
    for angle in [90.0f32, 45.0, 200.0] {
        for speed in [0.0f32, 0.3, 0.6, 1.0, 1.5, 2.0] {
            let (along, v) = sling_hit(MechKind::Sling, angle, speed, 0.0, 30);
            eprintln!("sling {angle}°: approach {speed:.1} m/s -> exit {along:.3} m/s");
            assert!(
                (EXIT_MIN..=EXIT_MAX).contains(&v.length()),
                "sling {angle}°, approach {speed}: exit {} outside {EXIT_MIN}..{EXIT_MAX}",
                v.length()
            );
            assert!(
                along > 0.99 * v.length(),
                "sling {angle}°, approach {speed}: head-on kick not along the kick direction ({v:?})"
            );
        }
    }
}

#[test]
fn sling_kicks_alike_along_its_band() {
    let mut speeds = Vec::new();
    for offset in [-0.035f32, -0.02, 0.0, 0.02, 0.035] {
        let (along, v) = sling_hit(MechKind::Sling, 90.0, 0.8, offset, 30);
        assert!(
            (EXIT_MIN..=EXIT_MAX).contains(&v.length()),
            "offset {offset}: exit {} outside the window",
            v.length()
        );
        speeds.push(along);
    }
    let mean = speeds.iter().sum::<f32>() / speeds.len() as f32;
    for v in &speeds {
        assert!(
            (v - mean).abs() < 0.1 * mean,
            "band not uniform: {speeds:?}"
        );
    }
}

#[test]
fn sling_without_coil_is_a_passive_rubber() {
    let (along, v) = sling_hit(MechKind::Sling, 90.0, 1.0, 0.0, 0);
    assert!(along > 0.2, "ball did not bounce off the band ({v:?})");
    assert!(v.length() < 1.0, "passive band sped the ball up: {v:?}");
}

#[test]
fn sling_switch_closes_on_contact_only() {
    let mut rig = kicker_rig(MechKind::Sling, 90.0, params());
    let t = defaults().mechs.sling.thickness;
    let h = rig.add(steel_ball(
        at_face(90.0, t, 0.02, 0.0),
        Vec3::new(0.0, -1.0, 0.0),
    ));
    let mut closed_at = None;
    for _ in 0..100 {
        let gap = rig.ball(h).position.y - (POS.y + 0.5 * t + R);
        rig.step();
        if rig.mechs.mech_switch_closed(0) {
            assert!(gap < 1.6e-3, "switch closed {gap} m before contact");
            assert!(rig.switch_bit(SW));
            closed_at = Some(rig.tick);
            break;
        }
        assert!(rig.edge(SW).is_none());
    }
    assert!(closed_at.is_some());
    rig.run(200);
    assert!(
        !rig.mechs.mech_switch_closed(0),
        "switch stuck after the ball left"
    );
}

#[test]
fn coil_pulse_without_a_ball_returns_the_kicker_to_rest() {
    for kind in [MechKind::Sling, MechKind::Kickback] {
        let mut rig = kicker_rig(kind, 90.0, params());
        let stroke = if kind == MechKind::Sling {
            defaults().mechs.sling.stroke
        } else {
            defaults().mechs.kickback.stroke
        };
        assert_eq!(rig.mechs.slider_state(0), Some((0.0, 0.0)));
        rig.mechs.fire_coil(COIL, 30);
        let mut max_x = 0.0f32;
        for _ in 0..30 {
            rig.step();
            let (x, _) = rig.mechs.slider_state(0).unwrap();
            assert!((-1.0e-6..=stroke + 1.0e-6).contains(&x), "{kind:?} x = {x}");
            max_x = max_x.max(x);
        }
        assert!(
            (max_x - stroke).abs() < 1.0e-4,
            "{kind:?} reached {max_x}, stroke {stroke}"
        );
        rig.run(500);
        let (x, v) = rig.mechs.slider_state(0).unwrap();
        assert!(
            x.abs() < 1.0e-5 && v.abs() < 1.0e-4,
            "{kind:?} did not return: {x} {v}"
        );
    }
}

#[test]
fn kicker_at_rest_never_pushes_a_resting_ball() {
    for kind in [MechKind::Sling, MechKind::Kickback] {
        let t = if kind == MechKind::Sling {
            defaults().mechs.sling.thickness
        } else {
            defaults().mechs.kickback.thickness
        };
        let mut rig = kicker_rig(kind, 90.0, params());
        let start = at_face(90.0, t, 2.0e-5, 0.0);
        let h = rig.add(steel_ball(start, Vec3::ZERO));
        for _ in 0..1000 {
            rig.step();
            assert!(rig.ball(h).velocity.length() < 0.01);
        }
        assert!((rig.ball(h).position - start).length() < 2.0e-4);
    }
}

#[test]
fn kickback_returns_a_draining_ball() {
    for speed in [0.0f32, 0.5, 1.0, 2.0] {
        let (along, v) = sling_hit(MechKind::Kickback, 90.0, speed, 0.0, 40);
        eprintln!("kickback: approach {speed:.1} m/s -> exit {along:.3} m/s");
        assert!(
            along > 1.5,
            "kickback, approach {speed}: exit only {along} m/s ({v:?})"
        );
        assert!(along < 8.0, "kickback exit {along} m/s is not plausible");
    }
}

#[test]
fn kicker_exit_speed_follows_force_and_stroke_parameters() {
    // More coil force or more stroke gives a faster ball: the exit speed is
    // emergent from the parameters.
    let run = |force: f32, stroke_mm: f32| {
        let mut p = params();
        p.force_n = Some(force);
        p.stroke = Some(stroke_mm);
        let mut rig = kicker_rig(MechKind::Sling, 90.0, p);
        let h = rig.add(steel_ball(
            at_face(90.0, defaults().mechs.sling.thickness, 2.0e-5, 0.0),
            Vec3::ZERO,
        ));
        rig.run(3);
        rig.mechs.fire_coil(COIL, 40);
        let mut best = 0.0f32;
        for _ in 0..80 {
            rig.step();
            best = best.max(rig.ball(h).velocity.y);
        }
        best
    };
    let forces: Vec<f32> = [15.0f32, 30.0, 60.0, 120.0]
        .iter()
        .map(|f| run(*f, 10.0))
        .collect();
    assert!(
        forces.windows(2).all(|w| w[1] > w[0] * 1.1),
        "exit speed not increasing with force: {forces:?}"
    );
    let strokes: Vec<f32> = [4.0f32, 8.0, 16.0].iter().map(|s| run(40.0, *s)).collect();
    assert!(
        strokes.windows(2).all(|w| w[1] > w[0] * 1.1),
        "exit speed not increasing with stroke: {strokes:?}"
    );
    // Work–energy bound: the ball cannot carry more than the coil's work.
    let m = steel().mass_kg;
    for (f, v) in [15.0f32, 30.0, 60.0, 120.0].iter().zip(&forces) {
        assert!(
            0.5 * m * v * v <= f * 0.010 * 1.05,
            "force {f}: ball energy {} J exceeds the coil work {} J",
            0.5 * m * v * v,
            f * 0.010
        );
    }
}

// ---------------------------------------------------------------- plunger

const PL_SW: SwitchId = SwitchId(30);
const PL_COIL: CoilId = CoilId(12);

struct Plunger {
    rig: Rig,
    ball: Option<BallHandle>,
    stroke: f32,
}

/// A plunger launching along +y on a playfield pitched 6.5°, optionally with
/// a ball resting against its tip.
fn plunger(p: MechParams, with_ball: bool) -> Plunger {
    let d = defaults().mechs.plunger;
    let stroke = p.stroke.map_or(d.stroke, |mm| mm * 1.0e-3);
    let thickness = p.thickness.map_or(d.thickness, |mm| mm * 1.0e-3);
    let mut rig = Rig::new(
        floor_world(),
        pitched_gravity(6.5),
        &[spec(MechKind::Plunger, POS)
            .with_coil(PL_COIL)
            .with_switch(PL_SW)
            .with_params(p)],
    );
    let ball = with_ball.then(|| {
        rig.add(steel_ball(
            POS + Vec3::new(0.0, 0.5 * thickness + R + 5.0e-5, R),
            Vec3::ZERO,
        ))
    });
    Plunger { rig, ball, stroke }
}

impl Plunger {
    fn x(&self) -> f32 {
        self.rig.mechs.slider_state(0).unwrap().0
    }
    fn v(&self) -> f32 {
        self.rig.mechs.slider_state(0).unwrap().1
    }
    /// Pulls to `depth` over 400 ms and holds 400 ms so a ball settles.
    fn pull(&mut self, depth: f32) {
        for i in 1..=400 {
            self.rig.mechs.set_plunger(depth * i as f32 / 400.0);
            self.rig.step();
        }
        self.rig.run(400);
    }
    /// Releases and returns the largest ball speed along +y in 150 ms.
    fn release(&mut self) -> f32 {
        self.rig.mechs.set_plunger(0.0);
        let mut best = 0.0f32;
        for _ in 0..150 {
            self.rig.step();
            if let Some(h) = self.ball {
                best = best.max(self.rig.ball(h).velocity.y);
            }
        }
        best
    }
}

#[test]
fn analog_input_maps_depth_to_rod_travel() {
    let mut pl = plunger(params(), false);
    assert_eq!(pl.rig.mechs.slider_state(0), Some((0.0, 0.0)));
    for depth in [0.25f32, 0.5, 1.0, 0.6, 0.1] {
        pl.rig.mechs.set_plunger(depth);
        pl.rig.run(300);
        assert!(
            (pl.x() + depth * pl.stroke).abs() < 2.0e-4,
            "depth {depth}: rod at {} (stroke {})",
            pl.x(),
            pl.stroke
        );
        assert!(pl.v().abs() < 1.0e-3);
    }
    // Out-of-range and NaN inputs are clamped.
    pl.rig.mechs.set_plunger(7.0);
    pl.rig.run(300);
    assert!((pl.x() + pl.stroke).abs() < 2.0e-4);
    pl.rig.mechs.set_plunger(-3.0);
    pl.rig.run(300);
    assert!(pl.x().abs() < 1.0e-5);
    pl.rig.mechs.set_plunger(f32::NAN);
    pl.rig.run(300);
    assert!(pl.x().abs() < 1.0e-5 && pl.v().abs() < 1.0e-4);
}

#[test]
fn spring_force_is_minus_k_x() {
    // After release the free rod accelerates at −k·x/m: the first-tick
    // velocity is k·|x|/m·dt for every pull depth, spring constant and mass.
    let d = defaults().mechs.plunger;
    for (k, mass_g) in [
        (d.spring_k, d.mass * 1.0e3),
        (400.0, 50.0),
        (1000.0, 120.0),
        (2500.0, 80.0),
    ] {
        for depth in [0.2f32, 0.5, 0.8, 1.0] {
            let mut p = params();
            p.spring_k_n_m = Some(k);
            p.mass_g = Some(mass_g);
            let mut pl = plunger(p, false);
            pl.rig.mechs.set_plunger(depth);
            pl.rig.run(400);
            let x0 = pl.x();
            assert!((x0 + depth * pl.stroke).abs() < 2.0e-4);
            pl.rig.mechs.set_plunger(0.0);
            pl.rig.step();
            let m = mass_g * 1.0e-3;
            let omega = (k / m).sqrt();
            // Exact SHM velocity after one tick; equals k|x|/m·dt to 1st order.
            let expected = -x0 * omega * (omega * DT).sin();
            let linear = k * -x0 / m * DT;
            let v1 = pl.v();
            assert!(
                (v1 - expected).abs() <= 0.03 * linear,
                "k {k} m {m} depth {depth}: v after 1 ms = {v1}, −k·x/m·dt = {linear}"
            );
            // Energy: ½·m·v² at the rest position equals ½·k·x².
            let mut vmax = v1;
            let mut ticks = 1;
            while pl.x() < -1.0e-6 && ticks < 500 {
                pl.rig.step();
                vmax = vmax.max(pl.v());
                ticks += 1;
            }
            let v_end = -x0 * omega;
            assert!(
                (vmax - v_end).abs() <= 0.03 * v_end,
                "k {k} m {m} depth {depth}: peak rod speed {vmax}, expected x·√(k/m) = {v_end}"
            );
            // Quarter period π/2·√(m/k).
            let quarter = 0.5 * std::f32::consts::PI / omega;
            assert!(
                (ticks as f32 * DT - quarter).abs() <= 0.1 * quarter + 1.5 * DT,
                "k {k} m {m} depth {depth}: reached rest after {ticks} ms, quarter period {quarter} s"
            );
            // Inelastic rest stop: the rod stays at rest.
            pl.rig.run(50);
            assert!(pl.x().abs() < 1.0e-5 && pl.v().abs() < 1.0e-3);
        }
    }
}

#[test]
fn launch_speed_increases_monotonically_with_pull_depth() {
    let mut speeds = Vec::new();
    for i in 1..=10 {
        let depth = 0.1 * i as f32;
        let mut pl = plunger(params(), true);
        pl.pull(depth);
        let h = pl.ball.unwrap();
        // The ball followed the tip down the pitched lane.
        let rest_y = POS.y + 0.5 * defaults().mechs.plunger.thickness + R;
        let y = pl.rig.ball(h).position.y;
        assert!(
            (y - (rest_y - depth * pl.stroke)).abs() < 1.5e-3,
            "depth {depth}: ball at y {y}, tip face + r at {}",
            rest_y - depth * pl.stroke
        );
        let v = pl.release();
        speeds.push(v);
        // Spring energy bounds the launch.
        let e_spring = 0.5 * defaults().mechs.plunger.spring_k * (depth * pl.stroke).powi(2);
        let e_ball = 0.5 * steel().mass_kg * v * v;
        assert!(
            e_ball <= 1.02 * e_spring,
            "depth {depth}: ball energy {e_ball} J above the spring energy {e_spring} J"
        );
        assert!(
            e_ball >= 0.2 * e_spring,
            "depth {depth}: ball got only {e_ball} J of the spring's {e_spring} J"
        );
    }
    eprintln!("plunger launch speeds by depth 0.1..1.0: {speeds:?}");
    assert!(
        speeds.windows(2).all(|w| w[1] > w[0]),
        "launch speed not strictly increasing with depth: {speeds:?}"
    );
    assert!(speeds[0] > 0.1 && speeds[9] > 2.0, "{speeds:?}");
}

#[test]
fn released_plunger_at_rest_never_pushes_the_ball() {
    // Never pulled.
    let mut pl = plunger(params(), true);
    let h = pl.ball.unwrap();
    let mut max_y = f32::NEG_INFINITY;
    let start_y = pl.rig.ball(h).position.y;
    for i in 0..3000 {
        if i == 1000 {
            pl.rig.mechs.set_plunger(0.0);
        }
        pl.rig.step();
        let b = pl.rig.ball(h);
        max_y = max_y.max(b.position.y);
        assert!(
            b.velocity.y < 0.02,
            "tick {i}: ball pushed at {} m/s",
            b.velocity.y
        );
    }
    assert!(
        max_y - start_y < 2.0e-4,
        "ball moved up the lane by {}",
        max_y - start_y
    );
    assert_eq!(pl.rig.mechs.slider_state(0), Some((0.0, 0.0)));
    // After a launch with no ball, a ball placed on the rested rod stays put.
    let mut pl = plunger(params(), false);
    pl.pull(1.0);
    pl.release();
    pl.rig.run(300);
    assert!(pl.x().abs() < 1.0e-5 && pl.v().abs() < 1.0e-4);
    let start = POS
        + Vec3::new(
            0.0,
            0.5 * defaults().mechs.plunger.thickness + R + 5.0e-5,
            R,
        );
    let h = pl.rig.add(steel_ball(start, Vec3::ZERO));
    for _ in 0..2000 {
        pl.rig.step();
        assert!(pl.rig.ball(h).velocity.y < 0.02);
    }
    assert!(pl.rig.ball(h).position.y - start.y < 2.0e-4);
}

#[test]
fn slow_release_does_not_launch() {
    // Easing the knob back over a second carries the ball, it does not fire.
    let mut pl = plunger(params(), true);
    pl.pull(1.0);
    let h = pl.ball.unwrap();
    for i in (0..1000).rev() {
        pl.rig.mechs.set_plunger(i as f32 / 1000.0);
        pl.rig.step();
        assert!(
            pl.rig.ball(h).velocity.y < 0.3,
            "ball thrown at {}",
            pl.rig.ball(h).velocity.y
        );
    }
    pl.rig.run(300);
    assert!(pl.rig.ball(h).velocity.length() < 0.05);
}

#[test]
fn auto_plunger_coil_launches_like_an_analog_pull_to_auto_pull() {
    let auto = defaults().mechs.plunger.auto_pull;
    let mut analog = plunger(params(), true);
    analog.pull(auto);
    let v_analog = analog.release();

    let mut pl = plunger(params(), true);
    let h = pl.ball.unwrap();
    pl.rig.mechs.hold_coil(PL_COIL, true);
    pl.rig.run(800);
    assert!(
        (pl.x() + auto * pl.stroke).abs() < 2.0e-4,
        "coil on: rod at {}, expected {}",
        pl.x(),
        -auto * pl.stroke
    );
    pl.rig.mechs.hold_coil(PL_COIL, false);
    let mut v_auto = 0.0f32;
    for _ in 0..150 {
        pl.rig.step();
        v_auto = v_auto.max(pl.rig.ball(h).velocity.y);
    }
    eprintln!("plunger: analog {auto} pull -> {v_analog:.3} m/s, auto-plunger -> {v_auto:.3} m/s");
    assert!(v_auto > 1.0, "auto-plunger launch only {v_auto} m/s");
    assert!(
        (v_auto - v_analog).abs() < 0.1 * v_analog,
        "auto {v_auto} vs analog {v_analog}"
    );

    // A timed pulse (launch button) also fires once it ends.
    let mut pl = plunger(params(), true);
    let h = pl.ball.unwrap();
    pl.rig.mechs.fire_coil(PL_COIL, 700);
    let mut best = 0.0f32;
    for _ in 0..900 {
        pl.rig.step();
        best = best.max(pl.rig.ball(h).velocity.y);
    }
    assert!(
        (best - v_analog).abs() < 0.1 * v_analog,
        "pulse launch {best} vs {v_analog}"
    );

    // auto_pull is a parameter.
    let mut p = params();
    p.auto_pull = Some(0.4);
    let mut pl = plunger(p, false);
    pl.rig.mechs.hold_coil(PL_COIL, true);
    pl.rig.run(600);
    assert!((pl.x() + 0.4 * pl.stroke).abs() < 2.0e-4);
}

#[test]
fn rod_stays_within_its_travel() {
    let mut pl = plunger(params(), true);
    for i in 0..4000u32 {
        let depth = match (i / 250) % 4 {
            0 => 1.0,
            1 => 0.0,
            2 => 0.35,
            _ => (i % 250) as f32 / 250.0,
        };
        pl.rig.mechs.set_plunger(depth);
        pl.rig.step();
        let x = pl.x();
        assert!(
            (-pl.stroke - 1.0e-5..=1.0e-6).contains(&x),
            "tick {i}: rod at {x} outside −stroke..0"
        );
        assert!(is_finite_ball(&pl.rig.ball(pl.ball.unwrap())));
    }
}

/// Largest ball speed along the kick direction (+y) within 60 ms of the
/// contact, for a ball starting `gap` from the band at `speed`, with the
/// coil fired `late` ticks after the tick that follows the switch edge.
fn sling_phase_exit(speed: f32, gap: f32, late: u64) -> (f32, pin_physics::ball::Ball) {
    let t = defaults().mechs.sling.thickness;
    let mut rig = kicker_rig(MechKind::Sling, 90.0, params());
    let h = rig.add(steel_ball(
        at_face(90.0, t, gap, 0.0),
        Vec3::new(0.0, -speed, 0.0),
    ));
    let mut contact = None;
    let mut v = 0.0f32;
    for _ in 0..300 {
        if contact.is_some_and(|c| rig.tick == c + late) {
            rig.mechs.fire_coil(COIL, 30);
        }
        rig.step();
        if contact.is_none() && rig.closed_edge(SW) {
            contact = Some(rig.tick);
        }
        if let Some(c) = contact {
            v = v.max(rig.ball(h).velocity.y);
            if rig.tick == c + 60 {
                return (v, rig.ball(h));
            }
        }
    }
    panic!("ball at {speed} m/s, gap {gap} never closed the sling switch");
}

/// Verification 6 over the contact phase: sling exit in the window wherever
/// within the tick the ball reaches the band.
///
/// Orchestrator ruling (D2): the window applies to a coil fired on the tick
/// after the switch edge. That is the engine's latency (PLAN §2.2: edges
/// collected in tick N are delivered to the script in tick N+1, whose coil
/// commands are applied in that same tick). Later firing is outside the
/// guarantee; see `late_sling_coil_never_traps_the_ball`.
#[test]
fn sling_exit_window_holds_for_every_contact_phase() {
    let mut bad = Vec::new();
    let mut range = (f32::INFINITY, f32::NEG_INFINITY);
    for si in 1..=30 {
        let speed = 0.1 * si as f32;
        for gi in 0..10 {
            let gap = 0.003 + 0.00031 * gi as f32;
            let (v, _) = sling_phase_exit(speed, gap, 0);
            range = (range.0.min(v), range.1.max(v));
            if !(EXIT_MIN..=EXIT_MAX).contains(&v) {
                bad.push((speed, gap, v));
            }
        }
    }
    eprintln!("sling exit speed over speeds × phases: {range:?} m/s");
    assert!(
        bad.is_empty(),
        "(approach, gap, exit) outside {EXIT_MIN}..{EXIT_MAX}: {bad:?}"
    );
}

/// A coil fired one or two ticks later than the engine's latency is outside
/// the exit-window guarantee (Orchestrator ruling on D2). It must still not
/// trap or over-kick the ball: the ball leaves the band along the kick
/// direction, finite and no faster than the window's upper bound.
#[test]
fn late_sling_coil_never_traps_the_ball() {
    let t = defaults().mechs.sling.thickness;
    for si in 1..=30 {
        let speed = 0.1 * si as f32;
        for gi in 0..10 {
            for late in [1u64, 2] {
                let gap = 0.003 + 0.00031 * gi as f32;
                let (v, b) = sling_phase_exit(speed, gap, late);
                assert!(is_finite_ball(&b));
                assert!(
                    v > 0.0 && v <= EXIT_MAX,
                    "approach {speed}, gap {gap}, {late} ticks late: exit {v}"
                );
                assert!(
                    b.velocity.y > 0.0 && b.position.y > POS.y + 0.5 * t + R + 1.0e-3,
                    "approach {speed}, gap {gap}, {late} ticks late: ball not clear of the band"
                );
            }
        }
    }
}

/// A step pull (0 → 0.9 in one tick, as a keyboard-mapped or noisy analog
/// input gives) with a ball resting behind the tip must not throw the ball.
///
/// Bound: the hand pulls the rod at a finite speed, so the ball can at most
/// be struck by a tip moving at the rod's own largest pull speed `v_rod` (as
/// `slider_state` reports it) and leave at `(1 + e)·v_rod` (`e`: steel on
/// rubber from the pair table), the speed of a ball at rest hit by a much
/// heavier body. 5 % and 0.05 m/s are allowed for integration. The pull
/// itself must stay below 3 m/s: a pull is the slow half of a plunge, well
/// under the launch speeds of verification 5, in the spirit of "a released
/// plunger at rest never pushes the ball" (the plunger adds energy only
/// through its spring).
#[test]
fn step_pull_does_not_throw_a_ball_behind_the_tip() {
    let e = defaults()
        .material_pairs
        .iter()
        .find(|p| p.matches(pin_core::Material::Steel, pin_core::Material::Rubber))
        .map(|p| p.restitution)
        .expect("steel–rubber pair");
    let d = defaults().mechs.plunger;
    for (behind, use_coil) in [
        (0.002f32, false),
        (0.010, false),
        (0.030, false),
        (0.040, false),
        (0.010, true),
        (0.030, true),
    ] {
        let mut rig = Rig::on_floor(&[spec(MechKind::Plunger, POS).with_coil(PL_COIL)]);
        // Ball `behind` metres behind the tip's back face (launch axis +y).
        let start = POS + Vec3::new(0.0, -(0.5 * d.thickness + R + behind), R);
        let h = rig.add(steel_ball(start, Vec3::ZERO));
        rig.run(20);
        if use_coil {
            rig.mechs.hold_coil(PL_COIL, true);
        } else {
            rig.mechs.set_plunger(0.9);
        }
        let mut v_ball = 0.0f32;
        let mut v_rod = 0.0f32;
        for _ in 0..400 {
            rig.step();
            let b = rig.ball(h);
            assert!(is_finite_ball(&b));
            v_ball = v_ball.max(b.velocity.length());
            v_rod = v_rod.max(rig.mechs.slider_state(0).unwrap().1.abs());
        }
        eprintln!(
            "plunger pull, ball {behind} m behind the tip (coil {use_coil}): ball max {v_ball:.3} m/s, rod max {v_rod:.3} m/s"
        );
        assert!(v_rod <= 3.0, "pull speed {v_rod} m/s");
        assert!(
            v_ball <= (1.0 + e) * v_rod * 1.05 + 0.05,
            "ball {behind} m behind the tip (coil {use_coil}) thrown at {v_ball} m/s by a rod pulled at {v_rod} m/s"
        );
        // The rod still reaches (or is blocked short of) its commanded depth.
        let x = rig.mechs.slider_state(0).unwrap().0;
        assert!((-d.stroke - 1.0e-5..=1.0e-6).contains(&x));
    }
}
