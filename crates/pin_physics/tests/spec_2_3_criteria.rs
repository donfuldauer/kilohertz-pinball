//! Subphase 2.3 spec tests (Independent Tester): the verification criteria
//! (timing sweep, drop catch, shot spread, zipper) and the feel windows
//! (stroke time, exit speed), with the Appendix C defaults, a 76 mm
//! StraightCapsule, standard_steel and a 6.5° playfield.
//!
//! "Rebound speed" (criteria 1 and 2) is measured as the ball's speed at
//! the first tick after its first impact with the bat at which it is clear
//! of the bat (axis distance ≥ ball radius + base radius + 15 mm); a ball
//! that never leaves within the window is "caught" and its speed at the
//! window end is used.

#[path = "spec_2_3_common.rs"]
mod common;

use common::*;
use glam::{Quat, Vec3};
use pin_core::FlipperConfig;
use pin_core::ids::FlipperId;
use pin_physics::ball::{BallHandle, MovingBodies};

const LEFT: FlipperId = FlipperId(0);

/// Lower-left flipper on the flat playfield: pivot at the origin, bat at
/// −30° (world, from +x) at rest, sweep +50°.
fn left_flipper() -> (FlipperConfig, BatFrame) {
    let base = [0.0, 0.0, PIVOT_Z_MM];
    let rest = rest_for(base, Vec3::Z, 50.0, flat_dir(-30.0));
    let c = config(0, base, Vec3::Z, rest, 50.0);
    let f = frames(std::slice::from_ref(&c))[0];
    (c, f)
}

fn table(c: &FlipperConfig) -> Scene {
    Scene::new(
        table_world(Quat::IDENTITY),
        table_gravity(Quat::IDENTITY),
        std::slice::from_ref(c),
    )
}

/// A ball 200 mm up the table above the middle of the resting bat,
/// travelling straight down the table at 3.5 m/s.
fn incoming(s: &mut Scene, f: &BatFrame) -> BallHandle {
    let c = f.pivot + f.e_rest * (0.5 * L);
    s.solver
        .add_ball(ball(v(c.x, c.y + 0.20, R), v(0.0, -3.5, 0.0)))
        .unwrap()
}

const WINDOW: u64 = 400;

/// Rebound from a bat energised at tick `flip` (None = never) and
/// released at `release`.
fn rebound(c: &FlipperConfig, f: &BatFrame, flip: Option<u64>, release: Option<u64>) -> Shot {
    let mut s = table(c);
    let h = incoming(&mut s, f);
    run_shot(&mut s, h, f, 0, WINDOW, |t, set| {
        if Some(t) == flip {
            set.set_energised(LEFT, true);
        }
        if Some(t) == release {
            set.set_energised(LEFT, false);
        }
    })
}

/// A ball 200 mm up the table travelling straight down at 3.5 m/s, aimed
/// at `frac` of the length of the *held* (raised) bat.
fn incoming_held(s: &mut Scene, c: &FlipperConfig, f: &BatFrame, frac: f32) -> BallHandle {
    let p = f.pivot + f.dir(c.end_angle_deg()) * (frac * L);
    s.solver
        .add_ball(ball(v(p.x, p.y + 0.20, R), v(0.0, -3.5, 0.0)))
        .unwrap()
}

fn rebound_held_aim(c: &FlipperConfig, f: &BatFrame, frac: f32, flip: Option<u64>) -> Shot {
    let mut s = table(c);
    let h = incoming_held(&mut s, c, f, frac);
    run_shot(&mut s, h, f, 0, WINDOW, |t, set| {
        if Some(t) == flip {
            set.set_energised(LEFT, true);
        }
    })
}

/// Bat state at the start of the first-contact tick.
#[derive(Copy, Clone, Debug, PartialEq)]
enum Phase {
    /// On the rest stop, not moving (the ball meets the resting bat first).
    Rest,
    /// Moving up, short of the end stop.
    Stroke,
    /// On the end stop, not moving.
    AtEnd,
    /// Moving back (bounced off the end stop or returning).
    Back,
}

