//! Subphase 6.1 spec tests (Independent Tester), S3: the rules (brief item
//! 4 a..g), by state injection on simulated time.
//!
//! Rule events are forced switch pulses (`set_switch`), cabinet inputs go
//! through the real input path (`SimStep::step_1ms`), and the observables
//! are `Simulation::score(0)`, the `[persist]` values (index = manifest
//! order), `balls_in_play()`, the snapshot (ball profiles, zipper offsets,
//! lamps) and the script health counters. Two items use real physics: the
//! dispensed ball's profile (b) and the Powerball detector passed by a real
//! plunge (e).
//!
//! The numbers asserted come from the brief; where it defers to the
//! README (the Overdrive route, the combo trigger, the shot switches and
//! the bonus counters) the README's "Rules" section is the source.

#[path = "fixtures/spec_6_1/common.rs"]
mod common;

use common::*;
use pin_core::input::CabinetInput;
use pin_core::schema::table::MechKind;
use pin_sim::Simulation;
use proptest::prelude::*;

// ---------------------------------------------------------------------------
// Names (brief / README)
// ---------------------------------------------------------------------------

/// The switch of the first mech of `kind`.
fn mech_switch(kind: MechKind, nth: usize) -> String {
    table()
        .manifest
        .mechs
        .iter()
        .filter(|m| m.kind == kind)
        .nth(nth)
        .and_then(|m| m.switch.clone())
        .unwrap_or_else(|| panic!("{kind:?} #{nth} has a switch"))
}

fn bumper() -> String {
    mech_switch(MechKind::Bumper, 0)
}
fn sling() -> String {
    mech_switch(MechKind::Sling, 0)
}
fn spinner() -> String {
    mech_switch(MechKind::Spinner, 0)
}
fn drop(n: usize) -> String {
    mech_switch(MechKind::DropTarget, n)
}
fn scoop() -> String {
    mech_switch(MechKind::Vuk, 0)
}

/// README switch names.
const ORBIT_TOP: &str = "orbit_top";
const RIGHT_ORBIT: &str = "right_orbit_entry";
const RAMP_MADE: &str = "ramp_made";
const LEFT_INLANE: &str = "left_inlane";
const MAGNET_OPTO: &str = "magnet_opto";
const MAGNET_EDDY: &str = "magnet_eddy";
const OVERDRIVE_LAMP: &str = "overdrive_lamp";

/// Bumper points at multiplier 1 (README "Points").
const BUMPER: i64 = 100;
const SLING: i64 = 10;

// ---------------------------------------------------------------------------
// Drivers
// ---------------------------------------------------------------------------

/// Pulses `name` (20 ticks closed) and returns the score delta within the
/// following 30 ticks.
fn hit(sim: &mut Simulation, name: &str) -> i64 {
    let before = score(sim);
    pulse(sim, name, 20);
    sim.advance(30);
    score(sim) - before
}

/// A fresh game with ball 1 served (on the plunger; not launched).
fn new_game() -> Simulation {
    let mut sim = fresh();
    sim.advance(500);
    start_game(&mut sim);
    settle(&mut sim, 5_000);
    sim
}

/// Drains the ball in play and waits (≤ 10 s) for the next one; returns
/// the score jump between the drain and the serve (the bonus).
fn next_ball(sim: &mut Simulation) -> i64 {
    let before = score(sim);
    drain_ball(sim);
    let mut jump = score(sim) - before;
    for _ in 0..10_000 {
        if sim.balls_in_play() > 0 {
            break;
        }
        sim.advance(1);
        jump = score(sim) - before;
    }
    assert!(sim.balls_in_play() > 0, "the next ball is served");
    settle(sim, 5_000);
    jump
}

/// A fresh game on ball `n` (served, not launched).
fn game_on_ball(n: u32) -> Simulation {
    let mut sim = new_game();
    for _ in 1..n {
        next_ball(&mut sim);
    }
    assert_eq!(persist(&sim, "ball"), i64::from(n), "on ball {n}");
    sim
}

/// Presses LAUNCH (50 ticks) with the served ball resting on the plunger
/// tip and steps until the plunger has fired it up the shooter lane
/// (≥ 1 m/s up the table, ≤ 3 s); returns whether it did.
fn plunge(sim: &mut Simulation) -> bool {
    let Some(&h) = sim.ball_handles().first() else {
        return false;
    };
    cabinet(sim, CabinetInput::Launch, true);
    let mut released = false;
    let mut fired = false;
    for i in 0..3_000 {
        if i == 50 {
            cabinet(sim, CabinetInput::Launch, false);
            released = true;
        } else {
            sim.advance(1);
        }
        if sim.ball(h).is_some_and(|b| b.velocity_m_s[1] > 1.0) {
            fired = true;
            break;
        }
    }
    if !released {
        cabinet(sim, CabinetInput::Launch, false);
    }
    fired
}

