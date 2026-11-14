//! Subphase 2.5 spec tests (Independent Tester): the fixed tick order of
//! PLAN §2.2 as seen from outside.
//!
//! 1. drain inputs, update cabinet switches
//! 2. deliver last tick's edges, then on_tick
//! 3. apply the command queue
//! 4. physics
//! 5. collect edges
//! 6. publish the snapshot
//!
//! `rules.lua` reports, as host player 1, the number of completed
//! `on_tick` calls at the moment the last `lane_opto` closed edge was
//! delivered, and as host player 2 the same for `action` edges.

#[path = "fixtures/spec_2_5/support.rs"]
mod support;

use pin_core::input::CabinetInput;
use pin_core::snapshot::FrameStateSnapshot;
use support::*;

/// A physics edge collected in tick K (step 5) is delivered to
/// `on_switch_edge` in tick K + 1 (step 2), before that tick's `on_tick`, and
/// the score the script queues there is applied in the same tick (step 3).
#[test]
fn physics_edge_of_tick_k_is_delivered_in_tick_k_plus_1_before_on_tick() {
    on_sim_thread(|| {
        for (y0, vy) in [
            (0.90f32, -0.4f32),
            (0.93, -1.0),
            (0.885, -0.05),
            (0.95, -2.5),
        ] {
            let mut sim = load();
            // lane_opto: centre (257, 850) mm, radius 20 mm.
            sim.spawn_ball("standard_steel", [0.257, y0, R], [0.0, vy, 0.0])
                .unwrap();
            let mut k = None;
            for t in 1..=1000u64 {
                sim.advance(1);
                assert_eq!(sim.tick(), t);
                if sim.switch_closed("lane_opto") == Some(true) {
                    k = Some(t);
                    break;
                }
                assert_eq!(sim.score(P_LANE_AT), Some(-1), "tick {t}: no edge yet");
            }
            let k = k.expect("the ball never reached lane_opto");
            assert!(k >= 2, "the ball starts outside the zone");
            // End of tick K: the edge was collected after the script ran.
            assert_eq!(sim.switch_hits("lane_opto"), Some(1));
            assert_eq!(
                sim.score(P_LANE_AT),
                Some(-1),
                "the script must not see an edge in the tick that produced it (K = {k})"
            );
            assert_eq!(sim.score(P_SCORE), Some(0));
            // Tick K + 1: delivered before on_tick (K on_tick calls so far),
            // and the queued scores were applied in this same tick.
            sim.advance(1);
            assert_eq!(
                sim.score(P_LANE_AT),
                Some(k as i64),
                "edge of tick {k} must be delivered in tick {} before on_tick",
                k + 1
            );
            assert_eq!(sim.score(P_SCORE), Some(10));
        }
    });
}

/// A switch forced between ticks T and T + 1 is delivered at tick T + 1.
#[test]
fn forced_switch_edge_is_delivered_at_the_next_tick() {
    on_sim_thread(|| {
        for t0 in [0u64, 1, 7, 250] {
            let mut sim = load();
            sim.advance(t0);
            sim.set_switch("lane_opto", true).unwrap();
            if t0 > 0 {
                assert_eq!(
                    sim.score(P_LANE_AT),
                    Some(-1),
                    "not delivered before a tick runs"
                );
            }
            sim.advance(1);
            assert_eq!(
                sim.score(P_LANE_AT),
                Some(t0 as i64),
                "forced at tick {t0}: delivered in tick {} before on_tick",
                t0 + 1
            );
            assert_eq!(sim.score(P_SCORE), Some(10));
        }
        // Opening edges are delivered too (`action` reports both edges).
        let mut sim = load();
        sim.advance(20);
        sim.set_switch("action", true).unwrap();
        sim.advance(5);
        assert_eq!(sim.score(P_ACTION_AT), Some(20));
        assert_eq!(
            sim.switch_closed("action"),
            Some(true),
            "a forced cabinet switch stays"
        );
        sim.set_switch("action", false).unwrap();
        sim.advance(1);
        assert_eq!(sim.score(P_ACTION_AT), Some(25));
        assert_eq!(sim.switch_hits("action"), Some(1));
    });
}

