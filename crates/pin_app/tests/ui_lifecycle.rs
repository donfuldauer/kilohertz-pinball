//! Headless menu state-machine tests (4.4 verification: "table switch
//! lifecycle"; navigation, settings edits, command stream, cabinet routing).

use pin_app::ui::{
    AppConfig, CameraPreset, DisplayMode, InputRoute, LoadRefusal, MENU_TOGGLE_BUTTON,
    MENU_TOGGLE_SCANCODE, MenuInput, MenuState, Panel, PipCorner, ScoreDisplayKind, SettingChange,
    SettingRow, StagingMode, TableEntry, TableInfo, TableLoadState, TableStatus, UiCommand,
    cabinet_menu_input, is_menu_toggle_button, is_menu_toggle_key,
};
use pin_core::input::CabinetInput;
use pin_input::GamepadButton;
use std::path::{Path, PathBuf};

fn ready(name: &str) -> TableEntry {
    TableEntry {
        path: PathBuf::from(format!("/tables/{name}")),
        file_name: name.to_owned(),
        status: TableStatus::Ready(TableInfo {
            id: name.to_owned(),
            name: name.to_uppercase(),
            author: "Author".into(),
            version: "1".into(),
            players_max: 1,
            features: Vec::new(),
            preview: None,
            preview_error: None,
            warning_count: 0,
        }),
    }
}

fn broken(name: &str) -> TableEntry {
    TableEntry {
        path: PathBuf::from(format!("/tables/{name}")),
        file_name: name.to_owned(),
        status: TableStatus::Broken("table.toml: bad".into()),
    }
}

fn menu() -> MenuState {
    let mut m = MenuState::default();
    m.set_tables(vec![ready("alpha"), broken("beta"), ready("gamma")]);
    m.apply(MenuInput::Toggle);
    assert_eq!(m.drain_commands(), [UiCommand::MenuOpened]);
    m
}

fn p(name: &str) -> PathBuf {
    PathBuf::from(format!("/tables/{name}"))
}

#[test]
fn lifecycle_success() {
    let mut m = menu();
    assert_eq!(m.load_state(), &TableLoadState::Idle);
    m.apply_all([MenuInput::Down, MenuInput::Select]);
    assert_eq!(m.load_state(), &TableLoadState::LoadRequested(p("alpha")));
    assert!(m.load_state().is_busy());
    assert_eq!(m.pending_commands(), &[UiCommand::LoadTable(p("alpha"))]);
    let cmds = m.drain_commands();
    assert_eq!(cmds, [UiCommand::LoadTable(p("alpha"))]);
    assert_eq!(m.load_state(), &TableLoadState::Loading(p("alpha")));
    assert!(m.report_load_result(Ok(())));
    assert_eq!(m.load_state(), &TableLoadState::Loaded(p("alpha")));
    assert!(!m.is_open());
    assert_eq!(m.config().last_table.as_deref(), Some(p("alpha").as_path()));
    let cmds = m.drain_commands();
    assert!(matches!(&cmds[0], UiCommand::ConfigChanged(c) if c.last_table == Some(p("alpha"))));
    assert_eq!(cmds[1], UiCommand::MenuClosed);
    // Nothing pending any more.
    assert!(!m.report_load_result(Ok(())));
    m.acknowledge_load();
    assert_eq!(m.load_state(), &TableLoadState::Idle);
}

#[test]
fn lifecycle_failure_and_ack() {
    let mut m = menu();
    m.apply(MenuInput::LoadTable(2));
    m.drain_commands();
    assert!(m.report_load_result(Err("boom".into())));
    assert_eq!(
        m.load_state(),
        &TableLoadState::Failed {
            path: p("gamma"),
            error: "boom".into()
        }
    );
    assert!(m.is_open());
    assert!(m.notice().is_some_and(|n| n.contains("boom")));
    assert!(m.config().last_table.is_none());
    assert!(m.drain_commands().is_empty());
    // Any input while open acknowledges.
    m.apply(MenuInput::Up);
    assert_eq!(m.load_state(), &TableLoadState::Idle);
}

