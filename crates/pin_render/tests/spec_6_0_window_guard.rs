//! Subphase 6.0 spec tests (Independent Tester), GPU-free: the window
//! lifetime guard of issue #9 item 1.
//!
//! Contract (brief 6.0 item 1): `pin_render::gpu::WindowGuard::new(window)`
//! for any `T: Send + Sync + 'static`, `Clone + Send + Sync`,
//! `strong_count()`; `pin_render::gpu::spawn_guarded(name, guard, body)`
//! spawns a thread that owns `guard` and drops it only after `body` returns;
//! `SurfaceSource::keep_alive: Option<WindowGuard>`.
//!
//! The "window" is a mock whose `Drop` counts how often it ran and records
//! whether a guarded body was still running at that moment. Stalls are
//! released through `mpsc` channels (no sleeps, no wall clock, D8).

use pin_render::gpu::{SurfaceSource, WindowGuard, spawn_guarded};
use proptest::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::thread::JoinHandle;

/// Shared observation state of one mock window.
#[derive(Default)]
struct Probe {
    /// How many times the mock was dropped.
    drops: AtomicUsize,
    /// Guarded bodies currently running.
    bodies_running: AtomicUsize,
    /// Set when the mock was dropped while a guarded body was running.
    dropped_during_body: AtomicBool,
}

/// Mock native window: no handles, only a drop probe.
struct MockWindow {
    probe: Arc<Probe>,
}

impl Drop for MockWindow {
    fn drop(&mut self) {
        if self.probe.bodies_running.load(Ordering::SeqCst) != 0 {
            self.probe.dropped_during_body.store(true, Ordering::SeqCst);
        }
        self.probe.drops.fetch_add(1, Ordering::SeqCst);
    }
}

fn mock() -> (WindowGuard, Arc<Probe>) {
    let probe = Arc::new(Probe::default());
    let guard = WindowGuard::new(MockWindow {
        probe: probe.clone(),
    });
    (guard, probe)
}

/// A stalled "presenter": signals `started`, blocks until `release` yields,
/// then marks itself finished. Runs under `spawn_guarded` with `guard`.
fn stalled_presenter(
    guard: WindowGuard,
    probe: &Arc<Probe>,
) -> (JoinHandle<()>, mpsc::Sender<()>, mpsc::Receiver<()>) {
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let (started_tx, started_rx) = mpsc::channel::<()>();
    let p = probe.clone();
    let handle = spawn_guarded("spec-6.0-presenter", guard, move || {
        p.bodies_running.fetch_add(1, Ordering::SeqCst);
        started_tx.send(()).expect("test alive");
        // The stall: a presenter blocked in a Fifo acquire/present.
        let _ = release_rx.recv();
        p.bodies_running.fetch_sub(1, Ordering::SeqCst);
    })
    .expect("spawn_guarded spawns the thread");
    (handle, release_tx, started_rx)
}

fn assert_traits<T: Clone + Send + Sync + 'static>() {}

/// `SurfaceSource::keep_alive` exists and is an `Option<WindowGuard>`.
fn keep_alive_count(s: &SurfaceSource) -> Option<usize> {
    s.keep_alive.as_ref().map(WindowGuard::strong_count)
}

#[test]
fn window_guard_is_clone_send_sync_and_surface_source_holds_one() {
    assert_traits::<WindowGuard>();
    let _typed: fn(&SurfaceSource) -> Option<usize> = keep_alive_count;
}

#[test]
fn strong_count_tracks_clones() {
    let (g, probe) = mock();
    assert_eq!(g.strong_count(), 1);
    let clones: Vec<WindowGuard> = (0..5).map(|_| g.clone()).collect();
    assert_eq!(g.strong_count(), 6);
    for c in &clones {
        assert_eq!(c.strong_count(), 6);
    }
    drop(clones);
    assert_eq!(g.strong_count(), 1);
    assert_eq!(probe.drops.load(Ordering::SeqCst), 0);
    drop(g);
    assert_eq!(probe.drops.load(Ordering::SeqCst), 1);
}

