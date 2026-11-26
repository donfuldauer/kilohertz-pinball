//! KP-005 (#85) black-box tests, REQ-VIEW-006 (SPEC section 7) and SPEC section 4
//! Subphase 4.4 (directory scan, menu table-load lifecycle): the table browser
//! is wired into the app. Headless, simulated time, no sleeps: `recv()` blocks
//! on the worker's reply and nothing asserts on wall-clock time.

#[path = "ui_fixtures.rs"]
mod fixtures;

use fixtures::*;
use pin_app::orchestrator::{IoReply, IoRequest, IoWorker, load_app_table};
use pin_app::ui::{
    LOAD_NOT_WIRED, MenuController, MenuInput, TABLES_DIR_NAME, TableEntry, TableLoadState,
    browser_dir, resolve_tables_dir, scan_tables,
};
use pin_audio::mixer::MixerConfig;
use pin_core::{FrameStateSnapshot, InputEvent, SimStep};
use proptest::prelude::*;
use std::path::{Path, PathBuf};

fn sink() -> impl FnMut(InputEvent) {
    |_| {}
}

fn args(list: &[&str]) -> Vec<String> {
    list.iter().map(|s| (*s).to_owned()).collect()
}

fn worker() -> IoWorker {
    IoWorker::spawn(MixerConfig::default())
}

/// `<tmp>/tables` with two valid bundles, a broken bundle, a hidden folder and
/// a loose file. Returns `(tables_dir, tmp)`.
fn populated(tag: &str) -> (PathBuf, PathBuf) {
    let tmp = temp_dir(tag);
    let dir = tmp.join(TABLES_DIR_NAME);
    let files = bundle_files();
    write_dir(&dir.join("b_apex"), &files);
    write_dir(
        &dir.join("A_second"),
        &edit_table(&files, "name = \"Apex Circuit\"", "name = \"Second Table\""),
    );
    write_dir(
        &dir.join("c_broken"),
        &with_file(&files, "table.toml", b"this is [not toml"),
    );
    write_dir(&dir.join(".hidden"), &files);
    std::fs::write(dir.join("notes.txt"), b"hello").unwrap();
    (dir, tmp)
}

fn summary(entries: &[TableEntry]) -> Vec<(String, String, bool)> {
    entries
        .iter()
        .map(|e| (e.file_name.clone(), e.title().to_owned(), e.is_ready()))
        .collect()
}

fn scan_on_worker(w: &IoWorker, dir: &Path) -> Vec<TableEntry> {
    w.request(IoRequest::Scan(dir.to_path_buf()));
    match w.recv().expect("worker answers") {
        IoReply::Scanned { root, tables } => {
            assert_eq!(root, dir);
            tables
        }
        _ => panic!("a scan request must be answered with Scanned"),
    }
}

/// Outcome of a worker load: `Ok(app)` or `Err(error)`; id and path are checked.
fn load_on_worker(
    w: &IoWorker,
    id: u64,
    path: &Path,
) -> Result<Box<pin_app::orchestrator::LoadedApp>, String> {
    w.request(IoRequest::Load {
        id,
        path: path.to_path_buf(),
    });
    match w.recv().expect("worker answers") {
        IoReply::Loaded {
            id: rid,
            path: p,
            app,
        } => {
            assert_eq!(rid, id, "reply id");
            assert_eq!(p, path, "reply path");
            Ok(app)
        }
        IoReply::LoadFailed {
            id: rid,
            path: p,
            error,
        } => {
            assert_eq!(rid, id, "reply id");
            assert_eq!(p, path, "reply path");
            Err(error)
        }
        IoReply::Scanned { .. } => panic!("a load request must not be answered with Scanned"),
    }
}

/// REQ-VIEW-006 req 1: directory resolution.
#[test]
fn req_view_006_resolve_tables_dir() {
    assert_eq!(TABLES_DIR_NAME, "tables");
    for (cfg, exe, want) in [
        (Some("/data/t"), "/opt/kp/kilohertz", "/data/t"),
        (Some("rel/t"), "/x", "rel/t"),
        (None, "/opt/kp/kilohertz", "/opt/kp/tables"),
        (None, "/a/b/c/app.exe", "/a/b/c/tables"),
        (None, "kilohertz", "./tables"),
    ] {
        let got = resolve_tables_dir(cfg.map(Path::new), Path::new(exe));
        assert_eq!(got, PathBuf::from(want), "cfg {cfg:?} exe {exe:?}");
    }
    // An exe path without a parent (the filesystem root) falls back to ./tables.
    assert_eq!(
        resolve_tables_dir(None, Path::new("")),
        PathBuf::from("./tables")
    );
}

