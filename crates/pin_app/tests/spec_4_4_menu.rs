//! Subphase 4.4 spec tests (Independent Tester): menu toggle, navigation,
//! cabinet-input routing and the live input monitor.

use pin_app::ui::{
    AppConfig, CameraPreset, InputMonitor, InputRoute, MENU_TOGGLE_BUTTON, MENU_TOGGLE_SCANCODE,
    MenuInput, MenuState, Panel, ScoreDisplayKind, SettingChange, SettingRow, UiCommand,
    cabinet_menu_input, is_menu_toggle_button, is_menu_toggle_key,
};
use pin_core::CabinetInput;
use pin_input::profiles::GamepadButton;
use proptest::prelude::*;

const INPUTS: [CabinetInput; 8] = [
    CabinetInput::LeftFlipper,
    CabinetInput::RightFlipper,
    CabinetInput::LeftUpper,
    CabinetInput::RightUpper,
    CabinetInput::Action,
    CabinetInput::Start,
    CabinetInput::Launch,
    CabinetInput::Tilt,
];

fn opened() -> MenuState {
    let mut m = MenuState::default();
    m.apply(MenuInput::Toggle);
    m.drain_commands();
    m
}

#[test]
fn starts_closed_on_the_table_browser() {
    let m = MenuState::default();
    assert!(!m.is_open());
    assert!(m.forwards_to_sim());
    assert_eq!(m.panel(), Panel::Tables);
    assert_eq!(m.focus(), 0);
    assert!(m.tables().is_empty());
    assert_eq!(*m.config(), AppConfig::default());
    assert!(m.pending_commands().is_empty());
    assert_eq!(m.profile_choices().first().map(String::as_str), Some("auto"));
}

#[test]
fn esc_is_the_keyboard_toggle() {
    assert_eq!(MENU_TOGGLE_SCANCODE, 41);
    assert_eq!(
        MENU_TOGGLE_SCANCODE as i64,
        sdl3::keyboard::Scancode::Escape as i64,
        "the toggle scancode is SDL's Escape"
    );
    assert!(is_menu_toggle_key(MENU_TOGGLE_SCANCODE));
    // Esc is not bound to a cabinet input, so it cannot leak into the sim.
    assert_eq!(
        pin_app::platform::keymap::key_input(sdl3::keyboard::Scancode::Escape),
        None
    );
    let toggles: Vec<u32> = (0u32..512).filter(|&k| is_menu_toggle_key(k)).collect();
    assert_eq!(toggles, vec![41], "only Esc toggles the menu");
    assert!(!is_menu_toggle_key(u32::MAX));
}

#[test]
fn controller_menu_button_is_the_gamepad_toggle() {
    assert!(is_menu_toggle_button(MENU_TOGGLE_BUTTON));
    assert_eq!(MENU_TOGGLE_BUTTON, GamepadButton::Guide);
    // None of the buttons the default keymap gives to the cabinet toggles the menu.
    for b in [
        GamepadButton::South,
        GamepadButton::East,
        GamepadButton::West,
        GamepadButton::North,
        GamepadButton::Back,
        GamepadButton::Start,
        GamepadButton::LeftStick,
        GamepadButton::RightStick,
        GamepadButton::LeftShoulder,
        GamepadButton::RightShoulder,
        GamepadButton::DpadUp,
        GamepadButton::DpadDown,
        GamepadButton::DpadLeft,
        GamepadButton::DpadRight,
    ] {
        assert!(!is_menu_toggle_button(b), "{b:?} must not toggle the menu");
    }
}

#[test]
fn toggle_opens_and_closes_and_reports_it() {
    let mut m = MenuState::default();
    m.apply(MenuInput::Toggle);
    assert!(m.is_open());
    assert!(!m.forwards_to_sim());
    assert_eq!(m.drain_commands(), vec![UiCommand::MenuOpened]);
    m.apply(MenuInput::Toggle);
    assert!(!m.is_open());
    assert!(m.forwards_to_sim());
    assert_eq!(m.drain_commands(), vec![UiCommand::MenuClosed]);
    assert!(m.drain_commands().is_empty());

    // open()/close() are idempotent and report only real transitions.
    m.open();
    m.open();
    assert!(m.is_open());
    assert_eq!(m.drain_commands(), vec![UiCommand::MenuOpened]);
    m.close();
    m.close();
    assert!(!m.is_open());
    assert_eq!(m.drain_commands(), vec![UiCommand::MenuClosed]);
}

