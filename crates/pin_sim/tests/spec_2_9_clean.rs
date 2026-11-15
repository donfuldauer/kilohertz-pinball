//! Subphase 2.9 spec tests (Independent Tester), T2: the clean fixture passes
//! with zero findings; the `pin_sim soak` command line exits 0 clean, 1 for
//! every planted defect and 2 for bad arguments.
//!
//!   pin_sim soak <bundle> --games N --seed S --skill novice|standard|expert
//!                         --threads T --out <dir>
//!   -> <dir>/soak_report.json, <dir>/soak_report.md

#[path = "fixtures/spec_2_9/support.rs"]
mod support;

#[path = "fixtures/spec_2_9/soak_support.rs"]
mod soak_support;

use pin_sim::autoplay::SkillLevel;
use soak_support::*;
use std::ffi::OsStr;
use std::process::{Command, Output};
use support::*;

fn run(args: &[&OsStr]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pin_sim"))
        .args(args)
        .output()
        .expect("run pin_sim")
}

fn text(o: &Output) -> String {
    format!(
        "status={} stdout={} stderr={}",
        o.status,
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

/// `pin_sim soak <bundle> --games N --seed 5 --skill standard --threads 2
/// --out <out>`.
fn cli_soak(bundle: &std::path::Path, games: &str, out: &std::path::Path) -> Output {
    run(&[
        OsStr::new("soak"),
        bundle.as_os_str(),
        OsStr::new("--games"),
        OsStr::new(games),
        OsStr::new("--seed"),
        OsStr::new("5"),
        OsStr::new("--skill"),
        OsStr::new("standard"),
        OsStr::new("--threads"),
        OsStr::new("2"),
        OsStr::new("--out"),
        out.as_os_str(),
    ])
}

#[test]
fn clean_fixture_has_zero_findings() {
    let r = soak(
        &clean(),
        cfg(3, 2024, SkillLevel::Standard, 2, 900 * SECOND),
    );
    let f = sim_findings(&r);
    assert!(f.is_empty(), "clean fixture findings:{}", describe(&r));
    let v = parse_json(&r.to_json());
    let listed: Vec<_> = json_findings(&v)
        .iter()
        .filter(|f| {
            f.get("kind")
                .and_then(serde_json::Value::as_str)
                .is_none_or(|k| norm(k) != norm("TickBudget"))
        })
        .collect();
    assert!(listed.is_empty(), "JSON lists findings: {listed:?}");
}

#[test]
fn cli_clean_fixture_exits_0_and_writes_both_reports() {
    let out = temp_dir("cli-clean");
    let o = cli_soak(&clean(), "2", &out);
    assert_eq!(o.status.code(), Some(0), "{}", text(&o));
    let json = std::fs::read_to_string(out.join("soak_report.json")).expect("soak_report.json");
    let md = std::fs::read_to_string(out.join("soak_report.md")).expect("soak_report.md");
    let v = parse_json(&json);
    assert_eq!(v.get("schema").and_then(serde_json::Value::as_u64), Some(1));
    assert!(!md.trim().is_empty());
    for s in CLEAN_SWITCHES {
        assert!(md.contains(s), "Markdown coverage lacks {s}");
    }
}

#[test]
fn cli_exits_1_for_every_planted_defect() {
    for (name, bundle) in defect_bundles() {
        let out = temp_dir(&format!("cli-{name}"));
        let o = cli_soak(&bundle, "1", &out);
        assert_eq!(o.status.code(), Some(1), "defect {name}: {}", text(&o));
        assert!(
            out.join("soak_report.json").is_file() && out.join("soak_report.md").is_file(),
            "defect {name}: reports not written"
        );
        let stderr = String::from_utf8_lossy(&o.stderr);
        assert!(!stderr.contains("panicked"), "defect {name}: {stderr}");
    }
}

fn assert_bad_args(args: &[&OsStr], what: &str) {
    let o = run(args);
    assert_eq!(o.status.code(), Some(2), "{what}: {}", text(&o));
    let stderr = String::from_utf8_lossy(&o.stderr);
    assert!(!stderr.contains("panicked"), "{what}: {stderr}");
    assert!(!stderr.trim().is_empty(), "{what}: no message on stderr");
}

#[test]
fn cli_bad_arguments_exit_2() {
    let out = temp_dir("cli-bad");
    let b = clean();
    let o = out.as_os_str();
    let bo = b.as_os_str();
    let s = OsStr::new;
    assert_bad_args(
        &[
            s("soak"),
            s("--games"),
            s("1"),
            s("--seed"),
            s("1"),
            s("--out"),
            o,
        ],
        "missing bundle",
    );
    assert_bad_args(
        &[
            s("soak"),
            bo,
            s("--games"),
            s("1"),
            s("--skill"),
            s("godlike"),
            s("--out"),
            o,
        ],
        "unknown skill",
    );
    assert_bad_args(
        &[s("soak"), bo, s("--games"), s("0"), s("--out"), o],
        "--games 0",
    );
    assert_bad_args(
        &[s("soak"), bo, s("--games"), s("many"), s("--out"), o],
        "--games many",
    );
    assert!(
        !out.join("soak_report.json").exists(),
        "a rejected command line must not write a report"
    );
}

#[test]
fn cli_nonexistent_bundle_fails_without_panicking() {
    let out = temp_dir("cli-nobundle");
    let missing = out.join("no_such_bundle");
    let o = cli_soak(&missing, "1", &out);
    assert!(!o.status.success(), "{}", text(&o));
    assert!(!String::from_utf8_lossy(&o.stderr).contains("panicked"));
}

#[test]
fn every_skill_name_is_accepted() {
    for sk in ["novice", "standard", "expert"] {
        let out = temp_dir(&format!("cli-skill-{sk}"));
        let o = run(&[
            OsStr::new("soak"),
            clean().as_os_str(),
            OsStr::new("--games"),
            OsStr::new("1"),
            OsStr::new("--seed"),
            OsStr::new("3"),
            OsStr::new("--skill"),
            OsStr::new(sk),
            OsStr::new("--threads"),
            OsStr::new("1"),
            OsStr::new("--out"),
            out.as_os_str(),
        ]);
        assert!(
            matches!(o.status.code(), Some(0 | 1)),
            "--skill {sk}: {}",
            text(&o)
        );
        assert!(
            out.join("soak_report.json").is_file(),
            "--skill {sk}: no report"
        );
    }
}
