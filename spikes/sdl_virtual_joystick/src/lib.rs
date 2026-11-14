//! Spike 0.1/Q2: SDL virtual joysticks created headless (D31 hardware simulation).
//!
//! The cabinet stand-in is a virtual gamepad with the buttons a pinball cabinet
//! maps (flippers, magna-save, start, launch) and one analog axis (plunger).

use sdl3::gamepad::{Axis, Button};
use sdl3::joystick::{JoystickType, VirtualJoystickConnection, VirtualJoystickDescription};
use sdl3::{IntegerOrSdlError, JoystickSubsystem};

/// Buttons on the simulated cabinet, in SDL gamepad-button order.
pub const CABINET_BUTTONS: [Button; 4] = [
    Button::South,
    Button::Start,
    Button::LeftShoulder,
    Button::RightShoulder,
];
/// Analog plunger axis.
pub const PLUNGER_AXIS: Axis = Axis::LeftY;

/// Hints that make SDL usable with no display and no window focus.
pub fn set_headless_hints() {
    sdl3::hint::set("SDL_VIDEO_DRIVER", "dummy");
    sdl3::hint::set("SDL_AUDIO_DRIVER", "dummy");
    sdl3::hint::set("SDL_JOYSTICK_ALLOW_BACKGROUND_EVENTS", "1");
}

/// Attach the simulated cabinet as a virtual joystick.
pub fn attach_cabinet(
    js: &JoystickSubsystem,
) -> Result<VirtualJoystickConnection, IntegerOrSdlError> {
    let desc = VirtualJoystickDescription::new()
        .name("kHz virtual cabinet")
        .joystick_type(JoystickType::Gamepad)
        .with_buttons(CABINET_BUTTONS)
        .with_axis(PLUNGER_AXIS);
    js.attach_virtual_joystick(desc)
}

/// SDL button index of a gamepad button on the virtual device.
pub fn button_index(b: Button) -> u32 {
    b as u32
}
