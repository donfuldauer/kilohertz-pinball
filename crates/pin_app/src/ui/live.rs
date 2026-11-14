//! Live input readings shown by the input panel.
//!
//! 3.2 review N5: `ProfileMapper` is last-writer-wins per cabinet input, so
//! with two devices bound to the same input (keyboard + gamepad) a release on
//! one would hide a press on the other. The monitor keeps one pressed bit per
//! *source* (device) per cabinet input and shows the OR, so the panel's live
//! display is correct whatever the mapper does. Wiring the same OR into the
//! platform layer is 6.0's.

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
    /// Per cabinet input (`CabinetInput::index`), bit `s` = source `s` holds it.
    sources: [u32; CabinetInput::COUNT],
    /// Latest axis values in `CabinetAxis::ALL` order.
    axes: [f32; 5],
    plunger: PlungerReading,
    nudge: [f32; 2],
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
        let bit = 1u32 << source.min(MAX_INPUT_SOURCES - 1);
        let slot = &mut self.sources[input.index()];
        if pressed {
            *slot |= bit;
        } else {
            *slot &= !bit;
        }
    }

    /// Whether any source holds `input` (the OR of the sources).
    pub fn is_pressed(&self, input: CabinetInput) -> bool {
        self.sources[input.index()] != 0
    }

    /// Bit `CabinetInput::index` set for every input any source holds (the
    /// `CabinetFrame::buttons` layout).
    pub fn pressed_mask(&self) -> u8 {
        CabinetInput::ALL
            .iter()
            .filter(|i| self.is_pressed(**i))
            .fold(0u8, |m, i| m | (1 << i.index()))
    }

    /// Releases everything `source` held (device disconnected).
    pub fn clear_source(&mut self, source: u8) {
        let bit = 1u32 << source.min(MAX_INPUT_SOURCES - 1);
        for s in &mut self.sources {
            *s &= !bit;
        }
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
