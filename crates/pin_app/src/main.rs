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

/// The permission bits for a trace temp file that replaces a destination
/// with mode `dest_mode` owned by `dest_uid:dest_gid`, when the temp file
/// is owned by `tmp_uid:tmp_gid`. Only the `0o777` part is ever copied
/// (never setuid, setgid or sticky). The destination's bits are kept as they
/// are only when the temp file has the destination's owner and group. When
/// either differs (a setgid directory, another owner) the group bits are
/// cleared and each `other` bit is kept only if the destination's old group
/// class also had it, so the rename never grants access to a user the old
/// file did not already admit (CWE-732).
#[cfg(any(unix, test))]
fn trace_mode(dest_mode: u32, dest_uid: u32, dest_gid: u32, tmp_uid: u32, tmp_gid: u32) -> u32 {
    let bits = dest_mode & 0o777;
    if dest_uid == tmp_uid && dest_gid == tmp_gid {
        bits
    } else {
        (bits & 0o700) | ((bits >> 3) & bits & 0o007)
    }
}

/// Whether a trace temp file carries an extended POSIX access ACL.
#[cfg(any(unix, test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AclState {
    /// No access ACL (or a filesystem without extended attributes).
    Absent,
    /// An access ACL is present (for example inherited from a directory
    /// default ACL).
    Present,
    /// The ACL state could not be read; treated like [`AclState::Present`].
    Unknown,
}

#[cfg(any(unix, test))]
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
impl AclState {
    /// The state of a pair of files (the trace temp file and the
    /// destination): [`AclState::Unknown`] if either is unknown, else
    /// [`AclState::Present`] if either has an ACL, else
    /// [`AclState::Absent`].
    fn combine(self, other: AclState) -> AclState {
        match (self, other) {
            (AclState::Unknown, _) | (_, AclState::Unknown) => AclState::Unknown,
            (AclState::Present, _) | (_, AclState::Present) => AclState::Present,
            (AclState::Absent, AclState::Absent) => AclState::Absent,
        }
    }
}

/// What the commit does to the trace temp file's mode before the rename.
#[cfg(any(unix, test))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TraceChmod {
    /// Set these permission bits.
    To(u32),
    /// Leave the file as created (`0600`).
    KeepCreated,
}

/// The mode change for a trace temp file with ACL state `acl` that
/// replaces a destination with mode `dest_mode` owned by
/// `dest_uid:dest_gid`, the temp file being owned by `tmp_uid:tmp_gid`.
/// Without an access ACL the bits are [`trace_mode`]'s. With one, or when
/// its state is unknown (fail closed), the file keeps its created `0600`:
/// a chmod would raise the ACL mask to the new group bits and let a named
/// user the old destination denied read the file (CWE-732), while the
/// `0600` mask leaves every named entry without effect.
#[cfg(any(unix, test))]
fn trace_chmod(
    dest_mode: u32,
    dest_uid: u32,
    dest_gid: u32,
    tmp_uid: u32,
    tmp_gid: u32,
    acl: AclState,
) -> TraceChmod {
    match acl {
        AclState::Absent => {
            TraceChmod::To(trace_mode(dest_mode, dest_uid, dest_gid, tmp_uid, tmp_gid))
        }
        AclState::Present | AclState::Unknown => TraceChmod::KeepCreated,
    }
}

/// Probes `file` for a POSIX access ACL (the `system.posix_acl_access`
/// extended attribute). A missing attribute or a filesystem without
/// extended attributes is [`AclState::Absent`]; any other error is
/// [`AclState::Unknown`].
#[cfg(target_os = "linux")]
fn acl_state(file: &std::fs::File) -> AclState {
    use std::os::fd::AsRawFd as _;
    let fd = file.as_raw_fd();
    // SAFETY: `fd` is an open descriptor borrowed from `file` for this
    // call, the name is a NUL-terminated C string literal, and a null
    // buffer of size 0 only asks for the value's length (nothing is
    // written).
    let r = unsafe {
        libc::fgetxattr(
            fd,
            c"system.posix_acl_access".as_ptr(),
            std::ptr::null_mut(),
            0,
        )
    };
    acl_state_from(r, std::io::Error::last_os_error().raw_os_error())
}

