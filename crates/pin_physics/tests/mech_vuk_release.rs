//! Work item vuk-remove-ball: `MechSet::release_ball` drops a VUK's hold on
//! a ball the caller removed from the solver, so a ball added to the same
//! slot afterwards is free and is captured only through the normal capture
//! path (DECISIONS.md "vuk-remove-ball — hold release on ball removal").

#[path = "mech_util.rs"]
mod util;

use glam::Vec3;
use pin_core::ids::{CoilId, SwitchId};
use pin_core::schema::table::MechKind;
use pin_physics::ball::BallHandle;
use pin_physics::mech::MechSpec;
use proptest::prelude::*;
use util::*;

const COIL: CoilId = CoilId(1);
const SW: u16 = 1;
const PITCH: f32 = 6.5;

/// One VUK at the origin (mech 0), exit 60 mm down the table on layer 0.
fn table() -> Table {
    let mut spec = MechSpec::new(MechKind::Vuk, Vec3::ZERO)
        .with_coil(COIL)
        .with_switch(SwitchId(SW));
    spec.params.exit_layer = Some(0);
    spec.params.exit_offset = Some([0.0, -60.0, 13.5]);
    spec.params.eject_speed_m_s = Some(0.2);
    spec.params.kick_speed_m_s = Some(0.1);
    Table::new(&[spec], world(&[]), PITCH)
}

fn hold_point() -> Vec3 {
    v(0.0, 0.0, radius(STEEL))
}

/// A free spot up the table and to the side, far from the VUK.
const AWAY: (f32, f32) = (0.15, 0.2);

fn capture(t: &mut Table) -> BallHandle {
    let h = t.ball(0.0, 0.0, Vec3::ZERO);
    t.tick();
    assert_eq!(t.mechs.vuk_ball(0), Some(h), "captured");
    assert!(t.closed(SW));
    h
}

#[test]
fn release_ball_clears_only_the_matching_hold() {
    let mut t = table();
    let h = capture(&mut t);
    t.mechs.release_ball(BallHandle(h.0 + 1));
    assert_eq!(t.mechs.vuk_ball(0), Some(h), "another slot: no change");
    t.solver.remove_ball(h).expect("ball");
    t.mechs.release_ball(h);
    assert_eq!(t.mechs.vuk_ball(0), None);
    // Releasing again (or a slot no mech refers to) is a no-op.
    t.mechs.release_ball(h);
    assert_eq!(t.mechs.vuk_ball(0), None);
}

#[test]
fn respawn_in_released_slot_is_free_and_switch_opens_with_an_edge() {
    let mut t = table();
    let h = capture(&mut t);
    t.tick();
    t.solver.remove_ball(h).expect("ball");
    t.mechs.release_ball(h);
    let n = t.ball(AWAY.0, AWAY.1, Vec3::ZERO);
    assert_eq!(n, h, "the solver reuses the slot");
    t.tick();
    assert_eq!(t.mechs.vuk_ball(0), None);
    assert!(!t.closed(SW), "switch opens on the next tick");
    let opened = t
        .mechs
        .edges()
        .iter()
        .filter(|&&(s, e)| s == SwitchId(SW) && e == pin_core::SwitchEdge::Opened)
        .count();
    assert_eq!(opened, 1, "one opening edge: {:?}", t.mechs.edges());
    let p = t.get(n).position;
    assert!((p - v(AWAY.0, AWAY.1, radius(STEEL))).length() < 0.01);
    for _ in 0..50 {
        t.tick();
        assert_eq!(t.mechs.vuk_ball(0), None);
    }
    assert!(
        t.get(n).position.y < AWAY.1 - 1.0e-4,
        "rolls down the table"
    );
}

#[derive(Copy, Clone, Debug)]
enum Coil {
    Off,
    Pulse,
    Hold,
}

