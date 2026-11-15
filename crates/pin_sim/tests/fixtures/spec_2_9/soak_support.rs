//! Autoplay helpers for the Subphase 2.9 spec tests (Independent Tester).
//! Written against the brief's contract only:
//! `pin_sim::autoplay::{SkillLevel, Bot, SoakConfig, run_soak, SoakReport,
//! Finding, FindingKind}`.

#![allow(dead_code)]

use crate::support::*;
use pin_sim::autoplay::{Bot, Finding, FindingKind, SkillLevel, SoakConfig, SoakReport, run_soak};
use pin_sim::{DEFAULT_SEED, Simulation};
use std::path::{Path, PathBuf};

/// The three skill levels by index (0 Novice, 1 Standard, 2 Expert).
pub fn skill(i: usize) -> SkillLevel {
    match i {
        0 => SkillLevel::Novice,
        1 => SkillLevel::Standard,
        _ => SkillLevel::Expert,
    }
}

/// A soak configuration from `Default` plus the brief's named fields.
pub fn cfg(
    games: u32,
    seed: u64,
    sk: SkillLevel,
    threads: usize,
    max_game_ticks: u64,
) -> SoakConfig {
    SoakConfig {
        games,
        seed,
        skill: sk,
        threads,
        max_game_ticks,
        ..Default::default()
    }
}

/// `run_soak` on a large-stack thread; panics with the error text on `Err`.
pub fn soak(bundle: &Path, c: SoakConfig) -> SoakReport {
    let b: PathBuf = bundle.to_path_buf();
    on_sim_thread(move || match run_soak(&b, &c) {
        Ok(r) => r,
        Err(e) => panic!("run_soak({}) failed: {e}", b.display()),
    })
}

/// The brief's finding kinds, by name (EndlessBall added by work item
/// A1-endless-detector).
pub const KINDS: [&str; 10] = [
    "StuckBall",
    "EndlessBall",
    "EscapedBall",
    "NonFinite",
    "UnreachableSwitch",
    "ShotCoverage",
    "ModeCoverage",
    "ScriptError",
    "QueueOverflow",
    "TickBudget",
];

/// The brief's name of a finding kind (`"other"` for kinds beyond the brief).
pub fn kind_name(k: &FindingKind) -> &'static str {
    if matches!(k, FindingKind::StuckBall) {
        "StuckBall"
    } else if matches!(k, FindingKind::EndlessBall) {
        "EndlessBall"
    } else if matches!(k, FindingKind::EscapedBall) {
        "EscapedBall"
    } else if matches!(k, FindingKind::NonFinite) {
        "NonFinite"
    } else if matches!(k, FindingKind::UnreachableSwitch) {
        "UnreachableSwitch"
    } else if matches!(k, FindingKind::ShotCoverage) {
        "ShotCoverage"
    } else if matches!(k, FindingKind::ModeCoverage) {
        "ModeCoverage"
    } else if matches!(k, FindingKind::ScriptError) {
        "ScriptError"
    } else if matches!(k, FindingKind::QueueOverflow) {
        "QueueOverflow"
    } else if matches!(k, FindingKind::TickBudget) {
        "TickBudget"
    } else {
        "other"
    }
}

/// Findings of kind `name`.
pub fn of_kind<'a>(r: &'a SoakReport, name: &str) -> Vec<&'a Finding> {
    r.findings
        .iter()
        .filter(|f| kind_name(&f.kind) == name)
        .collect()
}

/// Findings other than `TickBudget`, whose input is wall-clock time (D8:
/// tests never assert on it).
pub fn sim_findings(r: &SoakReport) -> Vec<&Finding> {
    r.findings
        .iter()
        .filter(|f| kind_name(&f.kind) != "TickBudget")
        .collect()
}

/// One line per finding, for assertion messages.
pub fn describe(r: &SoakReport) -> String {
    let mut s = String::new();
    for f in &r.findings {
        s.push_str(&format!(
            "\n  {} subject={:?} game={} tick={} detail={:?}",
            kind_name(&f.kind),
            f.subject,
            f.game,
            f.tick,
            f.detail
        ));
    }
    if s.is_empty() {
        s.push_str(" (none)");
    }
    s
}

/// Lowercase with `_`, `-` and spaces removed (`StuckBall` == `stuck_ball`
/// == `Stuck ball`).
pub fn norm(s: &str) -> String {
    s.chars()
        .filter(|c| !matches!(c, '_' | '-' | ' '))
        .flat_map(char::to_lowercase)
        .collect()
}

/// Removes every object member named `"timing"` (key, value and one adjoining
/// comma) from JSON text, leaving every other byte as it was.
pub fn strip_timing(json: &str) -> String {
    let b = json.as_bytes();
    let mut out = String::with_capacity(json.len());
    let mut i = 0;
    let mut copied = 0;
    while i < b.len() {
        if b[i] != b'"' {
            i += 1;
            continue;
        }
        let s_end = string_end(b, i);
        let is_key = &json[i..s_end] == "\"timing\"" && {
            let mut j = s_end;
            while j < b.len() && b[j].is_ascii_whitespace() {
                j += 1;
            }
            j < b.len() && b[j] == b':'
        };
        if !is_key {
            i = s_end;
            continue;
        }
        let mut j = s_end;
        while b[j] != b':' {
            j += 1;
        }
        j += 1;
        while j < b.len() && b[j].is_ascii_whitespace() {
            j += 1;
        }
        let v_end = value_end(b, j);
        // Drop a following comma, or else a preceding one.
        let mut k = v_end;
        while k < b.len() && b[k].is_ascii_whitespace() {
            k += 1;
        }
        let (cut_from, cut_to) = if k < b.len() && b[k] == b',' {
            let mut k2 = k + 1;
            while k2 < b.len() && b[k2].is_ascii_whitespace() {
                k2 += 1;
            }
            (i, k2)
        } else {
            let mut p = i;
            while p > copied && b[p - 1].is_ascii_whitespace() {
                p -= 1;
            }
            if p > copied && b[p - 1] == b',' {
                p -= 1;
            }
            (p, v_end)
        };
        out.push_str(&json[copied..cut_from]);
        copied = cut_to;
        i = cut_to;
    }
    out.push_str(&json[copied..]);
    out
}