/// Issue #9 done-when 1: the window is dropped (every main-thread handle
/// gone) while the presenter is stalled; the mock lives until the presenter
/// body has returned and is dropped exactly once, after it.
#[test]
fn window_dropped_while_presenter_stalled_outlives_the_stall() {
    for n in 1..=8usize {
        let (guard, probe) = mock();
        let clones: Vec<WindowGuard> = (0..n).map(|_| guard.clone()).collect();
        let (handle, release, started) = stalled_presenter(guard.clone(), &probe);
        started.recv().expect("presenter body started");
        assert!(guard.strong_count() >= n + 2, "n={n}: the thread holds one");

        // The main thread "destroys the window": every handle it has goes.
        drop(clones);
        drop(guard);
        assert_eq!(
            probe.drops.load(Ordering::SeqCst),
            0,
            "n={n}: window freed while the presenter is stalled (use-after-free)"
        );
        assert_eq!(probe.bodies_running.load(Ordering::SeqCst), 1);

        release.send(()).expect("presenter alive");
        handle.join().expect("presenter body did not panic");
        assert_eq!(
            probe.drops.load(Ordering::SeqCst),
            1,
            "n={n}: dropped exactly once"
        );
        assert!(
            !probe.dropped_during_body.load(Ordering::SeqCst),
            "n={n}: dropped while the body ran"
        );
    }
}

/// Several stalled presenters on one window: it lives until the last one
/// has finished, whatever order they are released in.
#[test]
fn window_outlives_every_stalled_presenter() {
    let (guard, probe) = mock();
    let mut threads: Vec<_> = (0..3)
        .map(|_| stalled_presenter(guard.clone(), &probe))
        .collect();
    for (_, _, started) in &threads {
        started.recv().expect("started");
    }
    drop(guard);
    // Release in the order 1, 2, 0.
    for i in [1usize, 1, 0] {
        let (h, release, _) = threads.remove(i);
        release.send(()).expect("alive");
        h.join().expect("no panic");
        let left = threads.len();
        let expect = usize::from(left == 0);
        assert_eq!(probe.drops.load(Ordering::SeqCst), expect);
    }
    assert!(!probe.dropped_during_body.load(Ordering::SeqCst));
}

/// The thread name is the one given.
#[test]
fn spawned_thread_has_the_given_name() {
    let (guard, probe) = mock();
    let (tx, rx) = mpsc::channel::<Option<String>>();
    let h = spawn_guarded("spec-6.0-named", guard, move || {
        tx.send(std::thread::current().name().map(str::to_owned))
            .expect("alive");
    })
    .expect("spawned");
    h.join().expect("no panic");
    assert_eq!(rx.recv().expect("sent"), Some("spec-6.0-named".to_owned()));
    assert_eq!(probe.drops.load(Ordering::SeqCst), 1);
}

/// A panicking body still drops the guard exactly once, after the body.
#[test]
fn body_panic_still_drops_the_guard_once() {
    let (guard, probe) = mock();
    let main_copy = guard.clone();
    let (go_tx, go_rx) = mpsc::channel::<()>();
    let p = probe.clone();
    let h = spawn_guarded("spec-6.0-panicking", guard, move || {
        p.bodies_running.fetch_add(1, Ordering::SeqCst);
        let _ = go_rx.recv();
        p.bodies_running.fetch_sub(1, Ordering::SeqCst);
        panic!("presenter body panics (expected by the test)");
    })
    .expect("spawned");
    drop(main_copy);
    assert_eq!(probe.drops.load(Ordering::SeqCst), 0);
    go_tx.send(()).expect("alive");
    assert!(h.join().is_err(), "the body's panic reaches join");
    assert_eq!(probe.drops.load(Ordering::SeqCst), 1);
    assert!(!probe.dropped_during_body.load(Ordering::SeqCst));
}

