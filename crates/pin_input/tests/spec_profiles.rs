//! Independent spec tests for Subphase 3.2 `pin_input::profiles`, written from
//! the brief's FIXED API and verification criteria 1 and 2 without sight of
//! the implementation.
//!
//! Only the derives the brief fixes are relied on (`SdlGuid` is `Copy + Eq`,
//! `InputEvent` is `PartialEq + Debug`); every other profile type is compared
//! by pattern or by discriminant.

use pin_core::input::{CabinetAxis, CabinetInput, InputEvent, InputEventKind};
use pin_input::profiles::{
    AxisBinding, DeviceMatch, DeviceProfile, GamepadAxis, GamepadButton, NudgeDirection,
    ProfileMapper, RawInput, SdlGuid, builtin_profiles, detect, detect_in, parse_profile,
};
use proptest::prelude::*;
use std::mem::discriminant;

const EXAMPLE: &str = include_str!("spec_fixtures/profiles/example_pad.toml");

// SDL_Scancode values used by the 1.3 default keymap.
const SC_A: u32 = 4;
const SC_B: u32 = 5;
const SC_T: u32 = 23;
const SC_1: u32 = 30;
const SC_RETURN: u32 = 40;
const SC_ESCAPE: u32 = 41;
const SC_SPACE: u32 = 44;
const SC_KP_ENTER: u32 = 88;
const SC_LCTRL: u32 = 224;
const SC_LSHIFT: u32 = 225;
const SC_RCTRL: u32 = 228;
const SC_RSHIFT: u32 = 229;

const BUTTON_COUNT: usize = 21;
const AXIS_COUNT: usize = 6;

/// SDL gamepad button by SDL index.
fn button(i: usize) -> GamepadButton {
    match i {
        0 => GamepadButton::South,
        1 => GamepadButton::East,
        2 => GamepadButton::West,
        3 => GamepadButton::North,
        4 => GamepadButton::Back,
        5 => GamepadButton::Guide,
        6 => GamepadButton::Start,
        7 => GamepadButton::LeftStick,
        8 => GamepadButton::RightStick,
        9 => GamepadButton::LeftShoulder,
        10 => GamepadButton::RightShoulder,
        11 => GamepadButton::DpadUp,
        12 => GamepadButton::DpadDown,
        13 => GamepadButton::DpadLeft,
        14 => GamepadButton::DpadRight,
        15 => GamepadButton::Misc1,
        16 => GamepadButton::RightPaddle1,
        17 => GamepadButton::LeftPaddle1,
        18 => GamepadButton::RightPaddle2,
        19 => GamepadButton::LeftPaddle2,
        20 => GamepadButton::Touchpad,
        _ => panic!("no SDL gamepad button {i}"),
    }
}

/// SDL gamepad axis by SDL index.
fn axis(i: usize) -> GamepadAxis {
    match i {
        0 => GamepadAxis::LeftX,
        1 => GamepadAxis::LeftY,
        2 => GamepadAxis::RightX,
        3 => GamepadAxis::RightY,
        4 => GamepadAxis::LeftTrigger,
        5 => GamepadAxis::RightTrigger,
        _ => panic!("no SDL gamepad axis {i}"),
    }
}

fn button_index(b: &GamepadButton) -> usize {
    (0..BUTTON_COUNT)
        .find(|&i| discriminant(&button(i)) == discriminant(b))
        .expect("button is one of the 21 SDL buttons")
}

fn parse_ok(text: &str) -> DeviceProfile {
    match parse_profile(text) {
        Ok(p) => p,
        Err(e) => panic!("expected a valid profile, got error `{e}` for:\n{text}"),
    }
}

fn parse_err(text: &str) -> String {
    match parse_profile(text) {
        Ok(p) => panic!("expected an error, got profile `{}` for:\n{text}", p.id),
        Err(e) => e.to_string(),
    }
}

fn builtin(id: &str) -> &'static DeviceProfile {
    builtin_profiles()
        .iter()
        .find(|p| p.id == id)
        .unwrap_or_else(|| panic!("built-in profile `{id}` is missing"))
}

/// An SDL3 joystick GUID for a USB device: bus in bytes 0-1, vendor in 4-5,
/// product in 8-9, all little endian.
fn guid(vendor: u16, product: u16) -> SdlGuid {
    let mut b = [0u8; 16];
    b[0] = 0x03;
    b[4..6].copy_from_slice(&vendor.to_le_bytes());
    b[8..10].copy_from_slice(&product.to_le_bytes());
    b[10] = 0x14;
    b[11] = 0x01;
    SdlGuid(b)
}

fn gamepad_ids(p: &DeviceProfile) -> Vec<(u16, u16)> {
    p.matches
        .iter()
        .filter_map(|m| match m {
            DeviceMatch::Gamepad { vendor, product } => Some((*vendor, *product)),
            _ => None,
        })
        .collect()
}

fn has_keyboard_match(p: &DeviceProfile) -> bool {
    p.matches.iter().any(|m| matches!(m, DeviceMatch::Keyboard))
}

fn has_any_gamepad_match(p: &DeviceProfile) -> bool {
    p.matches
        .iter()
        .any(|m| matches!(m, DeviceMatch::AnyGamepad))
}

fn bound_inputs(p: &DeviceProfile) -> Vec<CabinetInput> {
    let mut v: Vec<CabinetInput> = p.buttons.iter().map(|(_, c)| *c).collect();
    v.sort();
    v.dedup();
    v
}

fn pressed_input(ev: Option<InputEvent>) -> Option<CabinetInput> {
    match ev?.kind {
        InputEventKind::Button { input, .. } => Some(input),
        InputEventKind::Axis { .. } => panic!("a button mapping produced an axis event"),
    }
}

fn axis_value(ev: Option<InputEvent>) -> Option<(CabinetAxis, f32)> {
    match ev?.kind {
        InputEventKind::Axis { axis, value } => Some((axis, value)),
        InputEventKind::Button { .. } => panic!("an axis mapping produced a button event"),
    }
}

// ---------------------------------------------------------------------------
// SdlGuid
// ---------------------------------------------------------------------------

#[test]
fn guid_parses_known_sdl_guid_and_extracts_vendor_product() {
    // The SDL game controller database entry for the Xbox 360 controller.
    let g = SdlGuid::parse_hex("030000005e0400008e02000014010000").expect("valid guid");
    assert_eq!(g.vendor(), 0x045e);
    assert_eq!(g.product(), 0x028e);
    assert_eq!(g.0[0], 0x03);
    assert_eq!(g.0[4], 0x5e);
    assert_eq!(g.0[5], 0x04);
    assert_eq!(g.0[8], 0x8e);
    assert_eq!(g.0[9], 0x02);
    assert_eq!(g.0[12], 0x14);
}

#[test]
fn guid_parse_is_case_insensitive() {
    let lower = SdlGuid::parse_hex("030000005e0400008e020000abcdef00").expect("lower");
    let upper = SdlGuid::parse_hex("030000005E0400008E020000ABCDEF00").expect("upper");
    let mixed = SdlGuid::parse_hex("030000005e0400008E020000AbCdEf00").expect("mixed");
    assert_eq!(lower, upper);
    assert_eq!(lower, mixed);
    assert_eq!(lower.0[12..15], [0xab, 0xcd, 0xef]);
}

#[test]
fn guid_parse_rejects_anything_but_32_hex_chars() {
    let bad = [
        "",
        "0",
        "030000005e0400008e0200001401000",   // 31
        "030000005e0400008e020000140100000", // 33
        "030000005e0400008e02000014010000030000005e0400008e02000014010000", // 64
        "030000005e0400008e0200001401000g",
        "030000005e0400008e02000014010 00",
        " 30000005e0400008e02000014010000",
        "0x0000005e0400008e02000014010000",
        "+30000005e0400008e02000014010000", // a sign is not a hex digit
        "03000000+e0400008e02000014010000",
        "-30000005e0400008e02000014010000",
        "030000005e0400008e020000140100é", // 32 bytes, multi-byte char
        "ééééééééééééééééé",               // non-ASCII, odd boundaries
        "０３００００００５ｅ０４００００８ｅ０２００００１４０１００００", // full-width digits
    ];
    for s in bad {
        assert_eq!(SdlGuid::parse_hex(s), None, "{s:?} must be rejected");
    }
}

