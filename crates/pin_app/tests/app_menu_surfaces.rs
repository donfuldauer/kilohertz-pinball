//! REQ-VIEW-006 (issue #84 / KP-004): `AppSurfaces::{playfield_format,
//! set_menu_overlay, set_menu_state, menu_overlay}` on a real window and
//! surface (GPU job: Xvfb + lavapipe, Metal on macOS; a missing display or
//! adapter fails, never skips). SDL needs the main thread, so this is a
//! plain `main` (harness = false) through `pin_testmain`.

mod support;

use arrayvec::ArrayVec;
use pin_app::orchestrator::{
    AppSurfaces, TableRenderOptions, TableVisuals, load_checked, validate_table,
};
use pin_app::platform::WindowKeeper;
use pin_app::ui::{MenuInput, MenuState, menu_overlay_for};
use pin_render::gpu::{
    GpuContext, GpuOptions, SurfaceOptions, SurfaceSource, create_instance, create_surface,
};
use pin_render::topology::{Rect, SurfaceRole};
use pin_sim::SimConfig;

use support::{greybox_bundle, on_big_stack};

/// REQ-VIEW-006: format, state and paints through `AppSurfaces`.
fn run() -> pin_testmain::Outcome {
    let sdl = sdl3::init().expect("SDL_Init (a missing display is a failure here)");
    let video = sdl.video().expect("SDL video");
    let win = video
        .window("pin_app menu surfaces test", 160, 240)
        .build()
        .expect("window");
    let (lw, lh) = win.size();
    let (pw, ph) = win.size_in_pixels();
    let mut keeper = WindowKeeper::new();
    let guard = keeper.adopt(win);
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

    // Closed: frames render without the panel.
    let mut frame = snap;
    for _ in 0..3 {
        frame.tick += 1;
        surfaces.render(&frame);
    }
    assert_eq!(surfaces.menu_overlay().unwrap().paints(), 0);

    // Open: the playfield frames get the panel painted.
    let mut open = MenuState::default();
    open.apply(MenuInput::Toggle);
    surfaces.set_menu_state(open);
    assert!(surfaces.menu_overlay().unwrap().is_open());
    let mut presented = 0;
    for _ in 0..600 {
        frame.tick += 1;
        surfaces.render(&frame);
        presented = surfaces.menu_overlay().unwrap().paints();
        if presented >= 2 {
            break;
        }
    }
    assert!(presented >= 2, "menu painted {presented} times while open");

    // Closed again: painting stops.
    surfaces.set_menu_state(MenuState::default());
    assert!(!surfaces.menu_overlay().unwrap().is_open());
    let before = surfaces.menu_overlay().unwrap().paints();
    for _ in 0..3 {
        frame.tick += 1;
        surfaces.render(&frame);
    }
    assert_eq!(surfaces.menu_overlay().unwrap().paints(), before);

    drop(surfaces);
    drop(ctx);
    keeper.drain();
    pin_testmain::Outcome::Passed
}

fn main() {
    pin_testmain::libtest_main("app_menu_surfaces", run)
}