/// The plunge, then the ball is taken off the table (no drain) once it
/// has travelled 0.5 m up the shooter lane, well before it reaches the top
/// (decision log "6.1-T-4 — the injected plunge").
fn plunge_and_clear(sim: &mut Simulation) {
    let lanes = top_lanes();
    let before: Vec<u32> = lanes.iter().map(|l| hits(sim, l)).collect();
    let h = *sim.ball_handles().first().expect("a served ball");
    let y0 = sim.ball(h).expect("ball").position_m[1];
    assert!(plunge(sim), "LAUNCH fired the ball");
    let mut up = false;
    for _ in 0..1_000 {
        if sim.ball(h).is_some_and(|b| b.position_m[1] > y0 + 0.5) {
            up = true;
            break;
        }
        sim.advance(1);
    }
    assert!(up, "the plunged ball went 0.5 m up the shooter lane");
    clear_table(sim);
    let after: Vec<u32> = lanes.iter().map(|l| hits(sim, l)).collect();
    assert_eq!(
        before, after,
        "no top lane was hit before the ball was cleared"
    );
    sim.advance(20);
}

/// Bumper delta / 100: the playfield multiplier now (one bumper pulse).
fn multiplier(sim: &mut Simulation) -> i64 {
    let d = hit(sim, &bumper());
    assert_eq!(d % BUMPER, 0, "bumper delta {d} is a multiple of 100");
    d / BUMPER
}

/// Feeds the combo (spinner pulse) and makes `steps` combo hits on
/// `orbit_top`, each within 4 s; returns the tick the last pulse closed.
fn combo_steps(sim: &mut Simulation, steps: u32) -> u64 {
    pulse(sim, &spinner(), 10);
    sim.advance(200);
    let mut last = sim.tick();
    for _ in 0..steps {
        last = sim.tick() + 1;
        pulse(sim, ORBIT_TOP, 10);
        sim.advance(200);
    }
    last
}

fn lamp(sim: &Simulation, name: &str) -> f32 {
    let id = sim
        .ids()
        .lamp(name)
        .unwrap_or_else(|| panic!("lamp {name}"));
    sim.snapshot().lamps[usize::from(id.0)]
}

fn zipper_offsets(sim: &Simulation) -> [f32; 2] {
    let mut out = [0.0; 2];
    for (i, f) in table().manifest.flippers.iter().enumerate() {
        let k = match f.id.as_str() {
            "left_flipper" => 0,
            "right_flipper" => 1,
            _ => continue,
        };
        out[k] = sim.snapshot().flippers()[i].zipper_offset_m;
    }
    out
}

fn declared_slide_m() -> f32 {
    let f = table()
        .manifest
        .flippers
        .iter()
        .find(|f| f.id == "left_flipper")
        .unwrap();
    let a = f.zipper_slide_axis.expect("zipper axis");
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt() * 1e-3
}

/// Lights Overdrive Ready by the README route (3 `ramp_made` on a ball);
/// returns the tick at which it lit.
fn light_overdrive(sim: &mut Simulation) -> u64 {
    assert_eq!(persist(sim, "overdrive_ready"), 0, "not lit before");
    for _ in 0..3 {
        pulse(sim, RAMP_MADE, 10);
        sim.advance(100);
    }
    // Find the lighting tick precisely: it happened during the last pulse.
    assert_eq!(
        persist(sim, "overdrive_ready"),
        1,
        "3 ramps light Overdrive"
    );
    assert!(lamp(sim, OVERDRIVE_LAMP) > 0.0, "overdrive_lamp lit");
    sim.tick()
}

/// Counters the action features touch.
fn action_counters(sim: &Simulation) -> [i64; 3] {
    [
        persist(sim, "firewall_save_count"),
        persist(sim, "apex_jackpot_count"),
        persist(sim, "skill_shot_count"),
    ]
}

// ---------------------------------------------------------------------------
// Names used here exist
// ---------------------------------------------------------------------------

#[test]
fn readme_rule_names_exist() {
    big_stack(|| {
        let sim = fresh();
        for s in [
            ORBIT_TOP,
            RIGHT_ORBIT,
            RAMP_MADE,
            LEFT_INLANE,
            "right_inlane",
            MAGNET_OPTO,
            MAGNET_EDDY,
        ] {
            assert!(sim.switch_id(s).is_some(), "README switch {s}");
        }
        assert!(sim.ids().lamp(OVERDRIVE_LAMP).is_some(), "overdrive_lamp");
        for k in ["overdrive_ready", "last_bonus", "combo_level"] {
            assert!(sim.ids().persist_key(k).is_some(), "README persist {k}");
        }
        assert_eq!(top_lanes().len(), 3, "lane_1..3");
    });
}

// ---------------------------------------------------------------------------
// a. Game flow
// ---------------------------------------------------------------------------

