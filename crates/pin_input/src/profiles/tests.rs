use super::*;
use pin_core::input::{InputEvent, InputEventKind};

fn ok(text: &str) -> DeviceProfile {
    match parse_profile(text) {
        Ok(p) => p,
        Err(e) => panic!("{e}\n{text}"),
    }
}

fn err(text: &str) -> String {
    match parse_profile(text) {
        Ok(p) => panic!("accepted: {p:?}\n{text}"),
        Err(e) => e.to_string(),
    }
}

const HEAD: &str = "id = \"t\"\nname = \"T\"\n";

#[test]
fn guid_parse_and_layout() {
    let g = SdlGuid::parse_hex("03000000550a0000A1b2000000000000").unwrap();
    assert_eq!(g.bus(), 3);
    assert_eq!(g.vendor(), 0x0a55);
    assert_eq!(g.product(), 0xb2a1);
    assert_eq!(g.to_string(), "03000000550a0000a1b2000000000000");
    assert_eq!(SdlGuid::parse_hex(&g.to_string()), Some(g));
    assert_eq!(SdlGuid::from_ids(3, 0x0a55, 0xb2a1), g);
    assert_eq!(SdlGuid::parse_hex("0300"), None);
    assert_eq!(SdlGuid::parse_hex("g3000000550a0000a1b2000000000000"), None);
    assert_eq!(SdlGuid::parse_hex("03000000550a0000a1b20000000000é"), None);
}

#[test]
fn names_round_trip() {
    for (i, b) in GamepadButton::ALL.into_iter().enumerate() {
        assert_eq!(b as usize, i);
        assert_eq!(GamepadButton::from_name(b.name()), Some(b));
        let r = RawInput::Button(b);
        assert_eq!(RawInput::parse(&r.to_string()), Some(r));
    }
    for (i, a) in GamepadAxis::ALL.into_iter().enumerate() {
        assert_eq!(a as usize, i);
        assert_eq!(GamepadAxis::from_name(a.name()), Some(a));
        let r = RawInput::Axis(a);
        assert_eq!(RawInput::parse(&r.to_string()), Some(r));
    }
    for a in CabinetAxis::ALL {
        assert_eq!(cabinet_axis_from_name(cabinet_axis_name(a)), Some(a));
    }
    for d in NudgeDirection::ALL {
        assert_eq!(NudgeDirection::from_name(d.name()), Some(d));
    }
    assert_eq!(RawInput::parse("key:225"), Some(RawInput::Key(225)));
    assert_eq!(RawInput::parse("key:0xE1"), Some(RawInput::Key(225)));
    for bad in [
        "key:",
        "key:-1",
        "key:+1",
        "key:0x",
        "key:0x+1",
        "key:1a",
        "key:99999999999",
        "button:",
        "button:South",
        "axis:z",
        "pedal:1",
        "key225",
        "",
    ] {
        assert_eq!(RawInput::parse(bad), None, "{bad}");
    }
}

#[test]
fn builtins_parse_and_cover_inputs() {
    let all = builtin_profiles();
    let ids: Vec<&str> = all.iter().map(|p| p.id.as_str()).collect();
    for id in [
        "generic_keyboard",
        "generic_gamepad",
        "xarcade_keyboard",
        "xarcade_gamepad",
    ] {
        assert!(ids.contains(&id), "{id}");
    }
    for p in all {
        for c in [
            CabinetInput::LeftFlipper,
            CabinetInput::RightFlipper,
            CabinetInput::Start,
            CabinetInput::Launch,
        ] {
            assert!(p.maps(c), "{} misses {c:?}", p.id);
        }
        if p.id.starts_with("xarcade_") {
            for c in CabinetInput::ALL {
                assert!(p.maps(c), "{} misses {c:?}", p.id);
            }
        }
    }
    assert!(builtin_profile("nope").is_none());
    // Generic presets reproduce Subphase 1.3's keymap completely too.
    for c in CabinetInput::ALL {
        assert!(builtin_profile("generic_keyboard").unwrap().maps(c));
        assert!(builtin_profile("generic_gamepad").unwrap().maps(c));
    }
}

