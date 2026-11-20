//! w8g-d: the per-frame hooks of `SurfaceSet::render_with` on real windows
//! (gpu-tests; SDL windows under Xvfb on Linux CI, pattern of
//! `gpu_window.rs` / pin_app's `orchestrator_window.rs`).
//!
//! A recording hook sees, per `render_with` call: `begin_frame` once (before
//! anything else) and `end_frame` once (after everything else);
//! `wrap_submit` once per drawn surface (a presented surface, direct or
//! handed to its presenter thread); `overlay` only for the playfield and
//! only when it is drawn, before its `wrap_submit`. Surfaces that are not
//! drawn (unchanged, hidden, busy, occluded, skipped) get neither. And
//! `render_with(NoFrameHooks)` behaves like `render`.
//!
//! This is a `harness = false` target (pattern of `gpu_window.rs`): `main`
//! runs on the process main thread, which owns SDL (required on macOS), and
//! SDL is initialised once per process. Without a native video driver it
//! reports the target as skipped in libtest's `ignored` form, or fails when
//! `PIN_REQUIRE_GPU=1`; the dummy driver is never
//! used (sdl3's raw-window-handle impl panics on it, DECISIONS 0.1 Q4).
//! The target accepts libtest filters and `--list` through `pin_testmain`.
#![cfg(feature = "gpu-tests")]

use arrayvec::ArrayVec;
use pin_render::debug::{Camera, DebugLines, box_lines};
use pin_render::gpu::{
    GpuContext, GpuOptions, NoFrameHooks, SurfaceFrameHooks, SurfaceOptions, SurfaceSet,
    SurfaceSource, SurfaceStatus, create_instance, create_surface, require_gpu, wgpu,
};
use pin_render::topology::{Rect, SurfaceRole, WindowEvent, WindowEventKind};

/// Paced (vsync-limited) playfield frames a new window may take to become
/// presentable. Counts, not durations (D8): a presenting playfield that
/// never lets the secondary acquire fails after this many paced frames
/// instead of looping for hours.
const MAX_PACED_FRAMES: u32 = 600;
/// Unpaced render attempts (those that `yield_now`) allowed on top of the
/// paced frames while waiting for the windows to become presentable.
const MAX_UNPACED_ATTEMPTS: u32 = 200_000;

const NATIVE: [&str; 4] = ["windows", "cocoa", "x11", "wayland"];

fn skip(msg: &str) -> pin_testmain::Outcome {
    if require_gpu() {
        panic!("[gpu_surface_hooks_window] PIN_REQUIRE_GPU=1 but {msg}");
    }
    pin_testmain::Outcome::Skipped(format!("[gpu_surface_hooks_window] {msg}"))
}

/// One hook event, in call order.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
enum Ev {
    Begin,
    Wrap,
    Overlay(SurfaceRole, (u32, u32)),
    End,
}

/// Records every call; its `end_frame` submits one empty command buffer
/// through `submit` (the set's locked submit).
#[derive(Default)]
struct Recorder {
    events: Vec<Ev>,
    device: Option<wgpu::Device>,
}

impl SurfaceFrameHooks for Recorder {
    fn begin_frame(&mut self) {
        self.events.push(Ev::Begin);
    }

    fn wrap_submit(&mut self, frame: wgpu::CommandBuffer) -> ArrayVec<wgpu::CommandBuffer, 3> {
        self.events.push(Ev::Wrap);
        let mut v = ArrayVec::new();
        v.push(frame);
        v
    }

    fn overlay(
        &mut self,
        role: SurfaceRole,
        _queue: &wgpu::Queue,
        _encoder: &mut wgpu::CommandEncoder,
        _view: &wgpu::TextureView,
        size_px: (u32, u32),
    ) {
        self.events.push(Ev::Overlay(role, size_px));
    }

    fn end_frame(&mut self, submit: &mut dyn FnMut(wgpu::CommandBuffer)) {
        self.events.push(Ev::End);
        if let Some(d) = &self.device {
            submit(
                d.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None })
                    .finish(),
            );
        }
    }
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
        viewports.push((SurfaceRole::Backglass, Rect::new(0, 0, lw / 2, lw * 3 / 8)));
    }
    SurfaceSource {
        window: index,
        viewports,
        // SAFETY: the windows outlive every SurfaceSet built from these
        // surfaces (they are dropped at the end of the test).
        surface: unsafe { create_surface(instance, w) }.expect("create_surface"),
        logical_size: (lw, lh),
        pixel_size: (pw, ph),
        keep_alive: None,
    }
}

