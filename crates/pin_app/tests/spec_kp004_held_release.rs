//! KP-004 (#84) black-box tests, REQ-VIEW-006 (SPEC section 7.1), acceptance 2:
//! "Opening the menu releases a held flipper in the same tick (through
//! HeldInputs)": the release carries the Esc event's own timestamp. While open
//! no cabinet input reaches the sim and presses navigate the menu.

#[path = "spec_kp004_support/mod.rs"]
mod support;

use pin_app::ui::Panel;
use pin_core::spsc::channel;
use pin_core::{CabinetAxis, CabinetInput, InputEvent, InputEventKind};
use sdl3::event::Event;
use sdl3::gamepad::Axis;
use sdl3::joystick::JoystickId;
use sdl3::keyboard::Scancode;
use support::{Rig, key};

/// REQ-VIEW-006 acceptance 2: left flipper held, Esc at B: the ring holds the
/// press (A) then `button(B, LeftFlipper, false)`, same timestamp as Esc.
#[test]
fn req_view_006_acc2_left_flipper_released_with_esc_timestamp() {
    let mut r = Rig::new("heldleft", true);
    r.send(key(true, Scancode::LShift, 1_000, false));
    r.esc(2_000);
    assert_eq!(
        r.drain(),
        vec![
            InputEvent::button(1_000, CabinetInput::LeftFlipper, true),
            InputEvent::button(2_000, CabinetInput::LeftFlipper, false),
        ]
    );
    assert!(r.is_open());
}

/// REQ-VIEW-006 acceptance 2: same for the right flipper.
#[test]
fn req_view_006_acc2_right_flipper_released_with_esc_timestamp() {
    let mut r = Rig::new("heldright", true);
    r.send(key(true, Scancode::RShift, 10, false));
    r.esc(99);
    assert_eq!(
        r.drain(),
        vec![
            InputEvent::button(10, CabinetInput::RightFlipper, true),
            InputEvent::button(99, CabinetInput::RightFlipper, false),
        ]
    );
}

/// REQ-VIEW-006 acceptance 2: several buttons held together are released in
/// `CabinetInput::ALL` order, all stamped with the Esc timestamp.
#[test]
fn req_view_006_acc2_several_buttons_released_in_all_order() {
    let mut r = Rig::new("heldmany", true);
    // Pressed in reverse of ALL order.
    r.send(key(true, Scancode::Space, 100, false));
    r.send(key(true, Scancode::RShift, 200, false));
    r.send(key(true, Scancode::LShift, 300, false));
    let before = r.drain();
    assert_eq!(before.len(), 3, "three presses reach the ring: {before:?}");
    r.esc(5_000);
    let after = r.drain();
    let expected: Vec<InputEvent> = CabinetInput::ALL
        .into_iter()
        .filter(|i| {
            before.iter().any(|e| {
                matches!(e.kind, InputEventKind::Button { input, pressed: true } if input == *i)
            })
        })
        .map(|i| InputEvent::button(5_000, i, false))
        .collect();
    assert_eq!(expected.len(), 3);
    assert_eq!(after, expected);
    assert_eq!(
        after[0].kind,
        InputEvent::button(0, CabinetInput::LeftFlipper, false).kind
    );
}

/// REQ-VIEW-006 acceptance 2: nothing held, opening emits nothing; a
/// button released before Esc is not released again.
#[test]
fn req_view_006_acc2_nothing_held_emits_nothing() {
    let mut r = Rig::new("heldnone", true);
    r.esc(10);
    assert!(r.drain().is_empty());
    r.esc(20);
    r.send(key(true, Scancode::LShift, 30, false));
    r.send(key(false, Scancode::LShift, 40, false));
    let _ = r.drain();
    r.esc(50);
    assert!(
        r.drain().is_empty(),
        "released flipper is not released twice"
    );
}

/// REQ-VIEW-006 acceptance 2: the same holds on the unrouted keymap path.
#[test]
fn req_view_006_acc2_unrouted_release_with_esc_timestamp() {
    let mut r = Rig::new("heldunrouted", false);
    r.send(key(true, Scancode::LShift, 1_000, false));
    r.esc(2_000);
    assert_eq!(
        r.drain(),
        vec![
            InputEvent::button(1_000, CabinetInput::LeftFlipper, true),
            InputEvent::button(2_000, CabinetInput::LeftFlipper, false),
        ]
    );
}

