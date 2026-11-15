//! Issue #9b: releasing one of two raw inputs bound to the same cabinet input
//! must not release the cabinet input while the other is held
//! (`pin_app::platform::HeldInputs`, `InputRouter`, `EventForwarder`).

use pin_app::platform::{
    EventForwarder, HeldInputs, InputRouter, KEYBOARD_DEVICE, MAX_HELD_SOURCES, RAW_BUTTON,
    RAW_KEY, Translated, raw_code, source_device, source_id,
};
use pin_core::spsc::{SpscConsumer, channel};
use pin_core::{CabinetAxis, CabinetInput, InputEvent, InputEventKind};
use pin_input::profiles::builtin_profile;
use pin_input::{GamepadAxis, GamepadButton, RawInput};
use proptest::prelude::*;
use sdl3::event::Event;
use sdl3::gamepad::{Axis, Button};
use sdl3::joystick::JoystickId;
use sdl3::keyboard::{Mod, Scancode};

const LSHIFT: u32 = 225;
const SPACE: u32 = 44;
const RETURN: u32 = 40;

fn kb(raw: u32) -> u32 {
    source_id(KEYBOARD_DEVICE, RAW_KEY + raw)
}

// --- HeldInputs --------------------------------------------------------------

#[test]
fn key_and_gamepad_button_alias() {
    let mut h = HeldInputs::new();
    let key = kb(LSHIFT);
    let pad = source_id(1, RAW_BUTTON + GamepadButton::LeftShoulder as u32);
    assert_eq!(h.set(key, CabinetInput::LeftFlipper, true), Some(true));
    assert_eq!(h.set(pad, CabinetInput::LeftFlipper, true), None);
    assert!(h.is_held(CabinetInput::LeftFlipper));
    assert_eq!(h.set(key, CabinetInput::LeftFlipper, false), None);
    assert!(
        h.is_held(CabinetInput::LeftFlipper),
        "still held by the pad"
    );
    assert_eq!(h.set(pad, CabinetInput::LeftFlipper, false), Some(false));
    assert!(!h.is_held(CabinetInput::LeftFlipper));
}

#[test]
fn two_keys_on_one_device_alias() {
    let mut h = HeldInputs::new();
    assert_eq!(h.set(kb(SPACE), CabinetInput::Launch, true), Some(true));
    assert_eq!(h.set(kb(RETURN), CabinetInput::Launch, true), None);
    assert_eq!(h.set(kb(SPACE), CabinetInput::Launch, false), None);
    assert!(h.is_held(CabinetInput::Launch));
    assert_eq!(h.pressed_mask(), 1 << CabinetInput::Launch.index());
    assert_eq!(h.set(kb(RETURN), CabinetInput::Launch, false), Some(false));
    assert_eq!(h.pressed_mask(), 0);
}

#[test]
fn repeat_press_counts_once_and_unheld_release_is_ignored() {
    let mut h = HeldInputs::new();
    let a = kb(SPACE);
    assert_eq!(h.set(a, CabinetInput::Start, false), None, "never pressed");
    assert_eq!(h.set(a, CabinetInput::Start, true), Some(true));
    assert_eq!(h.set(a, CabinetInput::Start, true), None, "repeat press");
    assert_eq!(h.set(a, CabinetInput::Start, true), None, "repeat press");
    assert!(h.is_held_by(a, CabinetInput::Start));
    assert_eq!(
        h.set(kb(RETURN), CabinetInput::Start, false),
        None,
        "not a holder"
    );
    assert!(h.is_held(CabinetInput::Start));
    // One release undoes the repeated presses.
    assert_eq!(h.set(a, CabinetInput::Start, false), Some(false));
    assert_eq!(
        h.set(a, CabinetInput::Start, false),
        None,
        "already released"
    );
    assert_eq!(h.active_sources(), 0);
}

#[test]
fn one_source_can_hold_several_inputs() {
    let mut h = HeldInputs::new();
    let a = kb(SPACE);
    assert_eq!(h.set(a, CabinetInput::Start, true), Some(true));
    assert_eq!(h.set(a, CabinetInput::Launch, true), Some(true));
    assert_eq!(h.active_sources(), 1);
    assert_eq!(h.set(a, CabinetInput::Start, false), Some(false));
    assert!(h.is_held(CabinetInput::Launch));
    assert_eq!(
        h.release_source(a).as_slice(),
        &[CabinetInput::Launch],
        "release_source returns what became released"
    );
    assert!(h.release_source(a).is_empty());
}

