//! The soak runner: N seeded bot games per table, headless and in
//! parallel, merged into one [`SoakReport`].
//!
//! Game `i` simulates with [`game_seed`]`(cfg.seed, i)` and its bot draws
//! from [`bot_seed`] of that, both fixed hashes of the soak seed and the
//! game index, so a game does not depend on which thread runs it or in
//! which order. Each game runs on simulated time only (D8): the loop is
//! `bot.tick(&mut sim); sim.advance(1); monitor.observe(&sim)` until the
//! game ends ([`GameEnd`]). Results are merged in game order, so the report
//! is identical for every thread count. The only wall-clock reads are the
//! tick-time measurement around `sim.advance(1)` and the soak's total wall
//! time, both reported in the separate [`Timing`] section.
//!
//! A game stopped at `max_game_ticks` with a ball still on the table gets an
//! [`FindingKind::EndlessBall`] finding for that ball
//! ([`GameMonitor::end_at_cap`]); a ball older than
//! [`SoakConfig::endless_ball_ticks`] gets one as soon as it passes the
//! limit. Balls still on the table when a game ends count in the ball-time
//! distribution at their observed age, as censored samples
//! ([`SoakReport::ball_time_censored`]).

use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use super::bot::{Bot, BotConfig, BotStats, SkillLevel};
use super::monitor::{DEFAULT_ENDLESS_BALL_TICKS, DEFAULT_STUCK_TICKS, GameMonitor, GameOutcome};
use super::report::{
    CounterTotals, Distribution, Finding, FindingKind, GameEnd, GameSummary, ModeCount, SOAK_WIDE,
    ShotCount, SoakReport, SwitchCount, Timing,
};
use super::table::{MODE_PERSIST_PREFIX, TableInfo};
use crate::cli::SIM_THREAD_STACK_BYTES;
use crate::{DEFAULT_SEED, SimConfig, SimError, Simulation};

/// Default 99.9th-percentile tick budget, ms (Gate A1).
pub const DEFAULT_TICK_BUDGET_MS: f64 = 0.35;

/// Tick-time histogram resolution, ns.
pub const TIMING_BUCKET_NS: u64 = 100;

/// Tick-time histogram range, ns (longer ticks share the last bucket and
/// are tracked by the maximum).
pub const TIMING_RANGE_NS: u64 = 10_000_000;

/// Soak settings.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SoakConfig {
    /// Games to play.
    pub games: u32,
    /// Soak seed; game seeds derive from it ([`game_seed`]).
    pub seed: u64,
    /// Bot skill.
    pub skill: SkillLevel,
    /// Worker threads (at least 1; more than `games` are not started).
    pub threads: usize,
    /// Tick limit per game.
    pub max_game_ticks: u64,
    /// A game is over after this many ticks without a ball once one has
    /// been on the table.
    pub idle_end_ticks: u64,
    /// A game in which no ball appears within this many ticks ends.
    pub start_timeout_ticks: u64,
    /// Slow ticks before a ball is reported stuck.
    pub stuck_ticks: u64,
    /// Ball age after which a ball still on the table is an
    /// [`FindingKind::EndlessBall`], ticks.
    pub endless_ball_ticks: u64,
    /// Whether tick times are measured (wall clock).
    pub measure_timing: bool,
    /// Whether a 99.9th percentile above `tick_budget_ms` is a TickBudget
    /// finding.
    pub check_tick_budget: bool,
    /// The 99.9th-percentile tick budget, ms.
    pub tick_budget_ms: f64,
    /// Bot behaviour.
    pub bot: BotConfig,
    /// Simulation limits (the seed is replaced per game).
    pub sim: SimConfig,
}

impl Default for SoakConfig {
    /// 100 games, [`DEFAULT_SEED`], Standard skill, 1 thread, at most 10
    /// minutes of simulated time per game, game over after 5 s without a
    /// ball, 10 s start timeout, 10 s stuck time, 300 s endless-ball limit,
    /// timing measured and the 0.35 ms budget checked.
    fn default() -> Self {
        Self {
            games: 100,
            seed: DEFAULT_SEED,
            skill: SkillLevel::Standard,
            threads: 1,
            max_game_ticks: 600_000,
            idle_end_ticks: 5_000,
            start_timeout_ticks: 10_000,
            stuck_ticks: DEFAULT_STUCK_TICKS,
            endless_ball_ticks: DEFAULT_ENDLESS_BALL_TICKS,
            measure_timing: true,
            check_tick_budget: true,
            tick_budget_ms: DEFAULT_TICK_BUDGET_MS,
            bot: BotConfig::default(),
            sim: SimConfig::default(),
        }
    }
}

