//! Subphase 2.9 requirements 2 and 4: the report's JSON round trip and
//! Markdown, `pin_sim soak` exit codes and argument errors, arbitrary
//! bounded `SoakConfig`s never panicking, and a bounded random-input run of
//! the `soak` argument parser.

#[path = "fixtures/autoplay/support.rs"]
mod support;

use pin_sim::autoplay::report::SOAK_WIDE;
use pin_sim::autoplay::{
    BotConfig, BotStats, CounterTotals, Distribution, Finding, FindingKind, GameEnd, GameSummary,
    ModeCount, NudgeConfig, ShotCount, SkillLevel, SoakConfig, SoakReport, SwitchCount, Timing,
    run_soak,
};
use pin_sim::cli::{CliError, SOAK_JSON, SOAK_MARKDOWN, main_with_args, parse_soak_args};
use proptest::prelude::*;
use serde_json::Value;
use std::process::Command;
use support::*;

fn finding(kind: FindingKind, subject: &str, game: u32, tick: u64, detail: &str) -> Finding {
    Finding {
        kind,
        subject: subject.into(),
        game,
        tick,
        detail: detail.into(),
    }
}

/// A report with every section filled and awkward strings.
fn sample() -> SoakReport {
    SoakReport {
        table_id: "tbl \"q\" | x".into(),
        table_hash: 0x0123_4567_89AB_CDEF,
        games: 2,
        seed: u64::MAX,
        skill: SkillLevel::Expert,
        max_game_ticks: 1234,
        findings: vec![
            finding(
                FindingKind::StuckBall,
                "trap_pocket @ (1, 2, 3) mm",
                0,
                10,
                "slow\nfor ages",
            ),
            finding(
                FindingKind::EscapedBall,
                "walls[0] @ (-50, 9, 0) mm",
                1,
                20,
                "tab\there",
            ),
            finding(FindingKind::NonFinite, "ball slot 3", 1, 21, "NaN"),
            finding(
                FindingKind::ScriptError,
                "frenzy",
                1,
                30,
                "rules.lua:1: \"boom\" \\ é",
            ),
            finding(
                FindingKind::QueueOverflow,
                "script command queue",
                1,
                40,
                "3 entries dropped",
            ),
            finding(
                FindingKind::UnreachableSwitch,
                "hidden|opto",
                SOAK_WIDE,
                0,
                "never",
            ),
            finding(FindingKind::ShotCoverage, "orbit", SOAK_WIDE, 0, "never"),
            finding(FindingKind::ModeCoverage, "wizard", SOAK_WIDE, 0, "never"),
        ],
        switches: vec![
            SwitchCount {
                name: "a".into(),
                hits: 5,
            },
            SwitchCount {
                name: "hidden|opto".into(),
                hits: 0,
            },
        ],
        shots: vec![ShotCount {
            name: "orbit".into(),
            count: 0,
        }],
        modes: vec![ModeCount {
            name: "wizard".into(),
            starts: 0,
            observable: false,
        }],
        ball_time_ms: Distribution::of(&[100, 200, 300]),
        game_score: Distribution::of(&[-5, 7]),
        counters: CounterTotals {
            edges_dropped: 1,
            haptics_dropped: 2,
            inputs_dropped: 3,
            spawns_failed: 4,
            ball_events_dropped: 5,
            balls_drained: 6,
            balls_escaped: 7,
            script_queue_overflows: 8,
            script_faults: 9,
        },
        bot: BotStats {
            predictions: 10,
            flips_on_prediction: 9,
            misses: 1,
            cradles: 2,
            launches: 3,
            nudges: 4,
            starts: 5,
        },
        game_list: vec![
            GameSummary {
                game: 0,
                seed: 11,
                ticks: 100,
                end: GameEnd::Stuck,
                state_hash: 0xFEED,
                score: Some(-5),
                balls: 1,
            },
            GameSummary {
                game: 1,
                seed: 12,
                ticks: 200,
                end: GameEnd::NoBall,
                state_hash: 0xBEEF,
                score: None,
                balls: 2,
            },
        ],
        timing: Timing {
            threads: 3,
            measured: true,
            ticks_measured: 300,
            wall_s: 0.5,
            games_per_s: 4.0,
            ticks_per_s: 600.0,
            tick_p50_us: 1.0,
            tick_p99_us: 2.0,
            tick_p999_us: 400.0,
            tick_max_us: 3.5,
            budget_ms: 0.35,
            budget_checked: true,
            findings: vec![finding(
                FindingKind::TickBudget,
                "tick p99.9",
                SOAK_WIDE,
                0,
                "slow",
            )],
        },
    }
}

