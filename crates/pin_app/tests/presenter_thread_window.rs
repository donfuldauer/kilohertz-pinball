//! PR #21 round 4 (CodeRabbit thread 4577423193, D19, issue #9): the table
//! path's presenter-thread hand-off end to end on real windows.
//!
//! Three SDL windows (playfield, backglass, score) get wgpu surfaces and an
//! [`AppSurfaces`] with `force_presenter_thread`, so the backglass and score
//! surfaces take the presenter-thread path whatever present modes the
//! adapter offers (the path a `Fifo`-only secondary display takes). The
//! test renders frames with a snapshot and display commands and checks
//! that: the playfield is presented directly every iteration; the secondary
//! surfaces are only ever handed off (`Presented`) or skipped as `Busy`,
//! never acquired by the render thread; every renderer, including the
//! presenter-thread ones, receives every display command; the presenter
//! threads present frames on their surfaces (counters, polled with an
//! iteration bound, D8); and dropping `AppSurfaces` joins every presenter
//! thread, which releases its window guard.
//!
//! GPU job only (`--features gpu-tests`): without a native SDL video driver
//! or a GPU adapter it FAILS, it never skips. SDL needs the main thread,
//! so this is a plain `main` (harness = false).

mod support;

use arrayvec::ArrayVec;
use pin_app::orchestrator::{
    AppSurfaces, DisplayCommand, SlotPath, TableRenderOptions, TableVisuals, load_checked,
    validate_table,
};
use pin_app::platform::{WindowGuard, WindowKeeper};
use pin_core::snapshot::FrameStateSnapshot;
use pin_render::gpu::{
    GpuContext, GpuOptions, SurfaceOptions, SurfaceSource, SurfaceStatus, create_instance,
    create_surface, wgpu,
};
use pin_render::topology::{Rect, SurfaceRole};
use pin_sim::SimConfig;
use support::{greybox_bundle, on_big_stack};

/// Render iterations the windows may take to become presentable and the
/// presenter threads to present once (a count, not a duration: D8).
const WARM_UP_ITERS: u64 = 5_000_000;

/// Iterations of the checked run after the warm-up.
const RUN_ITERS: u64 = 60;

const NATIVE: [&str; 4] = ["windows", "cocoa", "x11", "wayland"];

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
        // SAFETY: every window outlives its surface: the keeper destroys a
        // window only after the last guard (one goes with each surface,
        // also into a presenter thread) is dropped.
        surface: unsafe { create_surface(instance, w) }.expect("create_surface"),
        logical_size: (lw, lh),
        pixel_size: (pw, ph),
        keep_alive: Some(keep_alive),
    }
}

/// The grey-box table's visuals, load-time snapshot and two scene ids.
fn table() -> (TableVisuals, FrameStateSnapshot, [pin_core::SceneId; 2]) {
    let dir = greybox_bundle().to_path_buf();
    on_big_stack(move || {
        let checked = validate_table(&dir).expect("greybox validates");
        let sim = load_checked(&checked, SimConfig::default()).expect("greybox loads");
        let ids = &checked.table.ids;
        let scenes = [
            ids.scene("scene_attract").expect("scene_attract"),
            ids.scene("scene_game_over").expect("scene_game_over"),
        ];
        (TableVisuals::of(&checked), *sim.snapshot(), scenes)
    })
}

