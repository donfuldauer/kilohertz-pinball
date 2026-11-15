//! [`WindowKeeper`]: SDL windows that render threads may outlive (issue #9a,
//! DECISIONS "6.0 — window lifetime").
//!
//! wgpu surfaces borrow their window's native handle. A presenter thread
//! that misses `PRESENTER_JOIN_TIMEOUT` is detached and keeps its surface,
//! so the window must stay alive until that thread exits. But an SDL window
//! is `!Send`/`!Sync` and must be destroyed on the main thread (Cocoa), so it
//! cannot simply sit in an `Arc` that a presenter thread may drop last.
//!
//! The keeper owns the windows on the main thread. [`WindowKeeper::adopt`]
//! returns a [`WindowGuard`] whose value is a [`DeferredDestroy`] token
//! (`Send + Sync`: a window id and a channel sender). Clones of the guard go
//! with the surfaces ([`SurfaceSource::keep_alive`](pin_render::gpu::SurfaceSource)).
//! Whichever thread drops the last clone, the token's `Drop` only sends the
//! id to the main thread; [`WindowKeeper::drain`] (run by
//! [`Platform::pump`](super::Platform::pump)) destroys the window there. A
//! window whose guard is still held anywhere is never destroyed.
//!
//! Generic over the window type so the lifetime rules are testable without
//! SDL; the app uses [`PlatformWindow`](super::PlatformWindow).

use pin_render::gpu::WindowGuard;
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::time::{Duration, Instant};

/// The guard value of a kept window: dropping it (on any thread) asks the
/// keeper's thread to destroy window `id`.
#[derive(Debug)]
pub struct DeferredDestroy {
    id: u64,
    tx: Sender<u64>,
}

impl DeferredDestroy {
    /// The keeper-assigned window id.
    pub fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for DeferredDestroy {
    fn drop(&mut self) {
        // The keeper may already be gone (process shutdown): nothing to do.
        let _ = self.tx.send(self.id);
    }
}

/// Owns windows on the creating (main) thread until no guard holds them.
pub struct WindowKeeper<W> {
    tx: Sender<u64>,
    rx: Receiver<u64>,
    /// `(id, window, released)` in adoption order.
    windows: Vec<(u64, W, bool)>,
    next_id: u64,
    destroyed: u64,
}

impl<W> Default for WindowKeeper<W> {
    fn default() -> Self {
        Self::new()
    }
}

impl<W> WindowKeeper<W> {
    /// An empty keeper.
    pub fn new() -> Self {
        let (tx, rx) = channel();
        Self {
            tx,
            rx,
            windows: Vec::new(),
            next_id: 0,
            destroyed: 0,
        }
    }

    /// Takes ownership of `window` and returns the guard that keeps it
    /// alive. The window is destroyed by [`drain`](Self::drain) (on this
    /// thread) once every clone of the guard has been dropped.
    pub fn adopt(&mut self, window: W) -> WindowGuard {
        let id = self.next_id;
        self.next_id += 1;
        self.windows.push((id, window, false));
        WindowGuard::new(DeferredDestroy {
            id,
            tx: self.tx.clone(),
        })
    }

    /// The kept windows (alive), in adoption order.
    pub fn windows(&self) -> impl Iterator<Item = &W> + '_ {
        self.windows.iter().map(|(_, w, _)| w)
    }

    /// Windows still alive.
    pub fn len(&self) -> usize {
        self.windows.len()
    }

    /// `true` when no window is kept.
    pub fn is_empty(&self) -> bool {
        self.windows.is_empty()
    }

    /// Windows destroyed so far.
    pub fn destroyed(&self) -> u64 {
        self.destroyed
    }

    fn mark(&mut self, id: u64) {
        if let Some(e) = self.windows.iter_mut().find(|e| e.0 == id) {
            e.2 = true;
        }
    }