#[test]
fn second_request_is_refused_while_busy() {
    let mut m = menu();
    m.request_load_index(0).unwrap();
    assert_eq!(m.request_load_index(2), Err(LoadRefusal::Busy(p("alpha"))));
    m.drain_commands();
    assert_eq!(
        m.request_load(Path::new("/x")),
        Err(LoadRefusal::Busy(p("alpha")))
    );
    assert!(m.notice().is_some_and(|n| n.contains("busy")));
    assert_eq!(m.load_state(), &TableLoadState::Loading(p("alpha")));
    // After the outcome a new request is accepted.
    m.report_load_result(Err("x".into()));
    m.request_load_index(2).unwrap();
    assert_eq!(m.load_state(), &TableLoadState::LoadRequested(p("gamma")));
}

#[test]
fn broken_and_missing_tables_are_refused() {
    let mut m = menu();
    assert_eq!(
        m.request_load_index(1),
        Err(LoadRefusal::Broken("table.toml: bad".into()))
    );
    assert_eq!(m.request_load_index(9), Err(LoadRefusal::NoSuchTable(9)));
    assert_eq!(m.load_state(), &TableLoadState::Idle);
    assert!(m.pending_commands().is_empty());
    for r in [
        LoadRefusal::Busy(p("a")),
        LoadRefusal::Broken("e".into()),
        LoadRefusal::NoSuchTable(3),
    ] {
        assert!(!r.to_string().is_empty());
    }
}

#[test]
fn report_before_drain_drops_the_command() {
    let mut m = menu();
    m.request_load_index(0).unwrap();
    assert!(m.report_load_result(Err("early".into())));
    assert!(m.pending_commands().is_empty());
}

#[test]
fn open_close_and_ack_on_open() {
    let mut m = MenuState::default();
    assert!(!m.is_open());
    // Ignored while closed.
    m.apply(MenuInput::Down);
    m.apply(MenuInput::ShowPanel(Panel::Input));
    assert_eq!(m.panel(), Panel::Tables);
    assert!(m.pending_commands().is_empty());
    m.request_load(Path::new("/t")).unwrap();
    m.drain_commands();
    m.report_load_result(Ok(()));
    assert!(matches!(m.load_state(), TableLoadState::Loaded(_)));
    m.open();
    m.open();
    assert_eq!(m.load_state(), &TableLoadState::Idle);
    m.close();
    m.close();
    let cmds = m.drain_commands();
    let opened = cmds.iter().filter(|c| **c == UiCommand::MenuOpened).count();
    let closed = cmds.iter().filter(|c| **c == UiCommand::MenuClosed).count();
    assert_eq!((opened, closed), (1, 1));
}

#[test]
fn tab_bar_navigation_wraps_and_back_closes() {
    let mut m = menu();
    assert_eq!(m.focus(), 0);
    m.apply(MenuInput::Right);
    assert_eq!(m.panel(), Panel::Display);
    m.apply(MenuInput::Select);
    assert_eq!(m.panel(), Panel::Input);
    m.apply(MenuInput::Right);
    assert_eq!(m.panel(), Panel::Tables);
    m.apply(MenuInput::Left);
    assert_eq!(m.panel(), Panel::Input);
    assert_eq!(m.row_count(), 1 + SettingRow::INPUT.len());
    // Up/Down clamp.
    m.apply(MenuInput::Up);
    assert_eq!(m.focus(), 0);
    for _ in 0..100 {
        m.apply(MenuInput::Down);
    }
    assert_eq!(m.focus(), SettingRow::INPUT.len());
    assert_eq!(
        m.focused_setting(),
        Some(SettingRow::INPUT[SettingRow::INPUT.len() - 1])
    );
    m.apply(MenuInput::FocusRow(999));
    assert_eq!(m.focus(), SettingRow::INPUT.len());
    m.apply(MenuInput::Back);
    assert_eq!(m.focus(), 0);
    m.apply(MenuInput::Back);
    assert!(!m.is_open());
    for p in Panel::ALL {
        assert_eq!(p.next().prev(), p);
        assert!(!p.label().is_empty());
    }
}