#[test]
fn device_disconnect_releases_only_what_that_device_alone_held() {
    let mut h = HeldInputs::new();
    let pad = |b: GamepadButton| source_id(3, RAW_BUTTON + b as u32);
    h.set(
        pad(GamepadButton::LeftShoulder),
        CabinetInput::LeftFlipper,
        true,
    );
    h.set(
        pad(GamepadButton::RightShoulder),
        CabinetInput::RightFlipper,
        true,
    );
    h.set(pad(GamepadButton::Start), CabinetInput::Start, true);
    h.set(kb(LSHIFT), CabinetInput::LeftFlipper, true);
    // Another gamepad holds tilt.
    h.set(source_id(4, RAW_BUTTON), CabinetInput::Tilt, true);
    let released = h.release_all_for_device(3);
    assert_eq!(
        released.as_slice(),
        &[CabinetInput::RightFlipper, CabinetInput::Start],
        "left flipper is still held by the keyboard"
    );
    assert!(h.is_held(CabinetInput::LeftFlipper));
    assert!(h.is_held(CabinetInput::Tilt));
    assert!(h.release_all_for_device(3).is_empty());
    assert_eq!(source_device(source_id(3, 7)), 3);
}

#[test]
fn capacity_is_at_least_256_distinct_sources() {
    let mut h = HeldInputs::new();
    const { assert!(MAX_HELD_SOURCES >= 256) };
    for s in 0..256u32 {
        let r = h.set(source_id(s, RAW_KEY + s), CabinetInput::Action, true);
        assert_eq!(r, (s == 0).then_some(true));
    }
    assert_eq!(h.active_sources(), 256);
    for s in 0..255u32 {
        assert_eq!(
            h.set(source_id(s, RAW_KEY + s), CabinetInput::Action, false),
            None
        );
    }
    assert_eq!(
        h.set(source_id(255, RAW_KEY + 255), CabinetInput::Action, false),
        Some(false)
    );
    assert_eq!(h.overflow(), 0);
}

#[test]
fn source_ids_never_collide_across_devices_or_kinds() {
    let mut seen = std::collections::BTreeSet::new();
    for device in 0..20u32 {
        for raw in [
            RawInput::Key(LSHIFT),
            RawInput::Key(SPACE),
            RawInput::Button(GamepadButton::South),
            RawInput::Button(GamepadButton::Touchpad),
            RawInput::Axis(GamepadAxis::LeftTrigger),
        ] {
            assert!(seen.insert(source_id(device, raw_code(raw))));
        }
    }
}

/// A random interleaving of (source, input, pressed) edges.
fn edges() -> impl Strategy<Value = Vec<(u32, usize, bool)>> {
    prop::collection::vec(
        (0u32..12, 0usize..CabinetInput::COUNT, any::<bool>()),
        0..200,
    )
}

proptest! {
    /// For any interleaving: `is_held` equals "at least one source is down"
    /// in a plain model, and the returned edges are exactly the model's
    /// transitions of that OR.
    #[test]
    fn is_held_matches_the_or_of_sources(ops in edges()) {
        let mut h = HeldInputs::new();
        let mut down = [[false; 12]; CabinetInput::COUNT];
        for (src, i, pressed) in ops {
            let input = CabinetInput::ALL[i];
            let was = down[i].iter().any(|d| *d);
            down[i][src as usize] = pressed;
            let now = down[i].iter().any(|d| *d);
            let expect = (was != now).then_some(now);
            // Sources spread over devices and raw kinds.
            let id = source_id(src % 3, RAW_KEY + src);
            prop_assert_eq!(h.set(id, input, pressed), expect);
            for (k, inp) in CabinetInput::ALL.iter().enumerate() {
                prop_assert_eq!(h.is_held(*inp), down[k].iter().any(|d| *d));
            }
        }
        let mask = CabinetInput::ALL
            .iter()
            .enumerate()
            .filter(|(k, _)| down[*k].iter().any(|d| *d))
            .fold(0u8, |m, (_, i)| m | 1 << i.index());
        prop_assert_eq!(h.pressed_mask(), mask);
    }

    /// Device release equals releasing each of its sources in the model.
    #[test]
    fn device_release_matches_model(ops in edges(), device in 0u32..3) {
        let mut h = HeldInputs::new();
        let mut down = [[false; 12]; CabinetInput::COUNT];
        for (src, i, pressed) in ops {
            down[i][src as usize] = pressed;
            h.set(source_id(src % 3, RAW_KEY + src), CabinetInput::ALL[i], pressed);
        }
        let before: Vec<bool> = down.iter().map(|d| d.iter().any(|x| *x)).collect();
        for d in &mut down {
            for (src, x) in d.iter_mut().enumerate() {
                if src as u32 % 3 == device {
                    *x = false;
                }
            }
        }
        let after: Vec<bool> = down.iter().map(|d| d.iter().any(|x| *x)).collect();
        let expect: Vec<CabinetInput> = CabinetInput::ALL
            .iter()
            .enumerate()
            .filter(|(k, _)| before[*k] && !after[*k])
            .map(|(_, i)| *i)
            .collect();
        prop_assert_eq!(h.release_all_for_device(device).to_vec(), expect);
        for (k, inp) in CabinetInput::ALL.iter().enumerate() {
            prop_assert_eq!(h.is_held(*inp), after[k]);
        }
    }
}

