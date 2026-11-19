//! Subphase 6.2 spec tests (Independent Tester): Gate-A2 readiness of the
//! "Sky Deck" table (brief C6a autoplay soak, C6c determinism).
//!
//! The soak uses the engine's own autoplay harness (`pin_sim::autoplay`),
//! seed 1, 2 worker threads, on simulated time only (tick timing is not
//! measured: no wall-clock assertion, PLAN §2.3 rule 5). The bot never
//! presses `action`, so a Night Overdrive start in the soak proves the
//! bot-reachable start path (the next `vuk_sw` closure once armed).
//!
//! * `c6a_soak_*` (ignored in the default run, see its doc): 40 games at
//!   each skill level.
//! * `c6a_smoke_soak`: a 3-game standard soak that runs every time, with
//!   the soundness checks only.
//! * `c6c_*`: replay determinism (sampled bot games and a recorded scripted
//!   input stream).

#[path = "fixtures/spec_6_2/common.rs"]
mod common;

use common::{big_stack, bundle, persist_index};
use pin_core::input::{CabinetInput, InputEvent};
use pin_core::sim::TICK_NS;
use pin_sim::autoplay::{
    FindingKind, GameEnd, SkillLevel, SoakConfig, SoakReport, TableInfo, play_soak_game, run_soak,
};
use pin_sim::replay::Recorder;
use pin_sim::{SimConfig, Simulation};

/// Games per skill level of the full readiness soak.
const GAMES: u32 = 40;

fn cfg(skill: SkillLevel, games: u32) -> SoakConfig {
    SoakConfig {
        games,
        seed: 1,
        skill,
        threads: 2,
        measure_timing: false,
        check_tick_budget: false,
        ..SoakConfig::default()
    }
}

/// The soundness finding kinds the brief requires to be absent.
const SOUNDNESS: [FindingKind; 6] = [
    FindingKind::StuckBall,
    FindingKind::EndlessBall,
    FindingKind::EscapedBall,
    FindingKind::NonFinite,
    FindingKind::ScriptError,
    FindingKind::QueueOverflow,
];

fn assert_sound(r: &SoakReport, what: &str) {
    let bad: Vec<_> = r
        .findings
        .iter()
        .filter(|f| SOUNDNESS.contains(&f.kind))
        .collect();
    assert!(bad.is_empty(), "{what}: soundness findings {bad:#?}");
    assert_eq!(r.counters.balls_escaped, 0, "{what}: escaped balls");
    assert_eq!(r.counters.script_faults, 0, "{what}: script faults");
    assert_eq!(
        r.counters.script_queue_overflows, 0,
        "{what}: queue overflows"
    );
}

fn summary(r: &SoakReport) -> String {
    let ends: Vec<&str> = r.game_list.iter().map(|g| g.end.name()).collect();
    let mut end_counts: Vec<(&str, usize)> = Vec::new();
    for e in ends {
        match end_counts.iter_mut().find(|(n, _)| *n == e) {
            Some((_, c)) => *c += 1,
            None => end_counts.push((e, 1)),
        }
    }
    format!(
        "{:?}: {} games, findings {}, ball time ms median {} (min {} max {}, censored {}), \
         game score median {} (min {} max {}), ends {:?}, modes {:?}, shots {:?}, \
         switches unhit {:?}",
        r.skill,
        r.games,
        r.findings.len(),
        r.ball_time_ms.median,
        r.ball_time_ms.min,
        r.ball_time_ms.max,
        r.ball_time_censored,
        r.game_score.median,
        r.game_score.min,
        r.game_score.max,
        end_counts,
        r.modes
            .iter()
            .map(|m| (m.name.as_str(), m.starts, m.observable))
            .collect::<Vec<_>>(),
        r.shots
            .iter()
            .map(|s| (s.name.as_str(), s.count))
            .collect::<Vec<_>>(),
        r.switches
            .iter()
            .filter(|s| s.hits == 0)
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
    )
}

