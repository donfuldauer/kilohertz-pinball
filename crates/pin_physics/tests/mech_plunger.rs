//! Verification 5 (plunger) of Subphase 2.4: spring force −kx, launch
//! speed monotone in pull depth, a plunger at rest never pushes the ball,
//! auto-plunger coil.

#[path = "mech_util.rs"]
mod util;

use glam::Vec3;
use pin_core::PhysicsDefaults;
use pin_core::ids::{CoilId, SwitchId};
use pin_core::schema::table::MechKind;
use pin_physics::mech::MechSpec;
use util::*;

const Y0: f32 = -0.3;
const COIL: CoilId = CoilId(9);
const LANE: u16 = 50;

fn plunger_table() -> Table {
    let spec = MechSpec::new(MechKind::Plunger, v(0.0, Y0, 0.0))
        .with_coil(COIL)
        .with_switch(SwitchId(LANE));
    Table::new(&[spec], world(&[]), 6.5)
}

/// Ball resting on the tip (after settling).
fn rest_ball(t: &mut Table) -> pin_physics::ball::BallHandle {
    let face = Y0 + 0.006;
    let h = t.ball(0.0, face + radius(STEEL) + 0.0005, Vec3::ZERO);
    for _ in 0..500 {
        t.tick();
    }
    h
}

/// Pull to `depth` over 300 ms, hold 100 ms, release at once; the ball's
/// peak speed within 60 ms after the release.
fn launch(depth: f32) -> f32 {
    let mut t = plunger_table();
    let h = rest_ball(&mut t);
    for i in 1..=300 {
        t.mechs.set_plunger(depth * i as f32 / 300.0);
        t.tick();
    }
    for _ in 0..100 {
        t.tick();
    }
    t.mechs.set_plunger(0.0);
    let mut peak = 0.0f32;
    for _ in 0..60 {
        t.tick();
        peak = peak.max(t.get(h).velocity.y);
    }
    peak
}

#[test]
fn launch_speed_increases_with_pull_depth() {
    let mut last = 0.0f32;
    for k in 1..=10 {
        let depth = k as f32 / 10.0;
        let s = launch(depth);
        println!("plunger: pull {depth:.1} -> launch {s:.3} m/s");
        assert!(s > last, "monotone: {depth} gave {s} after {last}");
        last = s;
    }
    assert!(last > 3.0, "full pull launches the ball: {last}");
}

#[test]
fn released_plunger_at_rest_never_pushes_the_ball() {
    let mut t = plunger_table();
    let h = rest_ball(&mut t);
    let p0 = t.get(h).position;
    let mut max_speed = 0.0f32;
    for _ in 0..2000 {
        t.tick();
        max_speed = max_speed.max(t.get(h).velocity.length());
        assert_eq!(t.mechs.slider_state(0), Some((0.0, 0.0)));
    }
    let p1 = t.get(h).position;
    assert!(
        max_speed < 1.0e-3,
        "resting ball stays at rest: {max_speed}"
    );
    assert!((p1 - p0).length() < 1.0e-4);
    assert!(
        t.closed(LANE),
        "shooter-lane switch closed by the resting ball"
    );
}

#[test]
fn spring_force_is_minus_k_x() {
    // Released from x0 with no ball: x(t) = x0 cos(ωt) until the stop, ω =
    // √(k/m). Measured: time to reach the rest stop (quarter period) and the
    // speed there (x0·ω).
    let d = PhysicsDefaults::builtin();
    let (k, m, stroke) = (
        d.mechs.plunger.spring_k,
        d.mechs.plunger.mass,
        d.mechs.plunger.stroke,
    );
    let w = libm::sqrtf(k / m);
    let mut t = plunger_table();
    t.mechs.set_plunger(1.0);
    for _ in 0..60 {
        t.tick();
    }
    assert_eq!(t.mechs.slider_state(0).unwrap(), (-stroke, 0.0));
    t.mechs.set_plunger(0.0);
    let mut ticks = 0;
    let mut peak = 0.0f32;
    let mut prev_x = -stroke;
    // Force check on the first free tick: Δv = (−k·x/m)·dt.
    t.tick();
    let (x1, v1) = t.mechs.slider_state(0).unwrap();
    assert!((v1 - k * stroke / m * DT).abs() / v1 < 1e-4, "{v1}");
    assert!(x1 > prev_x);
    prev_x = x1;
    while ticks < 100 {
        t.tick();
        ticks += 1;
        let (x, v) = t.mechs.slider_state(0).unwrap();
        peak = peak.max(v);
        if x == 0.0 {
            break;
        }
        prev_x = x;
    }
    let quarter = core::f32::consts::FRAC_PI_2 / w;
    let measured = (ticks + 1) as f32 * DT;
    println!(
        "plunger: quarter period {measured:.4} s (analytic {quarter:.4}), peak {peak:.3} m/s (analytic {:.3})",
        stroke * w
    );
    assert!((measured - quarter).abs() <= 2.0 * DT);
    assert!((peak - stroke * w).abs() / (stroke * w) < 0.05);
    let _ = prev_x;
}

#[test]
fn auto_plunger_coil_launches() {
    let mut t = plunger_table();
    let h = rest_ball(&mut t);
    t.mechs.fire_coil(COIL, 30);
    let mut peak = 0.0f32;
    for _ in 0..100 {
        t.tick();
        peak = peak.max(t.get(h).velocity.y);
    }
    println!("auto-plunger launch: {peak:.3} m/s");
    assert!(peak > 2.0);
    // Slow hand release (over 400 ms) is gentle compared to a snap release.
    let mut t = plunger_table();
    let h = rest_ball(&mut t);
    for i in 1..=200 {
        t.mechs.set_plunger(i as f32 / 200.0);
        t.tick();
    }
    let mut slow = 0.0f32;
    for i in (0..400).rev() {
        t.mechs.set_plunger(i as f32 / 400.0);
        t.tick();
        slow = slow.max(t.get(h).velocity.y);
    }
    println!("slow release peak: {slow:.3} m/s");
    assert!(slow < 0.5 * launch(1.0));
}

/// Regression (2.4 Tester O1): a step in the plunger input moved the tip
/// 45 mm in one tick while the rod reported velocity 0, and a ball behind
/// the tip was thrown at 74 m/s. The pull is rate-limited and the reported
/// velocity is the real motion.
#[test]
fn step_pull_is_rate_limited_and_reports_real_velocity() {
    let spec = MechSpec::new(MechKind::Plunger, v(0.0, Y0, 0.0));
    let mut t = Table::new(&[spec], world(&[]), 0.0);
    let behind = Y0 - 0.006 - radius(STEEL) - 0.001;
    let h = t.ball(0.0, behind, Vec3::ZERO);
    t.tick();
    t.mechs.set_plunger(0.9);
    let mut last = t.mechs.slider_state(0).unwrap().0;
    let mut peak = 0.0f32;
    for _ in 0..100 {
        t.tick();
        let (x, vel) = t.mechs.slider_state(0).unwrap();
        assert!(
            ((x - last) / DT - vel).abs() < 1e-3,
            "velocity {vel} vs motion {}",
            (x - last) / DT
        );
        assert!(vel.abs() <= pin_physics::mech::PULL_SPEED + 1e-4);
        last = x;
        peak = peak.max(t.get(h).velocity.length());
    }
    println!("ball behind a step-pulled tip: peak {peak:.3} m/s");
    assert!(peak < 2.0 * pin_physics::mech::PULL_SPEED, "{peak}");
    assert!((last + 0.9 * 0.05).abs() < 1e-5, "reaches the pull depth");
}