// --- InputRouter + EventForwarder (end to end, no SDL init) --------------------

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

fn pad(down: bool, which: u32, button: Button, ts: u64) -> Event {
    let which = JoystickId::new(which);
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

fn forwarder() -> (EventForwarder, SpscConsumer<InputEvent>) {
    let (itx, irx) = channel(64);
    let (wtx, _wrx) = channel(4);
    let mut f = EventForwarder::new(itx, wtx);
    f.set_router(InputRouter::new());
    (f, irx)
}

fn drain(rx: &mut SpscConsumer<InputEvent>) -> Vec<InputEvent> {
    std::iter::from_fn(|| rx.pop()).collect()
}

fn button(ts: u64, input: CabinetInput, pressed: bool) -> InputEvent {
    InputEvent::button(ts, input, pressed)
}

#[test]
fn forwarder_holds_keyboard_and_gamepad_aliases_until_last_release() {
    let (mut f, mut rx) = forwarder();
    assert_eq!(f.router().unwrap().keyboard_profile(), "generic_keyboard");
    f.gamepad_added(7, None);
    assert_eq!(f.router().unwrap().gamepad(7), Some((1, "generic_gamepad")));
    f.forward(&key(true, Scancode::LShift, 10, false));
    f.forward(&pad(true, 7, Button::LeftShoulder, 11));
    f.forward(&key(false, Scancode::LShift, 12, false));
    assert_eq!(
        drain(&mut rx),
        [button(10, CabinetInput::LeftFlipper, true)],
        "the key release must not release the flipper the pad holds"
    );
    assert_eq!(
        f.forward(&pad(false, 7, Button::LeftShoulder, 13)),
        Translated::Input(button(13, CabinetInput::LeftFlipper, false))
    );
    assert_eq!(
        drain(&mut rx),
        [button(13, CabinetInput::LeftFlipper, false)]
    );
    let st = f.router().unwrap().stats();
    assert_eq!((st.edges, st.absorbed), (2, 2));
}

#[test]
fn forwarder_holds_two_keys_and_ignores_repeats() {
    let (mut f, mut rx) = forwarder();
    f.forward(&key(true, Scancode::Space, 1, false));
    f.forward(&key(true, Scancode::Space, 2, true)); // OS key repeat
    assert_eq!(
        f.forward(&key(true, Scancode::Return, 3, false)),
        Translated::Ignored,
        "an alias press is absorbed"
    );
    f.forward(&key(false, Scancode::Space, 4, false));
    f.forward(&key(false, Scancode::Return, 5, false));
    f.forward(&key(false, Scancode::Return, 6, false)); // unheld release
    f.forward(&key(true, Scancode::Q, 7, false)); // unmapped
    assert_eq!(
        drain(&mut rx),
        [
            button(1, CabinetInput::Launch, true),
            button(5, CabinetInput::Launch, false)
        ]
    );
}

#[test]
fn gamepad_removal_releases_its_inputs_only() {
    let (mut f, mut rx) = forwarder();
    f.gamepad_added(3, None);
    f.gamepad_added(4, None);
    f.forward(&pad(true, 3, Button::Start, 1));
    f.forward(&pad(true, 3, Button::RightShoulder, 2));
    f.forward(&pad(true, 4, Button::RightShoulder, 3));
    let t = f.forward(&Event::GamepadRemoved {
        timestamp: 9,
        which: JoystickId::new(3),
    });
    assert_eq!(t, Translated::GamepadRemoved(JoystickId::new(3)));
    assert_eq!(
        drain(&mut rx),
        [
            button(1, CabinetInput::Start, true),
            button(2, CabinetInput::RightFlipper, true),
            button(9, CabinetInput::Start, false),
        ],
        "the right flipper is still held by pad 4"
    );
    assert!(f.router().unwrap().gamepad(3).is_none());
    // A reconnected pad gets the lowest free device number again.
    f.gamepad_added(5, None);
    assert_eq!(f.router().unwrap().gamepad(5).map(|g| g.0), Some(1));
    f.forward(&pad(false, 4, Button::RightShoulder, 10));
    assert_eq!(
        drain(&mut rx),
        [button(10, CabinetInput::RightFlipper, false)]
    );
}

#[test]
fn axis_as_button_and_analog_axes() {
    let mut r = InputRouter::new();
    // A profile binding the left trigger as the launch button.
    let p = pin_input::parse_profile(
        r#"
id = "trigger_launch"
name = "Trigger launch"
[buttons]
launch = [ "axis:left_trigger", "button:south" ]
[[axes]]
raw = "left_y"
axis = "plunger"
from = 0.0
to = 1.0
[nudge]
left = [ "button:west" ]
"#,
    )
    .unwrap();
    assert_eq!(r.add_gamepad_with_profile(9, &p), Some(1));
    let mut out = Vec::new();
    let mut push = |e: InputEvent| out.push(e);
    // The trigger reports many motion events while pressed: one edge.
    for (ts, v) in [(1, 30000), (2, 31000), (3, 32767)] {
        r.axis(ts, 9, GamepadAxis::LeftTrigger, v, &mut push);
    }
    r.button(4, 9, GamepadButton::South, true, &mut push);
    for (ts, v) in [(5, 0), (6, 100)] {
        r.axis(ts, 9, GamepadAxis::LeftTrigger, v, &mut push);
    }
    r.button(7, 9, GamepadButton::South, false, &mut push);
    // Plunger axis through the filter: at rest it reads 0.
    r.axis(8, 9, GamepadAxis::LeftY, 0, &mut push);
    // Digital nudge: a press pushes left, poll ends it.
    r.button(10_000_000, 9, GamepadButton::West, true, &mut push);
    r.poll(
        10_000_000 + 1_000_000 * u64::from(pin_app::platform::DIGITAL_NUDGE_MS),
        &mut push,
    );
    let kinds: Vec<InputEventKind> = out.iter().map(|e| e.kind).collect();
    assert_eq!(
        kinds,
        [
            InputEventKind::Button {
                input: CabinetInput::Launch,
                pressed: true
            },
            InputEventKind::Button {
                input: CabinetInput::Launch,
                pressed: false
            },
            InputEventKind::Axis {
                axis: CabinetAxis::Plunger,
                value: 0.0
            },
            InputEventKind::Axis {
                axis: CabinetAxis::NudgeX,
                value: -pin_app::platform::DIGITAL_NUDGE_STRENGTH
            },
            InputEventKind::Axis {
                axis: CabinetAxis::NudgeX,
                value: 0.0
            },
        ]
    );
    assert_eq!(out[1].timestamp_ns, 7, "released by the last holder");
}

#[test]
fn router_routes_sdl_axis_events_and_unknown_pads() {
    let (mut f, mut rx) = forwarder();
    // An event of a pad that was never announced uses the generic profile.
    let t = f.forward(&Event::GamepadAxisMotion {
        timestamp: 5,
        which: JoystickId::new(11),
        axis: Axis::TriggerRight,
        value: 32767,
    });
    assert_eq!(
        t,
        Translated::Input(InputEvent::axis(5, CabinetAxis::RightFlipper, 1.0))
    );
    assert_eq!(drain(&mut rx).len(), 1);
    assert_eq!(
        f.router().unwrap().gamepad(11).map(|g| g.1),
        Some("generic_gamepad")
    );
    // Non-input events still go the old way.
    assert_eq!(f.forward(&Event::Quit { timestamp: 1 }), Translated::Quit);
    // A profile switch through router_mut.
    let xk = builtin_profile("xarcade_gamepad").unwrap();
    f.router_mut().unwrap().add_gamepad_with_profile(11, xk);
    assert_eq!(
        f.router().unwrap().gamepad(11).map(|g| g.1),
        Some("xarcade_gamepad")
    );
}

#[test]
fn without_a_router_the_1_3_keymap_path_is_unchanged() {
    let (itx, mut irx) = channel(8);
    let (wtx, _w) = channel(4);
    let mut f = EventForwarder::new(itx, wtx);
    assert!(f.router().is_none());
    f.forward(&key(true, Scancode::Space, 1, false));
    f.forward(&key(true, Scancode::Return, 2, false));
    f.gamepad_added(1, None); // no router: no effect
    f.poll(5);
    assert_eq!(drain(&mut irx).len(), 2);
}
