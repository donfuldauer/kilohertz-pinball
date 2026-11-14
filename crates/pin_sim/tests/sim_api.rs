//! Subphase 2.5 deliverable 2: the test API (spawn, inject, set switches,
//! advance, query ball, switch and score state) on the fixture table.

mod sim_support;

use pin_core::sim::SimStep;
use pin_core::snapshot::{BallState, FrameStateSnapshot, MAX_BALLS};
use pin_sim::{SimConfig, SimError, Simulation};
use sim_support::*;

fn load() -> Simulation {
    Simulation::load(&fixture(), SimConfig::default()).expect("fixture loads")
}

#[test]
fn load_runs_on_init_and_starts_at_tick_zero() {
    on_sim_thread(|| {
        let sim = load();
        assert_eq!(sim.tick(), 0);
        assert_eq!(sim.snapshot().tick, 0);
        // on_init: score 0 reported, one ball launched from the dispenser.
        assert_eq!(sim.score(1), Some(0));
        assert_eq!(sim.score(2), None);
        assert_eq!(sim.balls_in_play(), 1);
        assert_eq!(sim.snapshot().balls().len(), 1);
        let b = sim.snapshot().balls()[0];
        assert_eq!(b.position_m, [0.257, 0.95, 0.0135]);
        assert_eq!(b.velocity_m_s, [0.35, -0.4, 0.0]);
        assert_eq!(sim.snapshot().flippers().len(), 2);
        assert!(!sim.script_faulted());
        assert_eq!(sim.script_fault_count(), 0);
        assert!(sim.script_error().is_none());
        assert_eq!(sim.config(), SimConfig::default());
        assert_eq!(SimConfig::default().seed, 0x4B485A);
        // EOS switches start closed (bats at rest), without counting a hit.
        assert_eq!(sim.switch_closed("left_eos"), Some(true));
        assert_eq!(sim.switch_hits("left_eos"), Some(0));
        assert!(sim.switch_id("trough").is_some());
        assert_eq!(sim.ids().switch("start").map(|s| s.0), Some(6));
    });
}

#[test]
fn spawn_query_and_inject_ball_state() {
    on_sim_thread(|| {
        let mut sim = load();
        let h = sim
            .spawn_ball("ceramic_powerball", [0.30, 0.50, 0.0135], [0.0, 0.5, 0.0])
            .unwrap();
        let s = sim.ball(h).unwrap();
        assert_eq!(s.position_m, [0.30, 0.50, 0.0135]);
        assert_eq!(s.velocity_m_s, [0.0, 0.5, 0.0]);
        assert_eq!(s.layer, 0);
        assert_eq!(sim.balls_in_play(), 2);
        sim.advance(100);
        assert_eq!(sim.tick(), 100);
        let s = sim.ball(h).unwrap();
        // Rolled uphill and slowed by gravity along -y.
        assert!(s.position_m[1] > 0.50, "{s:?}");
        assert!(s.velocity_m_s[1] < 0.5, "{s:?}");
        // State injection: exact bits back.
        let target = BallState {
            position_m: [0.40, 0.80, 0.0135],
            velocity_m_s: [-0.25, 0.0, -0.0],
            angular_velocity_rad_s: [0.0, 0.0, 3.0],
            ..s
        };
        assert!(sim.set_ball_state(h, &target));
        assert_eq!(sim.ball(h), Some(target));
        let bad = BallState {
            layer: 9,
            ..target
        };
        assert!(!sim.set_ball_state(h, &bad));
        // Layered spawn.
        let up = sim
            .spawn_ball_on_layer("mini_steel", 2, [0.1, 0.9, 0.05], [0.0; 3])
            .unwrap();
        assert_eq!(sim.ball(up).unwrap().layer, 2);
        assert_ne!(up, h);
        assert!(sim.remove_ball(up));
        assert!(!sim.remove_ball(up));
        assert_eq!(sim.ball(up), None);
        let handles = sim.ball_handles();
        assert!(handles.contains(&h));
        assert_eq!(handles.len(), sim.balls_in_play());
        let _ = up.slot();
        // Remanence handle readable (no magnet near: 0).
        assert_eq!(sim.magnetisation(h), 0.0);
    });
}

#[test]
fn spawn_errors() {
    on_sim_thread(|| {
        let mut sim = load();
        let e = sim.spawn_ball("granite", [0.2; 3], [0.0; 3]).unwrap_err();
        assert_eq!(e, SimError::UnknownProfile("granite".into()));
        assert!(e.to_string().contains("granite"));
        assert!(matches!(
            sim.spawn_ball_on_layer("standard_steel", 8, [0.2; 3], [0.0; 3]),
            Err(SimError::InvalidBall(_))
        ));
        assert!(matches!(
            sim.spawn_ball("standard_steel", [f32::NAN, 0.0, 0.0], [0.0; 3]),
            Err(SimError::InvalidBall(_))
        ));
        while sim.balls_in_play() < MAX_BALLS {
            let x = 0.03 * sim.balls_in_play() as f32 + 0.03;
            sim.spawn_ball("standard_steel", [x, 0.6, 0.0135], [0.0; 3])
                .unwrap();
        }
        let e = sim
            .spawn_ball("standard_steel", [0.2, 0.3, 0.0135], [0.0; 3])
            .unwrap_err();
        assert_eq!(e, SimError::BallArenaFull);
        assert!(!e.to_string().is_empty());
    });
}

