//! Gate A1 soak verdict on crafted reports: the ball-time window, the tick
//! budget (strict "under 0.35 ms") and each failing finding kind.

use pin_sim::autoplay::{
    BotStats, CheckStatus, CounterTotals, Distribution, Finding, FindingKind, GateA1Verdict,
    SkillLevel, SoakReport, SwitchCount, Timing,
};

fn good(skill: SkillLevel) -> SoakReport {
    SoakReport {
        table_id: "t".into(),
        table_hash: 1,
        games: 1000,
        seed: 1,
        skill,
        max_game_ticks: 1000,
        findings: vec![],
        switches: vec![SwitchCount {
            name: "a".into(),
            hits: 3,
        }],
        shots: vec![],
        modes: vec![],
        ball_time_ms: Distribution::of(&[30_000, 30_000, 30_000]),
        ball_time_censored: 0,
        game_score: Distribution::default(),
        counters: CounterTotals::default(),
        bot: BotStats::default(),
        game_list: vec![],
        timing: Timing {
            threads: 1,
            measured: true,
            ticks_measured: 1000,
            tick_p999_us: 100.0,
            budget_ms: 0.35,
            budget_checked: true,
            ..Timing::default()
        },
    }
}

fn with_median(skill: SkillLevel, ms: i64) -> SoakReport {
    let mut r = good(skill);
    r.ball_time_ms = Distribution::of(&[ms]);
    r
}

fn status(v: &GateA1Verdict, name: &str) -> CheckStatus {
    v.check(name)
        .unwrap_or_else(|| panic!("no check {name}"))
        .status
}

fn finding(kind: FindingKind) -> Finding {
    Finding {
        kind,
        subject: "x".into(),
        game: 0,
        tick: 1,
        detail: "d".into(),
    }
}

#[test]
fn good_report_passes_every_check() {
    let r = good(SkillLevel::Standard);
    let v = r.gate_a1(SkillLevel::Standard);
    assert!(v.passed(), "{}", v.lines());
    assert!(r.passed());
    assert_eq!(status(&v, "ball_time_samples"), CheckStatus::Reported);
    assert_eq!(v.check("ball_time_samples").unwrap().value, "3");
}

#[test]
fn median_window_is_inclusive_at_standard() {
    for (ms, ok) in [
        (14_999, false),
        (15_000, true),
        (120_000, true),
        (120_001, false),
    ] {
        let v = with_median(SkillLevel::Standard, ms).gate_a1(SkillLevel::Standard);
        let want = if ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        };
        assert_eq!(status(&v, "median_ball_time_15_120s"), want, "{ms}");
        assert_eq!(v.passed(), ok, "{ms}");
    }
}

#[test]
fn median_is_only_reported_at_other_skills() {
    for skill in [SkillLevel::Novice, SkillLevel::Expert] {
        for ms in [5_000, 500_000] {
            let v = with_median(skill, ms).gate_a1(skill);
            assert_eq!(
                status(&v, "median_ball_time_15_120s"),
                CheckStatus::Reported
            );
            assert!(
                v.check("median_ball_time_15_120s")
                    .unwrap()
                    .value
                    .contains(&ms.to_string())
            );
            assert!(v.passed(), "{}", v.lines());
        }
    }
}

#[test]
fn zero_ball_time_samples_fail_at_every_skill() {
    for skill in [SkillLevel::Novice, SkillLevel::Standard, SkillLevel::Expert] {
        let mut r = good(skill);
        r.ball_time_ms = Distribution::default();
        let v = r.gate_a1(skill);
        assert_eq!(status(&v, "median_ball_time_15_120s"), CheckStatus::Fail);
        assert!(!v.passed());
    }
}

#[test]
fn p999_tick_is_strictly_under_350_us() {
    for (us, ok) in [(349.999, true), (350.0, false), (350.001, false)] {
        let mut r = good(SkillLevel::Standard);
        r.timing.tick_p999_us = us;
        let v = r.gate_a1(SkillLevel::Standard);
        let want = if ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        };
        assert_eq!(status(&v, "p999_tick_under_0_35ms"), want, "{us}");
    }
}

#[test]
fn missing_timing_fails_never_passes_silently() {
    let mut a = good(SkillLevel::Standard);
    a.timing = Timing::default();
    let mut b = good(SkillLevel::Standard);
    b.timing.budget_checked = false; // --no-tick-budget
    let mut c = good(SkillLevel::Standard);
    c.timing.measured = false;
    for r in [a, b, c] {
        let v = r.gate_a1(SkillLevel::Standard);
        let c = v.check("p999_tick_under_0_35ms").unwrap();
        assert_eq!(c.status, CheckStatus::Fail);
        assert_eq!(c.value, "tick timing not measured");
        assert!(!v.passed());
    }
}

