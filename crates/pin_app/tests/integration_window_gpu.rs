//! 6.0 C6, the windowed path (feature `gpu-tests`, the CI GPU job only):
//! `pin_app --table <greybox bundle> --run-frames 3 --run-ticks 100` opens the windows of the single-monitor
//! topology plan, wires the table simulation, input router, audio thread and render
//! thread, quits once 3 frames were presented and the sim computed 100
//! ticks, and shuts down in the issue #9a order.
//!
//! It needs a native SDL video driver and a GPU (Xvfb + lavapipe on Linux
//! CI). Without them the test FAILS: it never skips (PR #21 review). There
//! is no wall-clock cap (D8): a hung run is bounded by the CI job timeout
//! (ci.yml `timeout-minutes: 45`); a failure prints the render diagnostics.

mod support;

use std::process::Command;
use support::greybox_bundle;

fn run(args: &[&str]) -> (bool, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_pin_app"))
        .args(args)
        .output()
        .expect("run pin_app");
    (
        out.status.success(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

/// The number before `label` in the shutdown line
/// (`pin_app: 1234 sim ticks (0 dropped), 56 frames, ...`).
fn count_before(text: &str, label: &str) -> Option<u64> {
    let i = text.find(label)?;
    text[..i].split_whitespace().last()?.parse().ok()
}

/// The render diagnostics line (`pin_app: render: ...`), for failure
/// messages.
fn diagnostics(err: &str) -> &str {
    err.lines()
        .find(|l| l.starts_with("pin_app: render: "))
        .unwrap_or("(no render diagnostics line)")
}

#[test]
fn windowed_app_opens_windows_and_renders_the_table() {
    let bundle = greybox_bundle().to_str().unwrap().to_owned();
    let (ok, out, err) = run(&[
        "--table",
        &bundle,
        "--run-frames",
        "3",
        "--run-ticks",
        "100",
    ]);
    assert!(
        ok,
        "pin_app failed (a missing display or GPU is a failure here)\n{}\nstdout {out}\nstderr {err}",
        diagnostics(&err)
    );
    println!("[integration_window_gpu] {err}");
    assert!(err.contains("pin_app: run condition met"), "{err}");
    let ticks = count_before(&err, "sim ticks").expect("shutdown line");
    let frames = count_before(&err, "frames").expect("frames");
    assert!(ticks >= 100, "{err}");
    assert!(frames >= 3, "{}\n{err}", diagnostics(&err));
    assert!(
        err.contains("PBR table on"),
        "table renderer not used: {err}"
    );
    assert!(err.contains("pin_app: render: gpu ready after "), "{err}");
    assert!(
        !err.contains("still holds a window"),
        "a presenter kept a window at shutdown: {err}"
    );
}
