//! Subphase 2.3 spec tests (Independent Tester): zipper slide kinematics
//! and the tick-exact Thing Flip schedule (deliverable 5).

#[path = "spec_2_3_common.rs"]
mod common;

use common::*;
use glam::Vec3;
use pin_core::ids::{FlipperId, SwitchId};
use pin_core::{FlipperConfig, FlipperStagingMode};
use pin_physics::ball::MovingBodies;

const ID: FlipperId = FlipperId(0);
const TRIGGER: SwitchId = SwitchId(500);

fn zipper(axis: Option<[f32; 3]>, speed: f32) -> FlipperConfig {
    let mut c = config(0, [0.0, 0.0, PIVOT_Z_MM], Vec3::Z, -30.0, 50.0);
    c.zipper_slide_axis_mm = axis;
    c.zipper_slide_speed_mm_s = speed;
    c
}

/// Sample points around the resting bat (probe distances are compared
/// before and after the slide).
fn probe_points(f: &BatFrame) -> Vec<Vec3> {
    let m = f.n.cross(f.e_rest);
    let mut out = Vec::new();
    for i in 0..8 {
        let t = -0.01 + i as f32 * 0.012;
        for s in [-0.03f32, -0.015, 0.015, 0.03] {
            out.push(f.pivot + f.e_rest * t + m * s + f.n * (0.002 * i as f32 - 0.007));
        }
    }
    out
}

#[test]
fn zipper_slides_at_its_speed_to_the_axis_extent_and_back() {
    // Axis (9, 12, 0) mm: extent 15 mm. 150 mm/s = 0.15 mm per tick.
    let c = zipper(Some([9.0, 12.0, 0.0]), 150.0);
    let f = frames(std::slice::from_ref(&c))[0];
    let mut s = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(&c));
    s.advance();
    let pts = probe_points(&f);
    let before: Vec<f32> = pts.iter().map(|&p| s.set.closest_point(0, 0.0, p).unwrap().distance).collect();
    s.set.set_zipper(ID, true);
    let mut prev = 0.0f32;
    for k in 1..=160u32 {
        s.advance();
        let o = s.set.base_offset_mm(ID);
        let ideal = (0.15 * k as f32).min(15.0);
        assert!(o >= prev - 1.0e-5, "slide went back at tick {k}: {prev} → {o}");
        assert!(o <= 15.0 + 1.0e-4, "overshoot {o}");
        assert!((o - ideal).abs() <= 0.15 + 1.0e-3, "tick {k}: offset {o}, expected ≈ {ideal}");
        prev = o;
    }
    assert!((prev - 15.0).abs() < 1.0e-3, "final offset {prev}");
    // The whole bat moved by the axis vector.
    let shift = v(9.0, 12.0, 0.0) * 1.0e-3;
    for (p, d0) in pts.iter().zip(&before) {
        let d1 = s.set.closest_point(0, 0.0, *p + shift).unwrap().distance;
        assert!((d1 - d0).abs() < 2.0e-5, "bat not translated by the axis: {d0} vs {d1} at {p:?}");
    }
    // Back when the command is off.
    s.set.set_zipper(ID, false);
    let mut prev = 15.0f32;
    for k in 1..=160u32 {
        s.advance();
        let o = s.set.base_offset_mm(ID);
        let ideal = (15.0 - 0.15 * k as f32).max(0.0);
        assert!(o <= prev + 1.0e-5 && o >= -1.0e-4, "tick {k}: {prev} → {o}");
        assert!((o - ideal).abs() <= 0.15 + 1.0e-3, "return tick {k}: offset {o}, expected ≈ {ideal}");
        prev = o;
    }
    assert!(prev.abs() < 1.0e-3);
    for (p, d0) in pts.iter().zip(&before) {
        let d1 = s.set.closest_point(0, 0.0, *p).unwrap().distance;
        assert!((d1 - d0).abs() < 2.0e-5, "bat not back home");
    }
}

#[test]
fn zipper_reverses_mid_slide() {
    let c = zipper(Some([0.0, 10.0, 0.0]), 100.0);
    let mut s = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(&c));
    s.set.set_zipper(ID, true);
    for _ in 0..40 {
        s.advance();
    }
    let mid = s.set.base_offset_mm(ID);
    assert!(mid > 3.0 && mid < 5.0, "{mid}");
    s.set.set_zipper(ID, false);
    for _ in 0..20 {
        s.advance();
    }
    let back = s.set.base_offset_mm(ID);
    assert!((back - (mid - 2.0)).abs() < 0.11, "{mid} → {back}");
}

#[test]
fn non_zipper_flipper_ignores_the_zipper_command() {
    let c = zipper(None, 200.0);
    let mut s = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(&c));
    s.set.set_zipper(ID, true);
    for _ in 0..100 {
        s.advance();
        assert_eq!(s.set.base_offset_mm(ID), 0.0);
    }
}