/// REQ-VIEW-006 req 1: `--table` skips the browser, anything else keeps it.
#[test]
fn req_view_006_browser_dir_respects_table_flag() {
    let exe = Path::new("/opt/kp/kilohertz");
    let cfg = Path::new("/data/t");
    assert_eq!(browser_dir(&args(&["--table", "x.zip"]), None, exe), None);
    assert_eq!(browser_dir(&args(&["--table=x.zip"]), Some(cfg), exe), None);
    assert_eq!(
        browser_dir(&args(&["--headless", "--table", "x"]), None, exe),
        None
    );
    for list in [
        &[][..],
        &["--headless"][..],
        &["--config", "x"][..],
        &["--run-ms", "5"][..],
        &["--headless", "--config", "x", "--run-ms", "5"][..],
    ] {
        assert_eq!(
            browser_dir(&args(list), None, exe),
            Some(PathBuf::from("/opt/kp/tables")),
            "{list:?}"
        );
        assert_eq!(
            browser_dir(&args(list), Some(cfg), exe),
            Some(cfg.to_path_buf()),
            "{list:?}"
        );
        assert_eq!(
            browser_dir(&args(list), Some(cfg), exe),
            Some(resolve_tables_dir(Some(cfg), exe))
        );
    }
}

/// REQ-VIEW-006 req 2: the worker scans like `scan_tables`.
#[test]
fn req_view_006_scan_lists_bundles_on_the_worker() {
    let (dir, tmp) = populated("kp005_scan");
    let w = worker();
    let tables = scan_on_worker(&w, &dir);
    let direct = scan_tables(&dir);
    assert_eq!(summary(&tables), summary(&direct));
    let names: Vec<&str> = tables.iter().map(|e| e.file_name.as_str()).collect();
    assert_eq!(names, ["A_second", "b_apex", "c_broken"]);
    assert_eq!(tables[0].title(), "Second Table");
    assert_eq!(tables[1].title(), "Apex Circuit");
    assert!(tables[0].is_ready() && tables[1].is_ready());
    assert!(!tables[2].is_ready());
    assert!(tables[2].error().is_some_and(|e| !e.is_empty()));
    let keys: Vec<String> = names.iter().map(|n| n.to_ascii_lowercase()).collect();
    assert!(keys.windows(2).all(|p| p[0] <= p[1]), "sorted: {keys:?}");
    assert!(
        names
            .iter()
            .all(|n| !n.starts_with('.') && *n != "notes.txt")
    );
    for (e, d) in tables.iter().zip(&direct) {
        assert_eq!(e.path, d.path);
    }
    let _ = std::fs::remove_dir_all(tmp);
}

/// REQ-VIEW-006 req 2: a missing directory is an empty scan, not a failure.
#[test]
fn req_view_006_scan_missing_directory_is_empty() {
    let tmp = temp_dir("kp005_nodir");
    let missing = tmp.join("nope");
    let w = worker();
    assert!(scan_on_worker(&w, &missing).is_empty());
    // An existing but empty directory behaves the same.
    assert!(scan_on_worker(&w, &tmp).is_empty());
    let _ = std::fs::remove_dir_all(tmp);
}

