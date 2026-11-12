//! Raw device event → cabinet event translation with fixed lookup tables.

use super::{AxisBinding, DeviceProfile, GamepadAxis, GamepadButton, NudgeDirection, RawInput};
use pin_core::input::{CabinetInput, InputEvent};

const BUTTONS: usize = GamepadButton::ALL.len();
const AXES: usize = GamepadAxis::ALL.len();

/// Translates raw device events through a [`DeviceProfile`].
///
/// Construction allocates (the keyboard tables are sorted vectors); the
/// per-event methods never allocate. Where a profile built by hand binds one
/// raw input twice, the first binding wins.
#[derive(Clone, Debug)]
pub struct ProfileMapper {
    /// `(scancode, input)`, sorted by scancode, unique.
    keys: Vec<(u32, CabinetInput)>,
    buttons: [Option<CabinetInput>; BUTTONS],
    axes: [Option<AxisBinding>; AXES],
    axis_buttons: [Option<CabinetInput>; AXES],
    nudge_keys: Vec<(u32, NudgeDirection)>,
    nudge_buttons: [Option<NudgeDirection>; BUTTONS],
    nudge_axes: [Option<NudgeDirection>; AXES],
}

/// Threshold at which a raw axis bound as a button counts as pressed.
pub const AXIS_BUTTON_THRESHOLD: f32 = 0.5;

impl ProfileMapper {
    /// Builds the lookup tables for `p`.
    pub fn new(p: &DeviceProfile) -> Self {
        let mut m = ProfileMapper {
            keys: Vec::new(),
            buttons: [None; BUTTONS],
            axes: [None; AXES],
            axis_buttons: [None; AXES],
            nudge_keys: Vec::new(),
            nudge_buttons: [None; BUTTONS],
            nudge_axes: [None; AXES],
        };
        for b in &p.axes {
            m.axes[b.raw as usize].get_or_insert(*b);
        }
        for &(raw, input) in &p.buttons {
            match raw {
                RawInput::Key(k) => insert_sorted(&mut m.keys, k, input),
                RawInput::Button(b) => {
                    m.buttons[b as usize].get_or_insert(input);
                }
                RawInput::Axis(a) => {
                    m.axis_buttons[a as usize].get_or_insert(input);
                }
            }
        }
        for &(raw, dir) in &p.nudge_buttons {
            match raw {
                RawInput::Key(k) => insert_sorted(&mut m.nudge_keys, k, dir),
                RawInput::Button(b) => {
                    m.nudge_buttons[b as usize].get_or_insert(dir);
                }
                RawInput::Axis(a) => {
                    m.nudge_axes[a as usize].get_or_insert(dir);
                }
            }
        }
        m
    }

    /// A keyboard key changed state.
    pub fn key(&self, ts_ns: u64, scancode: u32, pressed: bool) -> Option<InputEvent> {
        let input = lookup(&self.keys, scancode)?;
        Some(InputEvent::button(ts_ns, input, pressed))
    }

    /// A gamepad button changed state.
    pub fn button(&self, ts_ns: u64, b: GamepadButton, pressed: bool) -> Option<InputEvent> {
        let input = self.buttons[b as usize]?;
        Some(InputEvent::button(ts_ns, input, pressed))
    }

    /// A gamepad axis moved (`raw` is SDL's `i16` reading). An analog binding
    /// yields an axis event; an axis bound as a button yields a button event
    /// (pressed at a normalised value ≥ 0.5).
    pub fn axis(&self, ts_ns: u64, a: GamepadAxis, raw: i16) -> Option<InputEvent> {
        let v = a.normalise(raw);
        if let Some(b) = &self.axes[a as usize] {
            return Some(InputEvent::axis(ts_ns, b.axis, b.map(v)));
        }
        let input = self.axis_buttons[a as usize]?;
        Some(InputEvent::button(ts_ns, input, v >= AXIS_BUTTON_THRESHOLD))
    }

    /// The digital nudge direction bound to `raw`, if any.
    pub fn nudge_button(&self, raw: RawInput) -> Option<NudgeDirection> {
        match raw {
            RawInput::Key(k) => lookup(&self.nudge_keys, k),
            RawInput::Button(b) => self.nudge_buttons[b as usize],
            RawInput::Axis(a) => self.nudge_axes[a as usize],
        }
    }

    /// The cabinet input bound to `raw` (digital bindings only).
    pub fn input_for(&self, raw: RawInput) -> Option<CabinetInput> {
        match raw {
            RawInput::Key(k) => lookup(&self.keys, k),
            RawInput::Button(b) => self.buttons[b as usize],
            RawInput::Axis(a) => self.axis_buttons[a as usize],
        }
    }
}

fn insert_sorted<T: Copy>(v: &mut Vec<(u32, T)>, k: u32, t: T) {
    if let Err(i) = v.binary_search_by_key(&k, |e| e.0) {
        v.insert(i, (k, t));
    }
}

fn lookup<T: Copy>(v: &[(u32, T)], k: u32) -> Option<T> {
    v.binary_search_by_key(&k, |e| e.0).ok().map(|i| v[i].1)
}