fn phase(c: &FlipperConfig, s: &Shot) -> Phase {
    let (a, w) = s.bat_at_impact.expect("no contact");
    let sign = c.sweep_angle_deg.signum();
    if w * sign < -1.0e-3 {
        Phase::Back
    } else if (a - c.end_angle_deg()).abs() < 0.05 && w.abs() <= 1.0e-3 {
        Phase::AtEnd
    } else if (a - c.rest_angle_deg).abs() < 0.05 && w.abs() <= 1.0e-3 {
        Phase::Rest
    } else {
        Phase::Stroke
    }
}

/// Largest rebound step between neighbouring 1 ms flips that needs no
/// explanation (Orchestrator ruling on criterion 1).
const STEP: f32 = 0.75;

/// The ±20 ms timing sweep around `origin` (the flip pressed at
/// `origin + k`, held) for a ball aimed at `frac` of the held bat. Every step
/// larger than [`STEP`] must be explained by a rigid-stop event (the bat's
/// phase at contact differs: still in stroke / on the stop / bounced back /
/// resting) or by a hit or miss of a ball already rebounding (a different
/// number of contact episodes); the cause is printed. Returns the curve.
fn sweep(
    c: &FlipperConfig,
    f: &BatFrame,
    frac: f32,
    origin: u64,
    label: &str,
    gate_steps: bool,
) -> Vec<(i64, Shot)> {
    let mut curve = Vec::new();
    for k in -20i64..=20 {
        let flip = (origin as i64 + k) as u64;
        let s = rebound_held_aim(c, f, frac, Some(flip));
        assert!(s.impact_tick.is_some(), "{label}: offset {k}: no impact");
        curve.push((k, s));
    }
    println!("{label}: offset_ms rebound_m_s bat_phase_at_contact contact_episodes");
    for (k, s) in &curve {
        println!("{k:>4} {:.3} {:?} {}", s.speed, phase(c, s), s.episodes);
    }
    for w in curve.windows(2) {
        let (k0, s0) = (&w[0].0, &w[0].1);
        let (k1, s1) = (&w[1].0, &w[1].1);
        let d = (s1.speed - s0.speed).abs();
        if d <= STEP {
            continue;
        }
        let (p0, p1) = (phase(c, s0), phase(c, s1));
        let cause = if p0 != p1 {
            Some(format!("rigid-stop event: bat {p0:?} → {p1:?} at contact"))
        } else if s0.episodes != s1.episodes {
            Some(format!(
                "hit/miss of an already-rebounding ball ({} → {} contact episodes)",
                s0.episodes, s1.episodes
            ))
        } else {
            None
        };
        match cause {
            Some(cause) => println!("{label}: step {d:.3} m/s between {k0} and {k1} ms: {cause}"),
            None if gate_steps => panic!(
                "{label}: unexplained rebound step {d:.3} m/s between {k0} ms and {k1} ms \
                 (bat {p0:?} both, {} contact episodes both)",
                s0.episodes
            ),
            None => println!("{label}: UNEXPLAINED step {d:.3} m/s between {k0} and {k1} ms"),
        }
    }
    curve
}

fn min_of(curve: &[(i64, Shot)]) -> (i64, f32) {
    curve.iter().fold((0, f32::INFINITY), |m, x| {
        if x.1.speed < m.1 { (x.0, x.1.speed) } else { m }
    })
}

