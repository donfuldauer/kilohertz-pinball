//! Subphase 2.9 spec tests (Independent Tester): the fixture bundles behave
//! as planted, checked with the plain `Simulation` API of Subphase 2.5 (no
//! autoplay code). These pin down what the soak harness is expected to find
//! in each bundle, so a soak test failure points at the harness, not at the
//! fixture.

#[path = "fixtures/spec_2_9/support.rs"]
mod support;

use pin_sim::{BallHandle, Simulation};
use support::*;

/// Longest run of consecutive ticks any one ball spent below 1 cm/s, and
/// the tick at which that run ended.
#[derive(Default)]
struct StuckTracker {
    runs: Vec<(BallHandle, u64)>,
    max_run: u64,
    max_run_at: u64,
}

impl StuckTracker {
    fn observe(&mut self, sim: &Simulation) {
        let handles = sim.ball_handles();
        self.runs.retain(|(h, _)| handles.contains(h));
        for h in handles {
            let Some(b) = sim.ball(h) else { continue };
            let slow = speed(b.velocity_m_s) < STUCK_SPEED_M_S;
            match self.runs.iter_mut().find(|(k, _)| *k == h) {
                Some((_, n)) => *n = if slow { *n + 1 } else { 0 },
                None => self.runs.push((h, u64::from(slow))),
            }
        }
        for &(_, n) in &self.runs {
            if n > self.max_run {
                self.max_run = n;
                self.max_run_at = sim.tick();
            }
        }
    }
}

/// A crude, input-independent flipper pattern: both flippers flip for
/// 120 ms every `period` ticks (offset per flipper).
fn mash(sim: &mut Simulation, period: u64) {
    let t = sim.tick();
    for (name, phase) in [("left_flipper", 0), ("right_flipper", period / 2)] {
        let on = (t + phase) % period < 120;
        sim.set_switch(name, on).unwrap();
    }
}

/// Plays one clean-rules game from `start` until `games_done` grows by one
/// (or `cap` ticks), flipping with [`mash`] when `period` is non-zero.
/// Returns (ticks, tracker).
fn play_one(sim: &mut Simulation, period: u64, cap: u64) -> (u64, StuckTracker) {
    let done0 = sim.persist(KEY_GAMES_DONE);
    tap(sim, "start");
    let mut tr = StuckTracker::default();
    let t0 = sim.tick();
    while sim.persist(KEY_GAMES_DONE) == done0 && sim.tick() - t0 < cap {
        if period > 0 {
            mash(sim, period);
        }
        sim.advance(1);
        tr.observe(sim);
    }
    (sim.tick() - t0, tr)
}

#[test]
fn every_fixture_bundle_loads() {
    on_sim_thread(|| {
        for g in ["clean", "trap", "gap", "hidden"] {
            let _ = load(&geometry(g), 1);
        }
        for s in [
            "clean",
            "mode_error",
            "flood",
            "launch",
            "rest",
            "cradle",
            "fly",
            "trough_rest",
            "inert",
        ] {
            let b = variant("clean", s);
            let _ = load(&b, 1);
        }
        for (_, b) in defect_bundles() {
            let _ = load(&b, 1);
        }
    });
}

#[test]
fn clean_serve_passes_the_shot_in_order_and_hits_the_bumper() {
    on_sim_thread(|| {
        let mut sim = load(&clean(), 7);
        assert_eq!(sim.balls_in_play(), 0, "attract: no ball before start");
        tap(&mut sim, "start");
        let (mut top_at, mut mid_at) = (None, None);
        for _ in 0..3000 {
            sim.advance(1);
            if top_at.is_none() && sim.switch_hits("top_opto") == Some(1) {
                top_at = Some(sim.tick());
            }
            if mid_at.is_none() && sim.switch_hits("mid_opto") == Some(1) {
                mid_at = Some(sim.tick());
            }
        }
        let (top, mid) = (top_at.expect("top_opto hit"), mid_at.expect("mid_opto hit"));
        assert!(
            top < mid && mid - top <= 2000,
            "orbit_shot: top {top}, mid {mid}"
        );
        assert!(sim.switch_hits("bumper_1_ring").unwrap() >= 1);
        assert!(sim.score(0).unwrap() >= 120, "score {:?}", sim.score(0));
        let lamp = sim.ids().lamp("lamp_frenzy").unwrap();
        assert!(
            sim.snapshot().lamps[usize::from(lamp.0)] > 0.0,
            "mode frenzy lamp lit"
        );
    });
}