/// REQ-VIEW-006 req 3: a load returns a steppable table; two bundles differ by path.
#[test]
fn req_view_006_load_on_worker_steps_in_simulated_time() {
    let (dir, tmp) = populated("kp005_load");
    let w = worker();
    let a = dir.join("b_apex");
    let b = dir.join("A_second");
    let mut app_a = load_on_worker(&w, 7, &a).unwrap_or_else(|e| panic!("apex: {e}"));
    let mut app_b = load_on_worker(&w, 8, &b).unwrap_or_else(|e| panic!("second: {e}"));
    for app in [&mut app_a, &mut app_b] {
        let mut snap = FrameStateSnapshot::default();
        for tick in 1..=3u64 {
            app.sim.step_1ms(tick, &[], &mut snap);
            assert_eq!(snap.tick, tick);
        }
    }
    // The same bundle loaded twice steps to the same state (deterministic).
    let mut again = load_on_worker(&w, 9, &a).unwrap();
    let mut snap = FrameStateSnapshot::default();
    let mut snap2 = FrameStateSnapshot::default();
    let mut fresh = load_on_worker(&w, 10, &a).unwrap();
    for tick in 1..=20u64 {
        again.sim.step_1ms(tick, &[], &mut snap);
        fresh.sim.step_1ms(tick, &[], &mut snap2);
    }
    assert_eq!(again.sim.sim().state_hash(), fresh.sim.sim().state_hash());
    let _ = std::fs::remove_dir_all(tmp);
}

/// REQ-VIEW-006 req 3: `load_app_table` and the worker agree on Ok/Err.
#[test]
fn req_view_006_load_app_table_matches_worker() {
    let (dir, tmp) = populated("kp005_direct");
    std::fs::write(dir.join("g.zip"), b"PK garbage").unwrap();
    let w = worker();
    for (i, name) in ["b_apex", "A_second", "c_broken", "g.zip", "missing"]
        .iter()
        .enumerate()
    {
        let p = dir.join(name);
        let direct = load_app_table(&p, MixerConfig::default());
        let via = load_on_worker(&w, i as u64, &p);
        assert_eq!(direct.is_ok(), via.is_ok(), "{name}");
        assert_eq!(
            direct.is_ok(),
            matches!(*name, "b_apex" | "A_second"),
            "{name}"
        );
    }
    let _ = std::fs::remove_dir_all(tmp);
}

/// REQ-VIEW-006 req 4: failures are reported with id/path and the worker survives.
#[test]
fn req_view_006_load_failure_reports_and_worker_survives() {
    let (dir, tmp) = populated("kp005_fail");
    std::fs::write(dir.join("garbage.zip"), b"PK not really a zip").unwrap();
    let w = worker();
    let bad = [
        dir.join("c_broken"),
        dir.join("garbage.zip"),
        dir.join("does_not_exist"),
    ];
    for (i, p) in bad.iter().enumerate() {
        let err = load_on_worker(&w, 100 + i as u64, p).err();
        assert!(err.is_some_and(|e| !e.is_empty()), "{p:?}");
        // Still alive: a good load right after succeeds.
        assert!(load_on_worker(&w, 200 + i as u64, &dir.join("b_apex")).is_ok());
    }
    let _ = std::fs::remove_dir_all(tmp);
}

/// REQ-VIEW-006 req 5: N requests, N replies, in order; idle `try_recv` is None; drop joins.
#[test]
fn req_view_006_replies_in_request_order_and_teardown() {
    let (dir, tmp) = populated("kp005_order");
    let w = worker();
    assert!(w.try_recv().is_none(), "idle worker has no reply");
    w.request(IoRequest::Load {
        id: 1,
        path: dir.join("b_apex"),
    });
    w.request(IoRequest::Scan(dir.clone()));
    w.request(IoRequest::Load {
        id: 3,
        path: dir.join("c_broken"),
    });
    w.request(IoRequest::Scan(tmp.join("missing")));
    w.request(IoRequest::Load {
        id: 5,
        path: dir.join("A_second"),
    });
    assert!(matches!(w.recv(), Some(IoReply::Loaded { id: 1, .. })));
    assert!(
        matches!(w.recv(), Some(IoReply::Scanned { root, tables }) if root == dir && tables.len() == 3)
    );
    assert!(matches!(w.recv(), Some(IoReply::LoadFailed { id: 3, .. })));
    assert!(matches!(w.recv(), Some(IoReply::Scanned { tables, .. }) if tables.is_empty()));
    assert!(matches!(w.recv(), Some(IoReply::Loaded { id: 5, .. })));
    assert!(w.try_recv().is_none(), "no extra replies");
    drop(w);
    // Dropping with requests still queued returns.
    let w = worker();
    for i in 0..6 {
        w.request(IoRequest::Load {
            id: i,
            path: dir.join("b_apex"),
        });
    }
    w.request(IoRequest::Scan(dir.clone()));
    drop(w);
    let w = worker();
    drop(w);
    let _ = std::fs::remove_dir_all(tmp);
}

