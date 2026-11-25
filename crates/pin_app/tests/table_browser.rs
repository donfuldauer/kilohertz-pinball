//! Issue #85 (KP-005, REQ-VIEW-006, SPEC §4 Subphase 4.4): the table browser
//! wired into the app. Headless and GPU-free, simulated time only (no
//! sleeps; blocking on the I/O worker's reply channel is not sleeping).
//!
//! * the tables folder resolves to `<exe dir>/tables` (or `tables_dir`),
//!   `--table` disables the browser;
//! * the I/O worker scans and loads in request order, survives broken
//!   bundles, missing paths and random bytes, and joins when dropped;
//! * the menu hands loads and rescans to the app and reports the outcome;
//! * the whole flow: scan, pick, load, tick, `last_table` saved.

#[path = "apex_support/mod.rs"]
mod apex;
#[path = "ui_fixtures.rs"]
mod fixtures;
#[path = "support/mod.rs"]
mod greybox;

use pin_app::orchestrator::{IoReply, IoRequest, IoWorker, LoadedApp, run_headless_table};
use pin_app::ui::{
    LOAD_NOT_WIRED, LoadRefusal, MenuController, MenuInput, MenuState, TABLES_DIR_NAME, TableEntry,
    TableLoadState, browser_dir, resolve_tables_dir,
};
use pin_audio::mixer::MixerConfig;
use pin_core::{FrameStateSnapshot, SimStep};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// `<tmp>/pin_app` (a fake executable) and `<tmp>/tables` holding two valid
/// bundles: `a_greybox` and `b_apex` (folders). Built once per test binary.
fn exe_layout() -> &'static (PathBuf, PathBuf) {
    static L: OnceLock<(PathBuf, PathBuf)> = OnceLock::new();
    L.get_or_init(|| {
        let root = fixtures::temp_dir("kp005_layout");
        let exe = root.join("pin_app");
        std::fs::write(&exe, b"").unwrap();
        let tables = root.join(TABLES_DIR_NAME);
        greybox::build_greybox_bundle(&tables.join("a_greybox"));
        apex::build_apex_bundle(&tables.join("b_apex"));
        (exe, tables)
    })
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| (*s).to_owned()).collect()
}

fn scan(w: &IoWorker, dir: &Path) -> Vec<TableEntry> {
    w.request(IoRequest::Scan(dir.to_path_buf()));
    match w.recv() {
        Some(IoReply::Scanned { root, tables }) => {
            assert_eq!(root, dir);
            tables
        }
        other => panic!("expected Scanned, got {other:?}"),
    }
}

fn load(w: &IoWorker, id: u64, path: &Path) -> Result<Box<LoadedApp>, String> {
    w.request(IoRequest::Load {
        id,
        path: path.to_path_buf(),
    });
    match w.recv() {
        Some(IoReply::Loaded {
            id: got,
            path: p,
            app,
        }) => {
            assert_eq!((got, p.as_path()), (id, path));
            Ok(app)
        }
        Some(IoReply::LoadFailed {
            id: got,
            path: p,
            error,
        }) => {
            assert_eq!((got, p.as_path()), (id, path));
            Err(error)
        }
        other => panic!("expected a load reply, got {other:?}"),
    }
}

/// Ticks `app`'s simulation `n` times on a big-stack thread (simulated time,
/// empty input); returns the tick reached and the state hash.
fn tick(app: Box<LoadedApp>, n: u64) -> (u64, u64) {
    greybox::on_big_stack(move || {
        let mut app = app;
        let mut out = FrameStateSnapshot::default();
        // The sim loop numbers ticks from 1 (tick 0 is the loaded state).
        for t in 1..=n {
            app.sim.step_1ms(t, &[], &mut out);
        }
        (app.sim.sim().tick(), app.sim.sim().state_hash())
    })
}