#[test]
fn clean_game_without_flipping_ends_with_three_drains_and_no_stuck_ball() {
    on_sim_thread(|| {
        let mut sim = load(&clean(), 3);
        let (ticks, tr) = play_one(&mut sim, 0, 600 * SECOND);
        eprintln!(
            "no-flip game: {ticks} ticks, longest rest {} ms",
            tr.max_run
        );
        assert_eq!(
            sim.persist(KEY_GAMES_DONE),
            1,
            "game over after {ticks} ticks"
        );
        assert_eq!(sim.persist(KEY_BALLS_SERVED), 3);
        let c = sim.counters();
        assert_eq!((c.balls_drained, c.balls_escaped), (3, 0));
        assert_eq!(sim.switch_hits("trough_1"), Some(3));
        assert!(
            tr.max_run < 5 * SECOND,
            "a ball rested {} ms (ending tick {})",
            tr.max_run,
            tr.max_run_at
        );
        assert!(sim.persist(KEY_BALL_TICKS) > 0);
    });
}

#[test]
fn clean_game_with_flipping_covers_every_switch_without_stuck_or_escape() {
    on_sim_thread(|| {
        let mut sim = load(&clean(), 5);
        let (ticks, tr) = play_one(&mut sim, 700, 900 * SECOND);
        eprintln!(
            "flipping game: {ticks} ticks, longest rest {} ms",
            tr.max_run
        );
        assert_eq!(
            sim.persist(KEY_GAMES_DONE),
            1,
            "game over after {ticks} ticks"
        );
        assert_eq!(sim.counters().balls_escaped, 0);
        for s in CLEAN_SWITCHES {
            assert!(sim.switch_hits(s).unwrap() > 0, "{s} never hit");
        }
        // Resting on a briefly held flipper is allowed; nothing near 10 s.
        assert!(tr.max_run < 5 * SECOND, "a ball rested {} ms", tr.max_run);
    });
}

#[test]
fn trap_pocket_holds_the_served_ball_at_rest() {
    on_sim_thread(|| {
        let mut sim = load(&geometry("trap"), 1);
        tap(&mut sim, "start");
        sim.advance(3 * SECOND);
        let h = sim.ball_handles()[0];
        for _ in 0..15 * SECOND {
            sim.advance(1);
            let b = sim.ball(h).expect("trapped ball stays");
            assert!(
                speed(b.velocity_m_s) < STUCK_SPEED_M_S,
                "moving at {}",
                sim.tick()
            );
            let p = b.position_m;
            let d = libm::hypotf(p[0] * 1e3 - POCKET_MM[0], p[1] * 1e3 - POCKET_MM[1]);
            assert!(d < 40.0, "ball at {p:?} is not in the pocket");
        }
        assert!(names_pocket("ball at (255.0, 894.0) mm"));
        assert!(names_pocket("x=0.2551 y=0.8935"));
        assert!(names_pocket("walls[5]"));
        assert!(!names_pocket("ball at (100.0, 200.0) mm"));
    });
}

#[test]
fn gap_lets_drained_balls_leave_the_table() {
    on_sim_thread(|| {
        let mut sim = load(&geometry("gap"), 1);
        let (_, _) = play_one(&mut sim, 0, 120 * SECOND);
        let c = sim.counters();
        assert!(c.balls_escaped >= 1, "{c:?}");
        assert_eq!(c.balls_drained, 0);
    });
}

#[test]
fn hidden_switch_is_unreachable_under_flipping() {
    on_sim_thread(|| {
        let mut sim = load(&geometry("hidden"), 9);
        for (i, period) in [500, 700, 900].into_iter().enumerate() {
            let (ticks, _) = play_one(&mut sim, period, 600 * SECOND);
            sim.advance(10);
            assert_eq!(
                sim.persist(KEY_GAMES_DONE),
                i as i64 + 1,
                "game {i} unfinished after {ticks} ticks"
            );
            eprintln!("hidden: game {i} took {ticks} ticks");
        }
        assert_eq!(sim.switch_hits("hidden_opto"), Some(0));
        assert!(sim.switch_hits("top_opto").unwrap() > 0);
        assert!(sim.switch_hits("lower_left_eos").unwrap() > 0);
    });
}

