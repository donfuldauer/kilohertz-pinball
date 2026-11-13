//! Subphase 2.3 spec tests (Independent Tester): the fixed public API,
//! config validation, the coil/spring/stop model of one bat and the
//! moving-body plumbing. Black box, from the 2.3 brief and §3.3.

#[path = "spec_2_3_common.rs"]
mod common;

use common::*;
use glam::Vec3;
use pin_core::ids::FlipperId;
use pin_core::{FlipperConfig, FlipperShape, Material};
use pin_physics::ball::MovingBodies;
use pin_physics::flipper::{BatShape, FlipperError, FlipperSet, StraightCapsule};
use pin_physics::world::layers;

fn flat(id: u16, rest: f32, sweep: f32) -> FlipperConfig {
    config(id, [0.0, 0.0, PIVOT_Z_MM], Vec3::Z, rest, sweep)
}

fn banana(id: u16) -> FlipperConfig {
    let mut c = flat(id, -30.0, 50.0);
    c.shape = FlipperShape::CurvedBanana {
        length_mm: 80.0,
        arc_radius_mm: 100.0,
        arc_sweep_deg: 30.0,
        tube_radius_mm: 8.0,
    };
    c
}

// ------------------------------------------------------------ construction

#[test]
fn curved_banana_is_rejected_with_shape_not_supported() {
    // Deliverable 6: loading a CurvedBanana flipper before 2.6 is a typed
    // validation error from the constructor.
    match FlipperSet::new(&[banana(0)]) {
        Err(FlipperError::ShapeNotSupported { .. }) => {}
        Err(e) => panic!("wrong error for CurvedBanana: {e:?}"),
        Ok(_) => panic!("CurvedBanana accepted before 2.6"),
    }
    // Also when it is not the first flipper of the set.
    match FlipperSet::new(&[flat(0, -30.0, 50.0), banana(1)]) {
        Err(FlipperError::ShapeNotSupported { .. }) => {}
        Err(e) => panic!("wrong error for CurvedBanana at index 1: {e:?}"),
        Ok(_) => panic!("CurvedBanana accepted before 2.6 (index 1)"),
    }
}

#[test]
fn invalid_configs_are_rejected_not_panicking() {
    // FIXED PUBLIC API: other config errors (non-finite, zero inertia, ...)
    // are FlipperError variants.
    type Edit = fn(&mut FlipperConfig);
    let cases: [(&str, Edit); 12] = [
        ("zero inertia", |c| c.inertia_kg_m2 = 0.0),
        ("negative inertia", |c| c.inertia_kg_m2 = -1.0e-4),
        ("NaN inertia", |c| c.inertia_kg_m2 = f32::NAN),
        ("NaN base position", |c| c.base_position_mm[1] = f32::NAN),
        ("infinite base position", |c| c.base_position_mm[0] = f32::INFINITY),
        ("zero normal", |c| c.surface_normal = [0.0, 0.0, 0.0]),
        ("NaN normal", |c| c.surface_normal = [f32::NAN, 0.0, 1.0]),
        ("NaN rest angle", |c| c.rest_angle_deg = f32::NAN),
        ("infinite sweep", |c| c.sweep_angle_deg = f32::INFINITY),
        ("NaN power torque", |c| c.power_torque_nm = f32::NAN),
        ("NaN hold torque", |c| c.hold_torque_nm = f32::NAN),
        ("NaN rubber restitution", |c| c.rubber_restitution = f32::NAN),
    ];
    for (name, edit) in cases {
        let mut c = flat(0, -30.0, 50.0);
        edit(&mut c);
        let r = std::panic::catch_unwind(|| FlipperSet::new(&[c]).is_err());
        match r {
            Ok(true) => {}
            Ok(false) => panic!("{name}: accepted"),
            Err(_) => panic!("{name}: FlipperSet::new panicked instead of returning an error"),
        }
    }
}