fn main() {
    let sdl = sdl3::init().expect("SDL_Init (a missing display is a failure here)");
    let video = sdl.video().expect("SDL video");
    let driver = video.current_video_driver();
    assert!(
        NATIVE.contains(&driver),
        "SDL video driver {driver:?} has no native windows (run under xvfb-run on Linux)"
    );
    let mk = |title: &str, x: i32, w: u32, h: u32| {
        let mut win = video
            .window(title, w, h)
            .position(x, 64)
            .build()
            .expect("window");
        win.raise();
        win
    };
    let mut keeper = WindowKeeper::new();
    let guards = [
        keeper.adopt(mk("pin_app presenter test playfield", 32, 160, 240)),
        keeper.adopt(mk("pin_app presenter test backglass", 224, 160, 120)),
        keeper.adopt(mk("pin_app presenter test score", 416, 192, 48)),
    ];
    // The test's own guard references (one per window).
    let baseline: Vec<usize> = guards.iter().map(WindowGuard::strong_count).collect();
    assert_eq!(baseline, [1, 1, 1]);
    let mut pump = sdl.event_pump().expect("pump");
    let (visuals, snap, [attract, over]) = table();

    let opts = GpuOptions::from_env();
    let instance = create_instance(opts.backends);
    let roles = [
        SurfaceRole::Playfield,
        SurfaceRole::Backglass,
        SurfaceRole::Score,
    ];
    let sources: Vec<SurfaceSource> = keeper
        .windows()
        .zip(roles)
        .enumerate()
        .map(|(i, (w, role))| source(&instance, w, i, role, guards[i].clone()))
        .collect();
    let ctx = GpuContext::with_instance(instance, &opts, Some(&sources[0].surface))
        .expect("GPU adapter (a missing adapter is a failure here)");
    println!(
        "[presenter_thread_window] adapter {:?} on {driver}",
        ctx.adapter_info().name
    );
    for (i, s) in sources.iter().enumerate() {
        let caps = s.surface.get_capabilities(ctx.adapter());
        println!(
            "[presenter_thread_window] window {i}: present modes {:?}, usages {:?}",
            caps.present_modes, caps.usages
        );
    }
    let mut surfaces = AppSurfaces::new(
        &ctx,
        sources,
        SurfaceOptions {
            vrr: false,
            force_presenter_thread: true,
        },
        &visuals,
        TableRenderOptions::default(),
    )
    .expect("AppSurfaces");
    let slots: Vec<_> = surfaces.slots().collect();
    println!("[presenter_thread_window] slots {slots:#?}");
    assert_eq!(slots.len(), 3, "every surface is retained");
    assert_eq!(surfaces.plan_counts(), (1, 2, 0));
    assert_eq!(slots[0].role, SurfaceRole::Playfield);
    assert_eq!(slots[0].path, SlotPath::Direct);
    for s in &slots[1..] {
        assert!(
            matches!(s.path, SlotPath::PresenterThread(_)),
            "{s:?}: secondary surface not on a presenter thread"
        );
        assert!(s.presenter.is_some() && !s.dead, "{s:?}");
    }
    // The render thread presents only the playfield itself.
    let direct: Vec<_> = surfaces.drawn_modes().collect();
    assert_eq!(direct.len(), 1);
    assert_eq!(direct[0].0, SurfaceRole::Playfield);

    // Display commands reach every renderer, presenter-thread ones too.
    let cmds = [
        DisplayCommand::Scene(attract),
        DisplayCommand::Score {
            player: 0,
            score: 1230,
        },
    ];
    for c in cmds {
        surfaces.apply_display_command(c, 0);
    }
    for s in surfaces.slots() {
        assert_eq!(s.display_commands, 2, "{s:?}");
    }

    // Warm-up: until the playfield presented and every presenter thread
    // presented a frame on its surface (bounded by an iteration count).
    let mut frame = snap;
    let mut seen: Vec<Vec<SurfaceStatus>> = Vec::new();
    let mut playfield_presented = false;
    let mut warm = None;
    for iter in 0..WARM_UP_ITERS {
        while pump.poll_event().is_some() {}
        frame.tick = 500 + iter;
        let st = surfaces.render(&frame);
        // Structural: the render thread never acquires a presenter-thread
        // surface, so its status is only ever a hand-off or Busy.
        for (i, s) in st.iter().enumerate().skip(1) {
            assert!(
                matches!(s, SurfaceStatus::Presented | SurfaceStatus::Busy),
                "iteration {iter}: presenter-thread surface {i} status {s:?}"
            );
        }
        playfield_presented |= st[0] == SurfaceStatus::Presented;
        if seen.last() != Some(&st) {
            seen.push(st.clone());
        }
        let stats = surfaces.presenter_stats();
        if playfield_presented && stats.iter().all(|(_, _, s)| s.presented >= 1) {
            warm = Some(iter);
            break;
        }
        if !surfaces.frame_paced(&st) {
            std::thread::yield_now();
        }
    }
    let stats = surfaces.presenter_stats();
    let Some(warm) = warm else {
        panic!(
            "presenter threads did not present within {WARM_UP_ITERS} iterations: playfield \
             presented {playfield_presented}, presenter stats {stats:?}, status changes {seen:?}"
        );
    };
    println!(
        "[presenter_thread_window] presentable after {warm} iterations (status changes {seen:?}); \
         stats {stats:?}"
    );

    // The checked run: the playfield presents directly every iteration and
    // paces the loop; secondary surfaces are handed off or Busy.
    let before = surfaces.presenter_stats();
    let mut handed_off = [0u64; 3];
    for iter in 0..RUN_ITERS {
        while pump.poll_event().is_some() {}
        if iter == RUN_ITERS / 2 {
            surfaces.apply_display_command(DisplayCommand::Scene(over), frame.tick);
        }
        frame.tick += 1;
        let st = surfaces.render(&frame);
        assert_eq!(
            st[0],
            SurfaceStatus::Presented,
            "iteration {iter}: playfield not presented: {st:?}"
        );
        assert!(surfaces.frame_paced(&st), "{st:?}");
        assert!(
            !surfaces.frame_paced(&[
                SurfaceStatus::Busy,
                SurfaceStatus::Presented,
                SurfaceStatus::Presented
            ]),
            "a presenter-thread hand-off must never pace the loop"
        );
        for (i, s) in st.iter().enumerate().skip(1) {
            assert!(
                matches!(s, SurfaceStatus::Presented | SurfaceStatus::Busy),
                "iteration {iter}: presenter-thread surface {i} status {s:?}"
            );
            if *s == SurfaceStatus::Presented {
                handed_off[i] += 1;
            }
        }
    }
    for s in surfaces.slots() {
        assert_eq!(s.display_commands, 3, "{s:?}");
    }
    // Bounded poll (iterations, D8) until each presenter took the newest
    // hand-offs: presented frames and posts both advanced.
    let mut after = surfaces.presenter_stats();
    for _ in 0..WARM_UP_ITERS {
        if after
            .iter()
            .zip(&before)
            .all(|(a, b)| a.2.presented > b.2.presented)
        {
            break;
        }
        while pump.poll_event().is_some() {}
        std::thread::yield_now();
        after = surfaces.presenter_stats();
    }
    println!("[presenter_thread_window] handed off {handed_off:?}; stats {after:?}");
    for ((window, role, a), (_, _, b)) in after.iter().zip(&before) {
        assert!(a.posted > b.posted, "window {window} ({role:?}): {a:?}");
        assert!(
            a.presented > b.presented,
            "window {window} ({role:?}): the presenter thread presented nothing in the run: {a:?}"
        );
        assert!(a.presented >= 1 && a.posted > 0, "{a:?}");
        // Every post is accounted for (at most one queued, one in flight).
        let settled = a.presented + a.present_skipped + a.dropped_latest + a.discarded_stale;
        assert!(
            settled <= a.posted && a.posted <= settled + 2,
            "window {window}: {a:?}"
        );
        assert_eq!(a.configure_failures, 0, "window {window}: {a:?}");
    }
    assert!(
        handed_off[1] > 0 && handed_off[2] > 0,
        "handed off {handed_off:?}"
    );
    assert!(surfaces.frames() >= RUN_ITERS);

    // Shutdown: dropping AppSurfaces closes every hand-off and joins every
    // presenter thread (bounded by PRESENTER_JOIN_TIMEOUT); the joined
    // threads released their window guards.
    drop(surfaces);
    let after_drop: Vec<usize> = guards.iter().map(WindowGuard::strong_count).collect();
    assert_eq!(
        after_drop, baseline,
        "a surface or presenter thread still holds a window guard"
    );
    drop(ctx);
    drop(guards);
    keeper.drain();
    assert!(keeper.is_empty(), "windows not destroyed after the guards");
    println!("[presenter_thread_window] ok");
}