/// REQ-VIEW-006 acceptance: with `<exe dir>/tables` holding two bundles the
/// worker's scan lists both (sorted, both ready) and a load of the chosen
/// one gives a simulation that ticks from 0 and is a different table than
/// the first; the swapped-in table starts like a fresh `--table` run (R3).
#[test]
fn req_view_006_scan_lists_both_and_load_runs_the_chosen_one() {
    let (exe, tables_dir) = exe_layout();
    let dir = browser_dir(&[], None, exe).expect("no --table: browser on");
    assert_eq!(&dir, tables_dir);
    let w = IoWorker::spawn(MixerConfig::default());
    let tables = scan(&w, &dir);
    let names: Vec<&str> = tables.iter().map(|t| t.file_name.as_str()).collect();
    assert_eq!(names, ["a_greybox", "b_apex"]);
    assert!(tables.iter().all(TableEntry::is_ready), "{tables:?}");
    assert_ne!(tables[0].title(), tables[1].title());

    let first = load(&w, 1, &tables[0].path).unwrap_or_else(|e| panic!("{e}"));
    let second = load(&w, 2, &tables[1].path).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(
        second.sim.sim().tick(),
        0,
        "a loaded table starts at tick 0"
    );
    assert_ne!(
        (first.visuals.meshes.len(), first.visuals.bounds),
        (second.visuals.meshes.len(), second.visuals.bounds),
        "the second table's visuals differ from the first"
    );
    let (t1, _) = tick(first, 3);
    assert_eq!(t1, 3);
    let (t2, hash) = tick(second, 25);
    assert_eq!(t2, 25);
    let (last, fresh) = run_headless_table(&tables[1].path, 25).unwrap();
    assert_eq!(last, 25);
    assert_eq!(hash, fresh, "same state as a fresh --table start (R3)");
}

/// REQ-VIEW-006: a broken bundle and a missing path fail with an error
/// text; the worker keeps serving the following scan and load.
#[test]
fn req_view_006_failed_loads_keep_the_worker_serving() {
    let (_, tables_dir) = exe_layout();
    let root = fixtures::temp_dir("kp005_broken");
    let bad = root.join("broken");
    let files = fixtures::edit_table(&fixtures::bundle_files(), "[meta]", "[meta\n");
    fixtures::write_dir(&bad, &files);
    let w = IoWorker::spawn(MixerConfig::default());
    let e = load(&w, 7, &bad).err().expect("broken bundle fails");
    assert!(!e.is_empty());
    let e = load(&w, 8, &root.join("does_not_exist"))
        .err()
        .expect("missing");
    assert!(!e.is_empty());
    let listed = scan(&w, &root);
    assert_eq!(listed.len(), 1);
    assert!(!listed[0].is_ready());
    let ok = load(&w, 9, &tables_dir.join("a_greybox")).unwrap_or_else(|e| panic!("{e}"));
    assert_eq!(ok.sim.sim().tick(), 0);
}

/// REQ-VIEW-006: replies come back in request order; dropping the worker
/// with requests still queued joins it (no hang).
#[test]
fn req_view_006_replies_in_order_and_drop_joins() {
    let (_, tables_dir) = exe_layout();
    let root = fixtures::temp_dir("kp005_order");
    let w = IoWorker::spawn(MixerConfig::default());
    w.request(IoRequest::Load {
        id: 1,
        path: root.join("x"),
    });
    w.request(IoRequest::Scan(tables_dir.clone()));
    w.request(IoRequest::Load {
        id: 2,
        path: root.join("y"),
    });
    w.request(IoRequest::Scan(root.clone()));
    assert!(matches!(w.recv(), Some(IoReply::LoadFailed { id: 1, .. })));
    assert!(matches!(w.recv(), Some(IoReply::Scanned { tables, .. }) if tables.len() == 2));
    assert!(matches!(w.recv(), Some(IoReply::LoadFailed { id: 2, .. })));
    assert!(matches!(w.recv(), Some(IoReply::Scanned { tables, .. }) if tables.is_empty()));
    for id in 0..32 {
        w.request(IoRequest::Load {
            id,
            path: tables_dir.join("a_greybox"),
        });
    }
    drop(w);
}

