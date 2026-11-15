//! The `pin_sim` command line (Subphase 2.5).
//!
//! ```text
//! pin_sim <bundle> --ticks N [--seed S] [--record-inputs FILE] [--idle]
//! pin_sim <bundle> --replay-inputs FILE
//! ```
//!
//! The first form runs `N` ticks with the built-in input pattern
//! ([`pattern_events`]: start button, both flipper buttons and a nudge on
//! fixed schedules; `--idle` feeds no input) and, with `--record-inputs`,
//! writes the [`Recording`]. The second replays a recording on the bundle.
//! Both print `state_hash=<16 hex digits>` (the final
//! [`Simulation::state_hash`](crate::Simulation::state_hash)) on stdout;
//! diagnostics go to stderr. `S` is decimal or `0x` hex (default
//! [`DEFAULT_SEED`](crate::DEFAULT_SEED)). Exit code 0 on success, 2 for bad
//! arguments, 1 for any other failure.
//!
//! [`run`] does the work on the calling thread; the binary calls it on a
//! thread with [`SIM_THREAD_STACK_BYTES`] of stack (enough for a Wasm
//! script, see [`Simulation`](crate::Simulation)).
//!
//! # `soak` (Subphase 2.9)
//!
//! ```text
//! pin_sim soak <bundle> [--games N] [--seed S] [--skill novice|standard|expert]
//!                       [--threads T] [--max-ticks N] [--no-tick-budget] --out DIR
//! ```
//!
//! Plays `N` (default 100) seeded bot games with
//! [`run_soak`] and writes `DIR/soak_report.json`
//! and `DIR/soak_report.md`. stdout gets a summary naming every finding;
//! exit code 0 when the soak passes, 1 when it fails
//! ([`SoakReport::passed`]: a StuckBall, EscapedBall, NonFinite,
//! ScriptError or QueueOverflow finding, or incomplete switch / shot / mode
//! coverage; the summary then goes to stderr) or cannot run, 2 for bad
//! arguments. A bundle literally named `soak` must be given as `./soak`.

use core::fmt;
use pin_core::input::{CabinetAxis, CabinetInput, InputEvent};
use pin_core::sim::{SimStep, TICK_NS};
use std::path::{Path, PathBuf};

use crate::autoplay::{SkillLevel, SoakConfig, SoakReport, run_soak};
use crate::replay::{Recorder, Recording, replay};
use crate::{SimConfig, Simulation};

/// Stack of the thread the binary runs the simulation on, bytes.
pub const SIM_THREAD_STACK_BYTES: usize = 16 * 1024 * 1024;

/// Usage text.
pub const USAGE: &str = "usage: pin_sim <bundle> --ticks N [--seed S] [--record-inputs FILE] [--idle]\n       pin_sim <bundle> --replay-inputs FILE";

/// Why the command failed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CliError {
    /// Bad arguments (exit code 2).
    Usage(String),
    /// Loading, recording or replaying failed (exit code 1).
    Failed(String),
}

impl CliError {
    /// Process exit code.
    #[must_use]
    pub fn exit_code(&self) -> i32 {
        match self {
            CliError::Usage(_) => 2,
            CliError::Failed(_) => 1,
        }
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CliError::Usage(m) => write!(f, "{m}\n{USAGE}"),
            CliError::Failed(m) => f.write_str(m),
        }
    }
}

impl std::error::Error for CliError {}

/// What the command line asks for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Command {
    /// Run (and optionally record) `ticks` ticks.
    Run {
        /// Bundle path.
        bundle: PathBuf,
        /// Ticks to run.
        ticks: u64,
        /// RNG seed.
        seed: u64,
        /// Where to write the recording.
        record: Option<PathBuf>,
        /// Feed no input events.
        idle: bool,
    },
    /// Replay a recording.
    Replay {
        /// Bundle path.
        bundle: PathBuf,
        /// Recording path.
        file: PathBuf,
    },
}

fn parse_u64(flag: &str, v: &str) -> Result<u64, CliError> {
    let r = match v.strip_prefix("0x").or_else(|| v.strip_prefix("0X")) {
        Some(hex) => u64::from_str_radix(hex, 16),
        None => v.parse(),
    };
    r.map_err(|_| CliError::Usage(format!("{flag}: not a number: {v:?}")))
}