#[test]
fn settings_steps_clamp_cycle_and_coalesce() {
    let mut m = menu();
    m.apply(MenuInput::ShowPanel(Panel::Display));
    m.set_display_count(2);
    // Row 1 = DisplayMode (choice, cycles).
    m.apply(MenuInput::Down);
    assert_eq!(m.focused_setting(), Some(SettingRow::DisplayMode));
    m.apply(MenuInput::Right);
    assert_eq!(m.config().display_mode, DisplayMode::Multi);
    m.apply(MenuInput::Select);
    assert_eq!(m.config().display_mode, DisplayMode::Single);
    m.apply(MenuInput::Left);
    assert_eq!(m.config().display_mode, DisplayMode::Multi);
    // One coalesced ConfigChanged carrying the latest settings.
    let pending: Vec<_> = m
        .pending_commands()
        .iter()
        .filter(|c| matches!(c, UiCommand::ConfigChanged(_)))
        .collect();
    assert_eq!(pending.len(), 1);
    assert!(
        matches!(pending[0], UiCommand::ConfigChanged(c) if c.display_mode == DisplayMode::Multi)
    );
    m.drain_commands();
    // Backglass monitor cycles PiP → 1 → 2 → PiP.
    m.apply_all([MenuInput::Down, MenuInput::Down]);
    assert_eq!(m.focused_setting(), Some(SettingRow::BackglassDisplay));
    m.apply(MenuInput::Right);
    assert_eq!(m.config().backglass_display, Some(0));
    m.apply(MenuInput::Right);
    assert_eq!(m.config().backglass_display, Some(1));
    m.apply(MenuInput::Right);
    assert_eq!(m.config().backglass_display, None);
    // PiP width: numeric, snaps to the 0.05 grid and clamps at 1.0.
    m.apply(MenuInput::FocusRow(6));
    assert_eq!(m.focused_setting(), Some(SettingRow::BackglassPipWidth));
    for _ in 0..40 {
        m.apply(MenuInput::Right);
    }
    assert_eq!(m.config().backglass_pip.width_fraction, 1.0);
    // Select on a numeric row is a no-op.
    m.drain_commands();
    m.apply(MenuInput::Select);
    assert!(m.pending_commands().is_empty());
    // Unchanged value: no command.
    m.apply(MenuInput::Set(SettingChange::BackglassPipWidth(1.0)));
    assert!(m.pending_commands().is_empty());
    // Direct edits clamp; non-finite → default.
    m.apply(MenuInput::Set(SettingChange::ScorePipWidth(-3.0)));
    assert_eq!(m.config().score_pip.width_fraction, 0.05);
    m.apply(MenuInput::Set(SettingChange::ScorePipWidth(f32::NAN)));
    assert_eq!(m.config().score_pip.width_fraction, 0.25);
    m.apply_all([
        MenuInput::Set(SettingChange::CameraPreset(CameraPreset::TopDown)),
        MenuInput::Set(SettingChange::ScoreDisplay(ScoreDisplayKind::Lcd)),
        MenuInput::Set(SettingChange::BackglassPipCorner(PipCorner::BottomLeft)),
        MenuInput::Set(SettingChange::ScorePipCorner(PipCorner::TopLeft)),
        MenuInput::Set(SettingChange::PlayfieldDisplay(99)),
        MenuInput::Set(SettingChange::ScoreDisplayMonitor(Some(99))),
        MenuInput::Set(SettingChange::BackglassDisplay(Some(3))),
    ]);
    let c = m.config();
    assert_eq!(c.camera_preset, CameraPreset::TopDown);
    assert_eq!(c.score_display, ScoreDisplayKind::Lcd);
    assert_eq!(c.backglass_pip.corner, PipCorner::BottomLeft);
    assert_eq!(c.score_pip.corner, PipCorner::TopLeft);
    assert_eq!(c.playfield_display, 15);
    assert_eq!(c.score_display_monitor, None);
    assert_eq!(c.backglass_display, Some(3));
    // Every display row renders a value and choice rows cycle with Select.
    for (k, row) in SettingRow::DISPLAY.iter().enumerate() {
        assert!(!row.label().is_empty());
        assert!(!row.value_text(m.config()).is_empty());
        m.apply(MenuInput::FocusRow(k + 1));
        assert_eq!(m.focused_setting(), Some(*row));
        m.apply_all([MenuInput::Select, MenuInput::Left, MenuInput::Right]);
    }
}

