//! Regression test for fix-2.9-bot-timing: on the grey-box bundle the skill
//! levels order correctly. The 2.9 bot pressed the flipper at the predicted
//! crossing plus a late-only reaction delay, so the Expert's bat reached
//! the end of its stroke as the ball arrived (a dead bounce) and the Novice
//! out-scored it. With the shared drawn flip-delay window and the centred
//! reaction error (`BotConfig::flip_delay_ms`, `ReactionDraw::Centred`) the
//! Expert must beat the Novice on both mean score and mean ball time by
//! more than one standard error. The Standard is reported, not asserted
//! (see the test body).
//!
//! On the old behaviour (`flip_delay_ms: (0, 0)`, `ReactionDraw::Late`)
//! this test fails: see `docs/decisions/2026-11-18-fix-2.9-bot-timing.md`
//! for the measured means.
//!
//! Simulated time only (D8): every game is a seeded soak game played by
//! `play_soak_game`; nothing reads the wall clock. The bundle is built by
//! `tables/greybox/build.sh` into the target dir once per test binary.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use pin_sim::autoplay::{SkillLevel, SoakConfig, play_soak_game};

/// Games per skill and seed (game indices `0..GAMES` of the soak).
const GAMES: u32 = 16;
/// Soak seeds; every seed plays `GAMES` games per skill.
const SEEDS: [u64; 2] = [1, 2];
/// Simulated-time cap of one game, ticks (a game still running at the cap
/// counts its score and ball time so far).
const CAP: u64 = 300_000;
/// Worker threads.
const THREADS: u32 = 4;

/// The grey-box bundle, built once per test binary into the target dir.
fn greybox() -> &'static PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR"))
            .join(format!("fix_2_9_bot_timing_{}", std::process::id()))
            .join("greybox");
        let _ = std::fs::remove_dir_all(&out);
        build_greybox(&out);
        out
    })
}

/// Runs `tables/greybox/build.sh <out>` with the cargo that runs this test
/// first on `PATH`.
fn build_greybox(out: &Path) {
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tables/greybox/build.sh");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| env!("CARGO").into());
    let mut path = Vec::new();
    if let Some(dir) = Path::new(&cargo).parent() {
        path.push(dir.to_path_buf());
    }
    if let Some(p) = std::env::var_os("PATH") {
        path.extend(std::env::split_paths(&p));
    }
    let status = std::process::Command::new("sh")
        .arg(&script)
        .arg(out)
        .env("PATH", std::env::join_paths(path).expect("PATH"))
        .status()
        .expect("run tables/greybox/build.sh");
    assert!(status.success(), "tables/greybox/build.sh failed: {status}");
}

/// One game's score and mean ball time (ms).
#[derive(Copy, Clone, Debug)]
struct Game {
    score: f64,
    ball_ms: f64,
}

/// Plays every (seed, game) of `skill` with the default bot settings, on
/// [`THREADS`] threads; results in (seed, game) order.
fn games(skill: SkillLevel) -> Vec<Game> {
    let jobs: Vec<(u64, u32)> = SEEDS
        .iter()
        .flat_map(|&s| (0..GAMES).map(move |g| (s, g)))
        .collect();
    let bundle = greybox().clone();
    let mut out = vec![None; jobs.len()];
    let chunks: Vec<Vec<(usize, (u64, u32))>> = (0..THREADS as usize)
        .map(|t| {
            jobs.iter()
                .copied()
                .enumerate()
                .filter(|(i, _)| i % THREADS as usize == t)
                .collect()
        })
        .collect();
    let handles: Vec<_> = chunks
        .into_iter()
        .map(|chunk| {
            let bundle = bundle.clone();
            std::thread::Builder::new()
                .stack_size(pin_sim::cli::SIM_THREAD_STACK_BYTES)
                .spawn(move || {
                    chunk
                        .into_iter()
                        .map(|(i, (seed, game))| {
                            let cfg = SoakConfig {
                                games: GAMES,
                                seed,
                                skill,
                                threads: 1,
                                max_game_ticks: CAP,
                                measure_timing: false,
                                check_tick_budget: false,
                                ..SoakConfig::default()
                            };
                            let (s, o) = play_soak_game(&bundle, &cfg, game).expect("game");
                            // Balls still on the table at the end count at
                            // their observed age (censored), as in the soak.
                            let times = o.ball_times_ms.iter().chain(&o.censored_ball_times_ms);
                            let n = times.clone().count().max(1) as f64;
                            let ball_ms = times.map(|&t| t as f64).sum::<f64>() / n;
                            (
                                i,
                                Game {
                                    score: s.score.unwrap_or(0) as f64,
                                    ball_ms,
                                },
                            )
                        })
                        .collect::<Vec<_>>()
                })
                .expect("spawn")
        })
        .collect();
    for h in handles {
        for (i, g) in h.join().expect("game thread") {
            out[i] = Some(g);
        }
    }
    out.into_iter()
        .map(|g| g.expect("every game ran"))
        .collect()
}

/// Mean and standard error of the mean.
fn mean_se(x: &[f64]) -> (f64, f64) {
    let n = x.len() as f64;
    let m = x.iter().sum::<f64>() / n;
    let var = x.iter().map(|v| (v - m) * (v - m)).sum::<f64>() / (n - 1.0);
    (m, (var / n).sqrt())
}

/// `a` beats `b` on the metric by more than the standard error of the
/// difference of the means.
fn beats(a: &[f64], b: &[f64]) -> (bool, f64, f64, f64) {
    let (ma, sa) = mean_se(a);
    let (mb, sb) = mean_se(b);
    let margin = (sa * sa + sb * sb).sqrt();
    (ma - mb > margin, ma, mb, margin)
}

#[test]
fn greybox_skill_levels_order_on_score_and_ball_time() {
    let novice = games(SkillLevel::Novice);
    let standard = games(SkillLevel::Standard);
    let expert = games(SkillLevel::Expert);
    let score = |g: &[Game]| g.iter().map(|x| x.score).collect::<Vec<_>>();
    let ball = |g: &[Game]| g.iter().map(|x| x.ball_ms).collect::<Vec<_>>();

    for (name, g) in [
        ("novice", &novice),
        ("standard", &standard),
        ("expert", &expert),
    ] {
        let (sm, sse) = mean_se(&score(g));
        let (bm, bse) = mean_se(&ball(g));
        eprintln!(
            "{name}: mean score {sm:.0} (SE {sse:.0}), mean ball time {bm:.0} ms (SE {bse:.0})"
        );
    }
    let s = beats(&score(&expert), &score(&novice));
    assert!(
        s.0,
        "expert mean score {:.0} does not beat novice {:.0} by more than 1 SE ({:.0})\n\
         novice={novice:?}\nexpert={expert:?}",
        s.1, s.2, s.3
    );
    let b = beats(&ball(&expert), &ball(&novice));
    assert!(
        b.0,
        "expert mean ball time {:.0} ms does not beat novice {:.0} ms by more than 1 SE ({:.0})\n\
         novice={novice:?}\nexpert={expert:?}",
        b.1, b.2, b.3
    );
    // The Standard is not asserted: at this sample size its mean score is
    // not robustly more than 1 SE from either neighbour (seeds 1+2: 96,423
    // vs novice 87,607, SE 13,339), so it is printed above only. The
    // 100-game medians order on every table (decision record).
}
