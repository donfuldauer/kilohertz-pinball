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
    AXIS_BUTTON_THRESHOLD, DeviceProfile, DigitalNudge, GamepadAxis, GamepadButton, NudgeDirection,
    PlungerConfig, PlungerFilter, ProfileMapper, RawInput, SdlGuid, detect,
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

const AXES: usize = GamepadAxis::ALL.len();

/// Releases owed after a device lost its holds (profile swap), flushed with
/// the timestamp of the next router call.
const MAX_PENDING: usize = CabinetInput::COUNT + CabinetAxis::ALL.len();

#[derive(Copy, Clone, Debug, PartialEq)]
enum Pending {
    Button(CabinetInput),
    /// A cabinet axis goes neutral (0.0).
    Axis(CabinetAxis),
    /// A cabinet axis takes the last value of a pad that still drives it.
    AxisValue(CabinetAxis, f32),
}

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
    /// Per cabinet axis (indexed by [`axis_slot`]), the non-zero value this
    /// pad last emitted for it; `None` when it never drove the axis or its
    /// last emission was 0. Several raw axes may feed one cabinet axis, so
    /// this is per cabinet axis, not per raw axis.
    emitted: [Option<f32>; CabinetAxis::ALL.len()],
    /// Per raw axis, whether it is above the nudge threshold (edge state).
    nudge_down: [bool; AXES],
}

