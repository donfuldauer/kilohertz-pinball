//! The Subphase 4.4 menu and `config.toml` live in the app (REQ-VIEW-006,
//! issue #84 / KP-004): implementer integration tests.
//!
//! GPU-free (except the `gpu-tests` paint check at the end), no SDL init, no
//! sleeping and no wall-clock assertions (R6): SDL events are built as plain
//! values, as the unit tests of `platform::events` do. Config files live
//! under `CARGO_TARGET_TMPDIR`.

use pin_app::platform::{EventForwarder, InputRouter};
use pin_app::ui::{AppConfig, MenuController, MenuInput, Panel, SettingChange, cabinet_menu_input};
use pin_core::spsc::{SpscConsumer, channel};
use pin_core::{CabinetAxis, CabinetInput, InputEvent, InputEventKind};
use proptest::prelude::*;
use sdl3::event::Event;
use sdl3::gamepad::{Axis, Button};
use sdl3::joystick::JoystickId;
use sdl3::keyboard::{Mod, Scancode};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

/// A fresh, empty directory under the cargo test temp dir.
fn fresh_dir(tag: &str) -> PathBuf {
    static N: AtomicU32 = AtomicU32::new(0);
    let d = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!(
        "app_menu_{tag}_{}_{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn key(down: bool, sc: Scancode, ts: u64, repeat: bool) -> Event {
    if down {
        Event::KeyDown {
            timestamp: ts,
            window_id: 1,
            keycode: None,
            scancode: Some(sc),
            keymod: Mod::NOMOD,
            repeat,
            which: 0,
            raw: 0,
        }
    } else {
        Event::KeyUp {
            timestamp: ts,
            window_id: 1,
            keycode: None,
            scancode: Some(sc),
            keymod: Mod::NOMOD,
            repeat,
            which: 0,
            raw: 0,
        }
    }
}

fn pad_button(down: bool, button: Button, ts: u64) -> Event {
    let which = JoystickId::new(3);
    if down {
        Event::GamepadButtonDown {
            timestamp: ts,
            which,
            button,
        }
    } else {
        Event::GamepadButtonUp {
            timestamp: ts,
            which,
            button,
        }
    }
}

fn drain(rx: &mut SpscConsumer<InputEvent>) -> Vec<InputEvent> {
    std::iter::from_fn(|| rx.pop()).collect()
}

/// A forwarder with the default router and a menu on `dir/config.toml`.
fn forwarder(dir: &Path, router: bool) -> (EventForwarder, SpscConsumer<InputEvent>) {
    let (itx, irx) = channel(256);
    let (wtx, _wrx) = channel(16);
    let mut f = EventForwarder::new(itx, wtx);
    if router {
        f.set_router(InputRouter::new());
    }
    let (menu, warnings) = MenuController::load(dir.join("config.toml"));
    assert!(warnings.is_empty(), "{warnings:?}");
    f.set_menu(menu);
    (f, irx)
}

fn is_open(f: &EventForwarder) -> bool {
    f.menu().unwrap().is_open()
}

/// REQ-VIEW-006 "Esc toggles the menu state": a non-repeat `Esc` press
/// opens and closes the menu; repeats and releases do nothing; the
/// controller menu (Guide) button toggles too; other keys do not; the
/// toggle key never reaches the input ring.
#[test]
fn req_view_006_esc_toggles_the_menu_state() {
    let d = fresh_dir("toggle");
    let (mut f, mut rx) = forwarder(&d, true);
    assert!(!is_open(&f));
    assert_eq!(
        f.forward(&key(true, Scancode::Escape, 10, false)),
        pin_app::platform::Translated::Ignored
    );
    assert!(is_open(&f), "Esc opens");
    f.forward(&key(true, Scancode::Escape, 11, true));
    assert!(is_open(&f), "a repeat is ignored");
    f.forward(&key(false, Scancode::Escape, 12, false));
    assert!(is_open(&f), "the key-up is ignored");
    f.forward(&key(true, Scancode::Escape, 13, false));
    assert!(!is_open(&f), "Esc again closes");
    f.forward(&key(false, Scancode::Escape, 14, false));
    assert!(!is_open(&f));
    f.forward(&pad_button(true, Button::Guide, 15));
    assert!(is_open(&f), "the controller menu button opens");
    f.forward(&pad_button(false, Button::Guide, 16));
    assert!(is_open(&f), "its release is ignored");
    f.forward(&pad_button(true, Button::Guide, 17));
    assert!(!is_open(&f), "and closes");
    for sc in [Scancode::Q, Scancode::Return, Scancode::Space, Scancode::F1] {
        f.forward(&key(true, sc, 18, false));
        f.forward(&key(false, sc, 19, false));
        assert!(!is_open(&f), "{sc:?} does not toggle");
    }
    // Nothing was held, so the toggles sent nothing; the toggle key and
    // button never reach the sim.
    assert!(
        drain(&mut rx).iter().all(|e| e.timestamp_ns >= 18),
        "only the other keys' events (if mapped) reach the ring"
    );
    // Without a menu, Esc is just an unmapped key.
    let menu = f.take_menu().unwrap();
    assert!(!menu.is_open());
    assert!(f.menu().is_none() && f.menu_mut().is_none());
    f.forward(&key(true, Scancode::Escape, 20, false));
    assert!(drain(&mut rx).is_empty());
}

/// REQ-VIEW-006 "opening releases a held flipper in the same tick, through
/// HeldInputs": LShift held (ts A) then Esc (ts B) puts
/// `button(B, LeftFlipper, false)` in the ring, stamped with the Esc
/// event's own timestamp (so the sim applies it in the tick of the Esc
/// press). The physical key-up while open reaches neither the ring nor
/// the sim; after it the router's `HeldInputs` and the menu's tracking
/// agree nothing is held; after closing, a new press forwards again.
#[test]
fn req_view_006_opening_releases_a_held_flipper_through_held_inputs() {
    let d = fresh_dir("release");
    let (mut f, mut rx) = forwarder(&d, true);
    let (a, b) = (1_000_000u64, 1_250_000u64);
    f.forward(&key(true, Scancode::LShift, a, false));
    assert_eq!(
        drain(&mut rx),
        [InputEvent::button(a, CabinetInput::LeftFlipper, true)]
    );
    assert!(
        f.router()
            .unwrap()
            .held()
            .is_held(CabinetInput::LeftFlipper)
    );
    f.forward(&key(true, Scancode::Escape, b, false));
    assert_eq!(
        drain(&mut rx),
        [InputEvent::button(b, CabinetInput::LeftFlipper, false)],
        "the release carries the Esc event's timestamp"
    );
    assert!(is_open(&f));
    // The physical release while open: consumed.
    f.forward(&key(false, Scancode::LShift, b + 10, false));
    assert!(drain(&mut rx).is_empty(), "nothing reaches the sim");
    assert_eq!(f.router().unwrap().held().pressed_mask(), 0);
    assert!(
        !f.menu()
            .unwrap()
            .menu()
            .monitor()
            .is_pressed(CabinetInput::LeftFlipper)
    );
    // Close, then the menu's tracking is still empty: closing re-presses
    // nothing.
    f.forward(&key(true, Scancode::Escape, b + 20, false));
    assert!(!is_open(&f));
    assert!(drain(&mut rx).is_empty(), "closing sends nothing");
    // Opening again now sends nothing: nothing is held for the sim.
    f.forward(&key(true, Scancode::Escape, b + 30, false));
    assert!(drain(&mut rx).is_empty());
    f.forward(&key(true, Scancode::Escape, b + 40, false));
    f.forward(&key(true, Scancode::LShift, b + 50, false));
    assert_eq!(
        drain(&mut rx),
        [InputEvent::button(b + 50, CabinetInput::LeftFlipper, true)],
        "after close a new press forwards again"
    );
}

/// REQ-VIEW-006: the same for a non-neutral axis (the unrouted keymap path:
/// the left trigger drives the left flipper axis): opening sends
/// `axis(B, LeftFlipper, 0.0)`; axis motion while open is consumed.
#[test]
fn req_view_006_opening_neutralises_a_held_axis() {
    let d = fresh_dir("axis");
    let (mut f, mut rx) = forwarder(&d, false);
    let which = JoystickId::new(3);
    let axis = |ts: u64, value: i16| Event::GamepadAxisMotion {
        timestamp: ts,
        which,
        axis: Axis::TriggerLeft,
        value,
    };
    f.forward(&axis(100, 32767));
    assert_eq!(
        drain(&mut rx),
        [InputEvent::axis(100, CabinetAxis::LeftFlipper, 1.0)]
    );
    f.forward(&pad_button(true, Button::Guide, 200));
    assert_eq!(
        drain(&mut rx),
        [InputEvent::axis(200, CabinetAxis::LeftFlipper, 0.0)]
    );
    f.forward(&axis(210, 0));
    f.forward(&axis(220, 32767));
    assert!(
        drain(&mut rx).is_empty(),
        "axis motion while open is consumed"
    );
    f.forward(&pad_button(true, Button::Guide, 230));
    f.forward(&axis(240, 32767));
    assert_eq!(
        drain(&mut rx),
        [InputEvent::axis(240, CabinetAxis::LeftFlipper, 1.0)]
    );
}

/// REQ-VIEW-006 "settings round-trip through config.toml": a change writes
/// the file (atomically, through `AppConfig::save`), `AppConfig::load`
/// reads it back, and a second controller on the same path starts with it;
/// unknown keys survive; a no-op change writes nothing; a missing file stays
/// missing until a change; `take_config_change` returns the latest change
/// once.
#[test]
fn req_view_006_settings_round_trip_through_config_toml() {
    let d = fresh_dir("roundtrip");
    let path = d.join("config.toml");
    let (mut c, w) = MenuController::load(&path);
    assert!(w.is_empty());
    assert!(!path.exists(), "loading a missing file creates nothing");
    let mut sink = |_: InputEvent| panic!("nothing is held");
    c.toggle(1, &mut sink);
    c.apply(MenuInput::Down, 2, &mut sink);
    assert!(!path.exists(), "navigation writes nothing");
    assert!(c.take_config_change().is_none());
    c.apply(MenuInput::Set(SettingChange::Wireframe(true)), 3, &mut sink);
    assert!(path.exists());
    assert_eq!(c.saves(), 1);
    assert!(c.last_save_error().is_none());
    assert!(AppConfig::load(&path).config.wireframe);
    let latest = c.take_config_change().expect("a change");
    assert!(latest.wireframe);
    assert!(c.take_config_change().is_none(), "returned once");
    // A no-op change writes nothing.
    let before = std::fs::read_to_string(&path).unwrap();
    c.apply(MenuInput::Set(SettingChange::Wireframe(true)), 4, &mut sink);
    assert_eq!(c.saves(), 1);
    assert!(c.take_config_change().is_none());
    // Two changes before a take: the latest wins.
    c.apply(
        MenuInput::Set(SettingChange::Wireframe(false)),
        5,
        &mut sink,
    );
    c.apply(MenuInput::Set(SettingChange::Wireframe(true)), 6, &mut sink);
    assert!(c.take_config_change().unwrap().wireframe);
    assert_eq!(c.saves(), 3);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), before);
    // A second controller starts with the saved settings.
    let (c2, w2) = MenuController::load(&path);
    assert!(w2.is_empty(), "{w2:?}");
    assert!(c2.config().wireframe);
    assert_eq!(c2.config(), c2.menu().config());
    assert_eq!(c2.config_path(), path);

    // Unknown keys survive a save.
    let path3 = d.join("unknown.toml");
    std::fs::write(&path3, "future_key = \"keep me\"\n[future_table]\nx = 1\n").unwrap();
    let (mut c3, w3) = MenuController::load(&path3);
    assert_eq!(w3.len(), 2, "{w3:?}");
    c3.toggle(1, &mut sink);
    c3.apply(MenuInput::Set(SettingChange::Wireframe(true)), 2, &mut sink);
    let text = std::fs::read_to_string(&path3).unwrap();
    assert!(text.contains("future_key = \"keep me\""), "{text}");
    assert!(text.contains("[future_table]"), "{text}");
    assert!(AppConfig::load(&path3).config.wireframe);
}

