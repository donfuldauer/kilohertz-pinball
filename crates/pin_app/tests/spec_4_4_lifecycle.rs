//! Subphase 4.4 spec tests (Independent Tester): table switch lifecycle.
//!
//! Idle → LoadRequested(path) → Loading → Loaded | Failed(error) → Idle; a
//! second request while a load is pending is refused (published choice).
#![allow(clippy::field_reassign_with_default, clippy::unnecessary_cast)]

#[path = "fixtures/spec_4_4/support.rs"]
mod support;

use pin_app::ui::{
    AppConfig, LoadRefusal, MenuInput, MenuState, TableLoadState, UiCommand, scan_tables,
};
use proptest::prelude::*;
use std::path::{Path, PathBuf};
use support::*;

fn open_menu() -> MenuState {
    let mut m = MenuState::new(AppConfig::default());
    m.apply(MenuInput::Toggle);
    assert!(m.is_open());
    m.drain_commands();
    m
}

fn load_commands(cmds: &[UiCommand]) -> Vec<PathBuf> {
    cmds.iter()
        .filter_map(|c| match c {
            UiCommand::LoadTable(p) => Some(p.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn starts_idle() {
    let m = MenuState::default();
    assert_eq!(*m.load_state(), TableLoadState::Idle);
    assert!(!m.load_state().is_busy());
    assert!(m.pending_commands().is_empty());
}

#[test]
fn successful_load_walks_the_whole_lifecycle() {
    let mut m = open_menu();
    let path = Path::new("/tables/alpha.pinbundle");

    m.request_load(path).unwrap();
    assert_eq!(*m.load_state(), TableLoadState::LoadRequested(path.into()));
    assert!(m.load_state().is_busy());
    // Peeking at the command stream does not start the load.
    assert_eq!(
        load_commands(m.pending_commands()),
        vec![path.to_path_buf()]
    );
    assert_eq!(*m.load_state(), TableLoadState::LoadRequested(path.into()));

    let cmds = m.drain_commands();
    assert_eq!(load_commands(&cmds), vec![path.to_path_buf()]);
    assert_eq!(*m.load_state(), TableLoadState::Loading(path.into()));
    assert!(m.load_state().is_busy());
    assert!(m.pending_commands().is_empty());
    assert!(m.is_open(), "the menu stays up while loading");

    assert!(m.report_load_result(Ok(())));
    assert_eq!(*m.load_state(), TableLoadState::Loaded(path.into()));
    assert!(!m.load_state().is_busy());
    assert_eq!(m.config().last_table.as_deref(), Some(path));
    assert!(!m.is_open(), "a successful load closes the menu");
    assert!(!m.forwards_to_sim() || !m.is_open());
    let cmds = m.drain_commands();
    assert!(cmds.contains(&UiCommand::MenuClosed), "{cmds:?}");
    let changed: Vec<_> = cmds
        .iter()
        .filter_map(|c| match c {
            UiCommand::ConfigChanged(cfg) => Some(cfg.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(changed.len(), 1, "{cmds:?}");
    assert_eq!(changed[0].last_table.as_deref(), Some(path));
    assert!(load_commands(&cmds).is_empty());

    m.acknowledge_load();
    assert_eq!(*m.load_state(), TableLoadState::Idle);
    // The outcome was consumed.
    assert!(!m.report_load_result(Ok(())));
    assert_eq!(*m.load_state(), TableLoadState::Idle);
}

#[test]
fn failed_load_reports_the_error_and_keeps_the_menu_open() {
    let mut m = open_menu();
    let before = m.config().clone();
    let path = Path::new("/tables/broken.zip");
    m.request_load(path).unwrap();
    m.drain_commands();
    assert!(m.report_load_result(Err("mesh missing".to_owned())));
    assert_eq!(
        *m.load_state(),
        TableLoadState::Failed {
            path: path.into(),
            error: "mesh missing".to_owned()
        }
    );
    assert!(!m.load_state().is_busy());
    assert!(m.is_open());
    assert_eq!(
        *m.config(),
        before,
        "a failed load must not change last_table"
    );
    assert!(load_commands(m.pending_commands()).is_empty());
    assert!(!m.pending_commands().contains(&UiCommand::MenuClosed));

    m.acknowledge_load();
    assert_eq!(*m.load_state(), TableLoadState::Idle);
    assert!(m.is_open());
}

#[test]
fn second_request_while_pending_is_refused() {
    let mut m = open_menu();
    let first = Path::new("/tables/first");
    let second = Path::new("/tables/second");

    m.request_load(first).unwrap();
    // While LoadRequested.
    assert_eq!(m.request_load(second), Err(LoadRefusal::Busy(first.into())));
    assert_eq!(m.request_load(first), Err(LoadRefusal::Busy(first.into())));
    assert_eq!(*m.load_state(), TableLoadState::LoadRequested(first.into()));
    assert_eq!(
        load_commands(m.pending_commands()),
        vec![first.to_path_buf()]
    );

    // While Loading.
    m.drain_commands();
    assert_eq!(m.request_load(second), Err(LoadRefusal::Busy(first.into())));
    assert_eq!(*m.load_state(), TableLoadState::Loading(first.into()));
    assert!(
        load_commands(m.pending_commands()).is_empty(),
        "a refused request must not emit LoadTable"
    );

    // The pending load still completes for the first path.
    assert!(m.report_load_result(Ok(())));
    assert_eq!(*m.load_state(), TableLoadState::Loaded(first.into()));
    assert_eq!(m.config().last_table.as_deref(), Some(first));
}

#[test]
fn new_request_is_accepted_straight_after_an_outcome() {
    // From Failed.
    let mut m = open_menu();
    m.request_load(Path::new("/t/a")).unwrap();
    m.drain_commands();
    m.report_load_result(Err("x".into()));
    m.request_load(Path::new("/t/b")).unwrap();
    assert_eq!(
        *m.load_state(),
        TableLoadState::LoadRequested("/t/b".into())
    );
    assert_eq!(
        load_commands(&m.drain_commands()),
        vec![PathBuf::from("/t/b")]
    );
    // From Loaded.
    assert!(m.report_load_result(Ok(())));
    m.request_load(Path::new("/t/c")).unwrap();
    assert_eq!(
        *m.load_state(),
        TableLoadState::LoadRequested("/t/c".into())
    );
}

#[test]
fn outcome_returns_to_idle_on_menu_activity() {
    // Failed → Idle on any input while open.
    let mut m = open_menu();
    m.request_load(Path::new("/t/a")).unwrap();
    m.drain_commands();
    m.report_load_result(Err("x".into()));
    m.apply(MenuInput::Up);
    assert_eq!(*m.load_state(), TableLoadState::Idle);

    // Loaded (menu closed) → Idle when the menu is opened again.
    let mut m = open_menu();
    m.request_load(Path::new("/t/a")).unwrap();
    m.drain_commands();
    m.report_load_result(Ok(()));
    assert!(!m.is_open());
    m.open();
    assert!(m.is_open());
    assert_eq!(*m.load_state(), TableLoadState::Idle);
}

#[test]
fn menu_activity_does_not_cancel_a_pending_load() {
    let mut m = open_menu();
    let path = Path::new("/t/a");
    m.request_load(path).unwrap();
    m.apply_all([MenuInput::Up, MenuInput::Down, MenuInput::Right]);
    assert_eq!(*m.load_state(), TableLoadState::LoadRequested(path.into()));
    m.drain_commands();
    m.apply_all([MenuInput::Toggle, MenuInput::Toggle, MenuInput::Left]);
    m.acknowledge_load();
    assert_eq!(*m.load_state(), TableLoadState::Loading(path.into()));
    assert!(m.report_load_result(Ok(())));
}

#[test]
fn reporting_with_nothing_pending_is_rejected() {
    let mut m = open_menu();
    assert!(!m.report_load_result(Ok(())));
    assert!(!m.report_load_result(Err("late".into())));
    assert_eq!(*m.load_state(), TableLoadState::Idle);
    assert_eq!(m.config().last_table, None);
    assert!(m.is_open());
    assert!(m.pending_commands().is_empty());
}

#[test]
fn browser_select_requests_the_focused_table() {
    let tmp = TempDir::new("lc_select");
    write_valid_bundle(&tmp.join("aaa"), "aaa", "Aaa", "x");
    write_valid_bundle(&tmp.join("bbb"), "bbb", "Bbb", "x");
    let entries = scan_tables(tmp.path());
    assert_eq!(entries.len(), 2);

    let mut m = open_menu();
    m.set_tables(entries.clone());
    assert_eq!(m.tables(), &entries[..]);
    assert_eq!(m.visible_tables().len(), 2);
    assert_eq!(m.focus(), 0);
    assert_eq!(m.focused_table(), None, "the tab bar is focused first");

    m.apply_all([MenuInput::Down, MenuInput::Down]);
    let idx = m.focused_table().expect("second table row focused");
    assert_eq!(idx, m.visible_tables()[1]);
    m.apply(MenuInput::Select);
    let path = entries[idx].path.clone();
    assert_eq!(*m.load_state(), TableLoadState::LoadRequested(path.clone()));
    assert_eq!(load_commands(&m.drain_commands()), vec![path.clone()]);
    assert_eq!(*m.load_state(), TableLoadState::Loading(path.clone()));

    // One-click load of another table while loading: refused, nothing emitted.
    let other = m.visible_tables()[0];
    m.apply(MenuInput::LoadTable(other));
    assert_eq!(*m.load_state(), TableLoadState::Loading(path.clone()));
    assert!(load_commands(m.pending_commands()).is_empty());

    assert!(m.report_load_result(Ok(())));
    assert_eq!(m.config().last_table, Some(path));
}

#[test]
fn one_click_load_input_requests_that_table() {
    let tmp = TempDir::new("lc_click");
    write_valid_bundle(&tmp.join("aaa"), "aaa", "Aaa", "x");
    write_valid_bundle(&tmp.join("bbb"), "bbb", "Bbb", "x");
    let entries = scan_tables(tmp.path());
    let mut m = open_menu();
    m.set_tables(entries.clone());
    m.apply(MenuInput::LoadTable(1));
    assert_eq!(
        *m.load_state(),
        TableLoadState::LoadRequested(entries[1].path.clone())
    );
}

#[test]
fn broken_and_missing_tables_are_refused() {
    let tmp = TempDir::new("lc_broken");
    write_valid_bundle(&tmp.join("good"), "good", "Good", "x");
    write_garbage_manifest_bundle(&tmp.join("bad"));
    let entries = scan_tables(tmp.path());
    let bad = entries.iter().position(|e| !e.is_ready()).unwrap();
    let good = entries.iter().position(|e| e.is_ready()).unwrap();

    let mut m = open_menu();
    m.set_tables(entries.clone());
    match m.request_load_index(bad) {
        Err(LoadRefusal::Broken(e)) => assert_eq!(e, entries[bad].error().unwrap()),
        other => panic!("expected Broken, got {other:?}"),
    }
    assert_eq!(
        m.request_load_index(entries.len()),
        Err(LoadRefusal::NoSuchTable(entries.len()))
    );
    assert_eq!(
        m.request_load_index(usize::MAX),
        Err(LoadRefusal::NoSuchTable(usize::MAX))
    );
    m.apply(MenuInput::LoadTable(bad));
    m.apply(MenuInput::LoadTable(99));
    assert_eq!(*m.load_state(), TableLoadState::Idle);
    assert!(load_commands(m.pending_commands()).is_empty());

    m.request_load_index(good).unwrap();
    assert_eq!(
        *m.load_state(),
        TableLoadState::LoadRequested(entries[good].path.clone())
    );
    assert_eq!(
        m.request_load_index(good),
        Err(LoadRefusal::Busy(entries[good].path.clone()))
    );
}

#[test]
fn loaded_table_is_persisted_as_last_table() {
    let tmp = TempDir::new("lc_persist");
    let cfg_path = tmp.join("config.toml");
    let mut m = open_menu();
    let table = tmp.join("some table.pinbundle");
    m.request_load(&table).unwrap();
    m.drain_commands();
    m.report_load_result(Ok(()));
    for c in m.drain_commands() {
        if let UiCommand::ConfigChanged(cfg) = c {
            cfg.save(&cfg_path).unwrap();
        }
    }
    let loaded = AppConfig::load(&cfg_path);
    assert!(loaded.warnings.is_empty(), "{:?}", loaded.warnings);
    assert_eq!(loaded.config.last_table, Some(table));
}

#[derive(Clone, Debug)]
enum Op {
    Request(u8),
    Drain,
    ReportOk,
    ReportErr,
    Ack,
    Input(u8),
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        (0u8..4).prop_map(Op::Request),
        Just(Op::Drain),
        Just(Op::ReportOk),
        Just(Op::ReportErr),
        Just(Op::Ack),
        (0u8..7).prop_map(Op::Input),
    ]
}

/// The lifecycle as the brief states it, as a reference model.
#[derive(Clone, Debug, PartialEq)]
enum Model {
    Idle,
    Requested(PathBuf),
    Loading(PathBuf),
    Loaded(PathBuf),
    Failed(PathBuf),
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 256, failure_persistence: None, ..ProptestConfig::default() })]

    /// Under any interleaving: only legal transitions happen, exactly one
    /// `LoadTable` is emitted per accepted request, a pending load is never
    /// replaced, and `last_table` only ever names a table that loaded.
    #[test]
    fn lifecycle_follows_the_model(ops in proptest::collection::vec(op(), 0..60)) {
        let mut m = MenuState::new(AppConfig::default());
        let mut model = Model::Idle;
        let mut last_loaded: Option<PathBuf> = None;
        let mut emitted = 0usize;
        let mut accepted = 0usize;
        for op in ops {
            match op {
                Op::Request(n) => {
                    let p = PathBuf::from(format!("/t/{n}"));
                    let r = m.request_load(&p);
                    match &model {
                        Model::Requested(cur) | Model::Loading(cur) => {
                            prop_assert_eq!(r, Err(LoadRefusal::Busy(cur.clone())));
                        }
                        _ => {
                            prop_assert_eq!(r, Ok(()));
                            accepted += 1;
                            model = Model::Requested(p);
                        }
                    }
                }
                Op::Drain => {
                    let cmds = m.drain_commands();
                    let loads = load_commands(&cmds);
                    emitted += loads.len();
                    if let Model::Requested(p) = model.clone() {
                        prop_assert_eq!(loads, vec![p.clone()]);
                        model = Model::Loading(p);
                    } else {
                        prop_assert!(loads.is_empty());
                    }
                }
                Op::ReportOk | Op::ReportErr => {
                    // Reporting before the command was drained is not
                    // specified; the model only reports while Loading.
                    if let Model::Loading(p) = model.clone() {
                        if matches!(op, Op::ReportOk) {
                            prop_assert!(m.report_load_result(Ok(())));
                            last_loaded = Some(p.clone());
                            model = Model::Loaded(p);
                        } else {
                            prop_assert!(m.report_load_result(Err("boom".into())));
                            model = Model::Failed(p);
                        }
                    } else if !matches!(model, Model::Requested(_)) {
                        prop_assert!(!m.report_load_result(Ok(())));
                    }
                }
                Op::Ack => {
                    m.acknowledge_load();
                    if matches!(model, Model::Loaded(_) | Model::Failed(_)) {
                        model = Model::Idle;
                    }
                }
                Op::Input(n) => {
                    let was_open = m.is_open();
                    let input = match n {
                        0 => MenuInput::Toggle,
                        1 => MenuInput::Up,
                        2 => MenuInput::Down,
                        3 => MenuInput::Left,
                        4 => MenuInput::Right,
                        5 => MenuInput::Back,
                        _ => MenuInput::Rescan,
                    };
                    m.apply(input);
                    // Loaded/Failed fall back to Idle on activity while open
                    // or when the menu opens; both are allowed here.
                    if matches!(model, Model::Loaded(_) | Model::Failed(_))
                        && *m.load_state() == TableLoadState::Idle
                    {
                        prop_assert!(was_open || m.is_open());
                        model = Model::Idle;
                    }
                }
            }
            let expected = match &model {
                Model::Idle => TableLoadState::Idle,
                Model::Requested(p) => TableLoadState::LoadRequested(p.clone()),
                Model::Loading(p) => TableLoadState::Loading(p.clone()),
                Model::Loaded(p) => TableLoadState::Loaded(p.clone()),
                Model::Failed(p) => TableLoadState::Failed { path: p.clone(), error: "boom".into() },
            };
            prop_assert_eq!(m.load_state(), &expected);
            prop_assert_eq!(
                m.load_state().is_busy(),
                matches!(model, Model::Requested(_) | Model::Loading(_))
            );
            prop_assert_eq!(&m.config().last_table, &last_loaded);
            prop_assert_eq!(m.forwards_to_sim(), !m.is_open());
        }
        emitted += load_commands(&m.drain_commands()).len();
        prop_assert_eq!(emitted, accepted);
    }
}