/// C6a: the full readiness soak, 40 games at each of novice, standard and
/// expert (seed 1, 2 threads). Ignored in the default run: at the 20-120 s
/// ball times it asserts, the 120 games are 6-36 hours of simulated play,
/// minutes of CPU on the shared 4-core box (the brief allows `#[ignore]`
/// with the reason; it is run with `--ignored --nocapture` and its numbers
/// reported. `--nocapture` (or `--show-output`) is required: libtest hides a
/// passing test's stdout, and without it the `REPORT:` line with the median
/// scores by skill is dropped).
///
/// Asserts, per skill: no stuck / escaped / non-finite / endless ball, no
/// script error or queue overflow. Across the 120 games (decision log
/// 6.2-tester "coverage over the union of the three skill soaks"): 100 %
/// of `TableInfo::coverage_switches()` hit, every declared shot made, the
/// `night_overdrive` mode started (observed through its persist key).
/// Standard median ball time in 20-120 s. The median game scores by skill
/// (novice < standard < expert) are report-only here, printed on a
/// `REPORT:` line: at n=40 games per skill a correct bot passes a strict
/// ascending check only about 40 % of the time. The binding check is Gate
/// A2 A2.3b at 1,000 games per skill (PLAN.md: "scores increase with skill
/// level"); decision log 6.2-a2-score-order.
#[test]
#[ignore = "120-game soak: minutes of CPU; run with --ignored --nocapture"]
fn c6a_soak_readiness_40_games_per_skill() {
    let info = TableInfo::load(bundle()).expect("TableInfo");
    let mut reports = Vec::new();
    for skill in SkillLevel::ALL {
        let r = run_soak(bundle(), &cfg(skill, GAMES)).expect("soak runs");
        println!("{}", summary(&r));
        assert_sound(&r, &format!("{skill:?}"));
        reports.push(r);
    }
    // Coverage over the union.
    let cov: Vec<&str> = info.coverage_switches().map(|s| s.name.as_str()).collect();
    for name in &cov {
        let total: u64 = reports
            .iter()
            .flat_map(|r| r.switches.iter())
            .filter(|s| s.name == *name)
            .map(|s| s.hits)
            .sum();
        assert!(
            total > 0,
            "coverage switch {name} never hit in {} games",
            3 * GAMES
        );
    }
    for shot in &info.shots {
        let total: u64 = reports
            .iter()
            .flat_map(|r| r.shots.iter())
            .filter(|s| s.name == shot.name)
            .map(|s| s.count)
            .sum();
        assert!(total > 0, "shot {} never made", shot.name);
    }
    let mode = |r: &SoakReport| {
        r.modes
            .iter()
            .find(|m| m.name == "night_overdrive")
            .map(|m| (m.starts, m.observable))
            .expect("night_overdrive in the report")
    };
    assert!(
        reports.iter().all(|r| mode(r).1),
        "the mode start is observable"
    );
    let starts: u64 = reports.iter().map(|r| mode(r).0).sum();
    assert!(starts >= 1, "night_overdrive never started by the bot");
    let standard = &reports[1];
    let m = standard.ball_time_ms.median;
    assert!(
        (20_000..=120_000).contains(&m),
        "standard median ball time {m} ms outside 20-120 s"
    );
    // C6c: 5 sampled games of the standard soak replay to the same hash,
    // twice. All replay checks are collected and asserted together; score
    // order by skill is printed as a REPORT line, not asserted
    // (decision 6.2-a2-score-order). Each failure is reported.
    let mut replay_failures = Vec::new();
    for g in [0u32, 9, 17, 28, 39] {
        let expected = standard.game_list[g as usize].state_hash;
        for run in 0..2 {
            let (s, _) =
                play_soak_game(bundle(), &cfg(SkillLevel::Standard, GAMES), g).expect("replays");
            if s.state_hash != expected {
                replay_failures.push(format!(
                    "game {g} run {run}: state hash {:#x} != {expected:#x}",
                    s.state_hash
                ));
            }
        }
    }
    let med: Vec<i64> = reports.iter().map(|r| r.game_score.median).collect();
    let ascending = med[0] < med[1] && med[1] < med[2];
    println!(
        "REPORT: replay checks: {} failure(s) {replay_failures:?}; median scores by skill {med:?} \
         (ascending: {ascending})",
        replay_failures.len()
    );
    assert!(
        replay_failures.is_empty(),
        "state hash differs: {replay_failures:?} (score order ascending: {ascending}, {med:?})"
    );
}