/// Renders (plain `render`) until the playfield and the secondary surface
/// have each been presented once.
fn warm_up(set: &mut SurfaceSet, pump: &mut sdl3::EventPump, lines: &DebugLines, camera: &Camera) {
    let presenter = set.choices().any(|c| c.2.needs_presenter_thread());
    let (mut playfield, mut secondary) = (false, false);
    let (mut paced, mut unpaced) = (0u32, 0u32);
    while paced < MAX_PACED_FRAMES && unpaced < MAX_UNPACED_ATTEMPTS {
        while pump.poll_event().is_some() {}
        let st = set.render(lines, camera);
        playfield |= st[0] == SurfaceStatus::Presented;
        secondary |= if presenter {
            set.presenter_thread_frames() >= 1
        } else {
            st[1] == SurfaceStatus::Presented
        };
        if playfield && secondary {
            return;
        }
        if set.frame_paced(&st) {
            paced += 1;
        } else {
            unpaced += 1;
            std::thread::yield_now();
        }
    }
    panic!(
        "windows not presentable: playfield {playfield}, secondary {secondary} \
         (after {paced} paced frames, {unpaced} unpaced attempts)"
    );
}

/// Checks one `render_with` call's events against its statuses.
fn check_frame(ev: &[Ev], st: &[SurfaceStatus], playfield_px: (u32, u32), threaded: bool) {
    assert_eq!(ev.first(), Some(&Ev::Begin), "{ev:?}");
    assert_eq!(ev.last(), Some(&Ev::End), "{ev:?}");
    assert_eq!(ev.iter().filter(|e| **e == Ev::Begin).count(), 1, "{ev:?}");
    assert_eq!(ev.iter().filter(|e| **e == Ev::End).count(), 1, "{ev:?}");
    let wraps = ev.iter().filter(|e| **e == Ev::Wrap).count();
    let presented = st
        .iter()
        .filter(|s| **s == SurfaceStatus::Presented)
        .count();
    if threaded {
        // A hand-off refused after the submit (a full mailbox) reports Busy.
        let busy = usize::from(st[1] == SurfaceStatus::Busy);
        assert!(
            (presented..=presented + busy).contains(&wraps),
            "{ev:?} {st:?}"
        );
    } else {
        assert_eq!(wraps, presented, "{ev:?} {st:?}");
    }
    let overlays: Vec<_> = ev
        .iter()
        .filter_map(|e| match e {
            Ev::Overlay(r, s) => Some((*r, *s)),
            _ => None,
        })
        .collect();
    if st[0] == SurfaceStatus::Presented {
        assert_eq!(
            overlays,
            [(SurfaceRole::Playfield, playfield_px)],
            "{ev:?} {st:?}"
        );
        // The playfield's overlay is recorded before its frame is submitted.
        let o = ev
            .iter()
            .position(|e| matches!(e, Ev::Overlay(..)))
            .unwrap();
        assert_eq!(ev[o + 1], Ev::Wrap, "{ev:?}");
    } else {
        assert!(overlays.is_empty(), "{ev:?} {st:?}");
    }
}

fn main() {
    pin_testmain::libtest_main("gpu_surface_hooks_window", run)
}