fn parse_findings(v: &Value) -> Vec<Finding> {
    v.as_array()
        .unwrap()
        .iter()
        .map(|f| Finding {
            kind: FindingKind::from_name(f["kind"].as_str().unwrap()).unwrap(),
            subject: f["subject"].as_str().unwrap().into(),
            game: f["game"].as_u64().map_or(SOAK_WIDE, |g| g as u32),
            tick: f["tick"].as_u64().unwrap(),
            detail: f["detail"].as_str().unwrap().into(),
        })
        .collect()
}

fn dist(v: &Value) -> Distribution {
    Distribution {
        count: v["count"].as_u64().unwrap(),
        min: v["min"].as_i64().unwrap(),
        median: v["median"].as_i64().unwrap(),
        p90: v["p90"].as_i64().unwrap(),
        p99: v["p99"].as_i64().unwrap(),
        max: v["max"].as_i64().unwrap(),
    }
}

/// Rebuilds the deterministic part of a report from its JSON.
fn from_json(text: &str, timing: Timing) -> SoakReport {
    let v: Value = serde_json::from_str(text).expect("valid JSON");
    assert_eq!(v["schema"], 1);
    let hex = |v: &Value| u64::from_str_radix(v.as_str().unwrap(), 16).unwrap();
    let c = &v["counters"];
    let b = &v["bot"];
    let n = |v: &Value, k: &str| v[k].as_u64().unwrap();
    SoakReport {
        table_id: v["table"]["id"].as_str().unwrap().into(),
        table_hash: hex(&v["table"]["hash"]),
        games: n(&v["config"], "games") as u32,
        seed: n(&v["config"], "seed"),
        skill: SkillLevel::from_name(v["config"]["skill"].as_str().unwrap()).unwrap(),
        max_game_ticks: n(&v["config"], "max_game_ticks"),
        findings: parse_findings(&v["findings"]),
        switches: v["coverage"]["switches"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| SwitchCount {
                name: s["name"].as_str().unwrap().into(),
                hits: n(s, "hits"),
            })
            .collect(),
        shots: v["coverage"]["shots"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| ShotCount {
                name: s["id"].as_str().unwrap().into(),
                count: n(s, "count"),
            })
            .collect(),
        modes: v["coverage"]["modes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|m| ModeCount {
                name: m["id"].as_str().unwrap().into(),
                starts: n(m, "starts"),
                observable: m["observable"].as_bool().unwrap(),
            })
            .collect(),
        ball_time_ms: dist(&v["distributions"]["ball_time_ms"]),
        game_score: dist(&v["distributions"]["game_score"]),
        counters: CounterTotals {
            edges_dropped: n(c, "edges_dropped"),
            haptics_dropped: n(c, "haptics_dropped"),
            inputs_dropped: n(c, "inputs_dropped"),
            spawns_failed: n(c, "spawns_failed"),
            ball_events_dropped: n(c, "ball_events_dropped"),
            balls_drained: n(c, "balls_drained"),
            balls_escaped: n(c, "balls_escaped"),
            script_queue_overflows: n(c, "script_queue_overflows"),
            script_faults: n(c, "script_faults"),
        },
        bot: BotStats {
            predictions: n(b, "predictions"),
            flips_on_prediction: n(b, "flips_on_prediction"),
            misses: n(b, "misses"),
            cradles: n(b, "cradles"),
            launches: n(b, "launches"),
            nudges: n(b, "nudges"),
            starts: n(b, "starts"),
        },
        game_list: v["games"]
            .as_array()
            .unwrap()
            .iter()
            .map(|g| GameSummary {
                game: n(g, "game") as u32,
                seed: n(g, "seed"),
                ticks: n(g, "ticks"),
                end: [
                    GameEnd::GameOver,
                    GameEnd::MaxTicks,
                    GameEnd::Stuck,
                    GameEnd::NoBall,
                ]
                .into_iter()
                .find(|e| g["end"] == e.name())
                .unwrap(),
                state_hash: hex(&g["state_hash"]),
                score: g["score"].as_i64(),
                balls: n(g, "balls") as u32,
            })
            .collect(),
        timing,
    }
}

