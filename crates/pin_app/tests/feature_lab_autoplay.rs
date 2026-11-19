//! 6.4 R12: a small autoplay run on the Feature Lab through pin_sim's
//! public autoplay API (`play_soak_game`, the soak's own game loop, seeds
//! and monitor): a few games per skill level with no stuck, endless,
//! escaped or non-finite ball, no script error and no queue overflow.
//! Coverage is not asserted at this size (Gate A2 runs 1,000 games).

mod feature_lab_support;

use feature_lab_support::*;
use pin_sim::autoplay::{FindingKind, SkillLevel, SoakConfig, play_soak_game};

/// Games per skill level.
const GAMES: u32 = 6;

const FAILING: [FindingKind; 6] = [
    FindingKind::StuckBall,
    FindingKind::EndlessBall,
    FindingKind::EscapedBall,
    FindingKind::NonFinite,
    FindingKind::ScriptError,
    FindingKind::QueueOverflow,
];

#[test]
fn autoplay_games_have_no_failing_finding() {
    let dir = bundle().to_path_buf();
    on_big_stack(move || {
        for skill in SkillLevel::ALL {
            let cfg = SoakConfig {
                games: GAMES,
                seed: 1,
                skill,
                threads: 1,
                measure_timing: false,
                check_tick_budget: false,
                ..SoakConfig::default()
            };
            for g in 0..GAMES {
                let (summary, outcome) = play_soak_game(&dir, &cfg, g).unwrap();
                let bad: Vec<_> = outcome
                    .findings
                    .iter()
                    .filter(|f| FAILING.contains(&f.kind))
                    .collect();
                assert!(bad.is_empty(), "{} game {g}: {bad:#?}", skill.name());
                assert!(summary.balls >= 3, "{} game {g}: {summary:?}", skill.name());
                println!(
                    "{} game {g}: {} ms, score {:?}, modes {:?}",
                    skill.name(),
                    summary.ticks,
                    summary.score,
                    outcome.mode_starts
                );
            }
        }
    });
}
