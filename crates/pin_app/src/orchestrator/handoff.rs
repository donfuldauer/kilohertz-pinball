//! Latest-frame hand-off between the render thread and a presenter thread
//! (PR #21 round 4, D19), GPU-free and generic over the frame payload.
//!
//! A [`Handoff`] owns a pool of [`HANDOFF_SLOTS`] frame slots. The render
//! thread takes a free slot without blocking ([`Handoff::try_take_free`]),
//! renders into its payload (an offscreen texture in the app), submits, and
//! posts it ([`Handoff::post`]). The mailbox holds at most one posted frame:
//! posting while an older frame is still unconsumed returns the older one to
//! the free pool (latest wins, counted as `dropped_latest`). The presenter
//! thread waits for a posted frame or the close flag
//! ([`Handoff::wait_take`]), does its blocking work (acquire, copy, present)
//! and returns the slot ([`Handoff::release`]).
//!
//! Locking: one `Mutex` guards the pool and the mailbox and is held only for
//! a few instructions (a `Vec` push / pop on pre-reserved capacity, an
//! `Option` swap), never across a GPU call. The render thread never waits
//! on the `Condvar`; only the presenter thread does.
//!
//! Resize: [`Handoff::set_size`] records the current frame size. A posted
//! frame of another size is discarded at take time (`discarded_stale`), and
//! a free slot of another size is the render thread's cue to replace its
//! payload. A stale-size frame is therefore never presented.
//!
//! Shutdown: [`PresenterThread`] runs the presenter body on a thread started
//! with [`spawn_guarded`] (the body's surface drops before the window
//! guard), closes the hand-off when the body ends, also by a panic, and
//! joins with a bounded wait ([`join_with_timeout`]); on timeout it detaches
//! the thread, which keeps its window alive through the guard until it
//! exits.

use pin_render::gpu::{PRESENTER_JOIN_TIMEOUT, WindowGuard, spawn_guarded};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Frame slots per presenter-thread surface: one being rendered, one in
/// the mailbox, one being presented.
pub const HANDOFF_SLOTS: usize = 3;

/// One frame slot: its pool index, the pixel size its payload was made for
/// and the payload itself (`None` until the render thread creates one).
#[derive(Debug)]
pub(crate) struct Frame<T> {
    /// The slot's identity (read by tests and `Debug`).
    #[cfg_attr(not(test), allow(dead_code))]
    index: usize,
    size: (u32, u32),
    payload: Option<T>,
}

impl<T> Frame<T> {
    /// The slot index in the pool (`0..HANDOFF_SLOTS`).
    #[cfg(test)]
    pub(crate) fn index(&self) -> usize {
        self.index
    }

    /// The pixel size the payload was made for.
    pub(crate) fn size(&self) -> (u32, u32) {
        self.size
    }

    /// The payload, if one was created.
    pub(crate) fn payload(&self) -> Option<&T> {
        self.payload.as_ref()
    }

    /// Replaces the payload (dropping the old one) with one made for `size`.
    pub(crate) fn set_payload(&mut self, payload: T, size: (u32, u32)) {
        self.payload = Some(payload);
        self.size = size;
    }
}

/// Result of [`Handoff::try_take_free`].
#[derive(Debug)]
pub(crate) enum TakeFree<T> {
    /// A free slot to render into.
    Frame(Frame<T>),
    /// No free slot (the presenter holds one, one is posted): skip this
    /// surface this iteration.
    Busy,
    /// The presenter side is gone (closed or panicked).
    Closed,
}

/// What became of a frame the presenter thread took ([`Handoff::release`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Copied to the swap-chain image and presented.
    Presented,
    /// Not presented (acquire failed, surface not configured).
    Skipped,
    /// Its size no longer matched the current size.
    Discarded,
}

