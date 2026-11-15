//! Subphase 2.9 verification 1 and 2: every planted defect is caught and
//! named, in the report (struct, JSON, Markdown) and in the `pin_sim soak`
//! output; the clean fixture passes with zero findings and full coverage.

#[path = "fixtures/autoplay/support.rs"]
mod support;

use pin_sim::autoplay::{FindingKind, GameEnd, SkillLevel, SoakReport, run_soak};
use pin_sim::cli::{CliError, SOAK_JSON, SOAK_MARKDOWN, main_with_args};
use support::*;

/// The feature name of a positional subject (`<feature> @ (x, y, z) mm`).
fn feature(subject: &str) -> &str {
    subject.split(" @ ").next().unwrap_or(subject)
}

fn subjects(r: &SoakReport, kind: FindingKind) -> Vec<String> {
    r.findings_of(kind).map(|f| f.subject.clone()).collect()
}

/// The finding kinds of `r` (consecutive repeats merged) without the EndlessBall findings
/// that work item A1-endless-detector requires for a game stopped at the
/// `cap` with a ball in play, after checking that every EndlessBall is
/// exactly such a finding: at the cap tick, of a game that ended there.
fn kinds_besides_cap_endless(r: &SoakReport, cap: u64) -> Vec<FindingKind> {
    for f in r.findings_of(FindingKind::EndlessBall) {
        assert_eq!(f.tick, cap, "{f:?}");
        assert!(f.detail.contains("reached max_game_ticks"), "{f:?}");
        let g = &r.game_list[f.game as usize];
        assert!(g.end == GameEnd::MaxTicks && g.ticks == cap, "{g:?}");
    }
    let mut k: Vec<FindingKind> = r
        .findings
        .iter()
        .map(|f| f.kind)
        .filter(|k| *k != FindingKind::EndlessBall)
        .collect();
    k.dedup();
    k
}

/// Runs `pin_sim soak` in-process on `name`; returns the result and the
/// two report files.
fn cli(name: &str, games: u32, max_ticks: u64) -> (Result<String, CliError>, String, String) {
    cli_at(name, games, max_ticks, "standard")
}

/// [`cli`] at bot skill `skill`.
fn cli_at(
    name: &str,
    games: u32,
    max_ticks: u64,
    skill: &str,
) -> (Result<String, CliError>, String, String) {
    let out = scratch(&format!("cli_{name}"));
    let args: Vec<String> = [
        "soak",
        fixture(name).to_str().unwrap(),
        "--games",
        &games.to_string(),
        "--seed",
        "1",
        "--skill",
        skill,
        "--threads",
        "2",
        "--max-ticks",
        &max_ticks.to_string(),
        "--no-tick-budget",
        "--out",
        out.to_str().unwrap(),
    ]
    .iter()
    .map(|s| (*s).to_owned())
    .collect();
    let r = main_with_args(&args);
    let json = std::fs::read_to_string(out.join(SOAK_JSON)).expect("json report written");
    let md = std::fs::read_to_string(out.join(SOAK_MARKDOWN)).expect("markdown report written");
    (r, json, md)
}

