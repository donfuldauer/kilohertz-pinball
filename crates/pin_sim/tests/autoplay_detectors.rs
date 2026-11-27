//! Subphase 2.9 requirement 3: the stuck-ball and escaped-ball detectors on
//! injected ball states (`Simulation::set_ball_state`), their exemptions,
//! and the bot's table-free flipper inference, start, plunge and nudge
//! behaviour.

#[path = "fixtures/autoplay/support.rs"]
mod support;

use pin_core::input::CabinetInput;
use pin_core::snapshot::BallState;
use pin_sim::autoplay::{
    Bot, BotConfig, FindingKind, GameMonitor, NudgeConfig, SkillLevel, SkillParams, TableInfo,
};
use pin_sim::{SimConfig, Simulation};
use support::*;

const BALL_Z: f32 = 0.0135;

fn load(name: &str) -> (Simulation, TableInfo) {
    let b = fixture(name);
    (
        Simulation::load(&b, SimConfig::default()).unwrap(),
        TableInfo::load(&b).unwrap(),
    )
}

fn at_rest(p: [f32; 3]) -> BallState {
    BallState {
        position_m: p,
        ..BallState::default()
    }
}

/// Pins a ball at `p` (zero velocity) before every tick for `ticks` ticks,
/// observing after each; returns the findings.
fn pinned(
    sim: &mut Simulation,
    info: &TableInfo,
    p: [f32; 3],
    ticks: u64,
    stuck_ticks: u64,
) -> Vec<(FindingKind, String)> {
    let h = sim.spawn_ball("standard_steel", p, [0.0; 3]).unwrap();
    let mut mon = GameMonitor::new(info, sim, 0, stuck_ticks);
    for _ in 0..ticks {
        assert!(sim.set_ball_state(h, &at_rest(p)));
        sim.advance(1);
        mon.observe(sim);
    }
    assert!(
        mon.all_balls_stuck()
            == mon
                .findings()
                .iter()
                .any(|f| f.kind == FindingKind::StuckBall)
    );
    mon.findings()
        .iter()
        .map(|f| (f.kind, f.subject.clone()))
        .collect()
}

#[test]
fn a_pinned_ball_is_stuck_and_named_after_the_nearest_feature() {
    let (mut sim, info) = load("clean");
    // 30 mm above posts[1] (374, 760) mm, clear of it.
    let f = pinned(&mut sim, &info, [0.374, 0.790, BALL_Z], 60, 40);
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].0, FindingKind::StuckBall);
    assert!(f[0].1.starts_with("posts[1] @ (374, 790, "), "{}", f[0].1);
    // Shorter than the stuck time: nothing.
    let (mut sim, info) = load("clean");
    assert!(pinned(&mut sim, &info, [0.374, 0.790, BALL_Z], 30, 40).is_empty());
}

#[test]
fn a_ball_cradled_on_a_held_flipper_is_not_stuck() {
    let (mut sim, info) = load("clean");
    let pivot = sim
        .flipper_set()
        .pivot_m(sim.ids().flipper("lower_left").unwrap())
        .unwrap();
    // 4 cm from the pivot, 80 deg up: within the bat's reach but clear of
    // the bat both at rest (-30 deg) and raised (+20 deg).
    let a = 80f32.to_radians();
    let p = [
        pivot.x + 0.04 * libm::cosf(a),
        pivot.y + 0.04 * libm::sinf(a),
        BALL_Z,
    ];
    sim.set_switch("left_flipper", true).unwrap();
    sim.advance(50);
    let held = pinned(&mut sim, &info, p, 80, 40);
    assert!(held.is_empty(), "{held:?}");
    // Not held: the same rest is a stuck ball.
    let (mut sim, info) = load("clean");
    let free = pinned(&mut sim, &info, p, 80, 40);
    assert_eq!(free.len(), 1, "{free:?}");
    assert_eq!(free[0].0, FindingKind::StuckBall);
}

