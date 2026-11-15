//! [`InputRouter`]: raw SDL input → device profile → [`HeldInputs`] → cabinet
//! events for the sim (PLAN §2.2, Subphases 3.2 and 6.0; issue #9b).
//!
//! * Each device gets a `pin_input` [`ProfileMapper`] picked by
//!   [`pin_input::detect`] (keyboard: `generic_keyboard`; gamepads by USB
//!   vendor/product, else `generic_gamepad`), or one set explicitly.
//! * Digital cabinet edges go through [`HeldInputs`]: an edge is forwarded
//!   only when the first raw input presses a cabinet input or the last one
//!   releases it, so aliases (Space and Return both launch; keyboard and
//!   gamepad both flip) hold until the last release. Key repeats are ignored.
//! * The plunger axis passes through a [`PlungerFilter`] (rest tracking,
//!   deadzone) before it reaches the sim; profile nudge buttons drive a
//!   [`DigitalNudge`] whose vector is emitted as `NudgeX`/`NudgeY` axis
//!   events whenever it changes ([`InputRouter::poll`] ends a nudge when no
//!   other event arrives). Analog axes (triggers, thumbstick nudge) pass
//!   through the profile's axis mapping.
//! * A disconnected gamepad releases everything it held (and only that).
//!
//! Devices are numbered for [`source_id`]: the keyboard is
//! [`KEYBOARD_DEVICE`], a connected gamepad gets the lowest free number from
//! 1, so two connected devices never share a source id.
//!
//! Nothing here calls SDL: tests build `sdl3::event::Event` values directly.
//! Per-event routing does not allocate; connecting a gamepad builds its
//! mapper (allocates, main thread only).

use super::held_inputs::{HeldInputs, RAW_AXIS, RAW_BUTTON, RAW_KEY, source_id};
use arrayvec::ArrayVec;
use pin_core::input::{CabinetAxis, CabinetInput, InputEvent, InputEventKind};
use pin_input::{
    DeviceProfile, DigitalNudge, GamepadAxis, GamepadButton, NudgeDirection, PlungerConfig,
    PlungerFilter, ProfileMapper, RawInput, SdlGuid, detect,
};
use sdl3::event::Event;

/// Device number of the keyboard (all keyboards share it, as SDL merges
/// them into one key state).
pub const KEYBOARD_DEVICE: u32 = 0;

/// Gamepads routed at the same time; further ones are ignored until one
/// disconnects.
pub const MAX_ROUTED_GAMEPADS: usize = 16;

/// Nudge vector component of one digital nudge press (`-1..=1` axis units).
pub const DIGITAL_NUDGE_STRENGTH: f32 = 0.6;

/// How long one digital nudge press pushes, ms.
pub const DIGITAL_NUDGE_MS: u32 = 60;

const NS_PER_MS: u64 = 1_000_000;

/// The raw-input code of `raw` inside a [`source_id`].
pub const fn raw_code(raw: RawInput) -> u32 {
    match raw {
        RawInput::Key(k) => RAW_KEY + (k & 0x0FFF),
        RawInput::Button(b) => RAW_BUTTON + b as u32,
        RawInput::Axis(a) => RAW_AXIS + a as u32,
    }
}

/// The `pin_input` button of an SDL gamepad button (SDL order; buttons
/// past `Touchpad` have no profile name).
pub fn gamepad_button(b: sdl3::gamepad::Button) -> Option<GamepadButton> {
    usize::try_from(b as i32)
        .ok()
        .and_then(|i| GamepadButton::ALL.get(i).copied())
}

/// The `pin_input` axis of an SDL gamepad axis.
pub fn gamepad_axis(a: sdl3::gamepad::Axis) -> Option<GamepadAxis> {
    usize::try_from(a as i32)
        .ok()
        .and_then(|i| GamepadAxis::ALL.get(i).copied())
}

struct Pad {
    /// SDL joystick instance id.
    which: u32,
    device: u32,
    profile: String,
    mapper: ProfileMapper,
}

/// Counters kept by [`InputRouter`].
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct RouterStats {
    /// Cabinet button edges forwarded.
    pub edges: u64,
    /// Raw digital edges absorbed by [`HeldInputs`] (an alias already held,
    /// a repeat, a release of an unheld source).
    pub absorbed: u64,
    /// Axis events forwarded.
    pub axes: u64,
    /// Gamepads not routed because [`MAX_ROUTED_GAMEPADS`] were connected.
    pub pads_ignored: u64,
}

/// Raw input → cabinet events (module docs).
pub struct InputRouter {
    keyboard: ProfileMapper,
    keyboard_profile: String,
    pads: ArrayVec<Pad, MAX_ROUTED_GAMEPADS>,
    held: HeldInputs,
    plunger: PlungerFilter,
    nudge: DigitalNudge,
    nudge_out: [f32; 2],
    stats: RouterStats,
}

