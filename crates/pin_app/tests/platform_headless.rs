//! SDL headless test (dummy video/audio drivers): pushed keyboard events and a
//! virtual gamepad come out of the input ring as `InputEvent`s carrying SDL's
//! nanosecond timestamps; window events come out of the window ring; the
//! display topology manager creates the planned windows.
//!
//! SDL may only be initialised once per process and must run on the main
//! thread (macOS), so this is a `harness = false` binary. No sleeps: every
//! check uses SDL's own clock for ordering only (PLAN §2.3 rule 5).

use core::time::Duration;
use pin_app::platform::{
    DisplayTopologyManager, EventForwarder, INPUT_RING_CAPACITY, Platform, PlatformConfig,
    PlatformError, WINDOW_RING_CAPACITY, window_title,
};
use pin_core::spsc::{SpscConsumer, channel};
use pin_core::{CabinetAxis, CabinetInput, InputEvent, InputEventKind};
use pin_render::topology::{
    DEFAULT_BACKGLASS_PIP, DEFAULT_SCORE_PIP, SurfaceRole, TopologyError, TopologyMode,
    WindowEvent,
};
use sdl3::event::{Event, WindowEvent as SdlWindowEvent};
use sdl3::gamepad::{Axis, Button};
use sdl3::joystick::{JoystickType, VirtualJoystickDescription};
use sdl3::keyboard::{Mod, Scancode};

const DRAIN: Duration = Duration::ZERO;

fn ticks_ns() -> u64 {
    // SAFETY: plain FFI query, valid after SDL_Init.
    unsafe { sdl3::sys::timer::SDL_GetTicksNS() }
}

fn drain<T: Copy + Send>(rx: &mut SpscConsumer<T>) -> Vec<T> {
    std::iter::from_fn(|| rx.pop()).collect()
}

fn key(down: bool, sc: Scancode) -> Event {
    let (timestamp, window_id, keycode, scancode, keymod, repeat, which, raw) =
        (0, 0, None, Some(sc), Mod::NOMOD, false, 0, 0);
    if down {
        Event::KeyDown {
            timestamp,
            window_id,
            keycode,
            scancode,
            keymod,
            repeat,
            which,
            raw,
        }
    } else {
        Event::KeyUp {
            timestamp,
            window_id,
            keycode,
            scancode,
            keymod,
            repeat,
            which,
            raw,
        }
    }
}

/// Joystick button index of `b` on a virtual gamepad built with `buttons`:
/// SDL numbers the buttons of a virtual gamepad in ascending button order.
fn virtual_button_index(buttons: &[Button], b: Button) -> u32 {
    buttons.iter().filter(|x| (**x as u32) < (b as u32)).count() as u32
}

fn check(name: &str, ok: bool) {
    assert!(ok, "check failed: {name}");
    println!("[platform_headless] ok: {name}");
}