#[test]
fn guid_vendor_product_use_sdl3_byte_layout() {
    let mut b = [0u8; 16];
    for (i, v) in b.iter_mut().enumerate() {
        *v = 0x10 + i as u8;
    }
    let g = SdlGuid(b);
    assert_eq!(g.vendor(), u16::from_le_bytes([0x14, 0x15]));
    assert_eq!(g.product(), u16::from_le_bytes([0x18, 0x19]));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn guid_hex_round_trips(bytes in any::<[u8; 16]>(), upper in any::<bool>()) {
        let mut s = String::new();
        for b in bytes {
            if upper {
                s.push_str(&format!("{b:02X}"));
            } else {
                s.push_str(&format!("{b:02x}"));
            }
        }
        let g = SdlGuid::parse_hex(&s);
        prop_assert_eq!(g, Some(SdlGuid(bytes)));
        let g = g.unwrap();
        prop_assert_eq!(g.vendor(), u16::from_le_bytes([bytes[4], bytes[5]]));
        prop_assert_eq!(g.product(), u16::from_le_bytes([bytes[8], bytes[9]]));
    }

    #[test]
    fn guid_parse_never_panics(s in "\\PC{0,40}") {
        if let Some(g) = SdlGuid::parse_hex(&s) {
            prop_assert_eq!(s.len(), 32);
            prop_assert!(s.chars().all(|c| c.is_ascii_hexdigit()));
            let _ = (g.vendor(), g.product());
        }
    }

    #[test]
    fn guid_parse_rejects_wrong_lengths(s in "[0-9a-fA-F]{0,70}") {
        prop_assert_eq!(SdlGuid::parse_hex(&s).is_some(), s.len() == 32);
    }
}

// ---------------------------------------------------------------------------
// Raw enums mirror SDL's order
// ---------------------------------------------------------------------------

#[test]
fn gamepad_enums_follow_sdl_order() {
    for i in 0..BUTTON_COUNT {
        assert_eq!(button(i) as u32, i as u32, "SDL_GamepadButton {i}");
    }
    for i in 0..AXIS_COUNT {
        assert_eq!(axis(i) as u32, i as u32, "SDL_GamepadAxis {i}");
    }
}

// ---------------------------------------------------------------------------
// Built-in profiles (verification criterion 1)
// ---------------------------------------------------------------------------

#[test]
fn builtin_ids_are_the_fixed_set() {
    let all = builtin_profiles();
    for id in [
        "xarcade_keyboard",
        "xarcade_gamepad",
        "generic_keyboard",
        "generic_gamepad",
    ] {
        assert_eq!(
            all.iter().filter(|p| p.id == id).count(),
            1,
            "exactly one built-in `{id}`"
        );
    }
    for p in all {
        assert!(
            p.id == "generic_keyboard" || p.id == "generic_gamepad" || p.id.starts_with("xarcade_"),
            "unexpected built-in id `{}`: extra ids must start with `xarcade_`",
            p.id
        );
        assert!(!p.name.trim().is_empty(), "`{}` has no name", p.id);
        assert!(!p.matches.is_empty(), "`{}` matches no device", p.id);
    }
    let mut ids: Vec<&str> = all.iter().map(|p| p.id.as_str()).collect();
    ids.sort_unstable();
    let n = ids.len();
    ids.dedup();
    assert_eq!(ids.len(), n, "built-in ids are unique");
}

#[test]
fn builtin_profiles_is_a_stable_static_slice() {
    let a = builtin_profiles();
    let b = builtin_profiles();
    assert!(std::ptr::eq(a.as_ptr(), b.as_ptr()));
    assert_eq!(a.len(), b.len());
}

#[test]
fn every_builtin_maps_flippers_start_and_launch() {
    for p in builtin_profiles() {
        let bound = bound_inputs(p);
        for need in [
            CabinetInput::LeftFlipper,
            CabinetInput::RightFlipper,
            CabinetInput::Start,
            CabinetInput::Launch,
        ] {
            assert!(bound.contains(&need), "`{}` does not map {need:?}", p.id);
        }
    }
}

#[test]
fn xarcade_presets_map_all_eight_cabinet_inputs() {
    let xs: Vec<&DeviceProfile> = builtin_profiles()
        .iter()
        .filter(|p| p.id.starts_with("xarcade_"))
        .collect();
    assert!(xs.len() >= 2);
    for p in xs {
        let bound = bound_inputs(p);
        for need in CabinetInput::ALL {
            assert!(bound.contains(&need), "`{}` does not map {need:?}", p.id);
        }
    }
}

#[test]
fn xarcade_keyboard_is_a_keyboard_encoder_preset() {
    let p = builtin("xarcade_keyboard");
    assert!(has_keyboard_match(p), "matches a keyboard");
    assert!(gamepad_ids(p).is_empty(), "is not a gamepad preset");
    assert!(!has_any_gamepad_match(p));
    // All eight inputs are reachable from scancodes alone.
    for need in CabinetInput::ALL {
        assert!(
            p.buttons
                .iter()
                .any(|(raw, c)| *c == need && matches!(raw, RawInput::Key(_))),
            "{need:?} has no scancode"
        );
    }
}

#[test]
fn xarcade_gamepad_presets_match_by_vendor_product() {
    let pads: Vec<&DeviceProfile> = builtin_profiles()
        .iter()
        .filter(|p| p.id.starts_with("xarcade_") && p.id != "xarcade_keyboard")
        .filter(|p| !has_keyboard_match(p))
        .collect();
    assert!(pads.iter().any(|p| p.id == "xarcade_gamepad"));
    for p in pads {
        assert!(
            !gamepad_ids(p).is_empty(),
            "`{}` must match at least one vendor/product",
            p.id
        );
        assert!(
            !has_any_gamepad_match(p),
            "`{}` must not swallow every gamepad",
            p.id
        );
        for (v, _) in gamepad_ids(p) {
            assert_ne!(v, 0, "`{}` has a zero vendor id", p.id);
        }
    }
}

#[test]
fn generic_presets_match_their_device_class() {
    let k = builtin("generic_keyboard");
    assert!(has_keyboard_match(k));
    assert!(gamepad_ids(k).is_empty() && !has_any_gamepad_match(k));
    let g = builtin("generic_gamepad");
    assert!(has_any_gamepad_match(g));
    assert!(!has_keyboard_match(g));
}

