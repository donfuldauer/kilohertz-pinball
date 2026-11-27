//! REQ-VIEW-006 (issue #84 / KP-004): `AppSurfaces::{playfield_format,
//! set_menu_overlay, set_menu_state, menu_overlay}` on a real window and
//! surface (GPU job: Xvfb + lavapipe, Metal on macOS; a missing display or
//! adapter fails, never skips). SDL needs the main thread, so this is a
//! plain `main` (harness = false) through `pin_testmain`.
//!
//! On macOS the window must be raised and the event pump drained for the
//! Metal surface to stop reporting `Occluded`, so each phase renders until
//! a number of playfield frames really presented and counts those only.

mod support;

use arrayvec::ArrayVec;
use pin_app::orchestrator::{
    AppSurfaces, TableRenderOptions, TableVisuals, load_checked, validate_table,
};
use pin_app::platform::WindowKeeper;
use pin_app::ui::{MenuInput, MenuState, menu_overlay_for};
use pin_core::snapshot::FrameStateSnapshot;
use pin_render::gpu::{
    GpuContext, GpuOptions, SurfaceOptions, SurfaceSource, SurfaceStatus, create_instance,
    create_surface,
};
use pin_render::topology::{Rect, SurfaceRole};
use pin_sim::SimConfig;
use support::{greybox_bundle, on_big_stack};

/// Render calls one phase may make before a surface that never presents
/// is reported: a count, not a time (R6; a hang is the CI job's timeout).
/// Generous, so a window that needs a long warm-up (raised and drained on
/// macOS before its Metal surface stops reporting `Occluded`) still gets
/// there.
const MAX_ATTEMPTS: u64 = 1_000_000;

/// One frame: drains SDL events (as the app's main thread does), renders,
/// yields when the frame was not paced; returns the playfield status.
fn render_frame(
    surfaces: &mut AppSurfaces,
    pump: &mut sdl3::EventPump,
    frame: &mut FrameStateSnapshot,
) -> SurfaceStatus {
    while pump.poll_event().is_some() {}
    frame.tick += 1;
    let st = surfaces.render(frame);
    if !surfaces.frame_paced(&st) {
        std::thread::yield_now();
    }
    st[0]
}

/// Renders until `n` playfield frames returned `Presented` and returns `n`.
/// No wall-clock condition: the loop ends on presented frames only and
/// panics with the phase, the counts so far and the status-change history
/// after [`MAX_ATTEMPTS`] render calls without `n` presented frames.
fn render_presented(
    n: u64,
    surfaces: &mut AppSurfaces,
    pump: &mut sdl3::EventPump,
    frame: &mut FrameStateSnapshot,
    phase: &str,
) -> u64 {
    let mut presented = 0;
    let mut attempts: u64 = 0;
    let mut history: Vec<SurfaceStatus> = Vec::new();
    while presented < n {
        assert!(
            attempts < MAX_ATTEMPTS,
            "[{phase}] only {presented} of {n} playfield frames presented in {attempts} render \
             calls; status changes {history:?}"
        );
        attempts += 1;
        let st = render_frame(surfaces, pump, frame);
        if history.last() != Some(&st) {
            history.push(st);
        }
        if st == SurfaceStatus::Presented {
            presented += 1;
        }
    }
    n
}

/// REQ-VIEW-006: format, state and paints through `AppSurfaces`.
fn run() -> pin_testmain::Outcome {
    let sdl = sdl3::init().expect("SDL_Init (a missing display is a failure here)");
    let video = sdl.video().expect("SDL video");
    let mut win = video
        .window("pin_app menu surfaces test", 160, 240)
        .position(64, 64)
        .build()
        .expect("window");
    win.raise();
    let (lw, lh) = win.size();
    let (pw, ph) = win.size_in_pixels();
    let mut keeper = WindowKeeper::new();
    let guard = keeper.adopt(win);
    let mut pump = sdl.event_pump().expect("pump");
    let opts = GpuOptions::from_env();
    let instance = create_instance(opts.backends);
    let mut viewports = ArrayVec::new();
    viewports.push((SurfaceRole::Playfield, Rect::new(0, 0, lw, lh)));
    let window_ref = keeper.windows().next().expect("window");
    let source = SurfaceSource {
        window: 0,
        viewports,
        // SAFETY: the keeper destroys the window only after the guard
        // (held by the surface) is dropped.
        surface: unsafe { create_surface(&instance, window_ref) }.expect("create_surface"),
        logical_size: (lw, lh),
        pixel_size: (pw, ph),
        keep_alive: Some(guard),
    };
    let ctx = GpuContext::with_instance(instance, &opts, Some(&source.surface))
        .expect("GPU adapter (a missing adapter is a failure here)");
    let dir = greybox_bundle().to_path_buf();
    let (visuals, snap) = on_big_stack(move || {
        let checked = validate_table(&dir).expect("greybox validates");
        let sim = load_checked(&checked, SimConfig::default()).expect("greybox loads");
        (TableVisuals::of(&checked), *sim.snapshot())
    });
    let mut surfaces = AppSurfaces::new(
        &ctx,
        vec![source],
        SurfaceOptions {
            vrr: false,
            force_presenter_thread: false,
        },
        &visuals,
        TableRenderOptions::default(),
    )
    .expect("AppSurfaces");

    // The format: the one the overlay must be built for, one the egui
    // pipeline can draw onto.
    let format = surfaces.playfield_format().expect("a playfield slot");
    let overlay = menu_overlay_for(&ctx, Some(format)).expect("overlay for the surface");
    assert_eq!(overlay.format(), format);
    // No overlay yet: state is ignored, nothing to observe.
    assert!(surfaces.menu_overlay().is_none());
    surfaces.set_menu_state(MenuState::default());
    assert!(surfaces.menu_overlay().is_none());

    surfaces.set_menu_overlay(overlay);
    let o = surfaces.menu_overlay().expect("overlay retained");
    assert_eq!((o.format(), o.is_open(), o.paints()), (format, false, 0));

    // Closed: presented frames render without the panel.
    let mut frame = snap;
    render_presented(3, &mut surfaces, &mut pump, &mut frame, "closed");
    assert_eq!(surfaces.menu_overlay().unwrap().paints(), 0);

    // Open: the playfield frames get the panel painted, once per presented
    // frame (`paint_menu` runs on every frame that then presents).
    let mut open = MenuState::default();
    open.apply(MenuInput::Toggle);
    surfaces.set_menu_state(open);
    assert!(surfaces.menu_overlay().unwrap().is_open());
    let presented = render_presented(2, &mut surfaces, &mut pump, &mut frame, "open");
    let paints = surfaces.menu_overlay().unwrap().paints();
    assert!(
        presented >= 2,
        "only {presented} frames presented while open"
    );
    assert_eq!(
        paints, presented,
        "menu painted {paints} times over {presented} presented frames while open"
    );

    // Closed again: painting stops.
    surfaces.set_menu_state(MenuState::default());
    assert!(!surfaces.menu_overlay().unwrap().is_open());
    let before = surfaces.menu_overlay().unwrap().paints();
    render_presented(3, &mut surfaces, &mut pump, &mut frame, "closed again");
    assert_eq!(surfaces.menu_overlay().unwrap().paints(), before);

    drop(surfaces);
    drop(ctx);
    keeper.drain();
    pin_testmain::Outcome::Passed
}

fn main() {
    pin_testmain::libtest_main("app_menu_surfaces", run)
}