fn main() {
    let mut platform = Platform::init(PlatformConfig { headless: true }).expect("SDL init");
    println!(
        "[platform_headless] video driver: {}",
        platform.video().current_video_driver()
    );
    check(
        "dummy video driver",
        platform.video().current_video_driver() == "dummy",
    );

    let (itx, mut irx) = channel::<InputEvent>(INPUT_RING_CAPACITY);
    let (wtx, mut wrx) = channel::<WindowEvent>(WINDOW_RING_CAPACITY);
    let mut fwd = EventForwarder::new(itx, wtx);
    platform.pump(&mut fwd, DRAIN);
    drain(&mut irx);
    drain(&mut wrx);

    // --- Display topology -------------------------------------------------
    let mut manager = DisplayTopologyManager::query(platform.video()).expect("displays");
    println!("[platform_headless] displays: {:?}", manager.displays());
    check("at least one display", !manager.displays().is_empty());
    manager.refresh(platform.video()).expect("refresh");
    let d0 = manager.displays()[0];
    check("display 0 has a size", d0.bounds.w > 0 && d0.bounds.h > 0);

    let single = TopologyMode::SingleMonitor {
        display: 0,
        backglass: DEFAULT_BACKGLASS_PIP,
        score: DEFAULT_SCORE_PIP,
    };
    let plan = manager.plan(&single).expect("single-monitor plan");
    check(
        "unknown display rejected",
        manager.plan(&TopologyMode::Cabinet {
            playfield: 99,
            backglass: None,
            score: None,
        }) == Err(TopologyError::DisplayOutOfRange(99)),
    );
    let windows = DisplayTopologyManager::create_windows(platform.video(), &plan)
        .expect("create windows");
    check("one window created", windows.len() == 1);
    let w = &windows[0];
    check("window plan index", w.plan_index == 0);
    check(
        "window plan has three viewports",
        w.plan.viewports.len() == 3 && w.plan.primary_role() == Some(SurfaceRole::Playfield),
    );
    check(
        "window title",
        w.window.title() == window_title(Some(SurfaceRole::Playfield)),
    );
    let (ww, wh) = w.window.size();
    println!("[platform_headless] window size {ww}x{wh}, plan {:?}", w.plan.bounds);
    check("window has a size", ww > 0 && wh > 0);
    let ids: Vec<u32> = windows.iter().map(|w| w.id()).collect();
    fwd.register_windows(&ids);

    // Window creation events (shown/moved/fullscreen...) reach the window ring
    // tagged with the plan index.
    platform.pump(&mut fwd, DRAIN);
    let wevents = drain(&mut wrx);
    println!("[platform_headless] window events after create: {wevents:?}");
    check(
        "window events forwarded with plan index",
        !wevents.is_empty() && wevents.iter().all(|e| e.window == 0),
    );

    // A pushed window event keeps its SDL timestamp.
    let t0 = ticks_ns();
    platform
        .events()
        .push_event(Event::Window {
            timestamp: 0,
            window_id: ids[0],
            win_event: SdlWindowEvent::Exposed,
        })
        .expect("push window event");
    let t1 = ticks_ns();
    platform.pump(&mut fwd, DRAIN);
    let wevents = drain(&mut wrx);
    check(
        "pushed window event forwarded with SDL timestamp",
        wevents.len() == 1 && (t0..=t1).contains(&wevents[0].timestamp_ns),
    );

    // --- Keyboard: order and timestamps -----------------------------------
    let keys = [
        (true, Scancode::LShift, CabinetInput::LeftFlipper),
        (true, Scancode::RShift, CabinetInput::RightFlipper),
        (false, Scancode::LShift, CabinetInput::LeftFlipper),
        (true, Scancode::LCtrl, CabinetInput::LeftUpper),
        (true, Scancode::RCtrl, CabinetInput::RightUpper),
        (true, Scancode::Space, CabinetInput::Launch),
        (true, Scancode::Return, CabinetInput::Launch),
        (true, Scancode::_1, CabinetInput::Start),
        (true, Scancode::A, CabinetInput::Action),
        (false, Scancode::T, CabinetInput::Tilt),
        (true, Scancode::Q, CabinetInput::Tilt), // unmapped: dropped
    ];
    let t0 = ticks_ns();
    for (down, sc, _) in keys {
        platform.events().push_event(key(down, sc)).expect("push key");
    }
    let t1 = ticks_ns();
    let out = platform.pump(&mut fwd, DRAIN);
    check("pump counted the key events", out.events >= keys.len() as u32);
    let got = drain(&mut irx);
    println!("[platform_headless] keyboard input events: {got:?}");
    let expected: Vec<_> = keys[..keys.len() - 1]
        .iter()
        .map(|(down, _, input)| InputEventKind::Button {
            input: *input,
            pressed: *down,
        })
        .collect();
    check(
        "keyboard events in order",
        got.iter().map(|e| e.kind).collect::<Vec<_>>() == expected,
    );
    check(
        "keyboard timestamps on SDL ns clock and non-decreasing",
        got.iter().all(|e| (t0..=t1).contains(&e.timestamp_ns))
            && got.windows(2).all(|p| p[0].timestamp_ns <= p[1].timestamp_ns),
    );

    // --- Virtual gamepad --------------------------------------------------
    let buttons = [
        Button::South,
        Button::East,
        Button::Back,
        Button::Start,
        Button::LeftShoulder,
        Button::RightShoulder,
    ];
    let desc = VirtualJoystickDescription::new()
        .name("kHz virtual cabinet")
        .joystick_type(JoystickType::Gamepad)
        .with_buttons(buttons)
        .with_axis(Axis::LeftX)
        .with_axis(Axis::LeftY)
        .with_axis(Axis::TriggerLeft);
    let conn = platform
        .joystick()
        .attach_virtual_joystick(desc)
        .expect("virtual joystick");
    let id = conn.id();
    check("virtual device is a gamepad", platform.gamepad().is_gamepad(id));
    platform.pump(&mut fwd, DRAIN);
    check(
        "gamepad opened on GamepadAdded",
        platform.open_gamepads().contains(&id),
    );
    // Opening also emits initial-state events; discard them.
    drain(&mut irx);
    let js = platform.joystick().open(id).expect("open joystick");

    let rs = virtual_button_index(&buttons, Button::RightShoulder);
    js.set_virtual_button(rs, true).expect("press");
    let t0 = ticks_ns();
    platform.joystick().update();
    let t1 = ticks_ns();
    platform.pump(&mut fwd, DRAIN);
    let got = drain(&mut irx);
    println!("[platform_headless] gamepad press: {got:?} (t0={t0} t1={t1})");
    check(
        "gamepad right shoulder -> right flipper press with SDL timestamp",
        got.len() == 1
            && got[0].kind
                == InputEventKind::Button {
                    input: CabinetInput::RightFlipper,
                    pressed: true,
                }
            && (t0..=t1).contains(&got[0].timestamp_ns),
    );

    let start = virtual_button_index(&buttons, Button::Start);
    js.set_virtual_button(rs, false).expect("release");
    js.set_virtual_button(start, true).expect("start");
    platform.joystick().update();
    platform.pump(&mut fwd, DRAIN);
    let kinds: Vec<_> = drain(&mut irx).into_iter().map(|e| e.kind).collect();
    println!("[platform_headless] gamepad release/start: {kinds:?}");
    check(
        "gamepad release and start",
        kinds.contains(&InputEventKind::Button {
            input: CabinetInput::RightFlipper,
            pressed: false,
        }) && kinds.contains(&InputEventKind::Button {
            input: CabinetInput::Start,
            pressed: true,
        }),
    );

    // Trigger axis -> analog left flipper travel. The trigger is the third
    // axis of the device (LeftX, LeftY, TriggerLeft).
    js.set_virtual_axis(2, 32767).expect("axis");
    platform.joystick().update();
    platform.pump(&mut fwd, DRAIN);
    let axes: Vec<_> = drain(&mut irx)
        .into_iter()
        .filter_map(|e| match e.kind {
            InputEventKind::Axis { axis, value } => Some((axis, value)),
            InputEventKind::Button { .. } => None,
        })
        .collect();
    println!("[platform_headless] gamepad axes: {axes:?}");
    check(
        "trigger -> left flipper axis at full travel",
        axes.last() == Some(&(CabinetAxis::LeftFlipper, 1.0)),
    );

    drop(js);
    drop(conn);
    platform.pump(&mut fwd, DRAIN);
    check(
        "gamepad closed on GamepadRemoved",
        !platform.open_gamepads().contains(&id),
    );

    // --- Quit and the blocking wait -----------------------------------------
    platform
        .events()
        .push_event(Event::Quit { timestamp: 0 })
        .expect("push quit");
    // The 1 ms wait returns immediately because an event is queued.
    let out = platform.pump(&mut fwd, pin_app::platform::PUMP_TIMEOUT);
    check("quit reported", out.quit);
    let out = platform.pump(&mut fwd, DRAIN);
    check("queue drained completely", out.events == 0 && !out.quit);

    let stats = fwd.stats();
    println!("[platform_headless] stats: {stats:?}");
    check("nothing dropped", stats.input_dropped == 0 && stats.window_dropped == 0);
    check(
        "error type displays",
        !PlatformError::from(TopologyError::NoDisplays)
            .to_string()
            .is_empty()
            && !PlatformError::Sdl("x".into()).to_string().is_empty()
            && !PlatformError::Window("x".into()).to_string().is_empty(),
    );

    drop(windows);
    println!("[platform_headless] RESULT: all checks passed");
}
