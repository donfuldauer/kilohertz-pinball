//! `pin_app`: Kilohertz Pinball application binary.
//!
//! The modules live in the `pin_app` library (`src/lib.rs`).
//!
//! * `--headless-ticks N`: runs N sim ticks on simulated time (no SDL, GPU or
//!   sleeping), checks the snapshot ticks read back through the triple buffer
//!   are +1 each, prints `headless: N ticks, last tick N` and exits 0
//!   (non-zero on a violation). With `--table <path>` the ticks run the real
//!   table simulation (validated and loaded with
//!   `orchestrator::load_validated`) and the state hash is printed too.
//! * `--list-displays [--headless]`: prints SDL's displays (Subphase 1.3).
//! * otherwise: opens the playfield window (single-monitor topology), spawns
//!   the sim/render/audio/I/O threads and pumps SDL events on the main thread
//!   until quit. `--table <path>` (a bundle folder or `.zip`) runs that
//!   table — drawn with the PBR renderer and its backglass / score display,
//!   its sounds mixed into the default audio device — else the idle stub sim
//!   (debug view). `--headless` uses SDL's dummy drivers (no surfaces are
//!   created then, audio goes to the null sink), `--vrr` requests
//!   `FifoRelaxed` for the playfield, `--debug-overlay` draws the debug
//!   lines over the PBR table.
//! * Profiler (7.1): a table run is always profiled (tick time and input
//!   latency on SDL's event clock, input-overflow ticks, GPU frame time
//!   from timestamp queries where the device has them; nothing is
//!   recorded otherwise). `--profiler-hud` draws the four HUD lines over
//!   the playfield (refreshed four times per second) and prints them as
//!   `pin_app: profiler: ...` after the shutdown line; without it frames
//!   are unchanged.
//! * Run conditions (`orchestrator::RunLimits`): `--run-frames N` quits
//!   once N frames were presented, `--run-ticks M` once the sim computed M
//!   ticks; with both, once both hold. They work without `--run-ms` (a hung
//!   run is then bounded by whoever supervises it, e.g. a CI job timeout).
//!   The main thread evaluates them from the threads' atomic counters each
//!   pump; meeting them prints `pin_app: run condition met`. `--run-ms T`
//!   is an optional wall-clock safety cap meant for manual use, not for
//!   tests: reaching it before the conditions hold prints `pin_app: run cap
//!   reached` and exits non-zero (as does asking for frames when nothing
//!   renders). `--run-ms T` alone quits silently after T ms, as before.
//!   After the shutdown line (`pin_app: N sim ticks (D dropped), F frames,
//!   ...`) a render diagnostics line follows (`pin_app: render: gpu ready
//!   after X ms, first frame after Y ms, ...`).
//! * `--latency-trace <file>` (REQ-LAT-003): records every delivered button
//!   event's SDL timestamp, applying tick, first bat-move tick, the first
//!   frame that drew it and that frame's present time, plus the presented
//!   frame table, into preallocated rings
//!   (`orchestrator::trace`, which documents the CSV format). A sibling
//!   temp file is created before anything starts (an unwritable path fails the run with
//!   `pin_app: --latency-trace: <error>`, a value starting with `--` with
//!   `pin_app: --latency-trace needs a path`, and the `--table` bundle file
//!   itself is refused unmodified), written on the main thread after the
//!   threads were joined and renamed over the path (the path is never
//!   truncated, so a hard-linked bundle is safe), with the run's end on the SDL clock read when the
//!   run loop ended (`# run_end_ns=`), and summarised after the shutdown lines as
//!   `pin_app: latency trace: <path>, <n> events, <m> frames, <k>
//!   overflows`. While tracing, the main thread prints `pin_app: latency
//!   trace: first frame presented` once the first frame was presented (the
//!   windowed harness waits for it). Without the flag nothing changes.
//! * `--config <path>` (REQ-VIEW-006): the `config.toml` to load at start
//!   and save on every menu change; default `config.toml` in SDL's per-user
//!   data directory (`pin_app::ui::default_config_path`). `Esc` or the
//!   controller menu button opens the in-game menu over the running game
//!   (`pin_app::ui::MenuController`).
//!
//! The table is loaded (validated and simulated from one in-memory bundle
//! snapshot) and the headless table run happens on a thread with a
//! `SIM_STACK_BYTES` stack, never on the main thread (1 MiB on Windows).
//!
//! Input goes through the device profiles and `HeldInputs`
//! (`platform::InputRouter`). Windows are owned by the platform's
//! `WindowKeeper`; each surface holds a `WindowGuard` clone, so a presenter
//! thread detached at shutdown keeps its window alive (issue #9a). Shutdown
//! order: join the engine threads, destroy released windows on the main
//! thread, wait (bounded) for a detached presenter to release its window,
//! and if one never does, exit without destroying it.

use pin_app::orchestrator::trace::{FlipperBindings, latency_trace, summary_line};
use pin_app::orchestrator::{
    AudioConfig, AudioSink, ClosePolicy, DISPLAY_RING_CAPACITY, DeviceStream, DropMonitor,
    FlipperOutline, NullSink, RegisteredSound, RenderConfig, RenderGpu, RunExit, RunLimits,
    SOUND_RING_CAPACITY, TableRenderOptions, TableSim, TableVisuals, ThreadsConfig, load_checked,
    on_sim_stack, run_headless, run_headless_table, spawn_threads, ssf_playfield_mm, table_lines,
    table_samples, table_source_positions, validate_table,
};
use pin_app::platform::{
    DisplayTopologyManager, EventForwarder, INPUT_RING_CAPACITY, InputRouter, PUMP_TIMEOUT,
    Platform, PlatformConfig, WINDOW_RING_CAPACITY, WindowGuard,
};
use pin_app::profiler::{Profiler, hud_lines};
use pin_app::ui::{
    AppConfig, AppSetting, MenuController, MenuShare, changed_settings, keyboard_router,
};
use pin_audio::mixer::{MixerConfig, trigger_ring};
use pin_audio::ssf::SsfConfig;
use pin_input::haptics::{DEFAULT_HAPTICS_RING_CAPACITY, haptics_ring};
use pin_render::gpu::{
    GpuOptions, PRESENTER_JOIN_TIMEOUT, SurfaceOptions, SurfaceSource, create_instance,
    create_surface,
};
use pin_render::topology::{DEFAULT_BACKGLASS_PIP, DEFAULT_SCORE_PIP, TopologyMode};
use pin_sim::SimConfig;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// How long shutdown waits for a detached presenter thread to release its
/// window before exiting without destroying it.
const WINDOW_RELEASE_TIMEOUT: Duration = Duration::from_secs(2);

