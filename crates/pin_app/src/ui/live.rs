//! Live input readings shown by the input panel.
//!
//! 3.2 review N5: `ProfileMapper` is last-writer-wins per cabinet input, so
//! with two devices bound to the same input (keyboard + gamepad) a release on
//! one would hide a press on the other. The monitor keeps the set of
//! *sources* holding each cabinet input and shows the OR, so the panel's live
//! display is correct whatever the mapper does. It is built on
//! [`HeldInputs`](crate::platform::HeldInputs), the same type the platform
//! layer's input path (`pin_app::platform::InputRouter`, issue #9b) uses to
//! forward a cabinet edge to the sim only when the first source presses or
//! the last one releases.

use crate::platform::HeldInputs;
use pin_core::input::{CabinetAxis, CabinetInput};
use pin_input::PlungerOutput;

/// Most distinct input sources (devices) the monitor tracks; source ids are
/// `0..MAX_INPUT_SOURCES`.
pub const MAX_INPUT_SOURCES: u8 = 32;

/// Live plunger reading.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct PlungerReading {
    /// Raw normalised axis value.
    pub raw: f32,
    /// Filtered position, 0..=1.
    pub position: f32,
    /// Tracked rest position.
    pub rest: f32,
    /// Speed of the last detected release, full-scale/s.
    pub last_release: Option<f32>,
}

/// Live button, axis, plunger and nudge readings. Fixed size, no allocation.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct InputMonitor {
    /// Which sources hold each cabinet input (source id = the `u8` source).
    held: HeldInputs,
    /// Latest axis values in `CabinetAxis::ALL` order.
    axes: [f32; 5],
    plunger: PlungerReading,
    nudge: [f32; 2],
}

/// The [`HeldInputs`] source id of monitor source `source` (sources at or
/// above [`MAX_INPUT_SOURCES`] share the last one).
fn source_slot(source: u8) -> u32 {
    u32::from(source.min(MAX_INPUT_SOURCES - 1))
}

fn axis_slot(a: CabinetAxis) -> usize {
    CabinetAxis::ALL.iter().position(|x| *x == a).unwrap_or(0)
}

impl InputMonitor {
    /// An empty monitor (nothing pressed, axes 0).
    pub fn new() -> Self {
        Self::default()
    }

    /// Records that `source` pressed or released `input`. Sources at or above
    /// [`MAX_INPUT_SOURCES`] share the last slot.
    pub fn set_button(&mut self, source: u8, input: CabinetInput, pressed: bool) {
        let _ = self.held.set(source_slot(source), input, pressed);
    }

    /// Whether any source holds `input` (the OR of the sources).
    pub fn is_pressed(&self, input: CabinetInput) -> bool {
        self.held.is_held(input)
    }

    /// Bit `CabinetInput::index` set for every input any source holds (the
    /// `CabinetFrame::buttons` layout).
    pub fn pressed_mask(&self) -> u8 {
        self.held.pressed_mask()
    }

    /// Releases everything `source` held (device disconnected).
    pub fn clear_source(&mut self, source: u8) {
        let _ = self.held.release_source(source_slot(source));
    }

    /// Records an axis value.
    pub fn set_axis(&mut self, axis: CabinetAxis, value: f32) {
        self.axes[axis_slot(axis)] = value;
        if axis == CabinetAxis::Plunger {
            self.plunger.raw = value;
        }
    }

    /// The latest value of `axis`.
    pub fn axis(&self, axis: CabinetAxis) -> f32 {
        self.axes[axis_slot(axis)]
    }

    /// Records the plunger filter's output for the latest raw reading.
    pub fn set_plunger(&mut self, out: PlungerOutput, rest: f32) {
        self.plunger.position = out.position;
        self.plunger.rest = rest;
        if out.release.is_some() {
            self.plunger.last_release = out.release;
        }
    }

    /// The live plunger reading.
    pub fn plunger(&self) -> PlungerReading {
        self.plunger
    }

    /// Records the nudge filter's output `[x, y]`.
    pub fn set_nudge(&mut self, nudge: [f32; 2]) {
        self.nudge = nudge;
    }

    /// The latest filtered nudge `[x, y]`.
    pub fn nudge(&self) -> [f32; 2] {
        self.nudge
    }
}