/// Counter values of one presenter-thread hand-off (see
/// [`AppSurfaces::presenter_stats`](super::AppSurfaces::presenter_stats)).
///
/// Every posted frame ends up in exactly one bucket, so `posted` equals
/// `presented + present_skipped + dropped_latest + discarded_stale` plus
/// the frame still queued (0 or 1) and the one the presenter holds (0 or
/// 1).
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct HandoffStats {
    /// Frames the render thread posted.
    pub posted: u64,
    /// Frames the presenter thread presented.
    pub presented: u64,
    /// Frames the presenter took but could not present.
    pub present_skipped: u64,
    /// Posted frames replaced by a newer post before the presenter took
    /// them (latest wins).
    pub dropped_latest: u64,
    /// Posted frames discarded because their size became stale.
    pub discarded_stale: u64,
    /// Render iterations that found no free slot ([`SurfaceStatus::Busy`](pin_render::gpu::SurfaceStatus::Busy)).
    pub busy_skips: u64,
    /// Failed surface configurations on the presenter thread.
    pub configure_failures: u64,
}

#[derive(Default)]
struct Counters {
    posted: AtomicU64,
    presented: AtomicU64,
    present_skipped: AtomicU64,
    dropped_latest: AtomicU64,
    discarded_stale: AtomicU64,
    busy_skips: AtomicU64,
    configure_failures: AtomicU64,
}

struct State<T> {
    free: Vec<Frame<T>>,
    posted: Option<Frame<T>>,
    size: (u32, u32),
}

/// The latest-frame mailbox with its free-slot pool (see the [module
/// docs](self)).
pub(crate) struct Handoff<T> {
    state: Mutex<State<T>>,
    ready: Condvar,
    closed: AtomicBool,
    counters: Counters,
}

impl<T> Handoff<T> {
    /// A hand-off with `slots` empty frame slots, all free, for frames of
    /// `size` pixels.
    pub(crate) fn new(slots: usize, size: (u32, u32)) -> Handoff<T> {
        let mut free = Vec::with_capacity(slots);
        free.extend((0..slots).map(|index| Frame {
            index,
            size: (0, 0),
            payload: None,
        }));
        Handoff {
            state: Mutex::new(State {
                free,
                posted: None,
                size,
            }),
            ready: Condvar::new(),
            closed: AtomicBool::new(false),
            counters: Counters::default(),
        }
    }

    /// The state (a poisoned lock is still consistent: every critical
    /// section only moves frames between `free` and `posted`).
    fn lock(&self) -> MutexGuard<'_, State<T>> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Render side, never blocks on the presenter: a free slot, or why
    /// there is none.
    pub(crate) fn try_take_free(&self) -> TakeFree<T> {
        if self.is_closed() {
            return TakeFree::Closed;
        }
        match self.lock().free.pop() {
            Some(f) => TakeFree::Frame(f),
            None => {
                self.counters.busy_skips.fetch_add(1, Ordering::Relaxed);
                TakeFree::Busy
            }
        }
    }

    /// Render side: posts a rendered (and submitted) frame. An older,
    /// unconsumed frame goes back to the free pool (`dropped_latest`).
    /// Returns `false` (and keeps the frame in the pool) when the hand-off
    /// is closed.
    pub(crate) fn post(&self, frame: Frame<T>) -> bool {
        let mut st = self.lock();
        if self.is_closed() {
            st.free.push(frame);
            return false;
        }
        self.counters.posted.fetch_add(1, Ordering::Relaxed);
        if let Some(old) = st.posted.replace(frame) {
            st.free.push(old);
            self.counters.dropped_latest.fetch_add(1, Ordering::Relaxed);
        }
        drop(st);
        self.ready.notify_one();
        true
    }

    /// Presenter side: waits for a posted frame of the current size (stale
    /// ones are discarded on the way) and takes it; `None` once the
    /// hand-off is closed (a frame still queued then stays queued).
    pub(crate) fn wait_take(&self) -> Option<Frame<T>> {
        let mut st = self.lock();
        loop {
            if self.is_closed() {
                return None;
            }
            if let Some(f) = self.take_current(&mut st) {
                return Some(f);
            }
            st = self.ready.wait(st).unwrap_or_else(PoisonError::into_inner);
        }
    }

    /// [`wait_take`](Self::wait_take) without waiting: `None` when closed
    /// or nothing of the current size is posted.
    #[cfg(test)]
    pub(crate) fn try_take(&self) -> Option<Frame<T>> {
        let mut st = self.lock();
        if self.is_closed() {
            return None;
        }
        self.take_current(&mut st)
    }