/// REQ-VIEW-006 acceptance 2: a non-neutral axis is released after the
/// buttons with `axis(B, axis, 0.0)` (unrouted path: trigger -> flipper axis).
#[test]
fn req_view_006_acc2_axis_released_after_buttons() {
    let mut r = Rig::new("heldaxis", false);
    let which = JoystickId::new(7);
    r.send(key(true, Scancode::LShift, 100, false));
    r.send(Event::GamepadAxisMotion {
        timestamp: 200,
        which,
        axis: Axis::TriggerLeft,
        value: 32767,
    });
    let before = r.drain();
    assert_eq!(
        before,
        vec![
            InputEvent::button(100, CabinetInput::LeftFlipper, true),
            InputEvent::axis(200, CabinetAxis::LeftFlipper, 1.0),
        ]
    );
    r.esc(3_000);
    assert_eq!(
        r.drain(),
        vec![
            InputEvent::button(3_000, CabinetInput::LeftFlipper, false),
            InputEvent::axis(3_000, CabinetAxis::LeftFlipper, 0.0),
        ]
    );
}

/// REQ-VIEW-006 acceptance 2: an axis returned to 0.0 before Esc is not
/// released again, and an axis sim-visible as non-zero via a gamepad
/// profile is neutralised (routed path).
#[test]
fn req_view_006_acc2_routed_axis_neutralised() {
    let profile = pin_input::parse_profile(
        r#"
id = "kp004_trigger"
name = "KP004 trigger"
[buttons]
launch = "button:south"
[[axes]]
raw = "left_trigger"
axis = "left_flipper"
"#,
    )
    .expect("profile");
    let mut r = Rig::new("heldroutedaxis", true);
    r.fwd
        .router_mut()
        .expect("router")
        .add_gamepad_with_profile(7, &profile);
    let which = JoystickId::new(7);
    r.send(Event::GamepadAxisMotion {
        timestamp: 100,
        which,
        axis: Axis::TriggerLeft,
        value: 32767,
    });
    let before = r.drain();
    let live: Vec<CabinetAxis> = CabinetAxis::ALL
        .into_iter()
        .filter(|a| {
            before
                .iter()
                .rev()
                .find_map(|e| match e.kind {
                    InputEventKind::Axis { axis, value } if axis == *a => Some(value != 0.0),
                    _ => None,
                })
                .unwrap_or(false)
        })
        .collect();
    assert!(
        !live.is_empty(),
        "trigger yields a non-neutral axis: {before:?}"
    );
    r.esc(9_000);
    let after = r.drain();
    let expected: Vec<InputEvent> = live
        .into_iter()
        .map(|a| InputEvent::axis(9_000, a, 0.0))
        .collect();
    assert_eq!(after, expected);

    // Neutral before Esc: nothing to release.
    r.esc(9_500);
    r.send(Event::GamepadAxisMotion {
        timestamp: 10_000,
        which,
        axis: Axis::TriggerLeft,
        value: 32767,
    });
    r.send(Event::GamepadAxisMotion {
        timestamp: 10_100,
        which,
        axis: Axis::TriggerLeft,
        value: 0,
    });
    let _ = r.drain();
    r.esc(11_000);
    assert!(r.drain().is_empty());
}

/// REQ-VIEW-006 acceptance 2: the Esc timestamp is the event's own, not a
/// clock read: two different Esc stamps give two different release stamps.
#[test]
fn req_view_006_acc2_release_uses_event_timestamp() {
    for esc_ts in [1_001u64, 123_456_789, u64::MAX / 4] {
        let mut r = Rig::new("heldts", true);
        r.send(key(true, Scancode::LShift, 1, false));
        r.esc(esc_ts);
        let evs = r.drain();
        assert_eq!(evs.last().unwrap().timestamp_ns, esc_ts);
    }
}