fn controller(tag: &str) -> (MenuController, PathBuf, PathBuf, Vec<TableEntry>) {
    let (dir, tmp) = populated(tag);
    let cfg = tmp.join("config.toml");
    let (mut c, warnings) = MenuController::load(&cfg);
    assert!(warnings.is_empty());
    let tables = scan_tables(&dir);
    c.set_tables(tables.clone());
    (c, cfg, tmp, tables)
}

/// REQ-VIEW-006 req 6: menu hand-off in browser mode.
#[test]
fn req_view_006_menu_load_handoff_success() {
    let (mut c, _cfg, tmp, tables) = controller("kp005_menu_ok");
    c.set_browser_enabled(true);
    assert!(c.take_load_request().is_none(), "nothing requested yet");
    c.toggle(1, &mut sink());
    assert!(c.is_open());
    c.apply(MenuInput::LoadTable(1), 2, &mut sink());
    let want = tables[1].path.clone();
    let (id, path) = c.take_load_request().expect("a load request");
    assert_eq!(path, want);
    assert!(c.take_load_request().is_none(), "exactly once");
    assert_eq!(
        c.menu().load_state(),
        &TableLoadState::Loading(want.clone())
    );
    // A second load while one is pending is refused.
    c.apply(MenuInput::LoadTable(0), 3, &mut sink());
    assert!(
        c.take_load_request().is_none(),
        "no second request while busy"
    );
    assert_eq!(
        c.menu().load_state(),
        &TableLoadState::Loading(want.clone())
    );
    let _ = c.take_config_change();
    c.finish_load(id, Ok(()));
    assert_eq!(c.menu().load_state(), &TableLoadState::Loaded(want.clone()));
    assert_eq!(c.config().last_table.as_deref(), Some(want.as_path()));
    assert!(!c.is_open(), "a loaded table closes the menu");
    let changed = c.take_config_change().expect("config change handed out");
    assert_eq!(changed.last_table.as_deref(), Some(want.as_path()));
    assert!(c.take_config_change().is_none());
    assert!(c.take_load_request().is_none());
    let _ = std::fs::remove_dir_all(tmp);
}

/// REQ-VIEW-006 req 6: Down then Select picks the focused ready table.
#[test]
fn req_view_006_menu_down_select_requests_load() {
    let (mut c, _cfg, tmp, tables) = controller("kp005_menu_sel");
    c.set_browser_enabled(true);
    c.toggle(1, &mut sink());
    c.apply(MenuInput::Down, 2, &mut sink());
    c.apply(MenuInput::Select, 3, &mut sink());
    let (_, path) = c.take_load_request().expect("Select requests a load");
    assert!(tables.iter().any(|t| t.path == path && t.is_ready()));
    let _ = std::fs::remove_dir_all(tmp);
}

/// REQ-VIEW-006 req 6: a failed load keeps its text and leaves last_table alone.
#[test]
fn req_view_006_menu_load_failure_keeps_text() {
    let (mut c, _cfg, tmp, tables) = controller("kp005_menu_fail");
    c.set_browser_enabled(true);
    c.toggle(1, &mut sink());
    c.apply(MenuInput::LoadTable(0), 2, &mut sink());
    let (id, path) = c.take_load_request().unwrap();
    assert_eq!(path, tables[0].path);
    c.finish_load(id, Err("boom".into()));
    match c.menu().load_state() {
        TableLoadState::Failed { path: p, error } => {
            assert_eq!(p, &path);
            assert!(error.contains("boom"), "{error}");
            assert_ne!(error, LOAD_NOT_WIRED);
        }
        other => panic!("expected Failed, got {other:?}"),
    }
    assert_eq!(c.config().last_table, None);
    assert!(c.take_load_request().is_none());
    let _ = std::fs::remove_dir_all(tmp);
}

