//! Thread orchestrator: spawn sim/render/audio/io with IdleSim and no GPU,
//! shut down within 1 s (the brief's bound); window events reach the render
//! thread; a panicking sim is reported. Plus the non-gating real-time drift
//! benchmark (`#[ignore]`).

use pin_app::orchestrator::{
    ClosePolicy, HybridTimer, RenderConfig, Threads, ThreadsConfig, monotonic_now_ns,
    spawn_threads,
};
use pin_core::spsc::channel;
use pin_core::{FrameStateSnapshot, InputEvent, SimStep, TICK_NS};
use pin_render::topology::{WindowEvent, WindowEventKind};
use std::time::{Duration, Instant};

/// Waits (yielding) until `cond` holds; gives up after `limit` of wall time.
fn wait_for(threads: &Threads, limit: Duration, cond: impl Fn(&Threads) -> bool) -> bool {
    let t0 = Instant::now();
    while t0.elapsed() < limit {
        if cond(threads) {
            return true;
        }
        std::thread::yield_now();
    }
    cond(threads)
}

#[test]
fn spawn_run_and_shutdown_within_one_second() {
    let (_tx, rx) = channel::<InputEvent>(16);
    let threads = spawn_threads(ThreadsConfig::idle(rx)).expect("spawn");
    assert_eq!(threads.names(), vec!["sim", "render", "audio", "io"]);
    assert!(wait_for(&threads, Duration::from_secs(10), |t| {
        let s = t.stats();
        s.sim_ticks >= 20 && s.render_last_tick >= 1 && s.render_iterations >= 2
    }));
    let s = threads.stats();
    assert!(!s.gpu_ready && !s.gpu_error, "no GPU requested");
    let t0 = Instant::now();
    let fin = threads.shutdown().expect("clean shutdown");
    let took = t0.elapsed();
    assert!(took < Duration::from_secs(1), "shutdown took {took:?}");
    assert!(fin.sim_ticks >= 20);
    assert_eq!(fin.render_non_monotonic, 0);
}

#[test]
fn drop_without_shutdown_joins() {
    let (_tx, rx) = channel::<InputEvent>(4);
    let threads = spawn_threads(ThreadsConfig::idle(rx)).expect("spawn");
    let t0 = Instant::now();
    drop(threads);
    assert!(t0.elapsed() < Duration::from_secs(1));
}

#[test]
fn close_request_reaches_main_per_policy() {
    for (policy, window, quits) in [
        (ClosePolicy::PlayfieldOnly, 0, true),
        (ClosePolicy::PlayfieldOnly, 1, false),
        (ClosePolicy::AnyWindow, 2, true),
        (ClosePolicy::Ignore, 0, false),
    ] {
        let (_tx, rx) = channel::<InputEvent>(4);
        let (mut wtx, wrx) = channel::<WindowEvent>(8);
        let mut cfg = ThreadsConfig::idle(rx);
        cfg.render = RenderConfig {
            window_events: Some(wrx),
            close_policy: policy,
            idle_period: Duration::from_millis(1),
            ..RenderConfig::default()
        };
        let threads = spawn_threads(cfg).unwrap();
        wtx.push(WindowEvent {
            timestamp_ns: 1,
            window,
            kind: WindowEventKind::Resized { w: 10, h: 10 },
        })
        .unwrap();
        wtx.push(WindowEvent {
            timestamp_ns: 2,
            window,
            kind: WindowEventKind::CloseRequested,
        })
        .unwrap();
        // Two more render iterations guarantee the ring was drained.
        let base = threads.stats().render_iterations;
        assert!(wait_for(&threads, Duration::from_secs(10), |t| {
            t.close_requested() || t.stats().render_iterations >= base + 3
        }));
        assert_eq!(threads.close_requested(), quits, "{policy:?} window {window}");
        assert_eq!(threads.shutdown().unwrap().close_requested, quits);
    }
}

struct Panics;
impl SimStep for Panics {
    fn step_1ms(&mut self, tick: u64, _: &[InputEvent], out: &mut FrameStateSnapshot) {
        out.tick = tick;
        if tick == 3 {
            panic!("sim failure (expected by the test)");
        }
    }
}

#[test]
fn panicking_sim_is_reported() {
    let (_tx, rx) = channel::<InputEvent>(4);
    let mut cfg = ThreadsConfig::idle(rx);
    cfg.sim = Box::new(Panics);
    let threads = spawn_threads(cfg).unwrap();
    assert!(wait_for(&threads, Duration::from_secs(10), |t| t.finished() == ["sim"]));
    let e = threads.shutdown().unwrap_err();
    assert!(e.contains("sim"), "{e}");
}

/// Non-gating benchmark (verification criterion 6): real-time drift of the
/// sim thread over 10 s. Run with
/// `cargo test -p pin_app --release --test orchestrator_threads -- --ignored --nocapture`.
#[test]
#[ignore = "10 s real-time benchmark"]
fn realtime_drift_benchmark() {
    let (_tx, rx) = channel::<InputEvent>(4);
    let mut cfg = ThreadsConfig::idle(rx);
    cfg.timer = HybridTimer::default();
    cfg.now_ns = monotonic_now_ns;
    let t0 = monotonic_now_ns();
    let threads = spawn_threads(cfg).unwrap();
    std::thread::sleep(Duration::from_secs(10));
    let s = threads.stats();
    let elapsed = monotonic_now_ns() - t0;
    threads.shutdown().unwrap();
    let ideal = elapsed as f64 / TICK_NS as f64;
    let ran = s.sim_ticks as f64;
    let drift = (ideal - ran).abs() / ideal * 100.0;
    println!(
        "[drift] {:.3} s wall, {} ticks run, {} dropped, ideal {:.1}: drift {:.4} %",
        elapsed as f64 / 1e9,
        s.sim_ticks,
        s.sim_dropped_ticks,
        ideal,
        drift
    );
    assert!(drift < 0.1, "drift {drift:.4} % >= 0.1 %");
}
