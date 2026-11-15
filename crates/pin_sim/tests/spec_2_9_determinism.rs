//! Subphase 2.9 spec tests (Independent Tester), T3: determinism (D9).
//! Same (bundle, seed, skill, games) => byte-identical `to_json()` outside
//! the `"timing"` section, for repeated runs and any thread count; different
//! seeds play differently; a manually driven `Bot` gives identical
//! `state_hash` sequences.

#[path = "fixtures/spec_2_9/support.rs"]
mod support;

#[path = "fixtures/spec_2_9/soak_support.rs"]
mod soak_support;

use pin_sim::DEFAULT_SEED;
use pin_sim::autoplay::{Bot, SkillLevel};
use soak_support::*;
use support::*;

#[test]
fn strip_timing_removes_only_the_timing_member() {
    let a = r#"{"schema": 1, "timing": {"p999_ms": 0.2, "per": [1, {"x": "}"}]}, "findings": []}"#;
    assert_eq!(strip_timing(a), r#"{"schema": 1, "findings": []}"#);
    let b = "{\n  \"schema\": 1,\n  \"timing\": {\"a\": 1}\n}";
    assert_eq!(strip_timing(b), "{\n  \"schema\": 1\n}");
    let c = r#"{"detail": "timing", "x": {"timing": 3, "y": 2}}"#;
    assert_eq!(strip_timing(c), r#"{"detail": "timing", "x": {"y": 2}}"#);
}

#[test]
fn report_is_identical_across_runs_and_thread_counts() {
    let bundle = clean();
    let mut texts = Vec::new();
    for threads in [1usize, 1, 1, 2, 4] {
        let r = soak(
            &bundle,
            cfg(4, 0x5EED, SkillLevel::Standard, threads, 60 * SECOND),
        );
        let json = r.to_json();
        let _ = parse_json(&json);
        texts.push((threads, strip_timing(&json)));
    }
    let (_, first) = &texts[0];
    assert!(
        first.contains("\"schema\""),
        "stripped JSON lost the schema key: {first}"
    );
    for (threads, t) in &texts[1..] {
        assert_eq!(
            t, first,
            "threads={threads}: JSON differs outside \"timing\""
        );
    }
}

#[test]
fn findings_are_identical_across_thread_counts_on_a_defect_bundle() {
    let bundle = variant("hidden", "clean");
    let base = soak(&bundle, cfg(3, 77, SkillLevel::Novice, 1, 40 * SECOND));
    for threads in [2usize, 4] {
        let r = soak(
            &bundle,
            cfg(3, 77, SkillLevel::Novice, threads, 40 * SECOND),
        );
        assert_eq!(r.findings.len(), base.findings.len(), "threads={threads}");
        for (a, b) in r.findings.iter().zip(&base.findings) {
            assert_eq!(
                (kind_name(&a.kind), &a.subject, a.game, a.tick, &a.detail),
                (kind_name(&b.kind), &b.subject, b.game, b.tick, &b.detail),
                "threads={threads}"
            );
        }
        assert_eq!(
            strip_timing(&r.to_json()),
            strip_timing(&base.to_json()),
            "threads={threads}"
        );
    }
}

#[test]
fn different_bot_seeds_give_different_scores() {
    let bundle = clean();
    let outcomes: Vec<GameOutcome> = (0..8u64)
        .map(|seed| {
            let b = bundle.clone();
            on_sim_thread(move || bot_game(&b, 1000 + seed, SkillLevel::Standard, 90 * SECOND))
        })
        .collect();
    let first = (outcomes[0].score, outcomes[0].ball_ticks);
    assert!(
        outcomes.iter().any(|o| (o.score, o.ball_ticks) != first),
        "8 seeds played identically: {outcomes:?}"
    );
    assert!(
        outcomes.iter().any(|o| o.score != outcomes[0].score),
        "8 seeds scored identically: {outcomes:?}"
    );
}

#[test]
fn different_soak_seeds_give_different_score_distributions() {
    let bundle = clean();
    let mut seen: Vec<String> = Vec::new();
    for seed in 0..8u64 {
        let r = soak(
            &bundle,
            cfg(1, 500 + seed, SkillLevel::Standard, 1, 90 * SECOND),
        );
        let v = parse_json(&r.to_json());
        let score: Vec<_> = distributions(&v)
            .into_iter()
            .filter(|(p, _)| p.to_lowercase().contains("score"))
            .map(|(_, d)| d.to_string())
            .collect();
        assert!(!score.is_empty(), "no score distribution in {v}");
        seen.push(score.join("|"));
    }
    assert!(
        seen.iter().any(|s| s != &seen[0]),
        "8 seeds gave the same score distribution: {seen:?}"
    );
}

fn hash_sequence(seed: u64, sk: SkillLevel, ticks: u64) -> Vec<u64> {
    on_sim_thread(move || {
        let mut sim = load(&clean(), DEFAULT_SEED);
        let mut bot = Bot::new(seed, sk);
        let mut out = Vec::with_capacity(ticks as usize);
        for _ in 0..ticks {
            bot.tick(&mut sim);
            sim.advance(1);
            out.push(sim.state_hash());
        }
        out
    })
}

#[test]
fn manual_bot_runs_give_identical_state_hash_sequences() {
    for i in 0..3 {
        let a = hash_sequence(42, skill(i), 5000);
        let b = hash_sequence(42, skill(i), 5000);
        assert_eq!(a.len(), 5000);
        if let Some(t) = a.iter().zip(&b).position(|(x, y)| x != y) {
            panic!("skill {i}: state_hash diverged at tick {}", t + 1);
        }
    }
}

#[test]
fn bot_plays_the_ball_in_a_manual_run() {
    // The 5000-tick sequences above must cover real play: a game started
    // and the ball moved (otherwise identical hashes prove little).
    on_sim_thread(|| {
        let mut sim = load(&clean(), DEFAULT_SEED);
        let mut bot = Bot::new(42, SkillLevel::Standard);
        for _ in 0..5000 {
            bot.tick(&mut sim);
            sim.advance(1);
        }
        assert!(
            sim.persist(KEY_BALLS_SERVED) >= 1,
            "the bot never started a game"
        );
        assert!(sim.switch_hits("top_opto").unwrap() >= 1);
    });
}
