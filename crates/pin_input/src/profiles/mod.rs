//! X-Arcade and other device profiles.
//!
//! Owner: Subphase 3.2.
//!
//! A [`DeviceProfile`] binds raw device inputs to canonical cabinet inputs
//! ([`CabinetInput`], [`CabinetAxis`]) and to digital nudge directions. Raw
//! inputs use SDL's numbering without depending on SDL: keyboard scancodes are
//! `SDL_Scancode` values, gamepad buttons and axes follow the SDL standard
//! gamepad order, and devices are recognised by their SDL joystick GUID
//! ([`SdlGuid`], vendor/product ids).
//!
//! Profiles are TOML files. The built-in ones live in `crates/pin_input/profiles/`
//! and are embedded with `include_str!`; [`builtin_profiles`] parses them on
//! first use. [`detect`] picks a built-in profile for a newly connected device
//! and [`ProfileMapper`] translates raw events with no allocation per event.
//!
//! # Profile file format
//!
//! ```toml
//! id = "my_pad"            # required, non-empty
//! name = "My Pad"          # required, display name
//! # Devices this profile is auto-selected for (optional, default: none).
//! # Each entry has exactly one key:
//! #   { gamepad = { vendor = 0x1234, product = 0x5678 } }  exact USB vendor/product
//! #   { any_gamepad = true }                                 any gamepad
//! #   { keyboard = true }                                    the keyboard
//! match = [ { gamepad = { vendor = 0x1234, product = 0x5678 } } ]
//!
//! # Cabinet input name -> one raw input or a list of raw inputs.
//! # Cabinet names: left_flipper, right_flipper, left_upper, right_upper,
//! #                action, start, launch, tilt.
//! [buttons]
//! left_flipper = [ "key:225", "button:left_shoulder" ]
//! launch = "button:south"
//!
//! # Analog bindings, one table per raw axis.
//! [[axes]]
//! raw = "left_trigger"     # or "axis:left_trigger"
//! axis = "left_flipper"    # plunger, nudge_x, nudge_y, left_flipper, right_flipper
//! from = 0.0               # optional, default: lower end of the raw range (-1 stick, 0 trigger)
//! to = 1.0                 # optional, default 1.0; must differ from `from`
//! invert = false           # optional, default false
//!
//! # Digital nudge: direction -> raw inputs (left, right, forward).
//! [nudge]
//! left = [ "key:224" ]
//! ```
//!
//! Raw input specs:
//! * `key:<n>`: `SDL_Scancode` number `n` (decimal, or `0x` hex),
//! * `button:<name>`: SDL standard gamepad button (`south`, `east`, `west`,
//!   `north`, `back`, `guide`, `start`, `left_stick`, `right_stick`,
//!   `left_shoulder`, `right_shoulder`, `dpad_up`, `dpad_down`, `dpad_left`,
//!   `dpad_right`, `misc1`, `right_paddle1`, `left_paddle1`, `right_paddle2`,
//!   `left_paddle2`, `touchpad`),
//! * `axis:<name>`: SDL standard gamepad axis (`left_x`, `left_y`, `right_x`,
//!   `right_y`, `left_trigger`, `right_trigger`); used as a button it is
//!   pressed while its normalised value is at least 0.5.
//!
//! Axis mapping: the raw reading is normalised (sticks `raw / 32767` clamped to
//! `-1..=1`, triggers clamped to `0..=1`). `invert` mirrors it within its raw
//! range (sticks `-v`, triggers `1 - v`). The result is mapped linearly from
//! `[from, to]` onto the cabinet axis range (`0..=1` for plunger and flipper
//! travel, `-1..=1` for nudge) and clamped to it.
//!
//! Files larger than [`MAX_PROFILE_BYTES`] (64 KiB) are rejected.
//! Unknown keys, unknown names, malformed raw specs, wrong value types, a raw
//! input bound twice in one section, and a raw axis used both as an axis and as
//! a button are errors. Syntax errors display as `<line>:<col>: <message>`;
//! semantic errors as `<key>: <message> (line L, column C)` where `<key>` is
//! the dotted path of the offending entry (e.g. `buttons.left_flipper[1]`).

mod mapper;
mod parse;

use pin_core::input::{CabinetAxis, CabinetInput};
use std::fmt;
use std::sync::OnceLock;

pub use mapper::ProfileMapper;
pub use parse::{MAX_PROFILE_BYTES, parse_profile};

