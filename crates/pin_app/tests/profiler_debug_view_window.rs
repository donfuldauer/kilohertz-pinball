//! w8g-d: a run that draws the debug view records GPU frame time and paints
//! the profiler HUD (feature `gpu-tests`; SDL windows under Xvfb on Linux
//! CI, pattern of `orchestrator_window.rs`).
//!
//! A table run falls back to the debug view when its playfield surface has
//! no 8-bit format (`choose_presenter`, unit-tested in `threads.rs`); real
//! adapters (lavapipe included) always offer one, so the fallback cannot be
//! forced on a real window. The debug view is the same presenter for a run
//! without a table, so this test drives it that way:
//!
//! * Part 1: a `SurfaceSet` on real windows with the hooks the render
//!   thread builds for it (`debug_view_hooks`, the same function) and
//!   frames drawn through `SurfaceSet::render_with`: GPU frame time reaches
//!   the profiler (with `TIMESTAMP_QUERY`; bounded poll loop, no sleeps),
//!   and with HUD text the overlay is painted on every presented playfield
//!   frame. Without `TIMESTAMP_QUERY` there is no timer, no sample and no
//!   panic; the test prints which branch ran.
//! * Part 2: the orchestrator's render thread with no table, a profiler
//!   (`present_profiler`) and `--profiler-hud` (`hud`): its debug view
//!   records GPU frame time.
//!
//! This file has no `[[test]]` entry, so it runs under the libtest harness
//! with one `#[test]`: its thread is the only one that touches SDL, which
//! the X11 / Wayland drivers allow. Without a native video driver it
//! prints SKIP, or fails when `PIN_REQUIRE_GPU=1`.
#![cfg(feature = "gpu-tests")]

use arrayvec::ArrayVec;
use pin_app::orchestrator::{
    GpuFrameTimer, HudFeed, RenderConfig, RenderGpu, ThreadsConfig, debug_view_hooks, spawn_threads,
};
use pin_app::platform::{WindowGuard, WindowKeeper};
use pin_app::profiler::{Profiler, hud_lines};
use pin_render::debug::{Camera, DebugLines, box_lines};
use pin_render::gpu::{
    GpuContext, GpuOptions, SurfaceOptions, SurfaceSet, SurfaceSource, SurfaceStatus,
    create_instance, create_surface, require_gpu, wgpu,
};
use pin_render::topology::{Rect, SurfaceRole};

/// Render iterations a new window may take to become presentable.
const WARM_UP_ITERS: u64 = 5_000_000;
/// Bound on polls of the readback ring / the render thread (counts, D8).
const POLLS: u64 = 50_000_000;

const NATIVE: [&str; 4] = ["windows", "cocoa", "x11", "wayland"];

fn skip(msg: &str) {
    if require_gpu() {
        panic!("[profiler_debug_view_window] PIN_REQUIRE_GPU=1 but {msg}");
    }
    println!("[profiler_debug_view_window] SKIP: {msg}");
}

fn source(
    instance: &wgpu::Instance,
    w: &sdl3::video::Window,
    index: usize,
    role: SurfaceRole,
    keep_alive: WindowGuard,
) -> SurfaceSource {
    let (lw, lh) = w.size();
    let (pw, ph) = w.size_in_pixels();
    let mut viewports = ArrayVec::new();
    viewports.push((role, Rect::new(0, 0, lw, lh)));
    SurfaceSource {
        window: index,
        viewports,
        // SAFETY: the keeper owns the windows until the end of the test,
        // after every SurfaceSet / thread using these surfaces is dropped.
        surface: unsafe { create_surface(instance, w) }.expect("create_surface"),
        logical_size: (lw, lh),
        pixel_size: (pw, ph),
        keep_alive: Some(keep_alive),
    }
}