/// REQ-VIEW-006: a save into a read-only directory sets `last_save_error`
/// and never panics; the change still applies to the running app.
#[test]
fn req_view_006_save_error_is_reported_not_fatal() {
    let d = fresh_dir("readonly");
    let ro = d.join("ro");
    std::fs::create_dir_all(&ro).unwrap();
    let mut perms = std::fs::metadata(&ro).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&ro, perms.clone()).unwrap();
    // Root ignores directory permissions: use a path under a regular file
    // as the guaranteed-unwritable fallback.
    let probe = std::fs::write(ro.join("probe"), b"x").is_ok();
    let path = if probe {
        let f = d.join("file");
        std::fs::write(&f, b"x").unwrap();
        f.join("config.toml")
    } else {
        ro.join("config.toml")
    };
    let (mut c, _) = MenuController::load(&path);
    let mut sink = |_: InputEvent| {};
    c.toggle(1, &mut sink);
    c.apply(MenuInput::Set(SettingChange::Wireframe(true)), 2, &mut sink);
    let e = c.last_save_error().expect("the save failed").to_owned();
    assert!(e.contains("config.toml"), "{e}");
    assert_eq!(c.saves(), 0);
    assert!(c.take_config_change().unwrap().wireframe);
    #[allow(clippy::permissions_set_readonly_false)]
    perms.set_readonly(false);
    std::fs::set_permissions(&ro, perms).unwrap();
}