impl Default for InputRouter {
    fn default() -> Self {
        Self::new()
    }
}

impl InputRouter {
    /// The keyboard on its auto-detected profile, no gamepads.
    pub fn new() -> Self {
        Self::with_keyboard_profile(detect(None, true))
    }

    /// The keyboard on `profile`, no gamepads.
    pub fn with_keyboard_profile(profile: &DeviceProfile) -> Self {
        Self {
            keyboard: ProfileMapper::new(profile),
            keyboard_profile: profile.id.clone(),
            pads: ArrayVec::new(),
            held: HeldInputs::new(),
            plunger: PlungerFilter::new(PlungerConfig::default()),
            nudge: DigitalNudge::new(DIGITAL_NUDGE_STRENGTH, DIGITAL_NUDGE_MS),
            nudge_out: [0.0; 2],
            stats: RouterStats::default(),
        }
    }

    /// The keyboard's profile id.
    pub fn keyboard_profile(&self) -> &str {
        &self.keyboard_profile
    }

    /// Routes gamepad `which` (SDL joystick instance id) with the profile
    /// [`detect`] picks for `guid`. Returns its device number, `None` when
    /// [`MAX_ROUTED_GAMEPADS`] are connected. A gamepad already routed keeps
    /// its number and gets the new profile.
    pub fn add_gamepad(&mut self, which: u32, guid: Option<SdlGuid>) -> Option<u32> {
        self.add_gamepad_with_profile(which, detect(guid, false))
    }

    /// [`add_gamepad`](Self::add_gamepad) with an explicit profile.
    pub fn add_gamepad_with_profile(&mut self, which: u32, profile: &DeviceProfile) -> Option<u32> {
        if let Some(p) = self.pads.iter_mut().find(|p| p.which == which) {
            p.mapper = ProfileMapper::new(profile);
            p.profile.clone_from(&profile.id);
            return Some(p.device);
        }
        if self.pads.is_full() {
            self.stats.pads_ignored += 1;
            return None;
        }
        let device = (1..)
            .find(|d| self.pads.iter().all(|p| p.device != *d))
            .unwrap_or(1);
        self.pads.push(Pad {
            which,
            device,
            profile: profile.id.clone(),
            mapper: ProfileMapper::new(profile),
        });
        Some(device)
    }

    /// Stops routing gamepad `which` and releases every cabinet input only
    /// it held (release events at `ts_ns` go to `out`).
    pub fn remove_gamepad(&mut self, ts_ns: u64, which: u32, out: &mut impl FnMut(InputEvent)) {
        let Some(i) = self.pads.iter().position(|p| p.which == which) else {
            return;
        };
        let pad = self.pads.remove(i);
        for input in self.held.release_all_for_device(pad.device) {
            self.stats.edges += 1;
            out(InputEvent::button(ts_ns, input, false));
        }
    }

    /// Device number and profile id of gamepad `which`.
    pub fn gamepad(&self, which: u32) -> Option<(u32, &str)> {
        self.pads
            .iter()
            .find(|p| p.which == which)
            .map(|p| (p.device, p.profile.as_str()))
    }

    /// Who holds what.
    pub fn held(&self) -> &HeldInputs {
        &self.held
    }

    /// Counters so far.
    pub fn stats(&self) -> RouterStats {
        self.stats
    }

    /// The pad of `which`, routing an unknown gamepad on the generic
    /// profile (an event can precede `GamepadAdded` handling).
    fn pad_index(&mut self, which: u32) -> Option<usize> {
        if let Some(i) = self.pads.iter().position(|p| p.which == which) {
            return Some(i);
        }
        self.add_gamepad(which, None)?;
        self.pads.iter().position(|p| p.which == which)
    }

    fn digital(
        &mut self,
        ts_ns: u64,
        source: u32,
        input: CabinetInput,
        pressed: bool,
        out: &mut impl FnMut(InputEvent),
    ) {
        match self.held.set(source, input, pressed) {
            Some(p) => {
                self.stats.edges += 1;
                out(InputEvent::button(ts_ns, input, p));
            }
            None => self.stats.absorbed += 1,
        }
    }

    fn nudge_press(&mut self, ts_ns: u64, dir: NudgeDirection, out: &mut impl FnMut(InputEvent)) {
        self.nudge.press(dir, ts_ns / NS_PER_MS);
        self.poll(ts_ns, out);
    }

