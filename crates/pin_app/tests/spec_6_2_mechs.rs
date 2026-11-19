//! Subphase 6.2 spec tests (Independent Tester): the "Sky Deck" mechanisms
//! by physics contact (brief C4).
//!
//! A ball is spawned onto each mechanism (`spawn_ball_on_layer`) and the
//! physics, not a forced switch, must close the mechanism's switch: the
//! captive Newton ball, each upper-deck drop target and the complete bank,
//! both slings, the loop (`loop_entry` then `loop_top`) and ramp
//! (`ramp_entry` then `ramp_made`) shots, the shooter lane (plunger launch)
//! and a drain into the trough. A lower-flipper button press moves its
//! flipper in the published snapshot.

#[path = "fixtures/spec_6_2/common.rs"]
mod common;

use common::{
    STEEL, add, assert_healthy, at, big_stack, cabinet, clear_table, dir, dot, finite, game_seeded,
    hits, len, load, mech, mech_index, newton_aim, press, scale, steel_radius_m, strike_drop,
    table, wait_hit,
};
use pin_core::input::CabinetInput;
use pin_core::schema::table::SwitchKind;
use pin_sim::{BallHandle, Simulation};

// ---------------------------------------------------------------------------
// Newton ball
// ---------------------------------------------------------------------------

/// A ball rolled into the captive Newton ball moves it and closes
/// `newton_sw`: struck from behind along the captive path's tangent at 0.7,
/// 1.0 and 1.5 m/s, each on a fresh game, the switch closes within 300
/// ticks and the captive ball moves along its path (> 2 mm).
#[test]
fn c4_newton_ball_strike() {
    big_stack(|| {
        let def = mech("newton");
        let sw = def.switch.as_deref().expect("newton switch");
        assert_eq!(sw, "newton_sw");
        let layer = def.layer.unwrap_or(0);
        for speed in [1.5f32, 1.0, 0.7] {
            let mut sim = game_seeded(3);
            assert!(sim.solver().newton_chain_count() >= 1, "a Newton chain");
            let (d, c0) = newton_aim(&sim);
            let pos = [c0[0] - d[0] * 0.045, c0[1] - d[1] * 0.045, c0[2]];
            sim.spawn_ball_on_layer(STEEL, layer, pos, scale(d, speed))
                .expect("spawn");
            let mut moved = 0.0f32;
            let mut hit = None;
            let before = hits(&sim, sw);
            for _ in 0..300 {
                sim.advance(1);
                let q = sim.solver().newton_chain(0).ball_position(0).to_array();
                moved = moved.max(len([q[0] - c0[0], q[1] - c0[1], q[2] - c0[2]]));
                if hit.is_none() && hits(&sim, sw) > before {
                    hit = Some(sim.tick());
                }
            }
            println!("newton {speed} m/s: hit {hit:?}, captive moved up to {moved:.4} m");
            assert!(
                hit.is_some(),
                "newton: a {speed} m/s strike closed no switch"
            );
            assert!(moved > 0.002, "newton: captive moved {moved} m");
            clear_table(&mut sim);
            sim.advance(50);
            assert_healthy(&sim, "newton");
        }
    });
}

// ---------------------------------------------------------------------------
// Drop targets (upper deck)
// ---------------------------------------------------------------------------

/// Each drop target falls and closes its switch when a deck ball strikes
/// it (1.0 and 1.5 m/s).
#[test]
fn c4_each_drop_target_by_contact() {
    big_stack(|| {
        for id in ["drop_1", "drop_2", "drop_3"] {
            for speed in [1.0f32, 1.5] {
                let mut sim = game_seeded(5);
                clear_table(&mut sim);
                sim.advance(50);
                let idx = mech_index(id);
                assert_eq!(sim.mech_set().drop_target_down(idx), Some(false), "{id} up");
                let hit = strike_drop(&mut sim, id, speed);
                assert!(
                    hit.is_some(),
                    "{id} at {speed} m/s: switch closed by contact"
                );
                assert_eq!(
                    sim.mech_set().drop_target_down(idx),
                    Some(true),
                    "{id}: the impact dropped the target"
                );
                assert_healthy(&sim, id);
            }
        }
    });
}

/// The bank completes: struck one after the other in one game, all three
/// targets are down together when the third falls.
#[test]
fn c4_drop_bank_completes() {
    big_stack(|| {
        let mut sim = game_seeded(6);
        clear_table(&mut sim);
        sim.advance(50);
        let idx: Vec<usize> = ["drop_1", "drop_2", "drop_3"]
            .iter()
            .map(|d| mech_index(d))
            .collect();
        for id in ["drop_1", "drop_2", "drop_3"] {
            assert!(strike_drop(&mut sim, id, 1.2).is_some(), "{id} hit");
        }
        let down: Vec<Option<bool>> = idx
            .iter()
            .map(|&i| sim.mech_set().drop_target_down(i))
            .collect();
        assert_eq!(down, vec![Some(true); 3], "the whole bank is down");
        for s in ["drop_1_sw", "drop_2_sw", "drop_3_sw"] {
            assert!(hits(&sim, s) >= 1, "{s} counted");
        }
        assert_healthy(&sim, "bank");
    });
}

