//! 6.2 T6: bounded in-process autoplay soaks of the Sky Deck
//! (`pin_sim::autoplay::run_soak`, the 2.9 harness Gate A2 uses).
//!
//! * `soak_60_games_per_skill`: at every skill level, zero findings of any
//!   kind (stuck, endless, escaped, non-finite, coverage, script, queue),
//!   every declared switch closed, every declared shot made, the Night
//!   Overdrive mode started, and the median ball time inside the 2.9
//!   `--gate a1` band (15..120 s).
//! * `r6c_ball_times_200_games_per_skill` (ignored: about 5 minutes
//!   in the test profile; run with `--ignored`): the 6.2 brief's R6c band
//!   on the release smoke's sample size (200 games per skill, seed 1): the
//!   standard median ball time in 25..100 s, every skill's in 20..120 s, and
//!   no finding (no ball over 300 s).
//!
//! The full 1,000-game Gate A2 is the Orchestrator's (`pin_sim soak`).

mod sky_deck_support;

use pin_sim::autoplay::{SkillLevel, SoakConfig, SoakReport, run_soak};
use sky_deck_support::sky_deck_bundle;

fn soak(skill: SkillLevel, games: u32) -> SoakReport {
    let cfg = SoakConfig {
        games,
        seed: 1,
        skill,
        threads: 2,
        // Test builds are slower than the release gate runs; wall-clock
        // tick budgets are the release soak's (`pin_sim soak`) to judge.
        measure_timing: false,
        check_tick_budget: false,
        ..SoakConfig::default()
    };
    run_soak(sky_deck_bundle(), &cfg).unwrap_or_else(|e| panic!("{e}"))
}

/// Median ball time, s.
fn median_s(r: &SoakReport) -> f64 {
    r.ball_time_ms.median as f64 / 1000.0
}

fn check(skill: SkillLevel, games: u32) -> SoakReport {
    let r = soak(skill, games);
    let name = skill.name();
    assert_eq!(r.games, games);
    assert!(r.findings.is_empty(), "{name}: findings {:#?}", r.findings);
    let missing: Vec<&str> = r
        .switches
        .iter()
        .filter(|s| s.hits == 0)
        .map(|s| s.name.as_str())
        .collect();
    assert!(
        missing.is_empty(),
        "{name}: switches never closed: {missing:?}"
    );
    for shot in ["loop", "ramp", "deck"] {
        let c = r.shots.iter().find(|s| s.name == shot).expect(shot);
        assert!(c.count > 0, "{name}: shot {shot} never made");
    }
    let m = r
        .modes
        .iter()
        .find(|m| m.name == "night_overdrive")
        .expect("mode");
    assert!(m.observable, "{name}: mode not observable");
    assert!(m.starts > 0, "{name}: Night Overdrive never started");
    assert_eq!(r.counters.script_queue_overflows, 0, "{name}");
    assert_eq!(r.counters.script_faults, 0, "{name}");
    println!(
        "{name}: {games} games, median ball {:.1} s (max {:.1} s), median score {}, mode starts {}",
        median_s(&r),
        r.ball_time_ms.max as f64 / 1000.0,
        r.game_score.median,
        m.starts
    );
    r
}

#[test]
fn soak_60_games_per_skill() {
    let mut scores = Vec::new();
    for skill in [SkillLevel::Novice, SkillLevel::Standard, SkillLevel::Expert] {
        let r = check(skill, 60);
        let m = median_s(&r);
        assert!(
            (15.0..=120.0).contains(&m),
            "{}: median ball time {m} s",
            skill.name()
        );
        scores.push(r.game_score.median);
    }
    // Score order by skill is reported, not asserted: the 2.9 bot's flip
    // timing (fix-2.9-bot-timing) decides it, not the table (6.2 brief,
    // KNOWN BOT ISSUE).
    println!("median scores novice / standard / expert: {scores:?}");
}

#[test]
#[ignore = "200 games per skill: about 5 minutes in the test profile; run with --ignored"]
fn r6c_ball_times_200_games_per_skill() {
    for skill in [SkillLevel::Novice, SkillLevel::Standard, SkillLevel::Expert] {
        let r = check(skill, 200);
        let m = median_s(&r);
        let band = if skill == SkillLevel::Standard {
            25.0..=100.0
        } else {
            20.0..=120.0
        };
        assert!(
            band.contains(&m),
            "{}: median ball time {m} s",
            skill.name()
        );
    }
}