/// Maps a `getxattr`/`fgetxattr` length probe of `system.posix_acl_access`
/// to an [`AclState`]: `r >= 0` is [`AclState::Present`]; `-1` with
/// `ENODATA`, `ENOTSUP` or `EOPNOTSUPP` (the `errno` read right after the
/// call) is [`AclState::Absent`]; any other error is [`AclState::Unknown`].
#[cfg(target_os = "linux")]
fn acl_state_from(r: isize, errno: Option<i32>) -> AclState {
    if r >= 0 {
        return AclState::Present;
    }
    match errno {
        // EOPNOTSUPP equals ENOTSUP on most Linux targets; both are named.
        Some(e) if e == libc::ENODATA || e == libc::ENOTSUP || e == libc::EOPNOTSUPP => {
            AclState::Absent
        }
        _ => AclState::Unknown,
    }
}

/// No POSIX ACL probe off Linux: always [`AclState::Absent`].
#[cfg(all(unix, not(target_os = "linux")))]
fn acl_state(_: &std::fs::File) -> AclState {
    AclState::Absent
}

/// What the commit reads from the existing destination `dest`, all from
/// ONE inode: the destination itself is inspected with `symlink_metadata`
/// (never a symlink's target); only a regular file is then opened
/// read-only once (on Linux with `O_NOFOLLOW | O_NONBLOCK`, so a symlink or
/// FIFO swapped in cannot be followed or block), and that descriptor gives
/// both the mode, owner and group (`fstat`) and the access-ACL state
/// ([`acl_state`], `fgetxattr`). `Ok(None)` (the temp file keeps its created
/// `0600`) when `dest` is missing, a symlink or not a regular file, or when
/// the open fails for any reason, the opened file is not a regular file, or
/// its `(dev, ino)` differs from the inspected one (the destination was
/// replaced in between). An inspection error other than "not found" is
/// returned.
#[cfg(unix)]
fn dest_inode_inputs(dest: &Path) -> std::io::Result<Option<(std::fs::Metadata, AclState)>> {
    use std::os::unix::fs::MetadataExt as _;
    let seen = match std::fs::symlink_metadata(dest) {
        Ok(m) if m.is_file() => m,
        Ok(_) => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    let mut opts = std::fs::OpenOptions::new();
    opts.read(true);
    #[cfg(target_os = "linux")]
    {
        use std::os::unix::fs::OpenOptionsExt as _;
        opts.custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK);
    }
    // Fail closed: any open or fstat failure keeps the created 0600.
    let Ok(d) = opts.open(dest) else {
        return Ok(None);
    };
    let Ok(m) = d.metadata() else {
        return Ok(None);
    };
    if !m.is_file() || m.dev() != seen.dev() || m.ino() != seen.ino() {
        return Ok(None);
    }
    let acl = acl_state(&d);
    Ok(Some((m, acl)))
}