/// Parses the arguments (without the program name).
///
/// # Errors
/// [`CliError::Usage`].
pub fn parse_args(args: &[String]) -> Result<Command, CliError> {
    let mut bundle = None;
    let mut ticks = None;
    let mut seed = None;
    let mut record = None;
    let mut replay_file = None;
    let mut idle = false;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| CliError::Usage(format!("{flag} needs a value")))
        };
        match a.as_str() {
            "--ticks" => ticks = Some(parse_u64("--ticks", &value("--ticks")?)?),
            "--seed" => seed = Some(parse_u64("--seed", &value("--seed")?)?),
            "--record-inputs" => record = Some(PathBuf::from(value("--record-inputs")?)),
            "--replay-inputs" => replay_file = Some(PathBuf::from(value("--replay-inputs")?)),
            "--idle" => idle = true,
            s if s.starts_with("--") => return Err(CliError::Usage(format!("unknown flag {s}"))),
            s => {
                if bundle.replace(PathBuf::from(s)).is_some() {
                    return Err(CliError::Usage("more than one bundle given".into()));
                }
            }
        }
    }
    let bundle = bundle.ok_or_else(|| CliError::Usage("no bundle given".into()))?;
    match (replay_file, ticks) {
        (Some(file), None) => {
            if seed.is_some() || record.is_some() || idle {
                return Err(CliError::Usage(
                    "--replay-inputs takes no --seed, --record-inputs or --idle (the recording has them)".into(),
                ));
            }
            Ok(Command::Replay { bundle, file })
        }
        (None, Some(ticks)) => Ok(Command::Run {
            bundle,
            ticks,
            seed: seed.unwrap_or(crate::DEFAULT_SEED),
            record,
            idle,
        }),
        (Some(_), Some(_)) => Err(CliError::Usage(
            "--ticks and --replay-inputs cannot be combined".into(),
        )),
        (None, None) => Err(CliError::Usage(
            "give --ticks N or --replay-inputs FILE".into(),
        )),
    }
}

/// The built-in input pattern's events for `tick` (1-based), timestamped in
/// the middle of the tick: start pressed on tick 20 and released on 120;
/// left flipper pressed for 90 ticks every 337 (from tick 13); right
/// flipper pressed for 80 ticks every 421 (from tick 200); a nudge of
/// `NudgeX = 0.5` for one tick every 2500 (at 1250).
pub fn pattern_events(tick: u64, out: &mut Vec<InputEvent>) {
    let ts = tick.saturating_sub(1).saturating_mul(TICK_NS) + TICK_NS / 2;
    let button = |out: &mut Vec<InputEvent>, input, pressed| {
        out.push(InputEvent::button(ts, input, pressed));
    };
    match tick {
        20 => button(out, CabinetInput::Start, true),
        120 => button(out, CabinetInput::Start, false),
        _ => {}
    }
    match tick % 337 {
        13 => button(out, CabinetInput::LeftFlipper, true),
        103 => button(out, CabinetInput::LeftFlipper, false),
        _ => {}
    }
    match tick % 421 {
        200 => button(out, CabinetInput::RightFlipper, true),
        280 => button(out, CabinetInput::RightFlipper, false),
        _ => {}
    }
    match tick % 2500 {
        1250 => out.push(InputEvent::axis(ts, CabinetAxis::NudgeX, 0.5)),
        1251 => out.push(InputEvent::axis(ts, CabinetAxis::NudgeX, 0.0)),
        _ => {}
    }
}

/// Records `ticks` ticks of [`pattern_events`] (or no input when `idle`) on
/// `sim`.
#[must_use]
pub fn record_pattern(sim: Simulation, ticks: u64, idle: bool) -> (Simulation, Recording) {
    let mut rec = Recorder::new(sim);
    let mut events = Vec::new();
    for _ in 0..ticks {
        events.clear();
        if !idle {
            pattern_events(rec.sim().tick() + 1, &mut events);
        }
        rec.step(&events);
    }
    rec.finish()
}

/// Runs `ticks` ticks of [`pattern_events`] (or no input when `idle`) on
/// `sim` without recording: the input buffer is reused and no history is
/// kept. Same final state as [`record_pattern`].
#[must_use]
pub fn run_pattern(mut sim: Simulation, ticks: u64, idle: bool) -> Simulation {
    let mut events = Vec::new();
    let mut out = *sim.snapshot();
    for _ in 0..ticks {
        let tick = sim.tick() + 1;
        events.clear();
        if !idle {
            pattern_events(tick, &mut events);
        }
        sim.step_1ms(tick, &events, &mut out);
    }
    sim
}

/// Whether `record` would be written inside the folder bundle `bundle` (or
/// over the bundle file itself). A file there would become part of the
/// bundle's table hash and the recording could not replay. An existing
/// `record` (or a symlink, even a dangling one) is judged by where it
/// resolves to, so a symlink to a bundle file is caught.
fn record_inside_bundle(bundle: &Path, record: &Path) -> bool {
    let Ok(bundle) = bundle.canonicalize() else {
        return false;
    };
    let Some(target) = write_target(record) else {
        return false;
    };
    if bundle.is_dir() {
        target.starts_with(&bundle)
    } else {
        target == bundle
    }
}

