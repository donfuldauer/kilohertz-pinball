//! Subphase 6.4 spec test (Independent Tester), brief S6: the Feature Lab
//! table under pin_sim's autoplay bot, 6 games per skill level, soak seed 1.
//!
//! Each game runs the soak loop (`bot.tick`, `sim.advance(1)`,
//! `monitor.observe`) on simulated time with the soak's own game and bot
//! seeds (`game_seed(1, g)`, `bot_seed`) and a per-game tick cap. The test
//! asserts: no stuck, endless, escaped, non-finite, script-error or
//! queue-overflow finding; the bot never pressed `action` yet `mode_sel`
//! advanced at every ball start; and replaying a game with the same seeds
//! gives the same final score. Coverage is not asserted at this size.

#[path = "fixtures/spec_6_4/common.rs"]
mod common;

use common::*;
use pin_sim::SimConfig;
use pin_sim::autoplay::report::FindingKind;
use pin_sim::autoplay::{Bot, GameMonitor, SkillLevel, TableInfo, bot_seed, game_seed};

/// Soak seed (brief S6).
const SEED: u64 = 1;
/// Games per skill level.
const GAMES: u32 = 6;
/// Per-game tick cap (simulated ms).
const MAX_GAME_TICKS: u64 = 120_000;
/// As the soak's defaults.
const IDLE_END_TICKS: u64 = 5_000;
const START_TIMEOUT_TICKS: u64 = 10_000;
const STUCK_TICKS: u64 = 10_000;
const ENDLESS_TICKS: u64 = 300_000;

/// Kinds that fail this test (brief S6).
const FAILING: [FindingKind; 6] = [
    FindingKind::StuckBall,
    FindingKind::EndlessBall,
    FindingKind::EscapedBall,
    FindingKind::NonFinite,
    FindingKind::ScriptError,
    FindingKind::QueueOverflow,
];

/// What one game produced.
#[derive(Debug)]
struct Game {
    score: Option<i64>,
    ticks: u64,
    /// `mode_sel` after load (no game yet).
    initial_sel: i64,
    /// (ball number, mode_sel 300 ms into the ball).
    ball_starts: Vec<(i64, i64)>,
    action_hits: u32,
    findings: Vec<String>,
    state_hash: u64,
}

fn play(info: &TableInfo, skill: SkillLevel, game: u32) -> Game {
    let seed = game_seed(SEED, game);
    let mut sim = load_with(SimConfig {
        seed,
        ..SimConfig::default()
    });
    let mut bot = Bot::new(bot_seed(seed), skill).with_table(info);
    let mut mon = GameMonitor::new(info, &sim, game, STUCK_TICKS).with_endless_ticks(ENDLESS_TICKS);
    mon.observe(&sim);
    let mut idle = 0u64;
    let mut ball = pv(&sim, "ball");
    let initial_sel = pv(&sim, "mode_sel");
    let mut pending: Option<(i64, u64)> = None;
    let mut ball_starts = Vec::new();
    while sim.tick() < MAX_GAME_TICKS {
        bot.tick(&mut sim);
        sim.advance(1);
        mon.observe(&sim);
        let b = pv(&sim, "ball");
        if b != ball {
            if b != 0 {
                pending = Some((b, sim.tick()));
            }
            ball = b;
        }
        if let Some((n, t0)) = pending
            && sim.tick() >= t0 + 300
        {
            ball_starts.push((n, pv(&sim, "mode_sel")));
            pending = None;
        }
        if sim.balls_in_play() == 0 {
            idle += 1;
        } else {
            idle = 0;
        }
        if bot.seen_ball() && sim.balls_in_play() == 0 && idle >= IDLE_END_TICKS {
            break;
        }
        if !bot.seen_ball() && sim.balls_in_play() == 0 && sim.tick() >= START_TIMEOUT_TICKS {
            break;
        }
        if mon.all_balls_stuck() {
            break;
        }
    }
    let action_hits = hits(&sim, "action");
    let state_hash = sim.state_hash();
    let score = sim.score(0);
    let ticks = sim.tick();
    let out = mon.finish(&sim);
    let findings = out
        .findings
        .iter()
        .filter(|f| FAILING.contains(&f.kind))
        .map(|f| format!("{} {} @{}: {}", f.kind.name(), f.subject, f.tick, f.detail))
        .collect();
    Game {
        score,
        ticks,
        initial_sel,
        ball_starts,
        action_hits,
        findings,
        state_hash,
    }
}

fn skill_run(skill: SkillLevel) {
    big_stack(move || {
        let info = TableInfo::load(bundle()).expect("table info");
        let games: Vec<Game> = (0..GAMES).map(|g| play(&info, skill, g)).collect();
        let mut transitions = 0;
        for (g, r) in games.iter().enumerate() {
            eprintln!(
                "{skill:?} game {g}: score {:?}, {} ticks, ball starts {:?}",
                r.score, r.ticks, r.ball_starts
            );
            assert!(
                r.findings.is_empty(),
                "{skill:?} game {g}: findings {:#?}",
                r.findings
            );
            assert_eq!(
                r.action_hits, 0,
                "{skill:?} game {g}: the bot pressed action"
            );
            assert!(
                !r.ball_starts.is_empty(),
                "{skill:?} game {g}: at least one ball was played"
            );
            // Without action, mode_sel changes only at ball starts, by +1.
            let mut prev = r.initial_sel;
            for &(n, now) in &r.ball_starts {
                assert_eq!(
                    now,
                    (prev + 1) % 5,
                    "{skill:?} game {g} ball {n}: mode_sel {prev} -> {now} without action ({:?})",
                    r.ball_starts
                );
                prev = now;
                transitions += 1;
            }
        }
        // mode_sel moved across balls.
        let distinct: std::collections::BTreeSet<i64> = games
            .iter()
            .flat_map(|r| r.ball_starts.iter().map(|x| x.1))
            .collect();
        assert!(
            transitions >= 2 && distinct.len() >= 2,
            "{skill:?}: mode_sel moved across balls ({transitions} ball starts, values {distinct:?})"
        );
        // Same seeds, same scores (two games replayed).
        for g in [0, GAMES - 1] {
            let again = play(&info, skill, g);
            assert_eq!(
                again.score, games[g as usize].score,
                "{skill:?} game {g}: score reproducible"
            );
            assert_eq!(again.state_hash, games[g as usize].state_hash);
        }
    });
}

#[test]
fn autoplay_novice() {
    skill_run(SkillLevel::Novice);
}

#[test]
fn autoplay_standard() {
    skill_run(SkillLevel::Standard);
}

#[test]
fn autoplay_expert() {
    skill_run(SkillLevel::Expert);
}