/// Gives the open temp `file` the permission bits of the existing
/// regular-file destination `dest`, as [`trace_mode`] allows (group bits
/// only when the temp file's owner and group, from `fstat`, match the
/// destination's), so the rename never widens access. The destination's
/// mode, owner, group and ACL state are read from one opened descriptor of
/// the destination itself ([`dest_inode_inputs`]): a missing destination, a
/// symlink (its target never supplies bits), any other non-regular file, an
/// unopenable destination or one replaced between inspection and open
/// leaves `file` as created (`0600`). ACL rule ([`trace_chmod`],
/// [`AclState::combine`]): when the temp file or the destination carries a
/// POSIX access ACL (Linux; on the temp file typically inherited from a
/// directory default ACL) or either ACL state cannot be read, the temp file
/// is not chmod-ed at all and keeps its created `0600`: a chmod would raise
/// the temp file's ACL mask, and copying the destination's mode bits alone
/// would drop the named-user entries the destination's ACL denies
/// (CWE-732).
#[cfg(unix)]
fn keep_dest_permissions(file: &std::fs::File, dest: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
    let Some((m, dest_acl)) = dest_inode_inputs(dest)? else {
        return Ok(());
    };
    let t = file.metadata()?;
    let acl = acl_state(file).combine(dest_acl);
    match trace_chmod(m.mode(), m.uid(), m.gid(), t.uid(), t.gid(), acl) {
        TraceChmod::To(mode) => file.set_permissions(std::fs::Permissions::from_mode(mode)),
        TraceChmod::KeepCreated => Ok(()),
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
    if path.file_name().is_none() {
        return Err(io(std::io::Error::other("not a file name")));
    }
    let dir = match path.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let pid = std::process::id();
    let mut last = None;
    for n in 0..TRACE_TMP_TRIES {
        // A short fixed prefix, never the destination's name: a
        // destination name at the file-name limit still gets a temp file.
        let tmp = dir.join(format!(".kp-trace.{pid}.{n}.tmp"));
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

fn run_app(args: &[String]) -> Result<(), String> {
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
        .plan(&TopologyMode::SingleMonitor {
            display: manager.displays().first().map_or(0, |d| d.index),
            backglass: DEFAULT_BACKGLASS_PIP,
            score: DEFAULT_SCORE_PIP,
        })
        .map_err(|e| e.to_string())?;
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
    forwarder.set_router(InputRouter::new());
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
            debug_overlay,
            ..TableRenderOptions::default()
        },
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
    use super::{
        AclState, TraceChmod, commit_trace, merge_failure, open_latency_trace, teardown_exit_code,
        trace_chmod, trace_mode,
    };
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

    /// REQ-LAT-003 (gate 30, M2, CWE-732): the destination's group bits
    /// are copied only when the temp file has the destination's owner and
    /// group; otherwise they are cleared (owner and other bits kept), and
    /// setuid, setgid and sticky bits are never copied.
    #[test]
    fn req_lat_003_trace_mode_keeps_group_bits_only_for_the_same_group() {
        assert_eq!(trace_mode(0o640, 1000, 100, 1000, 100), 0o640);
        assert_eq!(trace_mode(0o600, 1000, 100, 1000, 100), 0o600);
        assert_eq!(trace_mode(0o664, 1000, 100, 1000, 100), 0o664);
        // A setgid directory gave the temp file another group.
        assert_eq!(trace_mode(0o640, 1000, 100, 1000, 200), 0o600);
        assert_eq!(trace_mode(0o664, 1000, 100, 1000, 200), 0o604);
        // Another owner (the destination belongs to someone else).
        assert_eq!(trace_mode(0o660, 0, 100, 1000, 100), 0o600);
        // Special bits never copied.
        assert_eq!(trace_mode(0o7640, 1000, 100, 1000, 100), 0o640);
        assert_eq!(trace_mode(0o7640, 1000, 100, 1000, 200), 0o600);
        assert_eq!(trace_mode(0o7644, 1000, 100, 1000, 200), 0o604);
        assert_eq!(trace_mode(0o6755, 1000, 100, 1000, 100), 0o755);
        // Never wider than the destination.
        for m in 0..=0o7777u32 {
            for g in [100, 200] {
                let t = trace_mode(m, 1000, 100, 1000, g);
                assert_eq!(t & !(m & 0o777), 0, "{m:o} {g}");
            }
        }
    }

    /// REQ-LAT-003 (gate 30 round 2, CWE-732): a differing group or owner
    /// clamps `other` to the bits the destination's old group also had.
    #[test]
    fn req_lat_003_trace_mode_clamps_other_bits_to_the_old_group() {
        // Equal owner and group: unchanged.
        assert_eq!(trace_mode(0o640, 1000, 100, 1000, 100), 0o640);
        assert_eq!(trace_mode(0o604, 1000, 100, 1000, 100), 0o604);
        // Different group.
        assert_eq!(trace_mode(0o604, 1000, 100, 1000, 200), 0o600);
        assert_eq!(trace_mode(0o644, 1000, 100, 1000, 200), 0o604);
        assert_eq!(trace_mode(0o664, 1000, 100, 1000, 200), 0o604);
        assert_eq!(trace_mode(0o606, 1000, 100, 1000, 200), 0o600);
        assert_eq!(trace_mode(0o640, 1000, 100, 1000, 200), 0o600);
        // Different uid, equal gid: same clamp.
        assert_eq!(trace_mode(0o604, 1000, 100, 0, 100), 0o600);
        // Special bits never in the result.
        for m in [0o4604u32, 0o2604, 0o1604, 0o7644] {
            for (u, g) in [(1000, 100), (1000, 200), (0, 100)] {
                assert_eq!(trace_mode(m, 1000, 100, u, g) & !0o777, 0, "{m:o}");
            }
        }
    }

    /// REQ-LAT-003 (gate 30 round 2, B1, CWE-732): without an access ACL
    /// the temp file takes exactly [`trace_mode`]'s bits; with one, or with
    /// an unreadable ACL state, it keeps its created `0600` (no chmod).
    #[test]
    fn req_lat_003_trace_chmod_never_raises_an_acl_mask() {
        // Absent, same owner and group: the destination's 0o777 bits.
        for m in [0o600u32, 0o640, 0o644, 0o664, 0o666, 0o755] {
            assert_eq!(
                trace_chmod(m, 1000, 100, 1000, 100, AclState::Absent),
                TraceChmod::To(m & 0o777),
                "{m:o}"
            );
        }
        // Absent, different group or owner: whatever `trace_mode` says.
        for m in [0o600u32, 0o640, 0o644, 0o664, 0o666, 0o604] {
            for (u, g) in [(1000, 200), (0, 100)] {
                assert_eq!(
                    trace_chmod(m, 1000, 100, u, g, AclState::Absent),
                    TraceChmod::To(trace_mode(m, 1000, 100, u, g)),
                    "{m:o} {u} {g}"
                );
            }
        }
        // An access ACL, or an unknown ACL state: never chmod-ed.
        for acl in [AclState::Present, AclState::Unknown] {
            for m in [0o640u32, 0o644, 0o666, 0o600, 0o7777] {
                for (u, g) in [(1000, 100), (1000, 200), (0, 100)] {
                    assert_eq!(
                        trace_chmod(m, 1000, 100, u, g, acl),
                        TraceChmod::KeepCreated,
                        "{acl:?} {m:o} {u} {g}"
                    );
                }
            }
        }
        // setuid, setgid and sticky never appear in `To(..)`.
        for m in 0..=0o7777u32 {
            for (u, g) in [(1000, 100), (1000, 200), (0, 100)] {
                match trace_chmod(m, 1000, 100, u, g, AclState::Absent) {
                    TraceChmod::To(bits) => assert_eq!(bits & !0o777, 0, "{m:o}"),
                    TraceChmod::KeepCreated => panic!("Absent must chmod ({m:o})"),
                }
            }
        }
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

    /// REQ-LAT-003 (gate 30 round 2, B1): an ordinary file in a fresh
    /// temp directory has no access ACL, so the commit chmods it as before.
    #[test]
    #[cfg(target_os = "linux")]
    fn req_lat_003_acl_state_of_a_plain_file_is_absent() {
        let dir = fresh_dir("aclplain");
        let p = dir.join("plain");
        let f = std::fs::File::create(&p).unwrap();
        let state = super::acl_state(&f);
        if state == AclState::Unknown {
            let errno = fgetxattr(&f, c"system.posix_acl_access").err();
            eprintln!(
                "SKIPPED req_lat_003_acl_state_of_a_plain_file_is_absent: ACL state unreadable here (errno {errno:?})"
            );
            std::fs::remove_dir_all(&dir).unwrap();
            return;
        }
        assert_eq!(state, AclState::Absent);
        drop(f);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-LAT-003 (gate 30 round 2, B1, CWE-732): in a directory whose
    /// default ACL names another user, the temp file inherits that entry
    /// masked by its `0600`; the commit over a `0640` destination (which
    /// the named user cannot read) must not raise the mask, so the
    /// published file still denies that user. The no-ACL case is
    /// `req_lat_003_trace_commit_never_widens_permissions`.
    #[test]
    #[cfg(target_os = "linux")]
    fn req_lat_003_commit_keeps_acl_file_closed_to_named_users() {
        use std::os::unix::fs::PermissionsExt as _;
        const NAMED: u32 = 65534;
        let dir = fresh_dir("acl");
        // The destination exists before the default ACL, so it has no
        // access ACL and its 0640 denies the named user.
        let p = dir.join("t.csv");
        std::fs::write(&p, "old").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o640)).unwrap();
        // Default ACL: user::rwx, user:NAMED:r--, group::---, mask::rwx,
        // other::--- (ascending tag order).
        let me = std::os::unix::fs::MetadataExt::uid(&std::fs::metadata(&dir).unwrap());
        assert_ne!(me, NAMED, "the named user must be someone else");
        let mut blob = 2u32.to_le_bytes().to_vec();
        for (tag, perm, id) in [
            (0x01u16, 7u16, u32::MAX),
            (ACL_USER, 4, NAMED),
            (0x04, 0, u32::MAX),
            (ACL_MASK, 7, u32::MAX),
            (0x20, 0, u32::MAX),
        ] {
            blob.extend_from_slice(&tag.to_le_bytes());
            blob.extend_from_slice(&perm.to_le_bytes());
            blob.extend_from_slice(&id.to_le_bytes());
        }
        if let Err(errno) = setxattr(&dir, c"system.posix_acl_default", &blob) {
            if [libc::ENOTSUP, libc::EOPNOTSUPP, libc::EPERM, libc::ENODATA].contains(&errno) {
                eprintln!(
                    "SKIPPED req_lat_003_commit_keeps_acl_file_closed: ACLs unsupported here (errno {errno})"
                );
                std::fs::remove_dir_all(&dir).unwrap();
                return;
            }
            panic!("setxattr(system.posix_acl_default) failed: errno {errno}");
        }
        let sink = open_latency_trace(&args(&["--latency-trace", p.to_str().unwrap()]))
            .unwrap()
            .unwrap();
        // The scenario is real: the temp file inherited an access ACL.
        assert_eq!(
            super::acl_state(sink.file.as_ref().unwrap()),
            AclState::Present,
            "the temp file inherits the directory's default ACL"
        );
        sink.commit(b"new").unwrap();
        // (3) The content is replaced.
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
        // (2) Mode stays 0600 (the group class reports the mask: 0).
        assert_eq!(mode(&p), 0o600, "the published trace keeps 0600");
        // (1) The mask is `---`, so the named user's effective access is 0.
        let f = std::fs::File::open(&p).unwrap();
        match fgetxattr(&f, c"system.posix_acl_access") {
            Ok(acl) => {
                let entries = acl_entries(&acl);
                let mask = entries
                    .iter()
                    .find(|e| e.0 == ACL_MASK)
                    .map(|e| e.1)
                    .expect("an ACL with a named user has a mask");
                assert_eq!(mask, 0, "ACL mask stays ---: {entries:?}");
                let named: Vec<_> = entries.iter().filter(|e| e.0 == ACL_USER).collect();
                assert!(!named.is_empty(), "{entries:?}");
                for e in named {
                    assert_eq!(e.1 & mask, 0, "named user {} gets no access", e.2);
                }
            }
            Err(errno) => assert_eq!(errno, libc::ENODATA, "reading the published ACL"),
        }
        drop(f);
        assert_eq!(names(&dir), vec!["t.csv".to_owned()]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-LAT-003 (gate 30 round 3, CWE-732): the ACL state of the temp
    /// file and of the destination combine fail-closed: `Unknown` wins,
    /// then `Present`; only `Absent` on both chmods, exactly as before.
    #[test]
    fn req_lat_003_acl_state_combine_is_fail_closed() {
        use AclState::{Absent, Present, Unknown};
        let all = [Absent, Present, Unknown];
        let want = [
            [Absent, Present, Unknown],
            [Present, Present, Unknown],
            [Unknown, Unknown, Unknown],
        ];
        for (i, a) in all.into_iter().enumerate() {
            for (j, b) in all.into_iter().enumerate() {
                assert_eq!(a.combine(b), want[i][j], "{a:?}.combine({b:?})");
            }
        }
        // A destination ACL (or an unreadable one) keeps the temp file 0600.
        for acl in [Absent.combine(Present), Absent.combine(Unknown)] {
            for m in [0o640u32, 0o644, 0o666] {
                for (u, g) in [(1000, 100), (1000, 200), (0, 100)] {
                    assert_eq!(
                        trace_chmod(m, 1000, 100, u, g, acl),
                        TraceChmod::KeepCreated,
                        "{acl:?} {m:o} {u} {g}"
                    );
                }
            }
        }
        // No ACL on either file: today's bits.
        for m in [0o600u32, 0o640, 0o644, 0o664, 0o666, 0o7644] {
            for (u, g) in [(1000, 100), (1000, 200), (0, 100)] {
                assert_eq!(
                    trace_chmod(m, 1000, 100, u, g, Absent.combine(Absent)),
                    TraceChmod::To(trace_mode(m, 1000, 100, u, g)),
                    "{m:o} {u} {g}"
                );
            }
        }
    }

    /// REQ-LAT-003 (gate 30 round 3; gate 30 round 4, review thread
    /// 4699765169): the destination's ACL is probed on its opened
    /// descriptor (the path probe is gone); a plain file written in a
    /// fresh temp directory, then opened read-only, has no access ACL.
    #[test]
    #[cfg(target_os = "linux")]
    fn req_lat_003_acl_state_of_opened_plain_file_is_absent() {
        let dir = fresh_dir("aclpath");
        let p = dir.join("plain");
        std::fs::write(&p, "x").unwrap();
        let f = std::fs::File::open(&p).unwrap();
        let state = super::acl_state(&f);
        if state == AclState::Unknown {
            let errno = fgetxattr(&f, c"system.posix_acl_access").err();
            eprintln!(
                "SKIPPED req_lat_003_acl_state_of_opened_plain_file_is_absent: ACL state unreadable here (errno {errno:?})"
            );
            std::fs::remove_dir_all(&dir).unwrap();
            return;
        }
        assert_eq!(state, AclState::Absent);
        drop(f);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// REQ-LAT-003 (gate 30 round 4, review thread 4699765150, CWE-732):
    /// a symlinked trace path whose target is an ACL-free `0644` file with
    /// the temp file's owner and group supplies no permission bits: the
    /// published file at the link path is a regular file with the created
    /// `0600`, and the target's bytes and `0644` are unchanged.
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
        assert_eq!(
            super::acl_state(sink.file.as_ref().unwrap()),
            AclState::Absent,
            "the temp file has no access ACL"
        );
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

    /// REQ-LAT-003 (gate 30 round 4, review threads 4699765150 and
    /// 4699765169, CWE-367): the destination's mode and ACL come from one
    /// opened descriptor of the destination itself. A regular `0644` file
    /// gives its metadata and `Absent`; a symlink to it, a directory, a
    /// FIFO and a missing path give `None` (the temp file keeps `0600`); an
    /// unreadable file gives `None` too (checked when not running as root,
    /// which can open any file).
    #[test]
    #[cfg(unix)]
    fn req_lat_003_dest_inode_inputs_reads_one_regular_inode() {
        use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};
        let dir = fresh_dir("inode");
        let p = dir.join("t.csv");
        std::fs::write(&p, "old").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        let (m, acl) = super::dest_inode_inputs(&p)
            .unwrap()
            .expect("a regular file");
        assert_eq!(m.mode() & 0o777, 0o644);
        assert_eq!(m.ino(), std::fs::symlink_metadata(&p).unwrap().ino());
        if acl == AclState::Unknown {
            eprintln!(
                "SKIPPED req_lat_003_dest_inode_inputs_reads_one_regular_inode ACL check: ACL state unreadable here"
            );
        } else {
            assert_eq!(acl, AclState::Absent);
        }
        let link = dir.join("link.csv");
        std::os::unix::fs::symlink(&p, &link).unwrap();
        assert!(
            super::dest_inode_inputs(&link).unwrap().is_none(),
            "symlink"
        );
        let sub = dir.join("sub");
        std::fs::create_dir(&sub).unwrap();
        assert!(
            super::dest_inode_inputs(&sub).unwrap().is_none(),
            "directory"
        );
        let fifo = dir.join("f.fifo");
        let made = std::process::Command::new("mkfifo").arg(&fifo).status();
        if !matches!(&made, Ok(s) if s.success()) {
            let _ = std::fs::remove_dir_all(&dir);
            panic!("mkfifo is required for the FIFO check and failed: {made:?}");
        }
        assert!(super::dest_inode_inputs(&fifo).unwrap().is_none(), "FIFO");
        let missing = dir.join("missing.csv");
        assert!(
            super::dest_inode_inputs(&missing).unwrap().is_none(),
            "missing"
        );
        let me = std::fs::metadata(&dir).unwrap().uid();
        if me != 0 {
            let locked = dir.join("locked.csv");
            std::fs::write(&locked, "x").unwrap();
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o200)).unwrap();
            assert!(
                super::dest_inode_inputs(&locked).unwrap().is_none(),
                "unreadable"
            );
        }
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

    /// REQ-LAT-003 (gate 30 round 3, CWE-732, review thread 4699043335):
    /// a `0644` destination whose access ACL denies a named user
    /// (`user:65534:---`) in a directory with NO default ACL. The temp
    /// file has no ACL and the same owner and group, so copying the mode
    /// would publish a `0644` file without the denial; the destination
    /// probe keeps it `0600` instead. Gate 30 round 4 (review thread
    /// 4699765169): the destination's ACL is probed on an opened
    /// descriptor, the way the commit now reads it.
    #[test]
    #[cfg(target_os = "linux")]
    fn req_lat_003_commit_keeps_dest_acl_denial() {
        use std::os::unix::fs::PermissionsExt as _;
        const NAMED: u32 = 65534;
        let dir = fresh_dir("destacl");
        let me = std::os::unix::fs::MetadataExt::uid(&std::fs::metadata(&dir).unwrap());
        assert_ne!(me, NAMED, "the named user must be someone else");
        let p = dir.join("t.csv");
        std::fs::write(&p, "old").unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644)).unwrap();
        // user::rw-, user:NAMED:---, group::r--, mask::r--, other::r--.
        let blob = acl_blob(&[
            (0x01, 6, u32::MAX),
            (ACL_USER, 0, NAMED),
            (0x04, 4, u32::MAX),
            (ACL_MASK, 4, u32::MAX),
            (0x20, 4, u32::MAX),
        ]);
        if let Err(errno) = setxattr(&p, c"system.posix_acl_access", &blob) {
            if [libc::ENOTSUP, libc::EOPNOTSUPP, libc::EPERM, libc::ENODATA].contains(&errno) {
                eprintln!(
                    "SKIPPED req_lat_003_commit_keeps_dest_acl: ACLs unsupported here (errno {errno})"
                );
                std::fs::remove_dir_all(&dir).unwrap();
                return;
            }
            panic!("setxattr(system.posix_acl_access) failed: errno {errno}");
        }
        // The scenario is real: the destination has an ACL, the directory
        // no default one (so the temp file has none).
        assert_eq!(
            super::acl_state(&std::fs::File::open(&p).unwrap()),
            AclState::Present
        );
        assert_eq!(mode(&p), 0o644);
        let sink = open_latency_trace(&args(&["--latency-trace", p.to_str().unwrap()]))
            .unwrap()
            .unwrap();
        assert_eq!(
            super::acl_state(sink.file.as_ref().unwrap()),
            AclState::Absent,
            "no default ACL: the temp file has no access ACL"
        );
        sink.commit(b"new trace").unwrap();
        // (1) Never the destination's 0644: the temp file keeps 0600.
        assert_eq!(mode(&p), 0o600, "the published trace keeps 0600");
        // (2) The content is replaced.
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new trace");
        // (3) If an ACL is still there, the named user gets nothing.
        let f = std::fs::File::open(&p).unwrap();
        match fgetxattr(&f, c"system.posix_acl_access") {
            Ok(acl) => {
                let entries = acl_entries(&acl);
                let mask = entries
                    .iter()
                    .find(|e| e.0 == ACL_MASK)
                    .map_or(0o7, |e| e.1);
                for e in entries.iter().filter(|e| e.0 == ACL_USER && e.2 == NAMED) {
                    assert_eq!(e.1 & mask, 0, "named user {} gets no access", e.2);
                }
            }
            // No ACL: the 0600 mode alone denies every other user.
            Err(errno) => assert_eq!(errno, libc::ENODATA, "reading the published ACL"),
        }
        drop(f);
        // (4) No leftover temp file.
        assert_eq!(names(&dir), vec!["t.csv".to_owned()]);
        std::fs::remove_dir_all(&dir).unwrap();
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
}
