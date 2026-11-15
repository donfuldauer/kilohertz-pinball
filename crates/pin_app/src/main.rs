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
//!   table, else the idle stub sim. `--headless` uses SDL's dummy drivers (no
//!   surfaces are created then), `--run-ms N` quits after N ms of wall time,
//!   `--vrr` requests `FifoRelaxed` for the playfield.
//!
//! Input goes through the device profiles and `HeldInputs`
//! (`platform::InputRouter`). Windows are owned by the platform's
//! `WindowKeeper`; each surface holds a `WindowGuard` clone, so a presenter
//! thread detached at shutdown keeps its window alive (issue #9a). Shutdown
//! order: join the engine threads, destroy released windows on the main
//! thread, wait (bounded) for a detached presenter to release its window,
//! and if one never does, exit without destroying it.

use pin_app::orchestrator::{
    AudioConfig, ClosePolicy, DropMonitor, FlipperOutline, RenderConfig, RenderGpu, TableSim,
    ThreadsConfig, run_headless, run_headless_table, spawn_threads, table_lines, validate_table,
};
use pin_app::platform::{
    DisplayTopologyManager, EventForwarder, INPUT_RING_CAPACITY, InputRouter, PUMP_TIMEOUT,
    Platform, PlatformConfig, WINDOW_RING_CAPACITY, WindowGuard,
};
use pin_input::haptics::{DEFAULT_HAPTICS_RING_CAPACITY, haptics_ring};
use pin_render::gpu::{
    GpuOptions, PRESENTER_JOIN_TIMEOUT, SurfaceOptions, SurfaceSource, create_instance,
    create_surface,
};
use pin_render::topology::{DEFAULT_BACKGLASS_PIP, DEFAULT_SCORE_PIP, TopologyMode};
use pin_sim::{SimConfig, Simulation};
use std::path::{Path, PathBuf};
use std::process::ExitCode;
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

/// The table to run: validated, loaded, with its debug-view lines.
struct LoadedApp {
    sim: Simulation,
    table_lines: Vec<pin_render::debug::DebugLine>,
    flippers: Vec<FlipperOutline>,
}

fn load_app_table(path: &Path) -> Result<LoadedApp, String> {
    let checked = validate_table(path)?;
    for w in &checked.warnings {
        eprintln!("pin_app: table warning: {w}");
    }
    let sim = Simulation::load(path, SimConfig::default())
        .map_err(|e| format!("table {}: {e}", path.display()))?;
    eprintln!(
        "pin_app: table {:?} loaded ({} switches, {} mechs)",
        checked.table.manifest.meta.name,
        checked.table.switches.len(),
        checked.table.mechs.len()
    );
    Ok(LoadedApp {
        sim,
        table_lines: table_lines(&checked.scene, MAX_TABLE_LINES),
        flippers: FlipperOutline::of(&checked.table),
    })
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

fn run_app(args: &[String]) -> Result<(), String> {
    let headless = args.iter().any(|a| a == "--headless");
    let run_for = value_of(args, "--run-ms")
        .transpose()?
        .map(Duration::from_millis);
    let vrr = args.iter().any(|a| a == "--vrr");
    // Load the table before opening windows: a bad table fails fast.
    let table = path_of(args, "--table")
        .transpose()?
        .map(|p| load_app_table(&p))
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

    let (mut cfg, table_render) = match table {
        Some(t) => {
            let (htx, hrx) = haptics_ring(DEFAULT_HAPTICS_RING_CAPACITY);
            let mut cfg =
                ThreadsConfig::with_sim(Box::new(TableSim::new(t.sim, Some(htx))), input_rx);
            cfg.audio = AudioConfig {
                haptics: Some(hrx),
                ..AudioConfig::default()
            };
            (cfg, Some((t.table_lines, t.flippers)))
        }
        None => (ThreadsConfig::idle(input_rx), None),
    };
    let (table_lines, flippers) = table_render.unwrap_or_default();
    cfg.now_ns = sdl_now_ns;
    cfg.render = RenderConfig {
        gpu,
        window_events: Some(window_rx),
        close_policy: ClosePolicy::PlayfieldOnly,
        table_lines,
        flippers,
        ..RenderConfig::default()
    };
    let threads = spawn_threads(cfg)?;

    let started = Instant::now();
    let mut input_drops = DropMonitor::default();
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
        if run_for.is_some_and(|d| started.elapsed() >= d) {
            break;
        }
    }
    // 1. Join the engine threads (the render thread drops the surfaces; a
    //    presenter stalled past PRESENTER_JOIN_TIMEOUT is detached and keeps
    //    its guard).
    let audio = threads.audio_stats();
    let stats = threads.shutdown()?;
    // The main thread's own guards: from here only a detached presenter can
    // still hold a window.
    drop(guards);
    eprintln!(
        "pin_app: {} sim ticks ({} dropped), {} frames, input dropped {}, {} haptic events",
        stats.sim_ticks,
        stats.sim_dropped_ticks,
        stats.render_frames,
        forwarder.stats().input_dropped,
        audio.haptic_events
    );
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
        std::process::exit(0);
    }
    drop(platform);
    Ok(())
}