#[test]
fn json_round_trips_the_whole_report() {
    let r = sample();
    let back = from_json(&r.to_json_deterministic(), r.timing.clone());
    assert_eq!(back, r);
    // The full form parses too and adds only `timing`, last.
    let full: Value = serde_json::from_str(&r.to_json()).unwrap();
    let t = &full["timing"];
    assert_eq!(t["threads"], 3);
    assert_eq!(t["tick_max_us"], 3.5);
    // A non-finite timing value is written as null.
    let mut nan = r.clone();
    nan.timing.wall_s = f64::NAN;
    let v: Value = serde_json::from_str(&nan.to_json()).unwrap();
    assert_eq!(v["timing"]["wall_s"], Value::Null);
    assert_eq!(parse_findings(&t["findings"]), r.timing.findings);
    let keys: Vec<&str> = full
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert!(keys.contains(&"timing"));
    let det: Value = serde_json::from_str(&r.to_json_deterministic()).unwrap();
    assert!(det.get("timing").is_none());
    assert!(
        r.to_json().ends_with("}\n")
            && r.to_json().rfind("\"timing\"") > r.to_json().rfind("\"games\"")
    );
    // Summary fields agree with the methods.
    assert_eq!(full["summary"]["passed"], false);
    assert_eq!(full["summary"]["coverage_complete"], false);
    assert_eq!(full["summary"]["findings_by_kind"]["StuckBall"], 1);
    assert_eq!(
        full["coverage"]["never_hit"],
        serde_json::json!(["hidden|opto"])
    );
}

#[test]
fn json_key_order_is_stable() {
    let j = sample().to_json();
    let order = [
        "\"schema\"",
        "\"table\"",
        "\"config\"",
        "\"summary\"",
        "\"findings\"",
        "\"coverage\"",
        "\"distributions\"",
        "\"counters\"",
        "\"bot\"",
        "\"games\"",
        "\"timing\"",
    ];
    // Top-level keys sit at two spaces of indent.
    let pos: Vec<usize> = order
        .iter()
        .map(|k| j.find(&format!("\n  {k}:")).unwrap())
        .collect();
    assert!(pos.windows(2).all(|w| w[0] < w[1]), "{pos:?}");
    assert!(j.starts_with("{\n  \"schema\": 1,"));
}

#[test]
fn markdown_names_every_finding_and_section() {
    let r = sample();
    let md = r.to_markdown();
    for f in r.findings.iter().chain(&r.timing.findings) {
        let cell = f.subject.replace('|', "\\|");
        assert!(md.contains(&cell), "missing {cell}");
    }
    for h in [
        "## Summary",
        "## Findings",
        "### StuckBall (1)",
        "### ModeCoverage (1)",
        "## Coverage",
        "## Distributions",
        "## Counters",
        "## Games",
        "## Timing",
        "**FAIL**",
        "Never-hit switches: hidden|opto",
        "| ball time (ms) | 3 | 100 | 200 | 300 | 300 | 300 |",
    ] {
        assert!(md.contains(h), "missing {h}");
    }
    let det = r.to_markdown_deterministic();
    assert!(!det.contains("## Timing") && md.starts_with(&det));
    // An empty report says so.
    let mut empty = sample();
    empty.findings.clear();
    empty.timing.findings.clear();
    empty.switches.truncate(1);
    empty.shots.clear();
    empty.modes.clear();
    assert!(empty.passed());
    let md = empty.to_markdown();
    assert!(
        md.contains("None.") && md.contains("**PASS**") && md.contains("Never-hit switches: none")
    );
}

#[test]
fn policy_follows_the_finding_kinds() {
    let mut r = sample();
    r.findings.clear();
    r.timing.findings.clear();
    r.switches.truncate(1);
    assert!(r.passed());
    // TickBudget lives in the timing section and never fails the soak.
    r.timing.findings.push(finding(
        FindingKind::TickBudget,
        "tick p99.9",
        SOAK_WIDE,
        0,
        "x",
    ));
    assert!(r.passed());
    assert_eq!(r.findings_of(FindingKind::TickBudget).count(), 1);
    for k in FindingKind::ALL {
        if k == FindingKind::TickBudget {
            continue;
        }
        let mut x = r.clone();
        x.findings.push(finding(k, "s", 0, 0, ""));
        assert!(!x.passed(), "{k:?} must fail the soak");
        assert_eq!(x.coverage_complete(), !k.is_coverage());
    }
}