/// Position of `axis` in [`CabinetAxis::ALL`].
fn axis_slot(axis: CabinetAxis) -> usize {
    CabinetAxis::ALL
        .iter()
        .position(|a| *a == axis)
        .unwrap_or(0)
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
    pending: ArrayVec<Pending, MAX_PENDING>,
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
            pending: ArrayVec::new(),
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
    ///
    /// Replacing the profile of a routed gamepad first releases everything
    /// that gamepad holds (buttons it alone holds; analog axes it drove go
    /// neutral, or take the value another pad still driving them last
    /// emitted); holds of other devices are kept. The
    /// release events are sent with the timestamp of the next router call
    /// ([`poll`](Self::poll), a key, button, axis or removal).
    pub fn add_gamepad_with_profile(&mut self, which: u32, profile: &DeviceProfile) -> Option<u32> {
        if let Some(i) = self.pads.iter().position(|p| p.which == which) {
            let device = self.pads[i].device;
            let emitted = self.pads[i].emitted;
            for input in self.held.release_all_for_device(device) {
                self.queue(Pending::Button(input));
            }
            self.queue_neutral_axes(device, &emitted);
            let p = &mut self.pads[i];
            p.emitted = [None; CabinetAxis::ALL.len()];
            p.nudge_down = [false; AXES];
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
            emitted: [None; CabinetAxis::ALL.len()],
            nudge_down: [false; AXES],
        });
        Some(device)
    }

    /// Queues `p`. A button release is dropped when already pending. An
    /// axis event replaces any earlier pending event for the same cabinet
    /// axis (the later value wins; neutral and valued events coalesce), so at
    /// most one entry per axis exists and [`MAX_PENDING`] cannot overflow.
    fn queue(&mut self, p: Pending) {
        let axis = match p {
            Pending::Axis(a) | Pending::AxisValue(a, _) => a,
            Pending::Button(_) => {
                if !self.pending.contains(&p) {
                    let _ = self.pending.try_push(p);
                }
                return;
            }
        };
        let earlier = self.pending.iter_mut().find(|q| match q {
            Pending::Axis(a) | Pending::AxisValue(a, _) => *a == axis,
            Pending::Button(_) => false,
        });
        match earlier {
            Some(q) => *q = p,
            None => {
                let _ = self.pending.try_push(p);
            }
        }
    }

    /// Queues, for each cabinet axis in `emitted` (driven by the leaving
    /// `device`: its last emitted value is non-zero), the value the axis must
    /// take now: the value another routed pad that still drives it last
    /// emitted for that cabinet axis, else neutral 0.0. No per-update
    /// recency is stored, so when several other pads drive the axis the one
    /// with the lowest device number wins (deterministic).
    fn queue_neutral_axes(&mut self, device: u32, emitted: &[Option<f32>; CabinetAxis::ALL.len()]) {
        for (slot, _) in emitted.iter().enumerate().filter(|(_, v)| v.is_some()) {
            let cab = CabinetAxis::ALL[slot];
            let remaining = self
                .pads
                .iter()
                .filter(|p| p.device != device)
                .filter_map(|p| p.emitted[slot].map(|v| (p.device, v)))
                .min_by_key(|(d, _)| *d);
            match remaining {
                Some((_, v)) => self.queue(Pending::AxisValue(cab, v)),
                None => self.queue(Pending::Axis(cab)),
            }
        }
    }

    fn flush_pending(&mut self, ts_ns: u64, out: &mut impl FnMut(InputEvent)) {
        for p in std::mem::take(&mut self.pending) {
            match p {
                Pending::Button(input) => {
                    self.stats.edges += 1;
                    out(InputEvent::button(ts_ns, input, false));
                }
                Pending::Axis(axis) => {
                    self.stats.axes += 1;
                    out(InputEvent::axis(ts_ns, axis, 0.0));
                }
                Pending::AxisValue(axis, value) => {
                    self.stats.axes += 1;
                    out(InputEvent::axis(ts_ns, axis, value));
                }
            }
        }
    }

    /// Stops routing gamepad `which` and releases every cabinet input only
    /// it held; analog cabinet axes it drove go neutral (0), or take the value
    /// another routed gamepad that still drives them last emitted (lowest
    /// device number if several do). Events at `ts_ns` go to `out`.
    pub fn remove_gamepad(&mut self, ts_ns: u64, which: u32, out: &mut impl FnMut(InputEvent)) {
        self.flush_pending(ts_ns, out);
        let Some(i) = self.pads.iter().position(|p| p.which == which) else {
            return;
        };
        let pad = self.pads.remove(i);
        for input in self.held.release_all_for_device(pad.device) {
            self.stats.edges += 1;
            out(InputEvent::button(ts_ns, input, false));
        }
        self.queue_neutral_axes(pad.device, &pad.emitted);
        self.flush_pending(ts_ns, out);
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
        self.flush_pending(now_ns, out);
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
        self.flush_pending(ts_ns, out);
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
        self.flush_pending(ts_ns, out);
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
        self.flush_pending(ts_ns, out);
        let Some(i) = self.pad_index(which) else {
            return;
        };
        let device = self.pads[i].device;
        let mapped = self.pads[i].mapper.axis(ts_ns, a, raw);
        let nudge = self.pads[i].mapper.nudge_button(RawInput::Axis(a));
        if let Some(ev) = mapped {
            match ev.kind {
                InputEventKind::Button { input, pressed } => {
                    let source = source_id(device, raw_code(RawInput::Axis(a)));
                    self.digital(ts_ns, source, input, pressed, out);
                }
                InputEventKind::Axis { axis, value } => {
                    let sent = if axis == CabinetAxis::Plunger {
                        self.plunger.update(value, ts_ns / NS_PER_MS).position
                    } else {
                        value
                    };
                    self.pads[i].emitted[axis_slot(axis)] = (sent != 0.0).then_some(sent);
                    self.stats.axes += 1;
                    out(InputEvent::axis(ts_ns, axis, sent));
                }
            }
        }
        // A nudge bound to an axis fires once per crossing of the threshold.
        let down = a.normalise(raw) >= AXIS_BUTTON_THRESHOLD;
        let was_down = std::mem::replace(&mut self.pads[i].nudge_down[a as usize], down);
        if down
            && !was_down
            && let Some(dir) = nudge
        {
            self.nudge_press(ts_ns, dir, out);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn flushed(r: &mut InputRouter) -> Vec<Pending> {
        let v: Vec<Pending> = r.pending.iter().copied().collect();
        r.pending.clear();
        v
    }

    #[test]
    fn queue_replaces_earlier_axis_events_of_the_same_axis() {
        let mut r = InputRouter::new();
        let ax = CabinetAxis::LeftFlipper;
        r.queue(Pending::Axis(ax));
        r.queue(Pending::AxisValue(ax, 0.5));
        assert_eq!(flushed(&mut r), [Pending::AxisValue(ax, 0.5)]);
        r.queue(Pending::AxisValue(ax, 0.5));
        r.queue(Pending::AxisValue(ax, 0.25));
        r.queue(Pending::AxisValue(ax, 0.5));
        assert_eq!(flushed(&mut r), [Pending::AxisValue(ax, 0.5)]);
        r.queue(Pending::AxisValue(ax, 0.5));
        r.queue(Pending::Axis(ax));
        assert_eq!(flushed(&mut r), [Pending::Axis(ax)]);
    }

    #[test]
    fn queue_keeps_axes_apart_and_dedups_buttons() {
        let mut r = InputRouter::new();
        let b = CabinetInput::LeftFlipper;
        r.queue(Pending::Button(b));
        r.queue(Pending::Button(b));
        r.queue(Pending::Axis(CabinetAxis::LeftFlipper));
        r.queue(Pending::Axis(CabinetAxis::Plunger));
        assert_eq!(flushed(&mut r).len(), 3);
        for a in CabinetAxis::ALL {
            r.queue(Pending::Axis(a));
            r.queue(Pending::AxisValue(a, 1.0));
        }
        for i in CabinetInput::ALL {
            r.queue(Pending::Button(i));
        }
        assert_eq!(r.pending.len(), MAX_PENDING);
    }
}