/// Most table lines drawn in the debug view.
const MAX_TABLE_LINES: usize = 8192;

/// SDL video drivers whose raw window handles wgpu can consume (DECISIONS
/// 0.1 Q4: sdl3's raw-window-handle impl panics on `dummy`/`offscreen`).
const NATIVE_DRIVERS: [&str; 4] = ["windows", "cocoa", "x11", "wayland"];

fn value_of(args: &[String], flag: &str) -> Option<Result<u64, String>> {
    let i = args.iter().position(|a| a == flag)?;
    Some(
        args.get(i + 1)
            .ok_or_else(|| format!("{flag} needs a number"))
            .and_then(|v| v.parse::<u64>().map_err(|e| format!("{flag} {v:?}: {e}"))),
    )
}

fn sdl_now_ns() -> u64 {
    // SAFETY: SDL_GetTicksNS has no preconditions and is thread-safe.
    unsafe { sdl3::sys::timer::SDL_GetTicksNS() }
}

fn path_of(args: &[String], flag: &str) -> Option<Result<PathBuf, String>> {
    let i = args.iter().position(|a| a == flag)?;
    Some(
        args.get(i + 1)
            .map(PathBuf::from)
            .ok_or_else(|| format!("{flag} needs a path")),
    )
}

fn headless(n: Result<u64, String>, table: Option<Result<PathBuf, String>>) -> ExitCode {
    let result = n.and_then(|n| match table.transpose()? {
        None => run_headless(n).map(|last| format!("headless: {n} ticks, last tick {last}")),
        Some(path) => run_headless_table(&path, n).map(|(last, hash)| {
            format!("headless: {n} ticks, last tick {last}, state hash {hash:016x}")
        }),
    });
    match result {
        Ok(line) => {
            println!("{line}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("pin_app: {e}");
            ExitCode::FAILURE
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(n) = value_of(&args, "--headless-ticks") {
        return headless(n, path_of(&args, "--table"));
    }
    if args.iter().any(|a| a == "--list-displays") {
        return list_displays(args.iter().any(|a| a == "--headless"));
    }
    match run_app(&args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pin_app: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The table to run: validated and loaded from one snapshot, with what the
/// render and audio threads need.
struct LoadedApp {
    sim: Box<TableSim>,
    visuals: TableVisuals,
    samples: Vec<RegisteredSound>,
    table_lines: Vec<pin_render::debug::DebugLine>,
    flippers: Vec<FlipperOutline>,
    source_positions: Vec<(u16, [f32; 2])>,
    haptics: pin_input::haptics::HapticsConsumer<pin_core::input::HapticsTelemetryEvent>,
    sounds: pin_audio::mixer::SoundReceiver,
    displays: pin_input::haptics::HapticsConsumer<pin_app::orchestrator::DisplayCommand>,
    bindings: FlipperBindings,
}

/// Validates and loads the table on a big-stack thread (the simulation is
/// built, wrapped and boxed there; only the box reaches this thread).
fn load_app_table(path: &Path, mixer: MixerConfig) -> Result<LoadedApp, String> {
    let path = path.to_path_buf();
    on_sim_stack("table-load", move || -> Result<LoadedApp, String> {
        let checked = validate_table(&path)?;
        for w in &checked.warnings {
            eprintln!("pin_app: table warning: {w}");
        }
        let sim = load_checked(&checked, SimConfig::default())?;
        let (htx, hrx) = haptics_ring(DEFAULT_HAPTICS_RING_CAPACITY);
        let (stx, srx) = trigger_ring(SOUND_RING_CAPACITY);
        let (dtx, drx) = haptics_ring(DISPLAY_RING_CAPACITY);
        let sim = Box::new(
            TableSim::new(sim, Some(htx))
                .with_sounds(stx)
                .with_displays(dtx),
        );
        let visuals = TableVisuals::of(&checked);
        for w in &visuals.warnings {
            eprintln!("pin_app: table display warning: {w}");
        }
        let (samples, errors) = table_samples(&checked, mixer.sample_rate);
        for e in &errors {
            eprintln!("pin_app: table sound warning: {e}");
        }
        eprintln!(
            "pin_app: table {:?} loaded ({} switches, {} mechs, {} sounds)",
            checked.table.manifest.meta.name,
            checked.table.switches.len(),
            checked.table.mechs.len(),
            samples.len()
        );
        Ok(LoadedApp {
            sim,
            table_lines: table_lines(&checked.scene, MAX_TABLE_LINES),
            flippers: FlipperOutline::of(&checked.table),
            bindings: FlipperBindings::of_flippers(&checked.table.flippers),
            source_positions: table_source_positions(&checked.table),
            visuals,
            samples,
            haptics: hrx,
            sounds: srx,
            displays: drx,
        })
    })?
}

/// The audio sink: the default playback device (its owner stays on this
/// thread until the audio thread was joined), or the null sink headless or
/// without a device.
fn audio_output(
    platform: &Platform,
    headless: bool,
    mixer: MixerConfig,
) -> (Option<DeviceStream>, Box<dyn AudioSink>) {
    if headless {
        return (None, Box::new(NullSink::default()));
    }
    match DeviceStream::open(platform.audio(), mixer.sample_rate) {
        Ok((owner, sink)) => (Some(owner), Box::new(sink)),
        Err(e) => {
            eprintln!("pin_app: no audio output device ({e}); audio goes to the null sink");
            (None, Box::new(NullSink::default()))
        }
    }
}

fn list_displays(headless: bool) -> ExitCode {
    let result = Platform::init(PlatformConfig { headless })
        .and_then(|p| DisplayTopologyManager::query(p.video()).map(|m| (p, m)));
    match result {
        Ok((_platform, manager)) => {
            for d in manager.displays() {
                println!(
                    "display {}: {}x{} at ({}, {}), {:.2} Hz, scale {:.2}",
                    d.index,
                    d.bounds.w,
                    d.bounds.h,
                    d.bounds.x,
                    d.bounds.y,
                    d.refresh_hz,
                    d.content_scale
                );
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("pin_app: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Exit code of the teardown that skips SDL's destruction because a
/// presenter still holds a window: 1 when the run recorded a failure, else 0.
fn teardown_exit_code(failure: Option<&str>) -> i32 {
    i32::from(failure.is_some())
}

/// Combines the failure recorded by the run loop with a thread-join error so
/// neither is lost: `"{earlier}; {join_error}"` when both are present.
fn merge_failure(earlier: Option<String>, join_error: Option<String>) -> Option<String> {
    match (earlier, join_error) {
        (Some(a), Some(b)) => Some(format!("{a}; {b}")),
        (a, None) => a,
        (None, b) => b,
    }
}

/// Whether `a` and `b` are the same file (same device and inode).
#[cfg(unix)]
fn same_file(a: &std::fs::Metadata, _: &Path, b: &std::fs::Metadata, _: &Path) -> bool {
    use std::os::unix::fs::MetadataExt as _;
    a.dev() == b.dev() && a.ino() == b.ino()
}

/// Whether `a` and `b` are the same file (same canonical path).
#[cfg(not(unix))]
fn same_file(_: &std::fs::Metadata, a: &Path, _: &std::fs::Metadata, b: &Path) -> bool {
    matches!(
        (std::fs::canonicalize(a), std::fs::canonicalize(b)),
        (Ok(x), Ok(y)) if x == y
    )
}

/// Makes a new trace temp file owner-only (`0600`, Unix) from creation,
/// before any trace data is written: a trace may hold input timing, and the
/// temp file must never have bits the destination lacks.
#[cfg(unix)]
fn owner_only(o: &mut std::fs::OpenOptions) -> &mut std::fs::OpenOptions {
    use std::os::unix::fs::OpenOptionsExt as _;
    o.mode(0o600)
}

/// Makes a new trace temp file owner-only: no-op off Unix.
#[cfg(not(unix))]
fn owner_only(o: &mut std::fs::OpenOptions) -> &mut std::fs::OpenOptions {
    o
}

/// Gives the open temp `file` the permission bits (`0o777` part) of the
/// existing regular-file destination `dest`, read now, so the rename never
/// widens access. A missing destination leaves `file` as created (`0600`).
#[cfg(unix)]
fn keep_dest_permissions(file: &std::fs::File, dest: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt as _;
    match std::fs::metadata(dest) {
        Ok(m) if m.is_file() => file.set_permissions(std::fs::Permissions::from_mode(
            m.permissions().mode() & 0o777,
        )),
        Ok(_) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}

/// Keeps the destination's permissions: no-op off Unix.
#[cfg(not(unix))]
fn keep_dest_permissions(_: &std::fs::File, _: &Path) -> std::io::Result<()> {
    Ok(())
}

/// Whether `m` is a FIFO (Unix), which a write-only open would block on.
#[cfg(unix)]
fn is_fifo(m: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::FileTypeExt as _;
    m.file_type().is_fifo()
}

/// Whether `m` is a FIFO: never off Unix.
#[cfg(not(unix))]
fn is_fifo(_: &std::fs::Metadata) -> bool {
    false
}

/// Whether the `path` argument ends in a path separator (`/` or the
/// platform's own).
fn ends_in_separator(path: &Path) -> bool {
    path.as_os_str()
        .as_encoded_bytes()
        .last()
        .is_some_and(|&b| b == b'/' || char::from(b) == std::path::MAIN_SEPARATOR)
}

/// The `--latency-trace` destination: the path the trace is written to and
/// the file it is written into first. For a regular (or not yet existing)
/// destination that is a sibling temp file, renamed over the destination by
/// [`commit`](Self::commit), so the destination is never truncated and a
/// path hard-linked to the table bundle only loses its link, never the
/// bundle's bytes. A destination that exists and is not a regular file (a
/// device; a FIFO is refused) is written directly. Dropped without a commit
/// (the run ended before the shutdown commit point: a startup error after
/// the sink was opened, a panic or an early return before the threads were
/// joined), the temp file is removed and the destination is left untouched.
/// Once the engine threads are joined, `run_app` commits the run's trace
/// even when the run failed (run cap not met, a thread join error), so a
/// failed run's trace can be diagnosed ([`commit_trace`]).
#[derive(Debug)]
struct TraceSink {
    /// The `--latency-trace` path.
    dest: PathBuf,
    /// The sibling temp file while it is not committed (`None` when the
    /// destination is written directly).
    tmp: Option<PathBuf>,
    /// The open temp file (or the destination when written directly).
    file: Option<std::fs::File>,
}

/// Attempts at a fresh sibling temp-file name.
const TRACE_TMP_TRIES: u32 = 8;

impl TraceSink {
    /// Writes `csv` and moves it into place: to the temp file (flushed and
    /// synced; a platform that cannot sync the file is tolerated), given
    /// the existing destination's permission bits (Unix; a new destination
    /// keeps the temp file's owner-only `0600`), then renamed over the
    /// destination. On any error the temp file is removed and the
    /// destination keeps its old content.
    fn commit(mut self, csv: &[u8]) -> Result<(), String> {
        let err = |e: std::io::Error| format!("--latency-trace: writing the trace: {e}");
        let Some(mut file) = self.file.take() else {
            return Err(err(std::io::Error::other("no open file")));
        };
        file.write_all(csv)
            .and_then(|()| file.flush())
            .map_err(err)?;
        let Some(tmp) = self.tmp.as_ref() else {
            // A device: written directly, nothing to move.
            return Ok(());
        };
        match file.sync_all() {
            Err(e)
                if !matches!(
                    e.kind(),
                    std::io::ErrorKind::Unsupported | std::io::ErrorKind::InvalidInput
                ) =>
            {
                return Err(err(e));
            }
            _ => {}
        }
        // Never widen access: the temp file takes the existing
        // destination's permission bits (it was created owner-only).
        keep_dest_permissions(&file, &self.dest).map_err(err)?;
        drop(file);
        std::fs::rename(tmp, &self.dest).map_err(err)?;
        // Committed: nothing left for `Drop` to remove.
        self.tmp = None;
        Ok(())
    }
}

/// The shutdown commit of the latency trace (REQ-LAT-003), run once every
/// engine thread was joined: commits `csv` to `sink` whether or not the run
/// failed (`failure` is not consulted, so a run that missed its run cap or
/// had a thread join error still leaves its trace for diagnosis). Returns
/// the destination on success; a commit error is printed and merged into
/// `failure`, so it makes the run fail.
fn commit_trace(sink: TraceSink, csv: &[u8], failure: &mut Option<String>) -> Option<PathBuf> {
    let dest = sink.dest.clone();
    match sink.commit(csv) {
        Ok(()) => Some(dest),
        Err(e) => {
            eprintln!("pin_app: {e}");
            *failure = merge_failure(failure.take(), Some(e));
            None
        }
    }
}

impl Drop for TraceSink {
    fn drop(&mut self) {
        drop(self.file.take());
        if let Some(tmp) = self.tmp.take() {
            let _ = std::fs::remove_file(tmp);
        }
    }
}

/// `--latency-trace <file>`: checked and prepared now so an unwritable path
/// fails before the app starts. A value that starts with `--` is an option,
/// not a path (`./--x` is a path). A destination that is the `--table`
/// bundle file (when that is an existing file: a `.zip` bundle) is refused
/// before anything is created. A destination whose metadata cannot be read
/// for a reason other than "not found" (e.g. `trace.csv/` under a regular
/// file), a missing destination named with a trailing path separator, and
/// (Unix) an existing FIFO, which a write-only open would block on, are
/// refused before anything is opened. The destination itself is never
/// opened for writing here (unless it is a device) and never truncated: the
/// trace goes to a sibling temp file, created now owner-only (`0600`, Unix)
/// with `create_new` as the writability probe and renamed over the
/// destination by [`TraceSink::commit`] at shutdown.
fn open_latency_trace(args: &[String]) -> Result<Option<TraceSink>, String> {
    let Some(path) = path_of(args, "--latency-trace") else {
        return Ok(None);
    };
    let path = path.map_err(|e| format!("--latency-trace: {e}"))?;
    if path.to_str().is_some_and(|p| p.starts_with("--")) {
        return Err("--latency-trace needs a path".to_owned());
    }
    let io = |e: std::io::Error| format!("--latency-trace: {}: {e}", path.display());
    let dest_meta = match std::fs::metadata(&path) {
        Ok(m) => Some(m),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        // E.g. `trace.csv/` under a regular file (ENOTDIR): not "missing".
        Err(e) => return Err(io(e)),
    };
    if dest_meta.is_none() && ends_in_separator(&path) {
        return Err(format!(
            "--latency-trace: {}: is a directory path",
            path.display()
        ));
    }
    if dest_meta.as_ref().is_some_and(is_fifo) {
        // A write-only open of a FIFO blocks until a reader appears.
        return Err(format!("--latency-trace: {}: is a FIFO", path.display()));
    }
    if let Some(fm) = &dest_meta
        && let Some(Ok(table)) = path_of(args, "--table")
        && let Ok(tm) = std::fs::metadata(&table)
        && tm.is_file()
        && same_file(fm, &path, &tm, &table)
    {
        return Err(format!(
            "--latency-trace: {} is the --table bundle",
            path.display()
        ));
    }
    if let Some(m) = &dest_meta
        && !m.is_file()
    {
        // A device (a directory fails to open): written directly.
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .map_err(io)?;
        return Ok(Some(TraceSink {
            dest: path,
            tmp: None,
            file: Some(file),
        }));
    }
    let name = path
        .file_name()
        .ok_or_else(|| io(std::io::Error::other("not a file name")))?
        .to_string_lossy()
        .into_owned();
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let pid = std::process::id();
    let mut last = None;
    for n in 0..TRACE_TMP_TRIES {
        let tmp = dir.join(format!(".{name}.{pid}.{n}.tmp"));
        match owner_only(std::fs::OpenOptions::new().write(true).create_new(true)).open(&tmp) {
            Ok(file) => {
                return Ok(Some(TraceSink {
                    dest: path,
                    tmp: Some(tmp),
                    file: Some(file),
                }));
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => last = Some(e),
            Err(e) => return Err(io(e)),
        }
    }
    Err(io(last.unwrap_or_else(|| {
        std::io::Error::other("no temp file name")
    })))
}

/// Applies a settings change from the menu (issue #84): live where the app
/// can (the input profile replaces the keyboard router; the menu is open,
/// so the simulation holds nothing), otherwise one stderr line saying when
/// it applies ([`AppSetting::timing`]).
fn apply_config_change(
    platform: &Platform,
    forwarder: &mut EventForwarder,
    applied: &mut AppConfig,
    new: AppConfig,
) {
    for s in changed_settings(applied, &new) {
        if s == AppSetting::InputProfile {
            forwarder.set_router(keyboard_router(&new.input_profile).unwrap_or_else(|e| {
                eprintln!("pin_app: config: {e}");
                InputRouter::new()
            }));
            for id in platform.open_gamepads() {
                forwarder.gamepad_added(id.raw(), platform.gamepad_guid(id));
            }
        }
        eprintln!(
            "pin_app: config: {} changed; {}",
            s.label(),
            s.timing().describe()
        );
    }
    *applied = new;
}

fn run_app(args: &[String]) -> Result<(), String> {
    // config.toml first (REQ-VIEW-006): its settings shape the windows.
    let (mut menu, config_lines) = MenuController::for_app(args)?;
    for line in &config_lines {
        eprintln!("{line}");
    }
    let mut applied = menu.config().clone();
    let headless = args.iter().any(|a| a == "--headless");
    let limits = RunLimits::from_args(args)?;
    let mut trace_file = open_latency_trace(args)?;
    let vrr = args.iter().any(|a| a == "--vrr");
    let debug_overlay = args.iter().any(|a| a == "--debug-overlay");
    let profiler_hud = args.iter().any(|a| a == "--profiler-hud");
    let mixer = MixerConfig::default();
    // Load the table before opening windows: a bad table fails fast.
    let table = path_of(args, "--table")
        .transpose()?
        .map(|p| load_app_table(&p, mixer))
        .transpose()?;

    let mut platform = Platform::init(PlatformConfig { headless }).map_err(|e| e.to_string())?;
    let manager = DisplayTopologyManager::query(platform.video()).map_err(|e| e.to_string())?;
    let plan = manager
        .plan(&applied.topology_mode())
        .or_else(|e| {
            eprintln!("pin_app: config: display settings unusable ({e}); one window instead");
            manager.plan(&TopologyMode::SingleMonitor {
                display: manager.displays().first().map_or(0, |d| d.index),
                backglass: DEFAULT_BACKGLASS_PIP,
                score: DEFAULT_SCORE_PIP,
            })
        })
        .map_err(|e| e.to_string())?;
    menu.set_display_count(u32::try_from(manager.displays().len()).unwrap_or(u32::MAX));
    let created = DisplayTopologyManager::create_windows(platform.video(), &plan)
        .map_err(|e| e.to_string())?;
    let ids: Vec<u32> = created.iter().map(|w| w.id()).collect();
    // The platform's keeper owns the windows from here on; the guards keep
    // each one alive while this function or a surface (or presenter thread)
    // still uses it.
    let guards: Vec<WindowGuard> = created
        .into_iter()
        .map(|w| platform.windows_mut().adopt(w))
        .collect();

    let (input_tx, input_rx) = pin_core::spsc::channel(INPUT_RING_CAPACITY);
    let (window_tx, window_rx) = pin_core::spsc::channel(WINDOW_RING_CAPACITY);
    let mut forwarder = EventForwarder::new(input_tx, window_tx);
    forwarder.set_router(keyboard_router(&applied.input_profile).unwrap_or_else(|e| {
        eprintln!("pin_app: config: {e}");
        InputRouter::new()
    }));
    let menu_share = MenuShare::new();
    menu.set_share(menu_share.clone());
    forwarder.set_menu(menu);
    for id in platform.open_gamepads() {
        forwarder.gamepad_added(id.raw(), platform.gamepad_guid(id));
    }
    forwarder.register_windows(&ids);

    // Surfaces are created here on the main thread and moved to the render
    // thread, which owns the device (PLAN §2.2).
    let driver = platform.video().current_video_driver();
    let gpu = if NATIVE_DRIVERS.contains(&driver) {
        let options = GpuOptions::for_app();
        let instance = create_instance(options.backends);
        let mut surfaces = Vec::with_capacity(guards.len());
        for (w, guard) in platform.windows().windows().zip(&guards) {
            // SAFETY: each surface carries a clone of its window's guard
            // (`keep_alive`); the keeper destroys the window only after the
            // last clone is gone, on this thread (DECISIONS "6.0 — window
            // lifetime").
            let surface =
                unsafe { create_surface(&instance, &w.window) }.map_err(|e| e.to_string())?;
            let (pw, ph) = w.window.size_in_pixels();
            surfaces.push(SurfaceSource {
                window: w.plan_index,
                viewports: w.plan.viewports.clone(),
                surface,
                logical_size: (w.plan.bounds.w, w.plan.bounds.h),
                pixel_size: (pw, ph),
                keep_alive: Some(guard.clone()),
            });
        }
        Some(RenderGpu {
            instance,
            options,
            surfaces,
            surface_options: SurfaceOptions {
                vrr,
                force_presenter_thread: false,
            },
        })
    } else {
        eprintln!("pin_app: SDL video driver {driver:?} has no native windows; not rendering");
        None
    };

    // Declared before `threads`: dropped after the audio thread was joined.
    let (audio_device, sink) = audio_output(&platform, headless, mixer);
    let mut render = RenderConfig {
        gpu,
        window_events: Some(window_rx),
        close_policy: ClosePolicy::PlayfieldOnly,
        table_render: TableRenderOptions {
            debug_overlay: debug_overlay || applied.wireframe,
            ..TableRenderOptions::default()
        },
        menu: Some(menu_share),
        ..RenderConfig::default()
    };
    // The profiler's reader (table runs only; the idle path is not
    // profiled).
    let mut profiler = None;
    let bindings = table
        .as_ref()
        .map_or_else(FlipperBindings::new, |t| t.bindings);
    let mut trace = None;
    let mut cfg = match table {
        Some(t) => {
            // The ProfiledSim's clock is the sim loop's: SDL's event
            // timestamp clock, which the input latencies are measured on.
            let (mut cfg, reader) =
                ThreadsConfig::with_profiled_sim(t.sim, input_rx, sdl_now_ns, Profiler::new());
            render.present_profiler = cfg.render.present_profiler.take();
            render.hud = profiler_hud.then(|| reader.clone());
            profiler = Some(reader);
            cfg.audio = AudioConfig {
                haptics: Some(t.haptics),
                ssf: SsfConfig {
                    playfield_mm: ssf_playfield_mm(t.visuals.bounds),
                    ..SsfConfig::default()
                },
                sounds: Some(t.sounds),
                mixer,
                samples: t.samples,
                sink: Some(sink),
                source_positions: t.source_positions,
                ..AudioConfig::default()
            };
            render.table_lines = t.table_lines;
            render.flippers = t.flippers;
            render.table = Some(Arc::new(t.visuals));
            render.displays = Some(t.displays);
            cfg
        }
        None => {
            let mut cfg = ThreadsConfig::idle(input_rx);
            cfg.audio.sink = Some(sink);
            cfg
        }
    };
    cfg.now_ns = sdl_now_ns;
    if trace_file.is_some() {
        // Every trace buffer is allocated here, before the threads start.
        let (collector, sim_writer, frame_writer) = latency_trace(bindings, sdl_now_ns);
        cfg.latency_trace = Some(sim_writer);
        render.latency_trace = Some(frame_writer);
        trace = Some(collector);
    }
    let rendering = render.gpu.is_some();
    cfg.render = render;
    let threads = spawn_threads(cfg)?;

    let started = Instant::now();
    let mut input_drops = DropMonitor::default();
    let mut failure = None;
    let mut first_frame_announced = false;
    loop {
        let outcome = platform.pump(&mut forwarder, PUMP_TIMEOUT);
        if let Some(new) = forwarder
            .menu_mut()
            .and_then(MenuController::take_config_change)
        {
            apply_config_change(&platform, &mut forwarder, &mut applied, new);
        }
        if let Some(n) = input_drops.check(forwarder.stats().input_dropped) {
            eprintln!(
                "pin_app: input ring full, dropped {n} event(s) ({} total)",
                input_drops.total()
            );
        }
        if outcome.quit || threads.close_requested() {
            break;
        }
        let live = threads.stats();
        // Only a playfield present (the trace's own frame count) raises it.
        if !first_frame_announced && trace.as_ref().is_some_and(|t| t.frames_presented() > 0) {
            first_frame_announced = true;
            eprintln!("pin_app: latency trace: first frame presented");
        }
        if limits.frames.is_some() && (!rendering || live.gpu_error) {
            eprintln!("pin_app: run condition unreachable: --run-frames but nothing renders");
            failure = Some("--run-frames cannot be met: nothing renders".to_owned());
            break;
        }
        match limits.check(&live, started.elapsed()) {
            Some(RunExit::ConditionMet) => {
                eprintln!("pin_app: run condition met");
                break;
            }
            Some(RunExit::CapReached) => {
                eprintln!("pin_app: run cap reached");
                failure = Some(format!(
                    "run cap reached before the run conditions held ({} frames of {:?}, {} \
                     ticks of {:?})",
                    live.render_frames, limits.frames, live.sim_ticks, limits.ticks
                ));
                break;
            }
            Some(RunExit::Elapsed) => break,
            None => {}
        }
    }
    // The run's end on the trace clock (`# run_end_ns`), read once the loop
    // ended, before the threads are joined.
    let run_end_ns = sdl_now_ns();
    // 1. Join the engine threads (the render thread drops the surfaces; a
    //    presenter stalled past PRESENTER_JOIN_TIMEOUT is detached and keeps
    //    its guard).
    //    A join error (a panicked thread) is recorded, never returned early:
    //    a detached presenter may still hold a window guard, so the teardown
    //    below must still run in order before `platform` is dropped.
    let (report, join_error) = match threads.shutdown_with_audio() {
        Ok(done) => (Some(done), None),
        Err(e) => {
            eprintln!("pin_app: {e}");
            (None, Some(e))
        }
    };
    failure = merge_failure(failure, join_error);
    // Every thread is joined: drain the trace rings and write the file here,
    // on the main thread, also when the run failed (for diagnosis).
    let trace_summary = match (trace, trace_file.take()) {
        (Some(collector), Some(sink)) => {
            let collected = collector.collect(run_end_ns);
            commit_trace(sink, collected.to_csv().as_bytes(), &mut failure)
                .map(|dest| summary_line(&dest, &collected))
        }
        _ => None,
    };
    // The audio thread is joined: the device stream can go.
    drop(audio_device);
    // The main thread's own guards: from here only a detached presenter can
    // still hold a window.
    drop(guards);
    if let Some((stats, audio)) = report {
        eprintln!(
            "pin_app: {} sim ticks ({} dropped), {} frames, input dropped {}, {} haptic events",
            stats.sim_ticks,
            stats.sim_dropped_ticks,
            stats.render_frames,
            forwarder.stats().input_dropped,
            audio.haptic_events
        );
        eprintln!("{}", stats.render_diagnostics());
        if let Some(reader) = profiler.as_ref().filter(|_| profiler_hud) {
            for line in hud_lines(&reader.snapshot()) {
                eprintln!("pin_app: profiler: {line}");
            }
        }
    }
    if let Some(line) = trace_summary {
        eprintln!("{line}");
    }
    // 2. Destroy the released windows here, on the main thread, and wait for
    //    a detached presenter to release its window.
    if !platform
        .windows_mut()
        .wait_all_released(WINDOW_RELEASE_TIMEOUT)
    {
        // 3. A presenter is still stuck in the swap chain: never destroy its
        //    window under it. Exit without running SDL's teardown (which
        //    would destroy every window); the OS reclaims everything.
        eprintln!(
            "pin_app: a presenter thread still holds a window {:?} after it was detached \
             ({PRESENTER_JOIN_TIMEOUT:?} join timeout); exiting without destroying it",
            WINDOW_RELEASE_TIMEOUT
        );
        if let Some(f) = &failure {
            eprintln!("pin_app: {f}");
        }
        std::process::exit(teardown_exit_code(failure.as_deref()));
    }
    drop(platform);
    failure.map_or(Ok(()), Err)
}

#[cfg(test)]
mod tests {
    use super::{commit_trace, merge_failure, open_latency_trace, teardown_exit_code};
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    /// A fresh, empty temp directory unique to this process and call.
    fn fresh_dir(tag: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("kp_trace_{tag}_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// The names in `dir`, sorted.
    fn names(dir: &Path) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        v.sort();
        v
    }

    /// REQ-LAT-003: `--latency-trace` prepares its file up front (a sibling
    /// temp file; the destination keeps its old content until the commit
    /// renames the trace over it); an unwritable path or a missing value is
    /// an error naming the flag; without the flag nothing is opened.
    #[test]
    fn req_lat_003_latency_trace_flag_opens_the_file_up_front() {
        assert!(
            open_latency_trace(&args(&["--table", "x"]))
                .unwrap()
                .is_none()
        );
        let e = open_latency_trace(&args(&["--latency-trace"])).unwrap_err();
        assert!(e.starts_with("--latency-trace: "), "{e}");
        let e = open_latency_trace(&args(&["--latency-trace", "/nonexistent-dir/x/t.csv"]))
            .unwrap_err();
        assert!(
            e.starts_with("--latency-trace: /nonexistent-dir/x/t.csv: "),
            "{e}"
        );
        let dir = fresh_dir("flag");
        let p = dir.join("t.csv");
        std::fs::write(&p, "old").unwrap();
        let sink = open_latency_trace(&args(&["--latency-trace", p.to_str().unwrap()]))
            .unwrap()
            .unwrap();
        assert_eq!(sink.dest, p);
        assert_eq!(
            std::fs::read_to_string(&p).unwrap(),
            "old",
            "never truncated at startup"
        );
        assert_eq!(names(&dir).len(), 2, "temp file created: {:?}", names(&dir));
        sink.commit(b"csv").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "csv");
        assert_eq!(names(&dir), vec!["t.csv".to_owned()], "temp file renamed");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-LAT-003: a token after `--latency-trace` that starts with `--`
    /// is an option, not a path: rejected before any file is created; an
    /// explicit relative path `./--x` stays a path.
    #[test]
    fn req_lat_003_latency_trace_rejects_an_option_as_its_path() {
        let e = open_latency_trace(&args(&["--latency-trace", "--run-ms", "5"])).unwrap_err();
        assert_eq!(e, "--latency-trace needs a path");
        assert!(
            !std::path::Path::new("--run-ms").exists(),
            "no file created"
        );
        let name = format!("./--kp002-trace-{}.csv", std::process::id());
        let sink = open_latency_trace(&args(&["--latency-trace", &name]))
            .unwrap()
            .unwrap();
        let path = sink.dest.clone();
        assert_eq!(path, Path::new(&name));
        sink.commit(b"x").unwrap();
        assert!(path.is_file());
        std::fs::remove_file(&path).unwrap();
    }

    /// REQ-LAT-003 (B1): a trace path hard-linked to the bundle, on every
    /// platform: the trace is written to a temp file and renamed over the
    /// path, so the bundle's bytes are unchanged, the trace path holds the
    /// CSV, and (where the platform can tell) the two paths are no longer
    /// the same file.
    #[test]
    fn req_lat_003_hard_links_the_bundle_to_the_trace_path() {
        let dir = fresh_dir("hard");
        let bundle = dir.join("table.zip");
        let bytes = b"PK\x03\x04 bundle bytes".to_vec();
        std::fs::write(&bundle, &bytes).unwrap();
        let hard = dir.join("hard.csv");
        std::fs::hard_link(&bundle, &hard).unwrap();
        let h = hard.to_str().unwrap();
        let sink = open_latency_trace(&args(&["--latency-trace", h]))
            .unwrap()
            .unwrap();
        assert_eq!(std::fs::read(&bundle).unwrap(), bytes, "open left it");
        sink.commit(b"trace csv\n").unwrap();
        assert_eq!(std::fs::read(&bundle).unwrap(), bytes, "bundle unchanged");
        assert_eq!(std::fs::read(&hard).unwrap(), b"trace csv\n");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt as _;
            let (a, b) = (
                std::fs::metadata(&bundle).unwrap(),
                std::fs::metadata(&hard).unwrap(),
            );
            assert_ne!((a.dev(), a.ino()), (b.dev(), b.ino()), "link replaced");
            assert_eq!(a.nlink(), 1);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-LAT-003: `--latency-trace` equal to the `--table` path is
    /// refused before anything is created and the bundle is unchanged; a
    /// distinct path keeps its content until the commit; a `--table`
    /// directory never aliases the trace file.
    #[test]
    fn req_lat_003_latency_trace_equal_to_the_table_is_refused() {
        let dir = fresh_dir("same");
        let bundle = dir.join("table.zip");
        let bytes = b"PK\x03\x04 bundle bytes".to_vec();
        std::fs::write(&bundle, &bytes).unwrap();
        let b = bundle.to_str().unwrap();
        let e = open_latency_trace(&args(&["--table", b, "--latency-trace", b])).unwrap_err();
        assert_eq!(e, format!("--latency-trace: {b} is the --table bundle"));
        assert_eq!(std::fs::read(&bundle).unwrap(), bytes, "bundle unchanged");
        assert_eq!(names(&dir), vec!["table.zip".to_owned()], "nothing created");
        // A distinct file next to it: kept until the commit replaces it.
        let other = dir.join("trace.csv");
        std::fs::write(&other, "old trace").unwrap();
        let o = other.to_str().unwrap();
        let sink = open_latency_trace(&args(&["--table", b, "--latency-trace", o]))
            .unwrap()
            .unwrap();
        assert_eq!(sink.dest, other);
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "old trace");
        sink.commit(b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "new");
        assert_eq!(std::fs::read(&bundle).unwrap(), bytes);
        // A bundle directory: no alias check applies.
        let d = dir.to_str().unwrap();
        assert!(
            open_latency_trace(&args(&["--table", d, "--latency-trace", o]))
                .unwrap()
                .is_some()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-LAT-003: a trace path that is the `--table` bundle through a
    /// symlink or a hard link is refused and the bundle is left
    /// byte-for-byte unchanged (Unix: same device and inode).
    #[test]
    #[cfg(unix)]
    fn req_lat_003_latency_trace_never_overwrites_the_table_bundle() {
        let dir = fresh_dir("alias");
        let bundle = dir.join("table.zip");
        let bytes = b"PK\x03\x04 bundle bytes".to_vec();
        std::fs::write(&bundle, &bytes).unwrap();
        let sym = dir.join("sym.csv");
        std::os::unix::fs::symlink(&bundle, &sym).unwrap();
        let hard = dir.join("hard.csv");
        std::fs::hard_link(&bundle, &hard).unwrap();
        let b = bundle.to_str().unwrap();
        for alias in [&sym, &hard, &bundle] {
            let a = alias.to_str().unwrap();
            let e = open_latency_trace(&args(&["--table", b, "--latency-trace", a])).unwrap_err();
            assert_eq!(e, format!("--latency-trace: {a} is the --table bundle"));
            assert_eq!(std::fs::read(&bundle).unwrap(), bytes, "bundle unchanged");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-LAT-003 (B1): a symlinked trace path (not the `--table`) is
    /// replaced by the trace; the link's target is left unchanged.
    #[test]
    #[cfg(unix)]
    fn req_lat_003_symlinked_trace_path_leaves_the_target_unchanged() {
        let dir = fresh_dir("sym");
        let target = dir.join("target.bin");
        std::fs::write(&target, b"target bytes").unwrap();
        let link = dir.join("link.csv");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let sink = open_latency_trace(&args(&["--latency-trace", link.to_str().unwrap()]))
            .unwrap()
            .unwrap();
        sink.commit(b"csv").unwrap();
        assert_eq!(std::fs::read(&target).unwrap(), b"target bytes");
        assert_eq!(std::fs::read(&link).unwrap(), b"csv");
        assert!(
            !std::fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-LAT-003: once the threads are joined, a failed run (run cap not
    /// met) still commits its trace over the destination, and its failure
    /// is kept.
    #[test]
    fn req_lat_003_failed_run_still_commits_its_trace() {
        let dir = fresh_dir("failcommit");
        let p = dir.join("t.csv");
        std::fs::write(&p, "old").unwrap();
        let sink = open_latency_trace(&args(&["--latency-trace", p.to_str().unwrap()]))
            .unwrap()
            .unwrap();
        let mut failure = Some("run cap reached before 100 presses".to_owned());
        let dest = commit_trace(sink, b"new trace", &mut failure);
        assert_eq!(dest.as_deref(), Some(p.as_path()));
        assert_eq!(std::fs::read(&p).unwrap(), b"new trace");
        assert_eq!(
            failure.as_deref(),
            Some("run cap reached before 100 presses")
        );
        assert_eq!(names(&dir), vec!["t.csv".to_owned()]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-LAT-003: a commit error is merged into the run's failure (also
    /// when the run already failed) and the destination keeps its content.
    #[test]
    fn req_lat_003_commit_error_fails_the_run() {
        let dir = fresh_dir("commiterr");
        let p = dir.join("t.csv");
        std::fs::write(&p, "old").unwrap();
        for earlier in [None, Some("join failed".to_owned())] {
            let mut sink = open_latency_trace(&args(&["--latency-trace", p.to_str().unwrap()]))
                .unwrap()
                .unwrap();
            // The sink's temp path names a file that does not exist: the
            // final rename fails (on every platform, no root needed).
            let real_tmp = sink.tmp.replace(dir.join("missing.tmp")).unwrap();
            let mut failure = earlier.clone();
            assert_eq!(commit_trace(sink, b"new", &mut failure), None);
            std::fs::remove_file(&real_tmp).unwrap();
            let f = failure.expect("a commit error fails the run");
            assert!(f.contains("--latency-trace: writing the trace"), "{f}");
            if let Some(e) = &earlier {
                assert!(f.contains(e.as_str()), "{f}");
            }
            assert_eq!(std::fs::read(&p).unwrap(), b"old");
            assert_eq!(names(&dir), vec!["t.csv".to_owned()]);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-LAT-003 (B1): a run that ends before the shutdown commit point (a
    /// startup error after the sink was opened, a panic or an early return
    /// before the threads were joined) drops the sink without a commit: the
    /// temp file is removed and an existing destination is left intact.
    #[test]
    fn req_lat_003_dropped_sink_removes_the_temp_file() {
        let dir = fresh_dir("drop");
        let p = dir.join("t.csv");
        std::fs::write(&p, "keep me").unwrap();
        let sink = open_latency_trace(&args(&["--latency-trace", p.to_str().unwrap()]))
            .unwrap()
            .unwrap();
        assert_eq!(names(&dir).len(), 2);
        drop(sink);
        assert_eq!(names(&dir), vec!["t.csv".to_owned()]);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "keep me");
        // A fresh destination: nothing left behind either.
        let q = dir.join("new.csv");
        drop(open_latency_trace(&args(&["--latency-trace", q.to_str().unwrap()])).unwrap());
        assert_eq!(names(&dir), vec!["t.csv".to_owned()]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-LAT-003: a destination that is not a regular file (a device) is
    /// written directly, without a temp file.
    #[test]
    #[cfg(unix)]
    fn req_lat_003_device_destination_is_written_directly() {
        let sink = open_latency_trace(&args(&["--latency-trace", "/dev/null"]))
            .unwrap()
            .unwrap();
        assert!(sink.tmp.is_none());
        sink.commit(b"csv").unwrap();
    }

    /// REQ-LAT-003 (C1): a destination whose metadata fails for a reason
    /// other than "not found" (`trace.csv/` under a regular file: ENOTDIR)
    /// is refused before anything is created.
    #[test]
    #[cfg(unix)]
    fn req_lat_003_trace_path_under_a_regular_file_is_refused() {
        let dir = fresh_dir("notdir");
        let file = dir.join("trace.csv");
        std::fs::write(&file, "keep").unwrap();
        let bad = format!("{}/", file.display());
        let e = open_latency_trace(&args(&["--latency-trace", &bad])).unwrap_err();
        assert!(e.starts_with(&format!("--latency-trace: {bad}: ")), "{e}");
        assert!(!e.ends_with("is a directory path"), "{e}");
        let under = format!("{}/x.csv", file.display());
        let e = open_latency_trace(&args(&["--latency-trace", &under])).unwrap_err();
        assert!(e.starts_with(&format!("--latency-trace: {under}: ")), "{e}");
        assert_eq!(names(&dir), vec!["trace.csv".to_owned()], "nothing created");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "keep");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-LAT-003 (C1): a missing destination named with a trailing path
    /// separator is a directory path, refused before anything is created;
    /// a new plain file name (also below a relative directory) still works.
    #[test]
    fn req_lat_003_trailing_separator_trace_path_is_refused() {
        let dir = fresh_dir("sep");
        for sep in ['/', std::path::MAIN_SEPARATOR] {
            let bad = format!("{}{sep}new{sep}", dir.display());
            let e = open_latency_trace(&args(&["--latency-trace", &bad])).unwrap_err();
            assert_eq!(e, format!("--latency-trace: {bad}: is a directory path"));
            assert!(names(&dir).is_empty(), "nothing created: {:?}", names(&dir));
        }
        let plain = dir.join("trace.csv");
        let sink = open_latency_trace(&args(&["--latency-trace", plain.to_str().unwrap()]))
            .unwrap()
            .unwrap();
        sink.commit(b"csv").unwrap();
        assert_eq!(std::fs::read_to_string(&plain).unwrap(), "csv");
        let sub = format!("./kp002-sep-{}", std::process::id());
        std::fs::create_dir_all(&sub).unwrap();
        let rel = format!("{sub}/trace.csv");
        let sink = open_latency_trace(&args(&["--latency-trace", &rel]))
            .unwrap()
            .unwrap();
        sink.commit(b"rel").unwrap();
        assert_eq!(std::fs::read_to_string(&rel).unwrap(), "rel");
        std::fs::remove_dir_all(&sub).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-LAT-003 (C3): an existing FIFO destination is refused before
    /// anything is opened, so the start can never block on it.
    #[test]
    #[cfg(unix)]
    fn req_lat_003_fifo_trace_destination_is_refused() {
        let dir = fresh_dir("fifo");
        let fifo = dir.join("trace.fifo");
        match std::process::Command::new("mkfifo").arg(&fifo).status() {
            Ok(s) if s.success() => {}
            other => {
                eprintln!("mkfifo unavailable ({other:?}): FIFO check skipped");
                std::fs::remove_dir_all(&dir).unwrap();
                return;
            }
        }
        let f = fifo.to_str().unwrap();
        let e = open_latency_trace(&args(&["--latency-trace", f])).unwrap_err();
        assert_eq!(e, format!("--latency-trace: {f}: is a FIFO"));
        assert_eq!(
            names(&dir),
            vec!["trace.fifo".to_owned()],
            "nothing created"
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// The mode bits (`0o777`) of `p`.
    #[cfg(unix)]
    fn mode(p: &Path) -> u32 {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::metadata(p).unwrap().permissions().mode() & 0o777
    }

    /// REQ-LAT-003 (C2, CWE-732): the temp file is owner-only (`0600`)
    /// from creation; the commit gives it the existing destination's
    /// permission bits (0600 stays 0600, 0640 stays 0640) and a new
    /// destination ends `0600`, so the rename never widens access.
    #[test]
    #[cfg(unix)]
    fn req_lat_003_trace_commit_never_widens_permissions() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = fresh_dir("perm");
        for want in [0o600, 0o640] {
            let p = dir.join(format!("t{want:o}.csv"));
            std::fs::write(&p, "old").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(want)).unwrap();
            let sink = open_latency_trace(&args(&["--latency-trace", p.to_str().unwrap()]))
                .unwrap()
                .unwrap();
            let tmp = sink.tmp.clone().unwrap();
            assert_eq!(mode(&tmp), 0o600, "temp owner-only before any write");
            sink.commit(b"new").unwrap();
            assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
            assert_eq!(mode(&p), want, "destination keeps {want:o}");
        }
        let fresh = dir.join("fresh.csv");
        let sink = open_latency_trace(&args(&["--latency-trace", fresh.to_str().unwrap()]))
            .unwrap()
            .unwrap();
        assert_eq!(mode(sink.tmp.as_ref().unwrap()), 0o600);
        sink.commit(b"csv").unwrap();
        assert_eq!(mode(&fresh), 0o600, "a new destination is owner-only");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn merge_failure_keeps_every_recorded_message() {
        assert_eq!(merge_failure(None, None), None);
        assert_eq!(merge_failure(Some("a".into()), None).as_deref(), Some("a"));
        assert_eq!(merge_failure(None, Some("b".into())).as_deref(), Some("b"));
        assert_eq!(
            merge_failure(Some("a".into()), Some("b".into())).as_deref(),
            Some("a; b")
        );
    }

    #[test]
    fn stuck_presenter_teardown_keeps_the_failure_exit_code() {
        assert_eq!(teardown_exit_code(Some("run cap reached")), 1);
        assert_eq!(teardown_exit_code(None), 0);
    }
}