/// SplitMix64 finaliser.
#[must_use]
pub fn splitmix64(mut z: u64) -> u64 {
    z = z.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// The simulation seed of game `game` of a soak seeded `seed`.
#[must_use]
pub fn game_seed(seed: u64, game: u32) -> u64 {
    splitmix64(seed ^ splitmix64(u64::from(game).wrapping_add(1)))
}

/// The bot seed of a game with simulation seed `game_seed`.
#[must_use]
pub fn bot_seed(game_seed: u64) -> u64 {
    splitmix64(game_seed ^ 0x426F_7453_6565_6421)
}

/// A tick-time histogram ([`TIMING_BUCKET_NS`] buckets up to
/// [`TIMING_RANGE_NS`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TickHistogram {
    buckets: Vec<u64>,
    count: u64,
    max_ns: u64,
}

impl Default for TickHistogram {
    fn default() -> Self {
        Self::new()
    }
}

impl TickHistogram {
    /// An empty histogram.
    #[must_use]
    pub fn new() -> Self {
        Self {
            buckets: vec![0; (TIMING_RANGE_NS / TIMING_BUCKET_NS) as usize + 1],
            count: 0,
            max_ns: 0,
        }
    }

    /// Records one tick of `ns` nanoseconds.
    pub fn record(&mut self, ns: u64) {
        let i = ((ns / TIMING_BUCKET_NS) as usize).min(self.buckets.len() - 1);
        self.buckets[i] += 1;
        self.count += 1;
        self.max_ns = self.max_ns.max(ns);
    }

    /// Adds `o`'s samples.
    pub fn merge(&mut self, o: &TickHistogram) {
        for (a, b) in self.buckets.iter_mut().zip(&o.buckets) {
            *a += b;
        }
        self.count += o.count;
        self.max_ns = self.max_ns.max(o.max_ns);
    }

    /// Samples recorded.
    #[must_use]
    pub fn count(&self) -> u64 {
        self.count
    }

    /// Longest sample, ns.
    #[must_use]
    pub fn max_ns(&self) -> u64 {
        self.max_ns
    }

    /// The nearest-rank `q` quantile, ns: the upper edge of the bucket
    /// holding it (capped by the maximum; the maximum itself for the
    /// overflow bucket); 0 when empty.
    #[must_use]
    pub fn quantile_ns(&self, q: f64) -> u64 {
        if self.count == 0 {
            return 0;
        }
        let rank = ((q * self.count as f64).ceil() as u64).clamp(1, self.count);
        let mut seen = 0u64;
        for (i, &n) in self.buckets.iter().enumerate() {
            seen += n;
            if seen >= rank {
                if i + 1 == self.buckets.len() {
                    // The overflow bucket: only the maximum is known.
                    return self.max_ns;
                }
                return ((i as u64 + 1) * TIMING_BUCKET_NS).min(self.max_ns);
            }
        }
        self.max_ns
    }
}

/// The [`FindingKind::TickBudget`] finding when the 99.9th-percentile tick
/// time in `hist` exceeds `budget_ms`; `None` for an empty histogram or a
/// tick time within budget. A pure comparison, kept apart from the timing
/// measurement.
#[must_use]
pub fn tick_budget_finding(hist: &TickHistogram, budget_ms: f64) -> Option<Finding> {
    if hist.count() == 0 {
        return None;
    }
    let p999_us = hist.quantile_ns(0.999) as f64 / 1000.0;
    if p999_us <= budget_ms * 1000.0 {
        return None;
    }
    Some(Finding {
        kind: FindingKind::TickBudget,
        subject: "tick p99.9".to_owned(),
        game: SOAK_WIDE,
        tick: 0,
        detail: format!(
            "99.9th percentile tick time {p999_us:.1} µs exceeds the {budget_ms:.3} ms budget ({} ticks measured)",
            hist.count()
        ),
    })
}

/// One finished game, before merging.
#[derive(Clone, Debug)]
struct GameResult {
    summary: GameSummary,
    outcome: GameOutcome,
    bot: BotStats,
    table_hash: u64,
}