#[test]
fn input_panel_rows() {
    let mut m = menu();
    m.apply(MenuInput::ShowPanel(Panel::Input));
    m.set_profile_choices(vec![
        "xarcade".into(),
        "auto".into(),
        "pad".into(),
        "pad".into(),
    ]);
    assert_eq!(m.profile_choices(), ["auto", "xarcade", "pad"]);
    m.apply(MenuInput::Down);
    assert_eq!(m.focused_setting(), Some(SettingRow::InputProfile));
    m.apply(MenuInput::Right);
    assert_eq!(m.config().input_profile, "xarcade");
    m.apply(MenuInput::Left);
    m.apply(MenuInput::Left);
    assert_eq!(m.config().input_profile, "pad");
    m.apply(MenuInput::Set(SettingChange::InputProfile("nope".into())));
    assert_eq!(m.config().input_profile, "pad");
    assert!(m.notice().is_some_and(|n| n.contains("nope")));
    m.apply(MenuInput::Down);
    m.apply(MenuInput::Select);
    assert_eq!(m.config().staging_mode, StagingMode::Dedicated);
    // Dual-stage thresholds stay ordered.
    m.apply(MenuInput::Set(SettingChange::DualStageUpper(0.3)));
    m.apply(MenuInput::Set(SettingChange::DualStageLower(0.9)));
    let cal = &m.config().calibration;
    assert!(cal.dual_stage_lower <= cal.dual_stage_upper);
    m.apply_all([
        MenuInput::Set(SettingChange::PlungerDeadzone(9.0)),
        MenuInput::Set(SettingChange::PlungerRestTracking(5.0)),
        MenuInput::Set(SettingChange::PlungerReleaseMinSpeed(3.0)),
        MenuInput::Set(SettingChange::PlungerReleaseWindow(0)),
        MenuInput::Set(SettingChange::NudgeHighPass(2.0)),
        MenuInput::Set(SettingChange::NudgeGain(f32::INFINITY)),
        MenuInput::Set(SettingChange::NudgeCurve(0.0)),
        MenuInput::Set(SettingChange::NudgeDeadzone(0.1)),
        MenuInput::Set(SettingChange::NudgeMax(3.0)),
    ]);
    let cal = &m.config().calibration;
    assert_eq!(cal.plunger.deadzone, 0.5);
    assert_eq!(cal.plunger.rest_tracking_per_s, 5.0);
    assert_eq!(cal.plunger.release_min_speed, 3.0);
    assert_eq!(cal.plunger.release_window_ms, 1);
    assert_eq!(cal.nudge.high_pass_hz, 2.0);
    assert_eq!(cal.nudge.gain, AppConfig::default().calibration.nudge.gain);
    assert!((cal.nudge.curve_exponent - 0.1).abs() < 1e-6);
    assert_eq!(cal.nudge.deadzone, 0.1);
    assert_eq!(cal.nudge.max, 3.0);
    // The release window steps on a grid anchored at 0: 100 ms → 110 → 100.
    m.apply(MenuInput::Set(SettingChange::PlungerReleaseWindow(100)));
    let row = 1 + SettingRow::INPUT
        .iter()
        .position(|r| *r == SettingRow::PlungerReleaseWindow)
        .unwrap();
    m.apply(MenuInput::FocusRow(row));
    m.apply(MenuInput::Right);
    assert_eq!(m.config().calibration.plunger.release_window_ms, 110);
    m.apply(MenuInput::Left);
    assert_eq!(m.config().calibration.plunger.release_window_ms, 100);
    for _ in 0..20 {
        m.apply(MenuInput::Left);
    }
    assert_eq!(m.config().calibration.plunger.release_window_ms, 1);
    m.apply(MenuInput::Right);
    assert_eq!(m.config().calibration.plunger.release_window_ms, 10);
    // Wireframe toggles with Select.
    let last = SettingRow::INPUT.len();
    m.apply(MenuInput::FocusRow(last));
    assert_eq!(m.focused_setting(), Some(SettingRow::Wireframe));
    m.apply(MenuInput::Select);
    assert!(m.config().wireframe);
    m.apply(MenuInput::Select);
    assert!(!m.config().wireframe);
    // Every numeric row: Right then Left returns to a grid value in range.
    for (k, row) in SettingRow::INPUT.iter().enumerate() {
        m.apply(MenuInput::FocusRow(k + 1));
        m.apply_all([MenuInput::Right, MenuInput::Left, MenuInput::Left]);
        if let (Some(r), Some(v)) = (row.range(), row.number(m.config())) {
            assert!(v >= r.min && v <= r.max, "{row:?} {v}");
            assert!(SettingChange::number(*row, v).is_some());
        } else {
            assert!(SettingChange::number(*row, 1.0).is_none());
        }
        assert!(!row.value_text(m.config()).is_empty());
    }
}

