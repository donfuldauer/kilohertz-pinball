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
//!   its sounds mixed into the default audio device. Without `--table` the
//!   table browser (issue #85, REQ-VIEW-006) starts: the idle stub sim
//!   (debug view) runs under the in-game menu, opened on its Tables panel,
//!   which lists the bundles in `<exe dir>/tables` (or `config.toml`'s
//!   `tables_dir`), scanned on the I/O worker (`orchestrator::IoWorker`) at
//!   start and on every rescan; a table picked there is validated and
//!   loaded on the worker and swapped in by restarting the engine threads
//!   on the same windows and menu (`pin_app: table swap: <name>
//!   (wireframe=<bool>)`), and a failed load leaves the running table and
//!   shows the error in the menu. Rescans asked for while one runs coalesce
//!   into one follow-up scan; a window close requested before a swap ends
//!   the run instead of starting the new table. The joined form
//!   `--table=<path>` is refused (`pin_app: --table=<path> is not
//!   supported; use --table <path>`, exit 1), headless too. `--headless` uses SDL's dummy drivers (no surfaces are
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
//!   itself and the configuration file (`--config` or the default, also
//!   before its first save: `--latency-trace: <path> is the configuration
//!   file`) are refused unmodified), written on the main thread after the
//!   threads were joined and renamed over the path (the path is never
//!   truncated, so a hard-linked bundle is safe), with the run's end on the SDL clock read when the
//!   run loop ended (`# run_end_ns=`), and summarised after the shutdown lines as
//!   `pin_app: latency trace: <path>, <n> events, <m> frames, <k>
//!   overflows`. A table swap keeps the stopped engine's trace: the file
//!   holds every engine of the run in order
//!   (`orchestrator::trace::CollectedTrace::append`). While tracing, a
//!   table browser selection that is the trace destination (a `.zip`
//!   bundle) or holds it (a folder bundle) is refused
//!   (`--latency-trace: <path> is the selected table bundle` / `is in the
//!   selected table bundle`) and not loaded, and the trace is then not
//!   published at shutdown (`pin_app: --latency-trace: <path> ...: the
//!   trace was not published`; the run fails). While tracing, the main thread prints `pin_app: latency
//!   trace: first frame presented` once any engine of the run presented
//!   its first playfield frame (the windowed harness waits for it). Without the flag nothing changes.
//! * Cabinet feedback (KP-006, SPEC §4 3.4 / 3.5): while a table runs, the
//!   main thread drives gamepad rumble from the sim's haptic impulses (one
//!   `RumbleDriver` per engine, reset on every table load and new session),
//!   the sim thread sends UDP telemetry (`docs/TELEMETRY_PROTOCOL.md`) to
//!   `config.toml`'s `[telemetry] endpoint` (default `127.0.0.1:35120`;
//!   `enabled = false` turns it off) through a sender thread, and on
//!   Windows the DOF bridge is started for a table with `[dof]` (`[dof]
//!   enabled`, on the telemetry port) and stopped at unload and exit. After
//!   the shutdown line one `pin_app: telemetry: <sent> datagrams sent to
//!   <addr> (...)` line (or `pin_app: telemetry: disabled`) follows.
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

use pin_app::dof::{BRIDGE_EXE_NAME, DofBridge, DofLaunchConfig, platform_supported};
use pin_app::orchestrator::trace::{
    CollectedTrace, FlipperBindings, TraceCollector, latency_trace, summary_line,
};
use pin_app::orchestrator::{
    AudioConfig, AudioSink, AudioStats, ClosePolicy, DeviceStream, DropMonitor, IoReply, IoRequest,
    IoWorker, LoadedApp, NullSink, RenderConfig, RenderGpu, RunExit, RunLimits, StatsSnapshot,
    TableRenderOptions, TelemetryFeed, TelemetryFeedStats, TelemetryLink, TelemetryReport, Threads,
    ThreadsConfig, load_app_table_with_budget, open_telemetry, parse_telemetry_endpoint,
    run_headless, run_headless_table, spawn_threads, ssf_playfield_mm,
};
use pin_app::platform::rumble::{RumbleCommand, RumbleDriver, RumbleOutput};
use pin_app::platform::{
    DisplayTopologyManager, EventForwarder, INPUT_RING_CAPACITY, InputRouter, PUMP_TIMEOUT,
    Platform, PlatformConfig, WINDOW_RING_CAPACITY, WindowGuard,
};
use pin_app::profiler::{Profiler, ProfilerReader, hud_lines};
use pin_app::ui::{
    AppConfig, AppSetting, MenuController, MenuShare, TableLoadState, browser_dir,
    changed_settings, config_path_arg, default_config_path, keyboard_router,
};
use pin_audio::mixer::MixerConfig;
use pin_audio::ssf::SsfConfig;
use pin_core::haptics::HapticEvent;
use pin_core::input::InputEvent;
use pin_core::spsc::SpscConsumer;
use pin_input::haptics::{DEFAULT_HAPTICS_RING_CAPACITY, RumbleSettings, haptics_ring};
use pin_render::gpu::{
    GpuOptions, PRESENTER_JOIN_TIMEOUT, SurfaceOptions, SurfaceSource, create_instance,
    create_surface,
};
use pin_render::topology::WindowEvent;
use pin_render::topology::{DEFAULT_BACKGLASS_PIP, DEFAULT_SCORE_PIP, TopologyMode};
use std::io::Write as _;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// How long shutdown waits for a detached presenter thread to release its
/// window before exiting without destroying it.
const WINDOW_RELEASE_TIMEOUT: Duration = Duration::from_secs(2);

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

/// `--table=<path>` is refused (gate 163 round 9, review thread
/// 4703749482): every reader of `--table` (`path_of`, the table browser's
/// `browser_dir`) takes only the separate form `--table <path>`, so the
/// joined form would silently start no table (and no browser). Checked at
/// the start of the headless run and of the app run, before anything is
/// loaded; the caller adds the `pin_app: ` prefix.
fn reject_table_equals(args: &[String]) -> Result<(), String> {
    if args.iter().any(|a| a.starts_with("--table=")) {
        return Err("--table=<path> is not supported; use --table <path>".to_owned());
    }
    Ok(())
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
        if let Err(e) = reject_table_equals(&args) {
            eprintln!("pin_app: {e}");
            return ExitCode::FAILURE;
        }
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

/// Gives a new trace temp file the mode `0600` (Unix) at creation (the
/// `open` mode, with `create_new`), before any trace data is written: a
/// trace may hold input timing. This is the mode every committed trace is
/// published with: the temp file is never chmod-ed afterwards, and the
/// destination's permissions, owner, group and ACL are not consulted. The
/// guarantee is for the Unix mode bits only. On Linux a directory default
/// ACL gets the mask `---` from this mode, so its named entries grant
/// nothing. On macOS, extended ACL entries inherited from the directory are
/// not removed and can grant other accounts access.
#[cfg(unix)]
fn owner_only(o: &mut std::fs::OpenOptions) -> &mut std::fs::OpenOptions {
    use std::os::unix::fs::OpenOptionsExt as _;
    o.mode(0o600)
}

/// No-op off Unix: no mode is set, so on Windows the trace takes the DACL
/// inherited from its directory (std's default file creation) and there is
/// no owner-only guarantee.
#[cfg(not(unix))]
fn owner_only(o: &mut std::fs::OpenOptions) -> &mut std::fs::OpenOptions {
    o
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
///
/// Permissions (REQ-LAT-003, CWE-732): on Unix the temp file is created
/// with mode `0600` ([`owner_only`]), never chmod-ed, and renamed over the
/// destination, so every committed trace has owner-only mode bits whether
/// or not a destination existed. The existing destination's permissions,
/// owner, group and ACL are not consulted or preserved: overwriting a trace
/// that others could read gives it mode `0600`. Beyond the mode bits: on
/// Linux a directory default ACL gets the mask `---` and grants nothing; on
/// macOS extended ACL entries inherited from the directory are not removed
/// and can grant other accounts access; on Windows the trace takes the DACL
/// inherited from its directory and there is no owner-only guarantee.
/// Directory-inherited ACLs are accepted as the directory owner's choice
/// (owner decision "Accept folder ACLs", DECISIONS "#163 gate review round
/// 6 — directory-inherited ACLs on macOS and Windows"). Because nothing is read from the
/// destination, a change to it at any moment cannot affect the published
/// mode (CWE-367). A device written directly keeps its own permissions.
#[derive(Debug)]
struct TraceSink {
    /// The `--latency-trace` path.
    dest: PathBuf,
    /// The sibling temp file while it is not committed (`None` when the
    /// destination is written directly).
    tmp: Option<PathBuf>,
    /// The open temp file (or the destination when written directly).
    file: Option<std::fs::File>,
    /// The run's configuration file ([`config_path`]), compared again by
    /// file identity before publication ([`finish_trace`]); `None` when
    /// there is none to protect.
    config: Option<PathBuf>,
    /// The table bundle loaded now (the `--table` path at start-up, then
    /// each bundle a table browser swap started, [`set_loaded`](Self::set_loaded)),
    /// rechecked by the load guard's bundle-membership rule before
    /// publication ([`finish_trace`]; gate 163 round 14, review comment
    /// 4713880925); `None` while no bundle is loaded.
    loaded: Option<PathBuf>,
}

/// Attempts at a fresh sibling temp-file name.
const TRACE_TMP_TRIES: u32 = 8;

impl TraceSink {
    /// Records `bundle` as the table bundle loaded now (a table browser
    /// swap started it), replacing the previous one: the shutdown step
    /// ([`finish_trace`]) does not publish the trace over it or into it.
    fn set_loaded(&mut self, bundle: PathBuf) {
        self.loaded = Some(bundle);
    }

    /// Writes `csv` and moves it into place: to the temp file (flushed and
    /// synced; a platform that cannot sync the file is tolerated), then
    /// renamed over the destination. The temp file keeps the mode it was
    /// created with (`0600` on Unix) and is never chmod-ed; the
    /// destination's permissions, owner, group and ACL are not consulted
    /// or preserved, so overwriting gives the trace mode `0600` (inherited
    /// directory ACLs: see [`TraceSink`]). On any
    /// error the temp file is removed and the destination keeps its old
    /// content.
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
        // The temp file keeps the mode it was created with: it
        // is never chmod-ed and nothing is read from the destination.
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
/// `failure`, so it makes the run fail. On Unix the published trace has
/// mode `0600`: the temp file is created that way, never chmod-ed and
/// renamed over the destination, whose permissions, owner, group and ACL
/// are not consulted or preserved ([`TraceSink::commit`]); inherited
/// directory ACLs are not removed (see [`TraceSink`]).
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

/// How an existing `--latency-trace` destination collides with a table
/// bundle ([`bundle_alias`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BundleAlias {
    /// The destination is the bundle file (a `.zip` bundle).
    File,
    /// The destination lies inside the bundle folder (a folder bundle).
    Member,
}

/// Whether the existing `--latency-trace` destination `dest` (with metadata
/// `dest_meta`) is the table bundle `bundle` or lies inside it, so the
/// trace commit's rename would replace (part of) the bundle. A bundle that
/// is an existing regular file (a `.zip` bundle) collides when it is the
/// same file as `dest` ([`same_file`]: also through a hard link or a
/// symlink). A bundle that is an existing directory (a folder bundle)
/// collides when `dest` lies inside it at any depth: the canonical bundle
/// path is compared component by component (never as a string prefix, so
/// `example2/x.csv` is not inside `example`) with both the canonical
/// destination (its symlinks, `..` components and symlinked directories
/// resolved) and the canonical directory entry the rename replaces (the
/// destination's canonical parent joined with its file name: a destination
/// symlink is itself replaced, not its target). Either inside counts. A
/// hard link outside the folder to one of its files is no member: the
/// rename replaces only that outside entry. A bundle that does not exist or
/// cannot be resolved collides with nothing. The one alias test of the
/// `--table` bundle at start-up ([`open_latency_trace`]), of a table
/// browser selection ([`trace_alias_guard`]) and of a selection still in
/// flight at shutdown ([`finish_trace`]).
fn bundle_alias(dest_meta: &std::fs::Metadata, dest: &Path, bundle: &Path) -> Option<BundleAlias> {
    let bm = std::fs::metadata(bundle).ok()?;
    if bm.is_file() {
        return same_file(dest_meta, dest, &bm, bundle).then_some(BundleAlias::File);
    }
    if !bm.is_dir() {
        return None;
    }
    let root = std::fs::canonicalize(bundle).ok()?;
    let target = std::fs::canonicalize(dest).ok();
    let entry = dest.file_name().and_then(|name| {
        let parent = match dest.parent() {
            Some(p) if !p.as_os_str().is_empty() => p,
            _ => Path::new("."),
        };
        std::fs::canonicalize(parent).ok().map(|p| p.join(name))
    });
    target
        .iter()
        .chain(entry.iter())
        .any(|p| p.starts_with(&root))
        .then_some(BundleAlias::Member)
}

/// The refusal text when the `--latency-trace` destination `dest` collides
/// with the table bundle `bundle` ([`bundle_alias`]), `None` when it does
/// not (also when `dest` does not exist or cannot be read): `--latency-trace:
/// <dest> is the selected table bundle` for a `.zip` bundle, `... is in the
/// selected table bundle` for a folder bundle holding it.
fn trace_alias(dest: &Path, bundle: &Path) -> Option<String> {
    let m = std::fs::metadata(dest).ok()?;
    let verb = match bundle_alias(&m, dest, bundle)? {
        BundleAlias::File => "is",
        BundleAlias::Member => "is in",
    };
    Some(format!(
        "--latency-trace: {} {verb} the selected table bundle",
        dest.display()
    ))
}

/// The table browser's load guard while tracing (gate 163 rounds 9 and 10,
/// review threads 4703749453 and comment 4704516459): a selected bundle
/// that is the `--latency-trace` destination `dest` or holds it
/// ([`trace_alias`]) is refused with `--latency-trace: <dest> is (in) the
/// selected table bundle` before it is loaded, and `blocked` is set, so the
/// shutdown step ([`finish_trace`]) does not publish the trace over it; the
/// menu shows the error and the running table stays. It runs on the I/O
/// worker ([`IoWorker::spawn_guarded`], R4), before each load. A
/// destination that does not exist (or cannot be read) is no bundle: the
/// selection loads and `blocked` is left as it is.
fn trace_alias_guard(
    dest: PathBuf,
    blocked: Arc<AtomicBool>,
) -> impl Fn(&Path) -> Result<(), String> + Send + 'static {
    move |bundle| match trace_alias(&dest, bundle) {
        Some(e) => {
            blocked.store(true, Ordering::Release);
            Err(e)
        }
        None => Ok(()),
    }
}

/// The table browser selection whose load may not have been answered when
/// the run ended: the menu's `LoadRequested` or `Loading` path. The I/O
/// worker drops a request still queued when it is stopped without running
/// its guard, so [`finish_trace`] checks this path itself.
fn unanswered_selection(menu: &MenuController) -> Option<&Path> {
    match menu.menu().load_state() {
        TableLoadState::LoadRequested(p) | TableLoadState::Loading(p) => Some(p),
        _ => None,
    }
}

/// The shutdown step of the latency trace (REQ-LAT-003, gate 163 round 10,
/// review thread 4703749453 comment 4704457456 and comment 4704516459), run
/// on the main thread once every engine thread and the I/O worker were
/// joined. When `blocked` is set (the browser guard refused a selection
/// that is or holds the destination, [`trace_alias_guard`]) or the
/// selection `in_flight` ([`unanswered_selection`]) is or holds the
/// destination ([`trace_alias`]), the trace is not published: `sink` is
/// dropped (its temp file is removed, the destination is untouched), one
/// `pin_app: --latency-trace: <dest> ...: the trace was not published` line
/// is printed and the same text is merged into `failure` (a requested
/// artifact was not produced), and `None` is returned. Otherwise the trace
/// is committed as before ([`commit_trace`]: also for a failed run).
///
/// The run's configuration file (the sink's `config`, [`config_path`] at
/// start-up) is also compared again here, by file identity
/// ([`config_identity_alias`], gate 163 round 12, review comment
/// 4710120054): a destination that did not exist at start-up can name the
/// configuration file the menu saved during the run (two spellings of one
/// directory entry on a case-insensitive volume), which the start-up
/// comparison ([`config_alias`]) cannot know. Then the trace is not
/// published either: `--latency-trace: <dest> is the configuration file:
/// the trace was not published`. The configuration's recovery copy
/// ([`AppConfig::recovery_path`], which a malformed configuration's load
/// makes) is compared the same way (gate 163 round 14, review comment
/// 4713880930): `... is the configuration recovery copy: the trace was not
/// published`.
///
/// The table bundle loaded now (the sink's `loaded`: the `--table` bundle
/// or the last one a table browser swap started) is checked with the load
/// guard's bundle-membership rule too ([`trace_alias`]; gate 163 round 14,
/// review comment 4713880925): a destination that did not exist when the
/// bundle was loaded but now is (in) it, e.g. a file created inside the
/// loaded folder bundle during the run, is not published over: `...
/// is in the selected table bundle: the trace was not published`. The
/// unanswered selection is still checked as well.
fn finish_trace(
    sink: TraceSink,
    blocked: &AtomicBool,
    in_flight: Option<&Path>,
    csv: &[u8],
    failure: &mut Option<String>,
) -> Option<PathBuf> {
    let alias = if blocked.load(Ordering::Acquire) {
        Some(format!(
            "--latency-trace: {} is or is in a selected table bundle",
            sink.dest.display()
        ))
    } else {
        in_flight
            .and_then(|p| trace_alias(&sink.dest, p))
            .or_else(|| {
                sink.loaded
                    .as_deref()
                    .and_then(|p| trace_alias(&sink.dest, p))
            })
            .or_else(|| {
                sink.config
                    .as_deref()
                    .and_then(|c| config_identity_alias(&sink.dest, c))
            })
    };
    let Some(alias) = alias else {
        return commit_trace(sink, csv, failure);
    };
    drop(sink);
    let e = format!("{alias}: the trace was not published");
    eprintln!("pin_app: {e}");
    *failure = merge_failure(failure.take(), Some(e));
    None
}

/// The shutdown recheck of the `--latency-trace` destination `dest`
/// against the configuration file `config` (REQ-LAT-003, REQ-VIEW-006;
/// gate 163 round 12, review comment 4710120054): when both exist now,
/// their fresh metadata (symlinks followed) is compared with [`same_file`]
/// (device and inode on Unix, canonical path elsewhere). The same file
/// gives `--latency-trace: <dest> is the configuration file`; distinct
/// files, or either path missing (or unreadable), give `None`.
///
/// The configuration's recovery copy ([`AppConfig::recovery_path`]) is
/// compared the same way (gate 163 round 14, review comment 4713880930):
/// the same file gives `--latency-trace: <dest> is the configuration
/// recovery copy`.
fn config_identity_alias(dest: &Path, config: &Path) -> Option<String> {
    let dm = std::fs::metadata(dest).ok()?;
    let backup = AppConfig::recovery_path(config);
    [
        (Some(config), "the configuration file"),
        (backup.as_deref(), "the configuration recovery copy"),
    ]
    .into_iter()
    .find_map(|(p, what)| {
        let p = p?;
        let pm = std::fs::metadata(p).ok()?;
        same_file(&dm, dest, &pm, p)
            .then(|| format!("--latency-trace: {} is {what}", dest.display()))
    })
}

/// The `config.toml` path of this run (REQ-VIEW-006): the `--config <file>`
/// value, else [`pin_app::ui::default_config_path`]. A malformed
/// `--config` is the same error `MenuController::for_app` reports.
fn config_path(args: &[String]) -> Result<PathBuf, String> {
    Ok(match config_path_arg(args)? {
        Some(p) => p,
        None => default_config_path().0,
    })
}

/// `path` made absolute (a relative path is resolved against the current
/// directory) and resolved the way the filesystem would, for comparing
/// paths that may not exist yet ([`is_config_path`]). The directory part
/// (every component but a final file name) is resolved from its deepest
/// existing ancestor: that ancestor, taken UNFOLDED (so a symlink followed
/// by `..` resolves to the symlink target's parent, as the filesystem does;
/// gate 163 round 12, review comment 4710120043), is replaced by its
/// canonical form (`std::fs::canonicalize`: symlinks and `..` resolved),
/// and only the remaining, not existing components are normalised
/// lexically (`.` dropped, `..` folded into its parent, never above the
/// root). When no ancestor can be canonicalised the whole path is
/// normalised lexically. The final file name itself is never resolved (a
/// symlink there is what the trace commit replaces, not its target).
fn normalised_path(path: &Path) -> PathBuf {
    use std::path::Component;
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(path)
    };
    let comps: Vec<Component<'_>> = abs.components().collect();
    let (dirs, name) = match comps.split_last() {
        Some((Component::Normal(n), rest)) => (rest, Some(*n)),
        _ => (&comps[..], None),
    };
    // Lexical normalisation of `tail` onto `out`.
    let fold = |mut out: PathBuf, tail: &[Component<'_>]| {
        for c in tail {
            match c {
                Component::CurDir => {}
                Component::ParentDir => {
                    // Never above the root (`pop` keeps the root component).
                    if out.parent().is_some() {
                        out.pop();
                    }
                }
                other => out.push(other.as_os_str()),
            }
        }
        out
    };
    let resolved = (1..=dirs.len()).rev().find_map(|k| {
        let ancestor: PathBuf = dirs[..k].iter().collect();
        std::fs::canonicalize(&ancestor)
            .ok()
            .map(|base| fold(base, &dirs[k..]))
    });
    let mut out = resolved.unwrap_or_else(|| fold(PathBuf::new(), dirs));
    if let Some(n) = name {
        out.push(n);
    }
    out
}