    /// Takes the posted frame if it has the current size; a stale one goes
    /// back to the free pool (`discarded_stale`).
    fn take_current(&self, st: &mut State<T>) -> Option<Frame<T>> {
        let f = st.posted.take()?;
        if f.size == st.size {
            return Some(f);
        }
        st.free.push(f);
        self.counters
            .discarded_stale
            .fetch_add(1, Ordering::Relaxed);
        None
    }

    /// Presenter side: returns a taken frame to the free pool, counting
    /// its `outcome`.
    pub(crate) fn release(&self, frame: Frame<T>, outcome: Outcome) {
        let c = match outcome {
            Outcome::Presented => &self.counters.presented,
            Outcome::Skipped => &self.counters.present_skipped,
            Outcome::Discarded => &self.counters.discarded_stale,
        };
        c.fetch_add(1, Ordering::Relaxed);
        self.lock().free.push(frame);
    }

    /// Render side: returns a frame that was taken but not posted.
    pub(crate) fn give_back(&self, frame: Frame<T>) {
        self.lock().free.push(frame);
    }

    /// Render side: the size of the frames from now on (a posted frame of
    /// another size is discarded, never presented).
    pub(crate) fn set_size(&self, size: (u32, u32)) {
        self.lock().size = size;
    }

    /// The current frame size.
    pub(crate) fn size(&self) -> (u32, u32) {
        self.lock().size
    }

    /// Closes the hand-off and wakes a waiting presenter. Idempotent.
    pub(crate) fn close(&self) {
        // Set under the lock so a presenter between its check and its wait
        // cannot miss the notification.
        let st = self.lock();
        self.closed.store(true, Ordering::Release);
        drop(st);
        self.ready.notify_all();
    }

    /// `true` once [`close`](Self::close) ran (or the presenter ended).
    pub(crate) fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }

    /// Counts one failed surface configuration (presenter side).
    pub(crate) fn record_configure_failure(&self) {
        self.counters
            .configure_failures
            .fetch_add(1, Ordering::Relaxed);
    }

    /// The counters.
    pub(crate) fn stats(&self) -> HandoffStats {
        let c = &self.counters;
        let g = |a: &AtomicU64| a.load(Ordering::Relaxed);
        HandoffStats {
            posted: g(&c.posted),
            presented: g(&c.presented),
            present_skipped: g(&c.present_skipped),
            dropped_latest: g(&c.dropped_latest),
            discarded_stale: g(&c.discarded_stale),
            busy_skips: g(&c.busy_skips),
            configure_failures: g(&c.configure_failures),
        }
    }

    /// `(free, queued)` slot counts.
    #[cfg(test)]
    pub(crate) fn occupancy(&self) -> (usize, usize) {
        let st = self.lock();
        (st.free.len(), usize::from(st.posted.is_some()))
    }
}

/// Closes the hand-off when dropped (also while unwinding from a panic in
/// the presenter body).
struct CloseOnDrop<T>(Arc<Handoff<T>>);

impl<T> Drop for CloseOnDrop<T> {
    fn drop(&mut self) {
        self.0.close();
    }
}

/// A presenter thread and its hand-off (render-thread side).
pub(crate) struct PresenterThread<T> {
    handoff: Arc<Handoff<T>>,
    handle: Option<JoinHandle<()>>,
    name: String,
}

impl<T: Send + 'static> PresenterThread<T> {
    /// Starts `body` on a thread named `name` holding `guard` (dropped only
    /// after `body` and everything it owns). The hand-off is closed when
    /// `body` returns or panics.
    ///
    /// # Errors
    /// The OS error when the thread cannot be spawned.
    pub(crate) fn spawn(
        name: &str,
        guard: WindowGuard,
        handoff: Arc<Handoff<T>>,
        body: impl FnOnce(&Handoff<T>) + Send + 'static,
    ) -> std::io::Result<PresenterThread<T>> {
        let h = Arc::clone(&handoff);
        let handle = spawn_guarded(name, guard, move || {
            let close = CloseOnDrop(h);
            body(&close.0);
        })?;
        Ok(PresenterThread {
            handoff,
            handle: Some(handle),
            name: name.to_owned(),
        })
    }
}

impl<T> PresenterThread<T> {
    /// The hand-off shared with the thread.
    pub(crate) fn handoff(&self) -> &Handoff<T> {
        &self.handoff
    }