/// An SDL joystick GUID (16 bytes, SDL3 layout).
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct SdlGuid(pub [u8; 16]);

impl SdlGuid {
    /// Parses SDL's 32-hex-character GUID string (case-insensitive).
    pub fn parse_hex(s: &str) -> Option<SdlGuid> {
        let b = s.as_bytes();
        if b.len() != 32 {
            return None;
        }
        let mut out = [0u8; 16];
        for (i, o) in out.iter_mut().enumerate() {
            let hi = hex_val(b[2 * i])?;
            let lo = hex_val(b[2 * i + 1])?;
            *o = (hi << 4) | lo;
        }
        Some(SdlGuid(out))
    }

    /// A GUID with the given bus type, vendor and product id and all other
    /// bytes zero (the layout SDL3 builds for USB/Bluetooth devices).
    pub const fn from_ids(bus: u16, vendor: u16, product: u16) -> SdlGuid {
        let bus = bus.to_le_bytes();
        let v = vendor.to_le_bytes();
        let p = product.to_le_bytes();
        let mut g = [0u8; 16];
        g[0] = bus[0];
        g[1] = bus[1];
        g[4] = v[0];
        g[5] = v[1];
        g[8] = p[0];
        g[9] = p[1];
        SdlGuid(g)
    }

    /// Bus type (bytes 0–1, little endian).
    pub const fn bus(&self) -> u16 {
        u16::from_le_bytes([self.0[0], self.0[1]])
    }

    /// USB vendor id (bytes 4–5, little endian).
    pub const fn vendor(&self) -> u16 {
        u16::from_le_bytes([self.0[4], self.0[5]])
    }

    /// USB product id (bytes 8–9, little endian).
    pub const fn product(&self) -> u16 {
        u16::from_le_bytes([self.0[8], self.0[9]])
    }
}

impl fmt::Display for SdlGuid {
    /// Lower-case 32-hex-character form, as `SDL_GUIDToString` writes it.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for b in self.0 {
            write!(f, "{b:02x}")?;
        }
        Ok(())
    }
}

fn hex_val(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// SDL standard gamepad buttons, in `SDL_GamepadButton` order.
#[allow(missing_docs)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GamepadButton {
    South,
    East,
    West,
    North,
    Back,
    Guide,
    Start,
    LeftStick,
    RightStick,
    LeftShoulder,
    RightShoulder,
    DpadUp,
    DpadDown,
    DpadLeft,
    DpadRight,
    Misc1,
    RightPaddle1,
    LeftPaddle1,
    RightPaddle2,
    LeftPaddle2,
    Touchpad,
}

impl GamepadButton {
    /// Every button, in SDL order (`ALL[b as usize] == b`).
    pub const ALL: [GamepadButton; 21] = [
        GamepadButton::South,
        GamepadButton::East,
        GamepadButton::West,
        GamepadButton::North,
        GamepadButton::Back,
        GamepadButton::Guide,
        GamepadButton::Start,
        GamepadButton::LeftStick,
        GamepadButton::RightStick,
        GamepadButton::LeftShoulder,
        GamepadButton::RightShoulder,
        GamepadButton::DpadUp,
        GamepadButton::DpadDown,
        GamepadButton::DpadLeft,
        GamepadButton::DpadRight,
        GamepadButton::Misc1,
        GamepadButton::RightPaddle1,
        GamepadButton::LeftPaddle1,
        GamepadButton::RightPaddle2,
        GamepadButton::LeftPaddle2,
        GamepadButton::Touchpad,
    ];

    /// The profile-file name (`"left_shoulder"`, ...).
    pub const fn name(self) -> &'static str {
        match self {
            GamepadButton::South => "south",
            GamepadButton::East => "east",
            GamepadButton::West => "west",
            GamepadButton::North => "north",
            GamepadButton::Back => "back",
            GamepadButton::Guide => "guide",
            GamepadButton::Start => "start",
            GamepadButton::LeftStick => "left_stick",
            GamepadButton::RightStick => "right_stick",
            GamepadButton::LeftShoulder => "left_shoulder",
            GamepadButton::RightShoulder => "right_shoulder",
            GamepadButton::DpadUp => "dpad_up",
            GamepadButton::DpadDown => "dpad_down",
            GamepadButton::DpadLeft => "dpad_left",
            GamepadButton::DpadRight => "dpad_right",
            GamepadButton::Misc1 => "misc1",
            GamepadButton::RightPaddle1 => "right_paddle1",
            GamepadButton::LeftPaddle1 => "left_paddle1",
            GamepadButton::RightPaddle2 => "right_paddle2",
            GamepadButton::LeftPaddle2 => "left_paddle2",
            GamepadButton::Touchpad => "touchpad",
        }
    }

    /// Inverse of [`GamepadButton::name`].
    pub fn from_name(name: &str) -> Option<GamepadButton> {
        GamepadButton::ALL.into_iter().find(|b| b.name() == name)
    }
}

