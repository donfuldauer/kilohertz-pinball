//! Subphase 6.1 spec tests (Independent Tester), S5: autoplay soakability
//! for Gate A2 (brief item 7).
//!
//! The in-process 2.9 soak (`pin_sim::autoplay::run_soak`) plays a bounded
//! number of seeded games on the built Apex Circuit bundle at each of the
//! three skill levels with one fixed seed and a bounded `max_game_ticks`.
//! Per skill: zero stuck, endless, escaped or non-finite findings, zero
//! script errors and zero command-queue overflows (the wall-clock
//! TickBudget finding is excluded, D8: timing is not measured). Across the
//! three soaks (decision log 2026-11-18-6.1-tester "6.1-T-3 — coverage
//! over the bounded soak"): every declared switch hit, every declared shot
//! made and every declared mode started at least once.

#[path = "fixtures/spec_6_1/common.rs"]
mod common;

use common::*;
use pin_sim::autoplay::{FindingKind, SkillLevel, SoakConfig, SoakReport, run_soak};
use std::collections::BTreeMap;

/// Games per skill (the brief's bound is 6).
const GAMES: u32 = 6;
/// The fixed soak seed.
const SEED: u64 = 0x6_1A9E;
/// Simulated time limit per game: 15 minutes.
const MAX_GAME_TICKS: u64 = 900_000;

fn soak(skill: SkillLevel) -> SoakReport {
    let cfg = SoakConfig {
        games: GAMES,
        seed: SEED,
        skill,
        threads: 2,
        max_game_ticks: MAX_GAME_TICKS,
        measure_timing: false,
        check_tick_budget: false,
        ..SoakConfig::default()
    };
    let b = bundle().to_path_buf();
    big_stack(move || run_soak(&b, &cfg).unwrap_or_else(|e| panic!("run_soak: {e}")))
}

fn describe(r: &SoakReport) -> String {
    let mut s = String::new();
    for f in &r.findings {
        s.push_str(&format!(
            "\n  {} {} game {} tick {}: {}",
            f.kind.name(),
            f.subject,
            f.game,
            f.tick,
            f.detail
        ));
    }
    s
}

#[test]
fn bounded_soak_is_clean_and_covers_the_table() {
    let mut switches: BTreeMap<String, u64> = BTreeMap::new();
    let mut shots: BTreeMap<String, u64> = BTreeMap::new();
    let mut modes: BTreeMap<String, u64> = BTreeMap::new();
    let mut failures = Vec::new();
    for skill in SkillLevel::ALL {
        let r = soak(skill);
        println!(
            "{skill:?}: {} games, scores {:?}, ball time {:?}, findings:{}",
            r.games,
            r.game_score,
            r.ball_time_ms,
            describe(&r)
        );
        assert_eq!(r.games, GAMES, "{skill:?}: games played");
        assert_eq!(r.table_id, TABLE_ID);
        for f in &r.findings {
            let hard = matches!(
                f.kind,
                FindingKind::StuckBall
                    | FindingKind::EndlessBall
                    | FindingKind::EscapedBall
                    | FindingKind::NonFinite
                    | FindingKind::ScriptError
                    | FindingKind::QueueOverflow
            );
            if hard {
                failures.push(format!(
                    "{skill:?}: {} {} game {} tick {}: {}",
                    f.kind.name(),
                    f.subject,
                    f.game,
                    f.tick,
                    f.detail
                ));
            }
        }
        assert_eq!(
            r.counters.script_faults,
            0,
            "{skill:?}: script faults{}",
            describe(&r)
        );
        assert_eq!(
            r.counters.script_queue_overflows,
            0,
            "{skill:?}: queue overflows{}",
            describe(&r)
        );
        assert_eq!(r.counters.balls_escaped, 0, "{skill:?}: escaped balls");
        assert!(
            r.game_list.iter().all(|g| g.score.unwrap_or(0) > 0),
            "{skill:?}: every game scored: {:?}",
            r.game_list.iter().map(|g| g.score).collect::<Vec<_>>()
        );
        for s in &r.switches {
            *switches.entry(s.name.clone()).or_default() += s.hits;
        }
        for s in &r.shots {
            *shots.entry(s.name.clone()).or_default() += s.count;
        }
        for m in &r.modes {
            assert!(m.observable, "{skill:?}: mode {} observable", m.name);
            *modes.entry(m.name.clone()).or_default() += m.starts;
        }
    }
    assert!(
        failures.is_empty(),
        "soak findings:\n{}",
        failures.join("\n")
    );

    // Coverage over the bounded soak.
    let t = table();
    for s in &t.manifest.switches {
        assert!(
            switches.contains_key(&s.id),
            "switch {} is in the coverage report",
            s.id
        );
    }
    let never: Vec<&String> = switches
        .iter()
        .filter(|(_, h)| **h == 0)
        .map(|(n, _)| n)
        .collect();
    assert!(never.is_empty(), "switches never hit: {never:?}");
    for s in &t.manifest.shots {
        assert!(
            shots.get(&s.id).copied().unwrap_or(0) > 0,
            "shot {} never made: {shots:?}",
            s.id
        );
    }
    for m in &t.manifest.modes {
        assert!(
            modes.get(&m.id).copied().unwrap_or(0) > 0,
            "mode {} never started: {modes:?}",
            m.id
        );
    }
}