/// Every `(raw, cabinet)` pair of every built-in translates through the mapper
/// to a canonical cabinet switch event, press and release, timestamp kept.
#[test]
fn builtin_mappers_translate_every_binding() {
    for p in builtin_profiles() {
        let m = ProfileMapper::new(p);
        for (raw, _) in &p.buttons {
            let allowed = |code_matches: &dyn Fn(&RawInput) -> bool| -> Vec<CabinetInput> {
                p.buttons
                    .iter()
                    .filter(|(r, _)| code_matches(r))
                    .map(|(_, c)| *c)
                    .collect()
            };
            match raw {
                RawInput::Key(k) => {
                    let ok = allowed(&|r| matches!(r, RawInput::Key(x) if x == k));
                    for pressed in [true, false] {
                        let ev = m.key(77, *k, pressed).unwrap_or_else(|| {
                            panic!("`{}`: scancode {k} is bound but unmapped", p.id)
                        });
                        assert_eq!(ev.timestamp_ns, 77);
                        match ev.kind {
                            InputEventKind::Button {
                                input,
                                pressed: got,
                            } => {
                                assert_eq!(got, pressed);
                                assert!(ok.contains(&input), "`{}`: key {k} -> {input:?}", p.id);
                            }
                            InputEventKind::Axis { .. } => panic!("key produced an axis event"),
                        }
                    }
                }
                RawInput::Button(b) => {
                    let bi = button_index(b);
                    let ok = allowed(
                        &|r| matches!(r, RawInput::Button(x) if discriminant(x) == discriminant(b)),
                    );
                    for pressed in [true, false] {
                        let ev = m.button(78, button(bi), pressed).unwrap_or_else(|| {
                            panic!("`{}`: button {bi} is bound but unmapped", p.id)
                        });
                        assert_eq!(ev.timestamp_ns, 78);
                        match ev.kind {
                            InputEventKind::Button {
                                input,
                                pressed: got,
                            } => {
                                assert_eq!(got, pressed);
                                assert!(ok.contains(&input), "`{}`: button {bi}", p.id);
                            }
                            InputEventKind::Axis { .. } => panic!("button produced an axis event"),
                        }
                    }
                }
                RawInput::Axis(_) => {}
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Generic presets reproduce the 1.3 defaults (deliverable 1, notes)
// ---------------------------------------------------------------------------

/// `pin_app::platform::keymap::key_input` as merged by Subphase 1.3.
fn keymap_1_3_key(scancode: u32) -> Option<CabinetInput> {
    Some(match scancode {
        SC_LSHIFT => CabinetInput::LeftFlipper,
        SC_RSHIFT => CabinetInput::RightFlipper,
        SC_LCTRL => CabinetInput::LeftUpper,
        SC_RCTRL => CabinetInput::RightUpper,
        SC_SPACE | SC_RETURN | SC_KP_ENTER => CabinetInput::Launch,
        SC_1 => CabinetInput::Start,
        SC_A => CabinetInput::Action,
        SC_T => CabinetInput::Tilt,
        _ => return None,
    })
}

/// `pin_app::platform::keymap::gamepad_button_input` (1.3), by SDL index.
fn keymap_1_3_button(i: usize) -> Option<CabinetInput> {
    Some(match i {
        9 => CabinetInput::LeftFlipper,
        10 => CabinetInput::RightFlipper,
        7 => CabinetInput::LeftUpper,
        8 => CabinetInput::RightUpper,
        0 => CabinetInput::Launch,
        1 => CabinetInput::Action,
        6 => CabinetInput::Start,
        4 => CabinetInput::Tilt,
        _ => return None,
    })
}

/// `pin_app::platform::keymap::gamepad_axis_input` (1.3), by SDL index.
fn keymap_1_3_axis(i: usize, raw: i16) -> Option<(CabinetAxis, f32)> {
    let signed = (f32::from(raw) / 32767.0).clamp(-1.0, 1.0);
    let unsigned = signed.max(0.0);
    Some(match i {
        4 => (CabinetAxis::LeftFlipper, unsigned),
        5 => (CabinetAxis::RightFlipper, unsigned),
        1 => (CabinetAxis::Plunger, unsigned),
        2 => (CabinetAxis::NudgeX, signed),
        3 => (CabinetAxis::NudgeY, signed),
        _ => return None,
    })
}

#[test]
fn generic_keyboard_equals_the_1_3_default_keymap() {
    let m = ProfileMapper::new(builtin("generic_keyboard"));
    assert_eq!(pressed_input(m.key(0, SC_B, true)), None);
    assert_eq!(pressed_input(m.key(0, SC_ESCAPE, true)), None);
    for sc in 0..=600u32 {
        for pressed in [true, false] {
            let want =
                keymap_1_3_key(sc).map(|c| InputEvent::button(1_000 + u64::from(sc), c, pressed));
            assert_eq!(
                m.key(1_000 + u64::from(sc), sc, pressed),
                want,
                "scancode {sc}"
            );
        }
    }
    assert_eq!(m.key(0, u32::MAX, true), None);
    // A keyboard preset has nothing to say about gamepad input.
    for i in 0..BUTTON_COUNT {
        assert_eq!(m.button(0, button(i), true), None);
    }
    for i in 0..AXIS_COUNT {
        assert_eq!(m.axis(0, axis(i), 20_000), None);
    }
}

#[test]
fn generic_gamepad_equals_the_1_3_default_buttons() {
    let m = ProfileMapper::new(builtin("generic_gamepad"));
    for i in 0..BUTTON_COUNT {
        for pressed in [true, false] {
            let want = keymap_1_3_button(i).map(|c| InputEvent::button(9, c, pressed));
            assert_eq!(m.button(9, button(i), pressed), want, "SDL button {i}");
        }
    }
    for sc in 0..=600u32 {
        assert_eq!(
            m.key(0, sc, true),
            None,
            "gamepad preset maps scancode {sc}"
        );
    }
}

#[test]
fn generic_gamepad_equals_the_1_3_default_axes_at_landmarks() {
    let m = ProfileMapper::new(builtin("generic_gamepad"));
    let exact = |i: usize, raw: i16, want_axis: CabinetAxis, want: f32| {
        let (a, v) = axis_value(m.axis(3, axis(i), raw)).expect("bound axis");
        assert_eq!(a, want_axis);
        assert!((v - want).abs() < 1e-5, "axis {i} raw {raw}: {v} vs {want}");
    };
    exact(4, 32767, CabinetAxis::LeftFlipper, 1.0);
    exact(5, 0, CabinetAxis::RightFlipper, 0.0);
    exact(1, -32768, CabinetAxis::Plunger, 0.0);
    exact(1, 32767, CabinetAxis::Plunger, 1.0);
    exact(2, -32768, CabinetAxis::NudgeX, -1.0);
    exact(2, 32767, CabinetAxis::NudgeX, 1.0);
    exact(2, 0, CabinetAxis::NudgeX, 0.0);
    exact(3, 16384, CabinetAxis::NudgeY, 16384.0 / 32767.0);
    assert_eq!(
        m.axis(3, axis(0), 100),
        None,
        "left stick X is unbound in 1.3"
    );
    assert_eq!(m.axis(3, axis(4), 1234).map(|e| e.timestamp_ns), Some(3));
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn generic_gamepad_equals_the_1_3_default_axes(i in 0..AXIS_COUNT, raw in any::<i16>(), ts in any::<u64>()) {
        let m = ProfileMapper::new(builtin("generic_gamepad"));
        let got = m.axis(ts, axis(i), raw);
        match keymap_1_3_axis(i, raw) {
            None => prop_assert_eq!(got, None),
            Some((want_axis, want)) => {
                let ev = got.expect("bound axis");
                prop_assert_eq!(ev.timestamp_ns, ts);
                let (a, v) = axis_value(Some(ev)).unwrap();
                prop_assert_eq!(a, want_axis);
                prop_assert!((v - want).abs() < 1e-5, "raw {} -> {} vs {}", raw, v, want);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// detect / detect_in
// ---------------------------------------------------------------------------

#[test]
fn detect_keyboard_is_generic_never_xarcade() {
    assert_eq!(detect(None, true).id, "generic_keyboard");
    // Whatever GUID accompanies a keyboard, the X-Arcade encoder is never auto-picked.
    assert_ne!(
        detect(Some(guid(0xAA55, 0x0101)), true).id,
        "xarcade_keyboard"
    );
}

#[test]
fn detect_unknown_gamepad_is_generic() {
    assert_eq!(detect(None, false).id, "generic_gamepad");
    // Xbox 360 pad from the SDL database.
    let xbox = SdlGuid::parse_hex("030000005e0400008e02000014010000").unwrap();
    let claimed = builtin_profiles()
        .iter()
        .any(|p| gamepad_ids(p).contains(&(xbox.vendor(), xbox.product())));
    if !claimed {
        assert_eq!(detect(Some(xbox), false).id, "generic_gamepad");
    }
    assert_eq!(detect(Some(SdlGuid([0; 16])), false).id, "generic_gamepad");
}

#[test]
fn detect_xarcade_gamepad_by_vendor_product() {
    let mut seen = 0;
    for p in builtin_profiles() {
        for (v, pr) in gamepad_ids(p) {
            seen += 1;
            let got = detect(Some(guid(v, pr)), false);
            assert!(
                got.id.starts_with("xarcade_"),
                "{v:04x}:{pr:04x} detected as `{}`",
                got.id
            );
            assert!(gamepad_ids(got).contains(&(v, pr)));
            // The first preset in built-in order that lists the pair wins.
            let first = builtin_profiles()
                .iter()
                .find(|q| gamepad_ids(q).contains(&(v, pr)))
                .unwrap();
            assert_eq!(got.id, first.id);
            // Only vendor and product decide: other GUID bytes are ignored.
            let mut other = guid(v, pr);
            other.0[0] = 0x05;
            other.0[2] = 0x9c;
            other.0[12] = 0x77;
            other.0[15] = 0x01;
            assert_eq!(detect(Some(other), false).id, got.id);
        }
    }
    assert!(
        seen >= 1,
        "at least one X-Arcade gamepad vendor/product is known"
    );
}

#[test]
fn detect_returns_one_of_the_builtins() {
    let all = builtin_profiles();
    for (g, kb) in [
        (None, true),
        (None, false),
        (Some(guid(1, 2)), false),
        (Some(guid(1, 2)), true),
    ] {
        let p = detect(g, kb);
        assert!(all.iter().any(|q| std::ptr::eq(p, q)));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn detect_gamepad_agrees_with_the_rule(bytes in any::<[u8; 16]>()) {
        let g = SdlGuid(bytes);
        let got = detect(Some(g), false);
        let exact = builtin_profiles()
            .iter()
            .find(|p| gamepad_ids(p).contains(&(g.vendor(), g.product())));
        match exact {
            Some(p) => prop_assert_eq!(&got.id, &p.id),
            None => prop_assert_eq!(got.id.as_str(), "generic_gamepad"),
        }
        // detect is detect_in over the built-ins for gamepads.
        let via = detect_in(builtin_profiles(), Some(g), false).expect("generic_gamepad matches");
        prop_assert_eq!(&via.id, &got.id);
        prop_assert_eq!(detect(Some(g), true).id.as_str(), "generic_keyboard");
    }
}

fn mini(id: &str, matches: &str) -> DeviceProfile {
    parse_ok(&format!(
        "id = \"{id}\"\nname = \"Profile {id}\"\nmatch = [ {matches} ]\n\n[buttons]\nleft_flipper = [ \"key:225\" ]\n"
    ))
}

fn pad_match(v: u16, p: u16) -> String {
    format!("{{ gamepad = {{ vendor = {v}, product = {p} }} }}")
}

const KB: &str = "{ keyboard = true }";
const ANY: &str = "{ any_gamepad = true }";

fn id_of(p: Option<&DeviceProfile>) -> Option<&str> {
    p.map(|p| p.id.as_str())
}

#[test]
fn detect_in_prefers_exact_match_over_earlier_any_gamepad() {
    let ps = [
        mini("any", ANY),
        mini("kb", KB),
        mini("exact", &pad_match(0x1234, 0x5678)),
    ];
    assert_eq!(
        id_of(detect_in(&ps, Some(guid(0x1234, 0x5678)), false)),
        Some("exact")
    );
    assert_eq!(
        id_of(detect_in(&ps, Some(guid(0x1234, 0x5679)), false)),
        Some("any")
    );
    assert_eq!(
        id_of(detect_in(&ps, Some(guid(0x1235, 0x5678)), false)),
        Some("any")
    );
    // Vendor and product are not interchangeable.
    assert_eq!(
        id_of(detect_in(&ps, Some(guid(0x5678, 0x1234)), false)),
        Some("any")
    );
    assert_eq!(id_of(detect_in(&ps, None, false)), Some("any"));
    assert_eq!(id_of(detect_in(&ps, None, true)), Some("kb"));
}

#[test]
fn detect_in_uses_slice_order_among_equals() {
    let ps = [
        mini("kb1", KB),
        mini("pad1", &pad_match(7, 9)),
        mini("kb2", KB),
        mini("pad2", &pad_match(7, 9)),
        mini("any1", ANY),
        mini("any2", ANY),
    ];
    assert_eq!(id_of(detect_in(&ps, None, true)), Some("kb1"));
    assert_eq!(id_of(detect_in(&ps, Some(guid(7, 9)), false)), Some("pad1"));
    assert_eq!(id_of(detect_in(&ps, Some(guid(9, 7)), false)), Some("any1"));
    assert_eq!(id_of(detect_in(&ps[2..], None, true)), Some("kb2"));
    assert_eq!(
        id_of(detect_in(&ps[2..], Some(guid(7, 9)), false)),
        Some("pad2")
    );
    assert_eq!(
        id_of(detect_in(&ps[5..], Some(guid(7, 9)), false)),
        Some("any2")
    );
}

#[test]
fn detect_in_returns_none_when_nothing_matches() {
    assert!(detect_in(&[], None, true).is_none());
    assert!(detect_in(&[], Some(guid(1, 2)), false).is_none());
    let kb_only = [mini("kb", KB)];
    assert!(detect_in(&kb_only, Some(guid(1, 2)), false).is_none());
    assert!(detect_in(&kb_only, None, false).is_none());
    let pads = [mini("exact", &pad_match(1, 2)), mini("any", ANY)];
    assert!(detect_in(&pads, None, true).is_none());
    let exact_only = [mini("exact", &pad_match(1, 2))];
    assert!(detect_in(&exact_only, Some(guid(1, 3)), false).is_none());
    assert!(detect_in(&exact_only, None, false).is_none());
    assert_eq!(
        id_of(detect_in(&exact_only, Some(guid(1, 2)), false)),
        Some("exact")
    );
}

#[test]
fn detect_in_checks_every_match_entry_of_a_profile() {
    let multi = format!(
        "{}, {}, {}",
        pad_match(1, 2),
        pad_match(3, 4),
        pad_match(0xFFFF, 0xFFFF)
    );
    let ps = [mini("any", ANY), mini("multi", &multi)];
    assert_eq!(gamepad_ids(&ps[1]), vec![(1, 2), (3, 4), (0xFFFF, 0xFFFF)]);
    for (v, p) in [(1, 2), (3, 4), (0xFFFF, 0xFFFF)] {
        assert_eq!(
            id_of(detect_in(&ps, Some(guid(v, p)), false)),
            Some("multi")
        );
    }
    assert_eq!(id_of(detect_in(&ps, Some(guid(1, 4)), false)), Some("any"));
}

/// What a profile matches, for the model below.
#[derive(Clone, Debug)]
enum M {
    Kb,
    Any,
    Pad(u16, u16),
}

fn m_strategy() -> impl Strategy<Value = M> {
    prop_oneof![
        Just(M::Kb),
        Just(M::Any),
        (0u16..4, 0u16..4).prop_map(|(v, p)| M::Pad(v, p)),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn detect_in_agrees_with_the_rule(
        spec in prop::collection::vec(prop::collection::vec(m_strategy(), 1..3), 0..7),
        v in 0u16..4,
        p in 0u16..4,
    ) {
        // A profile is either a keyboard profile or a gamepad profile.
        let spec: Vec<Vec<M>> = spec
            .into_iter()
            .map(|ms| {
                let kb = matches!(ms[0], M::Kb);
                ms.into_iter().filter(|m| matches!(m, M::Kb) == kb).collect()
            })
            .collect();
        let profiles: Vec<DeviceProfile> = spec
            .iter()
            .enumerate()
            .map(|(i, ms)| {
                let text: Vec<String> = ms
                    .iter()
                    .map(|m| match m {
                        M::Kb => KB.to_string(),
                        M::Any => ANY.to_string(),
                        M::Pad(v, p) => pad_match(*v, *p),
                    })
                    .collect();
                mini(&format!("p{i}"), &text.join(", "))
            })
            .collect();

        let exact = spec
            .iter()
            .position(|ms| ms.iter().any(|m| matches!(m, M::Pad(a, b) if *a == v && *b == p)));
        let any = spec.iter().position(|ms| ms.iter().any(|m| matches!(m, M::Any)));
        let kb = spec.iter().position(|ms| ms.iter().any(|m| matches!(m, M::Kb)));

        let want_pad = exact.or(any).map(|i| format!("p{i}"));
        let got = detect_in(&profiles, Some(guid(v, p)), false);
        prop_assert_eq!(id_of(got).map(str::to_string), want_pad);

        let got = detect_in(&profiles, None, false);
        prop_assert_eq!(id_of(got).map(str::to_string), any.map(|i| format!("p{i}")));

        let got = detect_in(&profiles, None, true);
        prop_assert_eq!(id_of(got).map(str::to_string), kb.map(|i| format!("p{i}")));
    }
}

// ---------------------------------------------------------------------------
// Parser: the documented format
// ---------------------------------------------------------------------------

#[test]
fn example_profile_parses_field_by_field() {
    let p = parse_ok(EXAMPLE);
    assert_eq!(p.id, "my_pad");
    assert_eq!(p.name, "My Pad");
    assert_eq!(p.matches.len(), 1);
    assert_eq!(gamepad_ids(&p), vec![(0x1234, 0x5678)]);

    let has_key = |k: u32, c: CabinetInput| {
        p.buttons
            .iter()
            .any(|(r, got)| *got == c && matches!(r, RawInput::Key(x) if *x == k))
    };
    let has_button = |b: GamepadButton, c: CabinetInput| {
        p.buttons.iter().any(|(r, got)| {
            *got == c && matches!(r, RawInput::Button(x) if discriminant(x) == discriminant(&b))
        })
    };
    assert_eq!(p.buttons.len(), 7);
    assert!(has_key(225, CabinetInput::LeftFlipper));
    assert!(has_button(
        GamepadButton::LeftShoulder,
        CabinetInput::LeftFlipper
    ));
    assert!(has_key(229, CabinetInput::RightFlipper));
    assert!(has_button(
        GamepadButton::RightShoulder,
        CabinetInput::RightFlipper
    ));
    assert!(has_button(GamepadButton::Start, CabinetInput::Start));
    assert!(has_button(GamepadButton::South, CabinetInput::Launch));
    assert!(has_key(40, CabinetInput::Launch));

    assert_eq!(p.axes.len(), 4);
    let find = |raw: GamepadAxis| -> &AxisBinding {
        p.axes
            .iter()
            .find(|a| discriminant(&a.raw) == discriminant(&raw))
            .expect("axis binding present")
    };
    let lt = find(GamepadAxis::LeftTrigger);
    assert_eq!(lt.axis, CabinetAxis::LeftFlipper);
    assert_eq!((lt.from, lt.to, lt.invert), (0.0, 1.0, false));
    let rx = find(GamepadAxis::RightX);
    assert_eq!(rx.axis, CabinetAxis::NudgeX);
    assert_eq!((rx.from, rx.to, rx.invert), (-1.0, 1.0, false));
    let ry = find(GamepadAxis::RightY);
    assert_eq!(ry.axis, CabinetAxis::NudgeY);
    assert_eq!((ry.from, ry.to, ry.invert), (-1.0, 1.0, true));
    let ly = find(GamepadAxis::LeftY);
    assert_eq!(ly.axis, CabinetAxis::Plunger);

    assert_eq!(p.nudge_buttons.len(), 4);
    let nudge = |k: u32| {
        p.nudge_buttons
            .iter()
            .find(|(r, _)| matches!(r, RawInput::Key(x) if *x == k))
            .map(|(_, d)| d)
    };
    assert!(matches!(nudge(224), Some(NudgeDirection::Left)));
    assert!(matches!(nudge(228), Some(NudgeDirection::Right)));
    assert!(matches!(nudge(44), Some(NudgeDirection::Forward)));
    assert!(p.nudge_buttons.iter().any(|(r, d)| matches!(
        (r, d),
        (
            RawInput::Button(GamepadButton::DpadRight),
            NudgeDirection::Right
        )
    )));
}

#[test]
fn example_profile_maps_buttons_keys_and_nudges() {
    let p = parse_ok(EXAMPLE);
    let m = ProfileMapper::new(&p);
    assert_eq!(
        m.key(11, 225, true),
        Some(InputEvent::button(11, CabinetInput::LeftFlipper, true))
    );
    assert_eq!(
        m.key(12, 225, false),
        Some(InputEvent::button(12, CabinetInput::LeftFlipper, false))
    );
    assert_eq!(
        m.key(u64::MAX, 229, true),
        Some(InputEvent::button(
            u64::MAX,
            CabinetInput::RightFlipper,
            true
        ))
    );
    assert_eq!(
        m.key(0, 40, true),
        Some(InputEvent::button(0, CabinetInput::Launch, true))
    );
    assert_eq!(m.key(0, 226, true), None);
    assert_eq!(m.key(0, 0, true), None);
    assert_eq!(
        m.button(5, GamepadButton::LeftShoulder, true),
        Some(InputEvent::button(5, CabinetInput::LeftFlipper, true))
    );
    assert_eq!(
        m.button(5, GamepadButton::Start, false),
        Some(InputEvent::button(5, CabinetInput::Start, false))
    );
    assert_eq!(
        m.button(5, GamepadButton::South, true),
        Some(InputEvent::button(5, CabinetInput::Launch, true))
    );
    assert_eq!(m.button(5, GamepadButton::North, true), None);
    assert_eq!(m.button(5, GamepadButton::Touchpad, true), None);

    assert!(matches!(
        m.nudge_button(RawInput::Key(224)),
        Some(NudgeDirection::Left)
    ));
    assert!(matches!(
        m.nudge_button(RawInput::Key(228)),
        Some(NudgeDirection::Right)
    ));
    assert!(matches!(
        m.nudge_button(RawInput::Key(44)),
        Some(NudgeDirection::Forward)
    ));
    assert!(matches!(
        m.nudge_button(RawInput::Button(GamepadButton::DpadRight)),
        Some(NudgeDirection::Right)
    ));
    assert!(m.nudge_button(RawInput::Key(225)).is_none());
    assert!(m.nudge_button(RawInput::Key(45)).is_none());
    assert!(
        m.nudge_button(RawInput::Button(GamepadButton::DpadLeft))
            .is_none()
    );
    assert!(m.nudge_button(RawInput::Axis(GamepadAxis::LeftX)).is_none());
    // Nudge keys are not cabinet switches unless also listed under [buttons].
    assert_eq!(m.key(0, 224, true), None);
}

#[test]
fn example_profile_maps_axes_linearly_and_clamped() {
    let p = parse_ok(EXAMPLE);
    let m = ProfileMapper::new(&p);
    let val = |a: GamepadAxis, raw: i16| axis_value(m.axis(1, a, raw)).expect("bound");
    let near = |got: (CabinetAxis, f32), want_axis: CabinetAxis, want: f32| {
        assert_eq!(got.0, want_axis);
        assert!((got.1 - want).abs() < 1e-4, "{} vs {want}", got.1);
    };
    // Trigger 0..1 -> flipper travel 0..1; negative raw clamps to 0.
    near(
        val(GamepadAxis::LeftTrigger, 0),
        CabinetAxis::LeftFlipper,
        0.0,
    );
    near(
        val(GamepadAxis::LeftTrigger, 32767),
        CabinetAxis::LeftFlipper,
        1.0,
    );
    near(
        val(GamepadAxis::LeftTrigger, 16384),
        CabinetAxis::LeftFlipper,
        16384.0 / 32767.0,
    );
    near(
        val(GamepadAxis::LeftTrigger, -32768),
        CabinetAxis::LeftFlipper,
        0.0,
    );
    near(
        val(GamepadAxis::LeftTrigger, -5),
        CabinetAxis::LeftFlipper,
        0.0,
    );
    // Stick -1..1 -> nudge -1..1; -32768 clamps to -1.
    near(val(GamepadAxis::RightX, 32767), CabinetAxis::NudgeX, 1.0);
    near(val(GamepadAxis::RightX, -32767), CabinetAxis::NudgeX, -1.0);
    near(val(GamepadAxis::RightX, -32768), CabinetAxis::NudgeX, -1.0);
    near(val(GamepadAxis::RightX, 0), CabinetAxis::NudgeX, 0.0);
    near(
        val(GamepadAxis::RightX, 8192),
        CabinetAxis::NudgeX,
        8192.0 / 32767.0,
    );
    // Inverted symmetric stick: the sign flips.
    near(val(GamepadAxis::RightY, 32767), CabinetAxis::NudgeY, -1.0);
    near(val(GamepadAxis::RightY, -32768), CabinetAxis::NudgeY, 1.0);
    near(
        val(GamepadAxis::RightY, 8192),
        CabinetAxis::NudgeY,
        -8192.0 / 32767.0,
    );
    // Stick with from = 0, to = 1 -> the positive half drives the plunger.
    near(val(GamepadAxis::LeftY, 32767), CabinetAxis::Plunger, 1.0);
    near(
        val(GamepadAxis::LeftY, 16384),
        CabinetAxis::Plunger,
        16384.0 / 32767.0,
    );
    near(val(GamepadAxis::LeftY, 0), CabinetAxis::Plunger, 0.0);
    near(val(GamepadAxis::LeftY, -20000), CabinetAxis::Plunger, 0.0);
    // Unbound axes say nothing.
    assert_eq!(m.axis(1, GamepadAxis::LeftX, 12345), None);
    assert_eq!(m.axis(1, GamepadAxis::RightTrigger, 12345), None);
    assert_eq!(
        m.axis(42, GamepadAxis::RightX, 5).map(|e| e.timestamp_ns),
        Some(42)
    );
}

#[test]
fn hex_and_decimal_vendor_product_are_equivalent() {
    let a = mini("a", "{ gamepad = { vendor = 0x1234, product = 0x5678 } }");
    let b = mini("b", "{ gamepad = { vendor = 4660, product = 22136 } }");
    assert_eq!(gamepad_ids(&a), vec![(0x1234, 0x5678)]);
    assert_eq!(gamepad_ids(&a), gamepad_ids(&b));
    let edge = mini("c", "{ gamepad = { vendor = 0, product = 65535 } }");
    assert_eq!(gamepad_ids(&edge), vec![(0, 0xFFFF)]);
}

#[test]
fn match_kinds_parse_to_their_variants() {
    let p = mini("k", KB);
    assert!(matches!(p.matches.as_slice(), [DeviceMatch::Keyboard]));
    let p = mini("a", ANY);
    assert!(matches!(p.matches.as_slice(), [DeviceMatch::AnyGamepad]));
    let p = mini("g", &format!("{}, {ANY}", pad_match(5, 6)));
    assert!(matches!(
        p.matches.as_slice(),
        [
            DeviceMatch::Gamepad {
                vendor: 5,
                product: 6
            },
            DeviceMatch::AnyGamepad
        ]
    ));
}

#[test]
fn every_cabinet_input_name_and_plain_button_name_parses() {
    let names = [
        ("south", 0usize),
        ("east", 1),
        ("west", 2),
        ("north", 3),
        ("back", 4),
        ("guide", 5),
        ("start", 6),
        ("left_stick", 7),
        ("right_stick", 8),
        ("left_shoulder", 9),
        ("right_shoulder", 10),
        ("dpad_up", 11),
        ("dpad_down", 12),
        ("dpad_left", 13),
        ("dpad_right", 14),
        ("touchpad", 20),
    ];
    for (k, cab) in CabinetInput::ALL.into_iter().enumerate() {
        for (name, idx) in names {
            let text = format!(
                "id = \"t\"\nname = \"T\"\nmatch = [ {ANY} ]\n[buttons]\n{} = [ \"button:{name}\", \"key:{}\" ]\n",
                cab.name(),
                100 + k
            );
            let p = parse_ok(&text);
            assert_eq!(p.buttons.len(), 2);
            let m = ProfileMapper::new(&p);
            assert_eq!(
                m.button(1, button(idx), true),
                Some(InputEvent::button(1, cab, true)),
                "button:{name} -> {}",
                cab.name()
            );
            assert_eq!(
                m.key(2, 100 + k as u32, false),
                Some(InputEvent::button(2, cab, false))
            );
            for other in 0..BUTTON_COUNT {
                if other != idx {
                    assert_eq!(m.button(1, button(other), true), None);
                }
            }
        }
    }
}

#[test]
fn every_axis_name_parses() {
    let raws = [
        "left_x",
        "left_y",
        "right_x",
        "right_y",
        "left_trigger",
        "right_trigger",
    ];
    let cabs = [
        ("plunger", CabinetAxis::Plunger),
        ("nudge_x", CabinetAxis::NudgeX),
        ("nudge_y", CabinetAxis::NudgeY),
        ("left_flipper", CabinetAxis::LeftFlipper),
        ("right_flipper", CabinetAxis::RightFlipper),
    ];
    for (ri, raw) in raws.into_iter().enumerate() {
        for (cname, cab) in cabs {
            let text = format!(
                "id = \"t\"\nname = \"T\"\nmatch = [ {ANY} ]\n[buttons]\nstart = [ \"key:30\" ]\n\n[[axes]]\nraw = \"{raw}\"\naxis = \"{cname}\"\nfrom = 0.0\nto = 1.0\ninvert = false\n"
            );
            let p = parse_ok(&text);
            assert_eq!(p.axes.len(), 1);
            assert_eq!(p.axes[0].axis, cab);
            assert_eq!(discriminant(&p.axes[0].raw), discriminant(&axis(ri)));
            let m = ProfileMapper::new(&p);
            for other in 0..AXIS_COUNT {
                let got = axis_value(m.axis(0, axis(other), 32767));
                if other == ri {
                    assert_eq!(got.map(|g| g.0), Some(cab), "{raw} -> {cname}");
                } else {
                    assert_eq!(got, None);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Parser: precise errors
// ---------------------------------------------------------------------------

/// `Display` is `<line>:<col>: <message>` or `<key>: <message>`. Returns the
/// location when the first form is used.
fn check_error_format(msg: &str) -> Option<(u32, u32)> {
    let (head, rest) = msg.split_once(": ").unwrap_or_else(|| {
        panic!("error `{msg}` is not `<line>:<col>: <message>` or `<key>: <message>`")
    });
    assert!(!rest.trim().is_empty(), "error `{msg}` has no message");
    assert!(!head.is_empty(), "error `{msg}` has an empty location");
    if let Some((l, c)) = head.split_once(':')
        && let (Ok(l), Ok(c)) = (l.parse::<u32>(), c.parse::<u32>())
    {
        assert!(
            l >= 1 && c >= 1,
            "error `{msg}`: line and column are 1-based"
        );
        return Some((l, c));
    }
    assert!(
        head.chars()
            .all(|ch| ch.is_ascii_alphanumeric() || "_.-[]\"'".contains(ch)),
        "error `{msg}`: `{head}` is neither `<line>:<col>` nor a key"
    );
    None
}

const VALID_HEAD: &str = "id = \"t\"\nname = \"T\"\nmatch = [ { any_gamepad = true } ]\n";

#[test]
fn syntax_errors_report_line_and_column() {
    let cases: [(&str, u32); 4] = [
        (
            "id = \"t\"\nname = = \"T\"\nmatch = [ { keyboard = true } ]\n",
            2,
        ),
        (
            "id = \"t\"\nname = \"T\"\nmatch = [ { keyboard = true }\n[buttons\n",
            4,
        ),
        ("id \"t\"\n", 1),
        (
            "id = \"t\"\nname = \"T\"\n\n\n[buttons]\nleft_flipper = [ \"key:225\", ]]\n",
            6,
        ),
    ];
    for (text, line) in cases {
        let msg = parse_err(text);
        let loc = check_error_format(&msg);
        let (l, _) = loc.unwrap_or_else(|| panic!("syntax error `{msg}` must carry <line>:<col>"));
        assert_eq!(l, line, "`{msg}` for:\n{text}");
    }
}

#[test]
fn syntax_error_column_points_at_the_fault() {
    //                 123456789
    let msg = parse_err("id = \"t\"\nname = = \"T\"\n");
    let (l, c) = check_error_format(&msg).expect("line:col");
    assert_eq!(l, 2);
    assert!(
        (7..=9).contains(&c),
        "column {c} should point at the second `=` (col 8): `{msg}`"
    );
}

#[test]
fn unknown_keys_are_errors_naming_the_key() {
    let cases = [
        (format!("{VALID_HEAD}bogus_key = 1\n[buttons]\nstart = [ \"key:30\" ]\n"), "bogus_key"),
        (format!("{VALID_HEAD}[buttons]\nstart = [ \"key:30\" ]\n[wibble_table]\nx = 1\n"), "wibble_table"),
        (
            format!(
                "{VALID_HEAD}[buttons]\nstart = [ \"key:30\" ]\n[[axes]]\nraw = \"left_x\"\naxis = \"nudge_x\"\nfrom = -1.0\nto = 1.0\ninvert = false\nscale_typo = 2.0\n"
            ),
            "scale_typo",
        ),
        (format!("{VALID_HEAD}[buttons]\nstart = [ \"key:30\" ]\n[nudge]\nupward = [ \"key:1\" ]\n"), "upward"),
        (
            "id = \"t\"\nname = \"T\"\nmatch = [ { mouse_thing = true } ]\n[buttons]\nstart = [ \"key:30\" ]\n".to_string(),
            "mouse_thing",
        ),
        (
            "id = \"t\"\nname = \"T\"\nmatch = [ { gamepad = { vendor = 1, product = 2, version_typo = 3 } } ]\n[buttons]\nstart = [ \"key:30\" ]\n"
                .to_string(),
            "version_typo",
        ),
    ];
    for (text, key) in cases {
        let msg = parse_err(&text);
        check_error_format(&msg);
        assert!(
            msg.contains(key),
            "error `{msg}` should name the unknown key `{key}`"
        );
    }
}

#[test]
fn unknown_cabinet_names_are_errors_naming_the_name() {
    for bad in [
        "coin",
        "LeftFlipper",
        "left-flipper",
        "flipper_left",
        "plunger",
    ] {
        let text =
            format!("{VALID_HEAD}[buttons]\nstart = [ \"key:30\" ]\n{bad} = [ \"key:31\" ]\n");
        let msg = parse_err(&text);
        check_error_format(&msg);
        assert!(msg.contains(bad), "error `{msg}` should name `{bad}`");
    }
    for bad in ["spinner", "start", "Plunger"] {
        let text = format!(
            "{VALID_HEAD}[buttons]\nstart = [ \"key:30\" ]\n[[axes]]\nraw = \"left_x\"\naxis = \"{bad}\"\nfrom = -1.0\nto = 1.0\ninvert = false\n"
        );
        let msg = parse_err(&text);
        check_error_format(&msg);
    }
    for bad in ["middle_x", "leftx", "south", ""] {
        let text = format!(
            "{VALID_HEAD}[buttons]\nstart = [ \"key:30\" ]\n[[axes]]\nraw = \"{bad}\"\naxis = \"plunger\"\nfrom = 0.0\nto = 1.0\ninvert = false\n"
        );
        let msg = parse_err(&text);
        check_error_format(&msg);
    }
}

#[test]
fn malformed_raw_specs_are_errors() {
    let bad = [
        "",
        "225",
        "key",
        "key:",
        "key:abc",
        "key:-1",
        "key:1.5",
        "key:99999999999",
        "key:1:2",
        "button:",
        "button:nope",
        "button:left_x",
        "button:9",
        "frob:1",
        ":225",
        "key=225",
        "keyboard:225",
    ];
    for spec in bad {
        let text = format!("{VALID_HEAD}[buttons]\nstart = [ \"key:30\", \"{spec}\" ]\n");
        let msg = parse_err(&text);
        check_error_format(&msg);
        let text = format!(
            "{VALID_HEAD}[buttons]\nstart = [ \"key:30\" ]\n[nudge]\nleft = [ \"{spec}\" ]\n"
        );
        let msg = parse_err(&text);
        check_error_format(&msg);
    }
}

#[test]
fn wrong_value_types_and_ranges_are_errors() {
    let cases = [
        "id = 5\nname = \"T\"\nmatch = [ { any_gamepad = true } ]\n[buttons]\nstart = [ \"key:30\" ]\n".to_string(),
        "id = \"t\"\nname = [ \"T\" ]\nmatch = [ { any_gamepad = true } ]\n[buttons]\nstart = [ \"key:30\" ]\n".to_string(),
        "id = \"t\"\nname = \"T\"\nmatch = \"keyboard\"\n[buttons]\nstart = [ \"key:30\" ]\n".to_string(),
        "id = \"t\"\nname = \"T\"\nmatch = [ { gamepad = { vendor = 0x12345, product = 1 } } ]\n[buttons]\nstart = [ \"key:30\" ]\n"
            .to_string(),
        "id = \"t\"\nname = \"T\"\nmatch = [ { gamepad = { vendor = 1, product = -1 } } ]\n[buttons]\nstart = [ \"key:30\" ]\n"
            .to_string(),
        "id = \"t\"\nname = \"T\"\nmatch = [ { gamepad = { vendor = \"045e\", product = 1 } } ]\n[buttons]\nstart = [ \"key:30\" ]\n"
            .to_string(),
        "id = \"t\"\nname = \"T\"\nmatch = [ { gamepad = { vendor = 1 } } ]\n[buttons]\nstart = [ \"key:30\" ]\n".to_string(),
        format!("{VALID_HEAD}[buttons]\nstart = [ 30 ]\n"),
        format!("{VALID_HEAD}[buttons]\nstart = 30\n"),
        format!("{VALID_HEAD}buttons = \"start\"\n"),
        format!(
            "{VALID_HEAD}[buttons]\nstart = [ \"key:30\" ]\n[[axes]]\nraw = \"left_x\"\naxis = \"nudge_x\"\nfrom = \"low\"\nto = 1.0\ninvert = false\n"
        ),
        format!(
            "{VALID_HEAD}[buttons]\nstart = [ \"key:30\" ]\n[[axes]]\nraw = \"left_x\"\naxis = \"nudge_x\"\nfrom = -1.0\nto = 1.0\ninvert = \"no\"\n"
        ),
        format!("{VALID_HEAD}[buttons]\nstart = [ \"key:30\" ]\n[nudge]\nleft = \"sideways\"\n"),
    ];
    for text in cases {
        let msg = parse_err(&text);
        check_error_format(&msg);
    }
}

#[test]
fn a_profile_without_an_id_is_an_error() {
    for text in [
        "",
        "\n",
        "# only a comment\n",
        "name = \"T\"\nmatch = [ { any_gamepad = true } ]\n[buttons]\nstart = [ \"key:30\" ]\n",
    ] {
        let msg = parse_err(text);
        check_error_format(&msg);
    }
}

#[test]
fn duplicate_toml_keys_are_errors() {
    let text = format!("{VALID_HEAD}[buttons]\nstart = [ \"key:30\" ]\nstart = [ \"key:31\" ]\n");
    let msg = parse_err(&text);
    check_error_format(&msg);
}

// ---------------------------------------------------------------------------
// Mapper properties
// ---------------------------------------------------------------------------

fn cabinet_axis_name(a: CabinetAxis) -> &'static str {
    match a {
        CabinetAxis::Plunger => "plunger",
        CabinetAxis::NudgeX => "nudge_x",
        CabinetAxis::NudgeY => "nudge_y",
        CabinetAxis::LeftFlipper => "left_flipper",
        CabinetAxis::RightFlipper => "right_flipper",
    }
}

fn cabinet_axis_range(a: CabinetAxis) -> (f32, f32) {
    match a {
        CabinetAxis::NudgeX | CabinetAxis::NudgeY => (-1.0, 1.0),
        CabinetAxis::Plunger | CabinetAxis::LeftFlipper | CabinetAxis::RightFlipper => (0.0, 1.0),
    }
}

const RAW_AXIS_NAMES: [&str; AXIS_COUNT] = [
    "left_x",
    "left_y",
    "right_x",
    "right_y",
    "left_trigger",
    "right_trigger",
];

fn axis_profile(raw: usize, cab: CabinetAxis, from: &str, to: &str, invert: bool) -> String {
    format!(
        "{VALID_HEAD}[buttons]\nstart = [ \"key:30\" ]\n\n[[axes]]\nraw = \"{}\"\naxis = \"{}\"\nfrom = {from}\nto = {to}\ninvert = {invert}\n",
        RAW_AXIS_NAMES[raw],
        cabinet_axis_name(cab)
    )
}

fn normalised(raw_axis: usize, raw: i16) -> f32 {
    let n = f32::from(raw) / 32767.0;
    if raw_axis >= 4 {
        n.clamp(0.0, 1.0)
    } else {
        n.clamp(-1.0, 1.0)
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// raw normalised, mapped linearly from [from, to] onto the cabinet axis
    /// range, clamped (brief, `AxisBinding`).
    #[test]
    fn axis_mapping_is_linear_from_to_and_clamped(
        ri in 0..AXIS_COUNT,
        ci in 0usize..5,
        from in -1.0f32..0.8,
        span in 0.1f32..2.0,
        raws in prop::collection::vec(any::<i16>(), 1..24),
    ) {
        let cab = CabinetAxis::ALL[ci];
        let to = from + span;
        let text = axis_profile(ri, cab, &format!("{from:?}"), &format!("{to:?}"), false);
        let p = parse_ok(&text);
        prop_assert_eq!(p.axes[0].from, from);
        prop_assert_eq!(p.axes[0].to, to);
        let m = ProfileMapper::new(&p);
        let (lo, hi) = cabinet_axis_range(cab);
        for raw in raws.into_iter().chain([i16::MIN, -32767, 0, 32767]) {
            let (a, v) = axis_value(m.axis(1, axis(ri), raw)).expect("bound");
            prop_assert_eq!(a, cab);
            let t = ((normalised(ri, raw) - from) / (to - from)).clamp(0.0, 1.0);
            let want = lo + (hi - lo) * t;
            prop_assert!((v - want).abs() < 2e-4, "raw {} -> {} vs {}", raw, v, want);
            prop_assert!((lo..=hi).contains(&v));
        }
    }

    /// Non-inverted bindings with from < to never decrease as the raw value grows.
    #[test]
    fn axis_mapping_is_monotonic(
        ri in 0..AXIS_COUNT,
        ci in 0usize..5,
        from in -1.0f32..0.8,
        span in 0.1f32..2.0,
        mut raws in prop::collection::vec(any::<i16>(), 2..32),
    ) {
        let cab = CabinetAxis::ALL[ci];
        let to = from + span;
        let p = parse_ok(&axis_profile(ri, cab, &format!("{from:?}"), &format!("{to:?}"), false));
        let m = ProfileMapper::new(&p);
        raws.sort_unstable();
        let mut last = f32::NEG_INFINITY;
        for raw in raws {
            let (_, v) = axis_value(m.axis(0, axis(ri), raw)).expect("bound");
            prop_assert!(v >= last - 1e-6, "raw {} gave {} after {}", raw, v, last);
            last = v;
        }
    }

    /// A symmetric stick binding with `invert = true` is the mirror image of
    /// the same binding without it.
    #[test]
    fn invert_mirrors_a_symmetric_stick(ri in 0usize..4, y in any::<bool>(), raw in any::<i16>()) {
        let cab = if y { CabinetAxis::NudgeY } else { CabinetAxis::NudgeX };
        let plain = parse_ok(&axis_profile(ri, cab, "-1.0", "1.0", false));
        let inv = parse_ok(&axis_profile(ri, cab, "-1.0", "1.0", true));
        prop_assert!(inv.axes[0].invert && !plain.axes[0].invert);
        let (_, a) = axis_value(ProfileMapper::new(&plain).axis(0, axis(ri), raw)).unwrap();
        let (_, b) = axis_value(ProfileMapper::new(&inv).axis(0, axis(ri), raw)).unwrap();
        prop_assert!((a + b).abs() < 2e-4, "{} and {} are not mirrored", a, b);
    }

    /// Whatever `from`/`to` a (downloaded, hand-edited) profile carries, a
    /// profile the parser accepts never yields a NaN or out-of-range value:
    /// the brief says the mapping is clamped to the cabinet axis range.
    #[test]
    fn accepted_axis_bindings_always_yield_values_in_range(
        ri in 0..AXIS_COUNT,
        ci in 0usize..5,
        from in prop_oneof![
            Just("0.0".to_string()), Just("1.0".to_string()), Just("-1.0".to_string()),
            Just("nan".to_string()), Just("inf".to_string()), Just("-inf".to_string()),
            Just("1e30".to_string()), Just("-1e30".to_string()), Just("1e-40".to_string()),
            (-3.0f32..3.0).prop_map(|v| format!("{v:?}")),
        ],
        to in prop_oneof![
            Just("0.0".to_string()), Just("1.0".to_string()), Just("-1.0".to_string()),
            Just("nan".to_string()), Just("inf".to_string()), Just("-inf".to_string()),
            Just("1e30".to_string()), Just("-1e30".to_string()), Just("1e-40".to_string()),
            (-3.0f32..3.0).prop_map(|v| format!("{v:?}")),
        ],
        invert in any::<bool>(),
        raws in prop::collection::vec(any::<i16>(), 1..16),
    ) {
        let cab = CabinetAxis::ALL[ci];
        let text = axis_profile(ri, cab, &from, &to, invert);
        if let Ok(p) = parse_profile(&text) {
            let m = ProfileMapper::new(&p);
            let (lo, hi) = cabinet_axis_range(cab);
            for raw in raws.into_iter().chain([i16::MIN, -1, 0, 1, i16::MAX]) {
                if let Some((a, v)) = axis_value(m.axis(0, axis(ri), raw)) {
                    prop_assert_eq!(a, cab);
                    prop_assert!(
                        v.is_finite() && (lo..=hi).contains(&v),
                        "from {} to {} invert {} raw {} -> {}", from, to, invert, raw, v
                    );
                }
            }
        }
    }

    /// Random valid button tables parse to exactly the listed pairs and every
    /// pair is honoured by the mapper.
    #[test]
    fn random_button_tables_round_trip(
        keys in prop::collection::btree_map(0u32..512, 0usize..8, 1..24),
        pads in prop::collection::btree_map(0usize..15, 0usize..8, 0..10),
        ts in any::<u64>(),
        pressed in any::<bool>(),
    ) {
        const NAMES: [&str; 15] = [
            "south", "east", "west", "north", "back", "guide", "start", "left_stick",
            "right_stick", "left_shoulder", "right_shoulder", "dpad_up", "dpad_down",
            "dpad_left", "dpad_right",
        ];
        let mut text = String::from(VALID_HEAD);
        text.push_str("[buttons]\n");
        for cab in CabinetInput::ALL {
            let mut specs: Vec<String> = Vec::new();
            for (k, c) in &keys {
                if CabinetInput::ALL[*c] == cab {
                    specs.push(format!("\"key:{k}\""));
                }
            }
            for (b, c) in &pads {
                if CabinetInput::ALL[*c] == cab {
                    specs.push(format!("\"button:{}\"", NAMES[*b]));
                }
            }
            if !specs.is_empty() {
                text.push_str(&format!("{} = [ {} ]\n", cab.name(), specs.join(", ")));
            }
        }
        let p = parse_ok(&text);
        prop_assert_eq!(p.buttons.len(), keys.len() + pads.len());
        prop_assert!(p.axes.is_empty());
        prop_assert!(p.nudge_buttons.is_empty());
        let m = ProfileMapper::new(&p);
        for sc in 0u32..512 {
            let want = keys.get(&sc).map(|c| InputEvent::button(ts, CabinetInput::ALL[*c], pressed));
            prop_assert_eq!(m.key(ts, sc, pressed), want, "scancode {}", sc);
        }
        for b in 0..BUTTON_COUNT {
            let want = pads.get(&b).map(|c| InputEvent::button(ts, CabinetInput::ALL[*c], pressed));
            prop_assert_eq!(m.button(ts, button(b), pressed), want, "button {}", b);
        }
        for a in 0..AXIS_COUNT {
            prop_assert_eq!(m.axis(ts, axis(a), 1000), None);
        }
    }
}
