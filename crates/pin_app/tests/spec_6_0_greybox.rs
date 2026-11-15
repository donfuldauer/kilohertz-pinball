//! Subphase 6.0 spec tests (Independent Tester): the grey-box table
//! (`tables/greybox`, built with `tests/support::build_greybox_bundle`),
//! brief 6.0 items 4 and 5 and verification criterion "headless scripted
//! run exercises every mechanism and asserts switch counts; replay
//! deterministic".
//!
//! Everything is black-box: structure through `pin_assets::load_table`,
//! behaviour through the public `pin_sim::Simulation` API (state injection,
//! switch hit counts, scores) on simulated time.

mod support;

#[path = "fixtures/spec_6_0/common.rs"]
mod common;

use common::{STEEL, greybox_bundle, greybox_table, steel_radius_m};
use pin_app::orchestrator::load_validated;
use pin_assets::FlipperStageHint;
use pin_core::input::{CabinetInput, InputEvent};
use pin_core::schema::table::{MechKind, SplineKind, SwitchKind};
use pin_core::sim::TICK_NS;
use pin_sim::replay::Recorder;
use pin_sim::{BallHandle, SimConfig, Simulation};

/// Runs `f` on a thread with a generous stack (script backends recurse).
fn big_stack<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static) -> R {
    std::thread::Builder::new()
        .name("spec-6.0-sim".into())
        .stack_size(64 << 20)
        .spawn(f)
        .expect("spawn")
        .join()
        .unwrap_or_else(|e| std::panic::resume_unwind(e))
}

fn load() -> Simulation {
    load_validated(greybox_bundle(), SimConfig::default()).expect("grey-box loads validated")
}

fn assert_healthy(sim: &Simulation, what: &str) {
    assert!(
        !sim.script_faulted(),
        "{what}: script faulted: {:?}",
        sim.script_error()
    );
    assert_eq!(
        sim.script_queue_overflows(),
        0,
        "{what}: script queue overflow"
    );
    assert_eq!(sim.counters().balls_escaped, 0, "{what}: a ball escaped");
    for h in sim.ball_handles() {
        let b = sim.ball(h).expect("live handle");
        let finite = b.position_m.iter().all(|v| v.is_finite())
            && b.velocity_m_s.iter().all(|v| v.is_finite())
            && b.angular_velocity_rad_s.iter().all(|v| v.is_finite());
        assert!(finite, "{what}: non-finite ball {b:?}");
    }
}

/// Closes switch `name` for `hold` ticks, opens it and hands it back to
/// physics.
fn pulse(sim: &mut Simulation, name: &str, hold: u64) {
    sim.set_switch(name, true).expect("known switch");
    sim.advance(hold);
    sim.set_switch(name, false).expect("known switch");
    sim.advance(5);
    sim.release_switch(name).expect("known switch");
}

fn hits(sim: &Simulation, name: &str) -> u32 {
    sim.switch_hits(name).expect("known switch")
}

// ---------------------------------------------------------------------------
// Structure (brief item 4)
// ---------------------------------------------------------------------------

#[test]
fn greybox_flippers_two_lower_one_upper() {
    let t = greybox_table();
    assert_eq!(t.manifest.flippers.len(), 3, "exactly 3 flippers");
    assert_eq!(t.flipper_stages.len(), 3);
    let lower: Vec<usize> = (0..3)
        .filter(|&i| t.flipper_stages[i] == FlipperStageHint::Lower)
        .collect();
    let upper = 3 - lower.len();
    assert_eq!(lower.len(), 2, "two lower flippers");
    assert_eq!(upper, 1, "one upper flipper");
    // The lower pair is a left and a right flipper.
    let xs: Vec<f32> = lower
        .iter()
        .map(|&i| t.manifest.flippers[i].base_position[0])
        .collect();
    assert!(xs[0] != xs[1], "lower flippers at different x");
}

#[test]
fn greybox_slings_bumpers_vuk_newton() {
    let t = greybox_table();
    let count = |k: MechKind| t.manifest.mechs.iter().filter(|m| m.kind == k).count();
    assert_eq!(count(MechKind::Sling), 2, "2 slings");
    assert_eq!(count(MechKind::Bumper), 3, "3 bumpers");
    assert_eq!(count(MechKind::Vuk), 1, "one VUK");
    assert_eq!(count(MechKind::NewtonBall), 1, "one newton_ball mech");
    let vuk = t
        .manifest
        .mechs
        .iter()
        .find(|m| m.kind == MechKind::Vuk)
        .expect("VUK");
    assert_eq!(
        vuk.exit_layer,
        Some(1),
        "the VUK delivers to the upper deck"
    );
}