#[test]
fn debug_view_records_gpu_frame_time_and_paints_the_hud() {
    let sdl = match sdl3::init() {
        Ok(s) => s,
        Err(e) => return skip(&format!("SDL_Init: {e}")),
    };
    let video = match sdl.video() {
        Ok(v) => v,
        Err(e) => return skip(&format!("SDL video: {e}")),
    };
    let driver = video.current_video_driver();
    if !NATIVE.contains(&driver) {
        return skip(&format!(
            "SDL video driver {driver:?} has no native windows"
        ));
    }
    let mut keeper = WindowKeeper::new();
    let g_playfield = keeper.adopt({
        let mut w = video
            .window("pin_app debug view profiler", 640, 200)
            .position(64, 64)
            .build()
            .expect("window");
        w.raise();
        w
    });
    let playfield = keeper.windows().next().expect("playfield");
    let mut pump = sdl.event_pump().expect("pump");
    let opts = GpuOptions::from_env();
    let mut lines = DebugLines::new(64);
    box_lines(
        [0.1, 0.1, 0.0],
        [0.9, 0.9, 0.0],
        [0.0, 1.0, 0.0, 1.0],
        &mut lines,
    );
    let camera = Camera::ortho_top_down([0.0, 0.0], [1.0, 1.0]);

    // Part 1: the render thread's debug-view hooks on a real SurfaceSet.
    let timed = {
        let instance = create_instance(opts.backends);
        let sources = vec![source(
            &instance,
            playfield,
            0,
            SurfaceRole::Playfield,
            g_playfield.clone(),
        )];
        let ctx = match GpuContext::with_instance(instance, &opts, Some(&sources[0].surface)) {
            Ok(c) => c,
            Err(e) => return skip(&e.to_string()),
        };
        let mut set =
            SurfaceSet::new(&ctx, sources, SurfaceOptions::default()).expect("SurfaceSet");
        let supported = GpuFrameTimer::supported(&ctx);
        let mut p = Profiler::new();
        p.sim.record_tick_ns(150_000);
        let mut hooks = debug_view_hooks(&ctx, &set, Some(p.present), true)
            .expect("profiler and HUD configured: hooks");
        assert_eq!(hooks.gpu_timer().is_some(), supported);
        assert!(hooks.hud().is_some(), "the playfield format takes the HUD");
        // The HUD text as the render loop feeds it.
        let mut feed = HudFeed::new(p.reader.clone(), 250_000_000);
        let text = feed.refresh(0).expect("first refresh");
        assert_eq!(text, hud_lines(&p.reader.snapshot()));
        hooks.set_hud_lines(&text);
        println!(
            "[profiler_debug_view_window] adapter {:?} on {driver}; TIMESTAMP_QUERY {supported}; \
             playfield format {:?}",
            ctx.adapter_info().name,
            set.formats().next()
        );
        let mut presented = 0u64;
        let mut iters = 0u64;
        while presented < 12 && iters < WARM_UP_ITERS {
            while pump.poll_event().is_some() {}
            let st = set.render_with(&lines, &camera, &mut hooks);
            presented += u64::from(st[0] == SurfaceStatus::Presented);
            if !set.frame_paced(&st) {
                std::thread::yield_now();
            }
            iters += 1;
        }
        assert_eq!(presented, 12, "playfield never presented 12 frames");
        assert_eq!(
            hooks.hud_paints(),
            presented,
            "the HUD is painted on every presented playfield frame"
        );
        let rect = hooks.hud().expect("HUD").rect_px();
        assert!(rect[0] < rect[2] && rect[1] < rect[3], "HUD rect {rect:?}");
        if supported {
            println!("[profiler_debug_view_window] branch: TIMESTAMP_QUERY; expecting samples");
            // Drain: each render_with collects finished samples first.
            let mut polls = 0u64;
            while p.reader.snapshot().gpu_frame.count == 0 && polls < POLLS {
                while pump.poll_event().is_some() {}
                let st = set.render_with(&lines, &camera, &mut hooks);
                if !set.frame_paced(&st) {
                    std::thread::yield_now();
                }
                polls += 1;
            }
            let snap = p.reader.snapshot();
            println!(
                "[profiler_debug_view_window] {} samples after {polls} extra frames: {}",
                snap.gpu_frame.count,
                hud_lines(&snap)[2]
            );
            assert!(snap.gpu_frame.count > 0, "no GPU frame sample recorded");
            assert!(snap.gpu_frame.min > 0, "a 0 ns sample was recorded");
        } else {
            println!(
                "[profiler_debug_view_window] branch: no TIMESTAMP_QUERY; expecting no samples"
            );
            assert_eq!(p.reader.snapshot().gpu_frame.count, 0);
        }
        // No profiler and no HUD: no hooks (plain `render`).
        assert!(debug_view_hooks(&ctx, &set, None, false).is_none());
        drop(hooks);
        drop(set);
        supported
    };

    // Part 2: the render thread's debug view (no table) with the profiler
    // and the HUD on.
    let instance = create_instance(opts.backends);
    let sources = vec![source(
        &instance,
        playfield,
        0,
        SurfaceRole::Playfield,
        g_playfield.clone(),
    )];
    let p = Profiler::new();
    let reader = p.reader.clone();
    let (_tx, rx) = pin_core::spsc::channel(16);
    let mut cfg = ThreadsConfig::idle(rx);
    cfg.render = RenderConfig {
        gpu: Some(RenderGpu {
            instance,
            options: opts,
            surfaces: sources,
            surface_options: SurfaceOptions::default(),
        }),
        present_profiler: Some(p.present),
        hud: Some(p.reader.clone()),
        ..RenderConfig::default()
    };
    assert!(cfg.render.table.is_none(), "no table: the debug view");
    let threads = spawn_threads(cfg).expect("spawn");
    for _ in 0..POLLS {
        let s = threads.stats();
        let sampled = reader.snapshot().gpu_frame.count > 0;
        if s.gpu_error || (s.render_frames >= 10 && (sampled || !timed)) {
            break;
        }
        while pump.poll_event().is_some() {}
        std::thread::yield_now();
    }
    let s = threads.stats();
    let snap = reader.snapshot();
    println!(
        "[profiler_debug_view_window] render thread stats {s:?}; {}",
        hud_lines(&snap)[2]
    );
    assert!(s.gpu_ready && !s.gpu_error, "{s:?}");
    assert!(s.render_frames >= 10, "{s:?}");
    if timed {
        assert!(
            snap.gpu_frame.count > 0,
            "the debug view recorded no GPU frame time"
        );
    } else {
        assert_eq!(snap.gpu_frame.count, 0);
    }
    threads.shutdown().expect("shutdown");
    drop(g_playfield);
    keeper.drain();
    println!("[profiler_debug_view_window] ok");
}