    /// Emits `NudgeX`/`NudgeY` events where the digital nudge vector at
    /// `now_ns` differs from the last one sent (call once per main-thread
    /// iteration so a nudge ends on time).
    pub fn poll(&mut self, now_ns: u64, out: &mut impl FnMut(InputEvent)) {
        let v = self.nudge.sample(now_ns / NS_PER_MS);
        for (k, axis) in [CabinetAxis::NudgeX, CabinetAxis::NudgeY]
            .into_iter()
            .enumerate()
        {
            if v[k].to_bits() != self.nudge_out[k].to_bits() {
                self.nudge_out[k] = v[k];
                self.stats.axes += 1;
                out(InputEvent::axis(now_ns, axis, v[k]));
            }
        }
    }

    /// A key changed state (`scancode` = `SDL_Scancode`).
    pub fn key(
        &mut self,
        ts_ns: u64,
        scancode: u32,
        pressed: bool,
        out: &mut impl FnMut(InputEvent),
    ) {
        let raw = RawInput::Key(scancode);
        if let Some(ev) = self.keyboard.key(ts_ns, scancode, pressed)
            && let InputEventKind::Button { input, pressed } = ev.kind
        {
            let source = source_id(KEYBOARD_DEVICE, raw_code(raw));
            self.digital(ts_ns, source, input, pressed, out);
        }
        if pressed && let Some(dir) = self.keyboard.nudge_button(raw) {
            self.nudge_press(ts_ns, dir, out);
        }
    }

    /// A button of gamepad `which` changed state.
    pub fn button(
        &mut self,
        ts_ns: u64,
        which: u32,
        b: GamepadButton,
        pressed: bool,
        out: &mut impl FnMut(InputEvent),
    ) {
        let Some(i) = self.pad_index(which) else {
            return;
        };
        let raw = RawInput::Button(b);
        let device = self.pads[i].device;
        let mapped = self.pads[i].mapper.button(ts_ns, b, pressed);
        let nudge = self.pads[i].mapper.nudge_button(raw);
        if let Some(ev) = mapped
            && let InputEventKind::Button { input, pressed } = ev.kind
        {
            self.digital(ts_ns, source_id(device, raw_code(raw)), input, pressed, out);
        }
        if pressed && let Some(dir) = nudge {
            self.nudge_press(ts_ns, dir, out);
        }
    }

    /// An axis of gamepad `which` moved (`raw` = SDL's `i16` reading).
    pub fn axis(
        &mut self,
        ts_ns: u64,
        which: u32,
        a: GamepadAxis,
        raw: i16,
        out: &mut impl FnMut(InputEvent),
    ) {
        let Some(i) = self.pad_index(which) else {
            return;
        };
        let device = self.pads[i].device;
        let Some(ev) = self.pads[i].mapper.axis(ts_ns, a, raw) else {
            return;
        };
        match ev.kind {
            InputEventKind::Button { input, pressed } => {
                let source = source_id(device, raw_code(RawInput::Axis(a)));
                self.digital(ts_ns, source, input, pressed, out);
            }
            InputEventKind::Axis {
                axis: CabinetAxis::Plunger,
                value,
            } => {
                let f = self.plunger.update(value, ts_ns / NS_PER_MS);
                self.stats.axes += 1;
                out(InputEvent::axis(ts_ns, CabinetAxis::Plunger, f.position));
            }
            InputEventKind::Axis { .. } => {
                self.stats.axes += 1;
                out(ev);
            }
        }
    }

    /// Routes one SDL event; returns `true` when it was a key, gamepad
    /// button, gamepad axis or gamepad removal (handled here, whether or not
    /// it produced a cabinet event). Key repeats are ignored.
    pub fn route(&mut self, event: &Event, out: &mut impl FnMut(InputEvent)) -> bool {
        let ts = event.get_timestamp();
        match *event {
            Event::KeyDown {
                scancode: Some(sc),
                repeat,
                ..
            } => {
                if !repeat {
                    self.key(ts, sc as u32, true, out);
                }
                true
            }
            Event::KeyUp {
                scancode: Some(sc),
                repeat,
                ..
            } => {
                if !repeat {
                    self.key(ts, sc as u32, false, out);
                }
                true
            }
            Event::GamepadButtonDown { which, button, .. }
            | Event::GamepadButtonUp { which, button, .. } => {
                let pressed = matches!(event, Event::GamepadButtonDown { .. });
                if let Some(b) = gamepad_button(button) {
                    self.button(ts, which.raw(), b, pressed, out);
                }
                true
            }
            Event::GamepadAxisMotion {
                which, axis, value, ..
            } => {
                if let Some(a) = gamepad_axis(axis) {
                    self.axis(ts, which.raw(), a, value, out);
                }
                true
            }
            Event::GamepadRemoved { which, .. } => {
                self.remove_gamepad(ts, which.raw(), out);
                true
            }
            _ => false,
        }
    }
}
