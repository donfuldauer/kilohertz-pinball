//! Independent spec tests for Subphase 3.2, verification criterion 2: the
//! profile parser survives bounded random-bytes and random-TOML runs without
//! panicking, and whatever it accepts can be turned into a working mapper.

use pin_core::input::{CabinetInput, InputEventKind};
use pin_input::profiles::{
    DeviceProfile, GamepadAxis, GamepadButton, ProfileMapper, RawInput, parse_profile,
};
use proptest::prelude::*;

const EXAMPLE: &str = include_str!("spec_fixtures/profiles/example_pad.toml");

/// Exercises an accepted profile: building the mapper and sending it events
/// must not panic, and every event it emits must be well formed.
fn exercise(p: &DeviceProfile) {
    let m = ProfileMapper::new(p);
    for sc in [0u32, 4, 30, 40, 44, 224, 225, 229, 511, 512, u32::MAX] {
        if let Some(ev) = m.key(1, sc, true) {
            assert!(matches!(
                ev.kind,
                InputEventKind::Button { pressed: true, .. }
            ));
        }
    }
    for b in [
        GamepadButton::South,
        GamepadButton::Start,
        GamepadButton::LeftShoulder,
        GamepadButton::Misc1,
        GamepadButton::LeftPaddle2,
        GamepadButton::Touchpad,
    ] {
        let _ = m.button(2, b, false);
    }
    for raw in [i16::MIN, -1, 0, 1, i16::MAX] {
        for a in [
            GamepadAxis::LeftX,
            GamepadAxis::LeftY,
            GamepadAxis::RightX,
            GamepadAxis::RightY,
            GamepadAxis::LeftTrigger,
            GamepadAxis::RightTrigger,
        ] {
            if let Some(ev) = m.axis(3, a, raw) {
                match ev.kind {
                    InputEventKind::Axis { value, .. } => {
                        assert!(
                            value.is_finite() && (-1.0..=1.0).contains(&value),
                            "axis value {value} out of range"
                        );
                    }
                    InputEventKind::Button { .. } => panic!("axis produced a button event"),
                }
            }
        }
    }
    let _ = m.nudge_button(RawInput::Key(224));
    let _ = m.nudge_button(RawInput::Button(GamepadButton::DpadLeft));
    let _ = m.nudge_button(RawInput::Axis(GamepadAxis::LeftX));
    for (_, c) in &p.buttons {
        assert!(CabinetInput::ALL.contains(c));
    }
}

fn run(text: &str) {
    match parse_profile(text) {
        Ok(p) => exercise(&p),
        Err(e) => {
            let msg = e.to_string();
            assert!(!msg.is_empty(), "an error must describe itself");
        }
    }
}

