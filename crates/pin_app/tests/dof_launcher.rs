//! Subphase 3.5: the bridge launcher (`pin_app::dof::DofBridge`).
//!
//! Process tests use a stand-in bridge (a `/bin/sh` script) and run on Unix;
//! the real bridge is tested by the .NET suite (`tools/dof_bridge/tests`).

use pin_app::dof::{
    BRIDGE_EXE_NAME, DEFAULT_STOP_GRACE, DofBridge, DofError, DofLaunchConfig, ResolvedDofMap,
    platform_supported,
};
use pin_core::schema::table::{DofElementType, TableManifest};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

const TABLE_TOML: &str = include_str!("../../../tools/dof_bridge/tests/fixtures/table.toml");

fn fixture_table() -> TableManifest {
    TableManifest::from_toml_str(TABLE_TOML).unwrap()
}

fn map() -> ResolvedDofMap {
    ResolvedDofMap::resolve(&fixture_table()).unwrap().unwrap()
}

fn scratch(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("khz-dof-launcher-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn config_defaults_and_args() {
    let c = DofLaunchConfig::new("bridge.exe", 35120);
    assert_eq!(c.stop_grace, DEFAULT_STOP_GRACE);
    assert_eq!(c.map_dir, std::env::temp_dir());
    assert!(c.dof_path.is_none() && c.log_file.is_none() && c.extra_args.is_empty());
    let m = map();
    let args = c.args(&m, Path::new("m.json"));
    let pid = std::process::id().to_string();
    let want: Vec<OsString> = [
        "--map",
        "m.json",
        "--port",
        "35120",
        "--rom",
        "dof_fixture_rom",
        "--table",
        "DOF \"Fixture\" Table",
        "--parent-pid",
        pid.as_str(),
        "--exit-on-stdin-eof",
    ]
    .into_iter()
    .map(OsString::from)
    .collect();
    assert_eq!(args, want);

    let mut c = c;
    c.dof_path = Some("C:/DirectOutput".into());
    c.extra_args = vec!["--sink".into(), "log".into()];
    let args = c.args(&m, Path::new("m.json"));
    assert_eq!(
        &args[11..],
        &["--dof-path", "C:/DirectOutput", "--sink", "log"].map(OsString::from)
    );
}

#[test]
fn bridge_exe_lookup() {
    let d = scratch("lookup");
    assert_eq!(DofLaunchConfig::find_bridge_exe(&d), None);
    std::fs::write(d.join(BRIDGE_EXE_NAME), b"").unwrap();
    assert_eq!(
        DofLaunchConfig::find_bridge_exe(&d),
        Some(d.join(BRIDGE_EXE_NAME))
    );
    std::fs::create_dir_all(d.join("dof_bridge")).unwrap();
    std::fs::write(d.join("dof_bridge").join(BRIDGE_EXE_NAME), b"").unwrap();
    assert_eq!(
        DofLaunchConfig::find_bridge_exe(&d),
        Some(d.join("dof_bridge").join(BRIDGE_EXE_NAME))
    );
    // Next to the test binary there is no bridge.
    assert_eq!(DofLaunchConfig::default_bridge_exe(), None);
    std::fs::remove_dir_all(d).unwrap();
}

#[test]
fn start_for_table_is_a_no_op_off_windows_or_without_dof() {
    let cfg = DofLaunchConfig::new("/nonexistent/KilohertzDofBridge", 35120);
    let mut no_dof = fixture_table();
    no_dof.dof = None;
    assert!(DofBridge::start_for_table(&no_dof, &cfg).unwrap().is_none());
    let r = DofBridge::start_for_table(&fixture_table(), &cfg);
    if platform_supported() {
        assert!(matches!(r, Err(DofError::Spawn(_))));
    } else {
        assert!(r.unwrap().is_none());
    }
    assert_eq!(platform_supported(), cfg!(windows));
}

#[test]
fn spawn_failure_removes_the_map_file() {
    let d = scratch("spawnfail");
    let mut cfg = DofLaunchConfig::new(d.join("missing-bridge"), 1);
    cfg.map_dir = d.clone();
    assert!(matches!(
        DofBridge::spawn(&map(), &cfg),
        Err(DofError::Spawn(_))
    ));
    cfg.log_file = Some(d.join("no-such-dir").join("log.txt"));
    assert!(matches!(
        DofBridge::spawn(&map(), &cfg),
        Err(DofError::Spawn(_))
    ));
    assert_eq!(std::fs::read_dir(&d).unwrap().count(), 0, "map files left");
    // A map dir that cannot be written.
    cfg.map_dir = d.join("missing");
    assert!(matches!(
        DofBridge::spawn(&map(), &cfg),
        Err(DofError::MapFile(_))
    ));
    std::fs::remove_dir_all(d).unwrap();
}

#[cfg(unix)]
mod unix {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::time::Duration;

    /// A stand-in bridge: records its args and the map file, then behaves per `body`.
    fn fake_bridge(dir: &Path, body: &str) -> PathBuf {
        let p = dir.join("fake-bridge.sh");
        std::fs::write(
            &p,
            format!(
                "#!/bin/sh\nfor a in \"$@\"; do printf '%s\\n' \"$a\"; done > \"$2.args\"\ncp \"$2\" \"$2.copy\"\necho started >&2\n{body}\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        p
    }

    #[test]
    fn spawn_passes_args_and_map_and_stops_on_stdin_eof() {
        let d = scratch("eof");
        let exe = fake_bridge(&d, "cat > /dev/null\nexit 0");
        let mut cfg = DofLaunchConfig::new(&exe, 40001);
        cfg.map_dir = d.clone();
        cfg.log_file = Some(d.join("bridge.log"));
        let m = map();
        let mut bridge = DofBridge::spawn(&m, &cfg).unwrap();
        assert!(bridge.id() > 0);
        let map_file = bridge.map_file().to_path_buf();
        assert!(map_file.starts_with(&d));
        assert!(map_file.exists());
        assert!(bridge.try_wait().unwrap().is_none());
        let handle = bridge.stop();
        let status = handle.wait().expect("exit status");
        assert!(status.success(), "{status:?}");
        assert!(!map_file.exists(), "map file deleted after stop");
        let args = std::fs::read_to_string(map_file.with_extension("json.args")).unwrap();
        let want: Vec<String> = cfg
            .args(&m, &map_file)
            .into_iter()
            .map(|a| a.into_string().unwrap())
            .collect();
        assert_eq!(args.lines().collect::<Vec<_>>(), want);
        let copy = std::fs::read_to_string(map_file.with_extension("json.copy")).unwrap();
        assert_eq!(copy, m.to_json());
        assert_eq!(
            std::fs::read_to_string(d.join("bridge.log")).unwrap(),
            "started\n"
        );
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn a_bridge_that_ignores_eof_is_killed_after_the_grace_period() {
        let d = scratch("kill");
        let exe = fake_bridge(&d, "trap '' TERM\nexec sleep 600");
        let mut cfg = DofLaunchConfig::new(&exe, 40002);
        cfg.map_dir = d.clone();
        cfg.stop_grace = Duration::from_millis(50);
        let bridge = DofBridge::spawn(&map(), &cfg).unwrap();
        let map_file = bridge.map_file().to_path_buf();
        let handle = bridge.stop();
        let status = handle.wait().expect("exit status");
        assert!(!status.success());
        assert!(!map_file.exists());
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn drop_stops_the_bridge_and_early_exit_is_seen() {
        let d = scratch("drop");
        // Exits by itself with the "DOF not found" code.
        let exe = fake_bridge(&d, "exit 4");
        let mut cfg = DofLaunchConfig::new(&exe, 40003);
        cfg.map_dir = d.clone();
        let mut bridge = DofBridge::spawn(&map(), &cfg).unwrap();
        let status = loop {
            if let Some(s) = bridge.try_wait().unwrap() {
                break s;
            }
            std::thread::yield_now();
        };
        assert_eq!(status.code(), Some(4));
        let map_file = bridge.map_file().to_path_buf();
        drop(bridge);
        // Dropped: the reaper deletes the map file; a fresh stop of a
        // never-started handle is a no-op.
        let mut waited = 0u32;
        while map_file.exists() && waited < 1_000_000 {
            std::thread::yield_now();
            waited += 1;
        }
        assert!(!map_file.exists());
        std::fs::remove_dir_all(d).unwrap();
    }

    #[test]
    fn stop_handle_reports_finish() {
        let d = scratch("handle");
        let exe = fake_bridge(&d, "cat > /dev/null");
        let mut cfg = DofLaunchConfig::new(&exe, 40004);
        cfg.map_dir = d.clone();
        let h = DofBridge::spawn(&map(), &cfg).unwrap().stop();
        let _ = h.is_finished();
        assert!(h.wait().is_some());
        std::fs::remove_dir_all(d).unwrap();
    }
}

#[test]
fn element_type_round_trips_into_json() {
    let json = map().to_json();
    for t in ["\"S\"", "\"W\"", "\"L\"", "\"G\"", "\"E\""] {
        assert!(json.contains(t), "{t}");
    }
    let _ = DofElementType::E;
}