#[test]
fn a_start_three_balls_game_over_attract() {
    big_stack(|| {
        let mut sim = fresh();
        sim.advance(2_000);
        assert_eq!(sim.balls_in_play(), 0, "no ball in attract");
        let attract = persist(&sim, "game_state");
        let played = persist(&sim, "games_played");
        start_game(&mut sim);
        assert_ne!(persist(&sim, "game_state"), attract, "a game is running");
        for ball in 1..=3i64 {
            if ball > 1 {
                assert!(wait_for_ball(&mut sim, 10_000), "ball {ball} served");
            }
            settle(&mut sim, 5_000);
            assert_eq!(persist(&sim, "ball"), ball, "ball {ball} in play");
            assert_eq!(sim.balls_in_play(), 1);
            // LAUNCH plunges it: the plunger fires the ball up the table.
            assert!(plunge(&mut sim), "ball {ball}: LAUNCH plunged it");
            sim.advance(100);
            let s = hit(&mut sim, &bumper());
            assert!(s > 0, "ball {ball}: scoring");
            let drained = sim.counters().balls_drained;
            drain_ball(&mut sim);
            assert!(
                sim.counters().balls_drained > drained,
                "ball {ball} drained"
            );
            assert_healthy(&sim, &format!("ball {ball}"));
        }
        // Game over: no fourth ball within 10 s; attract returns.
        let mut back = false;
        for _ in 0..15_000 {
            sim.advance(1);
            assert_eq!(sim.balls_in_play(), 0, "no ball after the 3rd drain");
            if persist(&sim, "game_state") == attract {
                back = true;
            }
        }
        assert!(back, "attract returned within 15 s");
        assert_eq!(persist(&sim, "ball"), 0, "no ball in play after the game");
        assert_eq!(
            persist(&sim, "games_played"),
            played + 1,
            "games_played + 1"
        );
        let final_score = score(&sim);
        assert!(final_score > 0, "final score {final_score}");
        assert!(
            persist(&sim, "high_score") >= final_score,
            "high score stored"
        );
        // START works again from attract.
        start_game(&mut sim);
        assert_eq!(persist(&sim, "ball"), 1);
        assert_healthy(&sim, "game over");
    });
}

// ---------------------------------------------------------------------------
// b. Dispenser order
// ---------------------------------------------------------------------------

#[test]
fn b_balls_1_and_3_steel_ball_2_powerball() {
    big_stack(|| {
        let mut sim = new_game();
        for (ball, want) in [(1, STEEL), (2, POWERBALL), (3, STEEL)] {
            if ball > 1 {
                next_ball(&mut sim);
            }
            assert_eq!(
                snapshot_profiles(&sim),
                vec![want.to_owned()],
                "ball {ball} profile"
            );
        }
        assert_healthy(&sim, "dispenser order");
    });
}

// ---------------------------------------------------------------------------
// c. Skill shot
// ---------------------------------------------------------------------------

/// (lane delta, skill_shot_count delta) of the first lane hit after a
/// plunge on ball `n`, for lane `lane`, on a fresh state.
fn skill_try(n: u32, lane: &str) -> (i64, i64) {
    let mut sim = game_on_ball(n);
    plunge_and_clear(&mut sim);
    let c0 = persist(&sim, "skill_shot_count");
    let d = hit(&mut sim, lane);
    let dc = persist(&sim, "skill_shot_count") - c0;
    assert_healthy(&sim, "skill shot");
    (d, dc)
}

#[test]
fn c_skill_shot_lit_lane_rotates_and_awards() {
    big_stack(|| {
        let lanes = top_lanes();
        let mut lit = Vec::new();
        for n in 1..=3u32 {
            let results: Vec<(String, (i64, i64))> =
                lanes.iter().map(|l| (l.clone(), skill_try(n, l))).collect();
            let big = 10_000 * i64::from(n);
            let lit_lanes: Vec<&String> = results
                .iter()
                .filter(|(_, (d, _))| *d == big)
                .map(|(l, _)| l)
                .collect();
            assert_eq!(
                lit_lanes.len(),
                1,
                "ball {n}: exactly one lane awards {big}: {results:?}"
            );
            for (l, (d, dc)) in &results {
                if *d == big {
                    assert_eq!(*dc, 1, "ball {n} {l}: skill_shot_count + 1");
                } else {
                    assert_eq!(*d, 1_000, "ball {n} {l}: an unlit lane awards 1,000");
                    assert_eq!(*dc, 0, "ball {n} {l}: no skill shot counted");
                }
            }
            lit.push(lit_lanes[0].clone());
        }
        assert_ne!(lit[0], lit[1], "the lit lane rotates (ball 1 -> 2)");
        assert_ne!(lit[1], lit[2], "the lit lane rotates (ball 2 -> 3)");
    });
}

