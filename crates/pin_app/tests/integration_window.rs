//! 6.0 C6: the windowed path. Runs the real binary,
//! `pin_app --table <greybox bundle> --run-ms N`, which opens the windows of
//! the single-monitor topology plan, wires the table simulation, input
//! router, audio thread and render thread, runs for N ms of wall time and
//! shuts down in the issue #9a order. Also `--headless --table` (SDL dummy
//! drivers, no surfaces) and `--headless-ticks N --table`.
//!
//! The windowed run needs a native SDL video driver (Xvfb + lavapipe on
//! Linux CI). Without one it prints SKIP, or fails when `PIN_REQUIRE_GPU=1`.

mod support;

use std::process::Command;
use support::greybox_bundle;

fn require_gpu() -> bool {
    std::env::var("PIN_REQUIRE_GPU").is_ok_and(|v| v.trim() == "1")
}

/// Slack (in ticks) on the `--run-ms` upper bound: pump overshoot plus the
/// join at shutdown. The bound only catches a runaway sim; progress is never
/// asserted from wall time.
const RUN_SLACK_TICKS: u64 = 2000;

/// The known "this machine cannot open a window / get a GPU" diagnostics of
/// `pin_app`, matched as whole stderr lines by prefix (never by a generic
/// substring). Returns what was unavailable.
fn unavailable(err: &str) -> Option<&'static str> {
    const KNOWN: [(&str, &str); 4] = [
        ("pin_app: SDL error: ", "SDL video unavailable"),
        (
            "pin_app: window creation failed: ",
            "SDL window creation unavailable",
        ),
        ("pin_app: SDL video driver ", "no native SDL video driver"),
        (
            "pin_app: GPU setup failed, rendering disabled: ",
            "no GPU adapter",
        ),
    ];
    err.lines().find_map(|line| {
        KNOWN.iter().find_map(|(prefix, why)| {
            let hit = line.starts_with(prefix)
                && (*prefix != "pin_app: SDL video driver "
                    || line.contains(" has no native windows"));
            hit.then_some(*why)
        })
    })
}

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
fn headless_app_runs_the_table_on_real_time() {
    let bundle = greybox_bundle().to_str().unwrap().to_owned();
    let (ok, out, err) = run(&["--headless", "--table", &bundle, "--run-ms", "600"]);
    assert!(ok, "stdout {out}\nstderr {err}");
    assert!(err.contains("table \"Grey Box\" loaded"), "{err}");
    let ticks = count_before(&err, "sim ticks").expect("shutdown line");
    // Bounded completion only (D8): the sim ran, and not past the run time.
    assert!(ticks > 0, "{err}");
    assert!(ticks <= 600 + RUN_SLACK_TICKS, "{err}");
}

#[test]
fn windowed_app_opens_windows_and_renders_the_table() {
    let bundle = greybox_bundle().to_str().unwrap().to_owned();
    let (ok, out, err) = run(&["--table", &bundle, "--run-ms", "2500"]);
    let skip = |why: &str| {
        assert!(
            !require_gpu(),
            "PIN_REQUIRE_GPU=1 but {why}\nstdout {out}\nstderr {err}"
        );
        println!("[integration_window] SKIP: {why}");
    };
    if let Some(why) = unavailable(&err) {
        return skip(why);
    }
    if !ok {
        panic!("pin_app failed\nstdout {out}\nstderr {err}");
    }
    println!("[integration_window] {err}");
    let ticks = count_before(&err, "sim ticks").expect("shutdown line");
    let frames = count_before(&err, "frames").expect("frames");
    assert!(ticks > 0, "{err}");
    assert!(ticks <= 2500 + RUN_SLACK_TICKS, "{err}");
    assert!(frames > 0, "no frame presented: {err}");
    assert!(
        !err.contains("still holds a window"),
        "a presenter kept a window at shutdown: {err}"
    );
}
