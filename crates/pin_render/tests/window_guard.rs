//! Issue #9a: a window must never be destroyed while a stalled presenter
//! thread still uses its surface (`pin_render::gpu::WindowGuard`,
//! `spawn_guarded`). GPU-free: a mock window whose `Drop` records when and how
//! often it ran, a mock surface owned by the thread body, and an `mpsc`
//! channel standing in for a swap chain that stalls past the stop deadline.
//! No sleeps (D8): every wait is a channel receive or a join.

use pin_render::gpu::{PRESENTER_JOIN_TIMEOUT, WindowGuard, spawn_guarded};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Shared event log: what was dropped, in order.
#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<&'static str>>>);

impl Log {
    fn push(&self, e: &'static str) {
        self.0.lock().unwrap().push(e);
    }
    fn events(&self) -> Vec<&'static str> {
        self.0.lock().unwrap().clone()
    }
}

/// A mock window: counts its drops and flips `dropped`.
struct MockWindow {
    dropped: Arc<AtomicBool>,
    drops: Arc<AtomicUsize>,
    log: Log,
}

impl Drop for MockWindow {
    fn drop(&mut self) {
        let before = self.drops.fetch_add(1, Ordering::SeqCst);
        assert_eq!(before, 0, "window dropped twice");
        assert!(
            !self.dropped.swap(true, Ordering::SeqCst),
            "window dropped twice"
        );
        self.log.push("window");
    }
}

/// A mock surface: must be dropped while its window is alive.
struct MockSurface {
    window_dropped: Arc<AtomicBool>,
    log: Log,
}

impl Drop for MockSurface {
    fn drop(&mut self) {
        assert!(
            !self.window_dropped.load(Ordering::SeqCst),
            "surface dropped after its window (use-after-free)"
        );
        self.log.push("surface");
    }
}

struct Fixture {
    dropped: Arc<AtomicBool>,
    drops: Arc<AtomicUsize>,
    log: Log,
    guard: WindowGuard,
}

fn fixture() -> Fixture {
    let dropped = Arc::new(AtomicBool::new(false));
    let drops = Arc::new(AtomicUsize::new(0));
    let log = Log::default();
    let guard = WindowGuard::new(MockWindow {
        dropped: Arc::clone(&dropped),
        drops: Arc::clone(&drops),
        log: log.clone(),
    });
    Fixture {
        dropped,
        drops,
        log,
        guard,
    }
}

/// The verification criterion: the presenter is stalled past the stop
/// deadline, every other guard clone is dropped (the render thread gave up
/// and the main thread let go of the window), and the window stays alive
/// until the presenter thread finishes; then it is dropped exactly once,
/// after the surface and after `body` returned.
#[test]
fn stalled_presenter_keeps_window_alive_until_it_exits() {
    let f = fixture();
    let (release_tx, release_rx) = mpsc::channel::<()>();
    let (entered_tx, entered_rx) = mpsc::channel::<()>();
    let body_done = Arc::new(AtomicBool::new(false));

    let surface = MockSurface {
        window_dropped: Arc::clone(&f.dropped),
        log: f.log.clone(),
    };
    let done = Arc::clone(&body_done);
    let log = f.log.clone();
    let handle = spawn_guarded("presenter-test", f.guard.clone(), move || {
        let surface = surface; // the thread owns the surface
        entered_tx.send(()).unwrap();
        // A swap-chain acquire that blocks past the stop deadline.
        release_rx.recv().unwrap();
        drop(surface);
        log.push("body end");
        done.store(true, Ordering::SeqCst);
    })
    .expect("spawn");

    entered_rx.recv().unwrap();
    assert_eq!(f.guard.strong_count(), 2, "the thread holds a clone");

    // The stop deadline passes: the owner detaches the thread (drops its
    // handle-side state) and releases its own guard clones.
    let main_clone = f.guard.clone();
    drop(main_clone);
    drop(f.guard);
    assert!(
        !f.dropped.load(Ordering::SeqCst),
        "window destroyed while the stalled presenter still holds its surface"
    );
    assert_eq!(f.drops.load(Ordering::SeqCst), 0);
    assert!(!handle.is_finished());

    // The swap chain unblocks; the thread finishes.
    release_tx.send(()).unwrap();
    handle.join().expect("presenter thread panicked");
    assert!(body_done.load(Ordering::SeqCst));
    assert!(f.dropped.load(Ordering::SeqCst), "window not dropped");
    assert_eq!(f.drops.load(Ordering::SeqCst), 1, "dropped exactly once");
    assert_eq!(f.log.events(), ["surface", "body end", "window"]);
}

/// When the owner still holds a clone after the thread exits, the window
/// is dropped with the owner's last clone instead, still exactly once.
#[test]
fn window_outlives_thread_while_owner_holds_a_clone() {
    let f = fixture();
    let h = spawn_guarded("short", f.guard.clone(), || {}).unwrap();
    h.join().unwrap();
    assert_eq!(f.guard.strong_count(), 1);
    assert!(!f.dropped.load(Ordering::SeqCst));
    drop(f.guard);
    assert_eq!(f.drops.load(Ordering::SeqCst), 1);
    assert_eq!(f.log.events(), ["window"]);
}

/// A body that panics still releases the guard (once) after unwinding past
/// the surface.
#[test]
fn panicking_body_releases_guard_once() {
    let f = fixture();
    let surface = MockSurface {
        window_dropped: Arc::clone(&f.dropped),
        log: f.log.clone(),
    };
    let h = spawn_guarded("panics", f.guard.clone(), move || {
        let _surface = surface;
        panic!("presenter failed");
    })
    .unwrap();
    drop(f.guard);
    assert!(h.join().is_err());
    assert_eq!(f.drops.load(Ordering::SeqCst), 1);
    assert_eq!(f.log.events(), ["surface", "window"]);
}

/// `WindowGuard` is `Clone + Send + Sync`; clones count; the thread gets
/// its name.
#[test]
fn guard_is_clone_send_sync_and_counts() {
    fn assert_traits<T: Clone + Send + Sync + 'static>() {}
    assert_traits::<WindowGuard>();
    let g = WindowGuard::new(5u32);
    assert_eq!(g.strong_count(), 1);
    let clones: Vec<WindowGuard> = (0..4).map(|_| g.clone()).collect();
    assert_eq!(g.strong_count(), 5);
    drop(clones);
    assert_eq!(g.strong_count(), 1);
    assert!(format!("{g:?}").contains("strong_count: 1"));
    let (tx, rx) = mpsc::channel();
    spawn_guarded("named-presenter", g, move || {
        tx.send(std::thread::current().name().map(str::to_owned))
            .unwrap();
    })
    .unwrap()
    .join()
    .unwrap();
    assert_eq!(rx.recv().unwrap().as_deref(), Some("named-presenter"));
}

#[test]
fn join_timeout_is_documented_value() {
    assert_eq!(PRESENTER_JOIN_TIMEOUT, Duration::from_millis(500));
}
