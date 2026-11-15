//! Work item vuk-remove-ball (DECISIONS.md "vuk-remove-ball — hold release
//! on ball removal"): removing a ball that a VUK holds, through
//! `Simulation::remove_ball` or the tick's own drain, releases the hold at
//! once. A ball spawned into the freed solver slot afterwards is a fresh,
//! free ball (it is captured only through the normal capture path), and the
//! VUK's switch opens on the next tick with the normal edge.

mod sim_support;

use pin_sim::{BallHandle, SimConfig, Simulation};
use sim_support::*;

/// A VUK on the open playfield (`vuk`, mech 1), a second VUK sitting inside
/// the trough zone (`trough_vuk`, mech 2), their coils and switches.
const EXTRA: &str = r#"
[[coils]]
id = "vuk_coil"
number = 4
kind = "vuk"

[[coils]]
id = "trough_vuk_coil"
number = 5
kind = "vuk"

[[switches]]
id = "vuk_sw"
number = 20
kind = "leaf"
layer = 0

[[switches]]
id = "trough_vuk_sw"
number = 21
kind = "leaf"
layer = 0

[[mechs]]
id = "vuk"
kind = "vuk"
coil = "vuk_coil"
switch = "vuk_sw"
position = [400.0, 400.0, 0.0]
angle_deg = 270.0
layer = 0
exit_layer = 0
exit_offset = [0.0, -60.0, 13.5]
eject_speed_m_s = 0.2
kick_speed_m_s = 0.1

[[mechs]]
id = "trough_vuk"
kind = "vuk"
coil = "trough_vuk_coil"
switch = "trough_vuk_sw"
position = [257.0, 40.0, 0.0]
angle_deg = 90.0
layer = 0
exit_layer = 0
exit_offset = [0.0, 60.0, 13.5]
eject_speed_m_s = 0.2
kick_speed_m_s = 0.1
"#;

/// No ball launches. `action` holds both VUK coils while closed; `launch`
/// pulses both. Score = 1000 × VUK-switch closing edges + opening edges
/// (both VUKs), as the script saw them.
const RULES: &str = r#"
local closes = 0
local opens = 0

function on_init()
  pinball.set_score(1, 0)
end

function on_switch(id, closed)
  if id == pinball.sw.action then
    pinball.hold_coil(pinball.coil.vuk_coil, closed)
    pinball.hold_coil(pinball.coil.trough_vuk_coil, closed)
  elseif id == pinball.sw.launch and closed then
    pinball.pulse_coil(pinball.coil.vuk_coil, 20)
    pinball.pulse_coil(pinball.coil.trough_vuk_coil, 20)
  elseif id == pinball.sw.vuk_sw or id == pinball.sw.trough_vuk_sw then
    if closed then closes = closes + 1 else opens = opens + 1 end
    pinball.set_score(1, closes * 1000 + opens)
  end
end

function on_tick()
end
"#;

const VUK: usize = 1;
const TROUGH_VUK: usize = 2;
const R: f32 = 0.0135;
/// The open-playfield VUK's hold point (centre of a held steel ball).
const VUK_HOLD: [f32; 3] = [0.400, 0.400, R];
/// The trough VUK's hold point (inside the trough zone).
const TROUGH_HOLD: [f32; 3] = [0.257, 0.040, R];
/// A free spot on the open playfield, far from both VUKs.
const AWAY: [f32; 3] = [0.200, 0.500, R];

fn bundle(tag: &str) -> ScratchDir {
    let table = fixture_table_toml() + EXTRA;
    fixture_variant(
        tag,
        &[
            ("table.toml", Some(table.as_bytes())),
            ("rules.lua", Some(RULES.as_bytes())),
        ],
    )
}

fn with_sim(tag: &str, f: impl FnOnce(&mut Simulation) + Send + 'static) {
    let dir = bundle(tag);
    let path = dir.to_path_buf();
    on_sim_thread(move || {
        let mut sim = Simulation::load(&path, SimConfig::default()).expect("load");
        sim.advance(1);
        assert_eq!(sim.balls_in_play(), 0, "the rules launch no ball");
        f(&mut sim);
    });
    drop(dir);
}