/// An input event passed to tick N updates the cabinet switch in tick N
/// (step 1). Its edge reaches the script in tick N (if the implementation
/// treats step-1 edges as already collected) or tick N + 1 (edges "collected
/// last tick"); never earlier, never later, and always before on_tick.
#[test]
fn input_event_updates_the_cabinet_switch_in_its_tick() {
    on_sim_thread(|| {
        let mut sim = load();
        let mut out = Box::new(FrameStateSnapshot::default());
        for _ in 0..30 {
            step(&mut sim, &[], &mut out);
        }
        assert_eq!(sim.switch_closed("action"), Some(false));
        // Tick 31 carries the press.
        step(
            &mut sim,
            &[button(31, CabinetInput::Action, true)],
            &mut out,
        );
        assert_eq!(sim.tick(), 31);
        assert_eq!(sim.switch_closed("action"), Some(true));
        assert_eq!(sim.switch_hits("action"), Some(1));
        step(&mut sim, &[], &mut out);
        let at = sim.score(P_ACTION_AT).unwrap();
        assert!(
            at == 30 || at == 31,
            "press in tick 31 must be delivered in tick 31 or 32, before on_tick; \
             the script saw it after {at} on_tick calls"
        );
        // Release in tick 40.
        for _ in 33..40 {
            step(&mut sim, &[], &mut out);
        }
        step(
            &mut sim,
            &[button(40, CabinetInput::Action, false)],
            &mut out,
        );
        assert_eq!(sim.switch_closed("action"), Some(false));
        step(&mut sim, &[], &mut out);
        let at2 = sim.score(P_ACTION_AT).unwrap();
        assert_eq!(at2 - at, 9, "press and release keep their 9-tick distance");
        assert_eq!(sim.switch_hits("action"), Some(1));
    });
}

/// Several input events in one tick: the cabinet state of a tick is the
/// state after its last event (pin_input yields one debounced frame per
/// tick, applied in timestamp / arrival order), and a closed edge is a
/// tick-to-tick transition.
#[test]
fn input_events_within_a_tick_resolve_to_the_last_one() {
    on_sim_thread(|| {
        let mut sim = load();
        let mut out = Box::new(FrameStateSnapshot::default());
        step(&mut sim, &[], &mut out);
        // press, release, press within tick 2: ends closed.
        step(
            &mut sim,
            &[
                button(2, CabinetInput::Action, true),
                button(2, CabinetInput::Action, false),
                button(2, CabinetInput::Action, true),
            ],
            &mut out,
        );
        assert_eq!(sim.switch_closed("action"), Some(true));
        assert_eq!(sim.switch_hits("action"), Some(1));
        // release, press: still closed, no new edge between ticks.
        step(
            &mut sim,
            &[
                button(3, CabinetInput::Action, false),
                button(3, CabinetInput::Action, true),
            ],
            &mut out,
        );
        assert_eq!(sim.switch_closed("action"), Some(true));
        assert_eq!(sim.switch_hits("action"), Some(1));
        // A repeated press is not an edge.
        step(&mut sim, &[button(4, CabinetInput::Action, true)], &mut out);
        assert_eq!(sim.switch_hits("action"), Some(1));
        // press, release: ends open.
        step(
            &mut sim,
            &[
                button(5, CabinetInput::Action, true),
                button(5, CabinetInput::Action, false),
            ],
            &mut out,
        );
        assert_eq!(sim.switch_closed("action"), Some(false));
        step(&mut sim, &[button(6, CabinetInput::Action, true)], &mut out);
        assert_eq!(sim.switch_closed("action"), Some(true));
        assert_eq!(sim.switch_hits("action"), Some(2));
        // Two different buttons in one tick both apply.
        step(
            &mut sim,
            &[
                button(7, CabinetInput::Launch, true),
                button(7, CabinetInput::Start, true),
            ],
            &mut out,
        );
        assert_eq!(sim.switch_closed("launch"), Some(true));
        assert_eq!(sim.switch_closed("start"), Some(true));
        // `start` spawns a ball in tick 7 (cabinet edge delivered in its own
        // tick) or tick 8 (delivered one tick later); exactly one either way.
        assert!(sim.balls_in_play() <= 1);
        step(&mut sim, &[], &mut out);
        assert_eq!(sim.balls_in_play(), 1, "start spawns by tick 8");
    });
}