#[test]
fn detection() {
    assert_eq!(detect(None, true).id, "generic_keyboard");
    assert_eq!(
        detect(Some(SdlGuid::from_ids(3, 0xAA55, 0x0101)), true).id,
        "generic_keyboard"
    );
    assert_eq!(detect(None, false).id, "generic_gamepad");
    assert_eq!(
        detect(Some(SdlGuid::from_ids(3, 0x045e, 0x028e)), false).id,
        "generic_gamepad"
    );
    for product in [0x0101, 0x0102] {
        assert_eq!(
            detect(Some(SdlGuid::from_ids(3, 0xAA55, product)), false).id,
            "xarcade_gamepad"
        );
    }
    // detect_in: exact match wins over an earlier AnyGamepad; slice order otherwise.
    let any = ok(&format!("{HEAD}match = [{{ any_gamepad = true }}]"));
    let exact = ok(
        "id = \"e\"\nname = \"E\"\nmatch = [{ gamepad = { vendor = 0x1234, product = 0x5678 } }]",
    );
    let list = vec![any.clone(), exact.clone()];
    let g = SdlGuid::from_ids(5, 0x1234, 0x5678);
    assert_eq!(detect_in(&list, Some(g), false).unwrap().id, "e");
    assert_eq!(detect_in(&list, None, false).unwrap().id, "t");
    assert!(detect_in(&list, None, true).is_none());
    assert!(detect_in(&[exact], Some(SdlGuid([0; 16])), false).is_none());
    // A detection fallback with no matching built-in entry still picks a profile.
    assert!(detect_in(&[], None, false).is_none());
}

fn button_of(e: Option<InputEvent>) -> Option<(CabinetInput, bool)> {
    match e?.kind {
        InputEventKind::Button { input, pressed } => Some((input, pressed)),
        InputEventKind::Axis { .. } => None,
    }
}

fn axis_of(e: Option<InputEvent>) -> Option<(CabinetAxis, f32)> {
    match e?.kind {
        InputEventKind::Axis { axis, value } => Some((axis, value)),
        InputEventKind::Button { .. } => None,
    }
}

#[test]
fn generic_keyboard_equals_1_3_keymap() {
    let m = ProfileMapper::new(builtin_profile("generic_keyboard").unwrap());
    // SDL scancodes: LShift 225, RShift 229, LCtrl 224, RCtrl 228, Space 44,
    // Return 40, KpEnter 88, 1 = 30, A = 4, T = 23.
    let cases = [
        (225, CabinetInput::LeftFlipper),
        (229, CabinetInput::RightFlipper),
        (224, CabinetInput::LeftUpper),
        (228, CabinetInput::RightUpper),
        (44, CabinetInput::Launch),
        (40, CabinetInput::Launch),
        (88, CabinetInput::Launch),
        (30, CabinetInput::Start),
        (4, CabinetInput::Action),
        (23, CabinetInput::Tilt),
    ];
    for (sc, c) in cases {
        assert_eq!(button_of(m.key(7, sc, true)), Some((c, true)));
        assert_eq!(m.key(7, sc, false).unwrap().timestamp_ns, 7);
    }
    for sc in 0..600u32 {
        if !cases.iter().any(|(k, _)| *k == sc) {
            assert_eq!(m.key(0, sc, true), None, "{sc}");
        }
    }
}

