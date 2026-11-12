//! Thread orchestrator (main, sim, render, audio, I/O) — PLAN §2.2, D5, D6.
//!
//! * [`SimClock`]: pure accumulator turning a monotonic nanosecond clock into
//!   1 ms ticks with a catch-up cap of [`MAX_CATCH_UP_TICKS`]; whole ticks
//!   beyond the cap are dropped (counted), the sub-tick remainder is kept.
//! * [`SimLoop`]: owns the [`SimStep`], the input ring consumer and the
//!   snapshot writer; [`SimLoop::run_ticks`] runs ticks back-to-back on
//!   simulated time without allocating.
//! * [`run_headless`]: `pin_app --headless-ticks N` (no SDL, GPU or sleep).
//! * [`spawn_threads`] / [`Threads`]: the sim, render, audio and I/O
//!   threads. The main thread stays the SDL pump (D6).
//! * [`HybridTimer`]: sleep + spin wait used on the real-time sim path.

mod threads;

pub use threads::{
    ClosePolicy, DropMonitor, OrchestratorStats, RenderConfig, RenderGpu, StatsSnapshot, Threads,
    ThreadsConfig, default_ball_radius, monotonic_now_ns, spawn_threads,
};

use arrayvec::ArrayVec;
use pin_core::sim::tick_end_ns;
use pin_core::spsc::SpscConsumer;
use pin_core::{
    FrameStateSnapshot, InputEvent, MAX_CATCH_UP_TICKS, SimStep, TICK_NS, TripleBuffer,
    TripleWriter,
};

/// Capacity of the per-tick input buffer of [`SimLoop`]. If more events are
/// due in one tick, the rest stay queued and are delivered on the next tick.
pub const INPUT_BUFFER_CAPACITY: usize = 256;

/// Pure tick accumulator: no real time inside, the caller passes `now_ns`.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct SimClock {
    start_ns: u64,
    last_ns: u64,
    /// Sub-tick remainder, always `< TICK_NS`.
    remainder_ns: u64,
    due_total: u64,
    dropped: u64,
}

impl SimClock {
    /// A clock whose tick 1 ends at `start_ns + TICK_NS`.
    pub fn new(start_ns: u64) -> Self {
        Self {
            start_ns,
            last_ns: start_ns,
            remainder_ns: 0,
            due_total: 0,
            dropped: 0,
        }
    }

    /// Ticks due at `now_ns`, at most [`MAX_CATCH_UP_TICKS`]. Whole elapsed
    /// ticks beyond the cap are dropped (counted in
    /// [`dropped_ticks`](Self::dropped_ticks)); the sub-tick remainder is
    /// kept. A `now_ns` earlier than the last call returns 0 and changes
    /// nothing.
    pub fn advance(&mut self, now_ns: u64) -> u32 {
        if now_ns < self.last_ns {
            return 0;
        }
        let elapsed = now_ns - self.last_ns;
        self.last_ns = now_ns;
        // remainder < TICK_NS and elapsed % TICK_NS < TICK_NS: no overflow.
        let part = self.remainder_ns + elapsed % TICK_NS;
        let whole = elapsed / TICK_NS + part / TICK_NS;
        self.remainder_ns = part % TICK_NS;
        let due = whole.min(u64::from(MAX_CATCH_UP_TICKS));
        self.dropped += whole - due;
        self.due_total += due;
        due as u32
    }

    /// Whole ticks dropped by the catch-up cap so far.
    pub fn dropped_ticks(&self) -> u64 {
        self.dropped
    }

    /// Ticks returned by [`advance`](Self::advance) so far.
    pub fn ticks_due_total(&self) -> u64 {
        self.due_total
    }

    /// The `start_ns` given to [`new`](Self::new).
    pub fn start_ns(&self) -> u64 {
        self.start_ns
    }

    /// Clock time at which the next tick becomes due (saturating).
    pub fn next_due_ns(&self) -> u64 {
        tick_end_ns(self.start_ns, self.due_total + self.dropped + 1)
    }
}

/// The stub [`SimStep`] used until Subphase 6.0: sets `out.tick = tick`,
/// otherwise the default snapshot.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct IdleSim;

impl SimStep for IdleSim {
    fn step_1ms(&mut self, tick: u64, _inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
        *out = FrameStateSnapshot {
            tick,
            ..FrameStateSnapshot::default()
        };
    }
}

/// Drives a [`SimStep`] tick by tick on simulated time.
pub struct SimLoop<S: SimStep> {
    sim: S,
    input: SpscConsumer<InputEvent>,
    out: TripleWriter<FrameStateSnapshot>,
    inputs: ArrayVec<InputEvent, INPUT_BUFFER_CAPACITY>,
    snapshot: FrameStateSnapshot,
    tick: u64,
    epoch_ns: u64,
    inputs_delivered: u64,
    input_overflow_ticks: u64,
}