fn json_subjects(json: &str, kind: &str) -> Vec<String> {
    let v: serde_json::Value = serde_json::from_str(json).expect("valid JSON");
    v["findings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|f| f["kind"] == kind)
        .map(|f| f["subject"].as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn trap_is_a_stuck_ball_named_after_the_pocket() {
    let r = on_sim_thread(|| run_soak(&fixture("trap"), &cfg(3, 1, 30_000)).unwrap());
    let stuck = subjects(&r, FindingKind::StuckBall);
    assert_eq!(stuck.len(), 3, "one stuck ball per game: {stuck:?}");
    for s in &stuck {
        assert_eq!(feature(s), "trap_pocket", "{s}");
        assert!(s.ends_with(" mm"), "{s}");
    }
    assert!(r.findings_of(FindingKind::EscapedBall).next().is_none());
    assert!(r.findings_of(FindingKind::ScriptError).next().is_none());
    assert!(!r.passed());
    // The game ends once its only ball is stuck: 10 s of rest, never the
    // tick limit.
    for g in &r.game_list {
        assert_eq!(g.end.name(), "stuck");
        assert!(g.ticks < 30_000);
    }

    let (res, json, md) = cli("trap", 2, 30_000);
    let Err(CliError::Failed(text)) = res else {
        panic!("trap must fail the soak: {res:?}");
    };
    assert!(text.contains("StuckBall trap_pocket @ "), "{text}");
    let json_stuck = json_subjects(&json, "StuckBall");
    assert_eq!(
        json_stuck.len(),
        2,
        "one stuck ball per game: {json_stuck:?}"
    );
    assert!(json_stuck.iter().all(|s| feature(s) == "trap_pocket"));
    assert!(
        md.contains("### StuckBall") && md.contains("trap_pocket @ "),
        "{md}"
    );
}

#[test]
fn wall_gap_is_an_escaped_ball_named_after_the_wall() {
    let r = on_sim_thread(|| run_soak(&fixture("gap"), &cfg(3, 1, 20_000)).unwrap());
    let escaped = subjects(&r, FindingKind::EscapedBall);
    assert_eq!(escaped.len(), 3, "{escaped:?}");
    for s in &escaped {
        assert_eq!(feature(s), "walls[0]", "{s}");
    }
    assert_eq!(r.counters.balls_escaped, 3);
    assert!(r.findings_of(FindingKind::StuckBall).next().is_none());
    assert!(!r.passed());

    let (res, json, md) = cli("gap", 2, 20_000);
    let Err(CliError::Failed(text)) = res else {
        panic!("gap must fail the soak: {res:?}");
    };
    assert!(text.contains("EscapedBall walls[0] @ "), "{text}");
    assert_eq!(json_subjects(&json, "EscapedBall").len(), 2);
    assert!(
        md.contains("### EscapedBall") && md.contains("walls[0] @ "),
        "{md}"
    );
}

#[test]
fn hidden_switch_is_the_only_unreachable_one() {
    let r = on_sim_thread(|| run_soak(&fixture("unreachable"), &cfg(8, 1, 60_000)).unwrap());
    assert_eq!(
        subjects(&r, FindingKind::UnreachableSwitch),
        ["hidden_opto"]
    );
    assert_eq!(r.never_hit(), ["hidden_opto"]);
    assert_eq!(
        kinds_besides_cap_endless(&r, 60_000),
        [FindingKind::UnreachableSwitch],
        "{:?}",
        r.findings
    );
    assert!(!r.coverage_complete() && !r.passed());
    for s in &r.switches {
        assert_eq!(s.hits == 0, s.name == "hidden_opto", "{s:?}");
    }

    let (res, json, md) = cli("unreachable", 8, 60_000);
    let Err(CliError::Failed(text)) = res else {
        panic!("an unreachable switch must fail the soak: {res:?}");
    };
    assert!(text.contains("UnreachableSwitch hidden_opto"), "{text}");
    assert!(text.contains("never-hit switches: hidden_opto"), "{text}");
    assert_eq!(json_subjects(&json, "UnreachableSwitch"), ["hidden_opto"]);
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(
        v["coverage"]["never_hit"],
        serde_json::json!(["hidden_opto"])
    );
    assert!(md.contains("Never-hit switches: hidden_opto"), "{md}");
}

#[test]
fn script_error_names_the_mode_and_the_unstarted_mode() {
    let r = on_sim_thread(|| run_soak(&fixture("script_error"), &cfg(8, 1, 60_000)).unwrap());
    let errors = subjects(&r, FindingKind::ScriptError);
    assert!(!errors.is_empty());
    assert!(errors.iter().all(|s| s == "frenzy"), "{errors:?}");
    for f in r.findings_of(FindingKind::ScriptError) {
        assert!(
            f.detail.contains("jackpot lamp table missing"),
            "{}",
            f.detail
        );
        assert!(
            f.detail.contains("last mode started: frenzy"),
            "{}",
            f.detail
        );
    }
    assert_eq!(subjects(&r, FindingKind::ModeCoverage), ["wizard"]);
    assert_eq!(
        kinds_besides_cap_endless(&r, 60_000)
            .into_iter()
            .filter(|k| *k != FindingKind::ScriptError)
            .collect::<Vec<_>>(),
        [FindingKind::ModeCoverage],
        "{:?}",
        r.findings
    );
    let frenzy = r.modes.iter().find(|m| m.name == "frenzy").unwrap();
    assert!(frenzy.starts > 0 && frenzy.observable);
    assert_eq!(r.counters.script_faults, errors.len() as u64);

    let (res, json, md) = cli("script_error", 8, 60_000);
    let Err(CliError::Failed(text)) = res else {
        panic!("a script error must fail the soak: {res:?}");
    };
    assert!(text.contains("ScriptError frenzy ("), "{text}");
    assert!(text.contains("ModeCoverage wizard"), "{text}");
    let json_errors = json_subjects(&json, "ScriptError");
    assert!(!json_errors.is_empty(), "the JSON report lists the faults");
    assert!(json_errors.iter().all(|s| s == "frenzy"));
    assert_eq!(json_subjects(&json, "ModeCoverage"), ["wizard"]);
    assert!(
        md.contains("### ScriptError") && md.contains("| frenzy |"),
        "{md}"
    );
    assert!(
        md.contains("### ModeCoverage") && md.contains("| wizard |"),
        "{md}"
    );
}

/// The clean fixture plays to game over and passes with zero findings.
///
/// Work item A1-endless-detector: a game stopped at `max_game_ticks` with a
/// ball in play is now an EndlessBall finding, and a Standard bot on this
/// easy table keeps a ball up for more than the 300 s endless limit, so the
/// soak runs at Novice skill with the default 600 000-tick cap, where every
/// game ends by game over (asserted) and the zero-findings check is exact.
#[test]
fn clean_fixture_passes_with_full_coverage() {
    let mut c = cfg(6, 1, 600_000);
    c.skill = SkillLevel::Novice;
    c.threads = 3;
    let r = on_sim_thread(move || run_soak(&fixture("clean"), &c).unwrap());
    assert!(r.findings.is_empty(), "{:?}", r.findings);
    assert!(
        r.game_list.iter().all(|g| g.end == GameEnd::GameOver),
        "{:?}",
        r.game_list
    );
    assert_eq!(r.ball_time_censored, 0);
    assert!(r.passed() && r.coverage_complete());
    assert!(r.never_hit().is_empty());
    assert!(
        !r.shots.is_empty() && r.shots.iter().all(|s| s.count > 0),
        "{:?}",
        r.shots
    );
    assert!(
        !r.modes.is_empty() && r.modes.iter().all(|m| m.starts > 0),
        "{:?}",
        r.modes
    );
    assert!(r.bot.flips_on_prediction > 0 && r.bot.starts == 6);
    assert_eq!(r.game_list.len(), 6);
    assert_eq!(r.game_score.count, 6);

    let (res, json, md) = cli_at("clean", 6, 600_000, "novice");
    let text = res.expect("the clean fixture passes");
    assert!(
        text.contains("0 findings, coverage complete -> PASS"),
        "{text}"
    );
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(v["summary"]["passed"], true);
    assert_eq!(v["findings"], serde_json::json!([]));
    assert!(md.contains("**PASS**"), "{md}");
}

#[test]
fn queue_overflow_is_named_after_the_queue() {
    let r = on_sim_thread(|| run_soak(&fixture("overflow"), &cfg(2, 1, 20_000)).unwrap());
    let q: Vec<_> = r.findings_of(FindingKind::QueueOverflow).collect();
    assert_eq!(q.len(), 2, "one per game: {q:?}");
    for f in &q {
        assert_eq!(f.subject, "script command queue");
        // Seven scores in the start callback (`add(0)` and six more), four
        // fit.
        assert_eq!(f.detail, "3 entries dropped");
    }
    assert_eq!(r.counters.script_queue_overflows, 6);
    assert!(!r.passed());

    let (res, json, md) = cli("overflow", 1, 20_000);
    let Err(CliError::Failed(text)) = res else {
        panic!("a queue overflow must fail the soak: {res:?}");
    };
    assert!(
        text.contains("QueueOverflow script command queue"),
        "{text}"
    );
    assert_eq!(
        json_subjects(&json, "QueueOverflow"),
        ["script command queue"]
    );
    assert!(md.contains("### QueueOverflow") && md.contains("| script command queue |"));
}
