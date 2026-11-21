//! Latency profiler (Subphase 7.1): the HUD data model and its
//! renderer-agnostic text.
//!
//! Metrics (PLAN 7.1):
//!
//! * **Tick time**: wall-clock ns of one `step_1ms`, judged at the 99.9th
//!   percentile against [`TICK_P99_9_TARGET_NS`] (strictly below 0.35 ms).
//! * **Input latency**: ns from an input event's SDL timestamp
//!   ([`InputEvent::timestamp_ns`]) to the start of the tick in which the
//!   input took effect (for a flipper, the tick its coil is energised;
//!   REQ-LAT-005), judged at the 99th percentile against
//!   [`INPUT_P99_TARGET_NS`] (at most 1.5 ms). See [`input_latency`] for
//!   the clock domain and the clamping of future timestamps.
//!
//! # Clock domain (REQ-LAT-004, REQ-LAT-005)
//!
//! The sim loop gates queued events on their raw SDL timestamp against its
//! epoch and hands the step each event re-stamped onto the **tick clock**
//! (`raw − epoch`; tick `k` ends at `k · TICK_NS`,
//! [`SimLoop::run_ticks`](crate::orchestrator::SimLoop::run_ticks)), so
//! the simulation applies an input in the tick that delivers it. The event
//! timestamps [`ProfiledSim`] sees are therefore tick-clock values, and
//! [`ProfiledSim`] reads its tick start on the same clock: `clock() − epoch`,
//! with the epoch shared by the sim thread
//! ([`ProfiledSim::with_epoch`]; 0 without one). Both sides of the latency
//! are on the tick clock, and their difference equals SDL now minus the raw
//! SDL stamp.
//! * **GPU frame time**: ns per presented frame, fed from the presenter side
//!   (p50 / p99 / max).
//! * **Command-queue overflows**: the script command queue's overflow count
//!   (kept by `pin_sim`, [`pin_sim::Simulation::script_queue_overflows`];
//!   the queue struct itself is unchanged, PLAN §3.4) plus the orchestrator's
//!   input-overflow ticks
//!   ([`SimLoop::input_overflow_ticks`](crate::orchestrator::SimLoop::input_overflow_ticks)).
//!
//! # Threads and the single-writer design
//!
//! [`Profiler::new`] allocates everything once and hands out one writer per
//! metric group plus a cloneable reader:
//!
//! * [`SimProfiler`] (sim thread): tick time, input latency, script queue
//!   overflows. [`ProfiledSim`] wraps any [`SimStep`] and feeds it.
//! * [`LoopProfiler`] (sim thread, the loop around the step): input-overflow
//!   ticks.
//! * [`PresentProfiler`] (presenter / render thread): GPU frame time.
//! * [`ProfilerReader`] (HUD thread, any number): [`ProfilerSnapshot`]s.
//!
//! The writers are neither `Clone` nor `Copy` and record through `&mut
//! self`, so each metric has exactly one writing thread. The storage is
//! relaxed atomics rather than a snapshot hand-off (a triple buffer of the
//! histograms): a record is a handful of uncontended relaxed atomic
//! updates, O(1), with no lock, no allocation and no copy of the 30 KiB
//! histogram per tick, and the reader never makes the writer wait. The
//! price is that a snapshot taken while a writer records may combine
//! counters from adjacent samples (a count one ahead of a percentile walk);
//! the percentile walk handles that ([`histogram::percentile_from_counts`]),
//! and a HUD refreshed a few times per second does not need a consistent
//! cut. No metric feeds back into the simulation: the profiler only reads
//! the clock and the inputs, so it cannot change simulation output or the
//! replay hash.

pub mod histogram;
mod hud;

pub use histogram::{HistogramSummary, LatencyHistogram, RELATIVE_ERROR_BOUND};
pub use hud::{
    FrameRateMeter, HudRates, format_fps, format_ms, hud_lines, hud_lines_with_rates,
    hud_rate_lines,
};