    fn destroy_released(&mut self) -> usize {
        let before = self.windows.len();
        // Destroyed here, on the keeper's thread, in adoption order.
        self.windows.retain(|e| !e.2);
        let n = before - self.windows.len();
        self.destroyed += n as u64;
        n
    }

    /// Destroys every window whose guards are all gone. Never blocks.
    /// Returns how many were destroyed.
    pub fn drain(&mut self) -> usize {
        while let Ok(id) = self.rx.try_recv() {
            self.mark(id);
        }
        self.destroy_released()
    }

    /// Waits up to `timeout` for the guards of every kept window to be
    /// dropped, destroying windows as they are released. Returns `true`
    /// when none is left. Used at shutdown, after the render thread was
    /// joined: only a detached presenter thread can still hold a guard.
    pub fn wait_all_released(&mut self, timeout: Duration) -> bool {
        let start = Instant::now();
        self.drain();
        while !self.windows.is_empty() {
            let left = timeout.saturating_sub(start.elapsed());
            match self.rx.recv_timeout(left) {
                Ok(id) => {
                    self.mark(id);
                    self.drain();
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
            }
        }
        self.windows.is_empty()
    }

    /// Gives up ownership of the still-guarded windows without destroying
    /// them (leaks them): the process is about to exit while a detached
    /// presenter thread may still use one. Returns how many were leaked.
    pub fn leak_remaining(&mut self) -> usize {
        let n = self.windows.len();
        for e in self.windows.drain(..) {
            std::mem::forget(e);
        }
        n
    }
}

impl<W> Drop for WindowKeeper<W> {
    fn drop(&mut self) {
        self.drain();
        // Windows whose guards are still held are leaked, never destroyed
        // under a thread that may still present to them.
        let leaked = self.leak_remaining();
        if leaked > 0 {
            eprintln!(
                "pin_app: {leaked} window(s) still in use by a presenter thread at shutdown; not destroyed"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct Mock(Arc<AtomicUsize>);
    impl Drop for Mock {
        fn drop(&mut self) {
            self.0.fetch_add(1, Ordering::SeqCst);
        }
    }

    #[test]
    fn destroyed_only_after_last_guard_and_drain() {
        let drops = Arc::new(AtomicUsize::new(0));
        let mut k = WindowKeeper::new();
        let g = k.adopt(Mock(Arc::clone(&drops)));
        let g2 = g.clone();
        assert_eq!(k.drain(), 0);
        drop(g);
        assert_eq!(k.drain(), 0);
        let t = std::thread::spawn(move || drop(g2));
        t.join().unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 0, "not destroyed off-thread");
        assert_eq!(k.drain(), 1);
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert!(k.is_empty());
        assert_eq!(k.destroyed(), 1);
    }

    #[test]
    fn wait_and_leak() {
        let drops = Arc::new(AtomicUsize::new(0));
        let mut k = WindowKeeper::new();
        let held = k.adopt(Mock(Arc::clone(&drops)));
        let released = k.adopt(Mock(Arc::clone(&drops)));
        assert_eq!(k.len(), 2);
        assert_eq!(k.windows().count(), 2);
        drop(released);
        assert!(!k.wait_all_released(Duration::ZERO));
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        assert_eq!(k.len(), 1);
        drop(k); // leaks the held window
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        drop(held); // keeper gone: the send fails quietly
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn wait_returns_once_released_from_another_thread() {
        let drops = Arc::new(AtomicUsize::new(0));
        let mut k = WindowKeeper::new();
        let g = k.adopt(Mock(Arc::clone(&drops)));
        let (go_tx, go_rx) = std::sync::mpsc::channel::<()>();
        let t = std::thread::spawn(move || {
            go_rx.recv().unwrap();
            drop(g);
        });
        go_tx.send(()).unwrap();
        assert!(k.wait_all_released(Duration::from_secs(60)));
        t.join().unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 1);
        let mut k2: WindowKeeper<Mock> = WindowKeeper::default();
        assert!(k2.wait_all_released(Duration::ZERO));
        assert_eq!(k2.leak_remaining(), 0);
    }
}