#[test]
fn inputs_are_ignored_while_closed() {
    let mut m = MenuState::default();
    let before = m.config().clone();
    m.apply_all([
        MenuInput::Up,
        MenuInput::Down,
        MenuInput::Left,
        MenuInput::Right,
        MenuInput::Select,
        MenuInput::Back,
        MenuInput::ShowPanel(Panel::Input),
        MenuInput::FocusTable(0),
        MenuInput::LoadTable(0),
        MenuInput::SetFilter("x".into()),
        MenuInput::Set(SettingChange::Wireframe(!before.wireframe)),
        MenuInput::Set(SettingChange::CameraPreset(CameraPreset::TopDown)),
        MenuInput::Rescan,
    ]);
    assert!(!m.is_open());
    assert_eq!(m.panel(), Panel::Tables);
    assert_eq!(m.focus(), 0);
    assert_eq!(*m.config(), before);
    assert_eq!(m.filter().to_string(), "");
    assert!(m.pending_commands().is_empty());
}

#[test]
fn closed_menu_forwards_every_cabinet_input() {
    let mut m = MenuState::default();
    for input in INPUTS {
        for pressed in [true, false] {
            assert_eq!(m.route_cabinet(input, pressed), InputRoute::Sim);
        }
    }
    assert!(!m.is_open());
    assert_eq!(m.panel(), Panel::Tables);
    assert_eq!(m.focus(), 0);
    assert!(m.pending_commands().is_empty());
}

#[test]
fn open_menu_swallows_every_cabinet_input() {
    for input in INPUTS {
        for pressed in [true, false] {
            let mut m = opened();
            assert_eq!(
                m.route_cabinet(input, pressed),
                InputRoute::Menu,
                "{input:?} pressed={pressed} leaked to the sim"
            );
        }
    }
    // Held through a toggle: the release that arrives while open is swallowed
    // too, and a release never navigates.
    let mut m = opened();
    m.apply(MenuInput::Down);
    let focus = m.focus();
    for input in INPUTS {
        assert_eq!(m.route_cabinet(input, false), InputRoute::Menu);
    }
    assert!(m.is_open());
    assert_eq!(m.focus(), focus);
    assert_eq!(m.panel(), Panel::Tables);
}

#[test]
fn cabinet_inputs_navigate_the_menu() {
    assert_eq!(cabinet_menu_input(CabinetInput::LeftFlipper), Some(MenuInput::Up));
    assert_eq!(cabinet_menu_input(CabinetInput::RightFlipper), Some(MenuInput::Down));
    assert_eq!(cabinet_menu_input(CabinetInput::LeftUpper), Some(MenuInput::Left));
    assert_eq!(cabinet_menu_input(CabinetInput::RightUpper), Some(MenuInput::Right));
    assert_eq!(cabinet_menu_input(CabinetInput::Start), Some(MenuInput::Select));
    assert_eq!(cabinet_menu_input(CabinetInput::Launch), Some(MenuInput::Select));
    assert_eq!(cabinet_menu_input(CabinetInput::Action), Some(MenuInput::Back));
    assert_eq!(cabinet_menu_input(CabinetInput::Tilt), None);

    // A press through route_cabinet has the same effect as the mapped input.
    for input in INPUTS {
        let mut via_cabinet = opened();
        let mut via_input = opened();
        for m in [&mut via_cabinet, &mut via_input] {
            m.apply_all([MenuInput::ShowPanel(Panel::Display), MenuInput::Down]);
            m.drain_commands();
        }
        via_cabinet.route_cabinet(input, true);
        if let Some(mapped) = cabinet_menu_input(input) {
            via_input.apply(mapped);
        }
        assert_eq!(via_cabinet.is_open(), via_input.is_open(), "{input:?}");
        assert_eq!(via_cabinet.panel(), via_input.panel(), "{input:?}");
        assert_eq!(via_cabinet.focus(), via_input.focus(), "{input:?}");
        assert_eq!(via_cabinet.config(), via_input.config(), "{input:?}");
        assert_eq!(
            via_cabinet.pending_commands(),
            via_input.pending_commands(),
            "{input:?}"
        );
    }
}

