//! Gate 163 round 9 #7 (review thread 4703749482, REQ-VIEW-006): the
//! joined form `--table=<path>` is refused with one clear error and a
//! failure exit status, on the headless path and on the app path, before
//! anything is loaded (no SDL, no window, no table).

use std::process::Command;

const WANT: &str = "pin_app: --table=<path> is not supported; use --table <path>";

fn run(args: &[&str]) -> (bool, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_pin_app"))
        .args(args)
        .output()
        .expect("run pin_app");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn gate163_r9_headless_run_rejects_table_equals() {
    let (ok, stderr) = run(&["--headless-ticks", "10", "--table=demo.zip"]);
    assert!(!ok, "{stderr}");
    assert!(stderr.lines().any(|l| l == WANT), "{stderr}");
    assert!(!stderr.contains("headless:"), "{stderr}");
}

#[test]
fn gate163_r9_app_run_rejects_table_equals() {
    let (ok, stderr) = run(&["--headless", "--table=demo.zip", "--run-ms", "1"]);
    assert!(!ok, "{stderr}");
    assert!(stderr.lines().any(|l| l == WANT), "{stderr}");
    assert!(!stderr.contains("table browser"), "{stderr}");
}