#[test]
fn each_failing_finding_fails_its_own_check() {
    for (kind, name) in [
        (FindingKind::StuckBall, "no_stuck"),
        (FindingKind::EndlessBall, "no_endless_balls"),
        (FindingKind::EscapedBall, "no_escaped"),
        (FindingKind::NonFinite, "no_nonfinite"),
        (FindingKind::UnreachableSwitch, "switch_coverage_100"),
        (FindingKind::ShotCoverage, "shot_mode_coverage"),
        (FindingKind::ModeCoverage, "shot_mode_coverage"),
    ] {
        let mut r = good(SkillLevel::Standard);
        r.findings.push(finding(kind));
        let v = r.gate_a1(SkillLevel::Standard);
        assert_eq!(status(&v, name), CheckStatus::Fail, "{kind:?}");
        assert!(!v.passed());
        for c in &v.checks {
            if c.name != name {
                assert_ne!(
                    c.status,
                    CheckStatus::Fail,
                    "{kind:?} also failed {}",
                    c.name
                );
            }
        }
    }
}

#[test]
fn a_never_hit_switch_fails_coverage() {
    let mut r = good(SkillLevel::Standard);
    r.switches[0].hits = 0;
    let v = r.gate_a1(SkillLevel::Standard);
    assert_eq!(status(&v, "switch_coverage_100"), CheckStatus::Fail);
}

#[test]
fn lines_format_and_passed_semantics_unchanged() {
    let r = with_median(SkillLevel::Novice, 5_000);
    let before = (r.passed(), r.to_json_deterministic());
    let v = r.gate_a1(SkillLevel::Novice);
    assert_eq!(before, (r.passed(), r.to_json_deterministic()));
    let text = v.lines();
    assert!(
        text.contains("REPORTED  median_ball_time_15_120s  5000 ms (n=1)"),
        "{text}"
    );
    assert!(text.contains("PASS  no_stuck  0 findings"), "{text}");
    // TickBudget timing findings alone still do not fail the soak.
    let mut t = good(SkillLevel::Standard);
    t.timing.findings.push(finding(FindingKind::TickBudget));
    assert!(t.passed());
}

#[test]
fn an_endless_ball_fails_no_endless_balls_at_every_skill() {
    for skill in [SkillLevel::Novice, SkillLevel::Standard, SkillLevel::Expert] {
        let clean = good(skill).gate_a1(skill);
        let c = clean
            .check("no_endless_balls")
            .expect("no_endless_balls row");
        assert_eq!(c.status, CheckStatus::Pass);
        assert_eq!(c.value, "0 findings");
        assert!(clean.passed(), "{}", clean.lines());

        let mut r = good(skill);
        r.findings.push(finding(FindingKind::EndlessBall));
        r.findings.push(finding(FindingKind::EndlessBall));
        let v = r.gate_a1(skill);
        let c = v.check("no_endless_balls").unwrap();
        assert_eq!(c.status, CheckStatus::Fail);
        assert_eq!(c.value, "2 findings");
        assert!(!v.passed());
        assert!(
            v.lines().contains("FAIL  no_endless_balls  2 findings"),
            "{}",
            v.lines()
        );
        // EndlessBall is a failing kind of the soak itself too.
        assert!(!r.passed());
    }
}

#[test]
fn censored_ball_times_are_reported_not_judged() {
    let mut r = good(SkillLevel::Standard);
    r.ball_time_censored = 2;
    let v = r.gate_a1(SkillLevel::Standard);
    let c = v.check("ball_time_censored").unwrap();
    assert_eq!(c.status, CheckStatus::Reported);
    assert!(c.value.starts_with("2 of 3 samples"), "{}", c.value);
    assert!(v.passed());
}

#[test]
fn game_count_must_reach_1000() {
    for (games, ok) in [(0u32, false), (999, false), (1000, true), (1001, true)] {
        let mut r = good(SkillLevel::Standard);
        r.games = games;
        let v = r.gate_a1(SkillLevel::Standard);
        let want = if ok {
            CheckStatus::Pass
        } else {
            CheckStatus::Fail
        };
        assert_eq!(status(&v, "games_1000"), want, "{games}");
        assert_eq!(
            v.check("games_1000").unwrap().value,
            format!("{games} games")
        );
        assert_eq!(v.passed(), ok, "{games}");
    }
}
