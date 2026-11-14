//! Surface smoke test for `pin_render::gpu::SurfaceSet` (gpu-tests): SDL
//! windows created on the main thread, surfaces from them, the §2.2 present
//! policy with the playfield on `Fifo`, a secondary surface forced onto a
//! presenter thread, resize/expose/minimise handling, and shutdown (dropping
//! the set joins the presenter thread).
//!
//! Needs a native SDL video driver (x11/wayland/windows/cocoa; Xvfb on Linux
//! CI). Without one it prints SKIP, or fails when `PIN_REQUIRE_GPU=1`.
//! sdl3's raw-window-handle impl panics on the dummy driver (DECISIONS 0.1
//! Q4), so the dummy driver is never used here.
//!
//! The main thread pumps SDL events between frames, as the app does. On
//! macOS this is what makes a new window visible to the swap chain: wgpu
//! reports a window as occluded until AppKit, on the main thread's event
//! loop, has delivered its first occlusion-state change. So each round first
//! renders (bounded) until both windows have presented once, then counts.

use arrayvec::ArrayVec;
use pin_render::debug::{Camera, DebugLines, box_lines};
use pin_render::gpu::{
    GpuContext, GpuOptions, PresentChoice, SurfaceOptions, SurfaceSet, SurfaceSource,
    SurfaceStatus, choose_present_mode, create_instance, create_surface, require_gpu, wgpu,
};
use pin_render::topology::{Rect, SurfaceRole, WindowEvent, WindowEventKind};
use std::time::{Duration, Instant};

/// How long a new window may take to become visible (presentable).
const WARM_UP: Duration = Duration::from_secs(15);

/// Renders until the playfield and the secondary surface have each
/// presented once (a presenter-thread surface counts once its thread
/// presented), pumping SDL events in between and sleeping briefly whenever
/// the frame was not paced (as the render thread does). Panics with the
/// statuses seen if that does not happen within [`WARM_UP`].
fn warm_up(set: &mut SurfaceSet, pump: &mut sdl3::EventPump, lines: &DebugLines, camera: &Camera) {
    let presenter = set.choices().any(|c| c.2.needs_presenter_thread());
    let t0 = Instant::now();
    let mut seen: Vec<[SurfaceStatus; 2]> = Vec::new();
    let (mut playfield, mut secondary) = (false, false);
    while t0.elapsed() < WARM_UP {
        while pump.poll_event().is_some() {}
        let st = set.render(lines, camera);
        playfield |= st[0] == SurfaceStatus::Presented;
        secondary |= if presenter {
            set.presenter_thread_frames() >= 1
        } else {
            st[1] == SurfaceStatus::Presented
        };
        if seen.last() != Some(&[st[0], st[1]]) {
            seen.push([st[0], st[1]]);
        }
        if playfield && secondary {
            println!(
                "[gpu_window] windows presentable after {:?} (status changes {seen:?})",
                t0.elapsed()
            );
            return;
        }
        if !set.frame_paced(&st) {
            std::thread::sleep(Duration::from_millis(5));
        }
    }
    panic!(
        "windows not presentable within {WARM_UP:?}: playfield {playfield}, secondary \
         {secondary}; status changes {seen:?}"
    );
}

const NATIVE: [&str; 4] = ["windows", "cocoa", "x11", "wayland"];

fn skip(msg: &str) {
    if require_gpu() {
        panic!("[gpu_window] PIN_REQUIRE_GPU=1 but {msg}");
    }
    println!("[gpu_window] SKIP: {msg}");
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
        // A backglass PiP placeholder inside the playfield window.
        viewports.push((SurfaceRole::Backglass, Rect::new(0, 0, lw / 2, lw * 3 / 8)));
    }
    SurfaceSource {
        window: index,
        viewports,
        // SAFETY: the windows outlive every SurfaceSet built from these
        // surfaces (they are dropped at the end of `main`).
        surface: unsafe { create_surface(instance, w) }.expect("create_surface"),
        logical_size: (lw, lh),
        pixel_size: (pw, ph),
    }
}

