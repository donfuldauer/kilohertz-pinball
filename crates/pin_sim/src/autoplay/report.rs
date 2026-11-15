//! Soak findings and the report: JSON (stable key order, `"schema": 1`) and
//! Markdown.
//!
//! Everything but the [`Timing`] section is a pure function of the bundle
//! and the [`SoakConfig`](super::SoakConfig) minus its thread count, so two
//! runs with the same seed give byte-identical
//! [`to_json_deterministic`](SoakReport::to_json_deterministic) and
//! [`to_markdown_deterministic`](SoakReport::to_markdown_deterministic)
//! output. The full [`to_json`](SoakReport::to_json) /
//! [`to_markdown`](SoakReport::to_markdown) add the wall-clock `timing`
//! section last.
//!
//! # Failure policy
//!
//! [`SoakReport::passed`] is false (and `pin_sim soak` exits 1) when any
//! finding of a [failing kind](FindingKind::fails_soak) exists (StuckBall,
//! EscapedBall, NonFinite, ScriptError, QueueOverflow) or the coverage is
//! incomplete: a declared switch never hit (UnreachableSwitch), a shot never
//! completed (ShotCoverage) or a mode never observed started
//! (ModeCoverage). TickBudget is wall-clock and lives in the timing section;
//! it is reported but never fails the soak (it is not reproducible across
//! machines; Gate A1 checks it separately).

use core::fmt::Write as _;

use super::bot::{BotStats, SkillLevel};

/// Schema version of the JSON report.
pub const REPORT_SCHEMA: u32 = 1;

/// `game` of a soak-wide finding (coverage, which is judged over all
/// games); written as `null` in JSON and `all` in Markdown.
pub const SOAK_WIDE: u32 = u32::MAX;

/// What a [`Finding`] is about.
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum FindingKind {
    /// A ball slower than 1 cm/s for more than 10 s outside a trough, lock,
    /// held cradle, holding magnet or plunger lane.
    StuckBall,
    /// A ball left the table's bounds (collider bounds plus a margin, or
    /// below the floor).
    EscapedBall,
    /// NaN or infinity in a ball or flipper state.
    NonFinite,
    /// A declared switch was never hit across the soak.
    UnreachableSwitch,
    /// A `[[shots]]` sequence was never completed within its window.
    ShotCoverage,
    /// A `[[modes]]` entry was never observed started.
    ModeCoverage,
    /// The rules script faulted.
    ScriptError,
    /// A command queue or event buffer overflowed.
    QueueOverflow,
    /// The 99.9th percentile tick time exceeded the budget (timing section
    /// only).
    TickBudget,
}

impl FindingKind {
    /// Every kind, in report order.
    pub const ALL: [FindingKind; 9] = [
        FindingKind::StuckBall,
        FindingKind::EscapedBall,
        FindingKind::NonFinite,
        FindingKind::UnreachableSwitch,
        FindingKind::ShotCoverage,
        FindingKind::ModeCoverage,
        FindingKind::ScriptError,
        FindingKind::QueueOverflow,
        FindingKind::TickBudget,
    ];

    /// The kind's name as written in reports (`StuckBall`, ...).
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            FindingKind::StuckBall => "StuckBall",
            FindingKind::EscapedBall => "EscapedBall",
            FindingKind::NonFinite => "NonFinite",
            FindingKind::UnreachableSwitch => "UnreachableSwitch",
            FindingKind::ShotCoverage => "ShotCoverage",
            FindingKind::ModeCoverage => "ModeCoverage",
            FindingKind::ScriptError => "ScriptError",
            FindingKind::QueueOverflow => "QueueOverflow",
            FindingKind::TickBudget => "TickBudget",
        }
    }

    /// Parses a [`name`](Self::name).
    #[must_use]
    pub fn from_name(s: &str) -> Option<FindingKind> {
        FindingKind::ALL.into_iter().find(|k| k.name() == s)
    }

    /// Whether a finding of this kind fails the soak by itself.
    #[must_use]
    pub fn fails_soak(self) -> bool {
        matches!(
            self,
            FindingKind::StuckBall
                | FindingKind::EscapedBall
                | FindingKind::NonFinite
                | FindingKind::ScriptError
                | FindingKind::QueueOverflow
        )
    }

    /// Whether this kind reports incomplete coverage (which also fails the
    /// soak).
    #[must_use]
    pub fn is_coverage(self) -> bool {
        matches!(
            self,
            FindingKind::UnreachableSwitch | FindingKind::ShotCoverage | FindingKind::ModeCoverage
        )
    }
}