/// REQ-VIEW-006: `--table` (any form) disables the browser; otherwise the
/// folder is `<exe dir>/tables`, a configured `tables_dir` wins, and a bare
/// executable name gives `./tables`.
#[test]
fn req_view_006_browser_dir_resolution() {
    let exe = Path::new("/srv/kp/pin_app");
    assert_eq!(browser_dir(&args(&["--table", "x"]), None, exe), None);
    assert_eq!(browser_dir(&args(&["--table=x"]), None, exe), None);
    assert_eq!(
        browser_dir(&args(&["--headless", "--run-ms", "9"]), None, exe),
        Some(PathBuf::from("/srv/kp/tables"))
    );
    assert_eq!(
        browser_dir(&[], Some(Path::new("/mine")), exe),
        Some(PathBuf::from("/mine"))
    );
    assert_eq!(
        resolve_tables_dir(None, Path::new("pin_app")),
        PathBuf::from("./tables")
    );
}

/// A controller on a fresh `config.toml` in `dir`, menu open, with `tables`.
fn controller(dir: &Path, tables: Vec<TableEntry>, browser: bool) -> MenuController {
    let (mut c, _) = MenuController::load(dir.join("config.toml"));
    c.set_browser_enabled(browser);
    c.toggle(1, &mut |_| {});
    c.set_tables(tables);
    c
}

/// REQ-VIEW-006: the menu hands loads and rescans to the app (enabled
/// browser): one-shot flags, `Loading` until `finish_load`, `Busy` refusal,
/// `Loaded` records `last_table` and closes, `Failed` keeps the error. With
/// the browser disabled the #84 answer (`LOAD_NOT_WIRED`) is unchanged.
#[test]
fn req_view_006_menu_hand_off() {
    let (_, tables_dir) = exe_layout();
    let w = IoWorker::spawn(MixerConfig::default());
    let tables = scan(&w, tables_dir);
    let d = fixtures::temp_dir("kp005_handoff");
    let mut c = controller(&d, tables.clone(), true);
    let mut sink = |_| {};
    c.apply(MenuInput::Rescan, 2, &mut sink);
    assert!(c.take_rescan_request());
    assert!(!c.take_rescan_request());
    c.apply(MenuInput::LoadTable(1), 3, &mut sink);
    let (id, path) = c.take_load_request().expect("load handed over");
    assert_eq!(path, tables[1].path);
    assert_eq!(Some(id), c.menu().pending_load_id());
    assert!(c.take_load_request().is_none());
    assert_eq!(
        c.menu().load_state(),
        &TableLoadState::Loading(path.clone())
    );
    // A second request while loading is refused, not queued.
    let mut probe: MenuState = c.menu().clone();
    assert_eq!(
        probe.request_load_index(0),
        Err(LoadRefusal::Busy(path.clone()))
    );
    c.apply(MenuInput::LoadTable(0), 4, &mut sink);
    assert!(c.take_load_request().is_none());
    assert_eq!(
        c.menu().load_state(),
        &TableLoadState::Loading(path.clone())
    );
    c.finish_load(id, Err("clearance check failed".into()));
    assert!(matches!(
        c.menu().load_state(),
        TableLoadState::Failed { error, .. } if error == "clearance check failed"
    ));
    assert!(c.is_open());
    c.apply(MenuInput::LoadTable(1), 5, &mut sink);
    let (id, path) = c.take_load_request().unwrap();
    c.finish_load(id, Ok(()));
    assert_eq!(c.menu().load_state(), &TableLoadState::Loaded(path.clone()));
    assert!(!c.is_open());
    assert_eq!(c.config().last_table.as_deref(), Some(path.as_path()));

    // Disabled: the #84 behaviour.
    let d2 = fixtures::temp_dir("kp005_handoff_off");
    let mut c = controller(&d2, tables, false);
    c.apply(MenuInput::LoadTable(0), 2, &mut sink);
    assert!(c.take_load_request().is_none());
    assert!(matches!(
        c.menu().load_state(),
        TableLoadState::Failed { error, .. } if error == LOAD_NOT_WIRED
    ));
    c.apply(MenuInput::Rescan, 3, &mut sink);
    assert!(!c.take_rescan_request());
}