use crate::orchestrator::{IdleSim, TableSim};
use core::sync::atomic::{AtomicU64, Ordering};
use pin_core::{CabinetInput, FrameStateSnapshot, InputEvent, InputEventKind, SimStep};
use std::sync::Arc;

/// Tick-time target: the 99.9th percentile must be strictly below this
/// (0.35 ms).
pub const TICK_P99_9_TARGET_NS: u64 = 350_000;
/// Input-latency target: the 99th percentile must be at most this (1.5 ms).
pub const INPUT_P99_TARGET_NS: u64 = 1_500_000;

/// Whether a tick-time p99.9 meets the target (`p99_9 < 0.35 ms`, strict).
#[must_use]
pub const fn tick_target_met(p99_9_ns: u64) -> bool {
    p99_9_ns < TICK_P99_9_TARGET_NS
}

/// Whether an input-latency p99 meets the target (`p99 ≤ 1.5 ms`).
#[must_use]
pub const fn input_target_met(p99_ns: u64) -> bool {
    p99_ns <= INPUT_P99_TARGET_NS
}

/// One input-latency measurement ([`input_latency`]).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct InputLatency {
    /// Latency, ns (0 when clamped).
    pub ns: u64,
    /// The event's timestamp was later than the tick start (a negative
    /// latency), so the sample was clamped to 0.
    pub clamped: bool,
}

/// The latency of an input event stamped `event_ts_ns` applied by the tick
/// that started at `tick_start_ns`.
///
/// **Clock domain.** Both values must be on the same clock. In the app
/// that is the sim loop's tick clock: the loop hands the step every event
/// re-stamped to `raw SDL stamp − epoch`
/// ([`SimLoop::run_ticks`](crate::orchestrator::SimLoop::run_ticks),
/// [`SimLoop::set_epoch_ns`](crate::orchestrator::SimLoop::set_epoch_ns)),
/// and [`ProfiledSim`] reads `tick_start_ns` as `clock() − epoch`, where
/// `clock` is the SDL event timestamp clock (`SDL_GetTicksNS`,
/// `ThreadsConfig::now_ns`) and the epoch the loop's, shared through
/// [`ProfiledSim::with_epoch`]. The difference is SDL now minus the raw SDL
/// stamp. With epoch 0 both are plain SDL clock values.
///
/// **Applying tick (REQ-LAT-005).** The tick that delivers an event is the
/// tick in which the simulation applies it (for a flipper button, the tick
/// its coil is energised or released), so the latency runs to the start of
/// the tick in which the input took effect, not to an earlier tick in which
/// the loop merely received it.
///
/// **Future timestamps.** An event stamped after the tick start (a negative
/// latency: clocks out of step, or a loop running ahead of real time, as a
/// headless run does) is clamped to 0 ns and flagged `clamped`; the profiler
/// records the 0 and counts it ([`ProfilerSnapshot::input_clamped`]), so a
/// clock problem is visible on the HUD instead of hidden. An old timestamp
/// gives a large positive latency, which is recorded as is (saturating the
/// histogram above about 68.7 s).
#[must_use]
pub const fn input_latency(event_ts_ns: u64, tick_start_ns: u64) -> InputLatency {
    if event_ts_ns > tick_start_ns {
        InputLatency {
            ns: 0,
            clamped: true,
        }
    } else {
        InputLatency {
            ns: tick_start_ns - event_ts_ns,
            clamped: false,
        }
    }
}

/// Whether `input` drives a coil directly when pressed or released (the
/// flipper buttons energise their flipper coils, LAUNCH the auto-plunger),
/// so its events count for the input-latency metric. ACTION, START and the
/// tilt bob reach coils only through the rules, if at all, and are not
/// measured.
#[must_use]
pub const fn drives_coil(input: CabinetInput) -> bool {
    matches!(
        input,
        CabinetInput::LeftFlipper
            | CabinetInput::RightFlipper
            | CabinetInput::LeftUpper
            | CabinetInput::RightUpper
            | CabinetInput::Launch
    )
}

