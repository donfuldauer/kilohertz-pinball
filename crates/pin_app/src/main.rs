//! `pin_app`: Kilohertz Pinball application binary.
//!
//! The modules live in the `pin_app` library (`src/lib.rs`).
//!
//! * `--headless-ticks N`: runs N sim ticks on simulated time (no SDL, GPU or
//!   sleeping), checks the snapshot ticks read back through the triple buffer
//!   are +1 each, prints `headless: N ticks, last tick N` and exits 0
//!   (non-zero on a violation).
//! * `--list-displays [--headless]`: prints SDL's displays (Subphase 1.3).
//! * otherwise: opens the playfield window (single-monitor topology), spawns
//!   the sim/render/audio/I/O threads and pumps SDL events on the main thread
//!   until quit. `--headless` uses SDL's dummy drivers (no surfaces are
//!   created then), `--run-ms N` quits after N ms of wall time, `--vrr`
//!   requests `FifoRelaxed` for the playfield.

use pin_app::orchestrator::{
    ClosePolicy, DropMonitor, RenderConfig, RenderGpu, ThreadsConfig, run_headless, spawn_threads,
};
use pin_app::platform::{
    DisplayTopologyManager, EventForwarder, INPUT_RING_CAPACITY, PUMP_TIMEOUT, Platform,
    PlatformConfig, PlatformWindow, WINDOW_RING_CAPACITY,
};
use pin_render::gpu::{GpuOptions, SurfaceOptions, SurfaceSource, create_instance, create_surface};
use pin_render::topology::{DEFAULT_BACKGLASS_PIP, DEFAULT_SCORE_PIP, TopologyMode};
use std::process::ExitCode;
use std::time::{Duration, Instant};

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

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if let Some(n) = value_of(&args, "--headless-ticks") {
        let result = n.and_then(|n| run_headless(n).map(|last| (n, last)));
        return match result {
            Ok((n, last)) => {
                println!("headless: {n} ticks, last tick {last}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("pin_app: {e}");
                ExitCode::FAILURE
            }
        };
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

    let mut platform = Platform::init(PlatformConfig { headless }).map_err(|e| e.to_string())?;
    let manager = DisplayTopologyManager::query(platform.video()).map_err(|e| e.to_string())?;
    let plan = manager
        .plan(&TopologyMode::SingleMonitor {
            display: manager.displays().first().map_or(0, |d| d.index),
            backglass: DEFAULT_BACKGLASS_PIP,
            score: DEFAULT_SCORE_PIP,
        })
        .map_err(|e| e.to_string())?;
    let windows: Vec<PlatformWindow> =
        DisplayTopologyManager::create_windows(platform.video(), &plan)
            .map_err(|e| e.to_string())?;

    let (input_tx, input_rx) = pin_core::spsc::channel(INPUT_RING_CAPACITY);
    let (window_tx, window_rx) = pin_core::spsc::channel(WINDOW_RING_CAPACITY);
    let mut forwarder = EventForwarder::new(input_tx, window_tx);
    let ids: Vec<u32> = windows.iter().map(PlatformWindow::id).collect();
    forwarder.register_windows(&ids);

    // Surfaces are created here on the main thread and moved to the render
    // thread, which owns the device (PLAN §2.2).
    let driver = platform.video().current_video_driver();
    let gpu = if NATIVE_DRIVERS.contains(&driver) {
        let options = GpuOptions::for_app();
        let instance = create_instance(options.backends);
        let mut surfaces = Vec::with_capacity(windows.len());
        for w in &windows {
            // SAFETY: `windows` outlives the surfaces: the render thread (which
            // drops them) is joined by `threads.shutdown()` below, before
            // `windows` is dropped.
            let surface =
                unsafe { create_surface(&instance, &w.window) }.map_err(|e| e.to_string())?;
            let (pw, ph) = w.window.size_in_pixels();
            surfaces.push(SurfaceSource {
                window: w.plan_index,
                viewports: w.plan.viewports.clone(),
                surface,
                logical_size: (w.plan.bounds.w, w.plan.bounds.h),
                pixel_size: (pw, ph),
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

    let mut cfg = ThreadsConfig::idle(input_rx);
    cfg.now_ns = sdl_now_ns;
    cfg.render = RenderConfig {
        gpu,
        window_events: Some(window_rx),
        close_policy: ClosePolicy::PlayfieldOnly,
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
    let stats = threads.shutdown()?;
    eprintln!(
        "pin_app: {} sim ticks ({} dropped), {} frames, input dropped {}",
        stats.sim_ticks,
        stats.sim_dropped_ticks,
        stats.render_frames,
        forwarder.stats().input_dropped
    );
    drop(windows);
    Ok(())
}