// ---------------------------------------------------------------------------
// Slings
// ---------------------------------------------------------------------------

/// A ball rolled into each sling's band closes its switch and is kicked
/// out along the kick direction (>= 1.5 m/s, the Appendix C sling window
/// minimum).
#[test]
fn c4_slings_by_contact() {
    big_stack(|| {
        for (id, sw) in [
            ("sling_left", "sling_left_sw"),
            ("sling_right", "sling_right_sw"),
        ] {
            let def = mech(id);
            assert_eq!(def.switch.as_deref(), Some(sw));
            let p = def.position.expect("sling position");
            let u = dir(def.angle_deg.unwrap_or(90.0));
            let mut sim = game_seeded(8);
            clear_table(&mut sim);
            sim.advance(50);
            let h = sim
                .spawn_ball_on_layer(
                    STEEL,
                    def.layer.unwrap_or(0),
                    at(p, u, 0.04),
                    scale(u, -1.0),
                )
                .expect("spawn");
            assert!(
                wait_hit(&mut sim, sw, 300).is_some(),
                "{id}: switch by contact"
            );
            let mut kick = 0.0f32;
            for _ in 0..150 {
                sim.advance(1);
                if let Some(b) = sim.ball(h) {
                    kick = kick.max(dot(b.velocity_m_s, u));
                }
            }
            println!("{id}: kick {kick:.3} m/s");
            assert!(kick >= 1.5, "{id}: kicked out at {kick} m/s");
            let _ = sim.remove_ball(h);
            assert_healthy(&sim, id);
        }
    });
}

// ---------------------------------------------------------------------------
// Loop and ramp shots
// ---------------------------------------------------------------------------

fn switch_pos(name: &str) -> ([f32; 3], u8) {
    let s = table()
        .switches
        .iter()
        .find(|s| s.name == name)
        .unwrap_or_else(|| panic!("switch {name}"));
    (
        s.position_m
            .unwrap_or_else(|| panic!("switch {name} has a position")),
        s.layer,
    )
}

/// Searches, in a fixed order, a launch (direction, speed) from 25 mm
/// before `entry` that makes the physics close `entry` and then `top`
/// within the shot's window. Directions: the bearing to `top` +-0..60 deg
/// and up the table (90 deg) +-0..40 deg; speeds 2-5 m/s. Returns the first
/// that works (attract mode; no switch is forced).
fn shot_by_contact(shot: &str, entry: &str, top: &str) -> Option<(f32, f32, u64)> {
    let window_ticks = table()
        .shots
        .iter()
        .find(|s| s.name == shot)
        .map(|s| (s.window_s * 1000.0).round() as u64)
        .unwrap_or_else(|| panic!("shot {shot}"));
    let (pe, layer) = switch_pos(entry);
    let (pt, _) = switch_pos(top);
    let bearing = (pt[1] - pe[1]).atan2(pt[0] - pe[0]).to_degrees();
    let mut dirs = Vec::new();
    for d in [0.0f32, 15.0, -15.0, 30.0, -30.0, 45.0, -45.0, 60.0, -60.0] {
        dirs.push(bearing + d);
    }
    for d in [0.0f32, 20.0, -20.0, 40.0, -40.0] {
        dirs.push(90.0 + d);
    }
    let r = steel_radius_m();
    let mut sim = load();
    sim.advance(100);
    for speed in [3.0f32, 4.0, 2.0, 5.0] {
        for &a in &dirs {
            clear_table(&mut sim);
            sim.advance(20);
            let u = dir(a);
            let pos = add([pe[0], pe[1], pe[2] + r], scale(u, -0.025));
            let Ok(h) = sim.spawn_ball_on_layer(STEEL, layer, pos, scale(u, speed)) else {
                continue;
            };
            let e0 = hits(&sim, entry);
            let t0 = hits(&sim, top);
            let mut entry_tick = None;
            for _ in 0..4_000 {
                sim.advance(1);
                if let Some(b) = sim.ball(h) {
                    assert!(finite(&b), "finite");
                }
                if entry_tick.is_none() && hits(&sim, entry) > e0 {
                    entry_tick = Some(sim.tick());
                }
                if hits(&sim, top) > t0 {
                    if let Some(et) = entry_tick {
                        let dt = sim.tick() - et;
                        if dt <= window_ticks {
                            clear_table(&mut sim);
                            assert_healthy(&sim, shot);
                            return Some((a, speed, dt));
                        }
                    }
                    break;
                }
                if sim.ball(h).is_none() {
                    break;
                }
            }
        }
    }
    None
}

/// The loop: a ball rolled into the upper-left wrap-around loop closes
/// `loop_entry` and then `loop_top` within the shot window.
#[test]
fn c4_loop_shot_by_contact() {
    big_stack(|| {
        let r = shot_by_contact("loop", "loop_entry", "loop_top");
        println!("loop: {r:?} (direction deg, speed m/s, entry->top ticks)");
        assert!(r.is_some(), "no launch made the loop shot by contact");
    });
}