#[test]
fn flipper_buttons_walk_the_panels_and_action_backs_out() {
    let mut m = opened();
    // Tab bar: upper flippers switch panels with wrap.
    m.route_cabinet(CabinetInput::RightUpper, true);
    assert_eq!(m.panel(), Panel::Display);
    m.route_cabinet(CabinetInput::RightUpper, true);
    assert_eq!(m.panel(), Panel::Input);
    m.route_cabinet(CabinetInput::RightUpper, true);
    assert_eq!(m.panel(), Panel::Tables);
    m.route_cabinet(CabinetInput::LeftUpper, true);
    assert_eq!(m.panel(), Panel::Input);
    // Rows.
    m.route_cabinet(CabinetInput::RightFlipper, true);
    assert_eq!(m.focus(), 1);
    assert_eq!(m.focused_setting(), Some(SettingRow::INPUT[0]));
    m.route_cabinet(CabinetInput::LeftFlipper, true);
    assert_eq!(m.focus(), 0);
    m.route_cabinet(CabinetInput::LeftFlipper, true);
    assert_eq!(m.focus(), 0, "Up clamps at the tab bar");
    // Back on a row → tab bar; Back on the tab bar → closed.
    m.route_cabinet(CabinetInput::RightFlipper, true);
    m.route_cabinet(CabinetInput::Action, true);
    assert!(m.is_open());
    assert_eq!(m.focus(), 0);
    assert_eq!(
        m.route_cabinet(CabinetInput::Action, true),
        InputRoute::Menu,
        "the press that closes the menu is not forwarded"
    );
    assert!(!m.is_open());
    assert_eq!(m.route_cabinet(CabinetInput::Action, false), InputRoute::Sim);
}

#[test]
fn panels_cycle_and_show_panel_jumps() {
    assert_eq!(Panel::ALL, [Panel::Tables, Panel::Display, Panel::Input]);
    for p in Panel::ALL {
        assert_eq!(p.next().prev(), p);
        assert!(!p.label().is_empty());
    }
    assert_eq!(Panel::Tables.next(), Panel::Display);
    assert_eq!(Panel::Input.next(), Panel::Tables);
    let mut m = opened();
    for p in [Panel::Input, Panel::Display, Panel::Tables, Panel::Display] {
        m.apply(MenuInput::ShowPanel(p));
        assert_eq!(m.panel(), p);
    }
}

#[test]
fn down_clamps_at_the_last_row() {
    for (panel, rows) in [
        (Panel::Display, SettingRow::DISPLAY.len()),
        (Panel::Input, SettingRow::INPUT.len()),
    ] {
        let mut m = opened();
        m.apply(MenuInput::ShowPanel(panel));
        let mut seen = Vec::new();
        for _ in 0..rows + 5 {
            m.apply(MenuInput::Down);
            seen.push(m.focused_setting());
        }
        let last = if panel == Panel::Display {
            *SettingRow::DISPLAY.last().unwrap()
        } else {
            *SettingRow::INPUT.last().unwrap()
        };
        assert_eq!(m.focused_setting(), Some(last));
        assert_eq!(m.focus(), rows);
        let expect: Vec<_> = (0..rows + 5)
            .map(|i| {
                let all = if panel == Panel::Display {
                    &SettingRow::DISPLAY[..]
                } else {
                    &SettingRow::INPUT[..]
                };
                Some(all[i.min(rows - 1)])
            })
            .collect();
        assert_eq!(seen, expect);
    }
}

#[test]
fn settings_change_the_config_and_emit_one_coalesced_command() {
    let mut m = opened();
    let d = AppConfig::default();
    assert_ne!(d.camera_preset, CameraPreset::TopDown);
    m.apply(MenuInput::Set(SettingChange::CameraPreset(CameraPreset::TopDown)));
    assert_eq!(m.config().camera_preset, CameraPreset::TopDown);
    let other = if d.score_display == ScoreDisplayKind::Dmd {
        ScoreDisplayKind::Lcd
    } else {
        ScoreDisplayKind::Dmd
    };
    m.apply(MenuInput::Set(SettingChange::ScoreDisplay(other)));
    m.apply(MenuInput::Set(SettingChange::Wireframe(!d.wireframe)));
    let cmds = m.drain_commands();
    assert_eq!(cmds.len(), 1, "coalesced: {cmds:?}");
    match &cmds[0] {
        UiCommand::ConfigChanged(cfg) => {
            assert_eq!(**cfg, *m.config());
            assert_eq!(cfg.camera_preset, CameraPreset::TopDown);
            assert_eq!(cfg.score_display, other);
            assert_eq!(cfg.wireframe, !d.wireframe);
        }
        c => panic!("unexpected {c:?}"),
    }
    // Setting the same value again is not a change.
    m.apply(MenuInput::Set(SettingChange::CameraPreset(CameraPreset::TopDown)));
    assert!(m.pending_commands().is_empty());
}