fn ev(window: usize, kind: WindowEventKind) -> WindowEvent {
    WindowEvent {
        timestamp_ns: 0,
        window,
        kind,
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
        return skip(&format!(
            "SDL video driver {driver:?} has no native windows"
        ));
    }
    // Side by side, so neither covers the other, and raised.
    let mk = |title: &str, x: i32, w: u32, h: u32| {
        let mut win = video
            .window(title, w, h)
            .position(x, 64)
            .build()
            .expect("window");
        win.raise();
        win
    };
    let playfield = mk("pin_render test playfield", 64, 160, 240);
    let backglass = mk("pin_render test backglass", 288, 128, 96);
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

    for force_presenter_thread in [true, false] {
        let instance = create_instance(opts.backends);
        let sources = vec![
            source(&instance, &playfield, 0, SurfaceRole::Playfield),
            source(&instance, &backglass, 1, SurfaceRole::Backglass),
        ];
        let ctx = match GpuContext::with_instance(instance, &opts, Some(&sources[0].surface)) {
            Ok(c) => c,
            Err(e) => return skip(&e.to_string()),
        };
        let offered = sources[1]
            .surface
            .get_capabilities(ctx.adapter())
            .present_modes;
        println!(
            "[gpu_window] adapter {:?} on {driver}, offered {offered:?}, force_presenter_thread {force_presenter_thread}",
            ctx.adapter_info().name
        );
        let mut set = SurfaceSet::new(
            &ctx,
            sources,
            SurfaceOptions {
                vrr: false,
                force_presenter_thread,
            },
        )
        .expect("SurfaceSet");
        assert_eq!(set.len(), 2);
        assert!(!set.is_empty());
        let choices: Vec<_> = set.choices().collect();
        println!("[gpu_window] choices {choices:?}");
        assert_eq!(
            choices[0],
            (
                0,
                SurfaceRole::Playfield,
                PresentChoice::Direct(wgpu::PresentMode::Fifo)
            )
        );
        let want_bg = if force_presenter_thread {
            PresentChoice::PresenterThread(wgpu::PresentMode::Fifo)
        } else {
            choose_present_mode(SurfaceRole::Backglass, &offered, false)
        };
        assert_eq!(choices[1], (1, SurfaceRole::Backglass, want_bg));
        let draw_formats: Vec<_> = set.formats().collect();
        if force_presenter_thread {
            assert_eq!(draw_formats[1], wgpu::TextureFormat::Rgba8Unorm);
        }

        warm_up(&mut set, &mut pump, &lines, &camera);
        let mut presented = [0u32; 2];
        let mut unchanged = 0;
        for frame in 0..40 {
            while pump.poll_event().is_some() {}
            match frame {
                10 => {
                    set.request_redraw(SurfaceRole::Backglass);
                    set.handle_window_event(&ev(
                        0,
                        WindowEventKind::PixelSizeChanged { w: 150, h: 230 },
                    ));
                    set.handle_window_event(&ev(0, WindowEventKind::Resized { w: 150, h: 230 }));
                }
                20 => {
                    set.handle_window_event(&ev(
                        1,
                        WindowEventKind::PixelSizeChanged { w: 120, h: 90 },
                    ));
                    set.handle_window_event(&ev(7, WindowEventKind::Exposed)); // unknown window: ignored
                }
                25 => set.handle_window_event(&ev(1, WindowEventKind::Minimized)),
                30 => set.handle_window_event(&ev(1, WindowEventKind::Restored)),
                _ => {}
            }
            let st = set.render(&lines, &camera);
            assert_eq!(st.len(), 2);
            if (26..30).contains(&frame) {
                assert_eq!(st[1], SurfaceStatus::Hidden);
            }
            for (i, s) in st.iter().enumerate() {
                if *s == SurfaceStatus::Presented {
                    presented[i] += 1;
                }
            }
            if st[1] == SurfaceStatus::Unchanged {
                unchanged += 1;
            }
            assert_eq!(set.frame_paced(&st), st[0] == SurfaceStatus::Presented);
            if !set.frame_paced(&st) {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
        // Let a pending presenter hand-off finish, then one more redraw.
        let t0 = Instant::now();
        while set.presenter_thread_frames() < u64::from(presented[1])
            && t0.elapsed() < Duration::from_secs(5)
        {
            std::thread::yield_now();
        }
        println!(
            "[gpu_window] presented playfield {} backglass {} (presenter thread frames {}), unchanged {unchanged}",
            presented[0],
            presented[1],
            set.presenter_thread_frames()
        );
        assert!(
            presented[0] >= 30,
            "playfield presented {} of 40",
            presented[0]
        );
        // Redraw on change only (the initial frame was in the warm-up):
        // request+resize, resize, restore (+ retries while a hand-off was
        // busy).
        assert!(
            (2..=8).contains(&presented[1]),
            "backglass presented {}",
            presented[1]
        );
        assert!(
            unchanged >= 20,
            "secondary should mostly be unchanged: {unchanged}"
        );
        if want_bg.needs_presenter_thread() {
            assert!(set.presenter_thread_frames() >= 1);
        } else {
            assert_eq!(set.presenter_thread_frames(), 0);
        }
        // Minimising the playfield: hidden, not paced (the caller sleeps).
        set.handle_window_event(&ev(0, WindowEventKind::Minimized));
        let st = set.render(&lines, &camera);
        assert_eq!(st[0], SurfaceStatus::Hidden);
        assert!(!set.frame_paced(&st));
        let t1 = Instant::now();
        drop(set); // joins the presenter thread
        assert!(
            t1.elapsed() < Duration::from_secs(1),
            "drop took {:?}",
            t1.elapsed()
        );
    }
    drop(backglass);
    drop(playfield);
    println!("[gpu_window] ok");
}