#[test]
fn c_skill_shot_only_first_hit_and_only_within_6s() {
    big_stack(|| {
        let lanes = top_lanes();
        // Ball 1: the lit lane, found on fresh states.
        let lit = lanes
            .iter()
            .find(|l| skill_try(1, l).0 == 10_000)
            .expect("a lit lane on ball 1")
            .clone();
        // Second hit in the window: no skill award.
        let mut sim = game_on_ball(1);
        plunge_and_clear(&mut sim);
        let other = lanes.iter().find(|l| **l != lit).unwrap();
        assert_eq!(hit(&mut sim, other), 1_000, "first hit, unlit lane");
        let c0 = persist(&sim, "skill_shot_count");
        assert_eq!(
            hit(&mut sim, &lit),
            1_000,
            "the lit lane after the first hit"
        );
        assert_eq!(persist(&sim, "skill_shot_count"), c0);
        // The lit lane after 6 s: no skill award.
        let mut sim = game_on_ball(1);
        plunge_and_clear(&mut sim);
        sim.advance(6_100);
        let c0 = persist(&sim, "skill_shot_count");
        assert_eq!(hit(&mut sim, &lit), 1_000, "the lit lane after 6 s");
        assert_eq!(persist(&sim, "skill_shot_count"), c0);
        // Within 6 s (at 5.8 s): awarded.
        let mut sim = game_on_ball(1);
        plunge_and_clear(&mut sim);
        sim.advance(5_700);
        assert_eq!(hit(&mut sim, &lit), 10_000, "the lit lane at 5.8 s");
        assert_healthy(&sim, "skill window");
    });
}

// ---------------------------------------------------------------------------
// d. Combo multipliers
// ---------------------------------------------------------------------------

#[test]
fn d_combo_levels_2x_to_5x_and_mode_start() {
    big_stack(|| {
        let mut sim = game_on_ball(1);
        assert_eq!(multiplier(&mut sim), 1, "no combo");
        let m0 = persist(&sim, "mode:combo_chain");
        combo_steps(&mut sim, 1);
        assert_eq!(multiplier(&mut sim), 2, "first combo hit: 2x");
        assert_eq!(
            persist(&sim, "mode:combo_chain"),
            m0 + 1,
            "combo_chain started"
        );
        assert_eq!(hit(&mut sim, &sling()), 2 * SLING, "sling at 2x");
        for level in 3..=5 {
            pulse(&mut sim, ORBIT_TOP, 10);
            sim.advance(200);
            assert_eq!(multiplier(&mut sim), level, "combo level {level}");
        }
        // Capped at 5.
        pulse(&mut sim, ORBIT_TOP, 10);
        sim.advance(200);
        assert_eq!(multiplier(&mut sim), 5, "combo capped at 5x");
        assert_eq!(
            persist(&sim, "mode:combo_chain"),
            m0 + 1,
            "combo_chain starts once"
        );
        assert_healthy(&sim, "combo");
    });
}

#[test]
fn d_combo_decays_after_12s_and_resets_on_drain() {
    big_stack(|| {
        let mut sim = game_on_ball(1);
        let last = combo_steps(&mut sim, 2);
        assert_eq!(multiplier(&mut sim), 3);
        // The bumper probes are not combo hits. At 11.5 s: still 3x.
        sim.advance(last + 11_500 - sim.tick());
        assert_eq!(multiplier(&mut sim), 3, "still 3x before 12 s");
        sim.advance(last + 12_300 - sim.tick());
        assert_eq!(multiplier(&mut sim), 2, "one step down after 12 s");
        sim.advance(last + 24_500 - sim.tick());
        assert_eq!(multiplier(&mut sim), 1, "another step after 24 s");
        // Back to 2x on the same ball does not restart the mode.
        let m = persist(&sim, "mode:combo_chain");
        combo_steps(&mut sim, 1);
        assert_eq!(multiplier(&mut sim), 2);
        assert_eq!(persist(&sim, "mode:combo_chain"), m, "once per ball");
        // Drain resets it; the next ball can start it again.
        combo_steps(&mut sim, 2);
        assert!(multiplier(&mut sim) >= 3);
        next_ball(&mut sim);
        assert_eq!(multiplier(&mut sim), 1, "reset on drain");
        combo_steps(&mut sim, 1);
        assert_eq!(multiplier(&mut sim), 2);
        assert_eq!(
            persist(&sim, "mode:combo_chain"),
            m + 1,
            "new ball, new start"
        );
        assert_healthy(&sim, "combo decay");
    });
}

#[test]
fn d_a_shot_without_a_feed_is_no_combo() {
    big_stack(|| {
        let mut sim = game_on_ball(1);
        pulse(&mut sim, ORBIT_TOP, 10);
        sim.advance(200);
        assert_eq!(multiplier(&mut sim), 1, "no spinner feed, no combo");
        pulse(&mut sim, &spinner(), 10);
        sim.advance(4_500);
        pulse(&mut sim, ORBIT_TOP, 10);
        sim.advance(200);
        assert_eq!(multiplier(&mut sim), 1, "a shot 4.5 s after the feed");
    });
}

// ---------------------------------------------------------------------------
// e. Powerball Mania
// ---------------------------------------------------------------------------

/// Launches the served ball and plays it (no input) until the magnet opto
/// closes by physics (≤ 4 s); returns whether it did.
fn plunge_past_detector(sim: &mut Simulation) -> bool {
    let opto = hits(sim, MAGNET_OPTO);
    if !plunge(sim) {
        return false;
    }
    for _ in 0..4_000 {
        if hits(sim, MAGNET_OPTO) > opto {
            sim.advance(100);
            return true;
        }
        sim.advance(1);
    }
    false
}