/// Storage shared by the writers and readers.
#[derive(Debug)]
struct Shared {
    tick: LatencyHistogram,
    input: LatencyHistogram,
    gpu: LatencyHistogram,
    input_clamped: AtomicU64,
    script_queue_overflows: AtomicU64,
    input_overflow_ticks: AtomicU64,
}

/// The profiler: [`Profiler::new`] returns its writers and first reader.
#[derive(Debug)]
pub struct Profiler {
    /// The sim-thread writer (tick time, input latency, script queue
    /// overflows).
    pub sim: SimProfiler,
    /// The sim-loop writer (input-overflow ticks).
    pub sim_loop: LoopProfiler,
    /// The presenter writer (GPU frame time).
    pub present: PresentProfiler,
    /// A reader (clone it for more).
    pub reader: ProfilerReader,
}

impl Default for Profiler {
    fn default() -> Self {
        Self::new()
    }
}

impl Profiler {
    /// Allocates the three histograms and the counters (the only allocation
    /// the profiler makes) and returns one writer per metric group and a
    /// reader.
    #[must_use]
    pub fn new() -> Profiler {
        let shared = Arc::new(Shared {
            tick: LatencyHistogram::new(),
            input: LatencyHistogram::new(),
            gpu: LatencyHistogram::new(),
            input_clamped: AtomicU64::new(0),
            script_queue_overflows: AtomicU64::new(0),
            input_overflow_ticks: AtomicU64::new(0),
        });
        Profiler {
            sim: SimProfiler {
                shared: Arc::clone(&shared),
            },
            sim_loop: LoopProfiler {
                shared: Arc::clone(&shared),
            },
            present: PresentProfiler {
                shared: Arc::clone(&shared),
            },
            reader: ProfilerReader { shared },
        }
    }
}

/// The sim thread's writer: tick time, input latency and the script command
/// queue's overflow count. Every method is O(1), lock-free and
/// allocation-free (callable from `step_1ms`, D5).
#[derive(Debug)]
pub struct SimProfiler {
    shared: Arc<Shared>,
}

impl SimProfiler {
    /// Records one tick's duration, ns.
    #[inline]
    pub fn record_tick_ns(&mut self, ns: u64) {
        self.shared.tick.record(ns);
    }

    /// Records a tick that started at `start_ns` and ended at `end_ns` (same
    /// clock; an end before the start records 0).
    #[inline]
    pub fn record_tick(&mut self, start_ns: u64, end_ns: u64) {
        self.record_tick_ns(end_ns.saturating_sub(start_ns));
    }

    /// Records the latency of an event stamped `event_ts_ns` applied by the
    /// tick that started at `tick_start_ns` ([`input_latency`]: a future
    /// timestamp records 0 and is counted). Returns the measurement.
    #[inline]
    pub fn record_input(&mut self, event_ts_ns: u64, tick_start_ns: u64) -> InputLatency {
        let l = input_latency(event_ts_ns, tick_start_ns);
        if l.clamped {
            self.shared.input_clamped.fetch_add(1, Ordering::Relaxed);
        }
        self.shared.input.record(l.ns);
        l
    }

    /// Records the latency of every coil-driving button event
    /// ([`drives_coil`]) of `inputs`, the inputs of the tick that started at
    /// `tick_start_ns`. Axis events and other buttons are skipped.
    #[inline]
    pub fn record_inputs(&mut self, inputs: &[InputEvent], tick_start_ns: u64) {
        for ev in inputs {
            if let InputEventKind::Button { input, .. } = ev.kind
                && drives_coil(input)
            {
                self.record_input(ev.timestamp_ns, tick_start_ns);
            }
        }
    }

    /// Publishes the script command queue's overflow total (`pin_sim`'s
    /// monotonic counter). A smaller value than already published is
    /// ignored: the HUD count never goes down.
    #[inline]
    pub fn set_script_queue_overflows(&mut self, total: u64) {
        self.shared
            .script_queue_overflows
            .fetch_max(total, Ordering::Relaxed);
    }
}

/// The sim loop's writer: the orchestrator's input-overflow ticks
/// ([`SimLoop::input_overflow_ticks`](crate::orchestrator::SimLoop::input_overflow_ticks)).
/// O(1), lock-free, allocation-free.
#[derive(Debug)]
pub struct LoopProfiler {
    shared: Arc<Shared>,
}