/// REQ-VIEW-006 req 6: rescan flag is set once per request.
#[test]
fn req_view_006_menu_rescan_request_once() {
    let (mut c, _cfg, tmp, _t) = controller("kp005_menu_rescan");
    c.set_browser_enabled(true);
    assert!(!c.take_rescan_request());
    c.toggle(1, &mut sink());
    c.apply(MenuInput::Rescan, 2, &mut sink());
    assert!(c.take_rescan_request());
    assert!(!c.take_rescan_request());
    c.apply(MenuInput::Rescan, 3, &mut sink());
    assert!(c.take_rescan_request());
    assert!(!c.take_rescan_request());
    let _ = std::fs::remove_dir_all(tmp);
}

/// REQ-VIEW-006 req 7 (and #84): without a browser a load answers LOAD_NOT_WIRED.
#[test]
fn req_view_006_menu_load_not_wired_by_default() {
    let (mut c, _cfg, tmp, _t) = controller("kp005_menu_nowire");
    c.toggle(1, &mut sink());
    c.apply(MenuInput::LoadTable(0), 2, &mut sink());
    assert!(c.take_load_request().is_none());
    match c.menu().load_state() {
        TableLoadState::Failed { error, .. } => assert_eq!(error, LOAD_NOT_WIRED),
        other => panic!("expected Failed(LOAD_NOT_WIRED), got {other:?}"),
    }
    assert_eq!(c.config().last_table, None);
    // Explicitly disabled behaves the same.
    let (mut c, _cfg, tmp2, _t) = controller("kp005_menu_nowire2");
    c.set_browser_enabled(true);
    c.set_browser_enabled(false);
    c.toggle(1, &mut sink());
    c.apply(MenuInput::LoadTable(0), 2, &mut sink());
    assert!(c.take_load_request().is_none());
    assert!(matches!(
        c.menu().load_state(),
        TableLoadState::Failed { error, .. } if error == LOAD_NOT_WIRED
    ));
    let _ = std::fs::remove_dir_all(tmp);
    let _ = std::fs::remove_dir_all(tmp2);
}

/// REQ-VIEW-006 req 8: scan -> choose -> worker load -> finish -> persisted.
#[test]
fn req_view_006_end_to_end_headless_flow() {
    let (dir, tmp) = populated("kp005_e2e");
    let cfg = tmp.join("config.toml");
    let (mut c, warnings) = MenuController::load(&cfg);
    assert!(warnings.is_empty());
    assert!(c.config().tables_dir.is_none());
    c.set_browser_enabled(true);
    let w = worker();
    let tables = scan_on_worker(&w, &dir);
    c.set_tables(tables.clone());
    c.toggle(1, &mut sink());
    c.apply(MenuInput::LoadTable(1), 2, &mut sink());
    let (id, path) = c.take_load_request().expect("load request");
    assert_eq!(path, tables[1].path);
    let mut app = load_on_worker(&w, id, &path).unwrap_or_else(|e| panic!("{e}"));
    let mut snap = FrameStateSnapshot::default();
    app.sim.step_1ms(1, &[], &mut snap);
    assert_eq!(snap.tick, 1);
    c.finish_load(id, Ok(()));
    assert_eq!(c.config().last_table.as_deref(), Some(path.as_path()));
    let taken = c.take_config_change().expect("config change");
    assert_eq!(taken.last_table.as_deref(), Some(path.as_path()));
    let (c2, w2) = MenuController::load(&cfg);
    assert!(w2.is_empty());
    assert_eq!(c2.config().last_table.as_deref(), Some(path.as_path()));
    let _ = std::fs::remove_dir_all(tmp);
}

fn xorshift(state: &mut u64) -> u64 {
    let mut x = *state;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    *state = x;
    x
}