#[test]
fn generic_gamepad_equals_1_3_map() {
    let m = ProfileMapper::new(builtin_profile("generic_gamepad").unwrap());
    let cases = [
        (GamepadButton::LeftShoulder, CabinetInput::LeftFlipper),
        (GamepadButton::RightShoulder, CabinetInput::RightFlipper),
        (GamepadButton::LeftStick, CabinetInput::LeftUpper),
        (GamepadButton::RightStick, CabinetInput::RightUpper),
        (GamepadButton::South, CabinetInput::Launch),
        (GamepadButton::East, CabinetInput::Action),
        (GamepadButton::Start, CabinetInput::Start),
        (GamepadButton::Back, CabinetInput::Tilt),
    ];
    for b in GamepadButton::ALL {
        let want = cases
            .iter()
            .find(|(x, _)| *x == b)
            .map(|(_, c)| (*c, false));
        assert_eq!(button_of(m.button(1, b, false)), want, "{b:?}");
    }
    // 1.3's axis_normalisation cases.
    assert_eq!(
        axis_of(m.axis(0, GamepadAxis::LeftTrigger, 32767)),
        Some((CabinetAxis::LeftFlipper, 1.0))
    );
    assert_eq!(
        axis_of(m.axis(0, GamepadAxis::RightTrigger, 0)),
        Some((CabinetAxis::RightFlipper, 0.0))
    );
    assert_eq!(
        axis_of(m.axis(0, GamepadAxis::LeftY, -32768)),
        Some((CabinetAxis::Plunger, 0.0))
    );
    assert_eq!(
        axis_of(m.axis(0, GamepadAxis::RightX, -32768)),
        Some((CabinetAxis::NudgeX, -1.0))
    );
    let (a, v) = axis_of(m.axis(0, GamepadAxis::RightY, 16384)).unwrap();
    assert_eq!(a, CabinetAxis::NudgeY);
    assert!((v - 0.5).abs() < 1e-3);
    assert_eq!(m.axis(0, GamepadAxis::LeftX, 100), None);
    // The full ranges agree with 1.3's formula.
    for raw in (i16::MIN..=i16::MAX).step_by(97) {
        let s = (f32::from(raw) / 32767.0).clamp(-1.0, 1.0);
        let u = s.max(0.0);
        let (_, v) = axis_of(m.axis(0, GamepadAxis::LeftTrigger, raw)).unwrap();
        assert!((v - u).abs() < 1e-6);
        let (_, v) = axis_of(m.axis(0, GamepadAxis::LeftY, raw)).unwrap();
        assert!((v - u).abs() < 1e-6);
        let (_, v) = axis_of(m.axis(0, GamepadAxis::RightX, raw)).unwrap();
        assert!((v - s).abs() < 1e-6);
    }
}

#[test]
fn xarcade_mappers() {
    let k = ProfileMapper::new(builtin_profile("xarcade_keyboard").unwrap());
    assert_eq!(
        button_of(k.key(0, 6, true)),
        Some((CabinetInput::LeftFlipper, true))
    );
    assert_eq!(
        button_of(k.key(0, 34, true)),
        Some((CabinetInput::RightFlipper, true))
    );
    assert_eq!(
        k.nudge_button(RawInput::Key(80)),
        Some(NudgeDirection::Left)
    );
    assert_eq!(
        k.nudge_button(RawInput::Key(79)),
        Some(NudgeDirection::Right)
    );
    assert_eq!(
        k.nudge_button(RawInput::Key(96)),
        Some(NudgeDirection::Forward)
    );
    assert_eq!(k.key(0, 80, true), None);
    let g = ProfileMapper::new(builtin_profile("xarcade_gamepad").unwrap());
    // Side buttons are shoulders only; the triggers are unbound.
    assert_eq!(g.axis(0, GamepadAxis::LeftTrigger, 30000), None);
    assert_eq!(
        button_of(g.button(0, GamepadButton::RightShoulder, true)),
        Some((CabinetInput::RightFlipper, true))
    );
    assert_eq!(
        g.nudge_button(RawInput::Button(GamepadButton::DpadUp)),
        Some(NudgeDirection::Forward)
    );
    assert_eq!(g.nudge_button(RawInput::Axis(GamepadAxis::LeftX)), None);
    assert_eq!(
        g.input_for(RawInput::Button(GamepadButton::West)),
        Some(CabinetInput::LeftUpper)
    );
    assert_eq!(g.input_for(RawInput::Axis(GamepadAxis::LeftTrigger)), None);
    assert_eq!(
        k.input_for(RawInput::Key(6)),
        Some(CabinetInput::LeftFlipper)
    );
}