/// SDL standard gamepad axes, in `SDL_GamepadAxis` order.
#[allow(missing_docs)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GamepadAxis {
    LeftX,
    LeftY,
    RightX,
    RightY,
    LeftTrigger,
    RightTrigger,
}

impl GamepadAxis {
    /// Every axis, in SDL order (`ALL[a as usize] == a`).
    pub const ALL: [GamepadAxis; 6] = [
        GamepadAxis::LeftX,
        GamepadAxis::LeftY,
        GamepadAxis::RightX,
        GamepadAxis::RightY,
        GamepadAxis::LeftTrigger,
        GamepadAxis::RightTrigger,
    ];

    /// The profile-file name (`"left_trigger"`, ...).
    pub const fn name(self) -> &'static str {
        match self {
            GamepadAxis::LeftX => "left_x",
            GamepadAxis::LeftY => "left_y",
            GamepadAxis::RightX => "right_x",
            GamepadAxis::RightY => "right_y",
            GamepadAxis::LeftTrigger => "left_trigger",
            GamepadAxis::RightTrigger => "right_trigger",
        }
    }

    /// Inverse of [`GamepadAxis::name`].
    pub fn from_name(name: &str) -> Option<GamepadAxis> {
        GamepadAxis::ALL.into_iter().find(|a| a.name() == name)
    }

    /// `true` for the two triggers (raw range `0..=1`).
    pub const fn is_trigger(self) -> bool {
        matches!(self, GamepadAxis::LeftTrigger | GamepadAxis::RightTrigger)
    }

    /// Normalises an SDL axis reading: sticks `raw / 32767` clamped to
    /// `-1..=1`, triggers clamped to `0..=1`.
    pub fn normalise(self, raw: i16) -> f32 {
        let v = (f32::from(raw) / 32767.0).clamp(-1.0, 1.0);
        if self.is_trigger() { v.max(0.0) } else { v }
    }

    /// Lower end of the normalised range (`-1` sticks, `0` triggers).
    pub const fn raw_min(self) -> f32 {
        if self.is_trigger() { 0.0 } else { -1.0 }
    }
}

/// A raw device input.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum RawInput {
    /// Keyboard key, by `SDL_Scancode`.
    Key(u32),
    /// Standard gamepad button.
    Button(GamepadButton),
    /// Standard gamepad axis used digitally (pressed at ≥ 0.5).
    Axis(GamepadAxis),
}

impl RawInput {
    /// Parses a raw spec (`key:225`, `button:south`, `axis:left_trigger`).
    pub fn parse(spec: &str) -> Option<RawInput> {
        let (kind, rest) = spec.split_once(':')?;
        match kind {
            "key" => {
                let n = if let Some(hex) = rest.strip_prefix("0x") {
                    if hex.is_empty() || hex.starts_with('+') {
                        return None;
                    }
                    u32::from_str_radix(hex, 16).ok()?
                } else {
                    if rest.is_empty() || !rest.bytes().all(|c| c.is_ascii_digit()) {
                        return None;
                    }
                    rest.parse().ok()?
                };
                Some(RawInput::Key(n))
            }
            "button" => GamepadButton::from_name(rest).map(RawInput::Button),
            "axis" => GamepadAxis::from_name(rest).map(RawInput::Axis),
            _ => None,
        }
    }
}

impl fmt::Display for RawInput {
    /// The profile-file spec ([`RawInput::parse`] round-trips it).
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RawInput::Key(k) => write!(f, "key:{k}"),
            RawInput::Button(b) => write!(f, "button:{}", b.name()),
            RawInput::Axis(a) => write!(f, "axis:{}", a.name()),
        }
    }
}

/// Which devices a profile is auto-selected for.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum DeviceMatch {
    /// The keyboard.
    Keyboard,
    /// A gamepad with this exact USB vendor/product id.
    Gamepad {
        /// USB vendor id.
        vendor: u16,
        /// USB product id.
        product: u16,
    },
    /// Any gamepad.
    AnyGamepad,
}

