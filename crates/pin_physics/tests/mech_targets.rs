//! Verification 2 (spinner pulse counts), 3 (VUK transfer from layer 0 to
//! layer 1) and 6 (drop targets) of Subphase 2.4.

#[path = "mech_util.rs"]
mod util;

use glam::Vec3;
use pin_core::SwitchEdge;
use pin_core::ids::{CoilId, SwitchId};
use pin_core::schema::table::MechKind;
use pin_physics::mech::MechSpec;
use pin_physics::world::layers;
use util::*;

const TARGET_SW: u16 = 60;
const RESET: CoilId = CoilId(12);

fn drop_target_table() -> Table {
    let mut a = MechSpec::new(MechKind::DropTarget, Vec3::ZERO)
        .with_coil(RESET)
        .with_switch(SwitchId(TARGET_SW));
    a.params.angle_deg = Some(0.0);
    let mut b = MechSpec::new(MechKind::DropTarget, v(0.0, 0.03, 0.0))
        .with_coil(RESET)
        .with_switch(SwitchId(TARGET_SW + 1));
    b.params.angle_deg = Some(0.0);
    Table::new(&[a, b], world(&[]), 0.0)
}

/// Hits target 0 head-on at `speed`; returns whether it dropped.
fn hit(speed: f32) -> (bool, Table) {
    let mut t = drop_target_table();
    t.ball(0.08, 0.0, v(-speed, 0.0, 0.0));
    for _ in 0..400 {
        t.tick();
    }
    (t.mechs.drop_target_down(0).unwrap(), t)
}

#[test]
fn drops_only_above_the_threshold_speed() {
    // Default threshold 0.5 m/s (normal impact speed). A ball launched
    // sliding at s rolls at about 5/7·s when it arrives.
    let mut first_drop = None;
    for k in 1..=30 {
        let s = k as f32 * 0.05;
        let (down, t) = hit(s);
        assert_eq!(t.closed(TARGET_SW), down);
        assert!(!t.mechs.drop_target_down(1).unwrap(), "neighbour untouched");
        match (down, first_drop) {
            (true, None) => first_drop = Some(s),
            (false, Some(f)) => panic!("{s} m/s did not drop after {f} m/s did"),
            _ => {}
        }
    }
    let f = first_drop.expect("drops at some speed");
    println!(
        "drop target: first launch speed that drops it {f:.2} m/s (threshold 0.5 m/s at impact)"
    );
    assert!(f > 0.5 && f < 1.0);
}

#[test]
fn dropped_target_lets_balls_pass_and_resets_without_trapping() {
    let (down, mut t) = hit(1.2);
    assert!(down);
    // A second ball rolls over the dropped target.
    let h = t.ball(0.08, 0.0, v(-0.8, 0.0, 0.0));
    for _ in 0..200 {
        t.tick();
    }
    assert!(
        t.get(h).position.x < -0.02,
        "passes over the dropped target"
    );
    t.solver.remove_ball(h);
    // A ball parked right over the target when the bank resets.
    let p = t.solver.balls().map(|(h, _)| h).collect::<Vec<_>>();
    for h in p {
        t.solver.remove_ball(h);
    }
    let h = t.ball(0.0, 0.0, Vec3::ZERO);
    for _ in 0..10 {
        t.tick();
    }
    t.mechs.fire_coil(RESET, 100);
    let mut opened = false;
    for _ in 0..1000 {
        t.tick();
        if t.mechs
            .edges()
            .contains(&(SwitchId(TARGET_SW), SwitchEdge::Opened))
        {
            opened = true;
        }
    }
    assert!(!t.mechs.drop_target_down(0).unwrap(), "target back up");
    assert!(opened && !t.closed(TARGET_SW));
    // The ball is not inside the target: outside its box (half 2.5 × 12.5 ×
    // 16 mm, centre z 16 mm) by at least its radius, or on top of it.
    let b = t.get(h);
    let local = b.position - v(0.0, 0.0, 0.016);
    let outside = local.x.abs() >= 0.0025 + radius(STEEL) - 1e-3
        || local.y.abs() >= 0.0125 + radius(STEEL) - 1e-3
        || local.z >= 0.016 + radius(STEEL) - 1e-3;
    println!("ball after reset under it: {:?}", b.position);
    assert!(outside, "ball not trapped: {:?}", b.position);
    assert!(b.velocity.length() < 2.0);
}

const SPIN_SW: u16 = 70;