/// C6a (always run): a 3-game standard soak is sound, and every game
/// starts (a ball is served).
#[test]
fn c6a_smoke_soak() {
    let r = run_soak(bundle(), &cfg(SkillLevel::Standard, 3)).expect("soak runs");
    println!("{}", summary(&r));
    assert_sound(&r, "smoke");
    assert_eq!(r.game_list.len(), 3);
    for g in &r.game_list {
        assert_ne!(
            g.end,
            GameEnd::NoBall,
            "game {} never served a ball",
            g.game
        );
        assert!(g.balls >= 3, "game {}: {} balls", g.game, g.balls);
    }
    let mode = r
        .modes
        .iter()
        .find(|m| m.name == "night_overdrive")
        .expect("mode in the report");
    assert!(mode.observable, "mode start observable");
    let info = TableInfo::load(bundle()).expect("TableInfo");
    assert_eq!(
        info.modes[0].persist_key,
        Some(persist_index("mode:night_overdrive"))
    );
}

/// C6c: 5 sampled bot games (the first 5 game indices of a 5-game soak)
/// replay twice to the soak's own `state_hash`.
#[test]
fn c6c_sampled_bot_games_replay_identically() {
    let c = cfg(SkillLevel::Standard, 5);
    let r = run_soak(bundle(), &c).expect("soak runs");
    for g in 0..5u32 {
        let expected = r.game_list[g as usize].state_hash;
        for run in 0..2 {
            let (s, _) = play_soak_game(bundle(), &c, g).expect("plays");
            assert_eq!(s.state_hash, expected, "game {g} run {run}");
            assert_eq!(
                s.ticks, r.game_list[g as usize].ticks,
                "game {g} run {run} length"
            );
        }
    }
}

/// C6c: a recorded scripted game (start, launch, flipper presses on all
/// three flippers, action presses) replays to the same `state_hash` 10
/// times (`pin_sim::replay`), and twice through `Recorder::play` on fresh
/// loads.
#[test]
fn c6c_recorded_inputs_replay_identically_10_times() {
    big_stack(|| {
        let sim = Simulation::load(bundle(), SimConfig::default()).expect("loads");
        let mut rec = Recorder::new(sim);
        let ticks = 20_000u64;
        let mut schedule: Vec<(u64, CabinetInput, bool)> = vec![
            (100, CabinetInput::Start, true),
            (180, CabinetInput::Start, false),
            (1_500, CabinetInput::Launch, true),
            (1_600, CabinetInput::Launch, false),
            (4_000, CabinetInput::Action, true),
            (4_060, CabinetInput::Action, false),
            (15_000, CabinetInput::Action, true),
            (15_040, CabinetInput::Action, false),
        ];
        let mut k = 2_000;
        let mut i = 0;
        while k < ticks - 500 {
            let input = [
                CabinetInput::LeftFlipper,
                CabinetInput::RightFlipper,
                CabinetInput::RightUpper,
            ][i % 3];
            schedule.push((k, input, true));
            schedule.push((k + 110, input, false));
            k += 353;
            i += 1;
        }
        schedule.sort_by_key(|e| (e.0, e.1.index(), e.2));
        let mut next = 0;
        let mut evs: Vec<InputEvent> = Vec::new();
        for tick in 1..=ticks {
            evs.clear();
            while next < schedule.len() && schedule[next].0 == tick {
                let (_, input, p) = schedule[next];
                evs.push(InputEvent::button(tick * TICK_NS - TICK_NS / 2, input, p));
                next += 1;
            }
            rec.step(&evs);
        }
        let (sim, recording) = rec.finish();
        assert!(sim.switch_hits("start").expect("start") >= 1);
        assert!(sim.switch_hits("right_upper").expect("right_upper") >= 1);
        assert!(sim.switch_hits("action").expect("action") >= 1);
        assert!(!sim.script_faulted(), "{:?}", sim.script_error());
        assert!(
            sim.switch_hits("shooter_lane").expect("shooter_lane") >= 1,
            "the scripted game launched a ball"
        );
        let expected = sim.state_hash();
        for run in 0..10 {
            let got = pin_sim::replay::replay(bundle(), &recording).expect("replays");
            assert_eq!(got, expected, "run {run}: replay diverged");
        }
        for run in 0..2 {
            let mut s = Simulation::load(bundle(), SimConfig::default()).expect("loads");
            assert_eq!(
                Recorder::play(&mut s, &recording),
                expected,
                "play run {run}"
            );
        }
    });
}