/// Sweep: removal 0..40 ticks after the capture; no coil, an eject pulse or
/// the hold coil switched on right before the removal (so the eject fires in
/// the first tick after it); respawn 0..5 ticks after the removal, away
/// from the VUK or inside its zone. Away, the new ball is never held;
/// inside, it is held only after a normal capture (end of a tick, at the
/// hold point), never ejected by a stale hold first.
#[test]
fn sweep_removal_tick_coil_and_respawn_offset() {
    for wait in 0..40u32 {
        for coil in [Coil::Off, Coil::Pulse, Coil::Hold] {
            for offset in 0..5u32 {
                for inside in [false, true] {
                    let case = format!("wait {wait} coil {coil:?} offset {offset} in {inside}");
                    let mut t = table();
                    let h = capture(&mut t);
                    for _ in 0..wait {
                        t.tick();
                    }
                    assert_eq!(t.mechs.vuk_ball(0), Some(h), "{case}: still held");
                    match coil {
                        Coil::Off => {}
                        Coil::Pulse => t.mechs.fire_coil(COIL, 20),
                        Coil::Hold => t.mechs.hold_coil(COIL, true),
                    }
                    t.solver.remove_ball(h).expect("ball");
                    t.mechs.release_ball(h);
                    assert_eq!(t.mechs.vuk_ball(0), None, "{case}: released at once");
                    for _ in 0..offset {
                        t.tick();
                        assert_eq!(t.mechs.vuk_ball(0), None, "{case}");
                        assert!(!t.closed(SW), "{case}");
                    }
                    let n = if inside {
                        t.ball(0.0, 0.0, Vec3::ZERO)
                    } else {
                        t.ball(AWAY.0, AWAY.1, Vec3::ZERO)
                    };
                    assert_eq!(n, h, "{case}: same slot");
                    t.tick();
                    let p = t.get(n).position;
                    if inside {
                        assert_eq!(t.mechs.vuk_ball(0), Some(n), "{case}: normal capture");
                        assert!((p - hold_point()).length() < 1.0e-5, "{case}: {p:?}");
                        assert!(t.closed(SW), "{case}");
                    } else {
                        assert_eq!(t.mechs.vuk_ball(0), None, "{case}: not held");
                        assert!((p - hold_point()).length() > 0.1, "{case}: {p:?}");
                        assert!(!t.closed(SW), "{case}");
                        for _ in 0..5 {
                            t.tick();
                            assert_eq!(t.mechs.vuk_ball(0), None, "{case}");
                        }
                        assert!(t.get(n).position.y < AWAY.1 - 1.0e-6, "{case}: rolls");
                    }
                }
            }
        }
    }
}

#[derive(Copy, Clone, Debug)]
enum Op {
    /// Add a ball in the VUK's zone.
    AddInside,
    /// Add a ball far from the VUK (it rolls away from it).
    AddAway,
    /// Remove the ball in slot `n % 16` (if any), then release it.
    Remove(u8),
    Pulse,
    Hold(bool),
    Tick(u8),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        Just(Op::AddInside),
        Just(Op::AddAway),
        any::<u8>().prop_map(Op::Remove),
        Just(Op::Pulse),
        any::<bool>().prop_map(Op::Hold),
        (0u8..20).prop_map(Op::Tick),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Any interleaving of adds, removals (each followed by
    /// `release_ball`), coil commands and ticks: the VUK only ever holds a
    /// live ball that was added inside its zone (balls added away never
    /// come near it), and right after a removal it holds nothing in that
    /// slot.
    #[test]
    fn hold_only_on_live_balls_added_inside(ops in proptest::collection::vec(op(), 1..60)) {
        let mut t = table();
        // Per slot: Some(true) = live ball added inside, Some(false) = away.
        let mut origin: [Option<bool>; 16] = [None; 16];
        for o in ops {
            match o {
                Op::AddInside | Op::AddAway => {
                    let inside = matches!(o, Op::AddInside);
                    let (x, y) = if inside { (0.0, 0.0) } else { AWAY };
                    let mut b = pin_physics::ball::Ball::new(STEEL, v(x, y, radius(STEEL)));
                    b.velocity = Vec3::ZERO;
                    if let Some(h) = t.solver.add_ball(b) {
                        origin[usize::from(h.0)] = Some(inside);
                    }
                }
                Op::Remove(n) => {
                    let h = BallHandle(n % 16);
                    if t.solver.remove_ball(h).is_some() {
                        t.mechs.release_ball(h);
                        origin[usize::from(h.0)] = None;
                    }
                    prop_assert_ne!(t.mechs.vuk_ball(0), Some(h));
                }
                Op::Pulse => t.mechs.fire_coil(COIL, 20),
                Op::Hold(on) => t.mechs.hold_coil(COIL, on),
                Op::Tick(k) => {
                    for _ in 0..k {
                        t.tick();
                        if let Some(h) = t.mechs.vuk_ball(0) {
                            prop_assert!(t.solver.ball(h).is_some(), "held a dead slot");
                            prop_assert_eq!(origin[usize::from(h.0)], Some(true),
                                "held a ball added away from the VUK");
                            let p = t.get(h).position;
                            prop_assert!((p - hold_point()).length() < 1.0e-5);
                        }
                        prop_assert_eq!(t.closed(SW), t.mechs.vuk_ball(0).is_some());
                    }
                }
            }
        }
    }
}
