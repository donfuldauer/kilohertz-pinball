//! #133 (KP-052): GPU device loss and rebuild in the real windowed app
//! (SPEC §4 Subphase 6.0 deliverable 1, SPEC §4 Subphase 1.4 deliverable 1,
//! SPEC §4 Subphase 5.4 item 4, D19; the R12 windowed evidence).
//!
//! An SDL playfield window gets a wgpu surface; the orchestrator's render
//! thread draws the grey-box table into it. The test forces a device loss
//! through `RenderConfig::force_device_loss` (the render thread destroys its
//! own device) and checks, with counters only, that the loss is counted,
//! the render thread rebuilds its context, renderers and surfaces, and
//! presents again within a bounded number of render-loop iterations; then
//! does it a second time (the rebuild budget resets after a success); then
//! shuts down cleanly and releases the window guards.
//!
//! Frames are counted, never timed (R6): no clocks and no sleeps; every wait
//! is a bounded poll loop with `yield_now`.
//!
//! GPU job only (`--features gpu-tests`): without a native SDL video driver
//! or a GPU adapter it FAILS, it never skips. SDL needs the main thread, so
//! this is a plain `main` (harness = false) through `pin_testmain`.

mod support;

use arrayvec::ArrayVec;
use pin_app::orchestrator::{
    MAX_DEVICE_REBUILD_ATTEMPTS, RenderConfig, RenderGpu, StatsSnapshot, TableVisuals, Threads,
    ThreadsConfig, load_checked, spawn_threads, validate_table,
};
use pin_app::platform::{WindowGuard, WindowKeeper};
use pin_render::gpu::{GpuOptions, SurfaceOptions, SurfaceSource, create_instance, create_surface};
use pin_render::topology::{Rect, SurfaceRole};
use pin_sim::SimConfig;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use support::{greybox_bundle, on_big_stack};

/// Counter polls (a `yield_now` and an SDL pump each) for any one wait (a
/// count, not a duration: D8).
const MAX_POLLS: u64 = 5_000_000;

/// Frames presented before the first forced loss.
const WARM_UP_FRAMES: u64 = 10;

/// Render-loop iterations within which the first present after a counted
/// loss must follow (the brief's bound).
const REBUILD_ITERATIONS: u64 = 60;

/// Further frames required after the loss, and the slack in iterations they
/// may take beyond [`REBUILD_ITERATIONS`].
const FRAMES_AFTER_LOSS: u64 = 5;

const NATIVE: [&str; 4] = ["windows", "cocoa", "x11", "wayland"];

fn source(
    instance: &pin_render::gpu::wgpu::Instance,
    w: &sdl3::video::Window,
    keep_alive: WindowGuard,
) -> SurfaceSource {
    let (lw, lh) = w.size();
    let (pw, ph) = w.size_in_pixels();
    let mut viewports = ArrayVec::new();
    viewports.push((SurfaceRole::Playfield, Rect::new(0, 0, lw, lh)));
    SurfaceSource {
        window: 0,
        viewports,
        // SAFETY: the keeper destroys the window only after the last guard
        // (one goes with the surface) is dropped.
        surface: unsafe { create_surface(instance, w) }.expect("create_surface"),
        logical_size: (lw, lh),
        pixel_size: (pw, ph),
        keep_alive: Some(keep_alive),
    }
}

/// The grey-box table's visuals.
fn visuals() -> TableVisuals {
    let dir = greybox_bundle().to_path_buf();
    on_big_stack(move || {
        let checked = validate_table(&dir).expect("greybox validates");
        let _sim = load_checked(&checked, SimConfig::default()).expect("greybox loads");
        TableVisuals::of(&checked)
    })
}

/// Polls `threads.stats()` (bounded, pumping SDL events and yielding) until
/// `done` holds; panics with the last stats and `what` otherwise.
fn wait_for(
    threads: &Threads,
    pump: &mut sdl3::EventPump,
    what: &str,
    done: impl Fn(&StatsSnapshot) -> bool,
) -> StatsSnapshot {
    let mut s = threads.stats();
    for _ in 0..MAX_POLLS {
        if done(&s) {
            return s;
        }
        while pump.poll_event().is_some() {}
        std::thread::yield_now();
        s = threads.stats();
    }
    if done(&s) {
        return s;
    }
    panic!("[device_loss_window] {what}: not reached within {MAX_POLLS} polls; stats {s:?}");
}

/// Forces loss number `n` and checks the counted recovery. Returns the
/// stats once the frames rose again.
fn force_loss_and_recover(
    threads: &Threads,
    pump: &mut sdl3::EventPump,
    flag: &AtomicBool,
    n: u64,
) -> StatsSnapshot {
    let before = threads.stats();
    assert_eq!(before.device_losses, n - 1, "{before:?}");
    assert_eq!(before.device_rebuilds, n - 1, "{before:?}");
    flag.store(true, Ordering::Release);
    let lost = wait_for(threads, pump, &format!("loss {n} counted"), |s| {
        s.device_losses >= n
    });
    let (i0, f0) = (lost.render_iterations, lost.render_frames);
    println!(
        "[device_loss_window] loss {n} counted at render iteration {i0}, frame {f0}: {lost:?}"
    );
    let back = wait_for(threads, pump, &format!("frames after loss {n}"), |s| {
        s.render_frames >= f0 + FRAMES_AFTER_LOSS || s.gpu_error
    });
    println!("[device_loss_window] after loss {n}: {back:?}");
    assert!(
        !back.gpu_error,
        "loss {n}: the render thread gave up: {back:?}"
    );
    assert!(back.render_frames >= f0 + FRAMES_AFTER_LOSS, "{back:?}");
    let iters = back.render_iterations - i0;
    assert!(
        iters <= REBUILD_ITERATIONS + FRAMES_AFTER_LOSS,
        "loss {n}: {FRAMES_AFTER_LOSS} frames took {iters} render iterations (bound {}): {back:?}",
        REBUILD_ITERATIONS + FRAMES_AFTER_LOSS
    );
    assert_eq!(back.device_losses, n, "loss {n}: {back:?}");
    assert_eq!(back.device_rebuilds, n, "loss {n}: {back:?}");
    assert!(back.gpu_ready, "loss {n}: {back:?}");
    assert!(
        !flag.load(Ordering::Acquire),
        "the render thread did not clear the force flag"
    );
    back
}