fn args(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| (*s).to_owned()).collect()
}

#[test]
fn soak_arguments_parse_and_errors_are_usage() {
    let a = parse_soak_args(&args(&[
        "b",
        "--games",
        "7",
        "--seed",
        "0x10",
        "--skill",
        "EXPERT",
        "--threads",
        "3",
        "--max-ticks",
        "99",
        "--no-tick-budget",
        "--out",
        "o",
    ]))
    .unwrap();
    assert_eq!(a.bundle.to_str(), Some("b"));
    assert_eq!(a.out.to_str(), Some("o"));
    assert_eq!(
        (
            a.cfg.games,
            a.cfg.seed,
            a.cfg.skill,
            a.cfg.threads,
            a.cfg.max_game_ticks
        ),
        (7, 16, SkillLevel::Expert, 3, 99)
    );
    assert!(!a.cfg.check_tick_budget);
    let d = parse_soak_args(&args(&["b", "--out", "o"])).unwrap();
    assert_eq!(d.cfg, SoakConfig::default());

    for bad in [
        &["--out", "o"][..],
        &["b"],
        &["b", "c", "--out", "o"],
        &["b", "--out", "o", "--games", "0"],
        &["b", "--out", "o", "--games", "x"],
        &["b", "--out", "o", "--games", "99999999999"],
        &["b", "--out", "o", "--threads", "0"],
        &["b", "--out", "o", "--threads", "5000"],
        &["b", "--out", "o", "--skill", "pro"],
        &["b", "--out", "o", "--max-ticks", "0"],
        &["b", "--out", "o", "--seed"],
        &["b", "--out", "o", "--frobnicate"],
    ] {
        let e = parse_soak_args(&args(bad)).unwrap_err();
        assert_eq!(e.exit_code(), 2, "{bad:?}");
        assert!(e.to_string().contains("pin_sim soak"), "{e}");
        let mut full = vec!["soak"];
        full.extend_from_slice(bad);
        assert_eq!(main_with_args(&args(&full)).unwrap_err().exit_code(), 2);
    }
}

#[test]
fn soak_command_failures() {
    // Missing bundle: the soak cannot run (exit 1).
    let out = scratch("missing");
    let e = main_with_args(&args(&[
        "soak",
        "/no/such/bundle",
        "--out",
        out.to_str().unwrap(),
    ]))
    .unwrap_err();
    assert!(matches!(e, CliError::Failed(_)), "{e:?}");
    // Reports inside the bundle are refused (exit 2).
    let inside = fixture("gap").join("reports");
    let e = main_with_args(&args(&[
        "soak",
        fixture("gap").to_str().unwrap(),
        "--out",
        inside.to_str().unwrap(),
    ]))
    .unwrap_err();
    assert_eq!(e.exit_code(), 2, "{e}");
    assert!(!inside.exists());
    // An --out that is a file cannot be created (exit 1).
    let file = scratch("file").join("f");
    std::fs::write(&file, b"x").unwrap();
    let e = main_with_args(&args(&[
        "soak",
        fixture("gap").to_str().unwrap(),
        "--games",
        "1",
        "--max-ticks",
        "10",
        "--out",
        file.to_str().unwrap(),
    ]))
    .unwrap_err();
    assert!(
        matches!(e, CliError::Failed(ref m) if m.contains("cannot create")),
        "{e:?}"
    );
}