fn run() -> pin_testmain::Outcome {
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
    let mk = |title: &str, x: i32, w: u32, h: u32| {
        let mut win = video
            .window(title, w, h)
            .position(x, 64)
            .build()
            .expect("window");
        win.raise();
        win
    };
    let playfield = mk("pin_render hooks playfield", 64, 160, 240);
    let backglass = mk("pin_render hooks backglass", 288, 128, 96);
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

    for force_presenter_thread in [false, true] {
        let instance = create_instance(opts.backends);
        let sources = vec![
            source(&instance, &playfield, 0, SurfaceRole::Playfield),
            source(&instance, &backglass, 1, SurfaceRole::Backglass),
        ];
        let pf_px = sources[0].pixel_size;
        let ctx = match GpuContext::with_instance(instance, &opts, Some(&sources[0].surface)) {
            Ok(c) => c,
            Err(e) => return skip(&e.to_string()),
        };
        let mut set = SurfaceSet::new(
            &ctx,
            sources,
            SurfaceOptions {
                vrr: false,
                force_presenter_thread,
            },
        )
        .expect("SurfaceSet");
        let threaded = set.choices().any(|c| c.2.needs_presenter_thread());
        println!(
            "[gpu_surface_hooks_window] {:?} on {driver}; force_presenter_thread \
             {force_presenter_thread}; choices {:?}",
            ctx.adapter_info().name,
            set.choices().collect::<Vec<_>>()
        );
        warm_up(&mut set, &mut pump, &lines, &camera);

        // The recording hook over 30 frames; the secondary surface is
        // redrawn on request at frames 10 and 20.
        let mut rec = Recorder {
            events: Vec::new(),
            device: Some(ctx.device().clone()),
        };
        let mut drawn = [0usize; 2];
        let mut overlay_frames = 0usize;
        for frame in 0..30 {
            while pump.poll_event().is_some() {}
            if frame == 10 || frame == 20 {
                set.request_redraw(SurfaceRole::Backglass);
            }
            rec.events.clear();
            let st = set.render_with(&lines, &camera, &mut rec);
            assert_eq!(st.len(), 2);
            check_frame(&rec.events, &st, pf_px, threaded);
            for (i, s) in st.iter().enumerate() {
                drawn[i] += usize::from(*s == SurfaceStatus::Presented);
            }
            overlay_frames += usize::from(rec.events.iter().any(|e| matches!(e, Ev::Overlay(..))));
            if !set.frame_paced(&st) {
                std::thread::yield_now();
            }
        }
        println!(
            "[gpu_surface_hooks_window] drawn playfield {} backglass {}, overlays {overlay_frames}",
            drawn[0], drawn[1]
        );
        assert!(drawn[0] >= 20, "playfield drawn {} of 30", drawn[0]);
        assert_eq!(overlay_frames, drawn[0]);
        assert!(drawn[1] >= 1, "the requested secondary redraws never ran");

        // A hidden playfield: begin/end only, no overlay, no wrap.
        set.handle_window_event(&WindowEvent {
            timestamp_ns: 0,
            window: 0,
            kind: WindowEventKind::Minimized,
        });
        rec.events.clear();
        let st = set.render_with(&lines, &camera, &mut rec);
        assert_eq!(st[0], SurfaceStatus::Hidden);
        assert_ne!(st[1], SurfaceStatus::Presented);
        assert!(
            rec.events == [Ev::Begin, Ev::End] || threaded,
            "{:?}",
            rec.events
        );
        check_frame(&rec.events, &st, pf_px, threaded);
        set.handle_window_event(&WindowEvent {
            timestamp_ns: 0,
            window: 0,
            kind: WindowEventKind::Restored,
        });

        // Smoke: `render_with(NoFrameHooks)` behaves like `render`. In the
        // steady state both present the playfield and leave the unchanged
        // secondary alone.
        let mut plain = Vec::new();
        let mut hooked = Vec::new();
        for _ in 0..20 {
            while pump.poll_event().is_some() {}
            let a = set.render(&lines, &camera);
            let b = set.render_with(&lines, &camera, &mut NoFrameHooks);
            for st in [&a, &b] {
                if !set.frame_paced(st) {
                    std::thread::yield_now();
                }
            }
            plain.push(a);
            hooked.push(b);
        }
        let steady = [SurfaceStatus::Presented, SurfaceStatus::Unchanged];
        let n_plain = plain.iter().filter(|s| s.as_slice() == steady).count();
        let n_hooked = hooked.iter().filter(|s| s.as_slice() == steady).count();
        println!(
            "[gpu_surface_hooks_window] steady frames: render {n_plain}/20, \
             render_with(NoFrameHooks) {n_hooked}/20"
        );
        assert!(n_plain >= 12 && n_hooked >= 12, "{plain:?} / {hooked:?}");
        drop(set);
    }
    drop(backglass);
    drop(playfield);
    println!("[gpu_surface_hooks_window] ok");
    pin_testmain::Outcome::Passed
}
