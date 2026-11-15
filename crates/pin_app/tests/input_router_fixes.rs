//! PR #21 review fixes for `InputRouter`: a profile swap releases the device's
//! holds, a removed gamepad neutralises its analog axes, and an axis bound
//! only in `[nudge]` fires a digital nudge on threshold crossings.

use pin_app::platform::InputRouter;
use pin_core::{CabinetAxis, CabinetInput, InputEvent, InputEventKind};
use pin_input::{DeviceProfile, GamepadAxis, GamepadButton, parse_profile};

fn profile(text: &str) -> DeviceProfile {
    parse_profile(text).expect("test profile parses")
}

fn collect<F: FnOnce(&mut &mut dyn FnMut(InputEvent))>(f: F) -> Vec<InputEvent> {
    let mut v = Vec::new();
    {
        let mut push = |e| v.push(e);
        let mut sink: &mut dyn FnMut(InputEvent) = &mut push;
        f(&mut sink);
    }
    v
}

const SOUTH_LEFT: &str = r#"
id = "south_left"
name = "South left"
[buttons]
left_flipper = "button:south"
"#;

const SOUTH_START: &str = r#"
id = "south_start"
name = "South start"
[buttons]
start = "button:south"
"#;

const TRIGGER_FLIPPER: &str = r#"
id = "trigger"
name = "Trigger"
[buttons]
launch = "button:south"
[[axes]]
raw = "left_trigger"
axis = "left_flipper"
"#;

const AXIS_NUDGE: &str = r#"
id = "axis_nudge"
name = "Axis nudge"
[nudge]
left = "axis:right_x"
forward = "axis:right_y"
"#;

fn is_button(e: &InputEvent, input: CabinetInput, pressed: bool) -> bool {
    matches!(e.kind, InputEventKind::Button { input: i, pressed: p } if i == input && p == pressed)
}

fn is_axis(e: &InputEvent, axis: CabinetAxis, value: f32) -> bool {
    matches!(e.kind, InputEventKind::Axis { axis: a, value: v } if a == axis && v == value)
}

#[test]
fn profile_change_releases_only_that_devices_holds() {
    let mut r = InputRouter::new();
    let a = profile(SOUTH_LEFT);
    r.add_gamepad_with_profile(1, &a);
    r.add_gamepad_with_profile(2, &a);
    let mut sink = |_: InputEvent| {};
    // Pad 1 holds LeftFlipper alone; pad 2 holds it too (alias) and Launch-free.
    r.button(1_000, 1, GamepadButton::South, true, &mut sink);
    r.button(2_000, 2, GamepadButton::South, true, &mut sink);
    assert!(r.held().is_held(CabinetInput::LeftFlipper));

    // Pad 1 changes profile: pad 2 still holds LeftFlipper, so no release yet.
    r.add_gamepad_with_profile(1, &profile(SOUTH_START));
    let ev = collect(|o| r.poll(3_000, o));
    assert!(ev.is_empty(), "other device still holds it: {ev:?}");
    assert!(r.held().is_held(CabinetInput::LeftFlipper));

    // Pad 2 alone holds a button; pad 2 changes profile: its hold is released.
    r.add_gamepad_with_profile(2, &profile(SOUTH_START));
    let ev = collect(|o| r.poll(4_000, o));
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert!(is_button(&ev[0], CabinetInput::LeftFlipper, false));
    assert_eq!(ev[0].timestamp_ns, 4_000);
    assert!(!r.held().is_held(CabinetInput::LeftFlipper));
    assert_eq!(r.held().active_sources(), 0, "nothing stuck");

    // The releases are not repeated and the new mapping works.
    assert!(collect(|o| r.poll(5_000, o)).is_empty());
    let ev = collect(|o| r.button(6_000, 1, GamepadButton::South, true, o));
    assert!(is_button(&ev[0], CabinetInput::Start, true));
}

#[test]
fn profile_change_keeps_other_device_hold_and_releases_own() {
    let mut r = InputRouter::new();
    r.add_gamepad_with_profile(1, &profile(SOUTH_LEFT));
    r.add_gamepad_with_profile(2, &profile(SOUTH_START));
    let mut sink = |_: InputEvent| {};
    r.button(1_000, 1, GamepadButton::South, true, &mut sink); // LeftFlipper
    r.button(1_000, 2, GamepadButton::South, true, &mut sink); // Start
    r.add_gamepad_with_profile(1, &profile(SOUTH_START));
    let ev = collect(|o| r.poll(2_000, o));
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert!(is_button(&ev[0], CabinetInput::LeftFlipper, false));
    assert!(r.held().is_held(CabinetInput::Start), "pad 2 hold persists");
    assert!(!r.held().is_held(CabinetInput::LeftFlipper));
}

#[test]
fn profile_change_neutralises_analog_axes() {
    let mut r = InputRouter::new();
    r.add_gamepad_with_profile(1, &profile(TRIGGER_FLIPPER));
    let mut sink = |_: InputEvent| {};
    r.axis(1_000, 1, GamepadAxis::LeftTrigger, i16::MAX, &mut sink);
    r.add_gamepad_with_profile(1, &profile(SOUTH_START));
    let ev = collect(|o| r.poll(2_000, o));
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert!(is_axis(&ev[0], CabinetAxis::LeftFlipper, 0.0));
}

