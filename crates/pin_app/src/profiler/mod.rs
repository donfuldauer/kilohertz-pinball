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
//! (`raw − epoch`, saturating at 0; tick `k` ends at `k · TICK_NS`,
//! [`SimLoop::run_ticks`](crate::orchestrator::SimLoop::run_ticks)), so
//! the simulation applies an input in the tick that delivers it. The
//! re-stamp loses the SDL stamp of an event stamped before the epoch (queued
//! before the sim started, or stamped in time the catch-up cap dropped), so
//! the latency is not measured on the re-stamped value: in the app the loop
//! itself records it, from the **raw** SDL stamp to the SDL clock reading at
//! the start of the tick that delivers (and applies) the event
//! ([`SimLoop::run_ticks_profiled`](crate::orchestrator::SimLoop::run_ticks_profiled),
//! [`LoopProfiler::record_inputs`]), and the step wrapper records tick time
//! only ([`ProfiledSim::tick_time_only`]). A bare [`ProfiledSim`] (no loop
//! epoch, so the stamps it sees are raw) records the latency itself.
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
//!   ticks, and input latency on the raw SDL stamps when the loop measures
//!   it ([`SimLoop::run_ticks_profiled`](crate::orchestrator::SimLoop::run_ticks_profiled)).
//! * [`PresentProfiler`] (presenter / render thread): GPU frame time.
//! * [`ProfilerReader`] (HUD thread, any number): [`ProfilerSnapshot`]s.
//!
//! The writers are neither `Clone` nor `Copy` and record through `&mut
//! self`, so each metric has exactly one writing thread (input latency can
//! be recorded through [`SimProfiler`] or [`LoopProfiler`], both owned by the
//! sim thread; a configuration uses one of them,
//! [`ProfiledSim::tick_time_only`]). The storage is
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
/// **Clock domain.** Both values must be on the same clock: in the app the
/// SDL event timestamp clock (`SDL_GetTicksNS`, `ThreadsConfig::now_ns`).
/// `event_ts_ns` is the event's **raw** SDL stamp, not the tick-clock value
/// the sim loop hands the step (`raw − epoch`, saturating at 0,
/// [`SimLoop::run_ticks`](crate::orchestrator::SimLoop::run_ticks)), which
/// would shorten the latency of an event stamped before the epoch by
/// `epoch − raw`; `tick_start_ns` is the SDL clock reading at the start of
/// the applying tick. The loop measures exactly that
/// ([`SimLoop::run_ticks_profiled`](crate::orchestrator::SimLoop::run_ticks_profiled)).
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

impl Shared {
    /// Records one input-latency sample ([`input_latency`]; a clamped one
    /// is counted).
    #[inline]
    fn record_input(&self, event_ts_ns: u64, tick_start_ns: u64) -> InputLatency {
        let l = input_latency(event_ts_ns, tick_start_ns);
        if l.clamped {
            self.input_clamped.fetch_add(1, Ordering::Relaxed);
        }
        self.input.record(l.ns);
        l
    }