/// Step 3 before step 4: a ball the script spawns in tick N exists at the end
/// of tick N and has already been stepped once by that tick's physics.
#[test]
fn command_queued_in_a_tick_reaches_physics_in_the_same_tick() {
    on_sim_thread(|| {
        let mut sim = load();
        sim.advance(100);
        assert_eq!(sim.balls_in_play(), 0);
        // `start` closed -> on_switch spawns from dispenser `feed`.
        sim.set_switch("start", true).unwrap();
        assert_eq!(
            sim.balls_in_play(),
            0,
            "nothing happens until the tick runs"
        );
        sim.advance(1); // tick 101: edge delivered, spawn queued, applied, stepped
        assert_eq!(
            sim.balls_in_play(),
            1,
            "a spawn queued in on_switch_edge is applied in the same tick"
        );
        let snap = sim.snapshot();
        assert_eq!(snap.tick, 101);
        assert_eq!(
            snap.ball_count, 1,
            "... and published in that tick's snapshot"
        );
        let b = snap.balls()[0];
        assert_eq!(b.layer, 0);
        // Stepped once: exit position + 1 ms of exit velocity (gravity adds
        // < 1 um in one tick), not the bare exit position.
        let mut err = 0.0f32;
        let mut moved = 0.0f32;
        for a in 0..3 {
            let want = FEED_POS[a] + FEED_VEL[a] * 0.001;
            err = err.max((b.position_m[a] - want).abs());
            moved = moved.max((b.position_m[a] - FEED_POS[a]).abs());
        }
        assert!(
            err < 5.0e-5,
            "after the spawn tick the ball must be one physics step from the exit: {:?}",
            b.position_m
        );
        assert!(
            moved > 4.0e-4,
            "the spawned ball was not stepped in its spawn tick: {:?}",
            b.position_m
        );
        assert!((b.velocity_m_s[1] - FEED_VEL[1]).abs() < 0.01);
        // The dispenser's first content is standard_steel.
        let mut probe = load();
        let h = probe
            .spawn_ball("standard_steel", [0.1, 0.5, R], [0.0; 3])
            .unwrap();
        assert_eq!(b.profile, probe.ball(h).unwrap().profile);

        // The script's RNG draw on `start` was reported in the same tick.
        let r = sim.score(P_RNG).unwrap();
        assert!(
            (1..=1_000_000).contains(&r),
            "pinball.random(1000000) = {r}"
        );
    });
}

/// A coil pulsed by the script (bumper, on the ring edge) acts on the ball
/// through the same tick pipeline: compare the ring hit with the same hit on
/// a table whose script never fires the coil.
#[test]
fn scripted_coil_pulse_changes_the_physics() {
    on_sim_thread(|| {
        let silent = bundle_with_script("nocoil", "function on_tick() end\n");
        let run = |sim: &mut pin_sim::Simulation| {
            let h = sim
                .spawn_ball("standard_steel", [0.259, 0.74, R], [0.0, -1.0, 0.0])
                .unwrap();
            let mut hit = None;
            let mut vmax = 0.0f32;
            for t in 1..=300u64 {
                sim.advance(1);
                if hit.is_none() && sim.switch_hits("bumper_1_ring").unwrap() > 0 {
                    hit = Some(t);
                }
                if let (Some(ht), Some(b)) = (hit, sim.ball(h))
                    && t <= ht + 40
                {
                    let v = b.velocity_m_s;
                    vmax = vmax.max((v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt());
                }
            }
            (hit.expect("no ring hit"), vmax)
        };
        let (hit_a, v_kicked) = run(&mut load());
        let (hit_b, v_passive) = run(&mut load_at(&silent, DEFAULT_SEED));
        assert_eq!(hit_a, hit_b, "identical approach until the ring closes");
        assert!(
            v_kicked > v_passive + 0.5,
            "pulsed coil {v_kicked} m/s vs passive rebound {v_passive} m/s"
        );
    });
}

/// The seed reaches the script's RNG: same seed, same draws; other seed,
/// other draws.
#[test]
fn script_rng_is_the_seeded_sim_rng() {
    on_sim_thread(|| {
        let draw = |seed: u64| {
            let mut sim = load_at(&bundle(), seed);
            sim.advance(3);
            let mut sum = Vec::new();
            for _ in 0..4 {
                sim.set_switch("start", true).unwrap();
                sim.advance(2);
                sim.set_switch("start", false).unwrap();
                sim.advance(150); // let the ball clear the dispenser exit
                sum.push(sim.score(P_RNG).unwrap());
            }
            sum
        };
        let a = draw(1);
        assert_eq!(a, draw(1));
        assert_ne!(a, draw(2));
        assert_ne!(a, draw(DEFAULT_SEED));
        assert!(
            a.windows(2).all(|w| w[1] > w[0]),
            "four draws accumulate: {a:?}"
        );
    });
}