/// An analog binding: a raw gamepad axis drives a cabinet axis.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct AxisBinding {
    /// Raw axis.
    pub raw: GamepadAxis,
    /// Cabinet axis it drives.
    pub axis: CabinetAxis,
    /// Mirror the raw value within its range (sticks `-v`, triggers `1 - v`).
    pub invert: bool,
    /// Raw value mapped to the low end of the cabinet axis range.
    pub from: f32,
    /// Raw value mapped to the high end of the cabinet axis range.
    pub to: f32,
}

impl AxisBinding {
    /// Maps a normalised raw value (sticks `-1..=1`, triggers `0..=1`) onto the
    /// cabinet axis range. NaN maps to the low end; a degenerate `from == to`
    /// maps to the low end below it and the high end at or above it.
    pub fn map(&self, raw: f32) -> f32 {
        let (lo, hi) = cabinet_axis_range(self.axis);
        let mut v = if raw.is_nan() { 0.0 } else { raw };
        if self.raw.is_trigger() {
            v = v.clamp(0.0, 1.0);
            if self.invert {
                v = 1.0 - v;
            }
        } else {
            v = v.clamp(-1.0, 1.0);
            if self.invert {
                v = -v;
            }
        }
        let span = self.to - self.from;
        let t = if span.is_finite() && span != 0.0 {
            (v - self.from) / span
        } else if v >= self.from {
            1.0
        } else {
            0.0
        };
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        lo + t * (hi - lo)
    }
}

/// The value range of a cabinet axis (`0..=1` travel, `-1..=1` nudge).
pub const fn cabinet_axis_range(axis: CabinetAxis) -> (f32, f32) {
    match axis {
        CabinetAxis::NudgeX | CabinetAxis::NudgeY => (-1.0, 1.0),
        CabinetAxis::Plunger | CabinetAxis::LeftFlipper | CabinetAxis::RightFlipper => (0.0, 1.0),
    }
}

/// The profile-file name of a cabinet axis.
pub const fn cabinet_axis_name(axis: CabinetAxis) -> &'static str {
    match axis {
        CabinetAxis::Plunger => "plunger",
        CabinetAxis::NudgeX => "nudge_x",
        CabinetAxis::NudgeY => "nudge_y",
        CabinetAxis::LeftFlipper => "left_flipper",
        CabinetAxis::RightFlipper => "right_flipper",
    }
}

/// Inverse of [`cabinet_axis_name`].
pub fn cabinet_axis_from_name(name: &str) -> Option<CabinetAxis> {
    CabinetAxis::ALL
        .into_iter()
        .find(|a| cabinet_axis_name(*a) == name)
}

/// Digital nudge direction.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum NudgeDirection {
    /// Push the cabinet to the left.
    Left,
    /// Push the cabinet to the right.
    Right,
    /// Push the cabinet forward (away from the player).
    Forward,
}

impl NudgeDirection {
    /// Every direction.
    pub const ALL: [NudgeDirection; 3] = [
        NudgeDirection::Left,
        NudgeDirection::Right,
        NudgeDirection::Forward,
    ];

    /// The profile-file name (`"left"`, `"right"`, `"forward"`).
    pub const fn name(self) -> &'static str {
        match self {
            NudgeDirection::Left => "left",
            NudgeDirection::Right => "right",
            NudgeDirection::Forward => "forward",
        }
    }

    /// Inverse of [`NudgeDirection::name`].
    pub fn from_name(name: &str) -> Option<NudgeDirection> {
        NudgeDirection::ALL.into_iter().find(|d| d.name() == name)
    }
}

/// A device profile: raw inputs → cabinet inputs.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceProfile {
    /// Stable identifier (`"generic_keyboard"`, ...).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Devices the profile is auto-selected for.
    pub matches: Vec<DeviceMatch>,
    /// Digital bindings, sorted by cabinet input (file order within one input).
    pub buttons: Vec<(RawInput, CabinetInput)>,
    /// Analog bindings, in file order.
    pub axes: Vec<AxisBinding>,
    /// Digital nudge bindings, sorted by direction (file order within one).
    pub nudge_buttons: Vec<(RawInput, NudgeDirection)>,
}

impl DeviceProfile {
    /// `true` when some raw input is bound to `input`.
    pub fn maps(&self, input: CabinetInput) -> bool {
        self.buttons.iter().any(|(_, c)| *c == input)
    }
}

