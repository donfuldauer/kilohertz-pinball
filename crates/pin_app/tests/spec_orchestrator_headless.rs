//! Independent spec test for `pin_app --headless-ticks N` (Subphase 1.4 deliverable 4,
//! verification criterion 1): runs the built binary, with every display variable removed,
//! and checks the exit status and the summary line.

use std::process::{Command, Output};

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pin_app"))
        .args(args)
        // No SDL, no GPU, no display server is needed for the headless path.
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("XDG_RUNTIME_DIR")
        .env_remove("SDL_VIDEO_DRIVER")
        .env_remove("SDL_VIDEODRIVER")
        .env_remove("WGPU_BACKEND")
        .output()
        .expect("pin_app binary runs")
}

fn summary_lines(out: &Output) -> Vec<String> {
    String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(|l| l.trim_end().to_owned())
        .filter(|l| l.starts_with("headless:"))
        .collect()
}

fn check(n: u64) {
    let out = run(&["--headless-ticks", &n.to_string()]);
    assert!(
        out.status.success(),
        "--headless-ticks {n} exited with {:?}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(out.status.code(), Some(0));
    assert_eq!(
        summary_lines(&out),
        vec![format!("headless: {n} ticks, last tick {n}")],
        "exactly one summary line on stdout"
    );
}

#[test]
fn headless_10000_ticks_completes_with_monotonic_snapshots() {
    check(10_000);
}

#[test]
fn headless_small_counts() {
    check(1);
    check(7);
    check(8);
    check(9);
}

/// 200 s of simulated time. On simulated time this is quick; a build that slept in real
/// time would hang here (no wall-clock assertion, D8).
#[test]
fn headless_long_run_uses_simulated_time() {
    check(200_000);
}

#[test]
fn headless_runs_are_repeatable() {
    let a = run(&["--headless-ticks", "2500"]);
    let b = run(&["--headless-ticks", "2500"]);
    assert!(a.status.success() && b.status.success());
    assert_eq!(a.stdout, b.stdout);
}