#[test]
fn e_powerball_plunge_starts_mania_steel_does_not() {
    big_stack(|| {
        let mut sim = game_on_ball(1);
        let m0 = persist(&sim, "mode:powerball_mania");
        let eddy = hits(&sim, MAGNET_EDDY);
        assert!(plunge_past_detector(&mut sim), "ball 1 passes the opto");
        assert!(hits(&sim, MAGNET_EDDY) > eddy, "steel closes the eddy too");
        sim.advance(200);
        assert_eq!(
            persist(&sim, "mode:powerball_mania"),
            m0,
            "a steel ball does not start Powerball Mania"
        );
        clear_table(&mut sim);
        sim.advance(50);
        assert_eq!(multiplier(&mut sim), 1);
        next_ball(&mut sim);
        // Ball 2: the Powerball.
        assert_eq!(snapshot_profiles(&sim), vec![POWERBALL.to_owned()]);
        let eddy = hits(&sim, MAGNET_EDDY);
        assert!(plunge_past_detector(&mut sim), "ball 2 passes the opto");
        assert_eq!(hits(&sim, MAGNET_EDDY), eddy, "the eddy stays open");
        assert_eq!(
            persist(&sim, "mode:powerball_mania"),
            m0 + 1,
            "the Powerball starts Powerball Mania"
        );
        clear_table(&mut sim);
        sim.advance(50);
        assert_eq!(multiplier(&mut sim), 2, "mania doubles playfield scoring");
        assert_eq!(hit(&mut sim, &sling()), 2 * SLING);
        sim.advance(30_000);
        assert_eq!(multiplier(&mut sim), 2, "until the ball drains");
        next_ball(&mut sim);
        assert_eq!(multiplier(&mut sim), 1, "ends at the drain");
        assert_healthy(&sim, "mania");
    });
}

#[test]
fn e_injected_opto_without_eddy_starts_mania_with_eddy_does_not() {
    big_stack(|| {
        // Opto then eddy within a few ms (a steel ball): no mode.
        let mut sim = game_on_ball(1);
        let m0 = persist(&sim, "mode:powerball_mania");
        sim.set_switch(MAGNET_OPTO, true).unwrap();
        sim.advance(3);
        sim.set_switch(MAGNET_EDDY, true).unwrap();
        sim.advance(30);
        sim.set_switch(MAGNET_OPTO, false).unwrap();
        sim.set_switch(MAGNET_EDDY, false).unwrap();
        sim.advance(100);
        sim.release_switch(MAGNET_OPTO).unwrap();
        sim.release_switch(MAGNET_EDDY).unwrap();
        assert_eq!(
            persist(&sim, "mode:powerball_mania"),
            m0,
            "eddy closed: steel"
        );
        assert_eq!(multiplier(&mut sim), 1);
        // Opto alone (the Powerball): mode starts.
        pulse(&mut sim, MAGNET_OPTO, 20);
        sim.advance(100);
        assert_eq!(persist(&sim, "mode:powerball_mania"), m0 + 1);
        assert_eq!(multiplier(&mut sim), 2);
        // With combo 5x: 10x, the cap.
        combo_steps(&mut sim, 4);
        assert_eq!(multiplier(&mut sim), 10, "5x combo x 2 mania = 10 (cap)");
        combo_steps(&mut sim, 2);
        assert!(multiplier(&mut sim) <= 10, "never above 10");
        assert_healthy(&sim, "injected mania");
    });
}

// ---------------------------------------------------------------------------
// f. Overdrive / action
// ---------------------------------------------------------------------------

#[test]
fn f_action_without_overdrive_changes_nothing() {
    big_stack(|| {
        let mut sim = game_on_ball(1);
        // Also after some scoring that does not reach Overdrive.
        pulse(&mut sim, RAMP_MADE, 10);
        sim.advance(100);
        pulse(&mut sim, LEFT_INLANE, 10);
        sim.advance(100);
        assert_eq!(persist(&sim, "overdrive_ready"), 0);
        let s0 = score(&sim);
        let c0 = action_counters(&sim);
        press(&mut sim, CabinetInput::Action, 50);
        for _ in 0..6_000 {
            sim.advance(1);
            assert_eq!(zipper_offsets(&sim), [0.0, 0.0], "no zipper");
        }
        assert_eq!(score(&sim), s0, "no score");
        assert_eq!(action_counters(&sim), c0, "no counter");
        assert_healthy(&sim, "action without overdrive");
    });
}