/// Criterion 1 for a ball aimed at `frac` of the held bat; time 0 = the
/// ball's contact with the held bat. Orchestrator ruling "2.3 criterion 1:
/// the 25 % depth moves to Gate A1": the 2.3 gate needs a dead spot of at
/// most 50 % of the held rebound at both 50 % and 70 % contact; steps over
/// 0.75 m/s are named (unexplained ones reported, not failed); the 25 %
/// depth is reported. Returns (minimum, held rebound).
fn criterion_1(frac: f32) -> (f32, f32) {
    let (c, f) = left_flipper();
    let held = rebound_held_aim(&c, &f, frac, Some(0));
    let th = held.impact_tick.expect("ball missed the held bat");
    assert!(th >= 25, "impact too early for a −20 ms flip: {th}");
    let label = format!("contact {:.0} % of the held bat", frac * 100.0);
    println!(
        "{label}: held-bat rebound {:.3} m/s (impact tick {th})",
        held.speed
    );
    let curve = sweep(&c, &f, frac, th, &label, false);
    let (k, min) = min_of(&curve);
    let ratio = min / held.speed;
    println!(
        "{label}: minimum {min:.3} m/s at {k} ms = {:.1} % of held (gate ≤ 50 %: {}; Gate A1 depth ≤ 25 %: {})",
        100.0 * ratio,
        if ratio <= 0.5 { "met" } else { "NOT met" },
        if ratio <= 0.25 {
            "met"
        } else {
            "not met (report)"
        }
    );
    (min, held.speed)
}

fn gate_dead_spot(frac: f32) {
    let (min, held) = criterion_1(frac);
    assert!(
        min <= 0.5 * held,
        "contact {frac}: sweep minimum {min:.3} m/s > 50 % of the held-flipper rebound {held:.3}"
    );
}

#[test]
fn v1_timing_sweep_mid_bat_50_percent() {
    gate_dead_spot(0.50);
}

#[test]
fn v1_timing_sweep_outer_bat_70_percent() {
    gate_dead_spot(0.70);
}

#[test]
fn v1_dead_spot_aim_scan_report() {
    // Report: sensitivity of the dead-spot depth to where the ball meets the
    // held bat (the minimum sits near the −20 ms edge of the sweep).
    let (c, f) = left_flipper();
    println!(
        "aim_on_held_bat held_m_s min_m_s at_ms ratio_% (flips at −25…−21 ms shown for context)"
    );
    for frac in [0.65f32, 0.70, 0.75, 0.80] {
        let held = rebound_held_aim(&c, &f, frac, Some(0));
        let th = held.impact_tick.expect("ball missed the held bat") as i64;
        let mut m = (0i64, f32::INFINITY);
        let mut outside = String::new();
        for k in -25i64..=20 {
            let s = rebound_held_aim(&c, &f, frac, Some((th + k) as u64));
            if k < -20 {
                outside.push_str(&format!(" {k}:{:.2}", s.speed));
            } else if s.speed < m.1 {
                m = (k, s.speed);
            }
        }
        println!(
            "{:>4.0} % {:.3} {:.3} {:>4} {:.1} [{}]",
            frac * 100.0,
            held.speed,
            m.1,
            m.0,
            100.0 * m.1 / held.speed,
            outside.trim()
        );
    }
}

#[test]
fn v2_drop_catch_rebounds_lower_than_a_held_flipper() {
    let (c, f) = left_flipper();
    let held = rebound(&c, &f, Some(0), None);
    let th = held.impact_tick.expect("ball missed the held bat");
    println!("held rebound {:.3} m/s, impact tick {th}", held.speed);
    for k in [2u64, 3, 5, 8, 10] {
        let s = rebound(&c, &f, Some(0), Some(th - k));
        println!(
            "release {k} ms before impact: rebound {:.3} m/s (caught {})",
            s.speed, s.caught
        );
        assert!(s.impact_tick.is_some(), "release {k} ms before: no impact");
        if k >= 5 {
            assert!(
                s.speed < held.speed,
                "release {k} ms before impact: rebound {:.3} ≥ held {:.3}",
                s.speed,
                held.speed
            );
        }
    }
}

/// A ball at rest against the top of the resting bat at `frac` of its
/// length; the flipper fires at tick 0. Returns the shot.
fn shot_from(frac: f32) -> Shot {
    let (c, f) = left_flipper();
    let mut s = table(&c);
    let m = f.n.cross(f.e_rest);
    let a = f.pivot + f.e_rest * (frac * L);
    let q = v(a.x, a.y, R) + m * 0.05;
    let d = s.set.closest_point(0, 0.0, q).map(|bp| bp.distance);
    // closest_point before the first tick: fall back to a measured set.
    let d = d.unwrap_or_else(|| {
        let mut probe = table(&c);
        probe.advance();
        probe.set.closest_point(0, 0.0, q).unwrap().distance
    });
    let centre = q - m * (d - (R + 2.0e-4));
    let h = s.solver.add_ball(ball(centre, Vec3::ZERO)).unwrap();
    run_shot(&mut s, h, &f, 0, WINDOW, |t, set| {
        if t == 0 {
            set.set_energised(LEFT, true);
        }
    })
}