#[test]
fn a_ball_outside_the_bounds_is_escaped_once() {
    let (mut sim, info) = load("clean");
    let h = sim
        .spawn_ball("standard_steel", [0.3, 0.5, BALL_Z], [0.0; 3])
        .unwrap();
    let mut mon = GameMonitor::new(&info, &sim, 4, 10_000);
    // Below the floor (more than the 5 cm margin).
    assert!(sim.set_ball_state(h, &at_rest([0.3, 0.5, -0.2])));
    mon.observe(&sim);
    assert_eq!(mon.findings().len(), 1);
    let f = &mon.findings()[0];
    assert_eq!(f.kind, FindingKind::EscapedBall);
    assert_eq!(f.game, 4);
    assert!(
        f.subject.contains(" @ (300, 500, -200) mm"),
        "{}",
        f.subject
    );
    // The simulation removes it as escaped; no second finding.
    sim.advance(1);
    mon.observe(&sim);
    assert_eq!(sim.balls_in_play(), 0);
    assert_eq!(sim.counters().balls_escaped, 1);
    assert_eq!(mon.findings().len(), 1);
    let out = mon.finish(&sim);
    assert_eq!(out.ball_times_ms, [1]);
    assert_eq!(out.counters.balls_escaped, 1);
}

#[test]
fn a_ball_the_simulation_removes_as_escaped_is_named() {
    let (mut sim, info) = load("clean");
    let h = sim
        .spawn_ball("standard_steel", [0.01, 0.5, BALL_Z], [0.0; 3])
        .unwrap();
    let mut mon = GameMonitor::new(&info, &sim, 0, 10_000);
    mon.observe(&sim);
    // Outside the left wall but inside the bounds (5 cm margin), flying
    // out at 100 m/s: the simulation removes it within the tick.
    let s = BallState {
        position_m: [-0.04, 0.5, BALL_Z],
        velocity_m_s: [-100.0, 0.0, 0.0],
        ..BallState::default()
    };
    assert!(sim.set_ball_state(h, &s));
    sim.advance(1);
    mon.observe(&sim);
    assert_eq!(sim.counters().balls_escaped, 1);
    let f = mon.findings();
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].kind, FindingKind::EscapedBall);
    // Named at its last observed position, beside the left wall.
    assert!(
        f[0].subject.starts_with("walls[0] @ (10, 500, "),
        "{}",
        f[0].subject
    );
}

#[test]
fn a_drained_ball_is_neither_stuck_nor_escaped() {
    let (mut sim, info) = load("clean");
    let mut mon = GameMonitor::new(&info, &sim, 0, 5);
    sim.spawn_ball("standard_steel", [0.257, 0.04, BALL_Z], [0.0; 3])
        .unwrap();
    for _ in 0..20 {
        sim.advance(1);
        mon.observe(&sim);
    }
    assert_eq!(sim.counters().balls_drained, 1);
    assert!(mon.findings().is_empty(), "{:?}", mon.findings());
    assert!(!mon.all_balls_stuck());
}

#[test]
fn bot_infers_flippers_without_table_data() {
    let (mut sim, info) = load("clean");
    let mut bot = Bot::new(1, SkillLevel::Standard);
    assert!(bot.flipper_models().is_empty());
    bot.tick(&mut sim);
    let m = bot.flipper_models();
    assert_eq!(m.len(), 2);
    let left = m
        .iter()
        .find(|f| f.button == Some(CabinetInput::LeftFlipper))
        .unwrap();
    let right = m
        .iter()
        .find(|f| f.button == Some(CabinetInput::RightFlipper))
        .unwrap();
    assert!(left.sweep_rad > 0.0 && right.sweep_rad < 0.0);
    assert!(left.rest_dir.x > 0.0 && right.rest_dir.x < 0.0);
    // -30 deg rest angle.
    assert!(
        (left.rest_dir.y - (-0.5)).abs() < 1e-3,
        "{:?}",
        left.rest_dir
    );

    // With the table: the declared geometry.
    let mut bot = Bot::new(1, SkillLevel::Standard).with_table(&info);
    bot.tick(&mut sim);
    let m = bot.flipper_models();
    assert!(m.iter().all(|f| (f.length_m - 0.076).abs() < 1e-6));
    assert!(
        m.iter()
            .all(|f| (f.sweep_rad.abs() - 50f32.to_radians()).abs() < 1e-5)
    );
}