impl<S: SimStep> SimLoop<S> {
    /// A loop at tick 0 with simulated-time epoch 0 (tick `k` ends at
    /// `k · TICK_NS`).
    pub fn new(
        sim: S,
        input: SpscConsumer<InputEvent>,
        out: TripleWriter<FrameStateSnapshot>,
    ) -> Self {
        Self {
            sim,
            input,
            out,
            inputs: ArrayVec::new(),
            snapshot: FrameStateSnapshot::default(),
            tick: 0,
            epoch_ns: 0,
            inputs_delivered: 0,
            input_overflow_ticks: 0,
        }
    }

    /// Sets the simulated-time epoch: tick `k` ends at `epoch_ns + k · TICK_NS`
    /// on the input timestamp clock (SDL's ns clock in the app).
    pub fn set_epoch_ns(&mut self, epoch_ns: u64) {
        self.epoch_ns = epoch_ns;
    }

    /// The simulated-time epoch.
    pub fn epoch_ns(&self) -> u64 {
        self.epoch_ns
    }

    /// Shifts simulated time forward by `ticks` dropped ticks (the catch-up
    /// cap was hit) so input timestamps keep mapping onto the wall clock.
    pub fn skip_ticks(&mut self, ticks: u64) {
        self.epoch_ns = self.epoch_ns.saturating_add(ticks.saturating_mul(TICK_NS));
    }

    /// Runs `n` ticks back-to-back on simulated time: tick `k`'s inputs are
    /// the queued events with `timestamp_ns ≤ epoch + k · TICK_NS` (events
    /// with a later timestamp stay queued); one snapshot is published per
    /// tick. Allocation-free.
    pub fn run_ticks(&mut self, n: u32) {
        for _ in 0..n {
            self.tick += 1;
            let end = tick_end_ns(self.epoch_ns, self.tick);
            self.inputs.clear();
            while !self.inputs.is_full() {
                match self.input.peek() {
                    Some(ev) if ev.timestamp_ns <= end => {
                        let _ = self.input.pop();
                        self.inputs.push(ev);
                    }
                    _ => break,
                }
            }
            if self.inputs.is_full() && self.input.peek().is_some_and(|ev| ev.timestamp_ns <= end) {
                self.input_overflow_ticks += 1;
            }
            self.inputs_delivered += self.inputs.len() as u64;
            self.sim
                .step_1ms(self.tick, &self.inputs, &mut self.snapshot);
            self.out.publish(self.snapshot);
        }
    }

    /// Last completed tick (0 before any).
    pub fn tick(&self) -> u64 {
        self.tick
    }

    /// The last snapshot written by the sim.
    pub fn snapshot(&self) -> &FrameStateSnapshot {
        &self.snapshot
    }

    /// Input events handed to the sim so far.
    pub fn inputs_delivered(&self) -> u64 {
        self.inputs_delivered
    }

    /// Ticks at which more than [`INPUT_BUFFER_CAPACITY`] events were due
    /// (the excess was deferred to the next tick).
    pub fn input_overflow_ticks(&self) -> u64 {
        self.input_overflow_ticks
    }

    /// The sim step.
    pub fn sim(&self) -> &S {
        &self.sim
    }
}

/// Runs `ticks` ticks of [`IdleSim`] on simulated time (no SDL, no GPU, no
/// sleeping), reading every snapshot back through the triple buffer. Returns
/// the last tick read back; `Err` if a snapshot's tick is not exactly the
/// previous one + 1. What `pin_app --headless-ticks N` runs.
pub fn run_headless(ticks: u64) -> Result<u64, String> {
    run_headless_with(IdleSim, ticks)
}

/// [`run_headless`] with any [`SimStep`].
pub fn run_headless_with<S: SimStep>(sim: S, ticks: u64) -> Result<u64, String> {
    let (_producer, consumer) = pin_core::spsc::channel::<InputEvent>(1);
    let (writer, mut reader) = TripleBuffer::new(FrameStateSnapshot::default());
    let mut sim_loop = SimLoop::new(sim, consumer, writer);
    let mut last = 0u64;
    for _ in 0..ticks {
        sim_loop.run_ticks(1);
        let got = reader.read().tick;
        if got != last + 1 {
            return Err(format!(
                "non-monotonic snapshot: expected tick {}, read back {got}",
                last + 1
            ));
        }
        last = got;
    }
    Ok(last)
}

/// Hybrid sleep + spin wait for the real-time sim path: sleeps while more
/// than `spin_ns` remain (OS sleeps overshoot), then spins.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct HybridTimer {
    /// Final stretch spent spinning, nanoseconds.
    pub spin_ns: u64,
}

impl Default for HybridTimer {
    fn default() -> Self {
        Self { spin_ns: 300_000 }
    }
}

impl HybridTimer {
    /// Waits until `now() ≥ deadline_ns` or `stop()` is true (checked at
    /// least every sleep slice). Returns the clock reading at exit.
    pub fn wait_until(
        &self,
        deadline_ns: u64,
        now: impl Fn() -> u64,
        stop: impl Fn() -> bool,
    ) -> u64 {
        loop {
            let t = now();
            if t >= deadline_ns || stop() {
                return t;
            }
            let remaining = deadline_ns - t;
            if remaining > self.spin_ns {
                std::thread::sleep(core::time::Duration::from_nanos(remaining - self.spin_ns));
            } else {
                std::hint::spin_loop();
            }
        }
    }
}

#[cfg(test)]
mod tests;