#[test]
fn empty_and_valid_sets_construct() {
    let s = FlipperSet::new(&[]).expect("an empty set is valid");
    assert_eq!(s.body_count(), 0);
    let s = FlipperSet::new(&[flat(0, -30.0, 50.0), flat(1, 210.0, -50.0)]).unwrap();
    assert_eq!(s.body_count(), 2, "one body per config");
}

#[test]
fn bat_shape_is_implemented_for_straight_capsule() {
    // Deliverable 6: a `BatShape` trait implemented for `StraightCapsule`.
    fn implements<T: BatShape>() {}
    implements::<StraightCapsule>();
}

#[test]
fn bodies_are_rubber_on_the_configured_layer() {
    let mut a = flat(0, -30.0, 50.0);
    a.collision_layer = 0;
    let mut b = flat(1, 210.0, -50.0);
    b.collision_layer = 1;
    let mut c = flat(2, 90.0, 40.0);
    c.collision_layer = 4;
    let s = FlipperSet::new(&[a, b, c]).unwrap();
    assert_eq!(s.layer_mask(0), layers::layer_bit(0));
    assert_eq!(s.layer_mask(1), layers::layer_bit(1));
    assert_eq!(s.layer_mask(2), layers::layer_bit(4));
    for i in 0..3 {
        // Deliverable 4: friction/restitution from the pair table "rubber vs
        // ball material".
        assert_eq!(s.material(i), Material::Rubber);
    }
}

// ------------------------------------------------------------ kinematics

#[test]
fn rest_pose_is_exactly_rest_angle() {
    let cfgs = [flat(0, -30.0, 50.0), flat(1, 210.0, -50.0)];
    let mut s = Scene::new(empty_world(), Vec3::ZERO, &cfgs);
    for _ in 0..200 {
        s.advance();
        for c in &cfgs {
            assert_eq!(s.set.angle_deg(c.id), c.rest_angle_deg);
            assert_eq!(s.set.angular_velocity(c.id), 0.0);
            assert_eq!(s.set.base_offset_mm(c.id), 0.0);
        }
    }
}

/// Runs an energised flipper for `ticks`, returning (angle, ω) per tick.
fn stroke(cfg: &FlipperConfig, ticks: usize, release_at: Option<usize>) -> Vec<(f32, f32)> {
    let mut s = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(cfg));
    s.set.set_energised(cfg.id, true);
    let mut out = Vec::new();
    for k in 0..ticks {
        if Some(k) == release_at {
            s.set.set_energised(cfg.id, false);
        }
        s.advance();
        out.push((s.set.angle_deg(cfg.id), s.set.angular_velocity(cfg.id)));
    }
    out
}

#[test]
fn sweep_sign_is_ccw_about_the_normal_and_held_bat_sits_at_end_angle() {
    // §3.3: sweep_angle_deg + = CCW about surface_normal, − = CW; the
    // geometry must rotate by the angle the API reports.
    let normals = [
        Vec3::Z,
        -Vec3::Z,
        v(0.3, -0.2, 0.93).normalize(),
        v(1.0, 0.0, 0.0),
    ];
    for n in normals {
        for sweep in [50.0f32, -50.0] {
            let c = config(0, [10.0, -20.0, 30.0], n, 15.0, sweep);
            let mut s = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(&c));
            s.advance();
            let rest = measure_frame(&s.set, 0, &c);
            s.set.set_energised(c.id, true);
            for _ in 0..200 {
                s.advance();
            }
            let a = s.set.angle_deg(c.id);
            assert!(
                (a - c.end_angle_deg()).abs() < 0.05,
                "n {n:?} sweep {sweep}: held angle {a}, end {}",
                c.end_angle_deg()
            );
            let held = measure_frame(&s.set, 0, &c);
            let turned = signed_angle_deg(rest.e_rest, held.e_rest, n);
            assert!(
                (turned - sweep).abs() < 0.5,
                "n {n:?}: bat turned {turned}° about the normal, sweep {sweep}°"
            );
            // The pivot does not move.
            assert!((rest.pivot - held.pivot).length() < 2.0e-4, "pivot moved");
        }
    }
}