#[test]
fn bot_starts_the_game_and_reports_its_settings() {
    let (mut sim, _) = load("clean");
    let mut bot = Bot::new(5, SkillLevel::Novice)
        .with_skill_params(SkillParams {
            reaction_jitter_ms: 0,
            miss_rate: 0.0,
        })
        .with_config(BotConfig {
            cradle: false,
            ..BotConfig::default()
        });
    assert_eq!(bot.seed(), 5);
    assert_eq!(bot.skill(), SkillLevel::Novice);
    assert_eq!(bot.skill_params().miss_rate, 0.0);
    assert!(!bot.config().cradle);
    for _ in 0..1_000 {
        bot.tick(&mut sim);
        sim.advance(1);
    }
    assert!(bot.seen_ball());
    assert_eq!(bot.stats().starts, 1);
    assert_eq!(sim.switch_hits("start"), Some(1));
    // `start` was released again.
    assert_eq!(sim.switch_closed("start"), Some(false));
    for _ in 0..20_000 {
        bot.tick(&mut sim);
        sim.advance(1);
    }
    let s = bot.stats();
    assert_eq!(s.cradles, 0);
    assert_eq!(s.misses, 0);
    assert!(s.flips_on_prediction > 0 && s.flips_on_prediction == s.predictions);
}

#[test]
fn bot_retries_start_when_no_ball_appears() {
    // The trap table's script serves on `start`; with a script that ignores
    // it no ball would appear. Simulate that by never letting a ball stay:
    // remove every ball as soon as it appears, before the bot sees it.
    let (mut sim, _) = load("trap");
    let mut bot = Bot::new(2, SkillLevel::Standard);
    for _ in 0..10_000 {
        bot.tick(&mut sim);
        sim.advance(1);
        for h in sim.ball_handles() {
            sim.remove_ball(h);
        }
    }
    assert!(!bot.seen_ball());
    assert_eq!(
        bot.stats().starts,
        u64::from(BotConfig::default().max_starts)
    );
}

#[test]
fn nudges_are_bounded_and_never_tilt() {
    let run = |nudge: NudgeConfig| {
        let (mut sim, _) = load("trap");
        let mut bot = Bot::new(4, SkillLevel::Standard).with_config(BotConfig {
            nudge,
            ..BotConfig::default()
        });
        for _ in 0..60_000 {
            bot.tick(&mut sim);
            sim.advance(1);
        }
        (bot.stats(), sim.switch_hits("tilt").unwrap())
    };
    let (on, tilts) = run(NudgeConfig::default());
    assert!(on.nudges > 0);
    assert!(on.nudges <= u64::from(NudgeConfig::default().max_per_game));

    assert!(on.launches > 0, "the resting ball is plunged: {on:?}");
    let (off, t_off) = run(NudgeConfig {
        enabled: false,
        ..NudgeConfig::default()
    });
    assert_eq!(off.nudges, 0);
    // The tilt switch closes no more often with nudges than without (the
    // bob closes it once while settling after load, nudged or not).
    assert_eq!(tilts, t_off);
}

#[test]
fn configured_start_delay_applies_to_the_first_press() {
    let (mut sim, _) = load("clean");
    let mut bot = Bot::new(5, SkillLevel::Standard).with_config(BotConfig {
        start_delay_ms: (0, 0),
        ..BotConfig::default()
    });
    for _ in 0..2 {
        bot.tick(&mut sim);
        sim.advance(1);
    }
    assert_eq!(bot.stats().starts, 1, "a zero delay presses start at once");
    // The default delay (20..=200 ms) is still honoured.
    let (mut sim, _) = load("clean");
    let mut bot = Bot::new(5, SkillLevel::Standard).with_config(BotConfig::default());
    for _ in 0..2 {
        bot.tick(&mut sim);
        sim.advance(1);
    }
    assert_eq!(bot.stats().starts, 0);
}