    /// Records every coil-driving button event ([`drives_coil`]) of
    /// `inputs` against `tick_start_ns`.
    #[inline]
    fn record_inputs(&self, inputs: &[InputEvent], tick_start_ns: u64) {
        for ev in inputs {
            if let InputEventKind::Button { input, .. } = ev.kind
                && drives_coil(input)
            {
                self.record_input(ev.timestamp_ns, tick_start_ns);
            }
        }
    }
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
        self.shared.record_input(event_ts_ns, tick_start_ns)
    }

    /// Records the latency of every coil-driving button event
    /// ([`drives_coil`]) of `inputs`, the inputs of the tick that started at
    /// `tick_start_ns`. Axis events and other buttons are skipped.
    #[inline]
    pub fn record_inputs(&mut self, inputs: &[InputEvent], tick_start_ns: u64) {
        self.shared.record_inputs(inputs, tick_start_ns);
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
/// ([`SimLoop::input_overflow_ticks`](crate::orchestrator::SimLoop::input_overflow_ticks))
/// and, when the loop measures it
/// ([`SimLoop::run_ticks_profiled`](crate::orchestrator::SimLoop::run_ticks_profiled)),
/// the input latency from each event's raw SDL stamp (REQ-LAT-005). O(1)
/// per sample, lock-free, allocation-free.
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

    /// Records the latency of an event with the **raw** SDL stamp
    /// `event_ts_ns` applied by the tick whose SDL clock reading at its
    /// start is `tick_start_ns` ([`input_latency`]: a future stamp records
    /// 0 and is counted). Returns the measurement.
    #[inline]
    pub fn record_input(&mut self, event_ts_ns: u64, tick_start_ns: u64) -> InputLatency {
        self.shared.record_input(event_ts_ns, tick_start_ns)
    }

    /// Records the latency of every coil-driving button event
    /// ([`drives_coil`]) of `inputs` (raw SDL stamps) against the SDL clock
    /// reading `tick_start_ns` at the start of their applying tick. Axis
    /// events and other buttons are skipped.
    #[inline]
    pub fn record_inputs(&mut self, inputs: &[InputEvent], tick_start_ns: u64) {
        self.shared.record_inputs(inputs, tick_start_ns);
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
/// inner step, records the tick time and, unless
/// [`tick_time_only`](Self::tick_time_only), the latency of the tick's
/// coil-driving inputs against the first reading, and publishes the inner
/// step's queue overflow total. It forwards `tick`, `inputs` and `out`
/// untouched, so the simulation computes exactly what it computes unwrapped
/// (same `state_hash`). Allocation-free, lock-free and I/O-free when
/// `clock` is.
///
/// The input latency it records is only right when the stamps it sees are
/// raw SDL stamps (a loop with epoch 0). Behind a sim loop with an epoch
/// the step sees re-stamped values, so there the loop records the latency
/// ([`SimLoop::run_ticks_profiled`](crate::orchestrator::SimLoop::run_ticks_profiled))
/// and the wrapper is [`tick_time_only`](Self::tick_time_only), as
/// `ThreadsConfig::with_profiled_sim` wires it.
pub struct ProfiledSim<S, C> {
    sim: S,
    clock: C,
    profiler: SimProfiler,
    record_inputs: bool,
}

impl<S, C> ProfiledSim<S, C>
where
    S: SimStep + CommandQueueOverflows,
    C: FnMut() -> u64 + Send,
{
    /// Wraps `sim`, timing it with `clock` into `profiler`; records tick
    /// time and the input latency of the stamps the step receives.
    pub fn new(sim: S, profiler: SimProfiler, clock: C) -> Self {
        Self {
            sim,
            clock,
            profiler,
            record_inputs: true,
        }
    }

    /// Records tick time (and the queue overflow total) only, no input
    /// latency: for a step behind a sim loop that re-stamps its events onto
    /// the tick clock and records the latency itself from the raw SDL
    /// stamps ([`SimLoop::run_ticks_profiled`](crate::orchestrator::SimLoop::run_ticks_profiled),
    /// REQ-LAT-005).
    #[must_use]
    pub fn tick_time_only(mut self) -> Self {
        self.record_inputs = false;
        self
    }

    /// Whether the wrapper records input latency (`false` after
    /// [`tick_time_only`](Self::tick_time_only)).
    pub fn records_inputs(&self) -> bool {
        self.record_inputs
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
        let start = (self.clock)();
        if self.record_inputs {
            self.profiler.record_inputs(inputs, start);
        }
        self.sim.step_1ms(tick, inputs, out);
        let end = (self.clock)();
        self.profiler.record_tick(start, end);
        self.profiler
            .set_script_queue_overflows(self.sim.command_queue_overflows());
    }
}

#[cfg(test)]
mod tests;