#[test]
fn set_switch_edges_reach_the_script_next_tick() {
    on_sim_thread(|| {
        let mut sim = load();
        assert_eq!(
            sim.set_switch("no_such_switch", true),
            Err(SimError::UnknownSwitch("no_such_switch".into()))
        );
        assert_eq!(sim.switch_closed("no_such_switch"), None);
        assert_eq!(sim.switch_hits("no_such_switch"), None);
        // rules.lua: a closed center_rollover edge scores 10..=19.
        sim.set_switch("center_rollover", true).unwrap();
        assert_eq!(sim.switch_closed("center_rollover"), Some(true));
        assert_eq!(sim.switch_hits("center_rollover"), Some(1));
        assert_eq!(sim.score(1), Some(0));
        sim.advance(1);
        let s = sim.score(1).unwrap();
        assert!((10..=19).contains(&s), "{s}");
        // Forced: physics does not reopen it.
        sim.advance(5);
        assert_eq!(sim.switch_closed("center_rollover"), Some(true));
        sim.set_switch("center_rollover", false).unwrap();
        sim.set_switch("center_rollover", true).unwrap();
        assert_eq!(sim.switch_hits("center_rollover"), Some(2));
        sim.release_switch("center_rollover").unwrap();
        assert_eq!(sim.switch_closed("center_rollover"), Some(false));
        assert!(sim.release_switch("nope").is_err());
        let bits = sim.switch_bits();
        assert_eq!(bits[0] & (1 << 14), 0);
    });
}

#[test]
fn forced_cabinet_switch_drives_the_flipper() {
    on_sim_thread(|| {
        let mut sim = load();
        sim.set_switch("left_flipper", true).unwrap();
        sim.advance(40);
        let (angle, _) = sim.flipper("lower_left").unwrap();
        assert!(angle > 0.8, "left bat raised: {angle}");
        assert_eq!(sim.flipper("lower_right").unwrap().0, 0.0);
        assert_eq!(sim.switch_closed("left_eos"), Some(false));
        assert_eq!(sim.flipper("missing"), None);
        // The flipper energise edge was a haptic impulse on tick 1.
        sim.release_switch("left_flipper").unwrap();
        sim.advance(300);
        assert!(sim.flipper("lower_left").unwrap().0.abs() < 1e-3);
        assert!(sim.flipper_set().len() == 2);
        assert_eq!(sim.mech_set().len(), 1);
        assert!(sim.solver().ball_count() <= 1);
    });
}

#[test]
fn sim_step_contract_and_snapshot() {
    on_sim_thread(|| {
        let mut sim = load();
        let mut out = FrameStateSnapshot::default();
        sim.step_1ms(1, &[], &mut out);
        assert_eq!(out.tick, 1);
        assert_eq!(sim.tick(), 1);
        assert_eq!(&out, sim.snapshot());
        let mut boxed: Box<dyn SimStep> = Box::new(sim);
        boxed.step_1ms(2, &[], &mut out);
        assert_eq!(out.tick, 2);
        assert_eq!(out.lamps.len(), 256);
        assert_eq!(out.gi, [1.0; 4]);
    });
}

#[test]
fn drained_ball_handle_goes_stale_and_the_script_relaunches() {
    on_sim_thread(|| {
        let mut sim = load();
        let first = sim.ball_handles()[0];
        // Drop a ball straight onto the trough switch.
        sim.set_ball_state(
            first,
            &BallState {
                position_m: [0.257, 0.05, 0.0135],
                velocity_m_s: [0.0, -0.2, 0.0],
                ..sim.ball(first).unwrap()
            },
        );
        sim.advance(1);
        assert_eq!(sim.ball(first), None);
        assert_eq!(sim.counters().balls_drained, 1);
        assert_eq!(sim.switch_hits("trough"), Some(1));
        sim.advance(1);
        // rules.lua relaunches on the trough edge: a new ball, a new handle.
        assert_eq!(sim.balls_in_play(), 1);
        let second = sim.ball_handles()[0];
        assert_eq!(second.slot(), first.slot());
        assert_ne!(second, first);
        assert_eq!(sim.score(1), Some(1));
        // A ball thrown off the table escapes.
        let h = sim
            .spawn_ball("standard_steel", [0.25, 0.5, 0.5], [0.0, 0.0, 50.0])
            .unwrap();
        sim.advance(20);
        assert_eq!(sim.ball(h), None);
        assert_eq!(sim.counters().balls_escaped, 1);
        assert_eq!(sim.counters().edges_dropped, 0);
        assert_eq!(sim.script_queue_overflows(), 0);
    });
}

#[test]
fn errors_display_their_cause() {
    let missing = std::env::temp_dir().join("pin_sim_2_5_no_such_bundle_dir");
    let e = Simulation::load(&missing, SimConfig::default())
        .err()
        .expect("missing bundle");
    assert!(matches!(e, SimError::Asset(_)));
    assert!(e.to_string().starts_with("bundle error"));
    let e: Box<dyn std::error::Error> = Box::new(SimError::Build("x".into()));
    assert!(e.to_string().contains('x'));
}
