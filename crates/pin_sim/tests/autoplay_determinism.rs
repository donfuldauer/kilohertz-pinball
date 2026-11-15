//! Subphase 2.9 verification 3 and requirement 6: the bot is deterministic
//! for a given seed (identical `state_hash` sequences), different seeds
//! play differently, the soak report and the per-game hashes are identical
//! across runs and thread counts, and the skill levels order as specified.

#[path = "fixtures/autoplay/support.rs"]
mod support;

use pin_sim::autoplay::{Bot, BotStats, SkillLevel, run_soak};
use pin_sim::{SimConfig, Simulation};
use support::*;

/// The `state_hash` after every tick of a bot game.
fn hash_sequence(
    bundle: &str,
    sim_seed: u64,
    bot_seed: u64,
    skill: SkillLevel,
    ticks: u64,
) -> Vec<u64> {
    let bundle = fixture(bundle);
    on_sim_thread(move || {
        let mut sim = Simulation::load(
            &bundle,
            SimConfig {
                seed: sim_seed,
                ..SimConfig::default()
            },
        )
        .unwrap();
        let mut bot = Bot::new(bot_seed, skill);
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
fn same_seed_same_state_hash_sequence() {
    let a = hash_sequence("clean", 9, 7, SkillLevel::Standard, 12_000);
    let b = hash_sequence("clean", 9, 7, SkillLevel::Standard, 12_000);
    assert_eq!(a, b);
}

#[test]
fn final_hash_is_stable_over_ten_runs() {
    let first = *hash_sequence("clean", 3, 3, SkillLevel::Expert, 4_000)
        .last()
        .unwrap();
    for _ in 0..9 {
        assert_eq!(
            *hash_sequence("clean", 3, 3, SkillLevel::Expert, 4_000)
                .last()
                .unwrap(),
            first
        );
    }
}

#[test]
fn different_bot_seeds_play_differently() {
    let a = hash_sequence("clean", 9, 1, SkillLevel::Standard, 6_000);
    let b = hash_sequence("clean", 9, 2, SkillLevel::Standard, 6_000);
    assert_ne!(a, b);
    // Same simulation seed: the difference comes from the bot's presses.
    assert_eq!(a[0], b[0]);
}

#[test]
fn soak_report_identical_across_runs_and_thread_counts() {
    let bundle = fixture("clean");
    let reports = on_sim_thread(move || {
        let mut v = Vec::new();
        for threads in [1usize, 2, 4, 1, 1] {
            let mut c = cfg(5, 0xC0FFEE, 15_000);
            c.threads = threads;
            v.push(run_soak(&bundle, &c).unwrap());
        }
        v
    });
    let json = reports[0].to_json_deterministic();
    let md = reports[0].to_markdown_deterministic();
    let hashes = reports[0].state_hashes();
    assert_eq!(hashes.len(), 5);
    for r in &reports[1..] {
        assert_eq!(r.to_json_deterministic(), json);
        assert_eq!(r.to_markdown_deterministic(), md);
        assert_eq!(r.state_hashes(), hashes);
    }
    // The thread count only shows in the timing section.
    assert_eq!(reports[1].timing.threads, 2);
    assert_eq!(reports[2].timing.threads, 4);
    assert!(reports[2].to_json().contains("\"threads\": 4"));
    assert!(!json.contains("\"timing\""));
    // Distinct games got distinct seeds.
    let mut seeds: Vec<u64> = reports[0].game_list.iter().map(|g| g.seed).collect();
    seeds.dedup();
    assert_eq!(seeds.len(), 5);
}

#[test]
fn different_soak_seeds_give_different_games() {
    let bundle = fixture("clean");
    let (a, b) = on_sim_thread(move || {
        (
            run_soak(&bundle, &cfg(2, 1, 8_000)).unwrap(),
            run_soak(&bundle, &cfg(2, 2, 8_000)).unwrap(),
        )
    });
    assert_ne!(a.state_hashes(), b.state_hashes());
}

fn play(seed: u64, skill: SkillLevel, ticks: u64) -> BotStats {
    let bundle = fixture("clean");
    on_sim_thread(move || {
        let mut sim = Simulation::load(
            &bundle,
            SimConfig {
                seed,
                ..SimConfig::default()
            },
        )
        .unwrap();
        let mut bot = Bot::new(seed, skill);
        for _ in 0..ticks {
            bot.tick(&mut sim);
            sim.advance(1);
        }
        bot.stats()
    })
}

/// Skill ordering over seeds: on every seed the Expert takes a larger share
/// of its predicted flips than the Novice (the miss rate), and over the
/// seeds it makes more flips on prediction in total.
#[test]
fn expert_flips_on_more_predictions_than_novice() {
    let mut expert_total = 0;
    let mut novice_total = 0;
    for seed in 0..6u64 {
        let n = play(seed, SkillLevel::Novice, 30_000);
        let e = play(seed, SkillLevel::Expert, 30_000);
        assert!(
            n.predictions > 0 && e.predictions > 0,
            "seed {seed}: {n:?} {e:?}"
        );
        let rate = |s: &BotStats| s.flips_on_prediction as f64 / s.predictions as f64;
        assert!(rate(&e) > rate(&n), "seed {seed}: {e:?} vs {n:?}");
        assert!(n.misses > e.misses, "seed {seed}: {n:?} vs {e:?}");
        expert_total += e.flips_on_prediction;
        novice_total += n.flips_on_prediction;
    }
    assert!(
        expert_total > novice_total,
        "expert {expert_total} vs novice {novice_total}"
    );
}