#[test]
fn remove_gamepad_neutralises_its_analog_axes() {
    let mut r = InputRouter::new();
    r.add_gamepad_with_profile(1, &profile(TRIGGER_FLIPPER));
    let mut sink = |_: InputEvent| {};
    r.axis(1_000, 1, GamepadAxis::LeftTrigger, i16::MAX, &mut sink);
    let ev = collect(|o| r.remove_gamepad(2_000, 1, o));
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert!(is_axis(&ev[0], CabinetAxis::LeftFlipper, 0.0));
    assert_eq!(ev[0].timestamp_ns, 2_000);
}

#[test]
fn remove_gamepad_keeps_axis_another_pad_drives() {
    let mut r = InputRouter::new();
    r.add_gamepad_with_profile(1, &profile(TRIGGER_FLIPPER));
    r.add_gamepad_with_profile(2, &profile(TRIGGER_FLIPPER));
    let mut sink = |_: InputEvent| {};
    r.axis(1_000, 1, GamepadAxis::LeftTrigger, i16::MAX, &mut sink);
    r.axis(1_500, 2, GamepadAxis::LeftTrigger, 16000, &mut sink);
    let ev = collect(|o| r.remove_gamepad(2_000, 1, o));
    assert!(ev.is_empty(), "pad 2 still drives the axis: {ev:?}");
    // Once pad 2 returns to zero it no longer drives it: removal is silent.
    r.axis(2_500, 2, GamepadAxis::LeftTrigger, 0, &mut sink);
    assert!(collect(|o| r.remove_gamepad(3_000, 2, o)).is_empty());
}

#[test]
fn nudge_only_axis_fires_once_per_crossing_and_rearms() {
    let mut r = InputRouter::new();
    r.add_gamepad_with_profile(1, &profile(AXIS_NUDGE));
    let ms = 1_000_000u64;
    let feed = |r: &mut InputRouter, t: u64, a: GamepadAxis, raw: i16| {
        collect(|o| r.axis(t, 1, a, raw, o))
    };
    // Below threshold: nothing.
    assert!(feed(&mut r, ms, GamepadAxis::RightX, 10_000).is_empty());
    // Crossing: exactly one nudge start (NudgeX axis event, left = negative).
    let ev = feed(&mut r, 2 * ms, GamepadAxis::RightX, 30_000);
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert!(matches!(
        ev[0].kind,
        InputEventKind::Axis { axis: CabinetAxis::NudgeX, value } if value < 0.0
    ));
    // Held above it: no repeats.
    assert!(feed(&mut r, 3 * ms, GamepadAxis::RightX, 32_000).is_empty());
    // Independent state per axis: RightY crossing is its own edge (forward).
    let ev = feed(&mut r, 4 * ms, GamepadAxis::RightY, 30_000);
    assert!(
        ev.iter().any(|e| matches!(
            e.kind,
            InputEventKind::Axis {
                axis: CabinetAxis::NudgeY,
                ..
            }
        )),
        "{ev:?}"
    );
    // Drop below, then cross again: re-armed.
    assert!(feed(&mut r, 5 * ms, GamepadAxis::RightX, 0).is_empty());
    let ev = feed(&mut r, 6 * ms, GamepadAxis::RightX, 30_000);
    assert!(
        ev.iter()
            .any(|e| matches!(e.kind, InputEventKind::Axis { axis: CabinetAxis::NudgeX, value } if value < 0.0)),
        "{ev:?}"
    );
}

#[test]
fn nudge_axis_state_is_per_pad() {
    let mut r = InputRouter::new();
    r.add_gamepad_with_profile(1, &profile(AXIS_NUDGE));
    r.add_gamepad_with_profile(2, &profile(AXIS_NUDGE));
    let ms = 1_000_000u64;
    let a = collect(|o| r.axis(ms, 1, GamepadAxis::RightX, 30_000, o));
    assert_eq!(a.len(), 1);
    // Pad 2 crossing is not suppressed by pad 1 being above the threshold
    // (the nudge vector is unchanged, so poll emits nothing new, but the
    // press must have been registered: letting it expire then re-pressing
    // on pad 2 alone proves independent state).
    let _ = collect(|o| r.axis(2 * ms, 2, GamepadAxis::RightX, 30_000, o));
    let _ = collect(|o| r.poll(500 * ms, o)); // nudge ended
    let _ = collect(|o| r.axis(501 * ms, 1, GamepadAxis::RightX, 32_000, o));
    let b = collect(|o| r.axis(502 * ms, 2, GamepadAxis::RightX, 0, o));
    assert!(b.is_empty());
    let c = collect(|o| r.axis(503 * ms, 2, GamepadAxis::RightX, 30_000, o));
    assert_eq!(c.len(), 1, "pad 2 re-armed on its own: {c:?}");
}