#[test]
fn wireframe_row_toggles_with_select() {
    let mut m = opened();
    let before = m.config().wireframe;
    m.apply(MenuInput::ShowPanel(Panel::Input));
    for _ in 0..SettingRow::INPUT.len() {
        m.apply(MenuInput::Down);
    }
    assert_eq!(m.focused_setting(), Some(SettingRow::Wireframe));
    m.route_cabinet(CabinetInput::Start, true);
    assert_eq!(m.config().wireframe, !before);
    m.apply(MenuInput::Select);
    assert_eq!(m.config().wireframe, before);
}

#[test]
fn numeric_rows_step_and_clamp_inside_their_range() {
    for (panel, rows) in [
        (Panel::Display, &SettingRow::DISPLAY[..]),
        (Panel::Input, &SettingRow::INPUT[..]),
    ] {
        for (i, row) in rows.iter().enumerate() {
            let Some(range) = row.range() else { continue };
            let (min, max, step) = (range.min as f64, range.max as f64, range.step as f64);
            assert!(min < max && step > 0.0, "{row:?}: bad range");
            let mut m = opened();
            m.apply(MenuInput::ShowPanel(panel));
            for _ in 0..=i {
                m.apply(MenuInput::Down);
            }
            assert_eq!(m.focused_setting(), Some(*row));
            let value = |m: &MenuState| row.number(m.config()).expect("numeric row") as f64;
            let start = value(&m);
            assert!((min..=max).contains(&start), "{row:?} default out of range");
            assert!(!row.value_text(m.config()).is_empty());
            // Select does nothing on a numeric row.
            m.apply(MenuInput::Select);
            assert_eq!(value(&m), start, "{row:?}");
            let eps = step * 1e-3 + 1e-6;
            let steps = ((max - min) / step).ceil() as usize + 3;
            let mut prev = start;
            for _ in 0..steps.min(100_000) {
                m.apply(MenuInput::Right);
                let v = value(&m);
                assert!(v >= prev - eps && v <= max + eps, "{row:?}: {prev} → {v}");
                assert!(v - prev <= step + eps, "{row:?}: step too large {prev} → {v}");
                prev = v;
            }
            assert!((prev - max).abs() <= eps, "{row:?} does not reach max: {prev}");
            for _ in 0..steps.min(100_000) {
                m.apply(MenuInput::Left);
                let v = value(&m);
                assert!(v <= prev + eps && v >= min - eps, "{row:?}: {prev} → {v}");
                prev = v;
            }
            assert!((prev - min).abs() <= eps, "{row:?} does not reach min: {prev}");
        }
    }
}

#[test]
fn hostile_setting_values_are_clamped() {
    for bad in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1e30, 1e30] {
        let mut m = opened();
        m.apply_all([
            MenuInput::Set(SettingChange::PlungerDeadzone(bad)),
            MenuInput::Set(SettingChange::NudgeGain(bad)),
            MenuInput::Set(SettingChange::BackglassPipWidth(bad)),
        ]);
        for row in [
            SettingRow::PlungerDeadzone,
            SettingRow::NudgeGain,
            SettingRow::BackglassPipWidth,
        ] {
            let r = row.range().unwrap();
            let v = row.number(m.config()).unwrap();
            assert!(v.is_finite(), "{row:?} = {v} after {bad}");
            assert!(
                (r.min..=r.max).contains(&v),
                "{row:?} = {v} outside {}..{} after {bad}",
                r.min,
                r.max
            );
        }
    }
}