/// Plays game `game` of the soak.
fn play_game(
    bundle: &Path,
    info: &TableInfo,
    cfg: &SoakConfig,
    game: u32,
    hist: &mut TickHistogram,
) -> Result<GameResult, SimError> {
    let seed = game_seed(cfg.seed, game);
    let mut sim = Simulation::load(bundle, SimConfig { seed, ..cfg.sim })?;
    let mut bot = Bot::new(bot_seed(seed), cfg.skill)
        .with_table(info)
        .with_config(cfg.bot);
    let mut mon = GameMonitor::new(info, &sim, game, cfg.stuck_ticks)
        .with_endless_ticks(cfg.endless_ball_ticks);
    mon.observe(&sim);
    let mut idle = 0u64;
    let mut end = GameEnd::MaxTicks;
    while sim.tick() < cfg.max_game_ticks {
        bot.tick(&mut sim);
        if cfg.measure_timing {
            let t0 = Instant::now();
            sim.advance(1);
            hist.record(u64::try_from(t0.elapsed().as_nanos()).unwrap_or(u64::MAX));
        } else {
            sim.advance(1);
        }
        mon.observe(&sim);
        if sim.balls_in_play() == 0 {
            idle += 1;
        } else {
            idle = 0;
        }
        if bot.seen_ball() && sim.balls_in_play() == 0 && idle >= cfg.idle_end_ticks {
            end = GameEnd::GameOver;
            break;
        }
        if !bot.seen_ball() && sim.balls_in_play() == 0 && sim.tick() >= cfg.start_timeout_ticks {
            end = GameEnd::NoBall;
            break;
        }
        if mon.all_balls_stuck() {
            end = GameEnd::Stuck;
            break;
        }
    }
    if end == GameEnd::MaxTicks {
        mon.end_at_cap(&sim);
    }
    let outcome = mon.finish(&sim);
    Ok(GameResult {
        summary: GameSummary {
            game,
            seed,
            ticks: sim.tick(),
            end,
            state_hash: sim.state_hash(),
            score: sim.score(0),
            balls: outcome.ball_times_ms.len() as u32,
        },
        outcome,
        bot: bot.stats(),
        table_hash: sim.table_hash(),
    })
}

/// Plays only game `game` of the soak `cfg` describes, exactly as
/// [`run_soak`] plays it (same seeds, same monitor), on the calling thread
/// without tick timing; returns its summary and outcome. For reproducing
/// one game of a large soak.
///
/// # Errors
/// The [`SimError`] of a bundle or table that does not load.
pub fn play_soak_game(
    bundle: &Path,
    cfg: &SoakConfig,
    game: u32,
) -> Result<(GameSummary, GameOutcome), SimError> {
    let info = TableInfo::load(bundle)?;
    let cfg = SoakConfig {
        measure_timing: false,
        ..*cfg
    };
    let mut hist = TickHistogram::new();
    let r = play_game(bundle, &info, &cfg, game, &mut hist)?;
    Ok((r.summary, r.outcome))
}

type WorkerOut = (Vec<(u32, Result<GameResult, SimError>)>, TickHistogram);