#[test]
fn f_overdrive_lights_by_three_ramps_or_five_combo_steps() {
    big_stack(|| {
        let mut sim = game_on_ball(1);
        light_overdrive(&mut sim);
        let mut sim = game_on_ball(1);
        combo_steps(&mut sim, 4);
        assert_eq!(
            persist(&sim, "overdrive_ready"),
            0,
            "4 combo steps: not yet"
        );
        pulse(&mut sim, ORBIT_TOP, 10);
        sim.advance(200);
        assert_eq!(
            persist(&sim, "overdrive_ready"),
            1,
            "5 combo steps light it"
        );
        assert!(lamp(&sim, OVERDRIVE_LAMP) > 0.0);
        // Lost at the drain.
        next_ball(&mut sim);
        assert_eq!(persist(&sim, "overdrive_ready"), 0, "lost at the drain");
        let c0 = action_counters(&sim);
        press(&mut sim, CabinetInput::Action, 50);
        sim.advance(500);
        assert_eq!(action_counters(&sim), c0, "nothing after the drain");
    });
}

/// Jackpot value for `elapsed_ms` since Overdrive lit (brief numbers).
fn jackpot(elapsed_ms: u64) -> i64 {
    (50_000 - 1_000 * (elapsed_ms / 100) as i64).max(5_000)
}

#[test]
fn f_apex_jackpot_decays_from_50000_by_1000_per_100ms_floor_5000() {
    big_stack(|| {
        for wait in [0u64, 250, 1_050, 2_480, 4_400, 6_000, 20_000] {
            // One ball on the plunger and no inlane: the Jackpot.
            let mut sim = game_on_ball(1);
            // Light it with the third ramp and note the tick.
            for _ in 0..2 {
                pulse(&mut sim, RAMP_MADE, 10);
                sim.advance(100);
            }
            sim.set_switch(RAMP_MADE, true).unwrap();
            let mut lit_at = None;
            for _ in 0..20 {
                sim.advance(1);
                if persist(&sim, "overdrive_ready") == 1 {
                    lit_at = Some(sim.tick());
                    break;
                }
            }
            let lit_at = lit_at.expect("third ramp lights Overdrive");
            sim.set_switch(RAMP_MADE, false).unwrap();
            sim.advance(5);
            sim.release_switch(RAMP_MADE).unwrap();
            let target = lit_at + wait.max(10);
            sim.advance(target - sim.tick());
            let c0 = action_counters(&sim);
            let s0 = score(&sim);
            let pressed_at = sim.tick() + 1;
            cabinet(&mut sim, CabinetInput::Action, true);
            sim.advance(20);
            cabinet(&mut sim, CabinetInput::Action, false);
            sim.advance(30);
            let d = score(&sim) - s0;
            let c1 = action_counters(&sim);
            assert_eq!(c1[0], c0[0], "wait {wait}: not a Firewall Save");
            assert_eq!(c1[1], c0[1] + 1, "wait {wait}: apex_jackpot_count + 1");
            // The press lands within ±2 ticks of `pressed_at - lit_at`.
            let e = pressed_at - lit_at;
            let ok = (e.saturating_sub(2)..=e + 2).any(|ms| jackpot(ms) == d);
            assert!(
                ok,
                "wait {wait}: jackpot {d} after {e} ms (expected {})",
                jackpot(e)
            );
            assert!((5_000..=50_000).contains(&d));
            assert_eq!(persist(&sim, "overdrive_ready"), 0, "used up");
            // A second press does nothing.
            let s1 = score(&sim);
            press(&mut sim, CabinetInput::Action, 20);
            sim.advance(200);
            assert_eq!(score(&sim), s1, "second press: no score");
            assert_eq!(action_counters(&sim), c1, "second press: no counter");
            assert_healthy(&sim, "jackpot");
        }
    });
}

#[test]
fn f_firewall_save_closes_the_zipper_for_5000_ticks() {
    big_stack(|| {
        let mut sim = game_on_ball(1);
        assert_eq!(sim.balls_in_play(), 1);
        light_overdrive(&mut sim);
        pulse(&mut sim, LEFT_INLANE, 10);
        sim.advance(100);
        let c0 = action_counters(&sim);
        let s0 = score(&sim);
        assert_eq!(zipper_offsets(&sim), [0.0, 0.0]);
        let slide = declared_slide_m();
        let mut first_move: Option<u64> = None;
        let mut full: Option<u64> = None;
        let mut retract: Option<u64> = None;
        let mut home: Option<u64> = None;
        let mut prev = zipper_offsets(&sim);
        for i in 0..8_000 {
            match i {
                0 => cabinet(&mut sim, CabinetInput::Action, true),
                20 => cabinet(&mut sim, CabinetInput::Action, false),
                _ => sim.advance(1),
            }
            let z = zipper_offsets(&sim);
            let t = sim.tick();
            if first_move.is_none() && (z[0] > 0.0 || z[1] > 0.0) {
                first_move = Some(t);
            }
            if full.is_none() && (z[0] - slide).abs() < 1e-5 && (z[1] - slide).abs() < 1e-5 {
                full = Some(t);
            }
            if full.is_some() && retract.is_none() && (z[0] < prev[0] || z[1] < prev[1]) {
                retract = Some(t);
            }
            if retract.is_some() && home.is_none() && z == [0.0, 0.0] {
                home = Some(t);
            }
            prev = z;
        }
        let first_move = first_move.expect("the zipper moved");
        assert!(full.is_some(), "both zippers reached the declared slide");
        let retract = retract.expect("the zipper reopened");
        assert!(home.is_some(), "the zipper returned to zero");
        let closed = retract - first_move;
        assert!(
            closed.abs_diff(5_000) <= 2,
            "zipper commanded closed for {closed} ticks (5000 +- 2)"
        );
        let c1 = action_counters(&sim);
        assert_eq!(c1[0], c0[0] + 1, "firewall_save_count + 1");
        assert_eq!(c1[1], c0[1], "not a Jackpot");
        assert!(score(&sim) - s0 < 5_000, "no jackpot award");
        assert_eq!(persist(&sim, "overdrive_ready"), 0, "used up");
        assert_healthy(&sim, "firewall");
    });
}