#[test]
fn v3_shot_spread_release_angle_is_monotone_in_contact_point() {
    let mut angles = Vec::new();
    for frac in [0.25f32, 0.5, 0.85] {
        let s = shot_from(frac);
        assert!(!s.caught && s.impact_tick.is_some(), "frac {frac}: {s:?}");
        let ang = libm::atan2f(s.velocity.y, s.velocity.x).to_degrees();
        println!(
            "contact {:.0} %: release angle {ang:.2}°, speed {:.3} m/s",
            frac * 100.0,
            s.speed
        );
        angles.push(ang);
    }
    let inc = angles[0] < angles[1] && angles[1] < angles[2];
    let dec = angles[0] > angles[1] && angles[1] > angles[2];
    assert!(inc || dec, "release angles not monotone: {angles:?}");
}

#[test]
fn feel_unloaded_stroke_time_rest_to_eos_is_20_to_35_ms() {
    let (c, _) = left_flipper();
    let w = pin_core::PhysicsDefaults::builtin()
        .feel_windows
        .flipper_stroke_time_ms;
    let mut s = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(&c));
    s.advance();
    let eos0 = s.set.eos_closed(LEFT);
    s.set.set_energised(LEFT, true);
    let mut by_angle = None;
    let mut by_switch = None;
    for k in 1..=200u32 {
        s.advance();
        if by_angle.is_none() && s.set.angle_deg(LEFT) >= c.eos_angle_deg() {
            by_angle = Some(k);
        }
        if by_switch.is_none() && s.set.eos_closed(LEFT) != eos0 {
            by_switch = Some(k);
        }
    }
    println!(
        "stroke rest→EOS: angle {by_angle:?} ms, switch {by_switch:?} ms (window {}–{})",
        w.min, w.max
    );
    let t = by_angle.expect("never reached EOS") as f32;
    assert!(
        t >= w.min && t <= w.max,
        "stroke time {t} ms outside {}–{} ms",
        w.min,
        w.max
    );
    let ts = by_switch.expect("EOS switch never changed") as f32;
    assert!(
        (ts - t).abs() <= 1.0,
        "EOS switch at {ts} ms, angle at {t} ms"
    );
}

#[test]
fn feel_full_shot_exit_speed_report() {
    // Report (Gate A1 tunes later): ball at rest on the lowered bat (where a
    // cradled ball lands when the cradle is released), full stroke.
    let w = pin_core::PhysicsDefaults::builtin()
        .feel_windows
        .flipper_exit_speed_m_s;
    for frac in [0.25f32, 0.35, 0.5, 0.7, 0.85] {
        let s = shot_from(frac);
        let inside = s.speed >= w.min && s.speed <= w.max;
        println!(
            "exit speed from {:.0} % of the bat: {:.3} m/s (window {}–{}: {})",
            frac * 100.0,
            s.speed,
            w.min,
            w.max,
            if inside { "inside" } else { "OUTSIDE" }
        );
        assert!(!s.caught, "frac {frac}: ball not launched");
        assert!(
            s.speed > 1.0 && s.speed < 20.0,
            "frac {frac}: implausible exit speed {}",
            s.speed
        );
    }
}

// ------------------------------------------------------------------ zipper

/// Narrowest passage width between two mirror-image bats along x = 0
/// (twice the smallest probe distance), and the y where it is.
fn passage(set: &pin_physics::flipper::FlipperSet, z: f32) -> (f32, f32) {
    let mut best = (f32::INFINITY, 0.0);
    for k in 0..2000 {
        let y = -0.15 + k as f32 * 1.5e-4;
        let p = v(0.0, y, z);
        let d0 = set
            .closest_point(0, 0.0, p)
            .map_or(f32::INFINITY, |b| b.distance);
        let d1 = set
            .closest_point(1, 0.0, p)
            .map_or(f32::INFINITY, |b| b.distance);
        let w = d0 + d1;
        if w < best.0 {
            best = (w, y);
        }
    }
    best
}