#[test]
fn an_escape_before_any_observation_is_still_reported() {
    let (mut sim, info) = load("clean");
    let mut mon = GameMonitor::new(&info, &sim, 0, 10_000);
    mon.observe(&sim);
    // Spawned beside the wall and flying out: removed within the tick it
    // appears, so the monitor never sees it.
    sim.spawn_ball("standard_steel", [-0.04, 0.5, BALL_Z], [-100.0, 0.0, 0.0])
        .unwrap();
    sim.advance(1);
    mon.observe(&sim);
    assert_eq!(sim.counters().balls_escaped, 1);
    let f = mon.findings();
    assert_eq!(f.len(), 1, "{f:?}");
    assert_eq!(f[0].kind, FindingKind::EscapedBall);
    assert_eq!(f[0].subject, "untracked ball");
    assert!(f[0].detail.contains("never tracked"), "{}", f[0].detail);
}

#[test]
fn idle_end_zero_does_not_end_a_game_with_a_ball_in_play() {
    use pin_sim::autoplay::{GameEnd, run_soak};
    let mut c = cfg(1, 1, 1_500);
    c.idle_end_ticks = 0;
    let r = on_sim_thread(move || run_soak(&fixture("clean"), &c).unwrap());
    let g = &r.game_list[0];
    // The ball is in play for the whole short game: only an empty table
    // may end it as game over.
    assert_ne!(g.end, GameEnd::GameOver, "{g:?}");
}

#[test]
fn a_ball_served_on_the_start_timeout_tick_is_not_no_ball() {
    use pin_sim::autoplay::{GameEnd, run_soak};
    // Whatever tick the serve lands on, a ball in play at the timeout tick
    // must not end the game as NoBall.
    let ends = on_sim_thread(|| {
        (1..=300u64)
            .map(|t| {
                let mut c = cfg(1, 1, t + 5);
                c.start_timeout_ticks = t;
                run_soak(&fixture("clean"), &c).unwrap().game_list[0].end
            })
            .collect::<Vec<_>>()
    });
    assert!(ends.contains(&GameEnd::NoBall), "early timeouts still fire");
    let first_ball = ends.iter().position(|e| *e != GameEnd::NoBall).unwrap();
    assert!(
        ends[first_ball..].iter().all(|e| *e != GameEnd::NoBall),
        "{ends:?}"
    );
}

/// Closes and reopens switch `name` (forced), observing after each tick,
/// with every ball in `pins` held at its position.
fn pulse_pinned(
    sim: &mut Simulation,
    mon: &mut GameMonitor<'_>,
    pins: &[(pin_sim::BallHandle, [f32; 3])],
    name: &str,
) {
    for closed in [true, false] {
        sim.set_switch(name, closed).unwrap();
        for &(h, p) in pins {
            assert!(sim.set_ball_state(h, &at_rest(p)));
        }
        sim.advance(1);
        mon.observe(sim);
    }
    sim.release_switch(name).unwrap();
}