/// Plays `cfg.games` seeded bot games on `bundle` and reports soundness and
/// coverage (see the [module docs](self) and [`SoakReport`]).
///
/// # Errors
/// The first (by game index) [`SimError`] of a game whose bundle does not
/// load, or the table-info load error; a worker thread that cannot start or
/// panics is [`SimError::Build`].
pub fn run_soak(bundle: &Path, cfg: &SoakConfig) -> Result<SoakReport, SimError> {
    let wall = Instant::now();
    let info = TableInfo::load(bundle)?;
    let games = cfg.games;
    let threads = cfg.threads.max(1).min(games.max(1) as usize);
    let next = AtomicU32::new(0);

    let outs: Vec<Result<WorkerOut, SimError>> = std::thread::scope(|s| {
        let mut handles = Vec::with_capacity(threads);
        for w in 0..threads {
            let (info, next) = (&info, &next);
            let h = std::thread::Builder::new()
                .name(format!("soak-{w}"))
                .stack_size(SIM_THREAD_STACK_BYTES)
                .spawn_scoped(s, move || {
                    let mut hist = TickHistogram::new();
                    let mut done = Vec::new();
                    loop {
                        let g = next.fetch_add(1, Ordering::Relaxed);
                        if g >= games {
                            break;
                        }
                        let r = play_game(bundle, info, cfg, g, &mut hist);
                        let failed = r.is_err();
                        done.push((g, r));
                        if failed {
                            break;
                        }
                    }
                    (done, hist)
                });
            handles.push(h);
        }
        handles
            .into_iter()
            .map(|h| match h {
                Ok(j) => j
                    .join()
                    .map_err(|_| SimError::Build("a soak worker panicked".into())),
                Err(e) => Err(SimError::Build(format!("cannot start a soak worker: {e}"))),
            })
            .collect()
    });

    let mut hist = TickHistogram::new();
    let mut results: Vec<Option<Result<GameResult, SimError>>> = (0..games).map(|_| None).collect();
    for out in outs {
        let (done, h) = out?;
        hist.merge(&h);
        for (g, r) in done {
            results[g as usize] = Some(r);
        }
    }
    let mut played = Vec::with_capacity(games as usize);
    for r in results {
        match r {
            Some(Ok(g)) => played.push(g),
            Some(Err(e)) => return Err(e),
            // Only after another game's error stopped its worker.
            None => {}
        }
    }
    if played.len() != games as usize {
        return Err(SimError::Build("a soak game did not run".into()));
    }
    let wall_s = wall.elapsed().as_secs_f64();
    Ok(merge(&info, cfg, threads, played, &hist, wall_s))
}

