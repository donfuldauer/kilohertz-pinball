//! Subphase 2.9 spec tests (Independent Tester), T6: report format.
//! JSON parses with `schema == 1` and every finding field; the Markdown
//! names every finding's subject; coverage lists every declared switch with
//! a hit count; ball-time and score distributions carry
//! count/min/median/p90/p99/max.

#[path = "fixtures/spec_2_9/support.rs"]
mod support;

#[path = "fixtures/spec_2_9/soak_support.rs"]
mod soak_support;

use pin_sim::autoplay::{SkillLevel, SoakReport};
use soak_support::*;
use support::*;

/// A report with at least one finding (UnreachableSwitch `hidden_opto`):
/// the hidden-switch table, 3 Standard games of at most 60 s.
fn hidden_report() -> SoakReport {
    soak(
        &variant("hidden", "clean"),
        cfg(3, 21, SkillLevel::Standard, 2, 60 * SECOND),
    )
}

#[test]
fn json_has_schema_1_and_every_finding_field() {
    let r = hidden_report();
    assert!(
        !r.findings.is_empty(),
        "the hidden table must yield findings"
    );
    let json = r.to_json();
    let v = parse_json(&json);
    assert_eq!(
        v.get("schema").and_then(serde_json::Value::as_u64),
        Some(1),
        "{json}"
    );
    assert!(
        json.contains("\"schema\": 1") || json.contains("\"schema\":1"),
        "{json}"
    );
    let fs = json_findings(&v);
    assert_eq!(fs.len(), r.findings.len(), "JSON and struct finding counts");
    for (j, f) in fs.iter().zip(&r.findings) {
        for key in ["kind", "subject", "game", "tick", "detail"] {
            assert!(j.get(key).is_some(), "finding lacks {key:?}: {j}");
        }
        assert_eq!(j["subject"].as_str(), Some(f.subject.as_str()));
        assert_eq!(j["detail"].as_str(), Some(f.detail.as_str()));
        if f.game == pin_sim::autoplay::report::SOAK_WIDE {
            assert!(
                j["game"].is_null(),
                "soak-wide finding has game {}",
                j["game"]
            );
        } else {
            assert_eq!(j["game"].as_u64(), Some(u64::from(f.game)));
        }
        assert_eq!(j["tick"].as_u64(), Some(f.tick));
        assert_eq!(
            norm(j["kind"].as_str().expect("kind is a string")),
            norm(kind_name(&f.kind))
        );
    }
}

#[test]
fn json_key_order_is_stable() {
    let a = hidden_report().to_json();
    let b = hidden_report().to_json();
    assert_eq!(strip_timing(&a), strip_timing(&b));
}

#[test]
fn markdown_names_every_finding_subject() {
    let r = hidden_report();
    let md = r.to_markdown();
    for f in &r.findings {
        assert!(
            md.contains(&f.subject),
            "Markdown lacks {:?}:\n{md}",
            f.subject
        );
    }
}

#[test]
fn coverage_lists_every_declared_switch_with_a_hit_count() {
    let r = hidden_report();
    let md = r.to_markdown();
    let json = r.to_json();
    let mut all: Vec<&str> = CLEAN_SWITCHES.to_vec();
    all.push("hidden_opto");
    for s in all {
        let line = md
            .lines()
            .find(|l| {
                l.split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                    .any(|w| w == s)
                    && l.replace(s, "").chars().any(|c| c.is_ascii_digit())
            })
            .unwrap_or_else(|| panic!("no Markdown line lists {s} with a count:\n{md}"));
        assert!(!line.is_empty());
        assert!(
            json.contains(&format!("\"{s}\"")),
            "JSON coverage lacks {s}"
        );
    }
    // Cabinet inputs are not part of switch coverage.
    let v = parse_json(&json);
    for f in json_findings(&v) {
        let subj = f["subject"].as_str().unwrap_or("");
        assert!(
            !["start", "launch", "tilt", "action"].contains(&subj),
            "cabinet input in findings: {f}"
        );
    }
}

#[test]
fn ball_time_and_score_distributions_have_all_statistics() {
    let r = soak(&clean(), cfg(3, 8, SkillLevel::Standard, 2, 120 * SECOND));
    let v = parse_json(&r.to_json());
    let d = distributions(&v);
    let paths: Vec<_> = d.iter().map(|(p, _)| p.to_lowercase()).collect();
    assert!(
        paths.iter().any(|p| p.contains("score")),
        "no score distribution (count/min/median/p90/p99/max) in: {paths:?}\n{v}"
    );
    assert!(
        paths.iter().any(|p| p.contains("ball")),
        "no ball-time distribution (count/min/median/p90/p99/max) in: {paths:?}\n{v}"
    );
    for (p, dist) in &d {
        let num = |k: &str| {
            dist[k]
                .as_f64()
                .unwrap_or_else(|| panic!("{p}.{k} is not a number: {dist}"))
        };
        let (mn, med, p90, p99, mx) = (
            num("min"),
            num("median"),
            num("p90"),
            num("p99"),
            num("max"),
        );
        assert!(
            mn <= med && med <= p90 && p90 <= p99 && p99 <= mx,
            "{p}: statistics out of order: {dist}"
        );
        assert!(num("count") >= 0.0);
    }
    let score = d
        .iter()
        .find(|(p, _)| p.to_lowercase().contains("score"))
        .unwrap();
    assert_eq!(
        score.1["count"].as_u64(),
        Some(3),
        "{}: one score per game",
        score.0
    );
    let md = r.to_markdown().to_lowercase();
    for k in ["median", "p90", "p99"] {
        assert!(md.contains(k), "Markdown lacks {k}");
    }
}