    /// Closes the hand-off and waits up to `timeout` for the thread. `Ok`
    /// when it was joined (or already was); on timeout one stderr note and
    /// `Err` with the detached thread's handle (dropping it detaches).
    pub(crate) fn stop(&mut self, timeout: Duration) -> Result<(), JoinHandle<()>> {
        self.handoff.close();
        let Some(h) = self.handle.take() else {
            return Ok(());
        };
        if join_with_timeout(&h, timeout) {
            // A panic in the body was already reported by the panic hook.
            let _ = h.join();
            Ok(())
        } else {
            // A stalled swap chain: detach rather than hang shutdown. The
            // thread still owns the surface, and its `WindowGuard` keeps the
            // window alive until it exits (DECISIONS "6.0 — window
            // lifetime", issue #9a).
            eprintln!(
                "pin_app: presenter thread {} did not stop within {timeout:?}, detaching it",
                self.name
            );
            Err(h)
        }
    }
}

impl<T> Drop for PresenterThread<T> {
    fn drop(&mut self) {
        let _ = self.stop(PRESENTER_JOIN_TIMEOUT);
    }
}

/// Waits (polling every millisecond, render side only) until `h` finished or
/// `timeout` passed; `true` when it finished.
pub(crate) fn join_with_timeout<T>(h: &JoinHandle<T>, timeout: Duration) -> bool {
    let start = Instant::now();
    while !h.is_finished() {
        if start.elapsed() >= timeout {
            return false;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{channel, sync_channel};

    const S: (u32, u32) = (64, 32);

    fn take(h: &Handoff<u32>) -> Frame<u32> {
        match h.try_take_free() {
            TakeFree::Frame(mut f) => {
                if f.size() != h.size() || f.payload().is_none() {
                    f.set_payload(f.index as u32, h.size());
                }
                f
            }
            other => panic!("no free slot: {other:?}"),
        }
    }

    /// `posted == presented + skipped + dropped + discarded + queued +
    /// in_flight`.
    fn balanced(h: &Handoff<u32>, in_flight: u64) {
        let s = h.stats();
        let queued = h.occupancy().1 as u64;
        assert_eq!(
            s.posted,
            s.presented
                + s.present_skipped
                + s.dropped_latest
                + s.discarded_stale
                + queued
                + in_flight,
            "{s:?} queued {queued} in flight {in_flight}"
        );
    }

    #[test]
    fn latest_post_wins_and_the_older_slot_returns_to_the_pool() {
        let h = Handoff::<u32>::new(HANDOFF_SLOTS, S);
        let a = take(&h);
        let b = take(&h);
        let (ia, ib) = (a.index(), b.index());
        assert_ne!(ia, ib);
        assert_eq!(h.occupancy(), (1, 0));
        assert!(h.post(a));
        assert!(h.post(b));
        assert_eq!(h.stats().dropped_latest, 1);
        // A's slot is free again, B is queued.
        assert_eq!(h.occupancy(), (2, 1));
        let got = h.wait_take().expect("posted frame");
        assert_eq!(got.index(), ib);
        balanced(&h, 1);
        h.release(got, Outcome::Presented);
        assert_eq!(h.occupancy(), (3, 0));
        let s = h.stats();
        assert_eq!((s.posted, s.presented, s.dropped_latest), (2, 1, 1));
        balanced(&h, 0);
    }

    #[test]
    fn no_free_slot_is_busy_without_blocking() {
        let h = Handoff::<u32>::new(HANDOFF_SLOTS, S);
        // The presenter holds one, one is posted, one is being rendered.
        let held = take(&h);
        assert!(h.post(held));
        let held = h.wait_take().unwrap();
        let queued = take(&h);
        assert!(h.post(queued));
        let rendering = take(&h);
        assert!(matches!(h.try_take_free(), TakeFree::Busy));
        assert!(matches!(h.try_take_free(), TakeFree::Busy));
        assert_eq!(h.stats().busy_skips, 2);
        h.give_back(rendering);
        h.release(held, Outcome::Presented);
        assert_eq!(h.occupancy(), (2, 1));
        balanced(&h, 0);
    }

    #[test]
    fn no_slot_leaks_over_many_cycles_with_closes_mid_cycle() {
        for close_at in [None, Some(0usize), Some(1), Some(2), Some(3), Some(7)] {
            let h = Handoff::<u32>::new(HANDOFF_SLOTS, S);
            let mut in_flight: Option<Frame<u32>> = None;
            for cycle in 0..50usize {
                if close_at == Some(cycle) {
                    h.close();
                }
                // Render side.
                match h.try_take_free() {
                    TakeFree::Frame(mut f) => {
                        f.set_payload(cycle as u32, S);
                        if cycle % 3 == 1 {
                            h.give_back(f); // rendering aborted
                        } else {
                            let ok = h.post(f);
                            assert_eq!(ok, !h.is_closed());
                        }
                    }
                    TakeFree::Busy => {}
                    TakeFree::Closed => assert!(close_at.is_some_and(|c| cycle >= c)),
                }
                // Presenter side, on alternate cycles.
                if cycle % 2 == 0 {
                    if let Some(f) = in_flight.take() {
                        let o = if cycle % 4 == 0 {
                            Outcome::Presented
                        } else {
                            Outcome::Skipped
                        };
                        h.release(f, o);
                    }
                    if !h.is_closed() && h.occupancy().1 == 1 {
                        in_flight = h.wait_take();
                    }
                }
                let (free, queued) = h.occupancy();
                assert_eq!(
                    free + queued + usize::from(in_flight.is_some()),
                    HANDOFF_SLOTS,
                    "cycle {cycle} close {close_at:?}"
                );
                balanced(&h, u64::from(in_flight.is_some()));
            }
            if let Some(f) = in_flight.take() {
                h.release(f, Outcome::Skipped);
            }
            let (free, queued) = h.occupancy();
            assert_eq!(free + queued, HANDOFF_SLOTS);
            balanced(&h, 0);
        }
    }

    #[test]
    fn close_wakes_a_waiting_taker_and_later_takes_return_none() {
        let h = Arc::new(Handoff::<u32>::new(HANDOFF_SLOTS, S));
        let (started_tx, started_rx) = channel();
        let h2 = Arc::clone(&h);
        let taker = std::thread::spawn(move || {
            started_tx.send(()).unwrap();
            h2.wait_take().map(|f| f.index())
        });
        started_rx.recv().unwrap();
        // Whether the taker is already waiting or not, close must end it.
        h.close();
        assert_eq!(taker.join().unwrap(), None);
        assert!(h.wait_take().is_none());
        assert!(matches!(h.try_take_free(), TakeFree::Closed));
        // A frame taken before the close goes back to the pool on post.
        let h = Handoff::<u32>::new(HANDOFF_SLOTS, S);
        let f = take(&h);
        h.close();
        assert!(!h.post(f));
        assert_eq!(h.occupancy(), (HANDOFF_SLOTS, 0));
        assert_eq!(h.stats().posted, 0);
    }

    #[test]
    fn a_waiting_taker_gets_the_frame_posted_later() {
        let h = Arc::new(Handoff::<u32>::new(HANDOFF_SLOTS, S));
        let (got_tx, got_rx) = sync_channel(1);
        let h2 = Arc::clone(&h);
        let taker = std::thread::spawn(move || {
            while let Some(f) = h2.wait_take() {
                got_tx.send(*f.payload().unwrap()).unwrap();
                h2.release(f, Outcome::Presented);
            }
        });
        for i in 0..20u32 {
            let mut f = match h.try_take_free() {
                TakeFree::Frame(f) => f,
                other => panic!("{other:?}"),
            };
            f.set_payload(i, S);
            assert!(h.post(f));
            // Lock-step: the next frame only after this one was presented.
            assert_eq!(got_rx.recv().unwrap(), i);
        }
        h.close();
        taker.join().unwrap();
        let s = h.stats();
        assert_eq!((s.posted, s.presented, s.dropped_latest), (20, 20, 0));
        balanced(&h, 0);
    }

    #[test]
    fn stale_size_frames_are_discarded_never_taken() {
        let h = Handoff::<u32>::new(HANDOFF_SLOTS, S);
        let old = take(&h);
        assert_eq!(old.size(), S);
        assert!(h.post(old));
        h.set_size((100, 50));
        // The stale frame is discarded at take time, never handed out.
        assert!(h.try_take().is_none());
        assert_eq!(h.stats().discarded_stale, 1);
        assert_eq!(h.occupancy(), (HANDOFF_SLOTS, 0));
        let mut fresh = take(&h);
        assert_eq!(fresh.size(), (100, 50));
        fresh.set_payload(9, (100, 50));
        assert!(h.post(fresh));
        let got = h.wait_take().unwrap();
        assert_eq!(got.size(), (100, 50));
        assert_eq!(got.payload(), Some(&9));
        assert_eq!(h.stats().discarded_stale, 1);
        balanced(&h, 1);
        // A frame that went stale while the presenter held it.
        h.set_size((10, 10));
        h.release(got, Outcome::Discarded);
        let s = h.stats();
        assert_eq!((s.discarded_stale, s.presented), (2, 0));
        assert_eq!(h.occupancy(), (HANDOFF_SLOTS, 0));
        balanced(&h, 0);
        // Free slots of the old size get a new payload from the render side.
        let f = take(&h);
        assert_eq!(f.size(), (10, 10));
    }

    #[test]
    fn bounded_join_finishes_or_times_out() {
        let quick = std::thread::spawn(|| ());
        assert!(join_with_timeout(&quick, Duration::from_secs(10)));
        quick.join().unwrap();
        let (tx, rx) = channel::<()>();
        let stuck = std::thread::spawn(move || {
            let _ = rx.recv();
        });
        assert!(!join_with_timeout(&stuck, Duration::from_millis(5)));
        drop(tx);
        stuck.join().unwrap();
    }

    #[test]
    fn a_stopped_presenter_joins_and_releases_its_guard() {
        let guard = WindowGuard::new(());
        let h = Arc::new(Handoff::<u32>::new(HANDOFF_SLOTS, S));
        let mut p = PresenterThread::spawn("test-presenter", guard.clone(), h, |h| {
            while let Some(f) = h.wait_take() {
                h.release(f, Outcome::Presented);
            }
        })
        .unwrap();
        let f = take(p.handoff());
        assert!(p.handoff().post(f));
        assert!(p.stop(Duration::from_secs(30)).is_ok());
        assert!(p.handoff().is_closed());
        assert_eq!(guard.strong_count(), 1);
        drop(p);
        assert_eq!(guard.strong_count(), 1);
    }

    #[test]
    fn a_presenter_blocked_past_the_timeout_is_detached_and_keeps_its_guard() {
        let guard = WindowGuard::new(());
        let h = Arc::new(Handoff::<u32>::new(HANDOFF_SLOTS, S));
        let (release_tx, release_rx) = channel::<()>();
        let (blocked_tx, blocked_rx) = channel::<()>();
        let mut p = PresenterThread::spawn("test-stuck", guard.clone(), h, move |_| {
            // Stands in for a Fifo acquire that never returns.
            blocked_tx.send(()).unwrap();
            let _ = release_rx.recv();
        })
        .unwrap();
        blocked_rx.recv().unwrap();
        let detached = p
            .stop(Duration::from_millis(5))
            .expect_err("a blocked presenter must be detached");
        assert!(p.handoff().is_closed());
        assert!(matches!(p.handoff().try_take_free(), TakeFree::Closed));
        // The detached thread still holds the window guard.
        assert_eq!(guard.strong_count(), 2);
        drop(p);
        assert_eq!(guard.strong_count(), 2);
        release_tx.send(()).unwrap();
        detached.join().unwrap();
        assert_eq!(guard.strong_count(), 1);
    }

    #[test]
    fn a_panicking_presenter_closes_the_handoff() {
        let guard = WindowGuard::new(());
        let h = Arc::new(Handoff::<u32>::new(HANDOFF_SLOTS, S));
        let mut p = PresenterThread::spawn("test-panic", guard.clone(), h, |h| {
            let _f = h.wait_take();
            panic!("presenter body panicked (expected by the test)");
        })
        .unwrap();
        let f = take(p.handoff());
        assert!(p.handoff().post(f));
        // The panic stays in the presenter thread; the hand-off was closed
        // by the unwinding body (not by the render side), the guard is gone.
        let handle = p.handle.take().expect("running");
        assert!(handle.join().is_err(), "the body panicked");
        assert!(p.handoff().is_closed());
        assert!(matches!(p.handoff().try_take_free(), TakeFree::Closed));
        assert_eq!(guard.strong_count(), 1);
        assert!(p.stop(Duration::from_secs(30)).is_ok());
    }
}