#[test]
fn angular_velocity_is_signed_like_the_sweep() {
    for sweep in [50.0f32, -50.0] {
        let c = flat(0, 0.0, sweep);
        let tr = stroke(&c, 300, Some(150));
        // Upstroke: the first few ticks.
        for (k, &(a, w)) in tr.iter().take(8).enumerate() {
            assert!(w * sweep > 0.0, "sweep {sweep}: tick {k} ω {w}");
            assert!(a * sweep >= 0.0, "sweep {sweep}: tick {k} angle {a}");
        }
        // Return after release: the first ticks after 150 move back.
        let moving_back = tr[151..160].iter().filter(|&&(_, w)| w * sweep < 0.0).count();
        assert!(moving_back >= 5, "sweep {sweep}: no return motion");
        // Angles stay within [rest, end].
        for &(a, _) in &tr {
            let (lo, hi) = if sweep > 0.0 { (0.0, sweep) } else { (sweep, 0.0) };
            assert!(a >= lo - 0.01 && a <= hi + 0.01, "angle {a} outside the sweep");
        }
    }
}

#[test]
fn angle_reports_absolute_degrees_consistent_with_omega() {
    // ω in rad/s, angle in degrees: the angle change over a tick lies
    // between the ω reported at its start and its end (monotone stroke).
    let c = flat(0, -30.0, 50.0);
    let tr = stroke(&c, 40, None);
    let mut prev = (c.rest_angle_deg, 0.0f32);
    for &(a, w) in &tr {
        if a >= c.end_angle_deg() - 1.0e-3 {
            break;
        }
        let rate = (a - prev.0).to_radians() / DT;
        let (lo, hi) = (prev.1.min(w), prev.1.max(w));
        assert!(
            rate >= lo - 0.5 && rate <= hi + 0.5,
            "Δangle/dt {rate} rad/s outside [{lo}, {hi}]"
        );
        prev = (a, w);
    }
}

#[test]
fn release_returns_to_rest_and_stays() {
    for sweep in [50.0f32, -50.0] {
        let c = flat(0, -30.0, sweep);
        let tr = stroke(&c, 700, Some(100));
        for &(a, w) in &tr[500..] {
            assert!((a - c.rest_angle_deg).abs() < 1.0e-3, "not at rest: {a}");
            assert!(w.abs() < 1.0e-3, "still moving: {w}");
        }
    }
}

#[test]
fn eos_switch_changes_exactly_at_the_eos_angle() {
    // Deliverable 2: EOS at eos_angle_fraction of the sweep; the reported
    // state flips when the bat passes the EOS angle (both directions).
    for sweep in [50.0f32, -50.0] {
        let c = flat(0, -30.0, sweep);
        let mut s = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(&c));
        s.advance();
        let at_rest = s.set.eos_closed(c.id);
        let eos = c.eos_angle_deg();
        let past = |a: f32| (a - eos) * sweep.signum();
        s.set.set_energised(c.id, true);
        let mut changes = 0;
        let mut last = at_rest;
        for k in 0..600 {
            if k == 300 {
                s.set.set_energised(c.id, false);
            }
            s.advance();
            let a = s.set.angle_deg(c.id);
            let st = s.set.eos_closed(c.id);
            if st != last {
                changes += 1;
                last = st;
            }
            if past(a).abs() > 0.05 {
                assert_eq!(
                    st != at_rest,
                    past(a) > 0.0,
                    "sweep {sweep}, tick {k}: angle {a}, eos angle {eos}, eos_closed {st}"
                );
            }
        }
        assert!(changes >= 2, "EOS never changed and changed back ({changes})");
        assert_eq!(last, at_rest, "EOS state not restored at rest");
    }
}