/// REQ-VIEW-006 full headless flow: worker scan -> menu list -> pick table 2
/// -> worker load -> `finish_load(Ok)`; `last_table` is the chosen bundle
/// and `config.toml` round-trips it.
#[test]
fn req_view_006_full_flow_saves_last_table() {
    let (exe, _) = exe_layout();
    let d = fixtures::temp_dir("kp005_flow");
    let cfg = d.join("config.toml");
    let (mut c, _) = MenuController::load(&cfg);
    let dir = browser_dir(&[], c.config().tables_dir.as_deref(), exe).unwrap();
    c.set_browser_enabled(true);
    c.toggle(1, &mut |_| {});
    let w = IoWorker::spawn(MixerConfig::default());
    c.set_tables(scan(&w, &dir));
    assert_eq!(c.menu().tables().len(), 2);
    let mut sink = |_| {};
    // Keyboard-style navigation: focus the second row and select it.
    c.apply(MenuInput::Down, 2, &mut sink);
    c.apply(MenuInput::Down, 3, &mut sink);
    c.apply(MenuInput::Select, 4, &mut sink);
    let (id, path) = c.take_load_request().expect("selected");
    assert_eq!(path, c.menu().tables()[1].path);
    let app = load(&w, id, &path).unwrap_or_else(|e| panic!("{e}"));
    let (t, _) = tick(app, 2);
    assert_eq!(t, 2);
    c.finish_load(id, Ok(()));
    assert_eq!(c.config().last_table.as_deref(), Some(path.as_path()));
    let change = c.take_config_change().expect("last_table change");
    assert_eq!(change.last_table.as_deref(), Some(path.as_path()));
    assert!(c.last_save_error().is_none());
    let (reloaded, warnings) = MenuController::load(&cfg);
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(
        reloaded.config().last_table.as_deref(),
        Some(path.as_path())
    );
}

/// R11: bounded random names and contents through the worker's scan and
/// load: no panic, no hang, one reply per request.
#[test]
fn req_view_006_worker_survives_random_bundles() {
    let root = fixtures::temp_dir("kp005_fuzz");
    // A small deterministic LCG (test data only).
    let mut s: u64 = 0x5eed_0085;
    let mut next = move || {
        s = s
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (s >> 33) as u32
    };
    let exts = ["zip", "pinbundle", "", "toml"];
    let mut paths = Vec::new();
    for i in 0..200 {
        let len = (next() % 512) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| next() as u8).collect();
        let ext = exts[(next() % 4) as usize];
        let name = if ext.is_empty() {
            format!("r{i}")
        } else {
            format!("r{i}.{ext}")
        };
        let p = root.join(&name);
        if next() % 3 == 0 {
            std::fs::create_dir_all(&p).unwrap();
            std::fs::write(p.join("table.toml"), &bytes).unwrap();
        } else {
            std::fs::write(&p, &bytes).unwrap();
        }
        paths.push(p);
    }
    let w = IoWorker::spawn(MixerConfig::default());
    let listed = scan(&w, &root);
    assert!(listed.iter().all(|t| !t.is_ready()));
    for (i, p) in paths.iter().enumerate().step_by(4) {
        let e = load(&w, i as u64, p)
            .err()
            .expect("random bytes never load");
        assert!(!e.is_empty());
    }
}