#[test]
fn greybox_orbit_and_wireform_ramp() {
    let t = greybox_table();
    assert!(
        t.manifest
            .splines
            .iter()
            .any(|s| s.kind == SplineKind::Wireform),
        "at least one wireform ramp"
    );
    // An orbit: a guide lane, switch or shot named as one (the tablegen
    // lane ids are generic, so any named element counts).
    let orbit = |s: &str| s.to_ascii_lowercase().contains("orbit");
    let named = t.manifest.splines.iter().any(|s| orbit(&s.id))
        || t.manifest.switches.iter().any(|s| orbit(&s.id))
        || t.manifest.shots.iter().any(|s| orbit(&s.id))
        || t.manifest.mechs.iter().any(|m| orbit(&m.id));
    assert!(named, "at least one orbit");
    assert!(
        t.manifest
            .splines
            .iter()
            .any(|s| s.kind == SplineKind::Guide),
        "the orbit has guide rails"
    );
}

#[test]
fn greybox_magnet_eddy_dispenser_trough() {
    let t = greybox_table();
    assert_eq!(t.manifest.magnets.len(), 1, "one magnet");
    assert!(
        t.manifest
            .switches
            .iter()
            .any(|s| s.kind == SwitchKind::Eddy),
        "an eddy sensor"
    );
    let powerball = t
        .manifest
        .dispensers
        .iter()
        .filter(|d| d.contents.iter().any(|p| p == "ceramic_powerball"))
        .count();
    assert_eq!(powerball, 1, "one dispenser holds ceramic_powerball");
    assert!(
        t.manifest
            .switches
            .iter()
            .any(|s| s.kind == SwitchKind::Trough),
        "trough switches"
    );
}

#[test]
fn greybox_shots_modes_cabinet_switches() {
    let t = greybox_table();
    assert!(t.manifest.shots.len() >= 2, "[[shots]] >= 2");
    assert!(!t.manifest.modes.is_empty(), "[[modes]] >= 1");
    let cab = t.manifest.cabinet_switches;
    let mut numbers: Vec<u16> = CabinetInput::ALL.iter().map(|&i| cab.number(i)).collect();
    numbers.sort_unstable();
    numbers.dedup();
    assert_eq!(numbers.len(), 8, "8 distinct cabinet switch numbers");
    big_stack(|| {
        let sim = load();
        for key in [
            "left_flipper",
            "right_flipper",
            "left_upper",
            "right_upper",
            "action",
            "start",
            "launch",
            "tilt",
        ] {
            assert!(sim.switch_id(key).is_some(), "cabinet switch {key}");
        }
    });
}

// ---------------------------------------------------------------------------
// Scripted run by state injection: every mechanism (verification criterion)
// ---------------------------------------------------------------------------

/// Every `[[mechs]]` entry: its switch is driven and counted; a switch-less
/// mech gets a ball spawned onto it. No ball is lost or non-finite.
#[test]
fn every_mech_is_exercised() {
    big_stack(|| {
        let t = greybox_table();
        let mut sim = load();
        sim.advance(200);
        let r = steel_radius_m();
        for m in &t.manifest.mechs {
            match &m.switch {
                Some(sw) => {
                    let before = hits(&sim, sw);
                    pulse(&mut sim, sw, 30);
                    sim.advance(200);
                    assert!(
                        hits(&sim, sw) > before,
                        "mech {} ({:?}): switch {sw} hit count did not increase",
                        m.id,
                        m.kind
                    );
                }
                None => {
                    let p = m
                        .position
                        .unwrap_or_else(|| panic!("switch-less mech {} has a position", m.id));
                    let layer = m.layer.unwrap_or(0);
                    let pos = [p[0] * 1e-3, p[1] * 1e-3, p[2] * 1e-3 + r];
                    sim.spawn_ball_on_layer(STEEL, layer, pos, [0.0; 3])
                        .unwrap_or_else(|e| panic!("spawn onto {}: {e}", m.id));
                    sim.advance(500);
                }
            }
            assert_healthy(&sim, &m.id);
        }
        sim.advance(2_000);
        assert_healthy(&sim, "after all mechs");
    });
}