// ---------------------------------------------------------------------------
// g. Bonus
// ---------------------------------------------------------------------------

/// (situation, injected switches, combo steps before the drain, expected
/// bonus counter sum).
fn bonus_cases() -> Vec<(&'static str, Vec<String>, u32, i64)> {
    vec![
        ("nothing", vec![], 0, 0),
        (
            "one of each counter",
            vec![
                RAMP_MADE.into(),
                ORBIT_TOP.into(),
                RIGHT_ORBIT.into(),
                drop(0),
                scoop(),
                top_lanes()[0].clone(),
            ],
            0,
            6,
        ),
        (
            "two orbits, two drops",
            vec![ORBIT_TOP.into(), ORBIT_TOP.into(), drop(0), drop(1)],
            0,
            4,
        ),
        // combo_steps adds one spinner (no counter) and `steps` orbit_top hits.
        ("combo 3x", vec![RAMP_MADE.into()], 2, 3),
        ("combo 2x", vec![], 1, 1),
    ]
}

#[test]
fn g_bonus_is_counter_sum_times_1000_times_combo_level() {
    big_stack(|| {
        for (what, switches, steps, sum) in bonus_cases() {
            let mut sim = game_on_ball(1);
            for s in &switches {
                pulse(&mut sim, s, 10);
                sim.advance(1_100);
            }
            if steps > 0 {
                combo_steps(&mut sim, steps);
            }
            let level = i64::from(steps) + 1;
            let bonus = next_ball(&mut sim);
            let expected = sum * 1_000 * level;
            assert_eq!(bonus, expected, "{what}: bonus at the drain");
            assert_eq!(persist(&sim, "last_bonus"), expected, "{what}: last_bonus");
            assert_healthy(&sim, what);
        }
    });
}

#[test]
fn g_full_game_scores_and_stays_healthy() {
    big_stack(|| {
        let mut sim = new_game();
        for ball in 1..=3 {
            press(&mut sim, CabinetInput::Launch, 50);
            sim.advance(3_000);
            for s in [
                bumper(),
                sling(),
                spinner(),
                ORBIT_TOP.into(),
                RAMP_MADE.into(),
            ] {
                hit(&mut sim, &s);
            }
            assert_healthy(&sim, &format!("ball {ball}"));
            drain_ball(&mut sim);
            if ball < 3 {
                assert!(wait_for_ball(&mut sim, 10_000));
                settle(&mut sim, 5_000);
            }
        }
        sim.advance(12_000);
        assert_eq!(sim.balls_in_play(), 0);
        assert!(score(&sim) > 0, "final score > 0");
        assert_eq!(sim.script_queue_overflows(), 0);
        assert!(!sim.script_faulted());
    });
}

// ---------------------------------------------------------------------------
// Property: multiplier bound and monotone score
// ---------------------------------------------------------------------------

/// Injected rule events.
#[derive(Clone, Debug)]
enum Ev {
    Bumper,
    Sling,
    Spinner,
    OrbitTop,
    RightOrbit,
    Ramp,
    Drop(usize),
    Lane(usize),
    Scoop,
    PowerballOpto,
    Inlane,
    Action,
    Wait(u64),
}

