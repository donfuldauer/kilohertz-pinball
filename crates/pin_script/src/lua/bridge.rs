//! Host state shared between the backend and the Rust functions registered in
//! the Lua state: the current callback's [`HostCtx`] (as raw pointers, valid
//! only while a callback runs), the overflow counter, the coil-hold set used
//! for the tilt release, and the instruction-budget counters.
//!
//! Everything here is plain `Cell`s in one fixed-size block: the per-callback
//! paths never allocate (gate step 6).

use pin_core::ids::CoilId;
use pin_core::script::QueueOverflow;
use pin_core::{HostCommandQueue, HostCtx, HostView, Pcg32};
use std::cell::Cell;

/// Words in the coil-hold bitset (one bit per possible `CoilId`).
const HOLD_WORDS: usize = (u16::MAX as usize + 1) / 64;

#[derive(Copy, Clone)]
struct RawCtx {
    view: *const HostView,
    rng: *mut Pcg32,
    cmd: *mut HostCommandQueue,
}

/// See the module docs.
pub(crate) struct Bridge {
    ctx: Cell<Option<RawCtx>>,
    overflow: Cell<u32>,
    holds: [Cell<u64>; HOLD_WORDS],
    /// Coils whose tilt release is still waiting for queue room. Kept apart
    /// from `holds` so a reload in the middle of the drain neither stops it
    /// nor hands those coils to the new script.
    releasing: [Cell<u64>; HOLD_WORDS],
    budget_hit: Cell<bool>,
    memory_hit: Cell<bool>,
    /// Instructions between two hook calls.
    pub(crate) hook_interval: u32,
}

/// Clears the bridge's context when the callback returns (also on unwind).
pub(crate) struct CtxGuard<'a>(&'a Bridge);

impl Drop for CtxGuard<'_> {
    fn drop(&mut self) {
        self.0.ctx.set(None);
    }
}

impl Bridge {
    pub(crate) fn new(hook_interval: u32) -> Self {
        Self {
            ctx: Cell::new(None),
            overflow: Cell::new(0),
            holds: [const { Cell::new(0) }; HOLD_WORDS],
            releasing: [const { Cell::new(0) }; HOLD_WORDS],
            budget_hit: Cell::new(false),
            memory_hit: Cell::new(false),
            hook_interval,
        }
    }

    /// Makes `ctx` visible to the API functions until the guard drops. The
    /// caller must not touch `ctx` while the guard lives.
    pub(crate) fn enter<'a>(&'a self, ctx: &mut HostCtx<'_>) -> CtxGuard<'a> {
        self.ctx.set(Some(RawCtx {
            view: std::ptr::from_ref(ctx.view),
            rng: std::ptr::from_mut(&mut *ctx.rng),
            cmd: std::ptr::from_mut(&mut *ctx.cmd),
        }));
        CtxGuard(self)
    }

    /// Runs `f` on the current callback's context, or fails when the API is
    /// used outside a callback (top-level chunk, reload hooks, finalizers run
    /// by the per-tick GC step).
    pub(crate) fn with_ctx<R>(
        &self,
        f: impl FnOnce(&HostView, &mut Pcg32, &mut HostCommandQueue) -> R,
    ) -> mlua::Result<R> {
        match self.ctx.get() {
            // SAFETY: the pointers were taken from a live `&mut HostCtx` in
            // `enter`, whose guard clears them before that borrow ends; the
            // backend does not touch the context while the guard lives, and the
            // Lua API functions run one at a time on this thread (no
            // re-entrancy: `f` never calls back into Lua).
            Some(raw) => Ok(unsafe { f(&*raw.view, &mut *raw.rng, &mut *raw.cmd) }),
            None => Err(mlua::Error::runtime(
                "the pinball API is only available inside on_init, on_tick and on_switch",
            )),
        }
    }

    /// Counts a dropped command.
    pub(crate) fn note(&self, r: Result<(), QueueOverflow>) -> bool {
        match r {
            Ok(()) => true,
            Err(QueueOverflow) => {
                self.overflow.set(self.overflow.get().saturating_add(1));
                false
            }
        }
    }

    pub(crate) fn overflow_count(&self) -> u32 {
        self.overflow.get()
    }

    /// Restores the overflow counter (rollback of a faulted callback).
    pub(crate) fn set_overflow_count(&self, n: u32) {
        self.overflow.set(n);
    }

    /// Records the hold state of `coil` after a successfully queued hold.
    /// The script now owns the coil again, so any tilt release still pending
    /// for it is dropped.
    pub(crate) fn set_held(&self, coil: CoilId, on: bool) {
        let i = coil.index();
        let w = &self.holds[i / 64];
        let bit = 1u64 << (i % 64);
        w.set(if on { w.get() | bit } else { w.get() & !bit });
        let r = &self.releasing[i / 64];
        r.set(r.get() & !bit);
    }