#[test]
fn audio_budget_row() {
    let mut m = menu();
    m.apply(MenuInput::ShowPanel(Panel::Display));
    let row = SettingRow::DISPLAY.len();
    m.apply(MenuInput::FocusRow(row));
    assert_eq!(m.focused_setting(), Some(SettingRow::DecodedAudioBudget));
    assert_eq!(m.config().audio.decoded_budget_mib, 512);
    m.apply(MenuInput::Right);
    assert_eq!(m.config().audio.decoded_budget_mib, 576);
    m.apply(MenuInput::Set(SettingChange::DecodedAudioBudget(1)));
    assert_eq!(m.config().audio.decoded_budget_mib, 64);
    m.apply(MenuInput::Set(SettingChange::DecodedAudioBudget(u32::MAX)));
    assert_eq!(m.config().audio.decoded_budget_mib, 8192);
    assert_eq!(m.config().audio.decoded_budget_bytes(), 8192 << 20);
    assert_eq!(
        SettingRow::DecodedAudioBudget.value_text(m.config()),
        "8192 MiB"
    );
    assert!(matches!(
        m.pending_commands().last(),
        Some(UiCommand::ConfigChanged(c)) if c.audio.decoded_budget_mib == 8192
    ));
}

#[test]
fn cabinet_inputs_drive_the_menu_and_never_reach_the_sim() {
    let mut m = MenuState::default();
    m.set_tables(vec![ready("alpha")]);
    for i in CabinetInput::ALL {
        assert_eq!(m.route_cabinet(i, true), InputRoute::Sim);
        assert_eq!(m.route_cabinet(i, false), InputRoute::Sim);
    }
    assert!(m.forwards_to_sim());
    assert!(!m.is_open());
    m.apply(MenuInput::Toggle);
    assert!(!m.forwards_to_sim());
    for i in CabinetInput::ALL {
        assert_eq!(m.route_cabinet(i, false), InputRoute::Menu);
    }
    // Right flipper = Down, Start = Select (loads the focused table).
    assert_eq!(
        m.route_cabinet(CabinetInput::RightFlipper, true),
        InputRoute::Menu
    );
    assert_eq!(m.focus(), 1);
    assert_eq!(m.route_cabinet(CabinetInput::Start, true), InputRoute::Menu);
    assert_eq!(m.load_state(), &TableLoadState::LoadRequested(p("alpha")));
    // Tilt does nothing; Action = Back.
    assert_eq!(m.route_cabinet(CabinetInput::Tilt, true), InputRoute::Menu);
    assert_eq!(
        m.route_cabinet(CabinetInput::Action, true),
        InputRoute::Menu
    );
    assert_eq!(m.focus(), 0);
    assert_eq!(
        cabinet_menu_input(CabinetInput::LeftFlipper),
        Some(MenuInput::Up)
    );
    assert_eq!(
        cabinet_menu_input(CabinetInput::LeftUpper),
        Some(MenuInput::Left)
    );
    assert_eq!(
        cabinet_menu_input(CabinetInput::RightUpper),
        Some(MenuInput::Right)
    );
    assert_eq!(
        cabinet_menu_input(CabinetInput::Launch),
        Some(MenuInput::Select)
    );
    assert_eq!(cabinet_menu_input(CabinetInput::Tilt), None);
    assert!(is_menu_toggle_key(MENU_TOGGLE_SCANCODE));
    assert!(!is_menu_toggle_key(40));
    assert!(is_menu_toggle_button(MENU_TOGGLE_BUTTON));
    assert!(!is_menu_toggle_button(GamepadButton::Start));
}