/// The canonical path a write to `path` lands on: the path itself with every
/// symlink followed when it exists, else its canonical parent joined with
/// its file name (following dangling symlinks first). `None` if that cannot
/// be worked out (missing parent, symlink loop).
fn write_target(path: &Path) -> Option<PathBuf> {
    let mut p = path.to_path_buf();
    // Bounded like the kernel's own symlink-follow limit.
    for _ in 0..40 {
        if let Ok(c) = p.canonicalize() {
            return Some(c);
        }
        let parent = match p.parent() {
            Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
            _ => PathBuf::from("."),
        };
        match std::fs::read_link(&p) {
            Ok(link) => p = parent.join(link),
            Err(_) => return Some(parent.canonicalize().ok()?.join(p.file_name()?)),
        }
    }
    None
}

/// Runs a parsed command; returns the final state hash.
///
/// # Errors
/// [`CliError::Failed`] for load, record, write or replay failures.
pub fn run(cmd: &Command) -> Result<u64, CliError> {
    let fail = |e: &dyn fmt::Display| CliError::Failed(e.to_string());
    match cmd {
        Command::Run {
            bundle,
            ticks,
            seed,
            record,
            idle,
        } => {
            if let Some(path) = record
                && record_inside_bundle(bundle, path)
            {
                return Err(CliError::Usage(format!(
                    "--record-inputs {} is inside the bundle {}; a recording there changes the \
                     table hash and cannot be replayed",
                    path.display(),
                    bundle.display()
                )));
            }
            let sim = Simulation::load(
                bundle,
                SimConfig {
                    seed: *seed,
                    ..SimConfig::default()
                },
            )
            .map_err(|e| fail(&e))?;
            let Some(path) = record else {
                return Ok(run_pattern(sim, *ticks, *idle).state_hash());
            };
            let (sim, recording) = record_pattern(sim, *ticks, *idle);
            recording.save(path).map_err(|e| fail(&e))?;
            Ok(sim.state_hash())
        }
        Command::Replay { bundle, file } => {
            let recording = Recording::load(file).map_err(|e| fail(&e))?;
            replay(bundle, &recording).map_err(|e| fail(&e))
        }
    }
}

/// The stdout line for a state hash.
#[must_use]
pub fn hash_line(hash: u64) -> String {
    format!("state_hash={hash:016x}")
}

/// Usage text of `pin_sim soak`.
pub const SOAK_USAGE: &str = "usage: pin_sim soak <bundle> [--games N] [--seed S] [--skill novice|standard|expert] [--threads T] [--max-ticks N] [--no-tick-budget] --out DIR";

/// File name of the JSON soak report.
pub const SOAK_JSON: &str = "soak_report.json";

/// File name of the Markdown soak report.
pub const SOAK_MARKDOWN: &str = "soak_report.md";

/// The arguments of `pin_sim soak`.
#[derive(Clone, Debug, PartialEq)]
pub struct SoakArgs {
    /// Bundle path.
    pub bundle: PathBuf,
    /// Report folder.
    pub out: PathBuf,
    /// Soak settings.
    pub cfg: SoakConfig,
}

fn soak_usage(m: String) -> CliError {
    CliError::Usage(format!("{m}\n{SOAK_USAGE}"))
}

/// Parses the arguments after `soak`.
///
/// # Errors
/// [`CliError::Usage`] for an unknown flag, a missing or malformed value,
/// zero games or threads, an unknown skill, a missing bundle or `--out`.
pub fn parse_soak_args(args: &[String]) -> Result<SoakArgs, CliError> {
    let mut bundle = None;
    let mut out = None;
    let mut cfg = SoakConfig::default();
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .cloned()
                .ok_or_else(|| soak_usage(format!("{flag} needs a value")))
        };
        let num = |flag: &str, v: String| {
            parse_u64(flag, &v).map_err(|_| soak_usage(format!("{flag}: not a number: {v:?}")))
        };
        match a.as_str() {
            "--games" => {
                let n = num("--games", value("--games")?)?;
                cfg.games = u32::try_from(n)
                    .ok()
                    .filter(|&n| n > 0)
                    .ok_or_else(|| soak_usage(format!("--games must be 1..={}", u32::MAX)))?;
            }
            "--seed" => cfg.seed = num("--seed", value("--seed")?)?,
            "--skill" => {
                let v = value("--skill")?;
                cfg.skill = SkillLevel::from_name(&v)
                    .ok_or_else(|| soak_usage(format!("--skill: unknown level {v:?}")))?;
            }
            "--threads" => {
                let n = num("--threads", value("--threads")?)?;
                cfg.threads = usize::try_from(n)
                    .ok()
                    .filter(|&n| (1..=1024).contains(&n))
                    .ok_or_else(|| soak_usage("--threads must be 1..=1024".into()))?;
            }
            "--max-ticks" => {
                let n = num("--max-ticks", value("--max-ticks")?)?;
                if n == 0 {
                    return Err(soak_usage("--max-ticks must be at least 1".into()));
                }
                cfg.max_game_ticks = n;
            }
            "--no-tick-budget" => cfg.check_tick_budget = false,
            "--out" => out = Some(PathBuf::from(value("--out")?)),
            s if s.starts_with("--") => return Err(soak_usage(format!("unknown flag {s}"))),
            s => {
                if bundle.replace(PathBuf::from(s)).is_some() {
                    return Err(soak_usage("more than one bundle given".into()));
                }
            }
        }
    }
    let bundle = bundle.ok_or_else(|| soak_usage("no bundle given".into()))?;
    let out = out.ok_or_else(|| soak_usage("give --out DIR".into()))?;
    Ok(SoakArgs { bundle, out, cfg })
}