/// The ramp: a ball rolled up the left ramp closes `ramp_entry` and then
/// `ramp_made` within the shot window.
#[test]
fn c4_ramp_shot_by_contact() {
    big_stack(|| {
        let r = shot_by_contact("ramp", "ramp_entry", "ramp_made");
        println!("ramp: {r:?} (direction deg, speed m/s, entry->top ticks)");
        assert!(r.is_some(), "no launch made the ramp shot by contact");
    });
}

// ---------------------------------------------------------------------------
// Shooter lane, trough, flippers
// ---------------------------------------------------------------------------

/// The served ball is launched with the cabinet `launch` button and passes
/// the shooter-lane switch.
#[test]
fn c4_shooter_lane_by_launch() {
    big_stack(|| {
        let mut sim = game_seeded(9);
        sim.advance(500);
        let before = hits(&sim, "shooter_lane");
        press(&mut sim, CabinetInput::Launch, 60);
        let hit = wait_hit(&mut sim, "shooter_lane", 3_000);
        assert!(
            hit.is_some() || hits(&sim, "shooter_lane") > before,
            "the launched ball passed shooter_lane"
        );
        assert_healthy(&sim, "shooter lane");
    });
}

/// A ball rolling down between the lower flippers drains: a trough switch
/// closes and the ball leaves the table (drain counter rises). Three start
/// points across the outlane gap.
#[test]
fn c4_drain_into_the_trough() {
    big_stack(|| {
        let t = table();
        let fl = |id: &str| {
            t.manifest
                .flippers
                .iter()
                .find(|f| f.id == id)
                .unwrap_or_else(|| panic!("{id}"))
                .base_position
        };
        let (l, r) = (fl("lower_left"), fl("lower_right"));
        let mid = [
            (l[0] + r[0]) / 2.0,
            (l[1] + r[1]) / 2.0,
            (l[2] + r[2]) / 2.0,
        ];
        let troughs: Vec<String> = t
            .switches
            .iter()
            .filter(|s| s.kind == SwitchKind::Trough)
            .map(|s| s.name.clone())
            .collect();
        assert!(troughs.len() >= 2, "trough switches");
        for dx in [-5.0f32, 0.0, 5.0] {
            let mut sim = load();
            sim.advance(100);
            let before: u32 = troughs.iter().map(|s| hits(&sim, s)).sum();
            let drained = sim.counters().balls_drained;
            let h: BallHandle = sim
                .spawn_ball_on_layer(
                    STEEL,
                    0,
                    at([mid[0] + dx, mid[1] + 30.0, mid[2]], [0.0; 3], 0.0),
                    [0.0, -1.0, 0.0],
                )
                .expect("spawn");
            let mut ok = false;
            for _ in 0..5_000 {
                sim.advance(1);
                let now: u32 = troughs.iter().map(|s| hits(&sim, s)).sum();
                if now > before && sim.ball(h).is_none() {
                    ok = true;
                    break;
                }
            }
            assert!(
                ok,
                "drain from dx {dx} mm: a trough switch closed and the ball left"
            );
            assert!(sim.counters().balls_drained > drained, "drain counted");
            assert_healthy(&sim, "drain");
        }
    });
}

/// A lower flipper button press moves that flipper in the published
/// snapshot (and only that lower flipper).
#[test]
fn c4_lower_flipper_press_moves_the_snapshot() {
    big_stack(|| {
        let mut sim = game_seeded(10);
        for (id, input, other) in [
            ("lower_left", CabinetInput::LeftFlipper, "lower_right"),
            ("lower_right", CabinetInput::RightFlipper, "lower_left"),
        ] {
            let fid = sim.ids().flipper(id).expect("flipper id");
            let oid = sim.ids().flipper(other).expect("flipper id");
            let angle = |s: &Simulation, f: pin_core::FlipperId| {
                let snap = s.snapshot();
                snap.flippers[..usize::from(snap.flipper_count)]
                    .iter()
                    .find(|x| x.id == f)
                    .map(|x| x.angle_rad)
                    .expect("flipper in the snapshot")
            };
            sim.advance(300);
            let (a0, o0) = (angle(&sim, fid), angle(&sim, oid));
            cabinet(&mut sim, input, true);
            sim.advance(60);
            let (a1, o1) = (angle(&sim, fid), angle(&sim, oid));
            cabinet(&mut sim, input, false);
            sim.advance(300);
            let a2 = angle(&sim, fid);
            println!("{id}: rest {a0:.3} rad, held {a1:.3} rad, released {a2:.3} rad");
            assert!(
                (a1 - a0).abs() > 0.3,
                "{id} swung in the snapshot ({a0} -> {a1})"
            );
            assert!((o1 - o0).abs() < 1e-3, "{other} stayed at rest");
            assert!((a2 - a0).abs() < 0.05, "{id} returned to rest");
        }
        assert_healthy(&sim, "flippers");
    });
}