#[test]
fn eos_switch_is_closed_at_rest_and_opens_at_end_of_stroke() {
    // Deliverable 2 wording: "power torque until the EOS switch opens at
    // eos_angle_fraction".
    let c = flat(0, -30.0, 50.0);
    let mut s = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(&c));
    s.advance();
    assert!(s.set.eos_closed(c.id), "EOS switch open at rest");
    s.set.set_energised(c.id, true);
    for _ in 0..100 {
        s.advance();
    }
    assert!(!s.set.eos_closed(c.id), "EOS switch still closed at end of stroke");
}

fn stop_bounce(e_stop: f32) -> (f32, f32, f32, f32, Vec<(f32, f32)>) {
    let mut c = flat(0, -30.0, 50.0);
    c.stop_restitution = e_stop;
    let tr = stroke(&c, 900, Some(300));
    // Top stop: peak ω before first reaching the end, most negative ω after.
    let end = c.end_angle_deg();
    let first_end = tr.iter().position(|&(a, _)| a >= end - 1.0e-3).expect("never reached end");
    let peak_up = tr[..=first_end].iter().map(|p| p.1).fold(0.0, f32::max);
    let bounce_top = tr[first_end..300].iter().map(|p| p.1).fold(0.0, f32::min);
    let rest = c.rest_angle_deg;
    let first_rest = 300 + tr[300..].iter().position(|&(a, _)| a <= rest + 1.0e-3).expect("never returned");
    let peak_dn = tr[300..=first_rest].iter().map(|p| p.1).fold(0.0, f32::min);
    let bounce_rest = tr[first_rest..].iter().map(|p| p.1).fold(0.0, f32::max);
    (peak_up, bounce_top, peak_dn, bounce_rest, tr)
}

#[test]
fn end_stops_bounce_with_stop_restitution() {
    // Deliverable 2: end-stop restitution at both stops.
    let p = pin_core::PhysicsDefaults::builtin().resolve(None);
    let e = p.flipper_stop_restitution;
    assert!(e > 0.0);
    let i = p.flipper_inertia;
    let (peak_up, bounce_top, peak_dn, bounce_rest, _) = stop_bounce(e);
    let a_power = p.flipper_power_torque / i * DT;
    let a_spring = p.flipper_return_spring_torque / i * DT;
    // Top stop.
    assert!(peak_up > 20.0, "weak stroke: {peak_up}");
    assert!(
        -bounce_top <= e * (peak_up + a_power) * 1.05 + 0.5,
        "top bounce {bounce_top} rad/s exceeds e·ω = {}",
        e * peak_up
    );
    assert!(
        -bounce_top >= 0.5 * e * peak_up,
        "top stop: no restitution bounce ({bounce_top}, ω {peak_up})"
    );
    // Rest stop.
    assert!(peak_dn < -1.0, "no return stroke: {peak_dn}");
    assert!(
        bounce_rest <= e * (-peak_dn + a_spring) * 1.05 + 0.5,
        "rest bounce {bounce_rest} exceeds e·ω = {}",
        -e * peak_dn
    );
    assert!(
        bounce_rest >= 0.5 * e * -peak_dn,
        "rest stop: no restitution bounce ({bounce_rest}, ω {peak_dn})"
    );
}

#[test]
fn inelastic_stops_do_not_bounce() {
    let (_, bounce_top, _, bounce_rest, tr) = stop_bounce(0.0);
    assert_eq!(bounce_top, 0.0, "e = 0 top stop bounced");
    assert_eq!(bounce_rest, 0.0, "e = 0 rest stop bounced");
    // Held at the end stop exactly once there.
    let end = -30.0 + 50.0;
    let first_end = tr.iter().position(|&(a, _)| a >= end - 1.0e-3).unwrap();
    for &(a, w) in &tr[first_end..300] {
        assert!((a - end).abs() < 1.0e-3 && w == 0.0, "left the stop: {a} {w}");
    }
}

