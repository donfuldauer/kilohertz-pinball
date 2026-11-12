//! Verification criterion 1: `pin_app --headless-ticks 10000` completes on
//! simulated time with monotonic snapshots (no SDL, no GPU, no sleeping).

use std::process::Command;

fn pin_app(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_pin_app"))
        .args(args)
        .output()
        .expect("run pin_app")
}

#[test]
fn headless_ticks_10000() {
    let out = pin_app(&["--headless-ticks", "10000"]);
    assert!(out.status.success(), "{out:?}");
    let stdout = String::from_utf8(out.stdout).unwrap();
    assert_eq!(stdout, "headless: 10000 ticks, last tick 10000\n");
}

#[test]
fn headless_ticks_zero_and_one() {
    let out = pin_app(&["--headless-ticks", "0"]);
    assert!(out.status.success());
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "headless: 0 ticks, last tick 0\n"
    );
    let out = pin_app(&["--headless-ticks", "1"]);
    assert_eq!(
        String::from_utf8_lossy(&out.stdout),
        "headless: 1 ticks, last tick 1\n"
    );
}

#[test]
fn headless_ticks_bad_argument_fails() {
    for args in [
        &["--headless-ticks"][..],
        &["--headless-ticks", "abc"],
        &["--headless-ticks", "-1"],
    ] {
        let out = pin_app(args);
        assert!(!out.status.success(), "{args:?}");
        assert!(out.stdout.is_empty());
        assert!(!out.stderr.is_empty());
    }
}