fn ev() -> impl Strategy<Value = Ev> {
    prop_oneof![
        3 => Just(Ev::Bumper),
        2 => Just(Ev::Sling),
        2 => Just(Ev::Spinner),
        3 => Just(Ev::OrbitTop),
        1 => Just(Ev::RightOrbit),
        1 => Just(Ev::Ramp),
        1 => (0usize..3).prop_map(Ev::Drop),
        1 => (0usize..3).prop_map(Ev::Lane),
        1 => Just(Ev::Scoop),
        1 => Just(Ev::PowerballOpto),
        1 => Just(Ev::Inlane),
        1 => Just(Ev::Action),
        2 => (50u64..13_000).prop_map(Ev::Wait),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 12, failure_persistence: None, ..ProptestConfig::default() })]

    #[test]
    fn prop_multiplier_never_exceeds_10_and_score_never_decreases(
        ball in 1u32..=2,
        seq in proptest::collection::vec(ev(), 1..40),
    ) {
        let r = big_stack(move || {
            let mut sim = game_on_ball(ball);
            let mut last = score(&sim);
            for e in &seq {
                match e {
                    Ev::Bumper => {
                        let d = hit(&mut sim, &bumper());
                        if d % BUMPER != 0 || d / BUMPER > 10 || d < BUMPER {
                            return Err(format!("bumper delta {d} after {seq:?}"));
                        }
                    }
                    Ev::Sling => {
                        let d = hit(&mut sim, &sling());
                        if !(SLING..=10 * SLING).contains(&d) {
                            return Err(format!("sling delta {d} after {seq:?}"));
                        }
                    }
                    Ev::Spinner => { hit(&mut sim, &spinner()); }
                    Ev::OrbitTop => { hit(&mut sim, ORBIT_TOP); }
                    Ev::RightOrbit => { hit(&mut sim, RIGHT_ORBIT); }
                    Ev::Ramp => { hit(&mut sim, RAMP_MADE); }
                    Ev::Drop(i) => { hit(&mut sim, &drop(*i)); }
                    Ev::Lane(i) => { hit(&mut sim, &top_lanes()[*i]); }
                    Ev::Scoop => { hit(&mut sim, &scoop()); }
                    Ev::PowerballOpto => { hit(&mut sim, MAGNET_OPTO); }
                    Ev::Inlane => { hit(&mut sim, LEFT_INLANE); }
                    Ev::Action => {
                        press(&mut sim, CabinetInput::Action, 20);
                        sim.advance(30);
                    }
                    Ev::Wait(ms) => {
                        // Score never decreases tick by tick either.
                        for _ in 0..*ms {
                            sim.advance(1);
                            let s = score(&sim);
                            if s < last {
                                return Err(format!("score fell {last} -> {s}"));
                            }
                            last = s;
                        }
                    }
                }
                let s = score(&sim);
                if s < last {
                    return Err(format!("score fell {last} -> {s} at {e:?}"));
                }
                last = s;
                if sim.script_faulted() || sim.script_queue_overflows() != 0 {
                    return Err(format!("script unhealthy at {e:?}: {:?}", sim.script_error()));
                }
            }
            Ok(())
        });
        prop_assert!(r.is_ok(), "{}", r.unwrap_err());
    }
}

/// The table of injected situations: (situation, expected score delta).
#[test]
fn injected_situation_table() {
    big_stack(|| {
        type Setup = fn(&mut Simulation);
        let none: Setup = |_| {};
        let mania: Setup = |s| {
            pulse(s, MAGNET_OPTO, 20);
            s.advance(100);
        };
        let combo3: Setup = |s| {
            combo_steps(s, 2);
        };
        let mania_combo5: Setup = |s| {
            pulse(s, MAGNET_OPTO, 20);
            s.advance(100);
            combo_steps(s, 4);
        };
        // (situation, setup, switch, expected delta)
        let cases: Vec<(&str, Setup, String, i64)> = vec![
            ("bumper", none, bumper(), 100),
            ("sling", none, sling(), 10),
            ("spinner", none, spinner(), 50),
            ("orbit top", none, ORBIT_TOP.into(), 2_000),
            ("right orbit", none, RIGHT_ORBIT.into(), 1_000),
            ("ramp made", none, RAMP_MADE.into(), 10_000),
            ("drop", none, drop(0), 750),
            ("scoop", none, scoop(), 5_000),
            ("inlane", none, LEFT_INLANE.into(), 100),
            ("lane, no plunge", none, top_lanes()[0].clone(), 1_000),
            ("bumper in mania", mania, bumper(), 200),
            ("ramp in mania", mania, RAMP_MADE.into(), 20_000),
            ("bumper at 3x", combo3, bumper(), 300),
            ("lane at 3x", combo3, top_lanes()[1].clone(), 3_000),
            (
                "bumper at 5x in mania (cap 10)",
                mania_combo5,
                bumper(),
                1_000,
            ),
        ];
        for (what, setup, sw, want) in cases {
            let mut sim = game_on_ball(1);
            setup(&mut sim);
            let d = hit(&mut sim, &sw);
            assert_eq!(d, want, "{what}: {sw}");
            assert_healthy(&sim, what);
        }
    });
}

#[test]
fn drop_bank_completion_scores_the_bank() {
    big_stack(|| {
        // A dropped target's switch stays closed until the bank resets:
        // the injection holds each one closed.
        let mut sim = game_on_ball(1);
        let mut deltas = Vec::new();
        for i in 0..3 {
            let before = score(&sim);
            sim.set_switch(&drop(i), true).unwrap();
            sim.advance(50);
            deltas.push(score(&sim) - before);
        }
        for i in 0..3 {
            sim.set_switch(&drop(i), false).unwrap();
        }
        sim.advance(10);
        for i in 0..3 {
            sim.release_switch(&drop(i)).unwrap();
        }
        assert_eq!(&deltas[..2], &[750, 750]);
        assert_eq!(deltas[2], 750 + 10_000, "third drop completes the bank");
        assert_healthy(&sim, "drop bank");
    });
}