#[test]
fn rescan_and_set_config() {
    let mut m = menu();
    m.apply(MenuInput::Rescan);
    assert_eq!(m.drain_commands(), [UiCommand::Rescan]);
    let mut c = AppConfig::default();
    c.calibration.nudge.gain = -5.0;
    c.wireframe = true;
    m.set_config(c);
    assert!(m.config().wireframe);
    assert_eq!(
        m.config().calibration.nudge.gain,
        AppConfig::default().calibration.nudge.gain
    );
    assert!(m.pending_commands().is_empty());
    m.set_display_count(0);
    assert_eq!(m.display_count(), 1);
    m.set_display_count(1000);
    assert_eq!(m.display_count(), 16);
}

#[test]
fn live_monitor_ors_sources() {
    let mut m = MenuState::default();
    let mon = m.monitor_mut();
    mon.set_button(0, CabinetInput::LeftFlipper, true);
    mon.set_button(1, CabinetInput::LeftFlipper, true);
    mon.set_button(0, CabinetInput::LeftFlipper, false);
    assert!(mon.is_pressed(CabinetInput::LeftFlipper));
    assert_eq!(mon.pressed_mask(), 1 << CabinetInput::LeftFlipper.index());
    mon.set_button(200, CabinetInput::Start, true);
    mon.clear_source(1);
    assert!(!mon.is_pressed(CabinetInput::LeftFlipper));
    assert!(mon.is_pressed(CabinetInput::Start));
    mon.clear_source(255);
    assert_eq!(mon.pressed_mask(), 0);
    mon.set_axis(pin_core::input::CabinetAxis::Plunger, 0.4);
    assert_eq!(mon.axis(pin_core::input::CabinetAxis::Plunger), 0.4);
    assert_eq!(mon.plunger().raw, 0.4);
    mon.set_plunger(
        pin_input::PlungerOutput {
            position: 0.3,
            release: Some(12.0),
        },
        0.05,
    );
    mon.set_plunger(
        pin_input::PlungerOutput {
            position: 0.0,
            release: None,
        },
        0.05,
    );
    assert_eq!(m.monitor().plunger().last_release, Some(12.0));
    assert_eq!(m.monitor().plunger().rest, 0.05);
    m.monitor_mut().set_nudge([0.1, -0.2]);
    assert_eq!(m.monitor().nudge(), [0.1, -0.2]);
}