/// One health-check result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Finding {
    /// What kind.
    pub kind: FindingKind,
    /// The exact switch, shot, mode, ball, flipper, queue or position
    /// (`<nearest feature> @ (x, y, z) mm`) it is about.
    pub subject: String,
    /// The game it happened in (0-based), or [`SOAK_WIDE`].
    pub game: u32,
    /// The tick it was detected in (0 for soak-wide findings).
    pub tick: u64,
    /// Human-readable detail.
    pub detail: String,
}

/// Summary statistics of a sample (nearest-rank percentiles).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct Distribution {
    /// Sample size.
    pub count: u64,
    /// Smallest value (0 when empty).
    pub min: i64,
    /// 50th percentile.
    pub median: i64,
    /// 90th percentile.
    pub p90: i64,
    /// 99th percentile.
    pub p99: i64,
    /// Largest value.
    pub max: i64,
}

/// The nearest-rank `q` quantile (0..=1) of an ascending, non-empty slice:
/// the value at rank `ceil(q·n)` (1-based, at least 1).
#[must_use]
pub fn nearest_rank<T: Copy>(sorted: &[T], q: f64) -> T {
    let n = sorted.len();
    let rank = (q * n as f64).ceil() as usize;
    sorted[rank.clamp(1, n) - 1]
}

impl Distribution {
    /// The statistics of `values` (any order).
    #[must_use]
    pub fn of(values: &[i64]) -> Distribution {
        if values.is_empty() {
            return Distribution::default();
        }
        let mut v = values.to_vec();
        v.sort_unstable();
        Distribution {
            count: v.len() as u64,
            min: v[0],
            median: nearest_rank(&v, 0.5),
            p90: nearest_rank(&v, 0.9),
            p99: nearest_rank(&v, 0.99),
            max: v[v.len() - 1],
        }
    }
}

/// Hits of one declared switch across the soak.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SwitchCount {
    /// Switch name.
    pub name: String,
    /// Closing edges over all games.
    pub hits: u64,
}

/// Completions of one shot across the soak.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShotCount {
    /// Shot id.
    pub name: String,
    /// Completed sequences over all games.
    pub count: u64,
}

/// Starts of one mode across the soak.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModeCount {
    /// Mode id.
    pub name: String,
    /// Observed starts over all games.
    pub starts: u64,
    /// Whether the table declares the `mode:<id>` persist key the start is
    /// observed through.
    pub observable: bool,
}

/// Why a game ended.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum GameEnd {
    /// No ball on the table for the configured idle time after play began.
    GameOver,
    /// The game reached `max_game_ticks`.
    MaxTicks,
    /// Every ball on the table was reported stuck.
    Stuck,
    /// No ball ever appeared within the start timeout.
    NoBall,
}

impl GameEnd {
    /// Name as written in reports.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            GameEnd::GameOver => "game_over",
            GameEnd::MaxTicks => "max_ticks",
            GameEnd::Stuck => "stuck",
            GameEnd::NoBall => "no_ball",
        }
    }
}

/// One game's summary.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GameSummary {
    /// Game index.
    pub game: u32,
    /// Its simulation seed.
    pub seed: u64,
    /// Ticks played.
    pub ticks: u64,
    /// Why it ended.
    pub end: GameEnd,
    /// Final [`Simulation::state_hash`](crate::Simulation::state_hash).
    pub state_hash: u64,
    /// Player 1's last reported score.
    pub score: Option<i64>,
    /// Balls that left the table during the game.
    pub balls: u32,
}

/// Simulation counters summed over all games.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct CounterTotals {
    /// Switch edges dropped.
    pub edges_dropped: u64,
    /// Haptic events dropped.
    pub haptics_dropped: u64,
    /// Input events dropped.
    pub inputs_dropped: u64,
    /// Failed ball spawns.
    pub spawns_failed: u64,
    /// Ball events dropped.
    pub ball_events_dropped: u64,
    /// Balls drained.
    pub balls_drained: u64,
    /// Balls removed as escaped.
    pub balls_escaped: u64,
    /// Script commands dropped (queue overflow).
    pub script_queue_overflows: u64,
    /// Script faults.
    pub script_faults: u64,
}