fn dist(a: [f32; 3], b: [f32; 3]) -> f32 {
    let d = [a[0] - b[0], a[1] - b[1], a[2] - b[2]];
    (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
}

fn vuk_slot(sim: &Simulation, mech: usize) -> Option<u8> {
    sim.mech_set().vuk_ball(mech).map(|h| h.0)
}

/// Spawns a ball at rest on the open VUK's hold point and checks that the
/// VUK captured it after one tick.
fn capture(sim: &mut Simulation) -> BallHandle {
    let h = sim
        .spawn_ball("standard_steel", VUK_HOLD, [0.0; 3])
        .expect("spawn");
    sim.advance(1);
    assert_eq!(vuk_slot(sim, VUK), Some(h.slot()), "captured");
    assert_eq!(sim.switch_closed("vuk_sw"), Some(true));
    h
}

/// Asserts that ball `h` is free and rolling down the pitched playfield:
/// not held by either VUK, away from both hold points, and moving.
fn assert_free_and_rolling(sim: &mut Simulation, h: BallHandle, ticks: u64) {
    let start = sim.ball(h).expect("new ball on the table").position_m;
    for t in 0..ticks {
        sim.advance(1);
        let b = sim.ball(h).expect("new ball still on the table");
        assert!(
            dist(b.position_m, VUK_HOLD) > 0.05 && dist(b.position_m, TROUGH_HOLD) > 0.05,
            "tick {t}: the new ball was pulled into a VUK: {:?}",
            b.position_m
        );
        assert_eq!(vuk_slot(sim, VUK), None, "tick {t}: the VUK holds no ball");
        assert_eq!(vuk_slot(sim, TROUGH_VUK), None, "tick {t}: trough VUK");
    }
    let end = sim.ball(h).expect("ball").position_m;
    assert!(
        end[1] < start[1] - 1.0e-4,
        "the new ball rolls down the table under gravity: {start:?} -> {end:?}"
    );
    let v = sim.ball(h).expect("ball").velocity_m_s;
    assert!(v[1] < 0.0, "rolling down the pitch: {v:?}");
}

/// Requirement 1: `remove_ball` on the held ball, then a spawn into the same
/// slot away from the VUK. The new ball must not be pinned in the VUK.
#[test]
fn removed_held_ball_releases_the_vuk_for_a_respawn_away() {
    with_sim("vuk_rm_away", |sim| {
        let h = capture(sim);
        sim.advance(5);
        assert_eq!(vuk_slot(sim, VUK), Some(h.slot()), "still held");
        assert!(sim.remove_ball(h));
        let n = sim
            .spawn_ball("standard_steel", AWAY, [0.0; 3])
            .expect("respawn");
        assert_eq!(n.slot(), h.slot(), "the solver reuses the slot");
        assert_ne!(n, h);
        assert_free_and_rolling(sim, n, 20);
        assert_eq!(sim.switch_closed("vuk_sw"), Some(false));
    });
}

/// Requirement 1 (in-zone case): a ball spawned into the same slot inside
/// the hold zone is captured, but only by the normal capture path: with the
/// hold coil on, a stale hold would eject it in the next tick's pre-step,
/// whereas a normal capture happens after the ball step (so after the tick
/// it is held at the hold point).
#[test]
fn respawn_inside_the_zone_is_captured_by_the_normal_path() {
    with_sim("vuk_rm_zone", |sim| {
        let h = capture(sim);
        sim.set_switch("action", true).unwrap(); // hold coil on from the next tick
        assert!(sim.remove_ball(h));
        let n = sim
            .spawn_ball("standard_steel", VUK_HOLD, [0.0; 3])
            .expect("respawn");
        assert_eq!(n.slot(), h.slot());
        sim.advance(1);
        assert_eq!(vuk_slot(sim, VUK), Some(n.slot()), "captured anew");
        let p = sim.ball(n).unwrap().position_m;
        assert!(dist(p, VUK_HOLD) < 1.0e-5, "at the hold point: {p:?}");
        // The coil is on, so the next tick ejects the fresh capture.
        sim.advance(1);
        assert_eq!(vuk_slot(sim, VUK), None, "ejected");
        let p = sim.ball(n).unwrap().position_m;
        assert!(dist(p, VUK_HOLD) > 0.03, "moved to the exit: {p:?}");
    });
}

/// Requirement 3: the VUK whose ball is removed opens its switch on the next
/// tick with the normal edge (one closing and one opening edge reach the
/// script), also when a ball is spawned into the freed slot at once.
#[test]
fn switch_opens_on_the_next_tick_with_an_edge() {
    with_sim("vuk_rm_switch", |sim| {
        let h = capture(sim);
        sim.advance(2);
        assert_eq!(sim.score(0), Some(1000), "one closing edge, no opening");
        assert_eq!(sim.switch_hits("vuk_sw"), Some(1));
        assert!(sim.remove_ball(h));
        let n = sim
            .spawn_ball("standard_steel", AWAY, [0.0; 3])
            .expect("respawn");
        assert_eq!(n.slot(), h.slot());
        sim.advance(1);
        assert_eq!(sim.switch_closed("vuk_sw"), Some(false), "opens next tick");
        sim.advance(1);
        assert_eq!(
            sim.score(0),
            Some(1001),
            "the opening edge reached the script"
        );
        sim.advance(10);
        assert_eq!(sim.switch_closed("vuk_sw"), Some(false));
        assert_eq!(sim.switch_hits("vuk_sw"), Some(1), "no new closing");
        assert_eq!(sim.score(0), Some(1001));
    });
}

/// Requirement 6: a ball captured by the trough VUK drains in the same tick
/// (the drain runs after the mechs' post-step), through the tick's own
/// drain path. The hold goes with it, and a ball spawned into the freed
/// slot is free.
#[test]
fn drained_held_ball_releases_the_vuk() {
    with_sim("vuk_drain", |sim| {
        let drained = sim.counters().balls_drained;
        let h = sim
            .spawn_ball("standard_steel", TROUGH_HOLD, [0.0; 3])
            .expect("spawn");
        sim.advance(1);
        assert!(sim.ball(h).is_none(), "drained");
        assert_eq!(sim.counters().balls_drained, drained + 1);
        let held_after_drain = vuk_slot(sim, TROUGH_VUK);
        let n = sim
            .spawn_ball("standard_steel", AWAY, [0.0; 3])
            .expect("respawn");
        assert_eq!(n.slot(), h.slot());
        assert_free_and_rolling(sim, n, 20);
        assert_eq!(sim.counters().balls_drained, drained + 1, "no second drain");
        assert_eq!(sim.switch_closed("trough_vuk_sw"), Some(false));
        assert_eq!(
            held_after_drain, None,
            "the hold went with the drained ball"
        );
    });
}

/// Requirement 7, drain path: the hold coil is on in the tick in which the
/// held ball is captured and drained. No panic, no hold left, and the
/// respawned ball is neither ejected nor pinned.
#[test]
fn drain_with_hold_coil_on_leaves_no_hold() {
    with_sim("vuk_drain_coil", |sim| {
        sim.set_switch("action", true).unwrap();
        sim.advance(2);
        let h = sim
            .spawn_ball("standard_steel", TROUGH_HOLD, [0.0; 3])
            .expect("spawn");
        sim.advance(1);
        assert!(sim.ball(h).is_none(), "drained");
        let held_after_drain = vuk_slot(sim, TROUGH_VUK);
        let n = sim
            .spawn_ball("standard_steel", AWAY, [0.0; 3])
            .expect("respawn");
        assert_eq!(n.slot(), h.slot());
        assert_free_and_rolling(sim, n, 20);
        assert_eq!(
            held_after_drain, None,
            "the hold went with the drained ball"
        );
    });
}

/// Coil applied around the removal in the sweep below.
#[derive(Copy, Clone, Debug)]
enum Coil {
    Off,
    /// `launch` closed right before the removal: the pulse lands in the
    /// first tick after it (the tick in which a stale hold would eject).
    Pulse,
    /// `action` closed right before the removal: hold coil on.
    Hold,
}

/// Sweep (requirements 1, 3, 7): removal 0..12 ticks after the capture,
/// with no coil, an eject pulse or the hold coil landing in the first tick
/// after the removal, and a respawn 0..5 ticks after it, away from the VUK
/// or inside its zone. Away, the new ball is never held; inside, it is
/// held only after a normal capture (at the end of a tick, at the hold
/// point).
#[test]
fn sweep_removal_tick_coil_and_respawn_offset() {
    with_sim("vuk_rm_sweep", |sim| {
        for wait in 0..12u64 {
            for coil in [Coil::Off, Coil::Pulse, Coil::Hold] {
                for offset in 0..5u64 {
                    for inside in [false, true] {
                        let case = format!("wait {wait} coil {coil:?} offset {offset} in {inside}");
                        assert_eq!(sim.balls_in_play(), 0, "{case}: clean start");
                        assert_eq!(vuk_slot(sim, VUK), None, "{case}: clean start");
                        let h = capture(sim);
                        sim.advance(wait);
                        if vuk_slot(sim, VUK).is_none() {
                            // Not reachable without a coil; keep the sweep honest.
                            panic!("{case}: the ball left the VUK on its own");
                        }
                        match coil {
                            Coil::Off => {}
                            Coil::Pulse => sim.set_switch("launch", true).unwrap(),
                            Coil::Hold => sim.set_switch("action", true).unwrap(),
                        }
                        assert!(sim.remove_ball(h), "{case}");
                        assert_eq!(vuk_slot(sim, VUK), None, "{case}: released");
                        sim.advance(offset);
                        assert_eq!(vuk_slot(sim, VUK), None, "{case}: empty VUK stays empty");
                        assert_eq!(sim.switch_closed("vuk_sw"), Some(offset == 0), "{case}");
                        let at = if inside { VUK_HOLD } else { AWAY };
                        let n = sim
                            .spawn_ball("standard_steel", at, [0.0; 3])
                            .expect("respawn");
                        assert_eq!(n.slot(), h.slot(), "{case}: same slot");
                        // The coil command reaches the mechs in the first tick
                        // after the removal; with offset 0 that is this tick.
                        sim.advance(1);
                        let b = sim.ball(n).expect("new ball on the table");
                        if inside {
                            // Normal capture at the end of the tick, at the
                            // hold point, whatever the coil.
                            assert_eq!(vuk_slot(sim, VUK), Some(n.slot()), "{case}: captured");
                            assert!(dist(b.position_m, VUK_HOLD) < 1.0e-5, "{case}: {b:?}");
                            assert_eq!(sim.switch_closed("vuk_sw"), Some(true), "{case}");
                        } else {
                            assert_eq!(vuk_slot(sim, VUK), None, "{case}: not held");
                            assert!(dist(b.position_m, VUK_HOLD) > 0.05, "{case}: {b:?}");
                            assert_eq!(sim.switch_closed("vuk_sw"), Some(false), "{case}");
                            sim.advance(3);
                            let p = sim.ball(n).expect("ball").position_m;
                            assert!(p[1] < AWAY[1] - 1.0e-6, "{case}: rolls: {p:?}");
                            assert_eq!(vuk_slot(sim, VUK), None, "{case}: not held");
                        }
                        // Clean up for the next case: coil off, ball gone.
                        sim.release_switch("launch").unwrap();
                        sim.release_switch("action").unwrap();
                        sim.set_switch("action", false).unwrap();
                        sim.advance(1);
                        sim.release_switch("action").unwrap();
                        assert!(sim.remove_ball(n), "{case}");
                        sim.advance(25);
                        assert_eq!(vuk_slot(sim, VUK), None, "{case}");
                        assert_eq!(sim.switch_closed("vuk_sw"), Some(false), "{case}");
                    }
                }
            }
        }
    });
}