/// REQ-VIEW-006: while the menu is open a flipper press produces nothing in
/// the input ring and navigates (RightUpper -> next panel, RightFlipper ->
/// focus row 1).
#[test]
fn req_view_006_open_consumes_input_and_navigates() {
    let mut r = Rig::new("nav", true);
    r.esc(10);
    assert_eq!(r.fwd.menu().unwrap().menu().panel(), Panel::Tables);
    r.send(key(true, Scancode::RCtrl, 20, false)); // RightUpper = Right
    assert_eq!(r.fwd.menu().unwrap().menu().panel(), Panel::Display);
    r.send(key(true, Scancode::RShift, 30, false)); // RightFlipper = Down
    assert_eq!(r.fwd.menu().unwrap().menu().focus(), 1);
    r.send(key(true, Scancode::LShift, 40, false)); // LeftFlipper = Up
    assert_eq!(r.fwd.menu().unwrap().menu().focus(), 0);
    r.send(key(true, Scancode::LCtrl, 50, false)); // LeftUpper = Left
    assert_eq!(r.fwd.menu().unwrap().menu().panel(), Panel::Tables);
    assert!(r.drain().is_empty(), "nothing reaches the sim while open");
    assert!(r.is_open());
}

/// REQ-VIEW-006: Action = Back (focus the tab bar), Start/Launch = Select,
/// Tilt = nothing.
#[test]
fn req_view_006_open_select_back_tilt_mapping() {
    let mut r = Rig::new("navsel", true);
    r.esc(10);
    // Tab bar, Start = Select: next panel.
    r.send(key(true, Scancode::_1, 20, false));
    assert_eq!(r.fwd.menu().unwrap().menu().panel(), Panel::Display);
    r.send(key(true, Scancode::RShift, 30, false));
    assert_eq!(r.fwd.menu().unwrap().menu().focus(), 1);
    // Action = Back: focus returns to the tab bar, menu stays open.
    r.send(key(true, Scancode::A, 40, false));
    assert_eq!(r.fwd.menu().unwrap().menu().focus(), 0);
    assert!(r.is_open());
    // Tilt = nothing.
    let (p, f) = {
        let m = r.fwd.menu().unwrap().menu();
        (m.panel(), m.focus())
    };
    r.send(key(true, Scancode::T, 50, false));
    let m = r.fwd.menu().unwrap().menu();
    assert_eq!((m.panel(), m.focus()), (p, f));
    assert!(r.is_open());
    assert!(r.drain().is_empty());
}

/// REQ-VIEW-006: a physical release of a button held when the menu opened
/// goes nowhere; a button still held at close is not re-pressed; a fresh
/// press forwards again.
#[test]
fn req_view_006_release_while_open_and_no_repress_on_close() {
    let mut r = Rig::new("reopen", true);
    r.send(key(true, Scancode::LShift, 100, false));
    r.esc(200);
    let _ = r.drain();
    r.send(key(false, Scancode::LShift, 300, false));
    assert!(
        r.drain().is_empty(),
        "physical release while open goes nowhere"
    );
    r.esc(400);
    assert!(r.drain().is_empty(), "close pushes nothing");
    r.send(key(true, Scancode::LShift, 500, false));
    assert_eq!(
        r.drain(),
        vec![InputEvent::button(500, CabinetInput::LeftFlipper, true)],
        "a fresh press forwards again"
    );

    // Still physically held across open and close: no re-press on close.
    r.esc(600); // open: release at 600
    let _ = r.drain();
    r.esc(700); // close, LShift still down
    assert!(r.drain().is_empty(), "nothing re-pressed on close");
    assert!(!r.is_open());
}

/// REQ-VIEW-006: the release stream lands in a ring the sim can read: a
/// consumer-side check with its own ring and the unrouted path at capacity.
#[test]
fn req_view_006_release_goes_through_the_input_ring() {
    let (itx, mut irx) = channel::<InputEvent>(8);
    let (wtx, _wrx) = channel::<pin_render::topology::WindowEvent>(8);
    let mut f = pin_app::platform::EventForwarder::new(itx, wtx);
    let dir = support::scratch("ringcheck");
    let (m, _) = pin_app::ui::MenuController::load(dir.join("config.toml"));
    f.set_menu(m);
    let _ = f.forward(&key(true, Scancode::RShift, 5, false));
    let _ = f.forward(&key(true, Scancode::Escape, 6, false));
    assert_eq!(
        irx.pop(),
        Some(InputEvent::button(5, CabinetInput::RightFlipper, true))
    );
    assert_eq!(
        irx.pop(),
        Some(InputEvent::button(6, CabinetInput::RightFlipper, false))
    );
    assert_eq!(irx.pop(), None);
    let _ = std::fs::remove_dir_all(dir);
}