    /// True if the script currently holds `coil`.
    #[cfg(test)]
    pub(crate) fn is_held(&self, coil: CoilId) -> bool {
        let i = coil.index();
        self.holds[i / 64].get() & (1u64 << (i % 64)) != 0
    }

    /// Starts the tilt release: every held coil moves to the pending-release
    /// set, then as many releases are queued as fit ([`Self::drain_releases`]).
    pub(crate) fn release_holds(&self, cmd: &mut HostCommandQueue) {
        for (r, h) in self.releasing.iter().zip(self.holds.iter()) {
            r.set(r.get() | h.get());
            h.set(0);
        }
        self.drain_releases(cmd);
    }

    /// Queues `(coil, false)` for every coil pending release while the queue
    /// has room; coils that do not fit stay pending for the next call.
    pub(crate) fn drain_releases(&self, cmd: &mut HostCommandQueue) {
        for (wi, w) in self.releasing.iter().enumerate() {
            let mut bits = w.get();
            while bits != 0 {
                let b = bits.trailing_zeros() as usize;
                // `wi * 64 + b` ≤ u16::MAX by construction of HOLD_WORDS.
                let coil = CoilId((wi * 64 + b) as u16);
                if cmd.hold_coil(coil, false).is_err() {
                    return;
                }
                bits &= bits - 1;
                w.set(bits);
            }
        }
    }

    /// True while any coil is marked held.
    #[cfg(test)]
    pub(crate) fn any_held(&self) -> bool {
        self.holds.iter().any(|w| w.get() != 0)
    }

    /// True while tilt releases are still waiting for queue room.
    pub(crate) fn releases_pending(&self) -> bool {
        self.releasing.iter().any(|w| w.get() != 0)
    }

    /// Clears the abort flags before a callback (the budget itself lives in
    /// the patched Lua state, see `Vm::reset_budget`).
    pub(crate) fn reset_flags(&self) {
        self.budget_hit.set(false);
        self.memory_hit.set(false);
    }

    /// Records that the instruction budget ran out.
    pub(crate) fn mark_budget_hit(&self) {
        self.budget_hit.set(true);
    }

    pub(crate) fn budget_hit(&self) -> bool {
        self.budget_hit.get()
    }

    pub(crate) fn mark_memory_hit(&self) {
        self.memory_hit.set(true);
    }

    pub(crate) fn memory_hit(&self) -> bool {
        self.memory_hit.get()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pin_core::SeedableRng;

    #[test]
    fn holds_release_in_order_and_resume_after_overflow() {
        let b = Bridge::new(1000);
        for c in 0..40u16 {
            b.set_held(CoilId(c * 3), true);
        }
        b.set_held(CoilId(u16::MAX), true);
        assert!(b.is_held(CoilId(u16::MAX)));
        let mut q = HostCommandQueue::default();
        b.release_holds(&mut q);
        assert_eq!(q.coil_holds.len(), 32);
        assert_eq!(q.coil_holds[0], (CoilId(0), false));
        assert_eq!(q.coil_holds[31], (CoilId(93), false));
        assert!(!b.any_held());
        assert!(b.releases_pending());
        q.clear();
        b.drain_releases(&mut q);
        assert_eq!(q.coil_holds.len(), 9);
        assert_eq!(q.coil_holds[8], (CoilId(u16::MAX), false));
        assert!(!b.releases_pending());
        b.set_held(CoilId(7), true);
        b.set_held(CoilId(7), false);
        assert!(!b.is_held(CoilId(7)));
    }

    #[test]
    fn flags_reset() {
        let b = Bridge::new(1000);
        b.mark_budget_hit();
        b.mark_memory_hit();
        assert!(b.budget_hit() && b.memory_hit());
        b.reset_flags();
        assert!(!b.budget_hit() && !b.memory_hit());
    }

    #[test]
    fn ctx_is_cleared_by_guard() {
        let b = Bridge::new(1);
        assert!(b.with_ctx(|_, _, _| ()).is_err());
        let view = HostView::default();
        let mut rng = Pcg32::seed_from_u64(0);
        let mut cmd = HostCommandQueue::default();
        let mut ctx = HostCtx {
            view: &view,
            rng: &mut rng,
            cmd: &mut cmd,
        };
        {
            let _g = b.enter(&mut ctx);
            let pushed = b.with_ctx(|v, _, c| {
                assert_eq!(v.tick, 0);
                c.pulse_coil(CoilId(1), 5)
            });
            assert!(b.note(pushed.unwrap()));
            assert!(!b.note(Err(QueueOverflow)));
        }
        assert!(b.with_ctx(|_, _, _| ()).is_err());
        assert_eq!(b.overflow_count(), 1);
        assert_eq!(cmd.coil_pulses.as_slice(), &[(CoilId(1), 5)]);
    }
}