impl CounterTotals {
    /// Adds `o` field by field (saturating).
    pub fn add(&mut self, o: &CounterTotals) {
        let f = |a: &mut u64, b: u64| *a = a.saturating_add(b);
        f(&mut self.edges_dropped, o.edges_dropped);
        f(&mut self.haptics_dropped, o.haptics_dropped);
        f(&mut self.inputs_dropped, o.inputs_dropped);
        f(&mut self.spawns_failed, o.spawns_failed);
        f(&mut self.ball_events_dropped, o.ball_events_dropped);
        f(&mut self.balls_drained, o.balls_drained);
        f(&mut self.balls_escaped, o.balls_escaped);
        f(&mut self.script_queue_overflows, o.script_queue_overflows);
        f(&mut self.script_faults, o.script_faults);
    }
}

/// Wall-clock measurements (excluded from determinism comparisons).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Timing {
    /// Worker threads used.
    pub threads: usize,
    /// Whether tick times were measured.
    pub measured: bool,
    /// Ticks measured.
    pub ticks_measured: u64,
    /// Wall time of the whole soak, s.
    pub wall_s: f64,
    /// Games per wall-clock second.
    pub games_per_s: f64,
    /// Simulated ticks per wall-clock second (all threads).
    pub ticks_per_s: f64,
    /// Median tick time, µs.
    pub tick_p50_us: f64,
    /// 99th percentile tick time, µs.
    pub tick_p99_us: f64,
    /// 99.9th percentile tick time, µs.
    pub tick_p999_us: f64,
    /// Longest tick, µs.
    pub tick_max_us: f64,
    /// Budget for the 99.9th percentile, ms.
    pub budget_ms: f64,
    /// Whether the budget was checked.
    pub budget_checked: bool,
    /// TickBudget findings.
    pub findings: Vec<Finding>,
}

/// The result of [`run_soak`](super::run_soak).
#[derive(Clone, Debug, PartialEq)]
pub struct SoakReport {
    /// `meta.id` of the table.
    pub table_id: String,
    /// [`Simulation::table_hash`](crate::Simulation::table_hash).
    pub table_hash: u64,
    /// Games played.
    pub games: u32,
    /// Soak seed.
    pub seed: u64,
    /// Bot skill.
    pub skill: SkillLevel,
    /// Tick limit per game.
    pub max_game_ticks: u64,
    /// Findings (per game in game order, then the soak-wide coverage ones).
    pub findings: Vec<Finding>,
    /// Hits per declared (non-cabinet) switch, in declaration order.
    pub switches: Vec<SwitchCount>,
    /// Shot completions, in declaration order.
    pub shots: Vec<ShotCount>,
    /// Mode starts, in declaration order.
    pub modes: Vec<ModeCount>,
    /// Time each ball spent on the table (balls that left it), ms.
    pub ball_time_ms: Distribution,
    /// Player 1's final score per game (games without a reported score
    /// count as 0).
    pub game_score: Distribution,
    /// Simulation counters.
    pub counters: CounterTotals,
    /// What the bots did.
    pub bot: BotStats,
    /// Per-game summaries, in game order.
    pub game_list: Vec<GameSummary>,
    /// Wall-clock measurements.
    pub timing: Timing,
}