fn zipper_pair(half_gap_offset_mm: f32) -> [FlipperConfig; 2] {
    let x = 96.32 + half_gap_offset_mm;
    let lb = [-x, 0.0, PIVOT_Z_MM];
    let rb = [x, 0.0, PIVOT_Z_MM];
    let mut l = config(
        0,
        lb,
        Vec3::Z,
        rest_for(lb, Vec3::Z, 50.0, flat_dir(-30.0)),
        50.0,
    );
    let mut r = config(
        1,
        rb,
        Vec3::Z,
        rest_for(rb, Vec3::Z, -50.0, flat_dir(210.0)),
        -50.0,
    );
    l.zipper_slide_axis_mm = Some([12.0, 0.0, 0.0]);
    r.zipper_slide_axis_mm = Some([-12.0, 0.0, 0.0]);
    l.zipper_slide_speed_mm_s = 200.0;
    r.zipper_slide_speed_mm_s = 200.0;
    [l, r]
}

#[test]
fn v4_zipper_closes_a_48_mm_tip_gap_to_a_27_mm_ball() {
    // Set the pivots so the narrowest gap between the bats is 48 mm.
    let mut off = 0.0f32;
    let mut cfgs = zipper_pair(off);
    for _ in 0..4 {
        let mut s = Scene::new(empty_world(), Vec3::ZERO, &cfgs);
        s.advance();
        let (gap, _) = passage(&s.set, PIVOT_Z_MM * 1.0e-3);
        off += 0.5 * (48.0e-3 - gap) * 1.0e3;
        cfgs = zipper_pair(off);
    }
    let fr = frames(&cfgs);
    let mut s0 = Scene::new(empty_world(), Vec3::ZERO, &cfgs);
    s0.advance();
    let (gap, gy) = passage(&s0.set, fr[0].pivot.z);
    assert!((gap - 0.048).abs() < 2.0e-4, "set-up gap {gap}");
    println!("tip gap {:.2} mm at y = {:.1} mm", gap * 1e3, gy * 1e3);

    for zip in [false, true] {
        let mut s = Scene::new(
            table_world(Quat::IDENTITY),
            table_gravity(Quat::IDENTITY),
            &cfgs,
        );
        if zip {
            s.set.set_zipper(FlipperId(0), true);
            s.set.set_zipper(FlipperId(1), true);
        }
        for _ in 0..100 {
            s.advance();
        }
        if zip {
            for id in [FlipperId(0), FlipperId(1)] {
                assert!(
                    (s.set.base_offset_mm(id) - 12.0).abs() < 1.0e-3,
                    "slide {}",
                    s.set.base_offset_mm(id)
                );
            }
            let (g, _) = passage(&s.set, fr[0].pivot.z);
            assert!((g - 0.024).abs() < 5.0e-4, "zipped gap {g}");
        }
        let h = s
            .solver
            .add_ball(ball(v(0.0, gy + 0.12, R), v(0.0, -0.5, 0.0)))
            .unwrap();
        let mut lowest = f32::INFINITY;
        for _ in 0..1500 {
            s.advance();
            lowest = lowest.min(s.ball(h).position.y);
        }
        let b = s.ball(h);
        println!(
            "zip {zip}: lowest ball y {:.1} mm, final speed {:.4}",
            lowest * 1e3,
            b.velocity.length()
        );
        if zip {
            assert!(
                lowest > gy,
                "27 mm ball passed a zipped gap (lowest y {lowest}, gap at {gy})"
            );
            assert!(
                b.velocity.length() < 0.05,
                "ball not held by the zipped flippers: {:?}",
                b.velocity
            );
        } else {
            assert!(
                lowest < gy - 0.05,
                "27 mm ball did not pass the open 48 mm gap (lowest {lowest})"
            );
        }
    }
}