fn blob(state: &mut u64, max: usize) -> Vec<u8> {
    let n = (xorshift(state) as usize) % (max + 1);
    let mut v: Vec<u8> = (0..n).map(|_| xorshift(state) as u8).collect();
    match xorshift(state) % 4 {
        0 if v.len() >= 4 => v[..4].copy_from_slice(b"PK\x03\x04"),
        1 if v.len() >= 8 => v[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n"),
        _ => {}
    }
    v
}

/// REQ-VIEW-006 req 9 (R11): bounded random bytes never panic or hang the worker.
#[test]
fn req_view_006_random_bytes_survive_scan_and_load() {
    let tmp = temp_dir("kp005_fuzz");
    let dir = tmp.join(TABLES_DIR_NAME);
    std::fs::create_dir_all(&dir).unwrap();
    let mut s = 0x9E37_79B9_7F4A_7C15u64;
    let files = bundle_files();
    for i in 0..100 {
        match i % 3 {
            0 => std::fs::write(dir.join(format!("r{i:03}.zip")), blob(&mut s, 2048)).unwrap(),
            1 => {
                std::fs::write(dir.join(format!("r{i:03}.pinbundle")), blob(&mut s, 2048)).unwrap()
            }
            _ => {
                let toml = blob(&mut s, 2048);
                write_dir(
                    &dir.join(format!("r{i:03}")),
                    &with_file(&files, "table.toml", &toml),
                );
            }
        }
    }
    let w = worker();
    let tables = scan_on_worker(&w, &dir);
    assert_eq!(tables.len(), 100, "every candidate listed, broken or not");
    for (i, t) in tables.iter().enumerate() {
        assert!(
            !t.is_ready(),
            "random bytes are not a table: {}",
            t.file_name
        );
        assert!(t.error().is_some_and(|e| !e.is_empty()));
        let r = load_on_worker(&w, i as u64, &t.path);
        assert!(r.is_err(), "{}", t.file_name);
    }
    // The worker still serves a good request after the barrage.
    write_dir(&dir.join("zz_good"), &files);
    assert!(load_on_worker(&w, 999, &dir.join("zz_good")).is_ok());
    let _ = std::fs::remove_dir_all(tmp);
}

#[derive(Clone, Copy, Debug)]
enum Kind {
    ScanValid,
    ScanMissing,
    LoadValid,
    LoadBroken,
    LoadMissing,
}

fn kind() -> impl Strategy<Value = Kind> {
    prop_oneof![
        Just(Kind::ScanValid),
        Just(Kind::ScanMissing),
        Just(Kind::LoadValid),
        Just(Kind::LoadBroken),
        Just(Kind::LoadMissing),
    ]
}

fn shared_dir() -> &'static (PathBuf, PathBuf) {
    static D: std::sync::OnceLock<(PathBuf, PathBuf)> = std::sync::OnceLock::new();
    D.get_or_init(|| populated("kp005_prop"))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(24))]

    /// REQ-VIEW-006 req 10: replies match the requests in count, order and kind.
    #[test]
    fn req_view_006_prop_one_reply_per_request_in_order(kinds in prop::collection::vec(kind(), 1..8)) {
        let (dir, tmp) = shared_dir();
        let w = worker();
        let path_for = |k: Kind| match k {
            Kind::LoadValid => dir.join("b_apex"),
            Kind::LoadBroken => dir.join("c_broken"),
            _ => tmp.join("nonexistent"),
        };
        for (i, k) in kinds.iter().enumerate() {
            match k {
                Kind::ScanValid => w.request(IoRequest::Scan(dir.clone())),
                Kind::ScanMissing => w.request(IoRequest::Scan(tmp.join("missing_dir"))),
                _ => w.request(IoRequest::Load { id: i as u64, path: path_for(*k) }),
            }
        }
        for (i, k) in kinds.iter().enumerate() {
            let reply = w.recv().expect("one reply per request");
            match (k, reply) {
                (Kind::ScanValid, IoReply::Scanned { root, tables }) => {
                    prop_assert_eq!(&root, dir);
                    prop_assert_eq!(tables.len(), 3);
                }
                (Kind::ScanMissing, IoReply::Scanned { tables, .. }) => {
                    prop_assert!(tables.is_empty());
                }
                (Kind::LoadValid, IoReply::Loaded { id, path, .. }) => {
                    prop_assert_eq!(id, i as u64);
                    prop_assert_eq!(path, path_for(*k));
                }
                (Kind::LoadBroken | Kind::LoadMissing, IoReply::LoadFailed { id, path, error }) => {
                    prop_assert_eq!(id, i as u64);
                    prop_assert_eq!(path, path_for(*k));
                    prop_assert!(!error.is_empty());
                }
                (k, _) => prop_assert!(false, "reply kind mismatch for {:?} at {}", k, i),
            }
        }
        prop_assert!(w.try_recv().is_none());
    }
}