/// Issue #138 acceptance 2 (SPEC §4 Subphase 2.9 detectors, EndlessBall
/// subject): `bumper_ring` has no zone of its own (its mech `bumper` has
/// a position). Ball A sits beside the bumper and hits `bumper_ring` ten
/// times; ball B, the other endless ball, hits `top_rollover` three times.
/// Each ball's EndlessBall finding names the switch that ball hit most:
/// B's is `top_rollover`, never `bumper_ring` (before #138 every
/// positionless hit was credited to every ball, so B was named after A's
/// switch).
#[test]
fn a_positionless_mech_switch_endless_ball_names_the_right_subject() {
    let (mut sim, info) = load("clean");
    assert!(
        info.switches
            .iter()
            .any(|s| s.name == "bumper_ring" && s.position_m.is_none()),
        "the fixture's bumper ring is positionless"
    );
    // A: 50 mm below the bumper (237, 700) mm; B: 52 mm from
    // top_rollover (257, 880) mm and 160 mm from the bumper.
    let pa = [0.237, 0.650, BALL_Z];
    let pb = [0.300, 0.850, BALL_Z];
    let a = sim.spawn_ball("standard_steel", pa, [0.0; 3]).unwrap();
    let b = sim.spawn_ball("standard_steel", pb, [0.0; 3]).unwrap();
    let pins = [(a, pa), (b, pb)];
    let mut mon = GameMonitor::new(&info, &sim, 0, u64::MAX).with_endless_ticks(100);
    mon.observe(&sim);
    for _ in 0..3 {
        pulse_pinned(&mut sim, &mut mon, &pins, "top_rollover");
    }
    for _ in 0..10 {
        pulse_pinned(&mut sim, &mut mon, &pins, "bumper_ring");
    }
    for _ in 0..150 {
        for &(h, p) in &pins {
            assert!(sim.set_ball_state(h, &at_rest(p)));
        }
        sim.advance(1);
        mon.observe(&sim);
    }
    let out = mon.finish(&sim);
    // Totals still count each closing once.
    let hits = |n: &str| {
        let i = info.coverage_switches().position(|s| s.name == n).unwrap();
        out.switch_hits[i]
    };
    assert_eq!((hits("top_rollover"), hits("bumper_ring")), (3, 10));
    let endless = |slot: u8| {
        out.findings
            .iter()
            .find(|f| {
                f.kind == FindingKind::EndlessBall
                    && f.detail.starts_with(&format!("ball in slot {slot} "))
            })
            .unwrap_or_else(|| panic!("no endless finding for slot {slot}: {:?}", out.findings))
    };
    let fb = endless(b.slot());
    assert_eq!(fb.subject, "top_rollover", "{fb:?}");
    assert!(!fb.detail.contains("bumper_ring"), "{}", fb.detail);
    let fa = endless(a.slot());
    assert_eq!(fa.subject, "bumper_ring", "{fa:?}");
}

/// Issue #138 deliverable 1, in-ball credit unchanged (SPEC §4 Subphase
/// 2.9 shot coverage): the `drop_lane` shot (`top_rollover` then
/// `bumper_ring` within 2 s) completed on one ball counts once; the same
/// two switches split across a drain and the next ball do not.
#[test]
fn a_shot_on_a_single_ball_still_counts() {
    let (mut sim, info) = load("clean");
    let i = info
        .shots
        .iter()
        .position(|s| s.name == "drop_lane")
        .unwrap();
    let p = [0.300, 0.850, BALL_Z];
    let h = sim.spawn_ball("standard_steel", p, [0.0; 3]).unwrap();
    let mut mon = GameMonitor::new(&info, &sim, 0, u64::MAX);
    mon.observe(&sim);
    pulse_pinned(&mut sim, &mut mon, &[(h, p)], "top_rollover");
    pulse_pinned(&mut sim, &mut mon, &[(h, p)], "bumper_ring");
    // Half the shot, then the ball drains and the next one finishes it.
    pulse_pinned(&mut sim, &mut mon, &[(h, p)], "top_rollover");
    assert!(sim.remove_ball(h));
    sim.advance(1);
    mon.observe(&sim);
    let h2 = sim.spawn_ball("standard_steel", p, [0.0; 3]).unwrap();
    mon.observe(&sim);
    pulse_pinned(&mut sim, &mut mon, &[(h2, p)], "bumper_ring");
    let out = mon.finish(&sim);
    assert_eq!(out.shot_counts[i], 1, "{:?}", out.shot_counts);
}