#[test]
fn binary_exit_codes() {
    let bin = env!("CARGO_BIN_EXE_pin_sim");
    let out = scratch("bin");
    let run = |bundle: &str, extra: &[&str]| {
        let mut c = Command::new(bin);
        c.arg("soak")
            .arg(fixture(bundle))
            .args(["--games", "1", "--threads", "1", "--out"])
            .arg(&out)
            .args(extra);
        c.output().unwrap()
    };
    // A short clean run: within 300 ticks the ball reaches no flipper or
    // trough, so coverage is incomplete (exit 1)...
    let o = run("clean", &["--max-ticks", "300", "--no-tick-budget"]);
    assert_eq!(o.status.code(), Some(1), "{o:?}");
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("UnreachableSwitch left_eos"), "{err}");
    assert!(out.join(SOAK_JSON).exists() && out.join(SOAK_MARKDOWN).exists());
    // ...a full one passes (exit 0) and prints the summary on stdout.
    let o = run(
        "clean",
        &[
            "--games",
            "6",
            "--threads",
            "2",
            "--max-ticks",
            "60000",
            "--seed",
            "1",
        ],
    );
    let text = String::from_utf8_lossy(&o.stdout);
    assert_eq!(o.status.code(), Some(0), "{o:?}");
    assert!(text.contains("PASS") && text.contains("games/s"), "{text}");
    // The gap fixture fails with the escape named.
    let o = run("gap", &["--max-ticks", "20000"]);
    assert_eq!(o.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&o.stderr).contains("EscapedBall walls[0] @ "));
    // Bad arguments.
    let o = run("gap", &["--skill", "wizard"]);
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("usage: pin_sim soak"));
}

#[test]
fn tick_budget_is_a_timing_finding_only() {
    let bundle = fixture("gap");
    let r = on_sim_thread(move || {
        let mut c = cfg(1, 1, 3_000);
        c.check_tick_budget = true;
        // An impossible budget: every measured tick exceeds it.
        c.tick_budget_ms = 0.0;
        run_soak(&bundle, &c).unwrap()
    });
    assert!(r.timing.budget_checked && r.timing.measured);
    assert_eq!(r.timing.ticks_measured, r.game_list[0].ticks);
    assert_eq!(r.timing.findings.len(), 1);
    assert_eq!(r.timing.findings[0].kind, FindingKind::TickBudget);
    assert!(r.findings.iter().all(|f| f.kind != FindingKind::TickBudget));
    // Not measured: nothing checked.
    let bundle = fixture("gap");
    let r = on_sim_thread(move || {
        let mut c = cfg(1, 1, 3_000);
        c.measure_timing = false;
        c.check_tick_budget = true;
        c.tick_budget_ms = 0.0;
        run_soak(&bundle, &c).unwrap()
    });
    assert!(!r.timing.budget_checked && r.timing.findings.is_empty());
    assert_eq!(r.timing.ticks_measured, 0);
}

fn arb_skill() -> impl Strategy<Value = SkillLevel> {
    prop_oneof![
        Just(SkillLevel::Novice),
        Just(SkillLevel::Standard),
        Just(SkillLevel::Expert)
    ]
}

prop_compose! {
    fn arb_bot()(
        lead in 0u32..200,
        hold in (0u32..300, 0u32..300),
        cooldown in 0u32..300,
        cradle in any::<bool>(),
        cradle_speed in 0.0f32..2.0,
        cradle_hold in (0u32..3000, 0u32..3000),
        post in 0u32..3000,
        rest in 0.0f32..1.0,
        launch_after in 0u32..2000,
        launch_hold in (0u32..1000, 0u32..1000),
        launch_interval in 0u32..3000,
        start_delay in (0u32..500, 0u32..500),
        retry in 0u32..5000,
        starts in 0u32..5,
        nudge in (any::<bool>(), 0u32..5000, 0u32..3000, 0u32..30, 0.0f32..=1.0, 0.0f32..2.0),
    ) -> BotConfig {
        BotConfig {
            lead_ms: lead,
            flip_hold_ms: hold,
            cooldown_ms: cooldown,
            cradle,
            cradle_speed_m_s: cradle_speed,
            cradle_hold_ms: cradle_hold,
            post_cradle_ms: post,
            rest_speed_m_s: rest,
            launch_after_ms: launch_after,
            launch_hold_ms: launch_hold,
            launch_interval_ms: launch_interval,
            start_delay_ms: start_delay,
            start_retry_ms: retry,
            max_starts: starts,
            nudge: NudgeConfig {
                enabled: nudge.0,
                after_rest_ms: nudge.1,
                min_interval_ms: nudge.2,
                max_per_game: nudge.3,
                strength: nudge.4,
                max_bob_fraction: nudge.5,
            },
        }
    }
}