#[test]
fn unknown_profile_is_refused_with_a_notice() {
    let mut m = opened();
    let before = m.config().clone();
    m.apply(MenuInput::Set(SettingChange::InputProfile(
        "no_such_profile_id".into(),
    )));
    assert_eq!(*m.config(), before);
    assert!(m.notice().is_some());
    assert!(m.pending_commands().is_empty());

    for id in m.profile_choices().to_vec() {
        m.apply(MenuInput::Set(SettingChange::InputProfile(id.clone())));
        assert_eq!(m.config().input_profile, id);
    }
    for p in pin_input::profiles::builtin_profiles() {
        assert!(
            m.profile_choices().iter().any(|c| *c == p.id),
            "builtin profile {} is not offered",
            p.id
        );
    }
}

#[test]
fn filter_narrows_the_visible_tables() {
    use pin_app::ui::{TableEntry, TableStatus};
    let entry = |name: &str| TableEntry {
        path: format!("/t/{name}").into(),
        file_name: name.to_owned(),
        status: TableStatus::Broken("x".into()),
    };
    let mut m = opened();
    m.set_tables(vec![entry("alpha"), entry("beta"), entry("alphabet")]);
    assert_eq!(m.visible_tables().len(), 3);
    m.apply(MenuInput::SetFilter("alpha".into()));
    assert_eq!(m.filter().to_string(), "alpha");
    let visible: Vec<&str> = m
        .visible_tables()
        .iter()
        .map(|&i| m.tables()[i].file_name.as_str())
        .collect();
    assert_eq!(visible.len(), 2, "{visible:?}");
    assert!(visible.iter().all(|n| n.contains("alpha")));
    m.apply(MenuInput::SetFilter("zzz-no-match".into()));
    assert!(m.visible_tables().is_empty());
    m.apply_all([MenuInput::Down, MenuInput::Select]);
    assert_eq!(m.focused_table(), None);
    m.apply(MenuInput::SetFilter(String::new()));
    assert_eq!(m.visible_tables().len(), 3);
}

#[test]
fn rescan_is_a_command_for_the_io_worker() {
    let mut m = opened();
    m.apply(MenuInput::Rescan);
    assert_eq!(m.drain_commands(), vec![UiCommand::Rescan]);
}

#[test]
fn input_monitor_ors_the_sources_per_input() {
    let mut mon = InputMonitor::new();
    for input in INPUTS {
        assert!(!mon.is_pressed(input));
    }
    assert_eq!(mon.pressed_mask(), 0);
    mon.set_button(0, CabinetInput::LeftFlipper, true);
    mon.set_button(1, CabinetInput::LeftFlipper, true);
    // Last writer releases; the other source still holds it (3.2 review N5).
    mon.set_button(1, CabinetInput::LeftFlipper, false);
    assert!(mon.is_pressed(CabinetInput::LeftFlipper));
    assert!(!mon.is_pressed(CabinetInput::RightFlipper));
    assert_ne!(mon.pressed_mask(), 0);
    mon.set_button(0, CabinetInput::LeftFlipper, false);
    assert!(!mon.is_pressed(CabinetInput::LeftFlipper));
    assert_eq!(mon.pressed_mask(), 0);

    mon.set_button(3, CabinetInput::Start, true);
    mon.set_button(4, CabinetInput::Start, true);
    mon.set_button(3, CabinetInput::Tilt, true);
    mon.clear_source(3);
    assert!(mon.is_pressed(CabinetInput::Start));
    assert!(!mon.is_pressed(CabinetInput::Tilt));
    mon.clear_source(4);
    assert_eq!(mon.pressed_mask(), 0);
    // Distinct inputs have distinct mask bits.
    let mut masks = Vec::new();
    for input in INPUTS {
        let mut one = InputMonitor::new();
        one.set_button(u8::MAX, input, true);
        assert_eq!(one.pressed_mask().count_ones(), 1);
        masks.push(one.pressed_mask());
    }
    masks.sort_unstable();
    masks.dedup();
    assert_eq!(masks.len(), 8);
}