/// Whether `a` and `b` name the same file (REQ-LAT-003, gate 163 round 11,
/// review comment 4705691334): when both exist, the same file
/// ([`same_file`]: device and inode on Unix, so a symlink or hard link to
/// the other counts); when either does not exist (yet), the same
/// [`normalised_path`].
fn is_config_path(a: &Path, a_meta: Option<&std::fs::Metadata>, b: &Path) -> bool {
    match (a_meta, std::fs::metadata(b).ok()) {
        (Some(am), Some(bm)) => same_file(am, a, &bm, b),
        _ => normalised_path(a) == normalised_path(b),
    }
}

/// The refusal when the `--latency-trace` destination `dest` (metadata
/// `dest_meta`, `None` when it does not exist) would overwrite or replace
/// the configuration file `config` (REQ-LAT-003, REQ-VIEW-006; gate 163
/// round 11, review comment 4705691334): `dest` is the configuration file
/// ([`is_config_path`]; also a path that does not exist yet, so a first
/// save is protected too), or `config` is one of the trace's sibling
/// temp-file names (`.kp-trace.<pid>.<n>.tmp` beside `dest`, which the
/// commit would move away). The text is `--latency-trace: <dest> is the
/// configuration file`. A `dest` that is the configuration's recovery copy
/// ([`AppConfig::recovery_path`], the same [`is_config_path`] test; gate
/// 163 round 14, review comment 4713880930) gives `--latency-trace: <dest>
/// is the configuration recovery copy`. `None` for a separate destination.
fn config_alias(
    dest: &Path,
    dest_meta: Option<&std::fs::Metadata>,
    config: &Path,
) -> Option<String> {
    let refuse = || {
        Some(format!(
            "--latency-trace: {} is the configuration file",
            dest.display()
        ))
    };
    if is_config_path(dest, dest_meta, config) {
        return refuse();
    }
    // The recovery copy a malformed configuration's load makes (gate 163
    // round 14, review comment 4713880930).
    if AppConfig::recovery_path(config).is_some_and(|b| is_config_path(dest, dest_meta, &b)) {
        return Some(format!(
            "--latency-trace: {} is the configuration recovery copy",
            dest.display()
        ));
    }
    let dir = trace_dir(dest);
    let config = normalised_path(config);
    let pid = std::process::id();
    if (0..TRACE_TMP_TRIES).any(|n| normalised_path(&dir.join(trace_tmp_name(pid, n))) == config) {
        return refuse();
    }
    None
}

/// The directory the trace's sibling temp file is created in: `dest`'s
/// parent, `.` for a bare file name.
fn trace_dir(dest: &Path) -> PathBuf {
    match dest.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    }
}

/// The trace temp file's name for attempt `n` of process `pid`: a short
/// fixed prefix, never the destination's name, so a destination name at
/// the file-name limit still gets a temp file.
fn trace_tmp_name(pid: u32, n: u32) -> String {
    format!(".kp-trace.{pid}.{n}.tmp")
}

/// Opens an existing non-regular `--latency-trace` destination (a device)
/// for direct writing. On Unix the open is nonblocking (`O_NONBLOCK`), so
/// a destination that turned into a FIFO after the metadata check (gate
/// 163 round 11, review 6070807835) cannot block the start: with no reader
/// the open fails at once (`ENXIO`), and with one the opened descriptor's
/// own metadata shows the FIFO, which is refused with `--latency-trace:
/// <dest>: is a FIFO`. A descriptor that is no FIFO (a device; a directory
/// fails to open for writing) has `O_NONBLOCK` cleared again before it is
/// kept, so later writes block as before.
#[cfg(unix)]
fn open_direct(path: &Path) -> Result<std::fs::File, String> {
    use sdl3::libc;
    use std::os::fd::AsRawFd as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let io = |e: std::io::Error| format!("--latency-trace: {}: {e}", path.display());
    let file = std::fs::OpenOptions::new()
        .write(true)
        .custom_flags(libc::O_NONBLOCK)
        .open(path)
        .map_err(io)?;
    if is_fifo(&file.metadata().map_err(io)?) {
        return Err(format!("--latency-trace: {}: is a FIFO", path.display()));
    }
    let fd = file.as_raw_fd();
    // SAFETY: `fd` is the open descriptor of `file`, alive for both calls;
    // F_GETFL and F_SETFL take and return plain integers.
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
    if flags < 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    // SAFETY: as above.
    if unsafe { libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK) } < 0 {
        return Err(io(std::io::Error::last_os_error()));
    }
    Ok(file)
}

/// Opens an existing non-regular `--latency-trace` destination (a device)
/// for direct writing: off Unix there are no FIFOs to guard against.
#[cfg(not(unix))]
fn open_direct(path: &Path) -> Result<std::fs::File, String> {
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .map_err(|e| format!("--latency-trace: {}: {e}", path.display()))
}