/// A body that panics while the main thread still holds a copy: the
/// thread's share is released, the main copy keeps the window alive.
#[test]
fn body_panic_releases_only_the_threads_share() {
    let (guard, probe) = mock();
    let h = spawn_guarded("spec-6.0-panicking-2", guard.clone(), || {
        panic!("presenter body panics (expected by the test)");
    })
    .expect("spawned");
    assert!(h.join().is_err());
    assert_eq!(guard.strong_count(), 1);
    assert_eq!(probe.drops.load(Ordering::SeqCst), 0);
    drop(guard);
    assert_eq!(probe.drops.load(Ordering::SeqCst), 1);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 64, .. ProptestConfig::default() })]

    /// `clones` holders (plus the original) dropped in a random order, with
    /// a stalled guarded thread released at a random point of that order:
    /// the mock is alive until the last holder is gone and dropped once.
    #[test]
    fn mock_outlives_every_holder(
        clones in 1usize..=8,
        order_seed in any::<u64>(),
        thread_at in 0usize..=9,
    ) {
        let (guard, probe) = mock();
        let mut holders: Vec<Option<WindowGuard>> = vec![Some(guard.clone())];
        holders.extend((0..clones).map(|_| Some(guard.clone())));
        let (h, release, started) = stalled_presenter(guard, &probe);
        started.recv().expect("started");

        // Fisher–Yates with a SplitMix stream over the holder indices.
        let mut order: Vec<usize> = (0..holders.len()).collect();
        let mut s = order_seed;
        for i in (1..order.len()).rev() {
            s = s.wrapping_add(0x9E37_79B9_7F4A_7C15);
            let mut z = s;
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_1EB5);
            z ^= z >> 31;
            order.swap(i, (z % (i as u64 + 1)) as usize);
        }
        let thread_at = thread_at.min(order.len());
        let mut thread = Some((h, release));
        let total = order.len() + 1;
        let mut gone = 0usize;
        for (k, idx) in order.iter().enumerate() {
            if k == thread_at && let Some((h, release)) = thread.take() {
                release.send(()).expect("alive");
                h.join().expect("no panic");
                gone += 1;
                prop_assert_eq!(probe.drops.load(Ordering::SeqCst), usize::from(gone == total));
            }
            holders[*idx] = None;
            gone += 1;
            prop_assert_eq!(probe.drops.load(Ordering::SeqCst), usize::from(gone == total));
        }
        if let Some((h, release)) = thread.take() {
            prop_assert_eq!(probe.drops.load(Ordering::SeqCst), 0);
            release.send(()).expect("alive");
            h.join().expect("no panic");
        }
        prop_assert_eq!(probe.drops.load(Ordering::SeqCst), 1);
        prop_assert!(!probe.dropped_during_body.load(Ordering::SeqCst));
    }
}

/// The doc comment directly above `fn <name>` in `src`, if any.
fn doc_above(src: &str, needle: &str) -> Option<String> {
    let lines: Vec<&str> = src.lines().collect();
    let at = lines.iter().position(|l| l.contains(needle))?;
    let mut doc = Vec::new();
    for l in lines[..at].iter().rev() {
        let t = l.trim_start();
        if let Some(d) = t.strip_prefix("///") {
            doc.push(d.to_owned());
        } else if t.starts_with("#[") {
            continue;
        } else {
            break;
        }
    }
    doc.reverse();
    Some(doc.join("\n"))
}

fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .expect("readable src dir")
        .map(|e| e.expect("dir entry").path())
        .collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

/// Issue #9 done-when 3: the Safety note of `create_surface` names the
/// guard that now closes the stalled-presenter gap.
#[test]
fn create_surface_safety_doc_mentions_the_window_guard() {
    let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    let doc = files
        .iter()
        .filter_map(|f| {
            let text = std::fs::read_to_string(f).expect("readable source");
            doc_above(&text, "pub unsafe fn create_surface")
                .or_else(|| doc_above(&text, "pub fn create_surface"))
        })
        .next()
        .expect("create_surface is defined in pin_render/src");
    assert!(
        doc.contains("# Safety"),
        "create_surface has a Safety section"
    );
    assert!(
        doc.contains("WindowGuard") || doc.contains("keep_alive"),
        "the Safety note of create_surface mentions the window guard:\n{doc}"
    );
}
