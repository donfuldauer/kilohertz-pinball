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
//!   (`orchestrator::trace`, which documents the CSV format). The file is
//!   created before anything starts (an unwritable path fails the run with
//!   `pin_app: --latency-trace: <error>`), written on the main thread after
//!   the threads were joined, and summarised after the shutdown lines as
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

use pin_app::orchestrator::trace::{CollectedTrace, FlipperBindings, latency_trace, summary_line};
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

/// `--latency-trace <file>`: the path and the file, created (truncated)
/// now so an unwritable path fails before the app starts.
fn open_latency_trace(args: &[String]) -> Result<Option<(PathBuf, std::fs::File)>, String> {
    let Some(path) = path_of(args, "--latency-trace") else {
        return Ok(None);
    };
    let path = path.map_err(|e| format!("--latency-trace: {e}"))?;
    let file = std::fs::File::create(&path)
        .map_err(|e| format!("--latency-trace: {}: {e}", path.display()))?;
    Ok(Some((path, file)))
}

/// Writes the collected trace to `file`.
fn write_latency_trace(file: &mut std::fs::File, trace: &CollectedTrace) -> Result<(), String> {
    file.write_all(trace.to_csv().as_bytes())
        .and_then(|()| file.flush())
        .map_err(|e| format!("--latency-trace: writing the trace: {e}"))
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
        if trace.is_some() && !first_frame_announced && live.render_frames > 0 {
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
    // on the main thread.
    let trace_summary = match (trace, trace_file.as_mut()) {
        (Some(collector), Some((path, file))) => {
            let collected = collector.collect();
            match write_latency_trace(file, &collected) {
                Ok(()) => Some(summary_line(path, &collected)),
                Err(e) => {
                    eprintln!("pin_app: {e}");
                    failure = merge_failure(failure, Some(e));
                    None
                }
            }
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
    use super::{merge_failure, open_latency_trace, teardown_exit_code};

    fn args(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| (*s).to_owned()).collect()
    }

    /// REQ-LAT-003: `--latency-trace` creates its file up front; an
    /// unwritable path or a missing value is an error naming the flag;
    /// without the flag nothing is opened.
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
        let dir = std::env::temp_dir().join(format!("kp_trace_flag_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("t.csv");
        std::fs::write(&p, "old").unwrap();
        let (path, _file) = open_latency_trace(&args(&["--latency-trace", p.to_str().unwrap()]))
            .unwrap()
            .unwrap();
        assert_eq!(path, p);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "", "truncated");
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