prop_compose! {
    fn arb_cfg()(
        games in 1u32..3,
        seed in any::<u64>(),
        skill in arb_skill(),
        threads in 1usize..5,
        max_game_ticks in 0u64..1500,
        idle in 0u64..2000,
        start_timeout in 0u64..2000,
        stuck in 0u64..1500,
        measure in any::<bool>(),
        check in any::<bool>(),
        budget in 0.0f64..1.0,
        bot in arb_bot(),
    ) -> SoakConfig {
        SoakConfig {
            games,
            seed,
            skill,
            threads,
            max_game_ticks,
            idle_end_ticks: idle,
            start_timeout_ticks: start_timeout,
            stuck_ticks: stuck,
            measure_timing: measure,
            check_tick_budget: check,
            tick_budget_ms: budget,
            bot,
            ..SoakConfig::default()
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, ..ProptestConfig::default() })]

    /// Arbitrary bounded configurations never panic, play exactly the
    /// requested games within the tick limit, and write valid JSON.
    #[test]
    fn arbitrary_soak_configs_never_panic(c in arb_cfg(), fixture_name in prop_oneof![Just("gap"), Just("trap"), Just("clean")]) {
        let bundle = fixture(fixture_name);
        let r = on_sim_thread(move || run_soak(&bundle, &c)).unwrap();
        prop_assert_eq!(r.game_list.len(), c.games as usize);
        for g in &r.game_list {
            prop_assert!(g.ticks <= c.max_game_ticks);
        }
        let v: Value = serde_json::from_str(&r.to_json()).unwrap();
        prop_assert_eq!(&v["schema"], 1);
        let _ = r.to_markdown();
    }

    /// The `soak` argument parser survives arbitrary tokens.
    #[test]
    fn soak_parser_survives_random_tokens(tokens in prop::collection::vec(
        prop_oneof![
            Just("--games".to_owned()), Just("--seed".to_owned()), Just("--skill".to_owned()),
            Just("--threads".to_owned()), Just("--max-ticks".to_owned()), Just("--out".to_owned()),
            Just("--no-tick-budget".to_owned()), Just("0x".to_owned()), Just("-1".to_owned()),
            "[0-9]{0,22}", ".{0,12}",
        ], 0..12)) {
        if let Err(e) = parse_soak_args(&tokens) {
            prop_assert_eq!(e.exit_code(), 2);
        }
    }

    /// ... and arbitrary bytes (lossily decoded, split on NUL).
    #[test]
    fn soak_parser_survives_random_bytes(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
        let text = String::from_utf8_lossy(&bytes);
        let tokens: Vec<String> = text.split('\0').map(str::to_owned).collect();
        let _ = parse_soak_args(&tokens);
        let mut with_cmd = vec!["soak".to_owned()];
        with_cmd.extend(tokens);
        if let Ok(a) = parse_soak_args(&with_cmd[1..]) {
            prop_assert!(a.cfg.games > 0 && a.cfg.threads > 0);
        }
    }
}

/// Copies fixture `name` into a scratch folder.
fn copy_fixture(name: &str, tag: &str) -> std::path::PathBuf {
    let dst = scratch(tag).join(name);
    let src = fixture(name);
    std::fs::create_dir_all(&dst).unwrap();
    for e in std::fs::read_dir(&src).unwrap() {
        let e = e.unwrap();
        if e.file_type().unwrap().is_file() {
            std::fs::copy(e.path(), dst.join(e.file_name())).unwrap();
        }
    }
    dst
}

#[test]
fn bundles_that_do_not_load_are_errors() {
    // A rules script that does not compile: the table data loads, every
    // game's simulation does not.
    let b = copy_fixture("gap", "bad_script");
    std::fs::write(b.join("rules.lua"), "this is not lua (").unwrap();
    let mut c = cfg(3, 1, 100);
    c.threads = 2;
    let e = on_sim_thread(move || run_soak(&b, &c)).unwrap_err();
    assert!(matches!(e, pin_sim::SimError::Script(_)), "{e:?}");
    // A layout that does not parse.
    let b = copy_fixture("gap", "bad_layout");
    std::fs::write(b.join("layout.toml"), "schema_version = [").unwrap();
    let e = on_sim_thread(move || run_soak(&b, &cfg(1, 1, 100))).unwrap_err();
    assert!(e.to_string().contains("autoplay table info"), "{e}");
    // A bundle that does not validate.
    let b = copy_fixture("gap", "bad_table");
    std::fs::write(b.join("table.toml"), "schema_version = 1\n").unwrap();
    let e = on_sim_thread(move || run_soak(&b, &cfg(1, 1, 100))).unwrap_err();
    assert!(matches!(e, pin_sim::SimError::Asset(_)), "{e:?}");
}