impl LoopProfiler {
    /// Publishes the input-overflow tick total (monotonic: a smaller value
    /// is ignored).
    #[inline]
    pub fn set_input_overflow_ticks(&mut self, total: u64) {
        self.shared
            .input_overflow_ticks
            .fetch_max(total, Ordering::Relaxed);
    }
}

/// The presenter's writer: GPU frame time. O(1), lock-free,
/// allocation-free.
#[derive(Debug)]
pub struct PresentProfiler {
    shared: Arc<Shared>,
}

impl PresentProfiler {
    /// Records one frame's GPU time, ns.
    #[inline]
    pub fn record_frame_ns(&mut self, ns: u64) {
        self.shared.gpu.record(ns);
    }

    /// Records a frame from `start_ns` to `end_ns` (same clock; an end
    /// before the start records 0).
    #[inline]
    pub fn record_frame(&mut self, start_ns: u64, end_ns: u64) {
        self.record_frame_ns(end_ns.saturating_sub(start_ns));
    }
}

/// A reader of the profiler (any thread, any number of clones).
#[derive(Clone, Debug)]
pub struct ProfilerReader {
    shared: Arc<Shared>,
}

impl ProfilerReader {
    /// The current values of every metric (cumulative since
    /// [`Profiler::new`]). Copies one histogram (30 KiB) at a time to the stack;
    /// meant for the HUD thread, not the sim thread.
    #[must_use]
    pub fn snapshot(&self) -> ProfilerSnapshot {
        let s = &self.shared;
        ProfilerSnapshot {
            tick: s.tick.summary(),
            input: s.input.summary(),
            input_clamped: s.input_clamped.load(Ordering::Relaxed),
            gpu_frame: s.gpu.summary(),
            script_queue_overflows: s.script_queue_overflows.load(Ordering::Relaxed),
            input_overflow_ticks: s.input_overflow_ticks.load(Ordering::Relaxed),
        }
    }
}

/// Plain-data values of every profiler metric (cumulative window), for the
/// HUD ([`hud_lines`]) and reports.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub struct ProfilerSnapshot {
    /// Tick time, ns.
    pub tick: HistogramSummary,
    /// Input latency (SDL event timestamp to the start of the tick in which
    /// the input took effect, REQ-LAT-005), ns.
    pub input: HistogramSummary,
    /// Input events whose timestamp was after the tick start (recorded as
    /// 0 ns).
    pub input_clamped: u64,
    /// GPU frame time, ns.
    pub gpu_frame: HistogramSummary,
    /// The script command queue's overflow total (`pin_sim`).
    pub script_queue_overflows: u64,
    /// Ticks at which the orchestrator's per-tick input buffer overflowed.
    pub input_overflow_ticks: u64,
}

impl ProfilerSnapshot {
    /// The tick-time verdict: `Some(p99.9 < 0.35 ms)`, `None` without
    /// samples.
    #[must_use]
    pub const fn tick_target_met(&self) -> Option<bool> {
        if self.tick.has_samples() {
            Some(tick_target_met(self.tick.p99_9))
        } else {
            None
        }
    }

    /// The input-latency verdict: `Some(p99 ≤ 1.5 ms)`, `None` without
    /// samples.
    #[must_use]
    pub const fn input_target_met(&self) -> Option<bool> {
        if self.input.has_samples() {
            Some(input_target_met(self.input.p99))
        } else {
            None
        }
    }

    /// Command-queue overflows: script queue overflows plus input-overflow
    /// ticks (saturating).
    #[must_use]
    pub const fn command_queue_overflows(&self) -> u64 {
        self.script_queue_overflows
            .saturating_add(self.input_overflow_ticks)
    }
}

/// A step whose script command queue overflow count the profiler can
/// publish.
pub trait CommandQueueOverflows {
    /// The overflow total so far (monotonic).
    fn command_queue_overflows(&self) -> u64;
}