fn any_input() -> impl Strategy<Value = MenuInput> {
    prop_oneof![
        3 => Just(MenuInput::Toggle),
        1 => Just(MenuInput::Up),
        1 => Just(MenuInput::Down),
        1 => Just(MenuInput::Left),
        1 => Just(MenuInput::Right),
        1 => Just(MenuInput::Select),
        1 => Just(MenuInput::Back),
        1 => Just(MenuInput::Rescan),
        1 => (0usize..3).prop_map(|i| MenuInput::ShowPanel(Panel::ALL[i])),
        1 => (0usize..6).prop_map(MenuInput::FocusTable),
        1 => (0usize..6).prop_map(MenuInput::LoadTable),
        1 => "[a-z]{0,3}".prop_map(MenuInput::SetFilter),
        1 => any::<f32>().prop_map(|v| MenuInput::Set(SettingChange::NudgeGain(v))),
        1 => any::<f32>().prop_map(|v| MenuInput::Set(SettingChange::ScorePipWidth(v))),
        1 => any::<u32>().prop_map(|v| MenuInput::Set(SettingChange::PlungerReleaseWindow(v))),
        1 => any::<Option<u32>>().prop_map(|v| MenuInput::Set(SettingChange::BackglassDisplay(v))),
        1 => any::<bool>().prop_map(|v| MenuInput::Set(SettingChange::Wireframe(v))),
        1 => "[a-z_]{0,6}".prop_map(|v| MenuInput::Set(SettingChange::InputProfile(v))),
    ]
}

#[derive(Clone, Debug)]
enum Step {
    Input(MenuInput),
    Cabinet(usize, bool),
    Drain,
    Report(bool),
}

fn step() -> impl Strategy<Value = Step> {
    prop_oneof![
        4 => any_input().prop_map(Step::Input),
        4 => (0usize..8, any::<bool>()).prop_map(|(i, p)| Step::Cabinet(i, p)),
        1 => Just(Step::Drain),
        1 => any::<bool>().prop_map(Step::Report),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, failure_persistence: None, ..ProptestConfig::default() })]

    /// Under any event sequence: cabinet inputs reach the sim exactly when
    /// the menu is closed, a closed menu is inert, and the state stays valid.
    #[test]
    fn routing_and_state_invariants(steps in proptest::collection::vec(step(), 0..80)) {
        use pin_app::ui::{TableEntry, TableStatus};
        let mut m = MenuState::default();
        m.set_tables((0..4).map(|i| TableEntry {
            path: format!("/t/{i}").into(),
            file_name: format!("t{i}"),
            status: if i == 2 { TableStatus::Broken("bad".into()) } else {
                TableStatus::Broken("also bad".into())
            },
        }).collect());
        for s in steps {
            let was_open = m.is_open();
            match s {
                Step::Input(input) => {
                    let toggle = input == MenuInput::Toggle;
                    let before = (m.panel(), m.focus(), m.config().clone(), m.filter().to_string());
                    let pending = m.pending_commands().len();
                    m.apply(input);
                    if toggle {
                        prop_assert_eq!(m.is_open(), !was_open);
                    } else if !was_open {
                        prop_assert!(!m.is_open());
                        prop_assert_eq!(
                            (m.panel(), m.focus(), m.config().clone(), m.filter().to_string()),
                            before
                        );
                        prop_assert_eq!(m.pending_commands().len(), pending);
                    }
                }
                Step::Cabinet(i, pressed) => {
                    let pending = m.pending_commands().len();
                    let route = m.route_cabinet(INPUTS[i], pressed);
                    if was_open {
                        prop_assert_eq!(route, InputRoute::Menu);
                    } else {
                        prop_assert_eq!(route, InputRoute::Sim);
                        prop_assert!(!m.is_open());
                        prop_assert_eq!(m.pending_commands().len(), pending);
                    }
                }
                Step::Drain => { m.drain_commands(); }
                Step::Report(ok) => {
                    m.report_load_result(if ok { Ok(()) } else { Err("e".into()) });
                }
            }
            prop_assert_eq!(m.forwards_to_sim(), !m.is_open());
            prop_assert!(m.focus() <= m.row_count());
            for row in SettingRow::DISPLAY.iter().chain(SettingRow::INPUT.iter()) {
                if let (Some(r), Some(v)) = (row.range(), row.number(m.config())) {
                    prop_assert!(v.is_finite() && v >= r.min && v <= r.max, "{:?} = {}", row, v);
                }
            }
            for &i in m.visible_tables() {
                prop_assert!(i < m.tables().len());
            }
            if let Some(i) = m.focused_table() {
                prop_assert!(m.visible_tables().contains(&i));
            }
            let changed = m.pending_commands().iter()
                .filter(|c| matches!(c, UiCommand::ConfigChanged(_))).count();
            prop_assert!(changed <= 1, "ConfigChanged not coalesced");
        }
    }
}