/// Index after the JSON string starting at `b[i] == '"'`.
fn string_end(b: &[u8], i: usize) -> usize {
    let mut j = i + 1;
    while j < b.len() {
        match b[j] {
            b'\\' => j += 2,
            b'"' => return j + 1,
            _ => j += 1,
        }
    }
    b.len()
}

/// Index after the JSON value starting at `b[i]`.
fn value_end(b: &[u8], i: usize) -> usize {
    match b.get(i) {
        Some(b'"') => string_end(b, i),
        Some(b'{' | b'[') => {
            let mut depth = 0usize;
            let mut j = i;
            while j < b.len() {
                match b[j] {
                    b'"' => {
                        j = string_end(b, j);
                        continue;
                    }
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => {
                        depth -= 1;
                        if depth == 0 {
                            return j + 1;
                        }
                    }
                    _ => {}
                }
                j += 1;
            }
            b.len()
        }
        _ => {
            let mut j = i;
            while j < b.len() && !matches!(b[j], b',' | b'}' | b']') && !b[j].is_ascii_whitespace()
            {
                j += 1;
            }
            j
        }
    }
}

/// Parses a report's JSON, panicking with the text on failure.
pub fn parse_json(text: &str) -> serde_json::Value {
    serde_json::from_str(text).unwrap_or_else(|e| panic!("to_json() is not JSON ({e}):\n{text}"))
}

/// The JSON findings array (top-level `"findings"`).
pub fn json_findings(v: &serde_json::Value) -> &Vec<serde_json::Value> {
    v.get("findings")
        .and_then(serde_json::Value::as_array)
        .unwrap_or_else(|| panic!("no top-level \"findings\" array in {v}"))
}

/// Whether the JSON report holds a finding of `kind` (normalised) whose
/// subject is exactly `subject`.
pub fn json_has(v: &serde_json::Value, kind: &str, subject: &str) -> bool {
    json_findings(v).iter().any(|f| {
        f.get("kind")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|k| norm(k) == norm(kind))
            && f.get("subject").and_then(serde_json::Value::as_str) == Some(subject)
    })
}

/// Requires `f` to appear in the report's Markdown (kind and subject) and
/// JSON (kind and exact subject).
pub fn assert_in_text(r: &SoakReport, f: &Finding) {
    let kind = kind_name(&f.kind);
    let md = r.to_markdown();
    assert!(
        md.contains(&f.subject),
        "Markdown lacks subject {:?} of {kind}:\n{md}",
        f.subject
    );
    assert!(
        norm(&md).contains(&norm(kind)),
        "Markdown never names kind {kind}:\n{md}"
    );
    let v = parse_json(&r.to_json());
    assert!(
        json_has(&v, kind, &f.subject),
        "JSON has no {kind} finding with subject {:?}:\n{}",
        f.subject,
        r.to_json()
    );
}

/// The outcome of one manually driven game (fixture rules' observables).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GameOutcome {
    pub score: i64,
    pub ball_ticks: i64,
    pub ticks: u64,
    pub finished: bool,
}

/// Drives a fresh simulation of `bundle` with `Bot::new(seed, sk)` (tick,
/// then `advance(1)`) until the fixture rules report one finished game or
/// `cap` ticks pass.
pub fn bot_game(bundle: &Path, seed: u64, sk: SkillLevel, cap: u64) -> GameOutcome {
    let mut sim: Simulation = load(bundle, DEFAULT_SEED);
    let mut bot = Bot::new(seed, sk);
    while sim.persist(KEY_GAMES_DONE) < 1 && sim.tick() < cap {
        bot.tick(&mut sim);
        sim.advance(1);
    }
    GameOutcome {
        score: sim.score(0).unwrap_or(0),
        ball_ticks: sim.persist(KEY_BALL_TICKS),
        ticks: sim.tick(),
        finished: sim.persist(KEY_GAMES_DONE) >= 1,
    }
}

/// Every object in `v` (recursively) that has all of the distribution keys
/// count/min/median/p90/p99/max, with the path of keys leading to it.
pub fn distributions(v: &serde_json::Value) -> Vec<(String, serde_json::Value)> {
    fn walk(v: &serde_json::Value, path: &str, out: &mut Vec<(String, serde_json::Value)>) {
        match v {
            serde_json::Value::Object(m) => {
                if ["count", "min", "median", "p90", "p99", "max"]
                    .iter()
                    .all(|k| m.contains_key(*k))
                {
                    out.push((path.to_owned(), v.clone()));
                }
                for (k, c) in m {
                    walk(c, &format!("{path}.{k}"), out);
                }
            }
            serde_json::Value::Array(a) => {
                for (i, c) in a.iter().enumerate() {
                    walk(c, &format!("{path}[{i}]"), out);
                }
            }
            _ => {}
        }
    }
    let mut out = Vec::new();
    walk(v, "", &mut out);
    out
}