/// Physically: a steel (conductive) ball on the eddy sensor closes it.
#[test]
fn eddy_sensor_sees_a_steel_ball() {
    big_stack(|| {
        let t = greybox_table();
        let eddy = t
            .switches
            .iter()
            .find(|s| s.kind == SwitchKind::Eddy)
            .expect("eddy switch");
        let p = eddy
            .position_m
            .unwrap_or_else(|| panic!("eddy switch {} has a position", eddy.name));
        let mut sim = load();
        sim.advance(100);
        let before = hits(&sim, &eddy.name);
        sim.spawn_ball_on_layer(
            STEEL,
            eddy.layer,
            [p[0], p[1], p[2] + steel_radius_m()],
            [0.0; 3],
        )
        .expect("spawn on the eddy sensor");
        sim.advance(3);
        assert!(
            hits(&sim, &eddy.name) > before,
            "steel ball over the eddy sensor"
        );
        assert_healthy(&sim, "eddy");
    });
}

/// Presses `start` and waits (≤ 10 s simulated) for a ball to be served.
fn start_game(sim: &mut Simulation) {
    pulse(sim, "start", 50);
    assert!(wait_for_ball(sim, 10_000), "start serves a ball");
}

/// Steps one tick at a time until a ball is on the table; `false` after
/// `max` ticks.
fn wait_for_ball(sim: &mut Simulation, max: u64) -> bool {
    for _ in 0..max {
        if sim.balls_in_play() > 0 {
            return true;
        }
        sim.advance(1);
    }
    sim.balls_in_play() > 0
}

/// The VUK delivers a ball to the upper deck (layer 1).
#[test]
fn vuk_delivers_to_layer_1() {
    big_stack(|| {
        let t = greybox_table();
        let vuk = t
            .manifest
            .mechs
            .iter()
            .find(|m| m.kind == MechKind::Vuk)
            .expect("VUK");
        let p = vuk.position.expect("VUK position");
        let layer = vuk.layer.unwrap_or(0);
        let mut sim = load();
        start_game(&mut sim);
        let r = steel_radius_m();
        let h = sim
            .spawn_ball_on_layer(
                STEEL,
                layer,
                [p[0] * 1e-3, p[1] * 1e-3, p[2] * 1e-3 + r],
                [0.0; 3],
            )
            .expect("spawn into the scoop");
        let vuk_hits = vuk.switch.as_ref().map(|s| (s.clone(), hits(&sim, s)));
        let mut delivered = false;
        for _ in 0..5_000 {
            sim.advance(1);
            if sim.ball(h).is_some_and(|b| b.layer == 1) {
                delivered = true;
                break;
            }
        }
        assert!(delivered, "the VUK put the ball on layer 1");
        if let Some((sw, before)) = vuk_hits {
            assert!(
                hits(&sim, &sw) > before,
                "VUK switch {sw} counted the capture"
            );
        }
        assert_healthy(&sim, "vuk");
    });
}

// ---------------------------------------------------------------------------
// Full 3-ball game (brief item 5)
// ---------------------------------------------------------------------------

/// Drains every ball on the table through the first trough switch: moved
/// onto it (when it has a position) or removed with the switch pulsed.
fn drain_all(sim: &mut Simulation, trough: &pin_assets::SwitchSi) {
    let handles: Vec<BallHandle> = sim.ball_handles().iter().copied().collect();
    for h in handles {
        let Some(mut b) = sim.ball(h) else { continue };
        match trough.position_m {
            Some(p) => {
                b.position_m = [p[0], p[1], p[2] + steel_radius_m()];
                b.velocity_m_s = [0.0; 3];
                b.angular_velocity_rad_s = [0.0; 3];
                b.layer = trough.layer;
                assert!(sim.set_ball_state(h, &b), "move the ball onto the trough");
                sim.advance(3);
                assert!(sim.ball(h).is_none(), "the ball drained into the trough");
            }
            None => {
                assert!(sim.remove_ball(h));
                pulse(sim, &trough.name, 5);
            }
        }
    }
}