fn merge(
    info: &TableInfo,
    cfg: &SoakConfig,
    threads: usize,
    played: Vec<GameResult>,
    hist: &TickHistogram,
    wall_s: f64,
) -> SoakReport {
    let cov: Vec<&super::table::SwitchInfo> = info.coverage_switches().collect();
    let mut switch_hits = vec![0u64; cov.len()];
    let mut shot_counts = vec![0u64; info.shots.len()];
    let mut mode_starts = vec![0u64; info.modes.len()];
    let mut findings: Vec<Finding> = Vec::new();
    let mut ball_times = Vec::new();
    let mut censored = 0u64;
    let mut scores = Vec::with_capacity(played.len());
    let mut counters = CounterTotals::default();
    let mut bot = BotStats::default();
    let mut game_list = Vec::with_capacity(played.len());
    let mut table_hash = 0;
    for g in played {
        for (a, b) in switch_hits.iter_mut().zip(&g.outcome.switch_hits) {
            *a += b;
        }
        for (a, b) in shot_counts.iter_mut().zip(&g.outcome.shot_counts) {
            *a += b;
        }
        for (a, b) in mode_starts.iter_mut().zip(&g.outcome.mode_starts) {
            *a += b;
        }
        findings.extend(g.outcome.findings);
        ball_times.extend(g.outcome.ball_times_ms);
        censored += g.outcome.censored_ball_times_ms.len() as u64;
        ball_times.extend(g.outcome.censored_ball_times_ms);
        scores.push(g.summary.score.unwrap_or(0));
        counters.add(&g.outcome.counters);
        bot.add(&g.bot);
        table_hash = g.table_hash;
        game_list.push(g.summary);
    }
    let games = game_list.len();
    let wide = |kind, subject: &str, detail: String| Finding {
        kind,
        subject: subject.to_owned(),
        game: SOAK_WIDE,
        tick: 0,
        detail,
    };
    for (s, &n) in cov.iter().zip(&switch_hits) {
        if n == 0 {
            findings.push(wide(
                FindingKind::UnreachableSwitch,
                &s.name,
                format!("switch {} was never hit in {games} games", s.name),
            ));
        }
    }
    for (s, &n) in info.shots.iter().zip(&shot_counts) {
        if n == 0 {
            let seq: Vec<&str> = s
                .sequence
                .iter()
                .map(|&id| info.switch_name(id).unwrap_or("?"))
                .collect();
            findings.push(wide(
                FindingKind::ShotCoverage,
                &s.name,
                format!(
                    "shot {} ({} within {} ms) was never completed in {games} games",
                    s.name,
                    seq.join(" -> "),
                    s.window_ticks
                ),
            ));
        }
    }
    for (m, &n) in info.modes.iter().zip(&mode_starts) {
        if n == 0 {
            let why = if m.persist_key.is_some() {
                String::new()
            } else {
                format!(
                    "; the table declares no persist key {MODE_PERSIST_PREFIX}{} to observe its start through",
                    m.name
                )
            };
            findings.push(wide(
                FindingKind::ModeCoverage,
                &m.name,
                format!(
                    "mode {} (start scene {}) was never observed started in {games} games{why}",
                    m.name, m.start_scene
                ),
            ));
        }
    }

    let ticks: u64 = game_list.iter().map(|g| g.ticks).sum();
    let us = |ns: u64| ns as f64 / 1000.0;
    let p999 = hist.quantile_ns(0.999);
    let mut timing = Timing {
        threads,
        measured: cfg.measure_timing,
        ticks_measured: hist.count(),
        wall_s,
        games_per_s: if wall_s > 0.0 {
            games as f64 / wall_s
        } else {
            0.0
        },
        ticks_per_s: if wall_s > 0.0 {
            ticks as f64 / wall_s
        } else {
            0.0
        },
        tick_p50_us: us(hist.quantile_ns(0.5)),
        tick_p99_us: us(hist.quantile_ns(0.99)),
        tick_p999_us: us(p999),
        tick_max_us: us(hist.max_ns()),
        budget_ms: cfg.tick_budget_ms,
        budget_checked: cfg.check_tick_budget && cfg.measure_timing,
        findings: Vec::new(),
    };
    if timing.budget_checked
        && let Some(f) = tick_budget_finding(hist, cfg.tick_budget_ms)
    {
        timing.findings.push(f);
    }

    SoakReport {
        table_id: info.table_id.clone(),
        table_hash,
        games: cfg.games,
        seed: cfg.seed,
        skill: cfg.skill,
        max_game_ticks: cfg.max_game_ticks,
        findings,
        switches: cov
            .iter()
            .zip(&switch_hits)
            .map(|(s, &hits)| SwitchCount {
                name: s.name.clone(),
                hits,
            })
            .collect(),
        shots: info
            .shots
            .iter()
            .zip(&shot_counts)
            .map(|(s, &count)| ShotCount {
                name: s.name.clone(),
                count,
            })
            .collect(),
        modes: info
            .modes
            .iter()
            .zip(&mode_starts)
            .map(|(m, &starts)| ModeCount {
                name: m.name.clone(),
                starts,
                observable: m.persist_key.is_some(),
            })
            .collect(),
        ball_time_ms: Distribution::of(&ball_times),
        ball_time_censored: censored,
        game_score: Distribution::of(&scores),
        counters,
        bot,
        game_list,
        timing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeds_are_fixed_hashes() {
        assert_eq!(game_seed(1, 0), game_seed(1, 0));
        assert_ne!(game_seed(1, 0), game_seed(1, 1));
        assert_ne!(game_seed(1, 0), game_seed(2, 0));
        assert_ne!(bot_seed(5), 5);
        // SplitMix64 reference value for input 0.
        assert_eq!(splitmix64(0), 0xE220_A839_7B1D_CDAF);
    }

    #[test]
    fn tick_budget_finding_is_a_pure_comparison() {
        let empty = TickHistogram::new();
        assert!(tick_budget_finding(&empty, 0.0).is_none());
        let mut h = TickHistogram::new();
        h.record(150);
        let f = tick_budget_finding(&h, 0.0).expect("over a zero budget");
        assert_eq!(f.kind, FindingKind::TickBudget);
        assert_eq!(f.game, SOAK_WIDE);
        assert!(tick_budget_finding(&h, 1.0).is_none());
    }

    #[test]
    fn histogram_quantiles() {
        let mut h = TickHistogram::new();
        assert_eq!(h.quantile_ns(0.5), 0);
        for ns in [150, 250, 950, 20_000_000] {
            h.record(ns);
        }
        assert_eq!(h.count(), 4);
        assert_eq!(h.quantile_ns(0.25), 200);
        assert_eq!(h.quantile_ns(0.5), 300);
        assert_eq!(h.quantile_ns(0.75), 1000);
        assert_eq!(h.quantile_ns(1.0), 20_000_000);
        let mut g = TickHistogram::default();
        g.record(50);
        g.merge(&h);
        assert_eq!(g.count(), 5);
        assert_eq!(g.max_ns(), 20_000_000);
        assert_eq!(g.quantile_ns(0.0), 100);
    }
}