/// `--latency-trace <file>`: checked and prepared now so an unwritable path
/// fails before the app starts. A value that starts with `--` is an option,
/// not a path (`./--x` is a path). An existing destination that is the
/// `--table` bundle file (a `.zip` bundle) or lies inside the `--table`
/// bundle folder (a folder bundle, at any depth: [`bundle_alias`]) is
/// refused before anything is created (`--latency-trace: <dest> is the
/// --table bundle` / `is in the --table bundle`). A destination that is the
/// configuration file (`--config <file>`, else
/// [`pin_app::ui::default_config_path`]), existing or not yet created, or
/// whose sibling temp-file name is the configuration path, is refused
/// before anything is created (`--latency-trace: <dest> is the
/// configuration file`, [`config_alias`]), so the shutdown commit can never
/// replace saved settings. A destination whose metadata cannot be read
/// for a reason other than "not found" (e.g. `trace.csv/` under a regular
/// file), a missing destination named with a trailing path separator, and
/// (Unix) an existing FIFO, which a write-only open would block on, are
/// refused before anything is opened; a device is opened nonblocking and
/// its descriptor checked again ([`open_direct`]). The destination itself is never
/// opened for writing here (unless it is a device) and never truncated: the
/// trace goes to a sibling temp file, created now with mode `0600` (Unix)
/// with `create_new` as the writability probe and renamed over the
/// destination by [`TraceSink::commit`] at shutdown. A temp-file candidate
/// that is the configuration save's scratch file
/// ([`AppConfig::save_scratch_path`], compared with [`is_config_path`]) is
/// skipped for the next one, so a settings save during the run never
/// renames the open trace file over the configuration (gate 163 round 14,
/// review comment 4713880926). The `--table` path is kept as the loaded
/// bundle the shutdown step rechecks ([`finish_trace`]).
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
    let config = config_path(args)?;
    if let Some(e) = config_alias(&path, dest_meta.as_ref(), &config) {
        return Err(e);
    }
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
    let table = path_of(args, "--table").and_then(Result::ok);
    if let Some(fm) = &dest_meta
        && let Some(table) = &table
        && let Some(alias) = bundle_alias(fm, &path, table)
    {
        let verb = match alias {
            BundleAlias::File => "is",
            BundleAlias::Member => "is in",
        };
        return Err(format!(
            "--latency-trace: {} {verb} the --table bundle",
            path.display()
        ));
    }
    if let Some(m) = &dest_meta
        && !m.is_file()
    {
        // A device (a directory fails to open): written directly.
        let file = open_direct(&path)?;
        return Ok(Some(TraceSink {
            dest: path,
            tmp: None,
            file: Some(file),
            config: Some(config),
            loaded: table,
        }));
    }
    if path.file_name().is_none() {
        return Err(io(std::io::Error::other("not a file name")));
    }
    let dir = trace_dir(&path);
    let pid = std::process::id();
    // The configuration save's own scratch file: never shared with the
    // trace (gate 163 round 14, review comment 4713880926).
    let scratch = AppConfig::save_scratch_path(&config);
    let mut last = None;
    for n in 0..TRACE_TMP_TRIES {
        let tmp = dir.join(trace_tmp_name(pid, n));
        if scratch
            .as_deref()
            .is_some_and(|s| is_config_path(&tmp, std::fs::metadata(&tmp).ok().as_ref(), s))
        {
            continue;
        }
        match owner_only(std::fs::OpenOptions::new().write(true).create_new(true)).open(&tmp) {
            Ok(file) => {
                return Ok(Some(TraceSink {
                    dest: path,
                    tmp: Some(tmp),
                    file: Some(file),
                    config: Some(config),
                    loaded: table,
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
/// can (the input profile replaces the keyboard router, first releasing
/// everything the old one holds at the current SDL time), otherwise one stderr line saying when
/// it applies ([`AppSetting::timing`]).
fn apply_config_change(
    platform: &Platform,
    forwarder: &mut EventForwarder,
    applied: &mut AppConfig,
    new: AppConfig,
) {
    for s in changed_settings(applied, &new) {
        if s == AppSetting::InputProfile {
            let router = keyboard_router(&new.input_profile).unwrap_or_else(|e| {
                eprintln!("pin_app: config: {e}");
                InputRouter::new()
            });
            // The old router's holds are released (at now) before the new
            // router sees any event, so nothing stays held (#197 finding 1).
            forwarder.replace_router(sdl_now_ns(), router);
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

/// What shapes every engine the run starts (at start-up and at each table
/// swap of the table browser, issue #85).
struct EngineOptions {
    /// `--vrr`.
    vrr: bool,
    /// `--debug-overlay`.
    debug_overlay: bool,
    /// `--profiler-hud`.
    profiler_hud: bool,
    /// The mixer configuration.
    mixer: MixerConfig,
    /// `--latency-trace` was given.
    tracing: bool,
}

/// The main thread's readers of one engine.
struct Engine {
    /// The profiler's reader (table runs only; the idle path is not
    /// profiled).
    profiler: Option<ProfilerReader>,
    /// The latency trace's collector (`--latency-trace`).
    trace: Option<TraceCollector>,
    /// Whether the render thread has a GPU and surfaces.
    rendering: bool,
}

/// The render thread's GPU: one surface per platform window, created here
/// on the main thread and moved to the render thread, which owns the device
/// (PLAN §2.2). `None` (with one stderr line) when SDL's video driver has no
/// native windows.
fn render_gpu(
    platform: &Platform,
    guards: &[WindowGuard],
    vrr: bool,
) -> Result<Option<RenderGpu>, String> {
    let driver = platform.video().current_video_driver();
    if !NATIVE_DRIVERS.contains(&driver) {
        eprintln!("pin_app: SDL video driver {driver:?} has no native windows; not rendering");
        return Ok(None);
    }
    let options = GpuOptions::for_app();
    let instance = create_instance(options.backends);
    let mut surfaces = Vec::with_capacity(guards.len());
    for (w, guard) in platform.windows().windows().zip(guards) {
        // SAFETY: each surface carries a clone of its window's guard
        // (`keep_alive`); the keeper destroys the window only after the
        // last clone is gone, on this thread (DECISIONS "6.0 — window
        // lifetime").
        let surface = unsafe { create_surface(&instance, &w.window) }.map_err(|e| e.to_string())?;
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
    Ok(Some(RenderGpu {
        instance,
        options,
        surfaces,
        surface_options: SurfaceOptions {
            vrr,
            force_presenter_thread: false,
        },
    }))
}

/// The thread configuration for `table` (the profiled table simulation with
/// its audio, render table and displays) or, without one, the idle path.
/// `wireframe` is the current `config.toml` setting (ORed with
/// `--debug-overlay`).
#[allow(clippy::too_many_arguments)]
fn engine_config(
    table: Option<LoadedApp>,
    input: SpscConsumer<InputEvent>,
    window: SpscConsumer<WindowEvent>,
    gpu: Option<RenderGpu>,
    sink: Box<dyn AudioSink>,
    menu: MenuShare,
    wireframe: bool,
    o: &EngineOptions,
) -> (ThreadsConfig, Engine) {
    let mut render = RenderConfig {
        gpu,
        window_events: Some(window),
        close_policy: ClosePolicy::PlayfieldOnly,
        table_render: TableRenderOptions {
            debug_overlay: o.debug_overlay || wireframe,
            ..TableRenderOptions::default()
        },
        menu: Some(menu),
        ..RenderConfig::default()
    };
    let mut profiler = None;
    let bindings = table
        .as_ref()
        .map_or_else(FlipperBindings::new, |t| t.bindings);
    let mut cfg = match table {
        Some(t) => {
            // The ProfiledSim's clock is the sim loop's: SDL's event
            // timestamp clock, which the input latencies are measured on.
            let (mut cfg, reader) =
                ThreadsConfig::with_profiled_sim(t.sim, input, sdl_now_ns, Profiler::new());
            render.present_profiler = cfg.render.present_profiler.take();
            render.hud = o.profiler_hud.then(|| reader.clone());
            profiler = Some(reader);
            cfg.audio = AudioConfig {
                haptics: Some(t.haptics),
                ssf: SsfConfig {
                    playfield_mm: ssf_playfield_mm(t.visuals.bounds),
                    ..SsfConfig::default()
                },
                sounds: Some(t.sounds),
                mixer: o.mixer,
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
            let mut cfg = ThreadsConfig::idle(input);
            cfg.audio.sink = Some(sink);
            cfg
        }
    };
    cfg.now_ns = sdl_now_ns;
    let mut trace = None;
    if o.tracing {
        // Every trace buffer is allocated here, before the threads start.
        let (collector, sim_writer, frame_writer) = latency_trace(bindings, sdl_now_ns);
        cfg.latency_trace = Some(sim_writer);
        render.latency_trace = Some(frame_writer);
        trace = Some(collector);
    }
    let rendering = render.gpu.is_some();
    cfg.render = render;
    (
        cfg,
        Engine {
            profiler,
            trace,
            rendering,
        },
    )
}

/// The run's cabinet feedback (KP-006, SPEC §4 3.4 / 3.5): per engine, the
/// telemetry sender thread and the sim thread's feed, the rumble driver and
/// the DOF bridge. [`attach`](Self::attach) wires a loaded table before its
/// engine threads start; [`detach`](Self::detach) runs after they were
/// joined (so the last datagrams flush) and stops the bridge, waiting for
/// it. Dropping it detaches too (an early error return).
struct Cabinet {
    /// The resolved telemetry destination; `None` when disabled in
    /// `config.toml` or when the endpoint did not resolve.
    telemetry: Option<SocketAddr>,
    /// `[dof] enabled`.
    dof_enabled: bool,
    link: Option<TelemetryLink>,
    feed: Option<TelemetryFeedStats>,
    /// Totals over every engine of the run.
    report: TelemetryReport,
    forced: u64,
    rumble: Option<RumbleDriver>,
    /// A command for the pads at the next [`service`](Self::service) (the
    /// "off" of an unloaded table).
    pending: Option<RumbleCommand>,
    dof: Option<DofBridge>,
}

impl Cabinet {
    /// The settings of `config`: a telemetry endpoint that does not resolve
    /// prints one line and disables telemetry (and so DOF) for the run.
    fn new(config: &AppConfig) -> Cabinet {
        let telemetry = if config.telemetry.enabled {
            parse_telemetry_endpoint(&config.telemetry.endpoint)
                .map_err(|e| eprintln!("pin_app: config: {e}; telemetry disabled"))
                .ok()
        } else {
            None
        };
        Cabinet {
            telemetry,
            dof_enabled: config.dof.enabled,
            link: None,
            feed: None,
            report: TelemetryReport::default(),
            forced: 0,
            rumble: None,
            pending: None,
            dof: None,
        }
    }

    /// Wires `app` before its engine threads start: opens the telemetry
    /// sender and gives its feed to the sim (a fresh encoder: full state on
    /// load), gives the sim a fresh rumble ring read by a new driver, and
    /// starts the DOF bridge when the table has `[dof]`.
    fn attach(&mut self, app: &mut LoadedApp) {
        if let Some(addr) = self.telemetry {
            match open_telemetry(&addr.to_string()) {
                Ok((tx, link)) => {
                    let feed = TelemetryFeed::new(tx, app.telemetry_map.clone());
                    self.feed = Some(feed.stats());
                    app.sim.set_telemetry(feed);
                    self.link = Some(link);
                }
                Err(e) => eprintln!("pin_app: {e}; this table runs without telemetry"),
            }
        }
        let (tx, rx) = haptics_ring::<HapticEvent>(DEFAULT_HAPTICS_RING_CAPACITY);
        app.sim.set_rumble(tx);
        let mut driver = RumbleDriver::new(RumbleSettings::default(), rx);
        if let Some(off) = driver.reset() {
            self.pending = Some(off);
        }
        self.rumble = Some(driver);
        self.start_dof(app);
    }

    /// Starts the DOF bridge for `app` when `[dof] enabled`, the table has
    /// `[dof]`, telemetry runs, the OS is supported and the bridge is
    /// installed; otherwise one line says why not. Errors never fail the run.
    fn start_dof(&mut self, app: &LoadedApp) {
        if !self.dof_enabled || app.manifest.dof.is_none() {
            return;
        }
        let Some(addr) = self.telemetry else {
            eprintln!("pin_app: dof: not started: telemetry is disabled (the bridge reads it)");
            return;
        };
        if !platform_supported() {
            eprintln!("pin_app: dof: not started: the DOF bridge runs on Windows only");
            return;
        }
        let Some(cfg) = DofLaunchConfig::for_app(addr.port()) else {
            eprintln!("pin_app: dof: not started: {BRIDGE_EXE_NAME} not found next to the engine");
            return;
        };
        match DofBridge::start_for_table(&app.manifest, &cfg) {
            Ok(Some(bridge)) => {
                eprintln!(
                    "pin_app: dof: bridge started (pid {}, port {})",
                    bridge.id(),
                    addr.port()
                );
                self.dof = Some(bridge);
            }
            Ok(None) => {}
            Err(e) => eprintln!("pin_app: dof: not started: {e}"),
        }
    }

    /// The table's engine threads were joined: stops the telemetry sender
    /// (final pump) and adds its counts, queues "rumble off" and drops the
    /// driver, and stops the DOF bridge, waiting for it.
    fn detach(&mut self) {
        if let Some(link) = self.link.take() {
            self.report.add(link.stop());
        }
        if let Some(feed) = self.feed.take() {
            self.forced += feed.forced_full_states();
        }
        if self.rumble.take().is_some() {
            self.pending = Some(RumbleCommand::OFF);
        }
        if let Some(bridge) = self.dof.take() {
            let status = bridge.stop().wait();
            eprintln!("pin_app: dof: bridge stopped ({status:?})");
        }
    }

    /// One main-loop step at sim tick `tick`: a pending command first, then
    /// the driver ([`RumbleDriver::service`]: a tick going backwards resets
    /// it).
    fn service(&mut self, tick: u64, out: &mut impl RumbleOutput) {
        if let Some(cmd) = self.pending.take() {
            out.set_gamepad_rumble(cmd);
        }
        if let Some(d) = self.rumble.as_mut() {
            d.service(tick, out);
        }
    }

    /// Shutdown (every engine thread joined): [`detach`](Self::detach),
    /// stops the pads, and returns the telemetry line.
    fn finish(&mut self, out: &mut impl RumbleOutput) -> String {
        self.detach();
        if let Some(cmd) = self.pending.take() {
            out.set_gamepad_rumble(cmd);
        }
        match self.telemetry {
            None => "pin_app: telemetry: disabled".to_owned(),
            Some(addr) => format!(
                "pin_app: telemetry: {} datagrams sent to {addr} ({} dropped, {} send errors, {} \
                 full states forced)",
                self.report.sent, self.report.ring_dropped, self.report.send_errors, self.forced
            ),
        }
    }
}

impl Drop for Cabinet {
    fn drop(&mut self) {
        self.detach();
    }
}

/// What a table swap needs from the run (issue #85).
struct SwapContext<'a> {
    platform: &'a Platform,
    guards: &'a [WindowGuard],
    menu_share: &'a MenuShare,
    headless: bool,
    options: &'a EngineOptions,
}

/// An engine's final counters ([`Threads::shutdown_with_audio`]).
type EngineReport = (StatsSnapshot, AudioStats);

/// What [`swap_table`] did.
enum Swap {
    /// The new table's engine runs.
    Started(Threads, Engine),
    /// A window close the close policy accepts was pending when the old
    /// engine stopped ([`stop_for_swap`]): the run quits instead of starting
    /// the new table. The old engine's final counters.
    Closed(EngineReport),
}

/// Stops the running engine `old` for a table swap: joins its threads and
/// then closes the old audio device stream (its sink went with the audio
/// thread). Returns the engine's final counters when a window close the
/// close policy accepts was requested before it stopped, also one the
/// render thread had not popped from the window ring yet (it records those
/// when it stops), so a close queued before the swap ends the run whatever
/// the render thread's timing (gate 163 round 9, review thread
/// 4703749444); `None` otherwise.
///
/// # Errors
/// A thread join error of the old engine.
fn stop_for_swap(
    old: Threads,
    audio_device: &mut Option<DeviceStream>,
) -> Result<Option<EngineReport>, String> {
    let report = old.shutdown_with_audio()?;
    // The audio thread was joined (its sink is gone): the stream can close.
    drop(audio_device.take());
    Ok(report.0.close_requested.then_some(report))
}

/// The table browser's swap (issue #85, DECISIONS "#85 — Table swap by
/// restarting the engine threads"): stops the running engine (`old`: the
/// threads are joined, the render thread drops the surfaces, the window
/// guards keep the windows; [`stop_for_swap`]), closes the old audio device
/// stream once its sink is gone with the audio thread, and, unless a window
/// close was pending ([`Swap::Closed`]: nothing is started), re-points the forwarder at fresh
/// input and window rings, recreates the surfaces on the same windows and
/// a fresh device stream, and starts the engine for `app` from tick 0 with
/// the current `wireframe` setting. The menu (in the forwarder) and the
/// windows persist.
///
/// # Errors
/// A thread join error of the old engine (the run then ends with it, as at
/// shutdown; the old threads are gone either way), or a surface or thread
/// spawn error for the new one.
#[allow(clippy::too_many_arguments)]
fn swap_table(
    old: Threads,
    mut app: LoadedApp,
    wireframe: bool,
    forwarder: &mut EventForwarder,
    audio_device: &mut Option<DeviceStream>,
    cabinet: &mut Cabinet,
    cx: &SwapContext<'_>,
) -> Result<Swap, String> {
    // Joins the old engine and closes its audio device stream (its sink
    // went with the audio thread).
    if let Some(report) = stop_for_swap(old, audio_device)? {
        return Ok(Swap::Closed(report));
    }
    // The old sim thread is gone: flush its telemetry, stop its bridge
    // (waiting for it) and its rumble, then wire the new table (KP-006).
    cabinet.detach();
    cabinet.attach(&mut app);
    let (input_tx, input_rx) = pin_core::spsc::channel(INPUT_RING_CAPACITY);
    let (window_tx, window_rx) = pin_core::spsc::channel(WINDOW_RING_CAPACITY);
    forwarder.replace_channels(input_tx, window_tx);
    let gpu = render_gpu(cx.platform, cx.guards, cx.options.vrr)?;
    let (device, sink) = audio_output(cx.platform, cx.headless, cx.options.mixer);
    *audio_device = device;
    let (cfg, engine) = engine_config(
        Some(app),
        input_rx,
        window_rx,
        gpu,
        sink,
        cx.menu_share.clone(),
        wireframe,
        cx.options,
    );
    Ok(Swap::Started(spawn_threads(cfg)?, engine))
}

/// Banks the trace of an engine stopped by a table swap (gate 163 round 9,
/// review thread 4703749460): `stopped` (the old engine's collector; its
/// threads were joined) is collected at `end_ns` and appended to `banked`
/// ([`CollectedTrace::append`]: joined on its own, so its ticks never meet
/// another engine's frames).
fn bank_trace(banked: &mut Option<CollectedTrace>, stopped: Option<TraceCollector>, end_ns: u64) {
    if let Some(c) = stopped {
        let t = c.collect(end_ns);
        match banked {
            Some(b) => b.append(t),
            None => *banked = Some(t),
        }
    }
}

/// The run's whole trace at shutdown: the banked traces of the engines
/// stopped by table swaps followed by the last engine's (`last`, collected
/// at `run_end_ns`, the trailer's `run_end_ns`). `None` without a trace.
fn final_trace(
    banked: Option<CollectedTrace>,
    last: Option<TraceCollector>,
    run_end_ns: u64,
) -> Option<CollectedTrace> {
    let last = last.map(|c| c.collect(run_end_ns));
    match (banked, last) {
        (Some(mut b), Some(l)) => {
            b.append(l);
            Some(b)
        }
        (Some(mut b), None) => {
            b.run_end_ns = run_end_ns;
            Some(b)
        }
        (None, l) => l,
    }
}

/// Whether any engine of the run has presented a playfield frame (gate 163
/// round 11, review 6070807835): the running engine's collector `current`
/// ([`TraceCollector::frames_presented`]) or the `banked` traces of the
/// engines stopped by table swaps ([`bank_trace`]), whose frame table holds
/// a row per presented playfield frame (a frame lost to a full ring is
/// counted in its `overflows.frames`). A frame the old engine presented
/// just before a swap banked it therefore still raises the run's one-shot
/// `pin_app: latency trace: first frame presented` line.
fn any_frame_presented(current: Option<&TraceCollector>, banked: Option<&CollectedTrace>) -> bool {
    current.is_some_and(|t| t.frames_presented() > 0)
        || banked.is_some_and(|b| !b.frames.is_empty() || b.overflows.frames > 0)
}

/// Whether the run's one-shot `pin_app: latency trace: first frame
/// presented` line is still due at shutdown (gate 163 round 12, review
/// 6076344934 finding 10): it was not `announced` in the run loop
/// ([`any_frame_presented`]) and the run's final trace `collected`
/// ([`final_trace`]) holds a presented playfield frame (a frame row, or a
/// frame lost to a full ring, counted in `overflows.frames`). A frame
/// presented in the pump that ended the run is therefore still announced.
fn first_frame_at_shutdown(announced: bool, collected: Option<&CollectedTrace>) -> bool {
    !announced && collected.is_some_and(|c| !c.frames.is_empty() || c.overflows.frames > 0)
}

/// Submits the menu's load request `(id, path)` to the I/O worker. When the
/// submission fails (the worker never started or has stopped: no reply can
/// come) the error is printed and reported to the menu with `id`, so it
/// leaves `Loading` and accepts another load (gate 163 round 9, review
/// thread 4703749471). A queued load stays asynchronous.
fn submit_load(worker: &IoWorker, menu: &mut MenuController, id: u64, path: PathBuf) {
    eprintln!("pin_app: table browser: loading {}", path.display());
    let shown = path.display().to_string();
    if let Err(e) = worker.try_request(IoRequest::Load { id, path }) {
        eprintln!("pin_app: table browser: loading {shown} failed: {e}");
        menu.finish_load(id, Err(e));
    }
}

/// Asks the I/O worker for a (coalesced, [`IoWorker::request_scan`]) scan
/// of `dir`; a failed submission is printed.
fn submit_scan(worker: &IoWorker, dir: &Path) {
    eprintln!("pin_app: table browser: scanning {}", dir.display());
    if let Err(e) = worker.request_scan(dir.to_path_buf()) {
        eprintln!(
            "pin_app: table browser: scanning {} failed: {e}",
            dir.display()
        );
    }
}

fn run_app(args: &[String]) -> Result<(), String> {
    reject_table_equals(args)?;
    // config.toml first (REQ-VIEW-006): its settings shape the windows.
    let (mut menu, config_lines) = MenuController::for_app(args)?;
    for line in &config_lines {
        eprintln!("{line}");
    }
    let mut applied = menu.config().clone();
    let headless = args.iter().any(|a| a == "--headless");
    let limits = RunLimits::from_args(args)?;
    let mut trace_file = open_latency_trace(args)?;
    // Set on the I/O worker when the browser guard refuses a selection that
    // is (or holds) the trace destination; read at shutdown after the
    // worker was joined ([`finish_trace`]).
    let trace_blocked = Arc::new(AtomicBool::new(false));
    let options = EngineOptions {
        vrr: args.iter().any(|a| a == "--vrr"),
        debug_overlay: args.iter().any(|a| a == "--debug-overlay"),
        profiler_hud: args.iter().any(|a| a == "--profiler-hud"),
        mixer: MixerConfig::default(),
        tracing: trace_file.is_some(),
    };
    // Load the table before opening windows: a bad table fails fast.
    let mut table = path_of(args, "--table")
        .transpose()?
        .map(|p| {
            load_app_table_with_budget(
                &p,
                options.mixer,
                menu.config().audio.decoded_budget_bytes(),
            )
        })
        .transpose()?;
    // Without `--table`: the table browser (issue #85) scans `<exe
    // dir>/tables` (or config.toml's `tables_dir`) on the I/O worker.
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("pin_app"));
    let browser = browser_dir(args, menu.config().tables_dir.as_deref(), &exe).map(|dir| {
        // While tracing, a selection that is the trace destination is
        // refused on the worker before it loads.
        let worker = match trace_file.as_ref() {
            Some(sink) => IoWorker::spawn_guarded(
                options.mixer,
                trace_alias_guard(sink.dest.clone(), Arc::clone(&trace_blocked)),
            ),
            None => IoWorker::spawn(options.mixer),
        };
        // The player's decoded-audio budget (SPEC §4 4.4, issue #129).
        worker.set_decoded_budget_bytes(menu.config().audio.decoded_budget_bytes());
        submit_scan(&worker, &dir);
        (dir, worker)
    });

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
    if browser.is_some() {
        // The table browser: menu loads and rescans go to the I/O worker,
        // and the menu opens on its Tables panel (the idle path runs under
        // it). Nothing is held yet, so opening releases nothing.
        menu.set_browser_enabled(true);
        menu.toggle(sdl_now_ns(), &mut |_| {});
    }
    forwarder.set_menu(menu);
    for id in platform.open_gamepads() {
        forwarder.gamepad_added(id.raw(), platform.gamepad_guid(id));
    }
    forwarder.register_windows(&ids);

    // Telemetry, rumble and DOF for the first table, before its sim thread
    // starts (KP-006).
    let mut cabinet = Cabinet::new(&applied);
    if let Some(t) = table.as_mut() {
        cabinet.attach(t);
    }
    let gpu = render_gpu(&platform, &guards, options.vrr)?;
    // Declared before `threads`: dropped after the audio thread was joined.
    let (mut audio_device, sink) = audio_output(&platform, headless, options.mixer);
    let (
        cfg,
        Engine {
            mut profiler,
            mut trace,
            mut rendering,
        },
    ) = engine_config(
        table,
        input_rx,
        window_rx,
        gpu,
        sink,
        menu_share.clone(),
        applied.wireframe,
        &options,
    );
    let mut threads = Some(spawn_threads(cfg)?);

    let started = Instant::now();
    let mut input_drops = DropMonitor::default();
    let mut failure = None;
    let mut first_frame_announced = false;
    // The traces of engines stopped by table swaps (`bank_trace`).
    let mut banked_trace: Option<CollectedTrace> = None;
    // The final counters of an engine stopped by a swap that found a
    // window close pending (`Swap::Closed`).
    let mut stopped_report: Option<EngineReport> = None;
    while let Some(running) = threads.as_ref() {
        let outcome = platform.pump(&mut forwarder, PUMP_TIMEOUT);
        if let Some(new) = forwarder
            .menu_mut()
            .and_then(MenuController::take_config_change)
        {
            // A changed decoded-audio budget applies to the next table load
            // (DECISIONS "Human ruling — decoded-audio budget").
            if let Some((_, worker)) = &browser {
                worker.set_decoded_budget_bytes(new.audio.decoded_budget_bytes());
            }
            apply_config_change(&platform, &mut forwarder, &mut applied, new);
        }
        if let Some(n) = input_drops.check(forwarder.stats().input_dropped) {
            eprintln!(
                "pin_app: input ring full, dropped {n} event(s) ({} total)",
                input_drops.total()
            );
        }
        if outcome.quit || running.close_requested() {
            break;
        }
        if let Some((dir, worker)) = &browser {
            // The main thread only posts requests and applies replies (R4).
            if let Some(m) = forwarder.menu_mut() {
                if m.take_rescan_request() {
                    submit_scan(worker, dir);
                }
                if let Some((id, path)) = m.take_load_request() {
                    submit_load(worker, m, id, path);
                }
            }
            while let Some(reply) = worker.try_recv() {
                match reply {
                    IoReply::Scanned { root, tables } => {
                        let ready = tables.iter().filter(|t| t.is_ready()).count();
                        eprintln!(
                            "pin_app: table browser: {} table(s) in {} ({ready} ready)",
                            tables.len(),
                            root.display()
                        );
                        if let Some(m) = forwarder.menu_mut() {
                            m.set_tables(tables);
                        }
                    }
                    IoReply::Loaded { id, path, app } => {
                        let pending = forwarder.menu().and_then(|m| m.menu().pending_load_id());
                        if pending != Some(id) {
                            // A stale load (never the case today): dropped.
                            continue;
                        }
                        let wireframe = forwarder.menu().is_some_and(|m| m.config().wireframe);
                        let name = path.file_name().map_or_else(
                            || path.display().to_string(),
                            |n| n.to_string_lossy().into_owned(),
                        );
                        eprintln!("pin_app: table swap: {name} (wireframe={wireframe})");
                        let Some(old) = threads.take() else { break };
                        let cx = SwapContext {
                            platform: &platform,
                            guards: &guards,
                            menu_share: &menu_share,
                            headless,
                            options: &options,
                        };
                        match swap_table(
                            old,
                            *app,
                            wireframe,
                            &mut forwarder,
                            &mut audio_device,
                            &mut cabinet,
                            &cx,
                        ) {
                            Ok(Swap::Started(t, e)) => {
                                threads = Some(t);
                                // The bundle rechecked at shutdown.
                                if let Some(sink) = trace_file.as_mut() {
                                    sink.set_loaded(path.clone());
                                }
                                // The stopped engine's trace is kept.
                                bank_trace(&mut banked_trace, trace.take(), sdl_now_ns());
                                profiler = e.profiler;
                                trace = e.trace;
                                rendering = e.rendering;
                                if let Some(m) = forwarder.menu_mut() {
                                    m.finish_load(id, Ok(()));
                                }
                            }
                            Ok(Swap::Closed(report)) => {
                                // A close queued before the swap ends the
                                // run; the new table is not started.
                                eprintln!(
                                    "pin_app: table swap cancelled: a window close was requested"
                                );
                                stopped_report = Some(report);
                                break;
                            }
                            Err(e) => {
                                eprintln!("pin_app: table swap failed: {e}");
                                if let Some(m) = forwarder.menu_mut() {
                                    m.finish_load(id, Err(e.clone()));
                                }
                                failure = Some(format!("table swap: {e}"));
                                break;
                            }
                        }
                    }
                    IoReply::LoadFailed { id, path, error } => {
                        eprintln!(
                            "pin_app: table browser: loading {} failed: {error}",
                            path.display()
                        );
                        if let Some(m) = forwarder.menu_mut() {
                            m.finish_load(id, Err(error));
                        }
                    }
                }
            }
        }
        let Some(running) = threads.as_ref() else {
            break;
        };
        let live = running.stats();
        cabinet.service(live.sim_ticks, &mut platform);
        // Only a playfield present (the trace's own frame count) raises it.
        if !first_frame_announced && any_frame_presented(trace.as_ref(), banked_trace.as_ref()) {
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
    // The run's end on the trace clock (`# run_end_ns`), read as soon as
    // the loop ended: before the threads are joined and before waiting on
    // the I/O worker (gate 163 round 9, review thread 4703749463).
    let run_end_ns = sdl_now_ns();
    // 1. Join the engine threads (the render thread drops the surfaces; a
    //    presenter stalled past PRESENTER_JOIN_TIMEOUT is detached and keeps
    //    its guard).
    //    A join error (a panicked thread) is recorded, never returned early:
    //    a detached presenter may still hold a window guard, so the teardown
    //    below must still run in order before `platform` is dropped.
    //    (No threads: a table swap already joined them; a failed one
    //    recorded the failure, one cancelled by a pending close kept the
    //    final counters.)
    let (report, join_error) = match threads.take().map(Threads::shutdown_with_audio) {
        Some(Ok(done)) => (Some(done), None),
        Some(Err(e)) => {
            eprintln!("pin_app: {e}");
            (None, Some(e))
        }
        None => (stopped_report, None),
    };
    failure = merge_failure(failure, join_error);
    // The I/O worker stops after the engine (a load in flight completes;
    // its result is dropped), before the platform's teardown below.
    drop(browser);
    // The selection the worker may have dropped unchecked at its stop.
    let in_flight = forwarder
        .menu()
        .and_then(unanswered_selection)
        .map(Path::to_path_buf);
    // Every engine thread is joined: flush the telemetry, stop the pads and
    // the DOF bridge (waiting for it) on every exit path below (KP-006).
    let telemetry_line = cabinet.finish(&mut platform);
    // Every thread is joined: drain the trace rings and write the file here,
    // on the main thread, also when the run failed (for diagnosis). The
    // file holds every engine of the run: those stopped by table swaps
    // (banked), then the last (`final_trace`); after a failed swap the old
    // collector is still `trace`, so nothing is lost.
    let collected = final_trace(banked_trace, trace, run_end_ns);
    // A first frame presented in the pump that ended the run (or by an
    // engine a close-cancelled swap stopped) still gets its one-shot line.
    if first_frame_at_shutdown(first_frame_announced, collected.as_ref()) {
        eprintln!("pin_app: latency trace: first frame presented");
    }
    let trace_summary = match (collected, trace_file.take()) {
        (Some(collected), Some(sink)) => finish_trace(
            sink,
            &trace_blocked,
            in_flight.as_deref(),
            collected.to_csv().as_bytes(),
            &mut failure,
        )
        .map(|dest| summary_line(&dest, &collected)),
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
        if let Some(reader) = profiler.as_ref().filter(|_| options.profiler_hud) {
            for line in hud_lines(&reader.snapshot()) {
                eprintln!("pin_app: profiler: {line}");
            }
        }
    }
    eprintln!("{telemetry_line}");
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

    /// A fresh, empty temp directory unique to this process and call. On
    /// Linux any default ACL it inherited from the temp root is removed
    /// (best effort), so files created in it carry no access ACL whatever
    /// the temp root's ACLs are (REQ-LAT-003, gate 30 round 4, review
    /// thread 4699765184). A test that needs a default ACL installs its own
    /// after this call.
    fn fresh_dir(tag: &str) -> PathBuf {
        static N: AtomicU64 = AtomicU64::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("kp_trace_{tag}_{}_{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        #[cfg(target_os = "linux")]
        {
            use std::os::unix::ffi::OsStrExt as _;
            if let Ok(c) = std::ffi::CString::new(dir.as_os_str().as_bytes()) {
                // SAFETY: `c` and the name are NUL-terminated C strings that
                // live for the call. Best effort: ENODATA (no default ACL),
                // ENOTSUP/EOPNOTSUPP (no xattrs) and any other failure are
                // ignored.
                let _ =
                    unsafe { libc::removexattr(c.as_ptr(), c"system.posix_acl_default".as_ptr()) };
            }
        }
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
    /// directory aliases the trace file only when it holds it.
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
        // A bundle directory that does not hold the trace is no alias; one
        // that holds it is refused (gate 163 round 10, comment 4704516459).
        let folder = dir.join("folder");
        std::fs::create_dir_all(&folder).unwrap();
        let f = folder.to_str().unwrap();
        assert!(
            open_latency_trace(&args(&["--table", f, "--latency-trace", o]))
                .unwrap()
                .is_some()
        );
        let d = dir.to_str().unwrap();
        assert_eq!(
            open_latency_trace(&args(&["--table", d, "--latency-trace", o])).unwrap_err(),
            format!("--latency-trace: {o} is in the --table bundle")
        );
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "new");
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
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        if !matches!(&made, Ok(s) if s.success()) {
            // Never a silent pass: a missing or failing mkfifo fails the
            // test (after cleaning up).
            let _ = std::fs::remove_dir_all(&dir);
            panic!("mkfifo is required for the FIFO check and failed: {made:?}");
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

    /// REQ-LAT-003 (C2, CWE-732; gate 30 round 6, owner decision "Always
    /// 0600"): the temp file is owner-only (`0600`) from creation, before
    /// any write, and is never chmod-ed: replacing an existing `0600` or
    /// `0640` destination publishes exactly `0600`, and a new destination
    /// ends `0600`, so the rename never widens access.
    #[test]
    #[cfg(unix)]
    fn req_lat_003_trace_commit_never_widens_permissions() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = fresh_dir("perm");
        for old in [0o600, 0o640] {
            let p = dir.join(format!("t{old:o}.csv"));
            std::fs::write(&p, "old").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(old)).unwrap();
            let sink = open_latency_trace(&args(&["--latency-trace", p.to_str().unwrap()]))
                .unwrap()
                .unwrap();
            let tmp = sink.tmp.clone().unwrap();
            assert_eq!(mode(&tmp), 0o600, "temp owner-only before any write");
            sink.commit(b"new").unwrap();
            assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
            assert_eq!(mode(&p), 0o600, "a {old:o} destination is published 0600");
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

    /// Opens the real `--latency-trace` sink on `p`, commits `csv` and
    /// returns the published file's mode bits (`0o777`).
    #[cfg(unix)]
    fn commit_and_mode(p: &Path, csv: &[u8]) -> u32 {
        let sink = open_latency_trace(&args(&["--latency-trace", p.to_str().unwrap()]))
            .unwrap()
            .unwrap();
        sink.commit(csv).unwrap();
        mode(p)
    }

    /// REQ-LAT-003 (gate 30 round 6, owner decision "Always 0600",
    /// CWE-732): an existing `0644` destination owned by the running user is
    /// replaced through the real commit path by an owner-only (`0600`)
    /// trace with the new content; the destination's bits are not copied.
    #[test]
    #[cfg(unix)]
    fn req_lat_003_existing_0644_destination_is_published_0600() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = fresh_dir("d0644");
        let p = dir.join("t.csv");
        std::fs::write(&p, "old").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(mode(&p), 0o644, "the scenario is real");
        assert_eq!(commit_and_mode(&p, b"new trace"), 0o600);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new trace");
        assert_eq!(names(&dir), vec!["t.csv".to_owned()]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-LAT-003 (gate 30 round 6, owner decision "Always 0600",
    /// CWE-732): existing group- and world-writable destinations (`0664`,
    /// `0666`, set explicitly so the umask does not matter) are each
    /// replaced by an owner-only (`0600`) trace through the real commit.
    #[test]
    #[cfg(unix)]
    fn req_lat_003_group_and_world_writable_destinations_are_published_0600() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = fresh_dir("dwide");
        for old in [0o664, 0o666] {
            let p = dir.join(format!("t{old:o}.csv"));
            std::fs::write(&p, "old").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(old)).unwrap();
            assert_eq!(mode(&p), old, "the scenario is real");
            assert_eq!(commit_and_mode(&p, b"new"), 0o600, "was {old:o}");
            assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
        }
        assert_eq!(
            names(&dir),
            vec!["t664.csv".to_owned(), "t666.csv".to_owned()]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-LAT-003 (gate 30 round 6, CodeRabbit comment 4700710254,
    /// CWE-367/CWE-732): a destination chmod-ed after the sink is opened and
    /// before the commit (to `0644`, `0666` and `0604`) cannot change the
    /// published mode: the trace is `0600` every time, because the commit
    /// never reads the destination's permissions and never chmods the temp
    /// file. The finding's cross-owner case (a destination owned by another
    /// user, published as `0604`) cannot be set up with one user; the code
    /// path that depended on the destination's mode, owner, group or ACL is
    /// gone, so no destination change at any moment reaches the result.
    #[test]
    #[cfg(unix)]
    fn req_lat_003_destination_chmod_after_open_still_publishes_0600() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = fresh_dir("chmodrace");
        for late in [0o644, 0o666, 0o604] {
            let p = dir.join(format!("t{late:o}.csv"));
            std::fs::write(&p, "old").unwrap();
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o600)).unwrap();
            let sink = open_latency_trace(&args(&["--latency-trace", p.to_str().unwrap()]))
                .unwrap()
                .unwrap();
            // The change lands after the sink was opened, before the commit.
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(late)).unwrap();
            assert_eq!(mode(&p), late, "the scenario is real");
            sink.commit(b"new").unwrap();
            assert_eq!(mode(&p), 0o600, "chmod {late:o} after open");
            assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// `fgetxattr(fd, name)` into a buffer: the value, or the errno.
    #[cfg(target_os = "linux")]
    fn fgetxattr(file: &std::fs::File, name: &std::ffi::CStr) -> Result<Vec<u8>, i32> {
        use std::os::fd::AsRawFd as _;
        let mut buf = vec![0u8; 4096];
        // SAFETY: `file`'s descriptor is open for the call, `name` is a
        // NUL-terminated C string and `buf` is writable for `buf.len()`.
        let r = unsafe {
            libc::fgetxattr(
                file.as_raw_fd(),
                name.as_ptr(),
                buf.as_mut_ptr().cast(),
                buf.len(),
            )
        };
        let Ok(n) = usize::try_from(r) else {
            return Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0));
        };
        buf.truncate(n);
        Ok(buf)
    }

    /// `setxattr(path, name, value)`: `Ok` or the errno.
    #[cfg(target_os = "linux")]
    fn setxattr(path: &Path, name: &std::ffi::CStr, value: &[u8]) -> Result<(), i32> {
        use std::os::unix::ffi::OsStrExt as _;
        let p = std::ffi::CString::new(path.as_os_str().as_bytes()).unwrap();
        // SAFETY: `p` and `name` are NUL-terminated C strings and `value`
        // is readable for `value.len()` bytes for the call.
        let r = unsafe {
            libc::setxattr(
                p.as_ptr(),
                name.as_ptr(),
                value.as_ptr().cast(),
                value.len(),
                0,
            )
        };
        if r == 0 {
            Ok(())
        } else {
            Err(std::io::Error::last_os_error().raw_os_error().unwrap_or(0))
        }
    }

    /// POSIX ACL xattr tags (Linux `posix_acl_xattr.h`).
    #[cfg(target_os = "linux")]
    const ACL_USER: u16 = 0x02;
    #[cfg(target_os = "linux")]
    const ACL_MASK: u16 = 0x10;

    /// The `(tag, perm, id)` entries of a version-2 POSIX ACL xattr blob.
    #[cfg(target_os = "linux")]
    fn acl_entries(blob: &[u8]) -> Vec<(u16, u16, u32)> {
        assert!(
            blob.len() >= 4 && (blob.len() - 4).is_multiple_of(8),
            "{blob:?}"
        );
        assert_eq!(u32::from_le_bytes(blob[..4].try_into().unwrap()), 2);
        blob[4..]
            .chunks_exact(8)
            .map(|e| {
                (
                    u16::from_le_bytes([e[0], e[1]]),
                    u16::from_le_bytes([e[2], e[3]]),
                    u32::from_le_bytes([e[4], e[5], e[6], e[7]]),
                )
            })
            .collect()
    }

    /// REQ-LAT-003 (gate 30 round 6, owner decision "Always 0600",
    /// CWE-732): in a directory whose default ACL grants a named user read
    /// (`user:65534:r--`), the trace temp file inherits that entry but is
    /// created `0600`, so its ACL mask is `---`; the commit never chmods it,
    /// so the published trace (a new destination, and one replacing an
    /// existing `0644` file) is `0600` and its access ACL, if any, has an
    /// effective mask of `---` (the named user gets nothing).
    ///
    /// The default-ACL fixture must be set up (gate 163 round 11, review
    /// 6070807835, R10): when `setxattr` fails and either
    /// `GITHUB_ACTIONS=true` (the CI Linux environment, which provides
    /// POSIX ACLs) or `KP_REQUIRE_POSIX_ACL=1` is set, the test fails with
    /// the errno. Only locally, with neither set, an unsupported filesystem
    /// prints one `SKIPPED ...` line and returns: an unsupported
    /// environment, not a pass of the property.
    #[test]
    #[cfg(target_os = "linux")]
    fn req_lat_003_default_acl_dir_never_opens_the_trace_to_named_users() {
        use std::os::unix::fs::PermissionsExt as _;
        const NAMED: u32 = 65534;
        let dir = fresh_dir("acl");
        let me = std::os::unix::fs::MetadataExt::uid(&std::fs::metadata(&dir).unwrap());
        assert_ne!(me, NAMED, "the named user must be someone else");
        // Created before the default ACL: no access ACL of its own.
        let old = dir.join("old.csv");
        std::fs::write(&old, "old").unwrap();
        std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o644)).unwrap();
        // Default ACL: user::rwx, user:NAMED:r--, group::r--, mask::rwx,
        // other::r-- (ascending tag order).
        let blob = acl_blob(&[
            (0x01, 7, u32::MAX),
            (ACL_USER, 4, NAMED),
            (0x04, 4, u32::MAX),
            (ACL_MASK, 7, u32::MAX),
            (0x20, 4, u32::MAX),
        ]);
        if let Err(errno) = setxattr(&dir, c"system.posix_acl_default", &blob) {
            std::fs::remove_dir_all(&dir).unwrap();
            let unsupported =
                [libc::ENOTSUP, libc::EOPNOTSUPP, libc::EPERM, libc::ENODATA].contains(&errno);
            if !unsupported {
                panic!("setxattr(system.posix_acl_default) failed: errno {errno}");
            }
            if acl_required(
                std::env::var_os("GITHUB_ACTIONS").as_deref(),
                std::env::var_os("KP_REQUIRE_POSIX_ACL").as_deref(),
            ) {
                panic!(
                    "POSIX ACL fixture setup failed (errno {errno}) but ACLs are required here \
                     (GITHUB_ACTIONS=true or KP_REQUIRE_POSIX_ACL=1): the ACL property was not tested"
                );
            }
            eprintln!(
                "SKIPPED req_lat_003_default_acl_dir_never_opens_the_trace_to_named_users: POSIX ACLs unsupported here (errno {errno}) (set KP_REQUIRE_POSIX_ACL=1 to fail instead)"
            );
            return;
        }
        let fresh = dir.join("fresh.csv");
        for p in [&fresh, &old] {
            let sink = open_latency_trace(&args(&["--latency-trace", p.to_str().unwrap()]))
                .unwrap()
                .unwrap();
            // The scenario is real: the temp file inherited the named entry.
            let inherited = fgetxattr(sink.file.as_ref().unwrap(), c"system.posix_acl_access")
                .expect("the temp file inherits the directory's default ACL");
            assert!(
                acl_entries(&inherited)
                    .iter()
                    .any(|e| e.0 == ACL_USER && e.2 == NAMED),
                "{:?}",
                acl_entries(&inherited)
            );
            sink.commit(b"new").unwrap();
            assert_eq!(std::fs::read_to_string(p).unwrap(), "new");
            // The group class reports the ACL mask: 0.
            assert_eq!(mode(p), 0o600, "{}", p.display());
            let f = std::fs::File::open(p).unwrap();
            match fgetxattr(&f, c"system.posix_acl_access") {
                Ok(acl) => {
                    let entries = acl_entries(&acl);
                    let mask = entries
                        .iter()
                        .find(|e| e.0 == ACL_MASK)
                        .map(|e| e.1)
                        .expect("an ACL with a named user has a mask");
                    assert_eq!(mask, 0, "ACL mask is ---: {entries:?}");
                    for e in entries.iter().filter(|e| e.0 == ACL_USER) {
                        assert_eq!(e.1 & mask, 0, "named user {} gets no access", e.2);
                    }
                }
                // No access ACL: the 0600 mode alone denies every other user.
                Err(errno) => assert_eq!(errno, libc::ENODATA, "reading the published ACL"),
            }
        }
        assert_eq!(
            names(&dir),
            vec!["fresh.csv".to_owned(), "old.csv".to_owned()]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-LAT-003 (gate 30 round 4, review thread 4699765150, CWE-732;
    /// gate 30 round 6, owner decision "Always 0600"): a symlinked trace
    /// path whose target is an ACL-free `0644` file with the temp file's
    /// owner and group supplies no permission bits: the published file at
    /// the link path is a regular file with the created `0600`, and the
    /// target's bytes and `0644` are unchanged.
    #[test]
    #[cfg(unix)]
    fn req_lat_003_symlinked_trace_path_publishes_0600() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = fresh_dir("sym0644");
        let target = dir.join("target.csv");
        std::fs::write(&target, b"target bytes").unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o644)).unwrap();
        let link = dir.join("link.csv");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let sink = open_latency_trace(&args(&["--latency-trace", link.to_str().unwrap()]))
            .unwrap()
            .unwrap();
        #[cfg(target_os = "linux")]
        {
            let acl = fgetxattr(sink.file.as_ref().unwrap(), c"system.posix_acl_access");
            assert!(
                matches!(acl, Err(e) if [libc::ENODATA, libc::ENOTSUP, libc::EOPNOTSUPP].contains(&e)),
                "the temp file has no access ACL: {acl:?}"
            );
        }
        sink.commit(b"csv").unwrap();
        let m = std::fs::symlink_metadata(&link).unwrap();
        assert!(m.is_file(), "the link path is now a regular file");
        assert_eq!(
            m.permissions().mode() & 0o777,
            0o600,
            "never the target's 0644"
        );
        assert_eq!(std::fs::read(&link).unwrap(), b"csv");
        assert_eq!(std::fs::read(&target).unwrap(), b"target bytes");
        assert_eq!(mode(&target), 0o644, "the target's mode is unchanged");
        assert_eq!(
            names(&dir),
            vec!["link.csv".to_owned(), "target.csv".to_owned()]
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A version-2 POSIX ACL xattr blob (little-endian) of `(tag, perm,
    /// id)` entries, given in ascending tag order.
    #[cfg(target_os = "linux")]
    fn acl_blob(entries: &[(u16, u16, u32)]) -> Vec<u8> {
        let mut blob = 2u32.to_le_bytes().to_vec();
        for &(tag, perm, id) in entries {
            blob.extend_from_slice(&tag.to_le_bytes());
            blob.extend_from_slice(&perm.to_le_bytes());
            blob.extend_from_slice(&id.to_le_bytes());
        }
        blob
    }

    /// REQ-LAT-003 (gate 30, M4): the temp file name does not contain the
    /// destination's name, so a destination whose name is 255 bytes (the
    /// usual file-name limit) still gets a temp file and commits.
    #[test]
    fn req_lat_003_long_trace_file_name_still_commits() {
        let dir = fresh_dir("long");
        let name = format!("{}.csv", "t".repeat(251));
        assert_eq!(name.len(), 255);
        let p = dir.join(&name);
        std::fs::write(&p, "old").unwrap();
        let sink = open_latency_trace(&args(&["--latency-trace", p.to_str().unwrap()]))
            .unwrap()
            .unwrap();
        let tmp = sink.tmp.clone().unwrap();
        let tmp_name = tmp.file_name().unwrap().to_str().unwrap().to_owned();
        assert!(
            tmp_name.starts_with(".kp-trace.") && tmp_name.ends_with(".tmp"),
            "{tmp_name}"
        );
        assert!(!tmp_name.contains(&name));
        sink.commit(b"new").unwrap();
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
        assert_eq!(names(&dir), vec![name]);
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

    /// Gate 163 round 9 #7 (review thread 4703749482, REQ-VIEW-006):
    /// `--table=<path>` is refused with one clear error; the separate form
    /// and other flags pass.
    #[test]
    fn gate163_r9_table_equals_is_rejected() {
        use super::reject_table_equals;
        let want = "--table=<path> is not supported; use --table <path>";
        for bad in [
            &["--table=demo.zip"][..],
            &["--table="],
            &["--headless", "--table=t", "--run-ms", "1"],
        ] {
            assert_eq!(
                reject_table_equals(&args(bad)).unwrap_err(),
                want,
                "{bad:?}"
            );
        }
        for ok in [
            &[][..],
            &["--table", "demo.zip"],
            &["--tables", "x"],
            &["--latency-trace", "t.csv"],
        ] {
            assert_eq!(reject_table_equals(&args(ok)), Ok(()), "{ok:?}");
        }
    }

    /// Gate 163 round 9 #2 (review thread 4703749453, REQ-LAT-003): the
    /// alias test shared by the start-up check and the browser guard: the
    /// trace destination is the bundle when both are the same existing
    /// file (also through a hard link); another file, a missing
    /// destination or a folder bundle that does not hold it is not.
    #[test]
    fn gate163_r9_browser_selection_alias_guard() {
        use super::{bundle_alias, trace_alias_guard};
        use std::sync::Arc;
        use std::sync::atomic::AtomicBool;
        let dir = fresh_dir("r9_alias");
        let dest = dir.join("t.zip");
        std::fs::write(&dest, b"PK bundle bytes").unwrap();
        let other = dir.join("other.zip");
        std::fs::write(&other, b"PK other").unwrap();
        let folder = dir.join("folder");
        std::fs::create_dir_all(&folder).unwrap();
        let m = std::fs::metadata(&dest).unwrap();
        assert!(bundle_alias(&m, &dest, &dest).is_some());
        assert!(bundle_alias(&m, &dest, &other).is_none());
        // `dest` is not inside `folder`.
        assert!(bundle_alias(&m, &dest, &folder).is_none());
        assert!(bundle_alias(&m, &dest, &dir.join("missing.zip")).is_none());
        let flag = Arc::new(AtomicBool::new(false));
        let guard = trace_alias_guard(dest.clone(), Arc::clone(&flag));
        let want = format!(
            "--latency-trace: {} is the selected table bundle",
            dest.display()
        );
        assert_eq!(guard(&dest).unwrap_err(), want);
        #[cfg(unix)]
        {
            let hard = dir.join("hard.zip");
            std::fs::hard_link(&dest, &hard).unwrap();
            assert_eq!(guard(&hard).unwrap_err(), want);
        }
        assert_eq!(guard(&other), Ok(()), "a separate path is unaffected");
        assert_eq!(guard(&folder), Ok(()));
        // A destination that does not exist is no bundle.
        assert_eq!(
            trace_alias_guard(dir.join("new.csv"), Arc::new(AtomicBool::new(false)))(&dest),
            Ok(())
        );
        // The browser's guarded worker refuses the aliased selection before
        // loading it; the archive is left intact.
        let w = pin_app::orchestrator::IoWorker::spawn_guarded(
            pin_audio::mixer::MixerConfig::default(),
            trace_alias_guard(dest.clone(), Arc::new(AtomicBool::new(false))),
        );
        for (id, path) in [(1, dest.clone()), (2, other.clone())] {
            w.try_request(pin_app::orchestrator::IoRequest::Load { id, path })
                .unwrap();
        }
        match w.recv() {
            Some(pin_app::orchestrator::IoReply::LoadFailed { id: 1, error, .. }) => {
                assert_eq!(error, want);
            }
            other => panic!("{other:?}"),
        }
        match w.recv() {
            Some(pin_app::orchestrator::IoReply::LoadFailed { id: 2, error, .. }) => {
                assert!(!error.contains("--latency-trace"), "{error}");
            }
            other => panic!("{other:?}"),
        }
        drop(w);
        assert_eq!(std::fs::read(&dest).unwrap(), b"PK bundle bytes");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// A guarded I/O worker as the table browser starts it while tracing
    /// to `dest`, and its shared "trace blocked" flag.
    fn r10_worker(
        dest: &Path,
    ) -> (
        pin_app::orchestrator::IoWorker,
        std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        let flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let w = pin_app::orchestrator::IoWorker::spawn_guarded(
            pin_audio::mixer::MixerConfig::default(),
            super::trace_alias_guard(dest.to_path_buf(), std::sync::Arc::clone(&flag)),
        );
        (w, flag)
    }

    /// A trace sink for `dest` as `--latency-trace <dest>` opens it.
    fn r10_sink(dest: &Path) -> super::TraceSink {
        open_latency_trace(&args(&["--latency-trace", dest.to_str().unwrap()]))
            .unwrap()
            .unwrap()
    }

    /// Gate 163 round 10 #1 (REQ-LAT-003, review thread 4703749453 comment
    /// 4704457456): a `.zip` bundle selected in the browser that is the
    /// trace destination is refused on the worker, and the shutdown step
    /// ([`super::finish_trace`]) then does not publish the trace: the
    /// archive keeps its bytes, no temp file is left, the run fails with a
    /// message naming the destination.
    #[test]
    fn gate163_r10_zip_alias_blocks_trace_publication() {
        use super::finish_trace;
        use pin_app::orchestrator::{IoReply, IoRequest};
        use std::sync::atomic::Ordering;
        let dir = fresh_dir("r10_zip");
        let dest = dir.join("t.zip");
        std::fs::write(&dest, b"PK bundle bytes").unwrap();
        let sink = r10_sink(&dest);
        let (w, flag) = r10_worker(&dest);
        w.try_request(IoRequest::Load {
            id: 1,
            path: dest.clone(),
        })
        .unwrap();
        assert!(matches!(w.recv(), Some(IoReply::LoadFailed { id: 1, .. })));
        drop(w);
        assert!(flag.load(Ordering::Acquire));
        let mut failure = None;
        assert_eq!(
            finish_trace(sink, &flag, None, b"csv,trace\n", &mut failure),
            None
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"PK bundle bytes");
        assert_eq!(names(&dir), ["t.zip"]);
        let f = failure.expect("the run fails");
        assert!(f.contains(&dest.display().to_string()), "{f}");
        assert!(f.contains("the trace was not published"), "{f}");
        assert!(f.starts_with("--latency-trace: "), "{f}");
        // An earlier failure is kept.
        let sink = r10_sink(&dest);
        let mut failure = Some("run cap".to_owned());
        assert_eq!(finish_trace(sink, &flag, None, b"x", &mut failure), None);
        assert!(failure.unwrap().starts_with("run cap; --latency-trace: "));
        assert_eq!(names(&dir), ["t.zip"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Gate 163 round 10 #2 (REQ-LAT-003, review comment 4704516459): with
    /// `--latency-trace <folder>/table.toml`, selecting the folder bundle
    /// is refused before it loads (`... is in the selected table bundle`),
    /// and the shutdown step does not replace `table.toml` with the trace.
    #[test]
    fn gate163_r10_folder_member_blocks_trace_publication() {
        use super::finish_trace;
        use pin_app::orchestrator::{IoReply, IoRequest};
        let dir = fresh_dir("r10_folder");
        let folder = dir.join("example");
        std::fs::create_dir_all(&folder).unwrap();
        let dest = folder.join("table.toml");
        std::fs::write(&dest, b"[table]\nname = \"x\"\n").unwrap();
        let sink = r10_sink(&dest);
        let (w, flag) = r10_worker(&dest);
        w.try_request(IoRequest::Load {
            id: 7,
            path: folder.clone(),
        })
        .unwrap();
        match w.recv() {
            Some(IoReply::LoadFailed { id: 7, error, .. }) => assert_eq!(
                error,
                format!(
                    "--latency-trace: {} is in the selected table bundle",
                    dest.display()
                )
            ),
            other => panic!("the load must not succeed: {other:?}"),
        }
        drop(w);
        let mut failure = None;
        assert_eq!(
            finish_trace(sink, &flag, None, b"csv\n", &mut failure),
            None
        );
        assert_eq!(
            std::fs::read(&dest).unwrap(),
            b"[table]\nname = \"x\"\n",
            "table.toml unchanged"
        );
        assert_eq!(names(&folder), ["table.toml"]);
        assert!(failure.is_some());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Gate 163 round 10 #1/#2 (REQ-LAT-003, review thread 4703749453
    /// comment 4704457456, comment 4704516459): shutdown while the aliased
    /// load is in flight. The worker is stopped right after the request is
    /// queued: it either ran the guard (the flag is set) or dropped the
    /// request unchecked; either way the shutdown step, given the menu's
    /// unanswered selection, does not publish the trace.
    #[test]
    fn gate163_r10_shutdown_with_load_in_flight_blocks_publication() {
        use super::finish_trace;
        use pin_app::orchestrator::IoRequest;
        let dir = fresh_dir("r10_race");
        let zip = dir.join("t.zip");
        std::fs::write(&zip, b"PK bundle bytes").unwrap();
        let folder = dir.join("example");
        std::fs::create_dir_all(&folder).unwrap();
        let member = folder.join("table.toml");
        std::fs::write(&member, b"manifest").unwrap();
        for _ in 0..20 {
            for (dest, bundle, bytes) in [
                (&zip, &zip, &b"PK bundle bytes"[..]),
                (&member, &folder, &b"manifest"[..]),
            ] {
                let sink = r10_sink(dest);
                let (w, flag) = r10_worker(dest);
                w.try_request(IoRequest::Load {
                    id: 1,
                    path: bundle.clone(),
                })
                .unwrap();
                drop(w);
                let mut failure = None;
                assert_eq!(
                    finish_trace(sink, &flag, Some(bundle), b"csv\n", &mut failure),
                    None
                );
                assert_eq!(std::fs::read(dest).unwrap(), bytes);
                assert!(failure.is_some());
            }
        }
        assert_eq!(names(&dir), ["example", "t.zip"]);
        assert_eq!(names(&folder), ["table.toml"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Gate 163 round 10 #1/#2 (REQ-LAT-003, review thread 4703749453
    /// comment 4704457456, comment 4704516459): a separate trace path
    /// beside the bundles still publishes in the same selection flow (the
    /// selection is not refused for the trace, the flag stays clear, the
    /// failure is unchanged), also with that bundle unanswered at shutdown.
    #[test]
    fn gate163_r10_separate_trace_path_still_publishes() {
        use super::finish_trace;
        use pin_app::orchestrator::{IoReply, IoRequest};
        use std::sync::atomic::Ordering;
        let dir = fresh_dir("r10_separate");
        let zip = dir.join("t.zip");
        std::fs::write(&zip, b"PK bundle bytes").unwrap();
        let folder = dir.join("example");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("table.toml"), b"manifest").unwrap();
        let dest = dir.join("other.csv");
        let sink = r10_sink(&dest);
        let (w, flag) = r10_worker(&dest);
        for (id, path) in [(1, zip.clone()), (2, folder.clone())] {
            w.try_request(IoRequest::Load { id, path }).unwrap();
            match w.recv() {
                Some(IoReply::LoadFailed { error, .. }) => {
                    assert!(!error.contains("--latency-trace"), "{error}");
                }
                Some(IoReply::Loaded { .. }) => {}
                other => panic!("{other:?}"),
            }
        }
        drop(w);
        assert!(!flag.load(Ordering::Acquire));
        let mut failure = Some("earlier".to_owned());
        assert_eq!(
            finish_trace(sink, &flag, Some(&folder), b"csv,trace\n", &mut failure),
            Some(dest.clone())
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"csv,trace\n");
        assert_eq!(failure.as_deref(), Some("earlier"));
        assert_eq!(std::fs::read(&zip).unwrap(), b"PK bundle bytes");
        assert_eq!(names(&dir), ["example", "other.csv", "t.zip"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Gate 163 round 10 #2 (REQ-LAT-003, review comment 4704516459):
    /// `--table <folder> --latency-trace <folder>/table.toml` (also deeper
    /// and through `..`) is refused at start-up before anything is
    /// created; a sibling destination outside the folder still opens.
    #[test]
    fn gate163_r10_table_folder_member_refused_at_startup() {
        let dir = fresh_dir("r10_startup");
        let folder = dir.join("example");
        std::fs::create_dir_all(folder.join("sub")).unwrap();
        let member = folder.join("table.toml");
        std::fs::write(&member, b"manifest").unwrap();
        let deep = folder.join("sub").join("x.csv");
        std::fs::write(&deep, b"deep").unwrap();
        let dotted = folder.join("sub").join("..").join("table.toml");
        let t = folder.to_str().unwrap();
        for dest in [&member, &deep, &dotted] {
            let e = open_latency_trace(&args(&[
                "--table",
                t,
                "--latency-trace",
                dest.to_str().unwrap(),
            ]))
            .expect_err("refused");
            assert_eq!(
                e,
                format!(
                    "--latency-trace: {} is in the --table bundle",
                    dest.display()
                )
            );
        }
        assert_eq!(names(&folder), ["sub", "table.toml"]);
        assert_eq!(names(&folder.join("sub")), ["x.csv"]);
        assert_eq!(std::fs::read(&member).unwrap(), b"manifest");
        // A sibling outside the folder (also a common-prefix folder).
        let prefix = dir.join("example2");
        std::fs::create_dir_all(&prefix).unwrap();
        for ok in [dir.join("other.csv"), prefix.join("x.csv")] {
            let sink = open_latency_trace(&args(&[
                "--table",
                t,
                "--latency-trace",
                ok.to_str().unwrap(),
            ]))
            .unwrap();
            assert!(sink.is_some(), "{}", ok.display());
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Gate 163 round 10 #2 (REQ-LAT-003, review comment 4704516459): the
    /// alias predicate [`super::bundle_alias`]: the same file and a hard
    /// link are the `.zip` bundle; a folder member at depth 1 and 2 and a
    /// `..` spelling are inside the folder bundle; a common-prefix sibling
    /// folder, a missing destination and a missing bundle are not; a
    /// symlinked folder and a destination symlink into or out of the folder
    /// are protected.
    #[test]
    fn gate163_r10_bundle_alias_predicate() {
        use super::{BundleAlias, bundle_alias, trace_alias};
        let dir = fresh_dir("r10_pred");
        let zip = dir.join("t.zip");
        std::fs::write(&zip, b"PK").unwrap();
        let folder = dir.join("example");
        std::fs::create_dir_all(folder.join("a").join("b")).unwrap();
        let d1 = folder.join("table.toml");
        let d2 = folder.join("a").join("x.csv");
        std::fs::write(&d1, b"m").unwrap();
        std::fs::write(&d2, b"x").unwrap();
        let prefix = dir.join("example2");
        std::fs::create_dir_all(&prefix).unwrap();
        let sib = prefix.join("x.csv");
        std::fs::write(&sib, b"s").unwrap();
        let alias = |d: &Path, b: &Path| bundle_alias(&std::fs::metadata(d).unwrap(), d, b);
        assert_eq!(alias(&zip, &zip), Some(BundleAlias::File));
        assert_eq!(alias(&d1, &folder), Some(BundleAlias::Member));
        assert_eq!(alias(&d2, &folder), Some(BundleAlias::Member));
        assert_eq!(alias(&d2, &folder.join("a")), Some(BundleAlias::Member));
        let dotted = folder.join("a").join("b").join("..").join("x.csv");
        assert_eq!(alias(&dotted, &folder), Some(BundleAlias::Member));
        let dotted_bundle = folder.join("a").join("..");
        assert_eq!(alias(&d1, &dotted_bundle), Some(BundleAlias::Member));
        assert_eq!(alias(&sib, &folder), None, "common prefix is not inside");
        assert_eq!(alias(&d1, &prefix), None);
        assert_eq!(alias(&zip, &folder), None);
        assert_eq!(alias(&d1, &zip), None);
        assert_eq!(alias(&d1, &dir.join("missing")), None);
        // A missing destination is no collision.
        assert_eq!(trace_alias(&folder.join("new.csv"), &folder), None);
        assert_eq!(trace_alias(&dir.join("new.zip"), &zip), None);
        #[cfg(unix)]
        {
            let hard = dir.join("hard.zip");
            std::fs::hard_link(&zip, &hard).unwrap();
            assert_eq!(alias(&hard, &zip), Some(BundleAlias::File));
            // A symlinked bundle folder resolves to the same folder.
            let link = dir.join("link");
            std::os::unix::fs::symlink(&folder, &link).unwrap();
            assert_eq!(alias(&d1, &link), Some(BundleAlias::Member));
            assert_eq!(
                alias(&link.join("table.toml"), &folder),
                Some(BundleAlias::Member)
            );
            // A destination symlink outside pointing in, and one inside
            // pointing out: both protected (the target or the entry the
            // rename replaces lies inside).
            let out_in = dir.join("out_in.toml");
            std::os::unix::fs::symlink(&d1, &out_in).unwrap();
            assert_eq!(alias(&out_in, &folder), Some(BundleAlias::Member));
            let in_out = folder.join("in_out.csv");
            std::os::unix::fs::symlink(&sib, &in_out).unwrap();
            assert_eq!(alias(&in_out, &folder), Some(BundleAlias::Member));
            // A hard link outside the folder is its own entry: the rename
            // replaces only it, the member keeps its bytes.
            let outside = dir.join("hard.toml");
            std::fs::hard_link(&d1, &outside).unwrap();
            assert_eq!(alias(&outside, &folder), None);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Gate 163 round 10 (REQ-LAT-003, review thread 4703749453 comment
    /// 4704457456): the menu's selection still waiting for an answer at
    /// shutdown is reported to the shutdown step; none when idle.
    #[test]
    fn gate163_r10_unanswered_selection() {
        use super::unanswered_selection;
        use pin_app::ui::{MenuController, MenuInput, TableEntry, TableInfo, TableStatus};
        let dir = fresh_dir("r10_pending");
        let (mut c, _) = MenuController::load(dir.join("config.toml"));
        assert_eq!(unanswered_selection(&c), None);
        let info = TableInfo {
            id: "t".into(),
            name: "T".into(),
            author: "a".into(),
            version: "1".into(),
            players_max: 1,
            features: Vec::new(),
            preview: None,
            preview_error: None,
            warning_count: 0,
        };
        let bundle = dir.join("t.zip");
        c.set_tables(vec![TableEntry {
            path: bundle.clone(),
            file_name: "t.zip".into(),
            status: TableStatus::Ready(info),
        }]);
        c.set_browser_enabled(true);
        c.toggle(1, &mut |_| {});
        c.apply(MenuInput::LoadTable(0), 2, &mut |_| {});
        let (id, _) = c.take_load_request().expect("a load request");
        assert_eq!(unanswered_selection(&c), Some(bundle.as_path()));
        c.finish_load(id, Err("refused".into()));
        assert_eq!(unanswered_selection(&c), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Gate 163 round 9 #1 (review thread 4703749444, REQ-VIEW-006): a
    /// playfield close pushed into the window ring before a table swap
    /// stops the engine makes `stop_for_swap` report it (the swap then
    /// starts nothing and the run quits), whether or not the render thread
    /// had popped it; a close the policy ignores (a secondary window) and
    /// no close let the swap go on.
    #[test]
    fn gate163_r9_close_before_swap_ends_the_run() {
        use super::stop_for_swap;
        use pin_app::orchestrator::{RenderConfig, ThreadsConfig, spawn_threads};
        use pin_render::topology::{WindowEvent, WindowEventKind};
        let run = |events: &[WindowEvent]| {
            let (_input_tx, input_rx) = pin_core::spsc::channel(8);
            let (mut window_tx, window_rx) = pin_core::spsc::channel(8);
            let mut cfg = ThreadsConfig::idle(input_rx);
            cfg.render = RenderConfig {
                window_events: Some(window_rx),
                ..RenderConfig::default()
            };
            let threads = spawn_threads(cfg).unwrap();
            for ev in events {
                window_tx.push(*ev).unwrap();
            }
            stop_for_swap(threads, &mut None).unwrap()
        };
        let close = |window| WindowEvent {
            timestamp_ns: 1,
            window,
            kind: WindowEventKind::CloseRequested,
        };
        let report = run(&[close(0)]).expect("the close cancels the swap");
        assert!(report.0.close_requested);
        assert!(run(&[close(1)]).is_none(), "a backglass close is ignored");
        assert!(run(&[]).is_none());
    }

    /// One engine's trace collector with `frames` presented frames (snapshot
    /// tick 0, present times from `base_ns`).
    fn engine_trace(frames: u64, base_ns: u64) -> pin_app::orchestrator::trace::TraceCollector {
        use pin_app::orchestrator::trace::{FlipperBindings, latency_trace};
        fn clock() -> u64 {
            0
        }
        let (collector, _sim, mut render) = latency_trace(FlipperBindings::new(), clock);
        for i in 0..frames {
            render.presented_at(true, &pin_core::FrameStateSnapshot::default(), base_ns + i);
        }
        collector
    }

    /// Gate 163 round 9 #3 (review thread 4703749460, REQ-LAT-003): the
    /// trace of every engine stopped by a swap is banked and merged with
    /// the last engine's at shutdown: no frame is lost, frame numbers stay
    /// unique and ascending, the file round-trips, and `run_end_ns` is the
    /// run's.
    #[test]
    fn gate163_r9_trace_keeps_every_engine() {
        use super::{bank_trace, final_trace};
        use pin_app::orchestrator::trace::CollectedTrace;
        let mut banked = None;
        bank_trace(&mut banked, None, 5);
        assert!(banked.is_none(), "no collector, nothing banked");
        bank_trace(&mut banked, Some(engine_trace(2, 100)), 150);
        bank_trace(&mut banked, Some(engine_trace(3, 200)), 250);
        let all = final_trace(banked, Some(engine_trace(4, 300)), 999).unwrap();
        let frames: Vec<u64> = all.frames.iter().map(|f| f.frame).collect();
        assert_eq!(frames, (0..9).collect::<Vec<_>>());
        let present: Vec<u64> = all.frames.iter().map(|f| f.present_ns).collect();
        assert_eq!(present, [100, 101, 200, 201, 202, 300, 301, 302, 303]);
        assert_eq!(all.run_end_ns, 999);
        let parsed = CollectedTrace::parse(&all.to_csv()).unwrap();
        assert_eq!(parsed.frames.len(), 9);
        assert_eq!(parsed.run_end_ns, 999);
        // Without swaps the file is the single engine's collection.
        let single = final_trace(None, Some(engine_trace(2, 7)), 50).unwrap();
        assert_eq!(single, engine_trace(2, 7).collect(50));
        // Nothing after the last swap (never the case in the app): the
        // banked engines are still written, ending at the run's end.
        let mut banked = None;
        bank_trace(&mut banked, Some(engine_trace(1, 1)), 2);
        let only = final_trace(banked, None, 77).unwrap();
        assert_eq!((only.frames.len(), only.run_end_ns), (1, 77));
        assert!(final_trace(None, None, 1).is_none());
    }

    /// Gate 163 round 9 #6 (review thread 4703749471, REQ-VIEW-006): a load
    /// the I/O worker cannot take (it never started) is reported to the
    /// menu at once, so the menu leaves `Loading` and accepts another load.
    #[test]
    fn gate163_r9_failed_load_submission_leaves_loading() {
        use super::submit_load;
        use pin_app::orchestrator::{IO_WORKER_NOT_RUNNING, IoWorker};
        use pin_app::ui::{
            MenuController, MenuInput, TableEntry, TableInfo, TableLoadState, TableStatus,
        };
        let dir = fresh_dir("r9_submit");
        let (mut c, _) = MenuController::load(dir.join("config.toml"));
        let info = TableInfo {
            id: "t".into(),
            name: "T".into(),
            author: "a".into(),
            version: "1".into(),
            players_max: 1,
            features: Vec::new(),
            preview: None,
            preview_error: None,
            warning_count: 0,
        };
        c.set_tables(vec![TableEntry {
            path: dir.join("t.zip"),
            file_name: "t.zip".into(),
            status: TableStatus::Ready(info),
        }]);
        c.set_browser_enabled(true);
        c.toggle(1, &mut |_| {});
        c.apply(MenuInput::LoadTable(0), 2, &mut |_| {});
        let (id, path) = c.take_load_request().expect("a load request");
        assert!(matches!(c.menu().load_state(), TableLoadState::Loading(_)));
        submit_load(&IoWorker::not_running(), &mut c, id, path.clone());
        match c.menu().load_state() {
            TableLoadState::Failed { path: p, error } => {
                assert_eq!(p, &path);
                assert!(error.contains(IO_WORKER_NOT_RUNNING), "{error}");
            }
            other => panic!("expected Failed, got {other:?}"),
        }
        assert_eq!(c.menu().pending_load_id(), None);
        c.apply(MenuInput::LoadTable(0), 3, &mut |_| {});
        assert!(c.take_load_request().is_some(), "another load is accepted");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Whether the POSIX ACL fixture of
    /// `req_lat_003_default_acl_dir_never_opens_the_trace_to_named_users`
    /// must be set up (gate 163 round 11, review 6070807835, R10): in CI
    /// (`GITHUB_ACTIONS=true`) or when `KP_REQUIRE_POSIX_ACL=1`.
    #[cfg(target_os = "linux")]
    fn acl_required(
        github_actions: Option<&std::ffi::OsStr>,
        require: Option<&std::ffi::OsStr>,
    ) -> bool {
        github_actions.is_some_and(|v| v == "true") || require.is_some_and(|v| v == "1")
    }

    /// REQ-LAT-003 (gate 163 round 11, review 6070807835, R10): an ACL
    /// fixture failure may only be reported as an unsupported environment
    /// locally; in CI or with `KP_REQUIRE_POSIX_ACL=1` it fails the test.
    #[test]
    #[cfg(target_os = "linux")]
    fn gate163_r11_acl_fixture_is_required_in_ci() {
        use std::ffi::OsStr;
        let t = Some(OsStr::new("true"));
        let one = Some(OsStr::new("1"));
        assert!(acl_required(t, None));
        assert!(acl_required(None, one));
        assert!(acl_required(t, one));
        assert!(!acl_required(None, None));
        assert!(!acl_required(
            Some(OsStr::new("false")),
            Some(OsStr::new("0"))
        ));
    }

    /// REQ-LAT-003 / REQ-VIEW-006 (gate 163 round 11, review comment
    /// 4705691334): a `--latency-trace` destination that is the `--config`
    /// file is refused before anything is created, whether the file exists
    /// (the same path, a symlink to it, a hard link) or not yet (relative
    /// vs absolute spelling, `./x` vs `x`, `d/../x`), and so is a
    /// configuration path equal to the trace's sibling temp-file name; the
    /// configuration file keeps its bytes. A separate destination still
    /// opens.
    #[test]
    fn req_lat_003_trace_destination_equal_to_the_config_is_refused() {
        let dir = fresh_dir("r11_config");
        let cfg = dir.join("session.toml");
        let c = cfg.to_str().unwrap().to_owned();
        let refused = |trace: &str, config: &str| {
            let e = open_latency_trace(&args(&["--config", config, "--latency-trace", trace]))
                .expect_err("refused");
            assert_eq!(
                e,
                format!("--latency-trace: {trace} is the configuration file")
            );
        };
        // Not yet existing (protects the first save).
        refused(&c, &c);
        let dot = format!("{}/./sub/../session.toml", dir.display());
        refused(&dot, &c);
        assert!(names(&dir).is_empty(), "nothing created: {:?}", names(&dir));
        // Relative vs absolute and `./x` vs `x`, resolved against the cwd.
        let rel = format!("kp-r11-cfg-{}.toml", std::process::id());
        let abs = std::env::current_dir().unwrap().join(&rel);
        refused(&rel, abs.to_str().unwrap());
        refused(&format!("./{rel}"), &rel);
        refused(abs.to_str().unwrap(), &format!("./{rel}"));
        assert!(!abs.exists(), "nothing created");
        // Existing configuration file.
        std::fs::write(&cfg, "wireframe = true\n").unwrap();
        refused(&c, &c);
        refused(&dot, &c);
        #[cfg(unix)]
        {
            let link = dir.join("link.csv");
            std::os::unix::fs::symlink(&cfg, &link).unwrap();
            refused(link.to_str().unwrap(), &c);
            let hard = dir.join("hard.csv");
            std::fs::hard_link(&cfg, &hard).unwrap();
            refused(hard.to_str().unwrap(), &c);
            // A symlinked parent directory naming a file not created yet.
            let alias = dir.join("alias");
            std::os::unix::fs::symlink(&dir, &alias).unwrap();
            let new_cfg = dir.join("new.toml");
            refused(
                alias.join("new.toml").to_str().unwrap(),
                new_cfg.to_str().unwrap(),
            );
            assert!(!new_cfg.exists(), "nothing created");
        }
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "wireframe = true\n");
        // The configuration path is the trace's temp-file name.
        let trace = dir.join("t.csv");
        let tmp = dir.join(super::trace_tmp_name(std::process::id(), 0));
        refused(trace.to_str().unwrap(), tmp.to_str().unwrap());
        assert!(!tmp.exists() && !trace.exists(), "nothing created");
        // A separate destination still opens and publishes.
        let sink = open_latency_trace(&args(&[
            "--config",
            &c,
            "--latency-trace",
            trace.to_str().unwrap(),
        ]))
        .unwrap()
        .unwrap();
        sink.commit(b"csv").unwrap();
        assert_eq!(std::fs::read_to_string(&trace).unwrap(), "csv");
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "wireframe = true\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-LAT-003 / REQ-VIEW-006 (gate 163 round 11, review comment
    /// 4705691334): without `--config` the configuration path compared is
    /// the default one (`pin_app::ui::default_config_path`); a trace
    /// destination naming it is refused before anything is created.
    #[test]
    fn req_lat_003_trace_destination_equal_to_the_default_config_is_refused() {
        let (default, _) = pin_app::ui::default_config_path();
        assert_eq!(super::config_path(&args(&[])).unwrap(), default);
        let existed = default.exists();
        let d = default.to_str().unwrap();
        let e = open_latency_trace(&args(&["--latency-trace", d])).unwrap_err();
        assert_eq!(e, format!("--latency-trace: {d} is the configuration file"));
        assert_eq!(default.exists(), existed, "nothing created");
        // An explicit `--config` replaces the default.
        let dir = fresh_dir("r11_default");
        let trace = dir.join("t.csv");
        let other = dir.join("other.toml");
        assert_eq!(
            super::config_path(&args(&["--config", other.to_str().unwrap()])).unwrap(),
            other
        );
        let sink = open_latency_trace(&args(&["--latency-trace", trace.to_str().unwrap()]))
            .unwrap()
            .unwrap();
        drop(sink);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-LAT-003 (gate 163 round 11, review 6070807835): the direct
    /// (device) open is nonblocking and checks the opened descriptor: a
    /// FIFO with no reader fails at once (it never blocks), a FIFO with a
    /// reader is refused as a FIFO, and `/dev/null` is opened with
    /// `O_NONBLOCK` cleared again and written.
    #[test]
    #[cfg(unix)]
    fn req_lat_003_direct_open_never_blocks_on_a_fifo() {
        use super::open_direct;
        use std::io::Write as _;
        use std::os::fd::AsRawFd as _;
        use std::os::unix::fs::OpenOptionsExt as _;
        let dir = fresh_dir("r11_fifo");
        let fifo = dir.join("trace.fifo");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        if !matches!(&made, Ok(s) if s.success()) {
            let _ = std::fs::remove_dir_all(&dir);
            panic!("mkfifo is required for the FIFO check and failed: {made:?}");
        }
        let f = fifo.to_str().unwrap();
        // No reader: a blocking write-only open would hang here.
        let e = open_direct(&fifo).unwrap_err();
        assert!(e.starts_with(&format!("--latency-trace: {f}: ")), "{e}");
        // A reader present: the open succeeds, the descriptor is a FIFO.
        let reader = std::fs::OpenOptions::new()
            .read(true)
            .custom_flags(sdl3::libc::O_NONBLOCK)
            .open(&fifo)
            .unwrap();
        assert_eq!(
            open_direct(&fifo).unwrap_err(),
            format!("--latency-trace: {f}: is a FIFO")
        );
        drop(reader);
        let mut null = open_direct(Path::new("/dev/null")).unwrap();
        // SAFETY: `null`'s descriptor is open for the call.
        let flags = unsafe { sdl3::libc::fcntl(null.as_raw_fd(), sdl3::libc::F_GETFL) };
        assert!(flags >= 0);
        assert_eq!(flags & sdl3::libc::O_NONBLOCK, 0, "O_NONBLOCK cleared");
        null.write_all(b"csv").unwrap();
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Gate 163 round 11 (review 6070807835, REQ-LAT-003): the run's
    /// one-shot `first frame presented` line counts the running engine's
    /// presented frames and those of engines already banked by a swap
    /// (also a frame lost to a full ring), so a frame presented just before
    /// a swap still raises it.
    #[test]
    fn gate163_r11_banked_frames_raise_the_first_frame_marker() {
        use super::{any_frame_presented, bank_trace};
        assert!(!any_frame_presented(None, None));
        let idle = engine_trace(0, 0);
        assert!(!any_frame_presented(Some(&idle), None));
        assert!(any_frame_presented(Some(&engine_trace(1, 5)), None));
        // The old engine presented, then a swap banked it.
        let mut banked = None;
        bank_trace(&mut banked, Some(engine_trace(1, 5)), 10);
        assert!(any_frame_presented(Some(&idle), banked.as_ref()));
        assert!(any_frame_presented(None, banked.as_ref()));
        // A banked engine with no frame does not raise it.
        let mut empty = None;
        bank_trace(&mut empty, Some(engine_trace(0, 0)), 10);
        assert!(!any_frame_presented(Some(&idle), empty.as_ref()));
        // A frame lost to a full ring still counts.
        let mut lost = empty.unwrap();
        lost.overflows.frames = 1;
        assert!(any_frame_presented(Some(&idle), Some(&lost)));
    }

    /// REQ-LAT-003 / REQ-VIEW-006 (gate 163 round 12, review comment
    /// 4710120043, reopened thread 4705691334): with `links/current` a
    /// symlink to `profiles/player` and neither file created yet,
    /// `--latency-trace links/current/../session.toml` names
    /// `profiles/session.toml` (the filesystem resolves the symlink before
    /// `..`), so with `--config profiles/session.toml` it is refused before
    /// anything is created (the first save is protected); a distinct
    /// `links/current/../trace.csv` still opens and publishes.
    #[test]
    #[cfg(unix)]
    fn req_lat_003_first_save_symlink_then_dotdot_is_refused() {
        let dir = fresh_dir("r12_symlink_dotdot");
        let profiles = dir.join("profiles");
        std::fs::create_dir_all(profiles.join("player")).unwrap();
        let links = dir.join("links");
        std::fs::create_dir_all(&links).unwrap();
        std::os::unix::fs::symlink("../profiles/player", links.join("current")).unwrap();
        let cfg = profiles.join("session.toml");
        let c = cfg.to_str().unwrap();
        let trace = links.join("current/../session.toml");
        let t = trace.to_str().unwrap();
        assert!(!cfg.exists() && !trace.exists(), "both missing");
        // The startup alias check itself.
        assert_eq!(
            super::config_alias(&trace, None, &cfg),
            Some(format!("--latency-trace: {t} is the configuration file"))
        );
        let e =
            open_latency_trace(&args(&["--config", c, "--latency-trace", t])).expect_err("refused");
        assert_eq!(e, format!("--latency-trace: {t} is the configuration file"));
        assert_eq!(names(&profiles), ["player"], "nothing created");
        assert_eq!(names(&links), ["current"], "nothing created");
        // A distinct destination through the same symlink still opens.
        let other = links.join("current/../trace.csv");
        assert_eq!(super::config_alias(&other, None, &cfg), None);
        let sink = open_latency_trace(&args(&[
            "--config",
            c,
            "--latency-trace",
            other.to_str().unwrap(),
        ]))
        .unwrap()
        .unwrap();
        sink.commit(b"csv").unwrap();
        assert_eq!(
            std::fs::read_to_string(profiles.join("trace.csv")).unwrap(),
            "csv"
        );
        assert!(!cfg.exists());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Gate 163 round 12 (review comment 4710120043): the resolved form of
    /// a path that does not exist yet. A symlink followed by `..` resolves
    /// to the target's parent, `.` and `..` in the missing tail fold
    /// lexically and never above the root, and the final file name is not
    /// resolved (a symlink there stays itself).
    #[test]
    #[cfg(unix)]
    fn req_lat_003_normalised_path_resolves_existing_ancestors_first() {
        use super::normalised_path;
        let dir = fresh_dir("r12_normalise");
        let root = std::fs::canonicalize(&dir).unwrap();
        std::fs::create_dir_all(dir.join("a/b")).unwrap();
        std::os::unix::fs::symlink("a/b", dir.join("l")).unwrap();
        assert_eq!(
            normalised_path(&dir.join("l/../x.toml")),
            root.join("a/x.toml")
        );
        assert_eq!(
            normalised_path(&dir.join("l/new/./deeper/../../x.toml")),
            root.join("a/b/x.toml")
        );
        assert_eq!(normalised_path(&dir.join("l")), root.join("l"));
        assert_eq!(
            normalised_path(Path::new("/../../kp-missing-r12/../y")),
            PathBuf::from("/y")
        );
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-LAT-003 / REQ-VIEW-006 (gate 163 round 12, review comment
    /// 4710120054): two spellings that are distinct at start-up (both
    /// missing; `Session.toml` vs `session.toml`) are accepted, but when
    /// the configuration file the menu saved during the run is the same
    /// file as the destination at shutdown (here a hard link, standing in
    /// for one directory entry on a case-insensitive volume, where it
    /// already is one), the shutdown step does not publish the trace: the
    /// configuration keeps its bytes, the temp file is removed, and the
    /// run fails with `... is the configuration file: the trace was not
    /// published`. A distinct destination still publishes.
    #[test]
    fn req_lat_003_config_saved_during_run_is_never_replaced_by_the_trace() {
        use super::finish_trace;
        use std::sync::atomic::AtomicBool;
        let dir = fresh_dir("r12_case");
        let cfg = dir.join("Session.toml");
        let dest = dir.join("session.toml");
        let c = cfg.to_str().unwrap();
        let d = dest.to_str().unwrap();
        let open = |trace: &str| {
            open_latency_trace(&args(&["--config", c, "--latency-trace", trace]))
                .unwrap()
                .unwrap()
        };
        let sink = open(d);
        // The menu's first save during the run.
        std::fs::write(&cfg, "wireframe = true\n").unwrap();
        // On a case-insensitive volume `dest` is already this entry.
        let case_insensitive = dest.exists();
        if !case_insensitive {
            std::fs::hard_link(&cfg, &dest).unwrap();
        }
        let flag = AtomicBool::new(false);
        let mut failure = None;
        assert_eq!(
            finish_trace(sink, &flag, None, b"csv,trace\n", &mut failure),
            None
        );
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "wireframe = true\n");
        assert_eq!(
            std::fs::read_to_string(&dest).unwrap(),
            "wireframe = true\n"
        );
        let expected =
            format!("--latency-trace: {d} is the configuration file: the trace was not published");
        assert_eq!(failure.as_deref(), Some(expected.as_str()));
        let mut want = vec!["Session.toml".to_owned()];
        if !case_insensitive {
            want.push("session.toml".to_owned());
        }
        assert_eq!(names(&dir), want, "no temp file left");
        // A distinct destination publishes, the configuration untouched.
        let other = dir.join("trace.csv");
        let sink = open(other.to_str().unwrap());
        let mut failure = None;
        assert_eq!(
            finish_trace(sink, &flag, None, b"csv,trace\n", &mut failure),
            Some(other.clone())
        );
        assert_eq!(failure, None);
        assert_eq!(std::fs::read_to_string(&other).unwrap(), "csv,trace\n");
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "wireframe = true\n");
        // The recheck itself: missing paths and distinct files pass.
        assert_eq!(
            super::config_identity_alias(&dir.join("missing.csv"), &cfg),
            None
        );
        assert_eq!(
            super::config_identity_alias(&other, &dir.join("missing.toml")),
            None
        );
        assert_eq!(super::config_identity_alias(&other, &cfg), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Gate 163 round 12 (review 6076344934 finding 10, REQ-LAT-003): the
    /// one-shot `first frame presented` line is still due at shutdown when
    /// the run loop did not announce it and the final trace holds a frame
    /// row or a frame lost to a full ring; never twice, never without a
    /// frame.
    #[test]
    fn req_lat_003_first_frame_marker_on_the_shutdown_path() {
        use super::{final_trace, first_frame_at_shutdown};
        let framed = final_trace(None, Some(engine_trace(1, 5)), 10);
        assert!(first_frame_at_shutdown(false, framed.as_ref()));
        assert!(!first_frame_at_shutdown(true, framed.as_ref()), "once");
        let mut empty = final_trace(None, Some(engine_trace(0, 0)), 10).unwrap();
        assert!(empty.frames.is_empty());
        assert!(!first_frame_at_shutdown(false, Some(&empty)));
        assert!(!first_frame_at_shutdown(false, None));
        empty.overflows.frames = 1;
        assert!(first_frame_at_shutdown(false, Some(&empty)));
        assert!(!first_frame_at_shutdown(true, Some(&empty)));
    }

    /// Gate 163 round 14 (review comment 4713880926, REQ-LAT-003,
    /// REQ-VIEW-006): a configuration file named so that its save scratch
    /// (`.<name>.tmp`, [`pin_app::ui::AppConfig::save_scratch_path`]) is a
    /// trace temp-file candidate (`kp-trace.<pid>.0` gives
    /// `.kp-trace.<pid>.0.tmp`) never shares that file with the trace: the
    /// trace takes the next candidate, so a menu save during the run writes
    /// the configuration and the trace still publishes to its destination.
    #[test]
    fn gate163_r14_config_save_scratch_is_never_the_trace_temp_file() {
        use pin_app::ui::AppConfig;
        let dir = fresh_dir("r14_scratch");
        let pid = std::process::id();
        let cfg = dir.join(format!("kp-trace.{pid}.0"));
        let scratch = AppConfig::save_scratch_path(&cfg).unwrap();
        assert_eq!(scratch, dir.join(super::trace_tmp_name(pid, 0)));
        let dest = dir.join("out.csv");
        let sink = open_latency_trace(&args(&[
            "--config",
            cfg.to_str().unwrap(),
            "--latency-trace",
            dest.to_str().unwrap(),
        ]))
        .unwrap()
        .unwrap();
        assert_ne!(sink.tmp.as_deref(), Some(scratch.as_path()));
        // The menu saves its settings during the run.
        let saved = AppConfig {
            wireframe: true,
            ..AppConfig::default()
        };
        saved.save(&cfg).unwrap();
        let text = saved.to_toml_string().unwrap();
        sink.commit(b"csv,trace\n").unwrap();
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), text);
        assert_eq!(std::fs::read(&dest).unwrap(), b"csv,trace\n");
        let want = {
            let mut v = vec![format!("kp-trace.{pid}.0"), "out.csv".to_owned()];
            v.sort();
            v
        };
        assert_eq!(names(&dir), want, "no scratch or temp file left");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Gate 163 round 14 (review comment 4713880930, REQ-LAT-003,
    /// REQ-VIEW-006): the recovery copy a malformed configuration's load
    /// makes (`<name>.bak`, [`pin_app::ui::AppConfig::recovery_path`]) is
    /// protected like the configuration file: `--latency-trace
    /// <config>.bak` is refused at start-up (existing or not yet), a
    /// destination that is the recovery copy at shutdown is not published
    /// over it, and a separate destination still publishes.
    #[test]
    fn gate163_r14_config_recovery_copy_is_never_replaced_by_the_trace() {
        use super::finish_trace;
        use pin_app::ui::AppConfig;
        use std::sync::atomic::AtomicBool;
        let dir = fresh_dir("r14_bak");
        let cfg = dir.join("session.toml");
        let bak = AppConfig::recovery_path(&cfg).unwrap();
        assert_eq!(bak, dir.join("session.toml.bak"));
        let c = cfg.to_str().unwrap();
        let b = bak.to_str().unwrap();
        let refused = |trace: &str| {
            let e = open_latency_trace(&args(&["--config", c, "--latency-trace", trace]))
                .expect_err("refused");
            assert_eq!(
                e,
                format!("--latency-trace: {trace} is the configuration recovery copy")
            );
        };
        // Not yet existing: the load may still make it.
        refused(b);
        assert!(names(&dir).is_empty(), "nothing created: {:?}", names(&dir));
        // A malformed configuration's load makes the recovery copy.
        std::fs::write(&cfg, "wireframe = [\n").unwrap();
        drop(AppConfig::load(&cfg));
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "wireframe = [\n");
        refused(b);
        let dot = format!("{}/./sub/../session.toml.bak", dir.display());
        refused(&dot);
        // A destination that becomes the recovery copy during the run
        // (a hard link, standing in for another spelling of the entry).
        let dest = dir.join("late.csv");
        let d = dest.to_str().unwrap();
        let open = |trace: &str| {
            open_latency_trace(&args(&["--config", c, "--latency-trace", trace]))
                .unwrap()
                .unwrap()
        };
        let sink = open(d);
        std::fs::hard_link(&bak, &dest).unwrap();
        let flag = AtomicBool::new(false);
        let mut failure = None;
        assert_eq!(
            finish_trace(sink, &flag, None, b"csv,trace\n", &mut failure),
            None
        );
        assert_eq!(
            failure.as_deref(),
            Some(
                format!(
                    "--latency-trace: {d} is the configuration recovery copy: the trace was not \
                     published"
                )
                .as_str()
            )
        );
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "wireframe = [\n");
        assert_eq!(
            names(&dir),
            ["late.csv", "session.toml", "session.toml.bak"]
        );
        // A separate destination publishes; both files keep their bytes.
        let other = dir.join("trace.csv");
        let sink = open(other.to_str().unwrap());
        let mut failure = None;
        assert_eq!(
            finish_trace(sink, &flag, None, b"csv,trace\n", &mut failure),
            Some(other.clone())
        );
        assert_eq!(failure, None);
        assert_eq!(std::fs::read(&other).unwrap(), b"csv,trace\n");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "wireframe = [\n");
        assert_eq!(std::fs::read_to_string(&cfg).unwrap(), "wireframe = [\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Gate 163 round 14 (review comment 4713880925, REQ-LAT-003): a
    /// destination that did not exist at start-up but is created inside
    /// the loaded `--table` folder bundle during the run is not published
    /// over at shutdown (the load guard's bundle-membership rule,
    /// [`super::trace_alias`], applied to the loaded bundle): the file keeps
    /// its bytes, no temp file is left, the run fails with `... is in the
    /// selected table bundle: the trace was not published`. A destination
    /// still missing at shutdown, or outside the bundle, still publishes.
    #[test]
    fn gate163_r14_destination_created_in_the_loaded_table_bundle() {
        use super::finish_trace;
        use std::sync::atomic::AtomicBool;
        let dir = fresh_dir("r14_loaded");
        let cfg = dir.join("config.toml");
        let folder = dir.join("example");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("table.toml"), b"manifest").unwrap();
        let dest = folder.join("notes.txt");
        let open = |d: &Path| {
            open_latency_trace(&args(&[
                "--config",
                cfg.to_str().unwrap(),
                "--table",
                folder.to_str().unwrap(),
                "--latency-trace",
                d.to_str().unwrap(),
            ]))
            .unwrap()
            .unwrap()
        };
        let sink = open(&dest);
        // Created inside the bundle during the run.
        std::fs::write(&dest, b"user notes").unwrap();
        let flag = AtomicBool::new(false);
        let mut failure = None;
        assert_eq!(
            finish_trace(sink, &flag, None, b"csv,trace\n", &mut failure),
            None
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"user notes");
        assert_eq!(names(&folder), ["notes.txt", "table.toml"]);
        assert_eq!(
            failure.as_deref(),
            Some(
                format!(
                    "--latency-trace: {} is in the selected table bundle: the trace was not \
                     published",
                    dest.display()
                )
                .as_str()
            )
        );
        // A destination outside the bundle publishes.
        let other = dir.join("trace.csv");
        let sink = open(&other);
        let mut failure = None;
        assert_eq!(
            finish_trace(sink, &flag, None, b"csv,trace\n", &mut failure),
            Some(other.clone())
        );
        assert_eq!(failure, None);
        assert_eq!(std::fs::read(&other).unwrap(), b"csv,trace\n");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// Gate 163 round 14 (review comment 4713880925, REQ-LAT-003): after a
    /// table browser swap the newly loaded bundle
    /// ([`super::TraceSink::set_loaded`]) is the one rechecked at shutdown:
    /// a destination created inside it during the run is not published
    /// over; the unanswered-selection check still applies alongside.
    #[test]
    fn gate163_r14_destination_created_in_a_swapped_in_bundle() {
        use super::finish_trace;
        use std::sync::atomic::AtomicBool;
        let dir = fresh_dir("r14_swapped");
        let folder = dir.join("example");
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("table.toml"), b"manifest").unwrap();
        let zip = dir.join("t.zip");
        std::fs::write(&zip, b"PK bundle bytes").unwrap();
        let dest = folder.join("notes.txt");
        let flag = AtomicBool::new(false);
        // Before any load, the folder is no loaded bundle: it publishes.
        let sink = r10_sink(&dest);
        let mut failure = None;
        assert_eq!(
            finish_trace(sink, &flag, None, b"csv\n", &mut failure),
            Some(dest.clone())
        );
        std::fs::remove_file(&dest).unwrap();
        // The browser swapped the folder in; the file appears afterwards.
        let mut sink = r10_sink(&dest);
        sink.set_loaded(folder.clone());
        std::fs::write(&dest, b"user notes").unwrap();
        let mut failure = None;
        assert_eq!(
            finish_trace(sink, &flag, None, b"csv\n", &mut failure),
            None
        );
        assert_eq!(std::fs::read(&dest).unwrap(), b"user notes");
        assert!(failure.unwrap().contains("the trace was not published"));
        // Loaded folder elsewhere, the unanswered selection still refuses.
        let zdest = zip.clone();
        let mut sink = r10_sink(&zdest);
        sink.set_loaded(folder.clone());
        let mut failure = None;
        assert_eq!(
            finish_trace(sink, &flag, Some(&zip), b"csv\n", &mut failure),
            None
        );
        assert_eq!(std::fs::read(&zip).unwrap(), b"PK bundle bytes");
        assert!(failure.is_some());
        assert_eq!(names(&folder), ["notes.txt", "table.toml"]);
        assert_eq!(names(&dir), ["example", "t.zip"]);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