#[test]
fn flipper_ids_address_their_own_bodies() {
    // Non-contiguous ids; body index = config order.
    let mut a = flat(7, -30.0, 50.0);
    a.base_position_mm = [-100.0, 0.0, PIVOT_Z_MM];
    let mut b = flat(3, 210.0, -50.0);
    b.base_position_mm = [100.0, 0.0, PIVOT_Z_MM];
    let cfgs = [a.clone(), b.clone()];
    let mut s = Scene::new(empty_world(), Vec3::ZERO, &cfgs);
    s.advance();
    let f0 = measure_frame(&s.set, 0, &a);
    s.set.set_energised(FlipperId(3), true);
    // An unknown id is ignored (no panic, nothing moves).
    s.set.set_energised(FlipperId(99), true);
    s.set.set_zipper(FlipperId(99), true);
    for _ in 0..100 {
        s.advance();
    }
    assert_eq!(s.set.angle_deg(FlipperId(7)), a.rest_angle_deg);
    assert!((s.set.angle_deg(FlipperId(3)) - b.end_angle_deg()).abs() < 0.05);
    let f0b = measure_frame(&s.set, 0, &a);
    assert!(f0.e_rest.dot(f0b.e_rest) > 0.99999, "body 0 moved for id 3");
    let f1 = measure_frame(&s.set, 1, &b);
    let mut fresh = Scene::new(empty_world(), Vec3::ZERO, &cfgs);
    fresh.advance();
    let f1r = measure_frame(&fresh.set, 1, &b);
    let turned = signed_angle_deg(f1r.e_rest, f1.e_rest, Vec3::Z);
    assert!((turned - b.sweep_angle_deg).abs() < 0.5, "body 1 turned {turned}");
}

#[test]
fn layer_filter_ball_passes_a_flipper_on_another_layer() {
    // Deliverable 1: flippers work on any layer; a ball on another layer
    // never touches the bat.
    for (flip_layer, ball_layer, expect_hit) in [(1u8, 0u8, false), (1, 1, true), (0, 0, true)] {
        let mut c = config(0, [0.0, 0.0, 0.0], Vec3::Z, 0.0, 40.0);
        c.collision_layer = flip_layer;
        let f = frames(std::slice::from_ref(&c))[0];
        let mut s = Scene::new(empty_world(), Vec3::ZERO, std::slice::from_ref(&c));
        let mid = f.pivot + f.e_rest * (0.5 * L);
        let side = f.n.cross(f.e_rest);
        let mut b = ball(mid + side * 0.06, -side * 2.0);
        b.layer = ball_layer;
        let h = s.solver.add_ball(b).unwrap();
        let mut hit = false;
        for _ in 0..80 {
            s.advance();
            hit |= s.hit(h, 0);
        }
        let (_, across, _) = f.local(c.rest_angle_deg, Vec3::ZERO, s.ball(h).position);
        assert_eq!(hit, expect_hit, "flipper layer {flip_layer}, ball layer {ball_layer}");
        if expect_hit {
            assert!(across > 0.0, "ball went through the bat");
        } else {
            assert!(across < -0.05, "ball on another layer was stopped ({across})");
        }
    }
}

#[test]
fn unloaded_stroke_is_independent_of_mount_normal_and_position() {
    // Deliverable 1: kinematics on arbitrary mounting planes. Without a ball
    // the angle trajectory cannot depend on where the bat is mounted.
    let flat_c = flat(0, -30.0, 50.0);
    let reference = stroke(&flat_c, 120, Some(60));
    for n in [
        v(0.0, 0.0, -1.0),
        v(0.0, -0.6, 0.8),
        v(0.577, 0.577, 0.577),
        v(1.0, 0.0, 0.0),
    ] {
        let c = config(0, [123.0, -45.0, 65.0], n, -30.0, 50.0);
        let tr = stroke(&c, 120, Some(60));
        for (k, (x, y)) in reference.iter().zip(&tr).enumerate() {
            assert!(
                (x.0 - y.0).abs() < 1.0e-3 && (x.1 - y.1).abs() < 1.0e-2,
                "normal {n:?} tick {k}: {y:?} vs flat {x:?}"
            );
        }
    }
}