/// A JSON string literal for `s`.
#[must_use]
pub fn json_string(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// A finite JSON number with 3 decimals (non-finite as `null`).
fn json_f64(v: f64) -> String {
    if v.is_finite() {
        format!("{v:.3}")
    } else {
        "null".into()
    }
}

fn hex(v: u64) -> String {
    format!("\"{v:016x}\"")
}

fn md_cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn finding_json(f: &Finding) -> String {
    let game = if f.game == SOAK_WIDE {
        "null".to_owned()
    } else {
        f.game.to_string()
    };
    format!(
        "{{\"kind\": {}, \"subject\": {}, \"game\": {game}, \"tick\": {}, \"detail\": {}}}",
        json_string(f.kind.name()),
        json_string(&f.subject),
        f.tick,
        json_string(&f.detail)
    )
}

fn dist_json(d: &Distribution) -> String {
    format!(
        "{{\"count\": {}, \"min\": {}, \"median\": {}, \"p90\": {}, \"p99\": {}, \"max\": {}}}",
        d.count, d.min, d.median, d.p90, d.p99, d.max
    )
}

fn list<T>(items: &[T], indent: &str, f: impl Fn(&T) -> String) -> String {
    if items.is_empty() {
        return "[]".into();
    }
    let mut o = String::from("[\n");
    for (i, it) in items.iter().enumerate() {
        o.push_str(indent);
        o.push_str("  ");
        o.push_str(&f(it));
        if i + 1 < items.len() {
            o.push(',');
        }
        o.push('\n');
    }
    o.push_str(indent);
    o.push(']');
    o
}

impl SoakReport {
    /// Findings that fail the soak by their kind.
    pub fn failing_findings(&self) -> impl Iterator<Item = &Finding> {
        self.findings.iter().filter(|f| f.kind.fails_soak())
    }

    /// Findings of `kind` (timing findings included for TickBudget).
    pub fn findings_of(&self, kind: FindingKind) -> impl Iterator<Item = &Finding> {
        self.findings
            .iter()
            .chain(&self.timing.findings)
            .filter(move |f| f.kind == kind)
    }

    /// Whether every switch, shot and mode was covered.
    #[must_use]
    pub fn coverage_complete(&self) -> bool {
        !self.findings.iter().any(|f| f.kind.is_coverage())
    }

    /// Whether the soak passes (see the [module docs](self)).
    #[must_use]
    pub fn passed(&self) -> bool {
        self.failing_findings().next().is_none() && self.coverage_complete()
    }

    /// Names of the declared switches never hit, in declaration order.
    #[must_use]
    pub fn never_hit(&self) -> Vec<&str> {
        self.switches
            .iter()
            .filter(|s| s.hits == 0)
            .map(|s| s.name.as_str())
            .collect()
    }

    /// The final state hash of every game, in game order.
    #[must_use]
    pub fn state_hashes(&self) -> Vec<u64> {
        self.game_list.iter().map(|g| g.state_hash).collect()
    }

    fn findings_by_kind(&self) -> Vec<(FindingKind, usize)> {
        FindingKind::ALL
            .into_iter()
            .map(|k| (k, self.findings.iter().filter(|f| f.kind == k).count()))
            .collect()
    }

    fn json_body(&self) -> String {
        let mut o = String::new();
        let _ = writeln!(o, "{{");
        let _ = writeln!(o, "  \"schema\": {REPORT_SCHEMA},");
        let _ = writeln!(
            o,
            "  \"table\": {{\"id\": {}, \"hash\": {}}},",
            json_string(&self.table_id),
            hex(self.table_hash)
        );
        let _ = writeln!(
            o,
            "  \"config\": {{\"games\": {}, \"seed\": {}, \"skill\": {}, \"max_game_ticks\": {}}},",
            self.games,
            self.seed,
            json_string(self.skill.name()),
            self.max_game_ticks
        );
        let by_kind = self
            .findings_by_kind()
            .iter()
            .map(|(k, n)| format!("{}: {n}", json_string(k.name())))
            .collect::<Vec<_>>()
            .join(", ");
        let ticks: u64 = self.game_list.iter().map(|g| g.ticks).sum();
        let balls: u64 = self.game_list.iter().map(|g| u64::from(g.balls)).sum();
        let _ = writeln!(
            o,
            "  \"summary\": {{\"passed\": {}, \"coverage_complete\": {}, \"games\": {}, \"ticks\": {ticks}, \"balls\": {balls}, \"findings\": {}, \"findings_by_kind\": {{{by_kind}}}}},",
            self.passed(),
            self.coverage_complete(),
            self.game_list.len(),
            self.findings.len()
        );
        let _ = writeln!(
            o,
            "  \"findings\": {},",
            list(&self.findings, "  ", finding_json)
        );
        let never = self
            .never_hit()
            .iter()
            .map(|n| json_string(n))
            .collect::<Vec<_>>()
            .join(", ");
        let _ = writeln!(o, "  \"coverage\": {{");
        let _ = writeln!(o, "    \"never_hit\": [{never}],");
        let _ = writeln!(
            o,
            "    \"switches\": {},",
            list(&self.switches, "    ", |s| format!(
                "{{\"name\": {}, \"hits\": {}}}",
                json_string(&s.name),
                s.hits
            ))
        );
        let _ = writeln!(
            o,
            "    \"shots\": {},",
            list(&self.shots, "    ", |s| format!(
                "{{\"id\": {}, \"count\": {}}}",
                json_string(&s.name),
                s.count
            ))
        );
        let _ = writeln!(
            o,
            "    \"modes\": {}",
            list(&self.modes, "    ", |m| format!(
                "{{\"id\": {}, \"starts\": {}, \"observable\": {}}}",
                json_string(&m.name),
                m.starts,
                m.observable
            ))
        );
        let _ = writeln!(o, "  }},");
        let _ = writeln!(
            o,
            "  \"distributions\": {{\"ball_time_ms\": {}, \"game_score\": {}}},",
            dist_json(&self.ball_time_ms),
            dist_json(&self.game_score)
        );
        let c = &self.counters;
        let _ = writeln!(
            o,
            "  \"counters\": {{\"edges_dropped\": {}, \"haptics_dropped\": {}, \"inputs_dropped\": {}, \"spawns_failed\": {}, \"ball_events_dropped\": {}, \"balls_drained\": {}, \"balls_escaped\": {}, \"script_queue_overflows\": {}, \"script_faults\": {}}},",
            c.edges_dropped,
            c.haptics_dropped,
            c.inputs_dropped,
            c.spawns_failed,
            c.ball_events_dropped,
            c.balls_drained,
            c.balls_escaped,
            c.script_queue_overflows,
            c.script_faults
        );
        let b = &self.bot;
        let _ = writeln!(
            o,
            "  \"bot\": {{\"predictions\": {}, \"flips_on_prediction\": {}, \"misses\": {}, \"cradles\": {}, \"launches\": {}, \"nudges\": {}, \"starts\": {}}},",
            b.predictions,
            b.flips_on_prediction,
            b.misses,
            b.cradles,
            b.launches,
            b.nudges,
            b.starts
        );
        let _ = write!(
            o,
            "  \"games\": {}",
            list(&self.game_list, "  ", |g| format!(
                "{{\"game\": {}, \"seed\": {}, \"ticks\": {}, \"end\": {}, \"state_hash\": {}, \"score\": {}, \"balls\": {}}}",
                g.game,
                g.seed,
                g.ticks,
                json_string(g.end.name()),
                hex(g.state_hash),
                g.score.map_or_else(|| "null".to_owned(), |s| s.to_string()),
                g.balls
            ))
        );
        o
    }

    fn timing_json(&self) -> String {
        let t = &self.timing;
        format!(
            "  \"timing\": {{\"threads\": {}, \"measured\": {}, \"ticks_measured\": {}, \"wall_s\": {}, \"games_per_s\": {}, \"ticks_per_s\": {}, \"tick_p50_us\": {}, \"tick_p99_us\": {}, \"tick_p999_us\": {}, \"tick_max_us\": {}, \"budget_ms\": {}, \"budget_checked\": {}, \"findings\": {}}}",
            t.threads,
            t.measured,
            t.ticks_measured,
            json_f64(t.wall_s),
            json_f64(t.games_per_s),
            json_f64(t.ticks_per_s),
            json_f64(t.tick_p50_us),
            json_f64(t.tick_p99_us),
            json_f64(t.tick_p999_us),
            json_f64(t.tick_max_us),
            json_f64(t.budget_ms),
            t.budget_checked,
            list(&t.findings, "  ", finding_json)
        )
    }

    /// The JSON report without the `timing` section: identical for the
    /// same bundle, seed, games, skill and limits, whatever the thread
    /// count.
    #[must_use]
    pub fn to_json_deterministic(&self) -> String {
        let mut o = self.json_body();
        o.push_str("\n}\n");
        o
    }

    /// The full JSON report (`timing` last).
    #[must_use]
    pub fn to_json(&self) -> String {
        let mut o = self.json_body();
        o.push_str(",\n");
        o.push_str(&self.timing_json());
        o.push_str("\n}\n");
        o
    }

    fn markdown_body(&self) -> String {
        let mut o = String::new();
        let _ = writeln!(o, "# Soak report: {}\n", md_cell(&self.table_id));
        let verdict = if self.passed() { "PASS" } else { "FAIL" };
        let ticks: u64 = self.game_list.iter().map(|g| g.ticks).sum();
        let balls: u64 = self.game_list.iter().map(|g| u64::from(g.balls)).sum();
        let _ = writeln!(o, "## Summary\n");
        let _ = writeln!(o, "| item | value |\n|---|---|");
        let _ = writeln!(o, "| result | **{verdict}** |");
        let _ = writeln!(o, "| table hash | `{:016x}` |", self.table_hash);
        let _ = writeln!(o, "| games | {} |", self.games);
        let _ = writeln!(o, "| seed | {} |", self.seed);
        let _ = writeln!(o, "| skill | {} |", self.skill.name());
        let _ = writeln!(o, "| max ticks per game | {} |", self.max_game_ticks);
        let _ = writeln!(o, "| ticks simulated | {ticks} |");
        let _ = writeln!(o, "| balls played | {balls} |");
        let _ = writeln!(
            o,
            "| coverage complete | {} |",
            if self.coverage_complete() {
                "yes"
            } else {
                "no"
            }
        );
        for (k, n) in self.findings_by_kind() {
            if k != FindingKind::TickBudget {
                let _ = writeln!(o, "| {} findings | {n} |", k.name());
            }
        }
        let _ = writeln!(o, "\n## Findings\n");
        if self.findings.is_empty() {
            let _ = writeln!(o, "None.");
        }
        for k in FindingKind::ALL {
            let fs: Vec<&Finding> = self.findings.iter().filter(|f| f.kind == k).collect();
            if fs.is_empty() {
                continue;
            }
            let _ = writeln!(o, "### {} ({})\n", k.name(), fs.len());
            let _ = writeln!(o, "| subject | game | tick | detail |\n|---|---|---|---|");
            for f in fs {
                let game = if f.game == SOAK_WIDE {
                    "all".to_owned()
                } else {
                    f.game.to_string()
                };
                let _ = writeln!(
                    o,
                    "| {} | {game} | {} | {} |",
                    md_cell(&f.subject),
                    f.tick,
                    md_cell(&f.detail)
                );
            }
            let _ = writeln!(o);
        }
        let _ = writeln!(o, "\n## Coverage\n");
        let never = self.never_hit();
        let _ = writeln!(
            o,
            "Never-hit switches: {}\n",
            if never.is_empty() {
                "none".to_owned()
            } else {
                never.join(", ")
            }
        );
        let _ = writeln!(o, "| switch | hits |\n|---|---|");
        for s in &self.switches {
            let _ = writeln!(o, "| {} | {} |", md_cell(&s.name), s.hits);
        }
        let _ = writeln!(o, "\n| shot | completions |\n|---|---|");
        for s in &self.shots {
            let _ = writeln!(o, "| {} | {} |", md_cell(&s.name), s.count);
        }
        let _ = writeln!(o, "\n| mode | starts | observable |\n|---|---|---|");
        for m in &self.modes {
            let _ = writeln!(
                o,
                "| {} | {} | {} |",
                md_cell(&m.name),
                m.starts,
                if m.observable { "yes" } else { "no" }
            );
        }
        let _ = writeln!(o, "\n## Distributions\n");
        let _ = writeln!(
            o,
            "| quantity | count | min | median | p90 | p99 | max |\n|---|---|---|---|---|---|---|"
        );
        for (name, d) in [
            ("ball time (ms)", &self.ball_time_ms),
            ("game score", &self.game_score),
        ] {
            let _ = writeln!(
                o,
                "| {name} | {} | {} | {} | {} | {} | {} |",
                d.count, d.min, d.median, d.p90, d.p99, d.max
            );
        }
        let c = &self.counters;
        let b = &self.bot;
        let _ = writeln!(o, "\n## Counters\n");
        let _ = writeln!(o, "| counter | total |\n|---|---|");
        for (n, v) in [
            ("edges dropped", c.edges_dropped),
            ("haptics dropped", c.haptics_dropped),
            ("inputs dropped", c.inputs_dropped),
            ("spawns failed", c.spawns_failed),
            ("ball events dropped", c.ball_events_dropped),
            ("balls drained", c.balls_drained),
            ("balls escaped", c.balls_escaped),
            ("script queue overflows", c.script_queue_overflows),
            ("script faults", c.script_faults),
            ("bot predictions", b.predictions),
            ("bot flips on prediction", b.flips_on_prediction),
            ("bot misses", b.misses),
            ("bot cradles", b.cradles),
            ("bot launches", b.launches),
            ("bot nudges", b.nudges),
            ("bot starts", b.starts),
        ] {
            let _ = writeln!(o, "| {n} | {v} |");
        }
        let _ = writeln!(o, "\n## Games\n");
        let _ = writeln!(
            o,
            "| game | seed | ticks | end | score | balls | state hash |\n|---|---|---|---|---|---|---|"
        );
        for g in &self.game_list {
            let _ = writeln!(
                o,
                "| {} | {} | {} | {} | {} | {} | `{:016x}` |",
                g.game,
                g.seed,
                g.ticks,
                g.end.name(),
                g.score.map_or_else(|| "-".to_owned(), |s| s.to_string()),
                g.balls,
                g.state_hash
            );
        }
        o
    }

    /// The Markdown report without the timing section.
    #[must_use]
    pub fn to_markdown_deterministic(&self) -> String {
        self.markdown_body()
    }

    /// The full Markdown report (timing last).
    #[must_use]
    pub fn to_markdown(&self) -> String {
        let mut o = self.markdown_body();
        let t = &self.timing;
        let _ = writeln!(o, "\n## Timing (wall clock, not deterministic)\n");
        let _ = writeln!(o, "| item | value |\n|---|---|");
        let _ = writeln!(o, "| threads | {} |", t.threads);
        let _ = writeln!(o, "| wall time (s) | {:.3} |", t.wall_s);
        let _ = writeln!(o, "| games/s | {:.3} |", t.games_per_s);
        let _ = writeln!(o, "| ticks/s | {:.0} |", t.ticks_per_s);
        if t.measured {
            let _ = writeln!(o, "| ticks measured | {} |", t.ticks_measured);
            let _ = writeln!(o, "| tick p50 (µs) | {:.1} |", t.tick_p50_us);
            let _ = writeln!(o, "| tick p99 (µs) | {:.1} |", t.tick_p99_us);
            let _ = writeln!(o, "| tick p99.9 (µs) | {:.1} |", t.tick_p999_us);
            let _ = writeln!(o, "| tick max (µs) | {:.1} |", t.tick_max_us);
        }
        let _ = writeln!(
            o,
            "| p99.9 budget (ms) | {:.3}{} |",
            t.budget_ms,
            if t.budget_checked {
                ""
            } else {
                " (not checked)"
            }
        );
        for f in &t.findings {
            let _ = writeln!(
                o,
                "\n**{}** {}: {}",
                f.kind.name(),
                md_cell(&f.subject),
                md_cell(&f.detail)
            );
        }
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn distribution_nearest_rank() {
        let d = Distribution::of(&[5, 1, 4, 2, 3]);
        assert_eq!(
            d,
            Distribution {
                count: 5,
                min: 1,
                median: 3,
                p90: 5,
                p99: 5,
                max: 5
            }
        );
        assert_eq!(Distribution::of(&[]), Distribution::default());
        let v: Vec<i64> = (1..=100).collect();
        let d = Distribution::of(&v);
        assert_eq!((d.median, d.p90, d.p99), (50, 90, 99));
    }

    #[test]
    fn json_string_escapes() {
        assert_eq!(
            json_string("a\"b\\c\n\t\r\u{1}"),
            "\"a\\\"b\\\\c\\n\\t\\r\\u0001\""
        );
        assert_eq!(json_f64(f64::NAN), "null");
        assert_eq!(json_f64(1.0), "1.000");
    }

    #[test]
    fn kinds_round_trip_and_policy() {
        for k in FindingKind::ALL {
            assert_eq!(FindingKind::from_name(k.name()), Some(k));
            assert!(!(k.fails_soak() && k.is_coverage()));
        }
        assert_eq!(FindingKind::from_name("nope"), None);
        assert!(!FindingKind::TickBudget.fails_soak());
        assert!(!FindingKind::TickBudget.is_coverage());
        for e in [
            GameEnd::GameOver,
            GameEnd::MaxTicks,
            GameEnd::Stuck,
            GameEnd::NoBall,
        ] {
            assert!(!e.name().is_empty());
        }
    }

    #[test]
    fn counters_add() {
        let mut a = CounterTotals {
            edges_dropped: u64::MAX,
            ..CounterTotals::default()
        };
        a.add(&CounterTotals {
            edges_dropped: 1,
            script_faults: 2,
            ..CounterTotals::default()
        });
        assert_eq!(a.edges_dropped, u64::MAX);
        assert_eq!(a.script_faults, 2);
    }

    #[test]
    fn md_cell_escapes_pipes() {
        assert_eq!(md_cell("a|b\nc"), "a\\|b c");
    }
}