/// The run of a single ball from a detector script: (tick of the last
/// motion at or above 1 cm/s, still present at the end).
fn settle(bundle: &std::path::Path, ticks: u64) -> (Simulation, u64) {
    let mut sim = load(bundle, 1);
    let mut last_moving = 0;
    let mut seen = false;
    for _ in 0..ticks {
        sim.advance(1);
        if let Some(&h) = sim.ball_handles().first() {
            seen = true;
            let b = sim.ball(h).unwrap();
            if speed(b.velocity_m_s) >= STUCK_SPEED_M_S {
                last_moving = sim.tick();
            }
        }
    }
    assert!(seen, "no ball was spawned");
    (sim, last_moving)
}

#[test]
fn rest_ball_settles_in_the_pocket_within_800_ms() {
    on_sim_thread(|| {
        let b = variant("trap", "rest");
        let (sim, last_moving) = settle(&b, 12 * SECOND);
        eprintln!("rest ball: last motion at tick {last_moving}");
        assert!(last_moving <= 800, "last motion at tick {last_moving}");
        assert_eq!(sim.balls_in_play(), 1);
        let p = sim.ball(sim.ball_handles()[0]).unwrap().position_m;
        let d = libm::hypotf(p[0] * 1e3 - POCKET_MM[0], p[1] * 1e3 - POCKET_MM[1]);
        assert!(d < 10.0, "rest ball at {p:?}");
    });
}

#[test]
fn cradle_ball_settles_on_the_held_flipper() {
    on_sim_thread(|| {
        let b = variant("clean", "cradle");
        let (sim, last_moving) = settle(&b, 12 * SECOND);
        eprintln!("cradle ball: last motion at tick {last_moving}");
        assert!(last_moving <= 2500, "last motion at tick {last_moving}");
        assert_eq!(sim.balls_in_play(), 1);
        let (angle, _) = sim.flipper("lower_left").unwrap();
        assert!(angle.abs() > 0.7, "flipper held up: angle {angle}");
        let p = sim.ball(sim.ball_handles()[0]).unwrap().position_m;
        assert!(
            p[1] > 0.14 && p[1] < 0.24 && p[0] > 0.13 && p[0] < 0.25,
            "ball on the flipper: {p:?}"
        );
    });
}

#[test]
fn fly_ball_escapes_over_the_wall() {
    on_sim_thread(|| {
        let mut sim = load(&variant("clean", "fly"), 1);
        sim.advance(SECOND);
        let c = sim.counters();
        assert_eq!((c.balls_escaped, c.balls_drained), (1, 0), "{c:?}");
    });
}

#[test]
fn trough_rest_ball_drains_at_once() {
    on_sim_thread(|| {
        let mut sim = load(&variant("clean", "trough_rest"), 1);
        sim.advance(5);
        let c = sim.counters();
        assert_eq!((c.balls_drained, c.balls_escaped), (1, 0), "{c:?}");
    });
}

#[test]
fn flood_script_overflows_the_command_queue() {
    on_sim_thread(|| {
        let mut sim = load(&variant("clean", "flood"), 1);
        tap(&mut sim, "start");
        sim.advance(SECOND);
        assert!(sim.script_queue_overflows() > 0);
        assert!(!sim.script_faulted());
    });
}

#[test]
fn mode_error_script_faults_when_the_mode_starts() {
    on_sim_thread(|| {
        let mut sim = load(&variant("clean", "mode_error"), 1);
        sim.advance(SECOND);
        assert!(!sim.script_faulted(), "no fault before the mode");
        tap(&mut sim, "start");
        sim.advance(3 * SECOND);
        assert!(sim.switch_hits("bumper_1_ring").unwrap() >= 1);
        assert!(sim.script_faulted());
        let e = sim.script_error().expect("error recorded").to_string();
        assert!(e.contains("planted defect"), "{e}");
    });
}

#[test]
fn launch_variant_serves_only_on_launch() {
    on_sim_thread(|| {
        let mut sim = load(&variant("clean", "launch"), 1);
        tap(&mut sim, "start");
        sim.advance(3 * SECOND);
        assert_eq!(sim.balls_in_play(), 0, "start alone serves nothing");
        tap(&mut sim, "launch");
        sim.advance(5);
        assert_eq!(sim.balls_in_play(), 1);
    });
}

#[test]
fn bad_shot_bundle_declares_the_impossible_shot() {
    let b = bad_shot_bundle();
    let toml = std::fs::read_to_string(b.join("table.toml")).unwrap();
    assert!(toml.contains("id = \"impossible_shot\""));
    assert!(toml.contains("id = \"orbit_shot\""));
}
