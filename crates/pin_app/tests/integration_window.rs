//! 6.0 C6: the app paths without a GPU. Runs the real binary:
//! `pin_app --headless --table <greybox bundle> --run-ticks M` (SDL dummy
//! drivers, no surfaces) wires the table simulation, input router, audio
//! and render threads and quits once the sim computed M ticks; and
//! `--headless-ticks N --table` (simulated time). The windowed run is in
//! `integration_window_gpu.rs` (feature `gpu-tests`).
//!
//! Exit conditions are progress counters, never wall time (D8): no test here
//! passes `--run-ms` or asserts on elapsed time; a hung run is bounded by the
//! CI job timeout (ci.yml `timeout-minutes: 45`). The cap logic is covered
//! on supplied durations by `RunLimits::check` (`run_limits_cap.rs`).

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

#[test]
fn headless_ticks_with_table_runs_the_real_sim() {
    let bundle = greybox_bundle().to_str().unwrap().to_owned();
    let (ok, out, err) = run(&["--headless-ticks", "300", "--table", &bundle]);
    assert!(ok, "stdout {out}\nstderr {err}");
    assert!(
        out.starts_with("headless: 300 ticks, last tick 300, state hash "),
        "{out}"
    );
    // Deterministic: the same hash twice.
    let (_, out2, _) = run(&["--headless-ticks", "300", "--table", &bundle]);
    assert_eq!(out, out2);
    // Without --table: the idle sim, output unchanged since 1.4.
    let (ok, out, _) = run(&["--headless-ticks", "5"]);
    assert!(ok);
    assert_eq!(out.trim(), "headless: 5 ticks, last tick 5");
    // Errors.
    let (ok, _, err) = run(&["--headless-ticks", "5", "--table"]);
    assert!(!ok && err.contains("--table needs a path"), "{err}");
    let (ok, _, err) = run(&["--headless-ticks", "5", "--table", "/nonexistent/table"]);
    assert!(!ok && err.contains("/nonexistent/table"), "{err}");
}

#[test]
fn headless_app_runs_the_table_until_the_tick_condition() {
    let bundle = greybox_bundle().to_str().unwrap().to_owned();
    let (ok, out, err) = run(&["--headless", "--table", &bundle, "--run-ticks", "600"]);
    assert!(ok, "stdout {out}\nstderr {err}");
    assert!(err.contains("table \"Grey Box\" loaded"), "{err}");
    assert!(err.contains("pin_app: run condition met"), "{err}");
    let ticks = count_before(&err, "sim ticks").expect("shutdown line");
    assert!(ticks >= 600, "{err}");
    assert!(err.contains("pin_app: render: "), "{err}");
}

/// `--run-frames` without rendering (dummy drivers) can never hold: the
/// app says so and fails at once.
#[test]
fn frames_condition_without_rendering_fails() {
    let (ok, _, err) = run(&["--headless", "--run-frames", "1"]);
    assert!(!ok, "{err}");
    assert!(err.contains("run condition unreachable"), "{err}");
    assert!(!err.contains("run condition met"), "{err}");
}