/// The summary `pin_sim soak` prints: the verdict, every finding by kind
/// and subject, the report paths and the speed.
#[must_use]
pub fn soak_summary(report: &SoakReport, out: &Path) -> String {
    let mut s = format!(
        "soak {}: {} games, {} findings, coverage {} -> {}",
        report.table_id,
        report.game_list.len(),
        report.findings.len(),
        if report.coverage_complete() {
            "complete"
        } else {
            "incomplete"
        },
        if report.passed() { "PASS" } else { "FAIL" }
    );
    for f in report.findings.iter().chain(&report.timing.findings) {
        let game = if f.game == crate::autoplay::report::SOAK_WIDE {
            "all games".to_owned()
        } else {
            format!("game {}, tick {}", f.game, f.tick)
        };
        s.push_str(&format!(
            "\n{} {} ({game}): {}",
            f.kind.name(),
            f.subject,
            f.detail
        ));
    }
    let never = report.never_hit();
    if !never.is_empty() {
        s.push_str(&format!("\nnever-hit switches: {}", never.join(", ")));
    }
    s.push_str(&format!(
        "\nreport: {} and {}\nspeed: {:.2} games/s, {:.0} ticks/s on {} threads",
        out.join(SOAK_JSON).display(),
        out.join(SOAK_MARKDOWN).display(),
        report.timing.games_per_s,
        report.timing.ticks_per_s,
        report.timing.threads
    ));
    s
}

/// Runs `pin_sim soak`: plays the games, writes both reports into
/// `args.out` (created if missing) and returns the summary.
///
/// # Errors
/// [`CliError::Usage`] when `--out` lies inside a folder bundle;
/// [`CliError::Failed`] when the soak cannot run, the reports cannot be
/// written, or the soak fails (the message is the summary, which names
/// every finding).
pub fn run_soak_command(args: &SoakArgs) -> Result<String, CliError> {
    if record_inside_bundle(&args.bundle, &args.out) {
        return Err(soak_usage(format!(
            "--out {} is inside the bundle {}; reports there change the table hash",
            args.out.display(),
            args.bundle.display()
        )));
    }
    let report =
        run_soak(&args.bundle, &args.cfg).map_err(|e| CliError::Failed(format!("soak: {e}")))?;
    let write = |name: &str, text: String| {
        std::fs::write(args.out.join(name), text).map_err(|e| {
            CliError::Failed(format!(
                "cannot write {}: {e}",
                args.out.join(name).display()
            ))
        })
    };
    std::fs::create_dir_all(&args.out)
        .map_err(|e| CliError::Failed(format!("cannot create {}: {e}", args.out.display())))?;
    write(SOAK_JSON, report.to_json())?;
    write(SOAK_MARKDOWN, report.to_markdown())?;
    let summary = soak_summary(&report, &args.out);
    if report.passed() {
        Ok(summary)
    } else {
        Err(CliError::Failed(summary))
    }
}

/// Parses the arguments and runs on a thread with
/// [`SIM_THREAD_STACK_BYTES`] of stack; returns the stdout line.
///
/// # Errors
/// As [`parse_args`] and [`run`].
pub fn main_with_args(args: &[String]) -> Result<String, CliError> {
    if args.first().map(String::as_str) == Some("soak") {
        let soak = parse_soak_args(&args[1..])?;
        return run_soak_command(&soak);
    }
    let cmd = parse_args(args)?;
    let handle = std::thread::Builder::new()
        .name("pin_sim".into())
        .stack_size(SIM_THREAD_STACK_BYTES)
        .spawn(move || run(&cmd))
        .map_err(|e| CliError::Failed(format!("cannot start the sim thread: {e}")))?;
    let hash = handle
        .join()
        .map_err(|_| CliError::Failed("the sim thread panicked".into()))??;
    Ok(hash_line(hash))
}
