//! Minimal default keymap (PLAN §4 Subphase 1.3). Full, data-driven input
//! profiles arrive in Subphase 3.2.
//!
//! Keyboard (by scancode, so it is layout independent):
//!
//! | Key | Cabinet input |
//! |---|---|
//! | Left Shift / Right Shift | left / right flipper |
//! | Left Ctrl / Right Ctrl | left / right upper flipper |
//! | Space, Enter, keypad Enter | launch |
//! | 1 | start |
//! | A | action |
//! | T | tilt |
//!
//! Gamepad: shoulders = flippers, stick clicks = upper flippers, South =
//! launch, East = action, Start = start, Back = tilt. Triggers drive the analog
//! flipper axes, left stick down drives the plunger, the right stick nudges.

use pin_core::{CabinetAxis, CabinetInput};
use sdl3::gamepad::{Axis, Button};
use sdl3::keyboard::Scancode;

/// Cabinet input bound to a keyboard scancode.
pub const fn key_input(scancode: Scancode) -> Option<CabinetInput> {
    Some(match scancode {
        Scancode::LShift => CabinetInput::LeftFlipper,
        Scancode::RShift => CabinetInput::RightFlipper,
        Scancode::LCtrl => CabinetInput::LeftUpper,
        Scancode::RCtrl => CabinetInput::RightUpper,
        Scancode::Space | Scancode::Return | Scancode::KpEnter => CabinetInput::Launch,
        Scancode::_1 => CabinetInput::Start,
        Scancode::A => CabinetInput::Action,
        Scancode::T => CabinetInput::Tilt,
        _ => return None,
    })
}

/// Cabinet input bound to a gamepad button.
pub const fn gamepad_button_input(button: Button) -> Option<CabinetInput> {
    Some(match button {
        Button::LeftShoulder => CabinetInput::LeftFlipper,
        Button::RightShoulder => CabinetInput::RightFlipper,
        Button::LeftStick => CabinetInput::LeftUpper,
        Button::RightStick => CabinetInput::RightUpper,
        Button::South => CabinetInput::Launch,
        Button::East => CabinetInput::Action,
        Button::Start => CabinetInput::Start,
        Button::Back => CabinetInput::Tilt,
        _ => return None,
    })
}

/// Cabinet axis and normalised value for a gamepad axis reading.
///
/// Triggers (SDL range `0..=32767`) map to `0..=1` flipper travel; left stick
/// Y maps its downward half to `0..=1` plunger pull; right stick X/Y map to
/// `-1..=1` nudge.
pub fn gamepad_axis_input(axis: Axis, raw: i16) -> Option<(CabinetAxis, f32)> {
    let signed = (f32::from(raw) / 32767.0).clamp(-1.0, 1.0);
    let unsigned = signed.max(0.0);
    Some(match axis {
        Axis::TriggerLeft => (CabinetAxis::LeftFlipper, unsigned),
        Axis::TriggerRight => (CabinetAxis::RightFlipper, unsigned),
        Axis::LeftY => (CabinetAxis::Plunger, unsigned),
        Axis::RightX => (CabinetAxis::NudgeX, signed),
        Axis::RightY => (CabinetAxis::NudgeY, signed),
        Axis::LeftX => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_keyboard_map() {
        let cases = [
            (Scancode::LShift, CabinetInput::LeftFlipper),
            (Scancode::RShift, CabinetInput::RightFlipper),
            (Scancode::LCtrl, CabinetInput::LeftUpper),
            (Scancode::RCtrl, CabinetInput::RightUpper),
            (Scancode::Space, CabinetInput::Launch),
            (Scancode::Return, CabinetInput::Launch),
            (Scancode::KpEnter, CabinetInput::Launch),
            (Scancode::_1, CabinetInput::Start),
            (Scancode::A, CabinetInput::Action),
            (Scancode::T, CabinetInput::Tilt),
        ];
        for (sc, input) in cases {
            assert_eq!(key_input(sc), Some(input), "{sc:?}");
        }
        assert_eq!(key_input(Scancode::B), None);
        assert_eq!(key_input(Scancode::Escape), None);
        // Every cabinet input is reachable from the keyboard.
        for input in CabinetInput::ALL {
            assert!(cases.iter().any(|(_, i)| *i == input));
        }
    }

    #[test]
    fn default_gamepad_map() {
        for input in CabinetInput::ALL {
            let reachable = [
                Button::LeftShoulder,
                Button::RightShoulder,
                Button::LeftStick,
                Button::RightStick,
                Button::South,
                Button::East,
                Button::Start,
                Button::Back,
            ]
            .into_iter()
            .any(|b| gamepad_button_input(b) == Some(input));
            assert!(reachable, "{input:?}");
        }
        assert_eq!(gamepad_button_input(Button::North), None);
    }

    #[test]
    fn axis_normalisation() {
        assert_eq!(
            gamepad_axis_input(Axis::TriggerLeft, 32767),
            Some((CabinetAxis::LeftFlipper, 1.0))
        );
        assert_eq!(
            gamepad_axis_input(Axis::TriggerRight, 0),
            Some((CabinetAxis::RightFlipper, 0.0))
        );
        assert_eq!(
            gamepad_axis_input(Axis::LeftY, -32768),
            Some((CabinetAxis::Plunger, 0.0))
        );
        assert_eq!(
            gamepad_axis_input(Axis::RightX, -32768),
            Some((CabinetAxis::NudgeX, -1.0))
        );
        let (axis, v) = gamepad_axis_input(Axis::RightY, 16384).unwrap();
        assert_eq!(axis, CabinetAxis::NudgeY);
        assert!((v - 0.5).abs() < 1e-3);
        assert_eq!(gamepad_axis_input(Axis::LeftX, 100), None);
    }
}