#[test]
fn full_format_example() {
    let p = ok(r#"
id = "my_pad"
name = "My Pad"
match = [ { gamepad = { vendor = 0x1234, product = 0x5678 } }, { keyboard = true }, { any_gamepad = true } ]
[buttons]
left_flipper = [ "key:225", "button:left_shoulder" ]
launch = "axis:right_trigger"
[[axes]]
raw = "left_trigger"
axis = "left_flipper"
from = 0.0
to = 1.0
invert = false
[[axes]]
raw = "axis:left_x"
axis = "nudge_x"
invert = true
[[axes]]
raw = "left_y"
axis = "plunger"
from = 1
to = 0
[nudge]
left = [ "key:224" ]
forward = "button:dpad_up"
"#);
    assert_eq!(p.id, "my_pad");
    assert_eq!(p.name, "My Pad");
    assert_eq!(
        p.matches,
        vec![
            DeviceMatch::Gamepad {
                vendor: 0x1234,
                product: 0x5678
            },
            DeviceMatch::Keyboard,
            DeviceMatch::AnyGamepad
        ]
    );
    assert_eq!(
        p.buttons,
        vec![
            (RawInput::Key(225), CabinetInput::LeftFlipper),
            (
                RawInput::Button(GamepadButton::LeftShoulder),
                CabinetInput::LeftFlipper
            ),
            (
                RawInput::Axis(GamepadAxis::RightTrigger),
                CabinetInput::Launch
            ),
        ]
    );
    assert_eq!(p.axes.len(), 3);
    assert_eq!(p.axes[1].from, -1.0);
    assert_eq!(p.axes[1].to, 1.0);
    assert!(p.axes[1].invert);
    assert_eq!(
        p.nudge_buttons,
        vec![
            (RawInput::Key(224), NudgeDirection::Left),
            (
                RawInput::Button(GamepadButton::DpadUp),
                NudgeDirection::Forward
            )
        ]
    );
    let m = ProfileMapper::new(&p);
    // Inverted stick to nudge: full right reads full left.
    assert_eq!(
        axis_of(m.axis(0, GamepadAxis::LeftX, 32767)),
        Some((CabinetAxis::NudgeX, -1.0))
    );
    // Reversed range: stick up (-1) clamps to raw 0 → ... from 1 to 0: raw 0 → 1.
    assert_eq!(
        axis_of(m.axis(0, GamepadAxis::LeftY, 0)),
        Some((CabinetAxis::Plunger, 1.0))
    );
    assert_eq!(
        axis_of(m.axis(0, GamepadAxis::LeftY, 32767)),
        Some((CabinetAxis::Plunger, 0.0))
    );
    assert_eq!(
        button_of(m.axis(0, GamepadAxis::RightTrigger, 16384)),
        Some((CabinetInput::Launch, true))
    );
    assert_eq!(
        m.nudge_button(RawInput::Key(224)),
        Some(NudgeDirection::Left)
    );
    // Minimal profile.
    let p = ok(HEAD);
    assert!(p.matches.is_empty() && p.buttons.is_empty() && p.axes.is_empty());
}

#[test]
fn axis_binding_map_edges() {
    let mut b = AxisBinding {
        raw: GamepadAxis::LeftTrigger,
        axis: CabinetAxis::Plunger,
        invert: true,
        from: 0.0,
        to: 1.0,
    };
    assert_eq!(b.map(0.25), 0.75);
    assert_eq!(b.map(f32::NAN), 1.0);
    b.invert = false;
    b.to = 0.0;
    assert_eq!(b.map(0.5), 1.0);
    assert_eq!(b.map(-0.5), 1.0); // trigger clamps to 0 = from → high end
    b.to = f32::NAN;
    assert_eq!(b.map(0.5), 1.0);
    b.raw = GamepadAxis::LeftX;
    b.axis = CabinetAxis::NudgeY;
    b.from = -1.0;
    b.to = 1.0;
    assert_eq!(b.map(0.0), 0.0);
    assert_eq!(b.map(-5.0), -1.0);
    b.from = 0.5;
    b.to = 0.5;
    assert_eq!(b.map(0.0), -1.0);
    assert_eq!(cabinet_axis_range(CabinetAxis::Plunger), (0.0, 1.0));
}

#[test]
fn syntax_errors_have_line_and_col() {
    let e = parse_profile("id = \"x\"\nname = \n").unwrap_err();
    let ProfileError::Syntax { line, col, .. } = &e else {
        panic!("{e:?}");
    };
    assert_eq!(*line, 2);
    assert!(*col >= 1);
    assert!(e.to_string().starts_with("2:"), "{e}");
    let e = err("id = \"x\"\nid = \"y\"\nname = \"n\"");
    assert!(e.starts_with("2:"), "{e}");
}

#[test]
fn semantic_errors_name_the_key() {
    let cases: &[(&str, &str)] = &[
        ("name = \"n\"", "id: missing"),
        ("id = \"x\"", "name: missing"),
        ("id = \"\"\nname = \"n\"", "id: must not be empty"),
        (
            "id = 3\nname = \"n\"",
            "id: expected a string, found an integer",
        ),
        ("bogus = 1", "bogus: unknown key"),
        (
            "[buttons]\nleft_flpper = \"key:1\"",
            "buttons.left_flpper: unknown cabinet input",
        ),
        (
            "[buttons]\nstart = \"key:x\"",
            "buttons.start: malformed raw input",
        ),
        (
            "[buttons]\nstart = [\"key:1\", \"btn:a\"]",
            "buttons.start[1]: malformed",
        ),
        (
            "[buttons]\nstart = 5",
            "buttons.start: expected a raw input string",
        ),
        (
            "[buttons]\nstart = [5]",
            "buttons.start[0]: expected a string",
        ),
        (
            "[buttons]\nstart = \"key:1\"\ntilt = \"key:1\"",
            "already bound",
        ),
        ("buttons = 1", "buttons: expected a table"),
        (
            "[nudge]\nup = \"key:1\"",
            "nudge.up: unknown nudge direction",
        ),
        (
            "[nudge]\nleft = \"key:1\"\nright = \"key:1\"",
            "already bound to nudge",
        ),
        ("nudge = []", "nudge: expected a table"),
        ("match = 1", "match: expected an array"),
        ("match = [1]", "match[0]: expected a table"),
        ("match = [{}]", "match[0]: expected exactly one"),
        (
            "match = [{ keyboard = true, any_gamepad = true }]",
            "expected exactly one",
        ),
        (
            "match = [{ keyboard = false }]",
            "match[0].keyboard: must be true",
        ),
        (
            "match = [{ keyboard = 1 }]",
            "match[0].keyboard: expected a boolean",
        ),
        (
            "match = [{ mouse = true }]",
            "match[0].mouse: unknown match kind",
        ),
        (
            "match = [{ gamepad = 1 }]",
            "match[0].gamepad: expected a table",
        ),
        (
            "match = [{ gamepad = { vendor = 1 } }]",
            "match[0].gamepad.product: missing",
        ),
        (
            "match = [{ gamepad = { product = 1 } }]",
            "match[0].gamepad.vendor: missing",
        ),
        (
            "match = [{ gamepad = { vendor = 1, product = 70000 } }]",
            "match[0].gamepad.product: expected an integer in 0..=0xFFFF",
        ),
        (
            "match = [{ gamepad = { vendor = -1, product = 1 } }]",
            "match[0].gamepad.vendor: expected an integer",
        ),
        (
            "match = [{ gamepad = { vendor = \"a\", product = 1 } }]",
            "match[0].gamepad.vendor: expected an integer, found a string",
        ),
        (
            "match = [{ gamepad = { vendor = 1, product = 1, bus = 3 } }]",
            "match[0].gamepad.bus: unknown key",
        ),
        ("axes = 1", "axes: expected an array"),
        ("axes = [1]", "axes[0]: expected a table"),
        ("[[axes]]\naxis = \"plunger\"", "axes[0].raw: missing"),
        ("[[axes]]\nraw = \"left_y\"", "axes[0].axis: missing"),
        (
            "[[axes]]\nraw = \"left_z\"\naxis = \"plunger\"",
            "axes[0].raw: unknown gamepad axis",
        ),
        (
            "[[axes]]\nraw = 1\naxis = \"plunger\"",
            "axes[0].raw: expected a string",
        ),
        (
            "[[axes]]\nraw = \"left_y\"\naxis = \"flippy\"",
            "axes[0].axis: unknown cabinet axis",
        ),
        (
            "[[axes]]\nraw = \"left_y\"\naxis = \"plunger\"\nfrom = 1.0",
            "axes[0].to: `from` and `to` must differ",
        ),
        (
            "[[axes]]\nraw = \"left_y\"\naxis = \"plunger\"\nfrom = 0.5\nto = 0.5",
            "must differ",
        ),
        (
            "[[axes]]\nraw = \"left_y\"\naxis = \"plunger\"\nfrom = \"a\"",
            "axes[0].from: expected a number",
        ),
        (
            "[[axes]]\nraw = \"left_y\"\naxis = \"plunger\"\nto = nan",
            "axes[0].to: number out of range",
        ),
        (
            "[[axes]]\nraw = \"left_y\"\naxis = \"plunger\"\nto = 1e9",
            "axes[0].to: number out of range",
        ),
        (
            "[[axes]]\nraw = \"left_y\"\naxis = \"plunger\"\ninvert = 1",
            "axes[0].invert: expected a boolean",
        ),
        (
            "[[axes]]\nraw = \"left_y\"\naxis = \"plunger\"\nscale = 1",
            "axes[0].scale: unknown key",
        ),
        (
            "[[axes]]\nraw = \"left_y\"\naxis = \"plunger\"\n[[axes]]\nraw = \"left_y\"\naxis = \"nudge_y\"",
            "axes[1].raw: raw axis left_y is already bound",
        ),
        (
            "[buttons]\nstart = \"axis:left_y\"\n[[axes]]\nraw = \"left_y\"\naxis = \"plunger\"",
            "buttons.start: raw axis axis:left_y is also bound",
        ),
        ("when = 1979-05-27", "when: unknown key"),
    ];
    for (body, want) in cases {
        let text = if body.starts_with("id") || body.starts_with("name") {
            body.to_string()
        } else {
            // Keys before tables so the header stays top-level.
            format!("{HEAD}{body}")
        };
        let e = err(&text);
        assert!(e.contains(want), "{text:?}\n  got: {e}\n  want: {want}");
    }
    // Located errors carry the line.
    let e = err(&format!("{HEAD}[buttons]\nleft_flpper = \"key:1\""));
    assert!(e.ends_with("(line 4, column 1)"), "{e}");
    let pe = parse_profile(&format!("{HEAD}x = 1")).unwrap_err();
    assert!(matches!(pe, ProfileError::Invalid { .. }));
    let _: &dyn std::error::Error = &pe;
}

#[test]
fn hand_built_profile_duplicates_first_wins() {
    let p = DeviceProfile {
        id: "h".into(),
        name: "H".into(),
        matches: vec![],
        buttons: vec![
            (RawInput::Key(9), CabinetInput::Start),
            (RawInput::Key(9), CabinetInput::Tilt),
            (RawInput::Key(u32::MAX), CabinetInput::Tilt),
            (
                RawInput::Button(GamepadButton::Touchpad),
                CabinetInput::Action,
            ),
            (
                RawInput::Button(GamepadButton::Touchpad),
                CabinetInput::Tilt,
            ),
        ],
        axes: vec![],
        nudge_buttons: vec![
            (RawInput::Key(9), NudgeDirection::Left),
            (RawInput::Key(9), NudgeDirection::Right),
            (RawInput::Axis(GamepadAxis::RightY), NudgeDirection::Forward),
        ],
    };
    let m = ProfileMapper::new(&p);
    assert_eq!(
        button_of(m.key(0, 9, true)),
        Some((CabinetInput::Start, true))
    );
    assert_eq!(
        button_of(m.key(0, u32::MAX, true)),
        Some((CabinetInput::Tilt, true))
    );
    assert_eq!(
        button_of(m.button(0, GamepadButton::Touchpad, true)),
        Some((CabinetInput::Action, true))
    );
    assert_eq!(m.nudge_button(RawInput::Key(9)), Some(NudgeDirection::Left));
    assert_eq!(
        m.nudge_button(RawInput::Axis(GamepadAxis::RightY)),
        Some(NudgeDirection::Forward)
    );
    assert_eq!(m.axis(0, GamepadAxis::RightY, 0), None);
}