/// REQ-VIEW-006 (4.4 routing ruling): while the menu is open, cabinet
/// presses navigate it as `cabinet_menu_input` says and none reaches the
/// input ring.
#[test]
fn req_view_006_open_menu_consumes_cabinet_events() {
    let d = fresh_dir("consume");
    let (mut f, mut rx) = forwarder(&d, true);
    f.forward(&key(true, Scancode::Escape, 1, false));
    assert_eq!(f.menu().unwrap().menu().panel(), Panel::Tables);
    assert_eq!(
        cabinet_menu_input(CabinetInput::RightUpper),
        Some(MenuInput::Right)
    );
    // Right upper (RCtrl): next panel.
    f.forward(&key(true, Scancode::RCtrl, 2, false));
    f.forward(&key(false, Scancode::RCtrl, 3, false));
    assert_eq!(f.menu().unwrap().menu().panel(), Panel::Display);
    // Right flipper (RShift): focus down, twice.
    for ts in [4, 6] {
        f.forward(&key(true, Scancode::RShift, ts, false));
        f.forward(&key(false, Scancode::RShift, ts + 1, false));
    }
    assert_eq!(f.menu().unwrap().menu().focus(), 2);
    // Left flipper (LShift): focus up.
    f.forward(&key(true, Scancode::LShift, 8, false));
    f.forward(&key(false, Scancode::LShift, 9, false));
    assert_eq!(f.menu().unwrap().menu().focus(), 1);
    // Left upper (LCtrl) on a setting row steps the value (display mode).
    let before = f.menu().unwrap().config().display_mode;
    f.forward(&key(true, Scancode::LCtrl, 10, false));
    f.forward(&key(false, Scancode::LCtrl, 11, false));
    assert_ne!(f.menu().unwrap().config().display_mode, before);
    assert!(drain(&mut rx).is_empty(), "nothing reached the sim");
    assert_eq!(f.stats().input_forwarded, 0);
    let menu = f.menu_mut().unwrap();
    assert!(menu.take_config_change().is_some());
    assert_eq!(menu.saves(), 1);
}