/// A profile parse error.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProfileError {
    /// The text is not valid TOML. Line and column are 1-based (column in
    /// characters).
    Syntax {
        /// 1-based line.
        line: usize,
        /// 1-based column.
        col: usize,
        /// What is wrong.
        message: String,
    },
    /// Valid TOML that is not a valid profile.
    Invalid {
        /// Dotted path of the offending entry (`buttons.left_flipper[1]`).
        key: String,
        /// 1-based line and column of the entry, when known.
        position: Option<(usize, usize)>,
        /// What is wrong.
        message: String,
    },
}

impl fmt::Display for ProfileError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ProfileError::Syntax { line, col, message } => write!(f, "{line}:{col}: {message}"),
            ProfileError::Invalid {
                key,
                position,
                message,
            } => {
                write!(f, "{key}: {message}")?;
                if let Some((line, col)) = position {
                    write!(f, " (line {line}, column {col})")?;
                }
                Ok(())
            }
        }
    }
}

impl std::error::Error for ProfileError {}

/// Built-in profile files: (file name, TOML text). Order matters for
/// [`detect_in`]: generic presets come first so a keyboard gets the generic
/// keyboard preset (a keyboard-encoder X-Arcade cannot be told apart).
pub const BUILTIN_PROFILE_FILES: [(&str, &str); 4] = [
    (
        "generic_keyboard.toml",
        include_str!("../../profiles/generic_keyboard.toml"),
    ),
    (
        "generic_gamepad.toml",
        include_str!("../../profiles/generic_gamepad.toml"),
    ),
    (
        "xarcade_keyboard.toml",
        include_str!("../../profiles/xarcade_keyboard.toml"),
    ),
    (
        "xarcade_gamepad.toml",
        include_str!("../../profiles/xarcade_gamepad.toml"),
    ),
];

/// The built-in profiles, parsed on first use. Ids: `generic_keyboard`,
/// `generic_gamepad`, `xarcade_keyboard`, `xarcade_gamepad`.
pub fn builtin_profiles() -> &'static [DeviceProfile] {
    static PROFILES: OnceLock<Vec<DeviceProfile>> = OnceLock::new();
    PROFILES.get_or_init(|| {
        BUILTIN_PROFILE_FILES
            .iter()
            .map(|(file, text)| match parse_profile(text) {
                Ok(p) => p,
                // Built-ins are covered by tests; a broken one is a build defect.
                Err(e) => panic!("built-in profile {file} is invalid: {e}"),
            })
            .collect()
    })
}

/// The built-in profile with this id.
pub fn builtin_profile(id: &str) -> Option<&'static DeviceProfile> {
    builtin_profiles().iter().find(|p| p.id == id)
}

/// Picks the built-in profile for a device: keyboards get
/// `generic_keyboard`; gamepads whose vendor/product match an X-Arcade preset
/// get that preset; any other gamepad gets `generic_gamepad`.
pub fn detect(guid: Option<SdlGuid>, is_keyboard: bool) -> &'static DeviceProfile {
    let all = builtin_profiles();
    if let Some(p) = detect_in(all, guid, is_keyboard) {
        return p;
    }
    let fallback = if is_keyboard {
        "generic_keyboard"
    } else {
        "generic_gamepad"
    };
    all.iter()
        .find(|p| p.id == fallback)
        .unwrap_or_else(|| &all[0])
}

/// Picks a profile from `profiles`: for a gamepad, the first profile with an
/// exact `Gamepad { vendor, product }` match for `guid`, else the first with
/// `AnyGamepad`; for a keyboard, the first with `Keyboard`.
pub fn detect_in(
    profiles: &[DeviceProfile],
    guid: Option<SdlGuid>,
    is_keyboard: bool,
) -> Option<&DeviceProfile> {
    if is_keyboard {
        return profiles
            .iter()
            .find(|p| p.matches.contains(&DeviceMatch::Keyboard));
    }
    if let Some(g) = guid {
        let exact = DeviceMatch::Gamepad {
            vendor: g.vendor(),
            product: g.product(),
        };
        if let Some(p) = profiles.iter().find(|p| p.matches.contains(&exact)) {
            return Some(p);
        }
    }
    profiles
        .iter()
        .find(|p| p.matches.contains(&DeviceMatch::AnyGamepad))
}

#[cfg(test)]
mod tests;
