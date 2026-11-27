//! Issue #258 (KP-120, SPEC §4 Subphase 7.2, REQ-VIEW-006): the default tables
//! folder follows the macOS packaging move. Inside a `.app` bundle it is
//! `<Contents>/Resources/tables`; everywhere else `<exe dir>/tables`; a
//! configured folder always wins. Pure path logic: every path below is
//! deliberately non-existent and no test touches the filesystem.

use pin_app::ui::{TABLES_DIR_NAME, browser_dir, resolve_tables_dir};
use std::path::{Path, PathBuf};

fn resolve(exe: &str) -> PathBuf {
    resolve_tables_dir(None, Path::new(exe))
}

fn check_table(cases: &[(&str, &str)]) {
    for (exe, want) in cases {
        assert_eq!(resolve(exe), PathBuf::from(want), "case exe={exe}");
    }
}

#[test]
fn req_view_006_kp120_bundle_exe_resolves_to_resources_tables() {
    check_table(&[
        (
            "/x/K.app/Contents/MacOS/kilohertz-pinball",
            "/x/K.app/Contents/Resources/tables",
        ),
        (
            "/Applications/Kilohertz Pinball.app/Contents/MacOS/kilohertz-pinball",
            "/Applications/Kilohertz Pinball.app/Contents/Resources/tables",
        ),
        (
            "K.app/Contents/MacOS/kilohertz-pinball",
            "K.app/Contents/Resources/tables",
        ),
    ]);
}

#[test]
fn spec_4_7_2_kp120_non_bundle_shapes_use_exe_dir_tables() {
    check_table(&[
        ("/x/MacOS/kilohertz-pinball", "/x/MacOS/tables"),
        ("/x/Other/MacOS/kilohertz-pinball", "/x/Other/MacOS/tables"),
        ("/MacOS/kilohertz-pinball", "/MacOS/tables"),
        ("/x/Contents/kilohertz-pinball", "/x/Contents/tables"),
        (
            "/x/Contents/Other/kilohertz-pinball",
            "/x/Contents/Other/tables",
        ),
        (
            "/x/K.app/Contents/macos/kilohertz-pinball",
            "/x/K.app/Contents/macos/tables",
        ),
        (
            "/x/K.app/contents/MacOS/kilohertz-pinball",
            "/x/K.app/contents/MacOS/tables",
        ),
    ]);
}

#[test]
fn spec_4_7_2_kp120_linux_and_windows_unchanged() {
    check_table(&[
        ("/opt/kilohertz/kilohertz-pinball", "/opt/kilohertz/tables"),
        ("C:/Games/K/kilohertz-pinball.exe", "C:/Games/K/tables"),
        ("kilohertz-pinball", "./tables"),
    ]);
}

#[cfg(windows)]
#[test]
fn spec_4_7_2_kp120_windows_backslash_path_unchanged() {
    assert_eq!(
        resolve(r"C:\Games\K\kilohertz-pinball.exe"),
        PathBuf::from(r"C:\Games\K\tables")
    );
}

#[test]
fn spec_4_7_2_kp120_root_and_empty_exe_not_under_resources() {
    for exe in ["/", ""] {
        let got = resolve(exe);
        assert!(got.ends_with("tables"), "case exe={exe:?}: {got:?}");
        assert!(
            !got.components().any(|c| c.as_os_str() == "Resources"),
            "case exe={exe:?}: {got:?}"
        );
    }
}

#[test]
fn req_view_006_kp120_configured_folder_wins() {
    let cfg = Path::new("/data/t");
    for exe in [
        "/x/K.app/Contents/MacOS/kilohertz-pinball",
        "/opt/kilohertz/kilohertz-pinball",
        "/x/MacOS/kilohertz-pinball",
        "kilohertz-pinball",
    ] {
        assert_eq!(
            resolve_tables_dir(Some(cfg), Path::new(exe)),
            PathBuf::from("/data/t"),
            "case exe={exe}"
        );
    }
}

#[test]
fn spec_4_7_2_kp120_results_independent_of_disk() {
    // None of these exist; two calls agree and match the rule.
    let exe = "/nonexistent-kp120/K.app/Contents/MacOS/kilohertz-pinball";
    assert_eq!(resolve(exe), resolve(exe));
    assert_eq!(
        resolve(exe),
        PathBuf::from("/nonexistent-kp120/K.app/Contents/Resources/tables")
    );
    assert!(!Path::new("/nonexistent-kp120").exists());
}

fn args(a: &[&str]) -> Vec<String> {
    a.iter().map(|s| s.to_string()).collect()
}

#[test]
fn req_view_006_kp120_browser_dir_follows_bundle_rule() {
    let bundle = Path::new("/x/K.app/Contents/MacOS/kilohertz-pinball");
    assert_eq!(
        browser_dir(&[], None, bundle),
        Some(PathBuf::from("/x/K.app/Contents/Resources/tables"))
    );
    assert_eq!(browser_dir(&args(&["--table", "x"]), None, bundle), None);
    assert_eq!(browser_dir(&args(&["--table=x"]), None, bundle), None);
    assert_eq!(
        browser_dir(&[], Some(Path::new("/data/t")), bundle),
        Some(PathBuf::from("/data/t"))
    );
    assert_eq!(
        browser_dir(&[], None, Path::new("/x/MacOS/kilohertz-pinball")),
        Some(PathBuf::from("/x/MacOS/tables"))
    );
}

#[test]
fn req_view_006_kp120_browser_dir_matches_resolve_for_many_inputs() {
    let exes = [
        "/x/K.app/Contents/MacOS/kilohertz-pinball",
        "K.app/Contents/MacOS/kilohertz-pinball",
        "/opt/kilohertz/kilohertz-pinball",
        "/x/Contents/MacOS/../kilohertz-pinball",
        "kilohertz-pinball",
        "/x/K.app/Contents/macos/kilohertz-pinball",
    ];
    let cfgs = [None, Some(Path::new("/data/t"))];
    for exe in exes {
        for cfg in cfgs {
            assert_eq!(
                browser_dir(&[], cfg, Path::new(exe)),
                Some(resolve_tables_dir(cfg, Path::new(exe))),
                "case exe={exe} cfg={cfg:?}"
            );
        }
    }
}

#[test]
fn spec_4_7_2_kp120_bundle_folder_name_and_parent() {
    assert_eq!(TABLES_DIR_NAME, "tables");
    let got = resolve("/x/K.app/Contents/MacOS/kilohertz-pinball");
    assert_eq!(got.file_name().unwrap(), TABLES_DIR_NAME);
    assert_eq!(got.parent().unwrap().file_name().unwrap(), "Resources");
}