/// Verification 2 against the documented model (see `pin_physics::mech::
/// spinner`): pulses within ±2 of an f64 RK4 reference of the same ODE (a
/// spin that barely goes over the top in one and not the other differs by
/// two crossings), the flap settles hanging, and fast spins approach the
/// pure-damping total `ω₀/d`.
#[test]
fn spinner_pulse_count_matches_the_documented_spin_down() {
    let l = 0.028f64;
    let w_c = 2.0 * (1.5 * 9.81 / l).sqrt();
    for (w0, d) in [(20.0f32, 0.8f32), (60.0, 0.8), (150.0, 1.5), (40.0, 0.25), (300.0, 0.8), (8.0, 2.0)] {
        let mut spec = MechSpec::new(MechKind::Spinner, Vec3::ZERO).with_switch(SwitchId(SPIN_SW));
        spec.params.damping_per_s = Some(d);
        let mut t = Table::new(&[spec], world(&[]), 0.0);
        t.mechs.set_spinner_speed(0, w0);
        let mut closures = 0u64;
        let ticks = 30_000;
        for _ in 0..ticks {
            t.tick();
            if t.mechs.edges().contains(&(SwitchId(SPIN_SW), SwitchEdge::Closed)) {
                closures += 1;
            }
        }
        let (ref_pulses, ref_angle) =
            spinner_reference(f64::from(w0), f64::from(d), l, 9.81, ticks as f64 * 1e-3);
        let pulses = t.mechs.spinner_pulses(0);
        let (angle, _) = t.mechs.spinner_state(0).unwrap();
        println!(
            "spinner ω0 {w0} d {d}: rotation {angle:.3} rad (reference {ref_angle:.3}, ω0/d {:.3}), pulses {pulses} (reference {ref_pulses})",
            w0 / d
        );
        assert!((pulses as i64 - ref_pulses as i64).abs() <= 2);
        // Settles hanging.
        let turns = (angle / core::f32::consts::TAU).round();
        assert!((angle - turns * core::f32::consts::TAU).abs() < 0.05, "hanging: {angle}");
        // High-speed limit: between (ω0 − 2ω_c)/d and ω0/d.
        let total = f64::from(angle);
        assert!(total <= f64::from(w0 / d) + 0.1);
        assert!(total >= (f64::from(w0) - 2.0 * w_c) / f64::from(d) - 6.3);
        assert!(closures >= pulses / 2 && closures <= pulses);
    }
}

/// Regression (2.4 review B2): the flap returns to hanging after every
/// shot, so every ball spins it.
#[test]
fn every_successive_shot_spins_the_spinner() {
    for speed in [1.0f32, 1.5, 3.0] {
        let spec = MechSpec::new(MechKind::Spinner, Vec3::ZERO).with_switch(SwitchId(SPIN_SW));
        let mut t = Table::new(&[spec], world(&[]), 0.0);
        let mut per_shot = Vec::new();
        for _ in 0..8 {
            let before = t.mechs.spinner_pulses(0);
            let h = t.ball(0.0, -0.08, v(0.0, speed, 0.0));
            for _ in 0..1500 {
                t.tick();
            }
            t.solver.remove_ball(h);
            for _ in 0..1500 {
                t.tick();
            }
            per_shot.push(t.mechs.spinner_pulses(0) - before);
        }
        println!("spinner pulses per shot at {speed} m/s: {per_shot:?}");
        assert!(per_shot.iter().all(|&p| p >= 2), "{per_shot:?}");
    }
}

#[test]
fn ball_spins_the_spinner() {
    let spec = MechSpec::new(MechKind::Spinner, Vec3::ZERO).with_switch(SwitchId(SPIN_SW));
    let mut t = Table::new(&[spec], world(&[]), 0.0);
    let h = t.ball(0.0, -0.08, v(0.0, 1.5, 0.0));
    for _ in 0..300 {
        t.tick();
    }
    let pulses = t.mechs.spinner_pulses(0);
    println!(
        "ball at 1.5 m/s through the spinner: {pulses} pulses, ball at {:?}",
        t.get(h).position
    );
    assert!(pulses >= 2);
    assert!(t.get(h).position.y > 0.05, "ball passed under the flap");
}

const SCOOP_SW: u16 = 80;
const VUK_COIL: CoilId = CoilId(20);

#[test]
fn vuk_transfers_layer_0_to_layer_1() {
    // Upper deck on layer 1 above and behind the scoop.
    let deck = (v(0.0, 0.1, 0.04), v(0.1, 0.15, 0.0065), layers::UPPER_DECK);
    let spec = MechSpec::new(MechKind::Vuk, Vec3::ZERO)
        .with_coil(VUK_COIL)
        .with_switch(SwitchId(SCOOP_SW));
    for start in [0.05f32, 0.1, 0.2] {
        let mut t = Table::new(&[spec], world(&[deck]), 0.0);
        let h = t.ball(0.0, -start, v(0.0, 0.6, 0.0));
        let mut captured_at = None;
        for i in 0..1000 {
            t.tick();
            if t.closed(SCOOP_SW) {
                captured_at = Some(i);
                break;
            }
        }
        assert!(captured_at.is_some(), "captured");
        assert_eq!(t.mechs.vuk_ball(0), Some(h));
        for _ in 0..200 {
            t.tick();
            assert!(t.get(h).velocity.length() < 1e-6, "held at rest");
        }
        t.mechs.fire_coil(VUK_COIL, 20);
        t.tick();
        assert_eq!(t.get(h).layer, 1, "ejected onto layer 1");
        assert!(t.mechs.vuk_ball(0).is_none());
        let deck_top = 0.0465 + radius(STEEL);
        let mut landed = None;
        for i in 0..1000 {
            t.tick();
            let b = t.get(h);
            assert_eq!(b.layer, 1);
            if landed.is_none()
                && (b.position.z - deck_top).abs() < 1e-3
                && b.velocity.z.abs() < 0.05
            {
                landed = Some((i, b.position));
            }
        }
        println!("VUK (start {start}): landed on the deck {landed:?}");
        assert!(landed.is_some(), "ends on the layer-1 deck, not stuck");
        assert!(!t.closed(SCOOP_SW));
        // The exit zone is published for the table's transitions.
        let z = t.mechs.transitions().zones()[0];
        assert_eq!((z.from_layer, z.to_layer), (0, 1));
    }
}
