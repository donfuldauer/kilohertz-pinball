//! Smoke test of the real presentation path (Subphase 1.4): SDL windows on
//! the main thread, wgpu surfaces created on main and moved to the render
//! side, the §2.2 present policy, a presenter thread for a `Fifo`-only
//! secondary surface (forced), and the orchestrator's render thread drawing
//! into a window.
//!
//! Needs a native SDL video driver (x11/wayland/windows/cocoa): under
//! `xvfb-run` on Linux CI. Without one it prints SKIP, or fails when
//! `PIN_REQUIRE_GPU=1`. sdl3's raw-window-handle impl panics on the dummy
//! driver (DECISIONS 0.1 Q4), so the dummy driver is never used here.

use arrayvec::ArrayVec;
use pin_app::orchestrator::{RenderConfig, RenderGpu, ThreadsConfig, spawn_threads};
use pin_render::debug::{Camera, DebugLines, box_lines};
use pin_render::gpu::{
    GpuContext, GpuOptions, PresentChoice, SurfaceOptions, SurfaceSet, SurfaceSource,
    SurfaceStatus, create_instance, create_surface, require_gpu, wgpu,
};
use pin_render::topology::{Rect, SurfaceRole, WindowEvent, WindowEventKind};
use std::time::{Duration, Instant};

const NATIVE: [&str; 4] = ["windows", "cocoa", "x11", "wayland"];

fn skip(msg: &str) {
    if require_gpu() {
        panic!("[orchestrator_window] PIN_REQUIRE_GPU=1 but {msg}");
    }
    println!("[orchestrator_window] SKIP: {msg}");
}

fn source(
    instance: &wgpu::Instance,
    w: &sdl3::video::Window,
    index: usize,
    role: SurfaceRole,
) -> SurfaceSource {
    let (lw, lh) = w.size();
    let (pw, ph) = w.size_in_pixels();
    let mut viewports = ArrayVec::new();
    viewports.push((role, Rect::new(0, 0, lw, lh)));
    if role == SurfaceRole::Playfield {
        viewports.push((SurfaceRole::Score, Rect::new(0, 0, lw / 2, lw / 8)));
    }
    SurfaceSource {
        window: index,
        viewports,
        // SAFETY: the windows live until the end of `main`, after every
        // SurfaceSet / thread using these surfaces is dropped.
        surface: unsafe { create_surface(instance, w) }.expect("create_surface"),
        logical_size: (lw, lh),
        pixel_size: (pw, ph),
    }
}

fn main() {
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
        return skip(&format!("SDL video driver {driver:?} has no native windows"));
    }
    let mk = |title: &str, w: u32, h: u32| video.window(title, w, h).build().expect("window");
    let playfield = mk("pin_app test playfield", 160, 240);
    let score = mk("pin_app test score", 128, 32);

    let opts = GpuOptions::from_env();
    let mut lines = DebugLines::new(64);
    box_lines([0.1, 0.1, 0.0], [0.9, 0.9, 0.0], [0.0, 1.0, 0.0, 1.0], &mut lines);
    let camera = Camera::ortho_top_down([0.0, 0.0], [1.0, 1.0]);

    // Part 1: SurfaceSet directly, secondary forced onto a presenter thread.
    {
        let instance = create_instance(opts.backends);
        let sources = vec![
            source(&instance, &playfield, 0, SurfaceRole::Playfield),
            source(&instance, &score, 1, SurfaceRole::Score),
        ];
        let ctx = match GpuContext::with_instance(instance, &opts, Some(&sources[0].surface)) {
            Ok(c) => c,
            Err(e) => return skip(&e.to_string()),
        };
        println!("[orchestrator_window] adapter {:?} on {driver}", ctx.adapter_info().name);
        let offered = sources[1].surface.get_capabilities(ctx.adapter()).present_modes;
        println!("[orchestrator_window] offered present modes {offered:?}");
        let mut set = SurfaceSet::new(
            &ctx,
            sources,
            SurfaceOptions {
                vrr: false,
                force_presenter_thread: true,
            },
        )
        .expect("SurfaceSet");
        let choices: Vec<_> = set.choices().collect();
        println!("[orchestrator_window] choices {choices:?}");
        assert_eq!(choices[0].2, PresentChoice::Direct(wgpu::PresentMode::Fifo));
        assert_eq!(choices[1].2, PresentChoice::PresenterThread(wgpu::PresentMode::Fifo));
        assert!(set.paces_frame());
        let mut presented = [0u32; 2];
        let mut statuses = Vec::new();
        for frame in 0..30 {
            if frame == 10 {
                set.request_redraw(SurfaceRole::Score);
                set.handle_window_event(&WindowEvent {
                    timestamp_ns: 0,
                    window: 0,
                    kind: WindowEventKind::PixelSizeChanged { w: 150, h: 230 },
                });
            }
            let st = set.render(&lines, &camera);
            for (i, s) in st.iter().enumerate() {
                if *s == SurfaceStatus::Presented {
                    presented[i] += 1;
                }
            }
            statuses.push(st);
        }
        println!(
            "[orchestrator_window] presented playfield {} score {} (presenter thread frames {})",
            presented[0],
            presented[1],
            set.presenter_thread_frames()
        );
        assert!(presented[0] >= 20, "playfield presented {} of 30", presented[0]);
        // The secondary surface redraws only on change: first frame + the
        // request at frame 10 (+ retries while busy).
        assert!((1..=4).contains(&presented[1]), "score presented {}", presented[1]);
        assert!(statuses.iter().any(|s| s[1] == SurfaceStatus::Unchanged));
        drop(set); // joins the presenter thread
    }

    // Part 2: the orchestrator's render thread presenting the playfield.
    let instance = create_instance(opts.backends);
    let sources = vec![source(&instance, &playfield, 0, SurfaceRole::Playfield)];
    let (_tx, rx) = pin_core::spsc::channel(16);
    let mut cfg = ThreadsConfig::idle(rx);
    cfg.render = RenderConfig {
        gpu: Some(RenderGpu {
            instance,
            options: opts,
            surfaces: sources,
            surface_options: SurfaceOptions::default(),
        }),
        ..RenderConfig::default()
    };
    let threads = spawn_threads(cfg).expect("spawn");
    let t0 = Instant::now();
    while t0.elapsed() < Duration::from_secs(20) {
        let s = threads.stats();
        if s.gpu_error || s.render_frames >= 10 {
            break;
        }
        // Keep the windows serviced (main thread = SDL pump).
        let mut pump = sdl.event_pump().expect("pump");
        while pump.poll_event().is_some() {}
        drop(pump);
        std::thread::yield_now();
    }
    let s = threads.stats();
    println!("[orchestrator_window] render thread stats {s:?}");
    assert!(s.gpu_ready && !s.gpu_error, "{s:?}");
    assert!(s.render_frames >= 10, "{s:?}");
    let t1 = Instant::now();
    threads.shutdown().expect("shutdown");
    assert!(t1.elapsed() < Duration::from_secs(1), "shutdown took {:?}", t1.elapsed());
    drop(score);
    drop(playfield);
    println!("[orchestrator_window] ok");
}