#[test]
fn zipper_slide_does_not_change_the_unloaded_stroke() {
    let a = zipper(Some([12.0, 0.0, 0.0]), 300.0);
    let b = zipper(None, 0.0);
    let mut sa = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(&a));
    let mut sb = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(&b));
    sa.set.set_zipper(ID, true);
    sa.set.set_energised(ID, true);
    sb.set.set_energised(ID, true);
    for k in 0..120 {
        if k == 60 {
            sa.set.set_energised(ID, false);
            sb.set.set_energised(ID, false);
        }
        sa.advance();
        sb.advance();
        assert!((sa.set.angle_deg(ID) - sb.set.angle_deg(ID)).abs() < 1.0e-4, "tick {k}");
    }
}

// --------------------------------------------------------------- Thing Flip

fn thing(delay_ms: f32, hold_ms: f32) -> FlipperConfig {
    let mut c = config(0, [0.0, 0.0, PIVOT_Z_MM], Vec3::Z, -30.0, 50.0);
    c.staging = FlipperStagingMode::AutoSensorThingFlip {
        trigger_switch: TRIGGER,
        delay_ms,
        hold_duration_ms: hold_ms,
    };
    c
}

type Trace = Vec<(u32, u32)>;

/// Thing Flip run: switch edges `(tick, switch, closed)` delivered before
/// the tick of that index runs.
fn run_thing(c: &FlipperConfig, edges: &[(u64, SwitchId, bool)], ticks: u64) -> Trace {
    let mut s = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(c));
    let mut out = Vec::new();
    while s.tick < ticks {
        for &(t, sw, closed) in edges {
            if t == s.tick {
                s.set.on_switch_edge(sw, closed, t);
            }
        }
        s.advance();
        out.push((s.set.angle_deg(ID).to_bits(), s.set.angular_velocity(ID).to_bits()));
    }
    out
}

/// The same bat driven by hand: energised for ticks `on..off` (per window).
fn run_manual(windows: &[(u64, u64)], ticks: u64) -> Trace {
    let c = thing(0.0, 0.0);
    let mut c = c;
    c.staging = FlipperStagingMode::DedicatedButton(pin_core::CabinetInput::LeftFlipper);
    let mut s = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(&c));
    let mut out = Vec::new();
    while s.tick < ticks {
        let on = windows.iter().any(|&(a, b)| s.tick >= a && s.tick < b);
        s.set.set_energised(ID, on);
        s.advance();
        out.push((s.set.angle_deg(ID).to_bits(), s.set.angular_velocity(ID).to_bits()));
    }
    out
}

fn first_diff(a: &Trace, b: &Trace) -> Option<usize> {
    a.iter().zip(b).position(|(x, y)| x != y)
}

#[test]
fn thing_flip_fires_delay_after_the_trigger_and_holds_tick_exact() {
    // Deliverable 5: energises delay_ms after the trigger closes, holds for
    // hold_duration_ms, tick-exact. Equivalent to the button pressed at
    // T + delay and released at T + delay + hold.
    for (t, d, h) in [(10u64, 30u64, 50u64), (7, 0, 20), (3, 12, 1), (40, 25, 120)] {
        let c = thing(d as f32, h as f32);
        let a = run_thing(&c, &[(t, TRIGGER, true)], 400);
        let b = run_manual(&[(t + d, t + d + h)], 400);
        assert_eq!(first_diff(&a, &b), None, "trigger {t}, delay {d}, hold {h}: first differing tick");
        // And it did move.
        assert!(a.iter().any(|x| f32::from_bits(x.0) != c.rest_angle_deg));
    }
}

#[test]
fn thing_flip_hold_is_timed_not_switch_driven() {
    // The trigger opening again during the delay or hold does not cut the
    // flip short.
    let c = thing(15.0, 60.0);
    let a = run_thing(&c, &[(5, TRIGGER, true), (8, TRIGGER, false)], 300);
    let b = run_manual(&[(20, 80)], 300);
    assert_eq!(first_diff(&a, &b), None);
    let a = run_thing(&c, &[(5, TRIGGER, true), (40, TRIGGER, false)], 300);
    assert_eq!(first_diff(&a, &b), None);
}

#[test]
fn thing_flip_ignores_other_switches_and_open_edges() {
    let c = thing(10.0, 30.0);
    let rest = run_manual(&[], 200);
    let a = run_thing(&c, &[(5, SwitchId(501), true), (20, TRIGGER, false), (30, c.eos_switch, true)], 200);
    assert_eq!(first_diff(&a, &rest), None);
}

#[test]
fn thing_flip_fires_again_on_a_later_trigger() {
    let c = thing(10.0, 30.0);
    let a = run_thing(&c, &[(5, TRIGGER, true), (6, TRIGGER, false), (300, TRIGGER, true)], 600);
    let b = run_manual(&[(15, 45), (310, 340)], 600);
    assert_eq!(first_diff(&a, &b), None);
}

#[test]
fn thing_flip_is_deterministic() {
    let c = thing(17.0, 33.0);
    let edges = [(12u64, TRIGGER, true), (13, TRIGGER, false), (150, TRIGGER, true)];
    let a = run_thing(&c, &edges, 400);
    for _ in 0..9 {
        assert_eq!(run_thing(&c, &edges, 400), a);
    }
}
