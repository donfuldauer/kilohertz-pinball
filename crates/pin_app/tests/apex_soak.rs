//! 6.1 T5: a bounded in-process autoplay soak of the Apex Circuit through
//! the `pin_sim` autoplay API (seed 1, standard skill, 15 games, every game
//! capped at 10 simulated minutes): no stuck, endless, escaped or
//! non-finite ball, no script error or queue overflow, every declared
//! switch hit, every shot made and both modes started (seen through the
//! `mode:<id>` persist keys). Tick timing is not judged here (debug and
//! shared machines); the release soak in the 6.1 report covers it.

mod apex_support;

use apex_support::{apex_bundle, on_big_stack};
use pin_sim::autoplay::{FindingKind, SkillLevel, SoakConfig, run_soak};

#[test]
fn bounded_autoplay_soak_covers_the_table() {
    let dir = apex_bundle().to_path_buf();
    let report = on_big_stack(move || {
        let cfg = SoakConfig {
            games: 15,
            seed: 1,
            skill: SkillLevel::Standard,
            threads: 2,
            max_game_ticks: 600_000,
            measure_timing: false,
            check_tick_budget: false,
            ..SoakConfig::default()
        };
        run_soak(&dir, &cfg).expect("soak runs")
    });
    for f in &report.findings {
        eprintln!("finding: {:?} {} {}", f.kind, f.subject, f.detail);
    }
    let count = |k: FindingKind| report.findings.iter().filter(|f| f.kind == k).count();
    for k in [
        FindingKind::StuckBall,
        FindingKind::EndlessBall,
        FindingKind::EscapedBall,
        FindingKind::NonFinite,
        FindingKind::ScriptError,
        FindingKind::QueueOverflow,
        FindingKind::UnreachableSwitch,
        FindingKind::ShotCoverage,
        FindingKind::ModeCoverage,
    ] {
        assert_eq!(count(k), 0, "{k:?}");
    }
    assert_eq!(report.table_id, "apex_circuit");
    assert_eq!(
        report.switches.len(),
        30,
        "every declared non-cabinet switch"
    );
    for s in &report.switches {
        assert!(s.hits > 0, "switch {} never hit", s.name);
    }
    let shots: Vec<&str> = report.shots.iter().map(|s| s.name.as_str()).collect();
    assert_eq!(
        shots,
        [
            "left_orbit",
            "right_orbit",
            "cross_ramp",
            "drop_bank",
            "lock_scoop"
        ]
    );
    for s in &report.shots {
        assert!(s.count > 0, "shot {} never made", s.name);
    }
    for m in &report.modes {
        assert!(m.observable, "mode {} has a mode: persist key", m.name);
        assert!(m.starts > 0, "mode {} never started", m.name);
    }
    // The Powerball is ball 2 of every game and its plunge passes the
    // detector: Powerball Mania starts once per game.
    let pm = report
        .modes
        .iter()
        .find(|m| m.name == "powerball_mania")
        .unwrap();
    assert_eq!(pm.starts, 15);
    assert_eq!(report.counters.script_faults, 0);
    assert_eq!(report.counters.script_queue_overflows, 0);
    assert_eq!(report.counters.balls_escaped, 0);
    assert_eq!(report.game_list.len(), 15);
}
