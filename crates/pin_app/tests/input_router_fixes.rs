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

/// Pad 2 drives LeftFlipper with 8192, then pad 1 with full travel; returns
/// the router and the value the router emitted for pad 2's sample.
fn two_trigger_pads() -> (InputRouter, f32) {
    let mut r = InputRouter::new();
    r.add_gamepad_with_profile(1, &profile(TRIGGER_FLIPPER));
    r.add_gamepad_with_profile(2, &profile(TRIGGER_FLIPPER));
    let ev = collect(|o| r.axis(1_000, 2, GamepadAxis::LeftTrigger, 8192, o));
    assert_eq!(ev.len(), 1, "{ev:?}");
    let InputEventKind::Axis { axis, value } = ev[0].kind else {
        panic!("axis event expected: {ev:?}");
    };
    assert_eq!(axis, CabinetAxis::LeftFlipper);
    assert!(value > 0.0 && value < 1.0, "{value}");
    let mut sink = |_: InputEvent| {};
    r.axis(1_500, 1, GamepadAxis::LeftTrigger, i16::MAX, &mut sink);
    (r, value)
}

#[test]
fn remove_gamepad_hands_axis_to_remaining_pad() {
    let (mut r, v2) = two_trigger_pads();
    let ev = collect(|o| r.remove_gamepad(2_000, 1, o));
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert!(is_axis(&ev[0], CabinetAxis::LeftFlipper, v2), "{ev:?}");
    assert_eq!(ev[0].timestamp_ns, 2_000);
    // Once pad 2 returns to zero it no longer drives it: removal is silent.
    let mut sink = |_: InputEvent| {};
    r.axis(2_500, 2, GamepadAxis::LeftTrigger, 0, &mut sink);
    assert!(collect(|o| r.remove_gamepad(3_000, 2, o)).is_empty());
}

#[test]
fn profile_change_hands_axis_to_remaining_pad() {
    let (mut r, v2) = two_trigger_pads();
    r.add_gamepad_with_profile(1, &profile(SOUTH_START));
    let ev = collect(|o| r.poll(2_000, o));
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert!(is_axis(&ev[0], CabinetAxis::LeftFlipper, v2), "{ev:?}");
    assert!(collect(|o| r.poll(3_000, o)).is_empty());
}

#[test]
fn remove_gamepad_only_driver_goes_neutral() {
    let (mut r, _) = two_trigger_pads();
    let mut sink = |_: InputEvent| {};
    r.axis(1_800, 2, GamepadAxis::LeftTrigger, 0, &mut sink);
    let ev = collect(|o| r.remove_gamepad(2_000, 1, o));
    assert_eq!(ev.len(), 1, "{ev:?}");
    assert!(is_axis(&ev[0], CabinetAxis::LeftFlipper, 0.0), "{ev:?}");
}

#[test]
fn remove_gamepad_on_other_axis_leaves_shared_axis_alone() {
    let mut r = InputRouter::new();
    r.add_gamepad_with_profile(1, &profile(TRIGGER_FLIPPER));
    r.add_gamepad_with_profile(2, &profile(SOUTH_START));
    let mut sink = |_: InputEvent| {};
    r.axis(1_000, 1, GamepadAxis::LeftTrigger, i16::MAX, &mut sink);
    // Pad 2 has no analog mapping for LeftFlipper: removing it emits nothing.
    r.axis(1_500, 2, GamepadAxis::LeftTrigger, 20_000, &mut sink);
    let ev = collect(|o| r.remove_gamepad(2_000, 2, o));
    assert!(ev.is_empty(), "pad 2 drove nothing: {ev:?}");
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
    // Pad 1 crosses: one negative NudgeX event.
    let a = collect(|o| r.axis(ms, 1, GamepadAxis::RightX, 30_000, o));
    assert_eq!(a.len(), 1, "{a:?}");
    assert!(matches!(
        a[0].kind,
        InputEventKind::Axis { axis: CabinetAxis::NudgeX, value } if value < 0.0
    ));
    // Pad 1 stays above the threshold; its nudge expires.
    assert!(collect(|o| r.axis(2 * ms, 1, GamepadAxis::RightX, 32_000, o)).is_empty());
    let ended = collect(|o| r.poll(500 * ms, o));
    assert_eq!(ended.len(), 1, "nudge ended: {ended:?}");
    // Pad 2's first high sample, with pad 1 still above threshold, is its own
    // crossing and must start a new nudge.
    let b = collect(|o| r.axis(501 * ms, 2, GamepadAxis::RightX, 30_000, o));
    assert_eq!(b.len(), 1, "pad 2 crossing independent of pad 1: {b:?}");
    assert!(matches!(
        b[0].kind,
        InputEventKind::Axis { axis: CabinetAxis::NudgeX, value } if value < 0.0
    ));
    // Pad 2 drops below and crosses again (after expiry): re-armed on its own.
    let _ = collect(|o| r.poll(1_000 * ms, o));
    assert!(collect(|o| r.axis(1_001 * ms, 2, GamepadAxis::RightX, 0, o)).is_empty());
    let c = collect(|o| r.axis(1_002 * ms, 2, GamepadAxis::RightX, 30_000, o));
    assert_eq!(c.len(), 1, "pad 2 re-armed on its own: {c:?}");
}