#[test]
fn full_three_ball_game_by_injection() {
    big_stack(|| {
        let t = greybox_table();
        let trough = t
            .switches
            .iter()
            .find(|s| s.kind == SwitchKind::Trough && s.position_m.is_some())
            .or_else(|| t.switches.iter().find(|s| s.kind == SwitchKind::Trough))
            .expect("trough switch")
            .clone();
        let scorers: Vec<String> = t
            .manifest
            .mechs
            .iter()
            .filter(|m| matches!(m.kind, MechKind::Bumper | MechKind::Sling))
            .filter_map(|m| m.switch.clone())
            .collect();
        assert_eq!(scorers.len(), 5, "3 bumpers + 2 slings have switches");

        let mut sim = load();
        sim.advance(2_000);
        assert_eq!(sim.balls_in_play(), 0, "no ball before start");
        start_game(&mut sim);

        for ball in 1..=3u32 {
            if ball > 1 {
                assert!(
                    wait_for_ball(&mut sim, 10_000),
                    "ball {ball} is served after the previous drain"
                );
            }
            pulse(&mut sim, "launch", 50);
            for sw in &scorers {
                let before = sim.score(0).unwrap_or(0);
                pulse(&mut sim, sw, 20);
                sim.advance(20);
                let after = sim.score(0).unwrap_or(0);
                assert!(
                    after > before,
                    "ball {ball}: {sw} scores ({before} -> {after})"
                );
            }
            let drained = sim.counters().balls_drained;
            let trough_hits = hits(&sim, &trough.name);
            drain_all(&mut sim, &trough);
            assert!(
                hits(&sim, &trough.name) > trough_hits,
                "trough switch counted"
            );
            if trough.position_m.is_some() {
                assert!(sim.counters().balls_drained > drained);
            }
            assert_healthy(&sim, &format!("ball {ball}"));
        }

        // Game over: no fourth ball within 10 s.
        for _ in 0..10_000 {
            sim.advance(1);
            assert_eq!(sim.balls_in_play(), 0, "no ball after the 3rd drain");
        }
        let score = sim.score(0).expect("score reported");
        assert!(score > 0, "final score {score} > 0");
        assert_healthy(&sim, "game over");
    });
}

// ---------------------------------------------------------------------------
// Replay determinism (verification criterion; gate 6)
// ---------------------------------------------------------------------------

/// A recorded input stream (start, launch, flipper presses) replays to the
/// same `state_hash` 10 times.
#[test]
fn recorded_inputs_replay_identically_10_times() {
    big_stack(|| {
        let sim = Simulation::load(greybox_bundle(), SimConfig::default()).expect("loads");
        let mut rec = Recorder::new(sim);
        let ticks = 12_000u64;
        let schedule: Vec<(u64, CabinetInput, bool)> = {
            let mut v = vec![
                (100, CabinetInput::Start, true),
                (180, CabinetInput::Start, false),
                (1_500, CabinetInput::Launch, true),
                (1_600, CabinetInput::Launch, false),
            ];
            let mut k = 2_000;
            let mut left = true;
            while k < ticks - 500 {
                let i = if left {
                    CabinetInput::LeftFlipper
                } else {
                    CabinetInput::RightFlipper
                };
                v.push((k, i, true));
                v.push((k + 120, i, false));
                if k % 3 == 0 {
                    v.push((k + 40, CabinetInput::RightUpper, true));
                    v.push((k + 90, CabinetInput::RightUpper, false));
                }
                k += 377;
                left = !left;
            }
            v.sort_by_key(|e| e.0);
            v
        };
        let mut next = 0;
        let mut evs: Vec<InputEvent> = Vec::new();
        for tick in 1..=ticks {
            evs.clear();
            while next < schedule.len() && schedule[next].0 == tick {
                let (_, i, p) = schedule[next];
                evs.push(InputEvent::button(tick * TICK_NS - TICK_NS / 2, i, p));
                next += 1;
            }
            rec.step(&evs);
        }
        let (sim, recording) = rec.finish();
        assert!(sim.switch_hits("start").expect("start") >= 1);
        assert!(sim.switch_hits("left_flipper").expect("lf") >= 1);
        assert!(!sim.script_faulted());
        let expected = sim.state_hash();
        for run in 0..10 {
            let got = pin_sim::replay::replay(greybox_bundle(), &recording).expect("replays");
            assert_eq!(got, expected, "run {run}: replay diverged");
        }
        // The same stream through SimStep directly, on fresh loads.
        for run in 0..2 {
            let mut s = Simulation::load(greybox_bundle(), SimConfig::default()).expect("loads");
            assert_eq!(
                Recorder::play(&mut s, &recording),
                expected,
                "play run {run}"
            );
        }
    });
}