fn fragment() -> impl Strategy<Value = String> {
    let word = prop_oneof![
        Just("id"),
        Just("name"),
        Just("match"),
        Just("buttons"),
        Just("axes"),
        Just("nudge"),
        Just("raw"),
        Just("axis"),
        Just("from"),
        Just("to"),
        Just("invert"),
        Just("left_flipper"),
        Just("right_flipper"),
        Just("left_upper"),
        Just("right_upper"),
        Just("action"),
        Just("start"),
        Just("launch"),
        Just("tilt"),
        Just("left"),
        Just("right"),
        Just("forward"),
        Just("gamepad"),
        Just("keyboard"),
        Just("any_gamepad"),
        Just("vendor"),
        Just("product"),
        Just("plunger"),
        Just("nudge_x"),
        Just("left_trigger"),
        Just("coin"),
    ]
    .prop_map(str::to_string);
    let value = prop_oneof![
        Just("true".to_string()),
        Just("false".to_string()),
        Just("nan".to_string()),
        Just("-inf".to_string()),
        Just("[]".to_string()),
        Just("{}".to_string()),
        Just("\"\"".to_string()),
        Just("[ { keyboard = true } ]".to_string()),
        Just("[ { any_gamepad = true } ]".to_string()),
        Just("[ { gamepad = { vendor = 0x1234, product = 0x5678 } } ]".to_string()),
        Just("[ { gamepad = { vendor = 99999999, product = -4 } } ]".to_string()),
        Just("[ \"key:225\", \"button:left_shoulder\" ]".to_string()),
        Just("[ \"key:\", \"button:\" ]".to_string()),
        Just("[ \"axis:left_x\" ]".to_string()),
        Just("\"left_trigger\"".to_string()),
        Just("\"left_flipper\"".to_string()),
        Just("1979-05-27T07:32:00Z".to_string()),
        any::<i64>().prop_map(|v| v.to_string()),
        any::<f64>().prop_map(|v| format!("{v:?}")),
        "[a-z_:0-9]{0,12}".prop_map(|s| format!("\"{s}\"")),
        "[a-z_:0-9]{0,12}".prop_map(|s| format!("[ \"key:{s}\", \"button:{s}\", \"{s}\" ]")),
        "\\PC{0,12}",
    ];
    prop_oneof![
        4 => (word.clone(), value).prop_map(|(k, v)| format!("{k} = {v}\n")),
        1 => word.clone().prop_map(|k| format!("[{k}]\n")),
        1 => word.clone().prop_map(|k| format!("[[{k}]]\n")),
        1 => (word.clone(), word).prop_map(|(a, b)| format!("[{a}.{b}]\n")),
        1 => "\\PC{0,20}".prop_map(|s| format!("{s}\n")),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    #[test]
    fn parser_survives_random_bytes(bytes in prop::collection::vec(any::<u8>(), 0..512)) {
        run(&String::from_utf8_lossy(&bytes));
    }

    #[test]
    fn parser_survives_random_unicode(text in "\\PC{0,256}") {
        run(&text);
    }

    #[test]
    fn parser_survives_random_toml_shaped_text(lines in prop::collection::vec(fragment(), 0..24)) {
        run(&lines.concat());
    }

    /// Byte-level damage to a valid profile: delete, replace or insert.
    #[test]
    fn parser_survives_mutations_of_a_valid_profile(
        edits in prop::collection::vec((any::<prop::sample::Index>(), 0u8..3, any::<u8>()), 1..8),
    ) {
        let mut bytes = EXAMPLE.as_bytes().to_vec();
        for (at, op, b) in edits {
            if bytes.is_empty() {
                break;
            }
            let i = at.index(bytes.len());
            match op {
                0 => {
                    bytes.remove(i);
                }
                1 => bytes[i] = b,
                _ => bytes.insert(i, b),
            }
        }
        run(&String::from_utf8_lossy(&bytes));
    }

    /// Line-level damage: drop, duplicate and shuffle lines of a valid profile.
    #[test]
    fn parser_survives_line_shuffles_of_a_valid_profile(
        picks in prop::collection::vec(any::<prop::sample::Index>(), 0..60),
    ) {
        let lines: Vec<&str> = EXAMPLE.lines().collect();
        let mut text = String::new();
        for p in picks {
            text.push_str(lines[p.index(lines.len())]);
            text.push('\n');
        }
        run(&text);
    }

    /// Truncation at every kind of boundary.
    #[test]
    fn parser_survives_truncation(cut in any::<prop::sample::Index>()) {
        let n = cut.index(EXAMPLE.len() + 1);
        run(&String::from_utf8_lossy(&EXAMPLE.as_bytes()[..n]));
    }
}

#[test]
fn parser_survives_pathological_inputs() {
    let deep_array = format!("id = {}{}\n", "[".repeat(2_000), "]".repeat(2_000));
    let deep_table = format!("id = {}1{}\n", "{ a = ".repeat(500), " }".repeat(500));
    let long_key = format!("{} = 1\n", "k".repeat(100_000));
    let many_specs = format!(
        "id = \"t\"\nname = \"T\"\nmatch = [ {{ keyboard = true }} ]\n[buttons]\nstart = [ {} ]\n",
        (0..5_000)
            .map(|i| format!("\"key:{i}\""))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let many_matches = format!(
        "id = \"t\"\nname = \"T\"\nmatch = [ {} ]\n[buttons]\nstart = [ \"key:30\" ]\n",
        (0..3_000)
            .map(|i| format!(
                "{{ gamepad = {{ vendor = {}, product = {} }} }}",
                i % 65536,
                i % 7
            ))
            .collect::<Vec<_>>()
            .join(", ")
    );
    let huge_scancode = "id = \"t\"\nname = \"T\"\nmatch = [ { keyboard = true } ]\n[buttons]\nstart = [ \"key:4294967295\" ]\n";
    for text in [
        deep_array.as_str(),
        deep_table.as_str(),
        long_key.as_str(),
        many_specs.as_str(),
        many_matches.as_str(),
        huge_scancode,
        "\u{feff}id = \"t\"\n",
        "\0\0\0\0",
        "id = \"\u{0}\"\n",
        "[buttons]\n[buttons]\n",
        "[[buttons]]\n",
        "[axes]\nraw = \"left_x\"\n",
        "axes = [ 1, 2, 3 ]\n",
        "match = [ 1, \"keyboard\", { } ]\n",
        "match = [ { keyboard = false } ]\n",
        "match = [ { keyboard = true, any_gamepad = true } ]\n",
    ] {
        run(text);
    }
}