impl CommandQueueOverflows for pin_sim::Simulation {
    fn command_queue_overflows(&self) -> u64 {
        u64::from(self.script_queue_overflows())
    }
}

impl CommandQueueOverflows for TableSim {
    fn command_queue_overflows(&self) -> u64 {
        self.sim().command_queue_overflows()
    }
}

impl CommandQueueOverflows for IdleSim {
    fn command_queue_overflows(&self) -> u64 {
        0
    }
}

/// A boxed step publishes its contents' count, so the app can profile the
/// `Box<TableSim>` it loads on a big stack without moving the simulation
/// out of the box.
impl<T: CommandQueueOverflows + ?Sized> CommandQueueOverflows for Box<T> {
    fn command_queue_overflows(&self) -> u64 {
        (**self).command_queue_overflows()
    }
}

/// A [`SimStep`] wrapper that profiles every tick: it reads `clock` (ns, the
/// SDL event timestamp clock, see [`input_latency`]) before and after the
/// inner step, moves both readings onto the tick clock by subtracting the
/// shared epoch ([`with_epoch`](Self::with_epoch); 0 without one), records
/// the tick time and the latency of the tick's coil-driving inputs, and
/// publishes the inner step's queue overflow total. It forwards `tick`,
/// `inputs` and `out` untouched, so the simulation computes exactly what
/// it computes unwrapped (same `state_hash`). Allocation-free, lock-free
/// and I/O-free when `clock` is (one relaxed atomic load per tick for the
/// epoch).
pub struct ProfiledSim<S, C> {
    sim: S,
    clock: C,
    profiler: SimProfiler,
    epoch_ns: Option<Arc<AtomicU64>>,
}

impl<S, C> ProfiledSim<S, C>
where
    S: SimStep + CommandQueueOverflows,
    C: FnMut() -> u64 + Send,
{
    /// Wraps `sim`, timing it with `clock` into `profiler`. The epoch is 0
    /// (the tick clock is `clock` itself) until [`with_epoch`](Self::with_epoch).
    pub fn new(sim: S, profiler: SimProfiler, clock: C) -> Self {
        Self {
            sim,
            clock,
            profiler,
            epoch_ns: None,
        }
    }

    /// Reads the tick clock as `clock() − epoch_ns` (saturating; one relaxed
    /// load per tick), where `epoch_ns` is the sim loop's epoch on `clock`,
    /// stored by the sim thread after
    /// [`SimLoop::set_epoch_ns`](crate::orchestrator::SimLoop::set_epoch_ns)
    /// and after every
    /// [`SimLoop::skip_ticks`](crate::orchestrator::SimLoop::skip_ticks).
    /// The loop hands the step its events on that tick clock, so the input
    /// latency compares values of one clock domain (REQ-LAT-005,
    /// [`input_latency`]).
    #[must_use]
    pub fn with_epoch(mut self, epoch_ns: Arc<AtomicU64>) -> Self {
        self.epoch_ns = Some(epoch_ns);
        self
    }

    /// The wrapped step.
    pub fn inner(&self) -> &S {
        &self.sim
    }

    /// The wrapped step, mutably.
    pub fn inner_mut(&mut self) -> &mut S {
        &mut self.sim
    }

    /// Unwraps the step and the writer.
    pub fn into_parts(self) -> (S, SimProfiler) {
        (self.sim, self.profiler)
    }
}

impl<S, C> SimStep for ProfiledSim<S, C>
where
    S: SimStep + CommandQueueOverflows,
    C: FnMut() -> u64 + Send,
{
    fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
        let epoch = self
            .epoch_ns
            .as_ref()
            .map_or(0, |e| e.load(Ordering::Relaxed));
        let start = (self.clock)().saturating_sub(epoch);
        self.profiler.record_inputs(inputs, start);
        self.sim.step_1ms(tick, inputs, out);
        let end = (self.clock)().saturating_sub(epoch);
        self.profiler.record_tick(start, end);
        self.profiler
            .set_script_queue_overflows(self.sim.command_queue_overflows());
    }
}

#[cfg(test)]
mod tests;
