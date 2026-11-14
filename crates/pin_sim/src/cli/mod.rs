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

use core::fmt;
use pin_core::input::{CabinetAxis, CabinetInput, InputEvent};
use pin_core::sim::{SimStep, TICK_NS};
use std::path::{Path, PathBuf};

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
/// bundle's table hash and the recording could not replay.
fn record_inside_bundle(bundle: &Path, record: &Path) -> bool {
    let Ok(bundle) = bundle.canonicalize() else {
        return false;
    };
    let file_name = match record.file_name() {
        Some(n) => n,
        None => return false,
    };
    let parent = match record.parent() {
        Some(p) if !p.as_os_str().is_empty() => p,
        _ => Path::new("."),
    };
    let Ok(parent) = parent.canonicalize() else {
        return false;
    };
    let target = parent.join(file_name);
    if bundle.is_dir() {
        target.starts_with(&bundle)
    } else {
        target == bundle
    }
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

/// Parses the arguments and runs on a thread with
/// [`SIM_THREAD_STACK_BYTES`] of stack; returns the stdout line.
///
/// # Errors
/// As [`parse_args`] and [`run`].
pub fn main_with_args(args: &[String]) -> Result<String, CliError> {
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