/// Optional visual evidence (R12, never the only evidence): when
/// `PIN_DEVICE_LOSS_SCREENSHOT=<path.xwd>` is set, a few more frames are
/// presented after the last rebuild and the X root window is dumped with
/// `xwd`. A failing dump is reported, not fatal: the counters are the test.
fn screenshot(threads: &Threads, pump: &mut sdl3::EventPump, frames: u64) {
    let Some(path) = std::env::var_os("PIN_DEVICE_LOSS_SCREENSHOT") else {
        return;
    };
    wait_for(threads, pump, "frames before the screenshot", |s| {
        s.render_frames >= frames + 10
    });
    let out = std::process::Command::new("xwd")
        .args(["-root", "-silent", "-out"])
        .arg(&path)
        .output();
    match out {
        Ok(o) if o.status.success() => {
            println!("[device_loss_window] screenshot written to {path:?}");
        }
        Ok(o) => println!(
            "[device_loss_window] xwd failed: {}",
            String::from_utf8_lossy(&o.stderr)
        ),
        Err(e) => println!("[device_loss_window] xwd not run: {e}"),
    }
}

fn main() {
    pin_testmain::libtest_main("device_loss_window", run)
}

/// SPEC §4 Subphase 6.0 deliverable 1, SPEC §4 Subphase 1.4 deliverable 1,
/// SPEC §4 Subphase 5.4 item 4: after a forced device loss the windowed app
/// rebuilds and presents again within a bounded number of render
/// iterations, twice, and shuts down cleanly.
fn run() -> pin_testmain::Outcome {
    assert_eq!(MAX_DEVICE_REBUILD_ATTEMPTS, 3, "rebuild budget per loss");
    let sdl = sdl3::init().expect("SDL_Init (a missing display is a failure here)");
    let video = sdl.video().expect("SDL video");
    let driver = video.current_video_driver();
    assert!(
        NATIVE.contains(&driver),
        "SDL video driver {driver:?} has no native windows (run under xvfb-run on Linux)"
    );
    let mut keeper = WindowKeeper::new();
    let guard = {
        let mut win = video
            .window("pin_app device loss test playfield", 240, 360)
            .position(32, 32)
            .build()
            .expect("window");
        win.raise();
        keeper.adopt(win)
    };
    assert_eq!(guard.strong_count(), 1);
    let mut pump = sdl.event_pump().expect("pump");
    let visuals = visuals();

    let options = GpuOptions::from_env();
    let instance = create_instance(options.backends);
    let playfield = keeper.windows().next().expect("playfield window");
    let surfaces = vec![source(&instance, playfield, guard.clone())];
    let flag = Arc::new(AtomicBool::new(false));
    let (_tx, rx) = pin_core::spsc::channel(16);
    let mut cfg = ThreadsConfig::idle(rx);
    cfg.render = RenderConfig {
        gpu: Some(RenderGpu {
            instance,
            options,
            surfaces,
            surface_options: SurfaceOptions::default(),
        }),
        table: Some(Arc::new(visuals)),
        force_device_loss: Some(flag.clone()),
        ..RenderConfig::default()
    };
    assert!(
        RenderConfig::default().force_device_loss.is_none(),
        "forcing a loss is opt-in"
    );
    let threads = spawn_threads(cfg).expect("spawn");

    // a. Warm-up (a missing adapter shows as gpu_error: a failure here).
    let warm = wait_for(&threads, &mut pump, "warm-up", |s| {
        s.render_frames >= WARM_UP_FRAMES || s.gpu_error
    });
    println!("[device_loss_window] warm: {warm:?}");
    assert!(warm.gpu_ready && !warm.gpu_error, "no GPU: {warm:?}");
    assert!(warm.render_frames >= WARM_UP_FRAMES, "{warm:?}");
    assert_eq!(warm.device_losses, 0, "{warm:?}");
    assert_eq!(warm.device_rebuilds, 0, "{warm:?}");

    // b. First forced loss.
    force_loss_and_recover(&threads, &mut pump, &flag, 1);
    // c. Second forced loss on the same run.
    let last = force_loss_and_recover(&threads, &mut pump, &flag, 2);
    println!("[device_loss_window] final stats {last:?}");
    screenshot(&threads, &mut pump, last.render_frames);

    // d. Clean shutdown: the join returns, the surface's guard is released,
    // the window is destroyed after the last guard.
    threads.shutdown().expect("shutdown");
    assert_eq!(
        guard.strong_count(),
        1,
        "the render thread or a rebuilt surface still holds the window guard"
    );
    drop(guard);
    keeper.drain();
    assert!(keeper.is_empty(), "window not destroyed after the guards");
    println!("[device_loss_window] ok");
    pin_testmain::Outcome::Passed
}