/// One step of the property test.
#[derive(Clone, Debug)]
enum Op {
    /// Flip the physical state of cabinet button `i` (press if up).
    Button(usize),
    /// Move axis `i` to `v` (0 = neutral).
    Axis(usize, i8),
    /// Esc.
    Toggle,
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        4 => (0..CabinetInput::COUNT).prop_map(Op::Button),
        2 => (0..CabinetAxis::ALL.len(), -2i8..=2).prop_map(|(a, v)| Op::Axis(a, v)),
        1 => Just(Op::Toggle),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// REQ-VIEW-006 property: for random sequences of presses, releases,
    /// axis moves and toggles (edges as the router produces them), the
    /// forwarded stream has non-decreasing timestamps, nothing is
    /// forwarded while the menu is open, no input is pressed twice without
    /// a release in between, and whenever the menu is open the replay of
    /// the forwarded events leaves every button released and every axis
    /// at 0.0.
    #[test]
    fn req_view_006_menu_routing_invariants(ops in proptest::collection::vec(op(), 0..64)) {
        let d = fresh_dir("prop");
        let (mut c, _) = MenuController::load(d.join("config.toml"));
        let mut physical = [false; CabinetInput::COUNT];
        let mut fwd: Vec<InputEvent> = Vec::new();
        let mut sim_buttons = [false; CabinetInput::COUNT];
        let mut sim_axes = [0.0f32; 5];
        for (k, op) in ops.into_iter().enumerate() {
            let ts = 1_000 * (k as u64 + 1);
            let was_open = c.is_open();
            let before = fwd.len();
            match op {
                Op::Button(i) => {
                    physical[i] = !physical[i];
                    let ev = InputEvent::button(ts, CabinetInput::ALL[i], physical[i]);
                    c.route(ev, &mut |e| fwd.push(e));
                }
                Op::Axis(a, v) => {
                    let ev = InputEvent::axis(ts, CabinetAxis::ALL[a], f32::from(v) / 2.0);
                    c.route(ev, &mut |e| fwd.push(e));
                }
                Op::Toggle => c.toggle(ts, &mut |e| fwd.push(e)),
            }
            if was_open {
                prop_assert_eq!(fwd.len(), before, "nothing is forwarded while open");
            }
            for e in &fwd[before..] {
                prop_assert_eq!(e.timestamp_ns, ts, "stamped with the causing event");
                match e.kind {
                    InputEventKind::Button { input, pressed } => {
                        let i = input.index();
                        prop_assert!(
                            !(pressed && sim_buttons[i]),
                            "two presses of {:?} without a release", input
                        );
                        sim_buttons[i] = pressed;
                    }
                    InputEventKind::Axis { axis, value } => {
                        let k = CabinetAxis::ALL.iter().position(|a| *a == axis).unwrap();
                        sim_axes[k] = value;
                    }
                }
            }
            if c.is_open() {
                prop_assert!(sim_buttons.iter().all(|b| !b), "a button is left pressed");
                prop_assert!(sim_axes.iter().all(|v| *v == 0.0), "an axis is left off neutral");
            }
        }
        prop_assert!(fwd.windows(2).all(|w| w[0].timestamp_ns <= w[1].timestamp_ns));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// REQ-VIEW-006 / R11: `MenuController::load` on random bytes (up to
    /// 64 KiB) never panics and always gives a usable, closed controller.
    #[test]
    fn req_view_006_load_survives_random_bytes(
        bytes in proptest::collection::vec(any::<u8>(), 0..65_536)
    ) {
        let d = fresh_dir("fuzz");
        let path = d.join("config.toml");
        std::fs::write(&path, &bytes).unwrap();
        let (mut c, _warnings) = MenuController::load(&path);
        prop_assert!(!c.is_open());
        let mut sink = |_: InputEvent| {};
        c.toggle(1, &mut sink);
        prop_assert!(c.is_open());
        c.apply(MenuInput::Down, 2, &mut sink);
        c.toggle(3, &mut sink);
        prop_assert!(!c.is_open());
        let _ = std::fs::remove_dir_all(&d);
    }
}

/// REQ-VIEW-006 / Subphase 4.4 deliverable 1 (D30 (a), analytic pixel
/// facts, no snapshot): the open menu painted over a flat colour darkens
/// the panel rectangle by the backdrop's alpha and leaves every pixel
/// outside it untouched; a closed menu paints nothing.
#[cfg(feature = "gpu-tests")]
#[test]
fn req_view_006_menu_overlay_paints_only_the_panel() {
    use pin_app::ui::{MENU_PIXELS_PER_POINT, MenuFrames, MenuState};
    use pin_render::ui::{BACKDROP, OverlayPainter, menu_panel_rect_px, paint_offscreen_with};
    static GPU: std::sync::OnceLock<Option<pin_render::gpu::GpuContext>> =
        std::sync::OnceLock::new();
    let gpu = GPU
        .get_or_init(|| pin_render::gpu::context_for_tests("app_menu"))
        .as_ref()
        .expect("app_menu gpu test needs a GPU adapter (lavapipe); it never skips");
    let (w, h) = (800u32, 600u32);
    let bg = [0.9f32, 0.8, 0.5, 1.0];
    let bg8 = bg.map(|c| (c * 255.0).round() as u8);
    let mut painter = OverlayPainter::new(
        gpu.device(),
        pin_render::gpu::wgpu::TextureFormat::Rgba8Unorm,
    );
    let mut frames = MenuFrames::new();
    let mut open = MenuState::default();
    open.apply(MenuInput::Toggle);
    frames.set_state(open);
    let mut px = Vec::new();
    for _ in 0..3 {
        let frame = frames.frame([w, h]).expect("open menu draws");
        px = paint_offscreen_with(gpu, &mut painter, w, h, bg, frame).unwrap();
    }
    let [x0, y0, x1, y1] = menu_panel_rect_px([w, h], MENU_PIXELS_PER_POINT);
    let at = |x: u32, y: u32| {
        let i = ((y * w + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2]]
    };
    // Outside the panel: untouched.
    for y in 0..h {
        for x in 0..w {
            if x < x0 || x >= x1 || y < y0 || y >= y1 {
                assert_eq!(
                    at(x, y),
                    [bg8[0], bg8[1], bg8[2]],
                    "({x},{y}) outside the panel"
                );
            }
        }
    }
    // Inside: the backdrop's darkening. Expected (premultiplied over):
    // c * (1 - a) + backdrop.
    let a = f32::from(BACKDROP.a()) / 255.0;
    let lum =
        |p: [u8; 3]| 0.2126 * f32::from(p[0]) + 0.7152 * f32::from(p[1]) + 0.0722 * f32::from(p[2]);
    let mut sum = 0.0f64;
    let mut n = 0u64;
    for y in y0 + 8..y1 - 8 {
        for x in x0 + 8..x1 - 8 {
            sum += f64::from(lum(at(x, y)));
            n += 1;
        }
    }
    let mean = (sum / n as f64) as f32;
    let bg_lum = lum([bg8[0], bg8[1], bg8[2]]);
    let backdrop_lum = lum([BACKDROP.r(), BACKDROP.g(), BACKDROP.b()]);
    let expect = bg_lum * (1.0 - a) + backdrop_lum;
    assert!(
        mean < bg_lum * 0.5,
        "panel mean {mean} vs background {bg_lum}"
    );
    // Text and highlights brighten parts of the panel: the mean is at least
    // the plain backdrop and well below the background.
    assert!(
        mean >= expect - 2.0,
        "panel mean {mean} below the backdrop {expect}"
    );
    // The left margin of the panel (no widgets there) is exactly the
    // blended backdrop.
    let p = at(x0 + 6, (y0 + y1) / 2);
    assert!(
        (lum(p) - expect).abs() <= 3.0,
        "margin pixel {p:?} (luminance {}) vs blended backdrop {expect}",
        lum(p)
    );
    // Legible text: bright pixels inside the panel.
    let bright = (y0..y1)
        .flat_map(|y| (x0..x1).map(move |x| (x, y)))
        .filter(|&(x, y)| lum(at(x, y)) > 150.0)
        .count();
    assert!(
        bright > 500,
        "only {bright} bright (text) pixels in the panel"
    );
    // Closed: nothing to paint.
    frames.set_state(MenuState::default());
    assert!(frames.frame([w, h]).is_none());
}

/// REQ-VIEW-006 / Subphase 4.4 deliverable 1: the render-thread overlay
/// (`MenuOverlay`, `menu_overlay_for`, `MenuDebugHooks`) records nothing
/// while the menu is closed and paints the panel over the frame (loading,
/// not clearing it) while it is open.
#[cfg(feature = "gpu-tests")]
#[test]
fn req_view_006_render_thread_overlay_paints_only_while_open() {
    use pin_app::ui::{MenuDebugHooks, MenuOverlay, MenuState, menu_overlay_for};
    use pin_render::gpu::{SurfaceFrameHooks, wgpu};
    use pin_render::topology::SurfaceRole;
    static GPU: std::sync::OnceLock<Option<pin_render::gpu::GpuContext>> =
        std::sync::OnceLock::new();
    let gpu = GPU
        .get_or_init(|| pin_render::gpu::context_for_tests("app_menu_overlay"))
        .as_ref()
        .expect("app_menu gpu test needs a GPU adapter (lavapipe); it never skips");
    let (device, queue) = (gpu.device(), gpu.queue());
    let format = wgpu::TextureFormat::Rgba8Unorm;
    assert!(
        menu_overlay_for(gpu, None).is_none(),
        "no playfield, no overlay"
    );
    let mut overlay: MenuOverlay = menu_overlay_for(gpu, Some(format)).expect("Rgba8Unorm works");
    assert_eq!(overlay.format(), format);
    assert!(format!("{overlay:?}").contains("MenuOverlay"));
    let (w, h) = (320u32, 240u32);
    let tex = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("menu overlay test"),
        size: wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let encoder = || device.create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    // No state, then a closed state: nothing painted.
    let mut e = encoder();
    assert!(!overlay.paint(queue, &mut e, &view, (w, h)));
    overlay.set_state(MenuState::default());
    assert!(!overlay.is_open());
    assert!(!overlay.paint(queue, &mut e, &view, (w, h)));
    queue.submit([e.finish()]);
    assert_eq!(overlay.paints(), 0);
    // Open: painted, directly and through the debug-view hooks (playfield
    // only).
    let mut open = MenuState::default();
    open.apply(MenuInput::Toggle);
    overlay.set_state(open);
    assert!(overlay.is_open());
    let mut e = encoder();
    assert!(overlay.paint(queue, &mut e, &view, (w, h)));
    queue.submit([e.finish()]);
    {
        let mut hooks = MenuDebugHooks::new(None, &mut overlay);
        hooks.begin_frame();
        let mut e = encoder();
        hooks.overlay(SurfaceRole::Backglass, queue, &mut e, &view, (w, h));
        hooks.overlay(SurfaceRole::Playfield, queue, &mut e, &view, (w, h));
        let cbs = hooks.wrap_submit(e.finish());
        assert_eq!(cbs.len(), 1);
        queue.submit(cbs);
        hooks.end_frame(&mut |cb| {
            queue.submit([cb]);
        });
    }
    assert_eq!(
        overlay.paints(),
        2,
        "one direct paint, one playfield hook paint"
    );
    // An inner hook set gets every call too.
    #[derive(Default)]
    struct Count(u32);
    impl SurfaceFrameHooks for Count {
        fn begin_frame(&mut self) {
            self.0 += 1;
        }
        fn end_frame(&mut self, _: &mut dyn FnMut(wgpu::CommandBuffer)) {
            self.0 += 1;
        }
    }
    let mut inner = Count::default();
    {
        let mut hooks = MenuDebugHooks::new(Some(&mut inner), &mut overlay);
        hooks.begin_frame();
        let mut e = encoder();
        hooks.overlay(SurfaceRole::Playfield, queue, &mut e, &view, (w, h));
        let cbs = hooks.wrap_submit(e.finish());
        queue.submit(cbs);
        hooks.end_frame(&mut |cb| {
            queue.submit([cb]);
        });
    }
    assert_eq!(inner.0, 2);
    assert_eq!(overlay.paints(), 3);
}
