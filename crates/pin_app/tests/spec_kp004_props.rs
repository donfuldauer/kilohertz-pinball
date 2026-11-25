//! KP-004 (#84) property tests, REQ-VIEW-006 (SPEC section 7.1) acceptance 2
//! (held inputs released on open, nothing forwarded while open) and the R11
//! parser rule for `config.toml` (bounded random bytes, SPEC 4.4).

#[path = "spec_kp004_support/mod.rs"]
mod support;

use pin_app::ui::MenuController;
use pin_core::{CabinetAxis, CabinetInput, InputEvent, InputEventKind};
use proptest::prelude::*;

#[derive(Clone, Debug)]
enum Op {
    Press(usize),
    Release(usize),
    Axis(usize, usize),
    Toggle,
}

const AXIS_VALUES: [f32; 5] = [0.0, 0.25, 1.0, -1.0, 0.5];

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => (0..CabinetInput::COUNT).prop_map(Op::Press),
        3 => (0..CabinetInput::COUNT).prop_map(Op::Release),
        2 => (0..CabinetAxis::ALL.len(), 0..AXIS_VALUES.len()).prop_map(|(a, v)| Op::Axis(a, v)),
        2 => Just(Op::Toggle),
    ]
}

/// REQ-VIEW-006 acceptance 2 (property): over random press / release / axis /
/// toggle sequences the forwarded stream is time-ordered, silent while open,
/// edge-consistent, and ends fully neutral when the menu ends open.
#[test]
fn req_view_006_acc2_prop_forwarded_stream_invariants() {
    let cfg = ProptestConfig::with_cases(256);
    let mut runner = proptest::test_runner::TestRunner::new(cfg);
    let dir = support::scratch("prop_seq");
    let path = dir.join("config.toml");
    let strat = proptest::collection::vec(op(), 0..80);
    runner
        .run(&strat, |ops| {
            let (mut c, _) = MenuController::load(&path);
            let mut forwarded: Vec<InputEvent> = Vec::new();
            // Physical state of the keys (what the generator has done) and
            // "tainted": the sim's view of the input may differ from the
            // physical one (released at open time, or toggled while open).
            let mut physical = [false; CabinetInput::COUNT];
            let mut tainted = [false; CabinetInput::COUNT];
            let mut model_b = [false; CabinetInput::COUNT];
            let mut model_a = [0.0f32; 5];
            let mut axis_phys = [0.0f32; 5];
            let mut ts = 0u64;
            let mut last_ts = 0u64;

            let mut check = |evs: &[InputEvent],
                             open_time: bool,
                             model_b: &mut [bool; CabinetInput::COUNT],
                             model_a: &mut [f32; 5],
                             tainted: &mut [bool; CabinetInput::COUNT]|
             -> Result<(), TestCaseError> {
                for e in evs {
                    prop_assert!(e.timestamp_ns >= last_ts, "timestamps non-decreasing");
                    last_ts = e.timestamp_ns;
                    match e.kind {
                        InputEventKind::Button { input, pressed } => {
                            let i = input.index();
                            if pressed {
                                prop_assert!(!model_b[i], "double press of {input:?}");
                                model_b[i] = true;
                                tainted[i] = false;
                            } else if model_b[i] {
                                model_b[i] = false;
                                if open_time {
                                    tainted[i] = true;
                                }
                            } else {
                                prop_assert!(
                                    !open_time && tainted[i],
                                    "release of unpressed {input:?}"
                                );
                                tainted[i] = false;
                            }
                        }
                        InputEventKind::Axis { axis, value } => {
                            let k = CabinetAxis::ALL.iter().position(|a| *a == axis).unwrap();
                            model_a[k] = value;
                        }
                    }
                }
                Ok(())
            };

            for o in ops {
                ts += 10;
                let mut out: Vec<InputEvent> = Vec::new();
                let was_open = c.is_open();
                match o {
                    Op::Press(i) => {
                        if physical[i] {
                            continue;
                        }
                        physical[i] = true;
                        if was_open {
                            tainted[i] = true;
                        }
                        c.route(
                            InputEvent::button(ts, CabinetInput::ALL[i], true),
                            &mut |e| out.push(e),
                        );
                    }
                    Op::Release(i) => {
                        if !physical[i] {
                            continue;
                        }
                        physical[i] = false;
                        if was_open {
                            tainted[i] = true;
                        }
                        c.route(
                            InputEvent::button(ts, CabinetInput::ALL[i], false),
                            &mut |e| out.push(e),
                        );
                    }
                    Op::Axis(a, v) => {
                        let value = AXIS_VALUES[v];
                        axis_phys[a] = value;
                        c.route(InputEvent::axis(ts, CabinetAxis::ALL[a], value), &mut |e| {
                            out.push(e)
                        });
                    }
                    Op::Toggle => {
                        c.toggle(ts, &mut |e| out.push(e));
                        prop_assert_ne!(was_open, c.is_open(), "toggle flips");
                        if !was_open {
                            for e in &out {
                                prop_assert_eq!(e.timestamp_ns, ts, "open-time release stamp");
                            }
                        } else {
                            prop_assert!(out.is_empty(), "closing pushes nothing");
                        }
                    }
                }
                if was_open && !matches!(o, Op::Toggle) {
                    prop_assert!(out.is_empty(), "nothing forwarded while open: {out:?}");
                }
                let open_time = matches!(o, Op::Toggle);
                check(&out, open_time, &mut model_b, &mut model_a, &mut tainted)?;
                forwarded.extend(out);
                if c.is_open() {
                    prop_assert!(
                        model_b.iter().all(|b| !b),
                        "open menu: nothing left pressed for the sim"
                    );
                    prop_assert!(
                        model_a.iter().all(|v| *v == 0.0),
                        "open menu: every axis neutral"
                    );
                }
            }
            let _ = forwarded;
            let _ = std::fs::remove_file(&path);
            Ok(())
        })
        .unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

/// REQ-VIEW-006 acceptance 2 (property): opening releases exactly the inputs
/// the sim last saw held (buttons in ALL order, then axes), all with the
/// toggling timestamp.
#[test]
fn req_view_006_acc2_prop_open_releases_exactly_what_sim_holds() {
    let cfg = ProptestConfig::with_cases(256);
    let mut runner = proptest::test_runner::TestRunner::new(cfg);
    let dir = support::scratch("prop_open");
    let path = dir.join("config.toml");
    let strat = (
        proptest::collection::vec(any::<bool>(), CabinetInput::COUNT),
        proptest::collection::vec(0..AXIS_VALUES.len(), 5),
    );
    runner
        .run(&strat, |(btns, axes)| {
            let (mut c, _) = MenuController::load(&path);
            let mut ts = 100u64;
            let mut fwd = Vec::new();
            for (i, on) in btns.iter().enumerate() {
                if *on {
                    ts += 1;
                    c.route(
                        InputEvent::button(ts, CabinetInput::ALL[i], true),
                        &mut |e| fwd.push(e),
                    );
                }
            }
            for (k, v) in axes.iter().enumerate() {
                ts += 1;
                c.route(
                    InputEvent::axis(ts, CabinetAxis::ALL[k], AXIS_VALUES[*v]),
                    &mut |e| fwd.push(e),
                );
            }
            let esc = ts + 1_000;
            let mut rel = Vec::new();
            c.toggle(esc, &mut |e| rel.push(e));
            let mut expected = Vec::new();
            for (i, on) in btns.iter().enumerate() {
                if *on {
                    expected.push(InputEvent::button(esc, CabinetInput::ALL[i], false));
                }
            }
            for (k, v) in axes.iter().enumerate() {
                if AXIS_VALUES[*v] != 0.0 {
                    expected.push(InputEvent::axis(esc, CabinetAxis::ALL[k], 0.0));
                }
            }
            prop_assert_eq!(rel, expected);
            Ok(())
        })
        .unwrap();
    let _ = std::fs::remove_dir_all(dir);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// REQ-VIEW-006 / R11 / SPEC 4.4: `MenuController::load` survives bounded
    /// random bytes (up to 64 KiB): no panic, a usable controller; a file
    /// whose content was dropped leaves a `.bak` sibling with the original.
    #[test]
    fn req_view_006_prop_load_random_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..65536)) {
        let dir = support::scratch("prop_bytes");
        let path = dir.join("config.toml");
        std::fs::write(&path, &bytes).unwrap();
        let (mut c, warnings) = MenuController::load(&path);
        prop_assert!(!c.is_open());
        prop_assert_eq!(c.config_path(), path.as_path());
        let lost = warnings.iter().any(|w| matches!(
            w,
            pin_app::ui::ConfigWarning::Corrupt(_) | pin_app::ui::ConfigWarning::InvalidValue { .. }
        ));
        if lost {
            let bak = dir.join("config.toml.bak");
            prop_assert!(bak.exists(), "corrupt file leaves a .bak: {warnings:?}");
            prop_assert_eq!(std::fs::read(&bak).unwrap(), bytes.clone());
        }
        // Usable: toggles and applies without panicking.
        c.toggle(1, &mut |_| {});
        c.toggle(2, &mut |_| {});
        let _ = std::fs::remove_dir_all(dir);
    }
}

/// REQ-VIEW-006 / SPEC 4.4: a corrupt file yields defaults plus a `.bak`
/// copy of the original, and an existing `.bak` is kept.
#[test]
fn req_view_006_corrupt_file_leaves_bak_and_keeps_existing_bak() {
    let dir = support::scratch("bak");
    let path = dir.join("config.toml");
    let bak = dir.join("config.toml.bak");
    std::fs::write(&path, "this = [is not valid").unwrap();
    let (c, w) = MenuController::load(&path);
    assert!(!w.is_empty());
    assert_eq!(c.config(), &pin_app::ui::AppConfig::default());
    assert_eq!(std::fs::read(&bak).unwrap(), b"this = [is not valid");
    std::fs::write(&path, "another = [bad").unwrap();
    let _ = MenuController::load(&path);
    assert_eq!(
        std::fs::read(&bak).unwrap(),
        b"this = [is not valid",
        "an existing .bak is kept"
    );
    let _ = std::fs::remove_dir_all(dir);
}
