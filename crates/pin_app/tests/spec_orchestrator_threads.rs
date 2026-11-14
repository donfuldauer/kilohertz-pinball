//! Independent spec test for `pin_app::orchestrator::{spawn_threads, Threads}` (Subphase
//! 1.4 deliverable 2): with `IdleSim` and no GPU, the sim, render, audio and I/O threads
//! start, the render thread tolerates "no surfaces", and `Threads::shutdown` joins all of
//! them promptly.
//!
//! The brief leaves `spawn_threads(...)`'s arguments free-form; this file uses the public
//! `ThreadsConfig::idle(input)` constructor the implementation exports (signature taken
//! from the public API after it compiled, not from the brief).
//!
//! The brief's bound is "joins every thread within 1 s of the request". This is the one
//! real-time path, so the test measures and prints the join time and asserts it against
//! the 1 s bound; a hang is caught by a watchdog instead of blocking the test run.

use pin_app::orchestrator::{ThreadsConfig, spawn_threads};
use pin_core::spsc;
use pin_core::{CabinetInput, InputEvent};
use std::sync::mpsc;
use std::time::{Duration, Instant};

const JOIN_BOUND: Duration = Duration::from_secs(1);
const WATCHDOG: Duration = Duration::from_secs(20);

/// Runs `f` on a helper thread; panics if it does not finish before the watchdog.
fn with_watchdog<T: Send + 'static>(what: &str, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    rx.recv_timeout(WATCHDOG)
        .unwrap_or_else(|_| panic!("{what} did not finish within {WATCHDOG:?} (hang)"))
}

#[test]
fn four_threads_start_and_shutdown_joins_them_all() {
    let (_producer, consumer) = spsc::channel::<InputEvent>(64);
    let threads = spawn_threads(ThreadsConfig::idle(consumer)).expect("threads spawn");
    let names = threads.names();
    assert_eq!(
        names.len(),
        4,
        "sim, render, audio and io threads: {names:?}"
    );
    for want in ["sim", "render", "audio", "io"] {
        assert!(
            names.iter().any(|n| n.to_lowercase().contains(want)),
            "no {want} thread among {names:?}"
        );
    }
    let elapsed = with_watchdog("Threads::shutdown", move || {
        let t0 = Instant::now();
        let result = threads.shutdown();
        (t0.elapsed(), result.map(|_| ()))
    });
    println!("immediate shutdown joined in {:?}", elapsed.0);
    elapsed.1.expect("no thread panicked");
    assert!(
        elapsed.0 <= JOIN_BOUND,
        "shutdown took {:?}, bound is 1 s",
        elapsed.0
    );
}

#[test]
fn sim_thread_ticks_in_real_time_and_render_follows_without_surfaces() {
    let (mut producer, consumer) = spsc::channel::<InputEvent>(64);
    let threads = spawn_threads(ThreadsConfig::idle(consumer)).expect("threads spawn");
    producer
        .push(InputEvent::button(0, CabinetInput::Start, true))
        .unwrap();

    let (threads, seen) = with_watchdog("waiting for sim ticks", move || {
        // Poll (no fixed sleep) until the sim has run and the render thread has read a
        // snapshot through the triple buffer.
        loop {
            let s = threads.stats();
            if s.sim_ticks >= 50 && s.render_last_tick >= 1 {
                return (threads, s);
            }
            std::thread::yield_now();
        }
    });
    assert!(
        seen.render_iterations >= 1,
        "render thread runs with no surfaces"
    );
    assert_eq!(
        seen.render_frames, 0,
        "nothing can be presented without a surface"
    );

    let (elapsed, result) = with_watchdog("Threads::shutdown", move || {
        let t0 = Instant::now();
        let result = threads.shutdown();
        (t0.elapsed(), result)
    });
    println!("shutdown after running joined in {elapsed:?}");
    let last = result.expect("no thread panicked");
    assert!(
        elapsed <= JOIN_BOUND,
        "shutdown took {elapsed:?}, bound is 1 s"
    );
    assert!(last.sim_ticks >= 50);
    assert!(last.render_last_tick <= last.sim_ticks);
    assert_eq!(
        last.render_non_monotonic, 0,
        "the render thread must never see snapshot ticks go backwards"
    );
    assert!(!last.close_requested);
}

#[test]
fn spawn_and_shutdown_repeatedly() {
    for round in 0..5 {
        let (_producer, consumer) = spsc::channel::<InputEvent>(8);
        let threads = spawn_threads(ThreadsConfig::idle(consumer)).expect("threads spawn");
        let (elapsed, ok) = with_watchdog("Threads::shutdown", move || {
            let t0 = Instant::now();
            let ok = threads.shutdown().is_ok();
            (t0.elapsed(), ok)
        });
        assert!(ok, "round {round}: a thread panicked");
        assert!(
            elapsed <= JOIN_BOUND,
            "round {round}: shutdown took {elapsed:?}"
        );
    }
}
