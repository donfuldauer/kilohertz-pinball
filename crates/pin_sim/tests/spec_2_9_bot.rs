//! Subphase 2.9 spec tests (Independent Tester), T4: bot behaviour, as
//! properties over fixed seeds in simulated time (never wall-clock).

#[path = "fixtures/spec_2_9/support.rs"]
mod support;

#[path = "fixtures/spec_2_9/soak_support.rs"]
mod soak_support;

use pin_sim::DEFAULT_SEED;
use pin_sim::autoplay::{Bot, SkillLevel};
use soak_support::*;
use support::*;

/// Seeds of the skill comparison.
const SEEDS: u64 = 12;
/// Simulated-time cap of one comparison game (a game still running at the
/// cap counts its ball time and score so far).
const CAP: u64 = 300 * SECOND;

fn games(sk: usize) -> Vec<GameOutcome> {
    (0..SEEDS)
        .map(|s| {
            let b = clean();
            on_sim_thread(move || bot_game(&b, 7000 + s, skill(sk), CAP))
        })
        .collect()
}

/// Mean and standard error of the mean.
fn mean_se(x: &[f64]) -> (f64, f64) {
    let n = x.len() as f64;
    let m = x.iter().sum::<f64>() / n;
    let var = x.iter().map(|v| (v - m) * (v - m)).sum::<f64>() / (n - 1.0);
    (m, (var / n).sqrt())
}

/// Expert beats Novice on mean ball time or mean score over 12 seeds (games
/// capped at 300 s). The margin is derived from the samples: the difference
/// of the means must exceed the standard error of that difference (and be
/// positive).
#[test]
fn expert_outplays_novice_over_fixed_seeds() {
    let novice = games(0);
    let expert = games(2);
    for (name, g) in [("novice", &novice), ("expert", &expert)] {
        assert!(
            g.iter().all(|o| o.ball_ticks > 0),
            "{name}: a game never put a ball in play: {g:?}"
        );
    }
    let metric = |g: &[GameOutcome], f: fn(&GameOutcome) -> i64| -> Vec<f64> {
        g.iter().map(|o| f(o) as f64).collect()
    };
    let mut verdicts = Vec::new();
    for (what, f) in [
        (
            "ball time",
            (|o: &GameOutcome| o.ball_ticks) as fn(&GameOutcome) -> i64,
        ),
        ("score", |o: &GameOutcome| o.score),
    ] {
        let (mn, sn) = mean_se(&metric(&novice, f));
        let (me, se) = mean_se(&metric(&expert, f));
        let margin = (sn * sn + se * se).sqrt();
        verdicts.push((what, me - mn > margin && me > mn, mn, me, margin));
    }
    assert!(
        verdicts.iter().any(|v| v.1),
        "Expert does not beat Novice by more than one standard error: {verdicts:?}\n\
         novice={novice:?}\nexpert={expert:?}"
    );
}

/// Longest run of ticks a flipper was held (button closed or the bat
/// raised more than 0.5 rad) in `n` bot ticks.
fn longest_hold(sim: &mut pin_sim::Simulation, bot: &mut Bot, n: u64) -> [u64; 2] {
    let mut run = [0u64; 2];
    let mut longest = [0u64; 2];
    for _ in 0..n {
        bot.tick(sim);
        sim.advance(1);
        for (i, (button, bat)) in [
            ("left_flipper", "lower_left"),
            ("right_flipper", "lower_right"),
        ]
        .into_iter()
        .enumerate()
        {
            let closed = sim.switch_closed(button).unwrap();
            let raised = sim.flipper(bat).unwrap().0.abs() > 0.5;
            run[i] = if closed || raised { run[i] + 1 } else { 0 };
            longest[i] = longest[i].max(run[i]);
        }
    }
    longest
}

#[test]
fn bot_releases_the_flippers_within_5_s_after_the_ball_is_removed() {
    for seed in [1u64, 2, 3] {
        for sk in 0..3 {
            on_sim_thread(move || {
                // Inert rules: nothing serves; the only ball is injected.
                let b = variant("clean", "inert");
                let mut sim = load(&b, DEFAULT_SEED);
                let mut bot = Bot::new(seed, skill(sk));
                let h = sim
                    .spawn_ball("standard_steel", [0.200, 0.300, 0.0135], [0.0, -0.3, 0.0])
                    .unwrap();
                // The ball reaches the left flipper; let the bot play it,
                // possibly cradling it.
                let _ = longest_hold(&mut sim, &mut bot, 3000);
                for h2 in sim.ball_handles() {
                    sim.remove_ball(h2);
                }
                assert!(sim.ball(h).is_none());
                assert_eq!(sim.balls_in_play(), 0);
                let held = longest_hold(&mut sim, &mut bot, 10 * SECOND);
                assert!(
                    held[0] < 5 * SECOND && held[1] < 5 * SECOND,
                    "seed {seed} skill {sk}: flipper held {held:?} ticks with no ball"
                );
            });
        }
    }
}

#[test]
fn bot_starts_the_game_from_attract_and_launches_a_ball() {
    for sk in 0..3 {
        on_sim_thread(move || {
            // Launch variant: `start` begins the game, `launch` serves.
            let b = variant("clean", "launch");
            let mut sim = load(&b, DEFAULT_SEED);
            let mut bot = Bot::new(5, skill(sk));
            let mut saw_ball = false;
            for _ in 0..30 * SECOND {
                bot.tick(&mut sim);
                sim.advance(1);
                saw_ball |= sim.balls_in_play() > 0;
                if saw_ball {
                    break;
                }
            }
            assert!(
                sim.switch_hits("start").unwrap() >= 1,
                "skill {sk}: start never pressed"
            );
            assert!(
                sim.switch_hits("launch").unwrap() >= 1,
                "skill {sk}: launch never pressed"
            );
            assert!(saw_ball, "skill {sk}: no ball launched in 30 s");
            assert_eq!(
                sim.persist(KEY_GAME_STATE),
                1,
                "skill {sk}: game not running"
            );
        });
    }
}

#[test]
fn bot_starts_the_auto_serve_game_and_plays_it_to_the_end() {
    on_sim_thread(|| {
        let o = bot_game(&clean(), 9, SkillLevel::Standard, 900 * SECOND);
        assert!(o.finished, "game did not end in 900 s: {o:?}");
        assert!(o.ball_ticks > 0 && o.score > 0, "{o:?}");
    });
}

#[test]
fn bot_flips_when_the_ball_comes_down() {
    // Over a played game the bot must have flipped both flippers
    // (end-of-stroke switches closed).
    on_sim_thread(|| {
        let mut sim = load(&clean(), DEFAULT_SEED);
        let mut bot = Bot::new(13, SkillLevel::Expert);
        while sim.persist(KEY_GAMES_DONE) < 1 && sim.tick() < 900 * SECOND {
            bot.tick(&mut sim);
            sim.advance(1);
        }
        assert!(sim.switch_hits("lower_left_eos").unwrap() > 0);
        assert!(sim.switch_hits("lower_right_eos").unwrap() > 0);
    });
}