#[test]
fn nan_in_preserved_keys_does_not_fake_changes() {
    let l = AppConfig::from_toml_str("future = nan\n");
    let mut m = MenuState::new(l.config);
    m.apply(MenuInput::Toggle);
    m.drain_commands();
    m.apply(MenuInput::Set(SettingChange::Wireframe(false)));
    assert!(m.pending_commands().is_empty());
    m.apply(MenuInput::Set(SettingChange::Wireframe(true)));
    assert_eq!(m.pending_commands().len(), 1);
    assert!(
        matches!(&m.pending_commands()[0], UiCommand::ConfigChanged(c) if c.extra.contains_key("future"))
    );
}

#[test]
fn opening_releases_what_the_sim_holds() {
    use pin_core::input::CabinetAxis;
    let mut m = MenuState::default();
    // Held into the menu: left flipper, start, plunger pulled, nudge.
    assert_eq!(
        m.route_cabinet(CabinetInput::LeftFlipper, true),
        InputRoute::Sim
    );
    assert_eq!(m.route_cabinet(CabinetInput::Start, true), InputRoute::Sim);
    assert_eq!(
        m.route_cabinet(CabinetInput::RightFlipper, true),
        InputRoute::Sim
    );
    assert_eq!(
        m.route_cabinet(CabinetInput::RightFlipper, false),
        InputRoute::Sim
    );
    assert_eq!(m.route_axis(CabinetAxis::Plunger, 0.7), InputRoute::Sim);
    assert_eq!(m.route_axis(CabinetAxis::NudgeX, 0.2), InputRoute::Sim);
    assert_eq!(m.route_axis(CabinetAxis::NudgeX, 0.0), InputRoute::Sim);
    assert_eq!(
        m.route_axis(CabinetAxis::RightFlipper, 0.4),
        InputRoute::Sim
    );
    m.apply(MenuInput::Toggle);
    assert_eq!(
        m.drain_commands(),
        [
            UiCommand::ReleaseToSim {
                buttons: vec![CabinetInput::LeftFlipper, CabinetInput::Start],
                axes: vec![CabinetAxis::Plunger, CabinetAxis::RightFlipper],
            },
            UiCommand::MenuOpened
        ]
    );
    // The real release now goes to the menu; nothing is left for the sim.
    assert_eq!(
        m.route_cabinet(CabinetInput::LeftFlipper, false),
        InputRoute::Menu
    );
    assert_eq!(m.route_axis(CabinetAxis::Plunger, 0.0), InputRoute::Menu);
    m.apply(MenuInput::Toggle);
    m.apply(MenuInput::Toggle);
    assert_eq!(
        m.drain_commands(),
        [UiCommand::MenuClosed, UiCommand::MenuOpened]
    );
}

#[test]
fn loads_can_be_cancelled_and_stale_reports_rejected() {
    let mut m = menu();
    assert_eq!(m.pending_load_id(), None);
    assert!(!m.cancel_load());
    m.request_load_index(0).unwrap();
    let first = m.pending_load_id().unwrap();
    assert!(m.cancel_load());
    assert_eq!(m.load_state(), &TableLoadState::Idle);
    assert!(m.pending_commands().is_empty());
    assert!(m.notice().is_some_and(|n| n.contains("cancelled")));
    // A new request gets a new id; the stale report is rejected.
    m.request_load_index(2).unwrap();
    m.drain_commands();
    let second = m.pending_load_id().unwrap();
    assert_ne!(first, second);
    assert!(!m.report_load_result_for(first, Ok(())));
    assert_eq!(m.load_state(), &TableLoadState::Loading(p("gamma")));
    assert!(m.report_load_result_for(second, Err("no".into())));
    assert!(matches!(m.load_state(), TableLoadState::Failed { .. }));
    assert!(!m.report_load_result_for(second, Ok(())));
}

#[test]
fn new_sanitizes_the_config() {
    let mut c = AppConfig::default();
    c.calibration.plunger.deadzone = 7.0;
    c.audio.decoded_budget_mib = 1;
    let m = MenuState::new(c);
    let d = AppConfig::default();
    assert_eq!(
        m.config().calibration.plunger.deadzone,
        d.calibration.plunger.deadzone
    );
    assert_eq!(m.config().audio.decoded_budget_mib, 512);
}
