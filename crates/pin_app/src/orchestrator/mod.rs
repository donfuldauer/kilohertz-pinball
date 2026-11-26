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
//! * [`load_validated`] / [`TableSim`] (Subphase 6.0): a table bundle
//!   validated (manifests, 2.8 clearance for every ball profile) and loaded
//!   into the real [`pin_sim::Simulation`], driven by the sim thread instead
//!   of [`IdleSim`]. Validation and the simulation use one immutable
//!   [`BundleSnapshot`](pin_sim::load::BundleSnapshot) of the bundle
//!   ([`validate_table`] + [`load_checked`]).
//! * Wiring (6.0): [`TableSim`] pushes each tick's haptic impulses, sound
//!   triggers and display commands into wait-free rings; the audio thread
//!   mixes the sounds (3.3 mixer) with the SSF exciter channels
//!   ([`AudioEngine`]) into an [`AudioSink`] (SDL device, offline recording
//!   or null); the render thread draws the table with the 5.1 PBR renderer
//!   and the 5.3 displays ([`TableRenderer`], presented by [`AppSurfaces`]:
//!   directly for the playfield and non-blocking secondary windows, through a
//!   per-surface presenter thread and a latest-frame hand-off for blocking
//!   (`Fifo`-only) backglass / score windows, D19).
//! * Profiler wiring (7.1 hooks): [`ThreadsConfig::with_profiled_sim`]
//!   wraps the step in a tick-time-only
//!   [`ProfiledSim`](crate::profiler::ProfiledSim) on the sim loop's clock
//!   and hands the loop writer to the sim thread (input latency from the
//!   raw SDL stamps, [`SimLoop::run_ticks_profiled`], and input-overflow
//!   ticks) and the GPU frame-time writer to the render
//!   thread ([`GpuFrameTimer`], timestamp queries only); [`HudFeed`] and
//!   [`HudOverlay`] draw the opt-in HUD (`--profiler-hud`). The debug view
//!   gets the same timing and HUD through [`DebugViewHooks`] (w8g-d).

mod audio;
mod gpu_timer;
mod handoff;
mod hud;
mod present;
mod presenter;
mod table;
mod threads;
pub mod trace;
mod view;

pub use audio::{
    AUDIO_BLOCK_FRAMES, AudioEngine, AudioSink, DEVICE_QUEUE_BLOCKS, DeviceSink, DeviceStream,
    NullSink, OfflineRecording, OfflineSink, RegisteredSound, ssf_playfield_mm, table_samples,
};
pub use gpu_timer::{GPU_TIMER_RING, GpuFrameTimer, gpu_sample_ns};
pub use handoff::{HANDOFF_SLOTS, HandoffStats};
pub use hud::{DebugViewHooks, HudFeed, HudOverlay, hud_format_supported};
pub use present::{
    AppSurfaces, SlotDecision, SlotInfo, SlotPath, SurfacePlan, is_blocking_mode, plan_surface,
    slot_decision, table_set_accepted,
};
pub use presenter::{CopyPath, FrameCopy, offscreen_frame_texture};
pub use table::{
    DISPLAY_RING_CAPACITY, DisplayCommand, FlipperOutline, IO_WORKER_NOT_RUNNING,
    IO_WORKER_STOPPED, IoReply, IoRequest, IoWorker, LoadedApp, SIM_STACK_BYTES,
    SOUND_RING_CAPACITY, TABLE_LINE_COLOR, TableSim, ValidatedTable, load_app_table, load_checked,
    load_validated, on_sim_stack, run_headless_table, table_lines, table_source_positions,
    validate_snapshot, validate_table,
};
pub use threads::{
    AudioConfig, AudioStats, ClosePolicy, DropMonitor, FLIPPER_LINE_COLOR, HUD_REFRESH_NS,
    OrchestratorStats, RenderConfig, RenderGpu, RunExit, RunLimits, StatsSnapshot, Threads,
    ThreadsConfig, debug_view_hooks, default_ball_radius, monotonic_now_ns, spawn_threads,
};
pub use view::{
    DisplayAssets, MAX_OVERLAY_TABLE_LINES, TableRenderOptions, TableRenderer, TableVisuals,
    VisualMesh, load_scene_images, routes_for_window, table_camera, vertex_normals,
};

use crate::profiler::LoopProfiler;
use arrayvec::ArrayVec;
use pin_core::sim::tick_end_ns;
use pin_core::spsc::SpscConsumer;
use pin_core::{
    FrameStateSnapshot, InputEvent, MAX_CATCH_UP_TICKS, SimStep, TICK_NS, TripleBuffer,
    TripleWriter,
};

/// Capacity of the per-tick input buffer of [`SimLoop`]. If more events are
/// due in one tick, the rest stay queued and are delivered in a later
/// executed tick (the next one, or later ones when more than this many
/// remain).
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

/// The stub [`SimStep`] used when no table is loaded (`pin_app` without
/// `--table`): sets `out.tick = tick`, otherwise the default snapshot.
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
    trace: Option<trace::SimTraceWriter>,
    late_trace_refusals: u64,
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
            trace: None,
            late_trace_refusals: 0,
        }
    }

    /// Installs the sim thread's latency-trace writer (`--latency-trace`,
    /// REQ-LAT-003): from now on every executed tick records its scheduling
    /// marks, every delivered button event a row and every watched press
    /// its bat's first move ([`trace::SimTraceWriter`]; allocation-free).
    /// Without it (the default) nothing is recorded and the ticks are
    /// unchanged.
    ///
    /// **Precondition: install before the first tick.** The writer starts
    /// at the epoch: the first executed tick records the epoch as its
    /// previous tick end and drain, which is right only for tick 1. Once a
    /// tick has run ([`tick`](Self::tick) `!= 0`) the call is refused: the
    /// writer is dropped uninstalled, any trace already installed keeps
    /// recording unchanged, and the refusal is counted
    /// ([`late_trace_refusals`](Self::late_trace_refusals)). It never
    /// panics. Installing at tick 0 works with or without a later
    /// [`set_epoch_ns`](Self::set_epoch_ns), which restarts the writer at
    /// the new epoch.
    pub fn set_latency_trace(&mut self, mut writer: trace::SimTraceWriter) {
        if self.tick != 0 {
            self.late_trace_refusals = self.late_trace_refusals.saturating_add(1);
            return;
        }
        writer.start(self.epoch_ns);
        self.trace = Some(writer);
    }

    /// Whether a latency-trace writer is installed
    /// ([`set_latency_trace`](Self::set_latency_trace)).
    pub fn latency_trace_installed(&self) -> bool {
        self.trace.is_some()
    }

    /// Calls to [`set_latency_trace`](Self::set_latency_trace) refused
    /// because a tick had already run (the writer was not installed).
    pub fn late_trace_refusals(&self) -> u64 {
        self.late_trace_refusals
    }

    /// Sets the simulated-time epoch: tick `k` ends at `epoch_ns + k · TICK_NS`
    /// on the input timestamp clock (SDL's ns clock in the app). The epoch
    /// only gates the raw timestamps in [`run_ticks`](Self::run_ticks); the
    /// step itself always sees the tick clock (tick `k` ends at
    /// `k · TICK_NS`), because every delivered event is re-stamped to
    /// `raw − epoch_ns` (REQ-LAT-004). An epoch of 0 (the default) makes the
    /// re-stamp the identity.
    pub fn set_epoch_ns(&mut self, epoch_ns: u64) {
        self.epoch_ns = epoch_ns;
        if let Some(t) = self.trace.as_mut() {
            t.start(epoch_ns);
        }
    }

    /// The simulated-time epoch.
    pub fn epoch_ns(&self) -> u64 {
        self.epoch_ns
    }

    /// Shifts simulated time forward by `ticks` dropped ticks (the catch-up
    /// cap was hit) so input timestamps keep mapping onto the wall clock.
    /// The epoch absorbs the drop: an event stamped inside the dropped time
    /// is re-stamped onto the tick clock at or before the end of the next
    /// executed tick, so that tick applies it (REQ-LAT-004).
    pub fn skip_ticks(&mut self, ticks: u64) {
        self.epoch_ns = self.epoch_ns.saturating_add(ticks.saturating_mul(TICK_NS));
        if let Some(t) = self.trace.as_mut() {
            t.ticks_dropped(ticks);
        }
    }

    /// Runs `n` ticks back-to-back on simulated time: tick `k`'s inputs are
    /// the queued events whose **raw** timestamp is `≤ epoch + k · TICK_NS`
    /// (events with a later timestamp stay queued); one snapshot is
    /// published per tick. Allocation-free.
    ///
    /// **Tick clock (REQ-LAT-004).** Each delivered event reaches the step
    /// re-stamped to `timestamp_ns − epoch` (saturating at 0), so the step
    /// sees the tick clock on which tick `k` ends at `k · TICK_NS`; kind and
    /// order are untouched. A re-stamped event is therefore never after the
    /// end of the tick that delivers it, and a simulation that gates on
    /// `tick_end_ns(0, tick)` applies it in that same tick. An event that
    /// reaches the ring after its own tick has run is delivered, and
    /// applied, in the next executed tick; after [`skip_ticks`](Self::skip_ticks)
    /// the same holds for an event stamped inside the dropped time. With
    /// epoch 0 the re-stamp is the identity.
    pub fn run_ticks(&mut self, n: u32) {
        self.run_ticks_inner(n, None::<(&mut LoopProfiler, &mut fn() -> u64)>);
    }

    /// [`run_ticks`](Self::run_ticks) that also measures input latency
    /// (REQ-LAT-005): at the start of each tick it reads `clock` (the input
    /// timestamp clock, SDL's ns clock in the app) once and records into
    /// `profiler`, for every coil-driving button event the tick delivers
    /// ([`drives_coil`](crate::profiler::drives_coil)), the latency from the
    /// event's **raw** timestamp to that reading
    /// ([`LoopProfiler::record_inputs`]). The delivering tick is the tick
    /// that applies the event, and the raw stamp is used even below the
    /// epoch (an event queued before the sim started, or stamped in dropped
    /// time), so the full SDL-clock latency is recorded where the re-stamped
    /// value would have saturated at 0. Delivery, the re-stamp and the
    /// step's inputs are exactly those of [`run_ticks`](Self::run_ticks).
    /// One clock read per tick; allocation-, lock- and I/O-free when `clock`
    /// is.
    pub fn run_ticks_profiled<C: FnMut() -> u64>(
        &mut self,
        n: u32,
        profiler: &mut LoopProfiler,
        mut clock: C,
    ) {
        self.run_ticks_inner(n, Some((profiler, &mut clock)));
    }

    fn run_ticks_inner<C: FnMut() -> u64>(
        &mut self,
        n: u32,
        mut probe: Option<(&mut LoopProfiler, &mut C)>,
    ) {
        for _ in 0..n {
            self.tick += 1;
            let end = tick_end_ns(self.epoch_ns, self.tick);
            // The SDL clock at the start of this (the applying) tick.
            let start = probe.as_mut().map_or(0, |(_, clock)| clock());
            if let Some(t) = self.trace.as_mut() {
                t.begin_tick(end, &self.snapshot);
            }
            self.inputs.clear();
            while !self.inputs.is_full() {
                match self.input.peek() {
                    Some(ev) if ev.timestamp_ns <= end => {
                        let _ = self.input.pop();
                        if let Some((profiler, _)) = probe.as_mut() {
                            // Latency from the raw SDL stamp (REQ-LAT-005).
                            profiler.record_inputs(core::slice::from_ref(&ev), start);
                        }
                        if let Some(t) = self.trace.as_mut() {
                            t.delivered(self.tick, &ev);
                        }
                        // Gate on the raw stamp, hand the step the tick clock.
                        self.inputs.push(InputEvent {
                            timestamp_ns: ev.timestamp_ns.saturating_sub(self.epoch_ns),
                            ..ev
                        });
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
            if let Some(t) = self.trace.as_mut() {
                t.end_tick(self.tick, &self.snapshot);
            }
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
    /// (the excess was deferred to a later executed tick).
    pub fn input_overflow_ticks(&self) -> u64 {
        self.input_overflow_ticks
    }

    /// The sim step.
    pub fn sim(&self) -> &S {
        &self.sim
    }

    /// The sim step, mutably (state injection between ticks in tests).
    pub fn sim_mut(&mut self) -> &mut S {
        &mut self.sim
    }

    /// Ends the loop, returning the sim step.
    pub fn into_sim(self) -> S {
        self.sim
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
    run_headless_returning(sim, ticks).map(|(last, _)| last)
}

/// [`run_headless_with`] that also returns the sim step afterwards (to read
/// its final state).
pub fn run_headless_returning<S: SimStep>(sim: S, ticks: u64) -> Result<(u64, S), String> {
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
    Ok((last, sim_loop.into_sim()))
}

/// Longest single sleep of [`HybridTimer::wait_until`] (50 ms), so a stop
/// request is seen promptly even before a far deadline.
pub const MAX_SLEEP_SLICE_NS: u64 = 50_000_000;

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
    /// least every sleep slice of at most [`MAX_SLEEP_SLICE_NS`]). Returns the clock reading at exit.
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
                let slice = (remaining - self.spin_ns).min(MAX_SLEEP_SLICE_NS);
                std::thread::sleep(core::time::Duration::from_nanos(slice));
            } else {
                std::hint::spin_loop();
            }
        }
    }
}

#[cfg(test)]
mod tests;

/// Input-epoch unit tests (issue #81, REQ-LAT-004 / REQ-LAT-005): the loop
/// gates on the raw stamp and hands the step the tick clock; it measures
/// input latency from the raw stamp.
#[cfg(test)]
mod epoch_tests {
    use super::*;
    use crate::profiler::{ProfiledSim, Profiler};
    use pin_core::CabinetInput;
    use pin_core::spsc::{SpscProducer, channel};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// Records, per tick, the delivered stamps; checks each against the
    /// tick-clock end of its tick.
    #[derive(Default)]
    struct Stamps {
        seen: Vec<(u64, Vec<u64>)>,
    }

    impl SimStep for Stamps {
        fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
            self.seen
                .push((tick, inputs.iter().map(|e| e.timestamp_ns).collect()));
            out.tick = tick;
        }
    }

    fn setup<S: SimStep>(
        sim: S,
        cap: usize,
    ) -> (
        SimLoop<S>,
        SpscProducer<InputEvent>,
        pin_core::TripleReader<FrameStateSnapshot>,
    ) {
        let (tx, rx) = channel::<InputEvent>(cap);
        let (w, r) = TripleBuffer::new(FrameStateSnapshot::default());
        (SimLoop::new(sim, rx, w), tx, r)
    }

    /// REQ-LAT-004 (a): with epoch E and D dropped ticks the step sees
    /// `raw − epoch` stamps, never above its tick's end on the tick clock;
    /// events stamped later stay queued; epoch 0 is the identity.
    #[test]
    fn req_lat_004_run_ticks_restamps_onto_the_tick_clock() {
        for epoch in [0u64, 1_000_000_000, 10_000_000_000] {
            for drops in [0u64, 120] {
                let (mut l, mut tx, _r) = setup(Stamps::default(), 64);
                l.set_epoch_ns(epoch);
                l.run_ticks(3);
                l.skip_ticks(drops);
                let e = l.epoch_ns();
                assert_eq!(e, epoch + drops * TICK_NS);
                // Ticks 4.. end at e + k ms (raw). Stamp presses mid tick 5
                // and 7, plus one exactly on tick 6's end.
                let raws = [
                    e + 4 * TICK_NS + TICK_NS / 2,
                    e + 6 * TICK_NS,
                    e + 6 * TICK_NS + TICK_NS / 2,
                ];
                for r in raws {
                    tx.push(InputEvent::button(r, CabinetInput::LeftFlipper, true))
                        .unwrap();
                }
                l.run_ticks(1); // tick 4: nothing due
                assert!(l.sim().seen[3].1.is_empty(), "later raw stamps stay queued");
                l.run_ticks(3); // ticks 5, 6, 7
                let got: Vec<_> = l.sim().seen[4..].to_vec();
                assert_eq!(
                    got,
                    vec![
                        (5, vec![4 * TICK_NS + TICK_NS / 2]),
                        (6, vec![6 * TICK_NS]),
                        (7, vec![6 * TICK_NS + TICK_NS / 2]),
                    ],
                    "epoch {epoch}, drops {drops}"
                );
                for (tick, stamps) in &l.sim().seen {
                    for s in stamps {
                        assert!(*s <= tick_end_ns(0, *tick), "{s} after tick {tick}");
                    }
                }
                if epoch == 0 && drops == 0 {
                    // Identity at epoch 0.
                    assert_eq!(got[0].1[0], raws[0]);
                }
            }
        }
    }

    /// REQ-LAT-004 (b): a raw stamp below the epoch re-stamps to 0 and is
    /// delivered in the next executed tick.
    #[test]
    fn req_lat_004_stamp_before_the_epoch_saturates_to_zero() {
        let (mut l, mut tx, _r) = setup(Stamps::default(), 8);
        l.set_epoch_ns(5_000_000_000);
        l.run_ticks(2);
        tx.push(InputEvent::button(7, CabinetInput::RightFlipper, true))
            .unwrap();
        l.run_ticks(1);
        assert_eq!(l.sim().seen[2], (3, vec![0]));
        assert_eq!(l.inputs_delivered(), 1);
    }

    /// REQ-LAT-004: an event pushed after its own tick ran is applied in
    /// the next executed tick, re-stamped at or before that tick's end;
    /// events inside skipped time land in the first tick after the skip.
    #[test]
    fn req_lat_004_late_and_dropped_time_events_go_to_the_next_tick() {
        let epoch = 1_000_000_000;
        let (mut l, mut tx, _r) = setup(Stamps::default(), 8);
        l.set_epoch_ns(epoch);
        l.run_ticks(10);
        // Stamped mid tick 3, arrives after tick 10 ran.
        tx.push(InputEvent::button(
            epoch + 2 * TICK_NS + 1,
            CabinetInput::LeftFlipper,
            true,
        ))
        .unwrap();
        l.run_ticks(1);
        assert_eq!(l.sim().seen[10], (11, vec![2 * TICK_NS + 1]));
        // 20 ms stall, cap 8: 12 dropped, skip before run. A press 5 ms
        // into the stall goes to the first catch-up tick, one 19 ms in to
        // the catch-up tick that ends at 19 ms.
        let stall_start = epoch + 11 * TICK_NS;
        for ms in [5u64, 19] {
            tx.push(InputEvent::button(
                stall_start + ms * TICK_NS,
                CabinetInput::LeftFlipper,
                ms == 5,
            ))
            .unwrap();
        }
        l.skip_ticks(12);
        l.run_ticks(8);
        let after: Vec<_> = l.sim().seen[11..]
            .iter()
            .filter(|(_, s)| !s.is_empty())
            .cloned()
            .collect();
        // After the skip, tick 12 ends (raw) 13 ms into the stall and tick
        // 18 ends 19 ms into it.
        assert_eq!(after.len(), 2, "{after:?}");
        assert_eq!(after[0].0, 12, "5 ms into the stall: first catch-up tick");
        assert_eq!(after[1].0, 18, "19 ms into the stall: tick ending at 19 ms");
        assert_eq!(
            tick_end_ns(l.epoch_ns(), 18),
            stall_start + 19 * TICK_NS,
            "tick 18 ends 19 ms into the stall"
        );
        for (tick, stamps) in &after {
            assert!(stamps[0] <= tick_end_ns(0, *tick));
        }
    }

    /// A stub flipper: energises its coil in the tick it receives the press
    /// (gating on the tick clock like the simulation: `ts ≤ tick_end_ns(0,
    /// tick)`, else it waits). Records the energise tick.
    #[derive(Default)]
    struct StubFlipper {
        pending: Vec<InputEvent>,
        energised_at: Option<u64>,
    }

    impl SimStep for StubFlipper {
        fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
            self.pending.extend_from_slice(inputs);
            let end = tick_end_ns(0, tick);
            if self.energised_at.is_none() && self.pending.iter().any(|e| e.timestamp_ns <= end) {
                self.energised_at = Some(tick);
            }
            out.tick = tick;
        }
    }

    impl crate::profiler::CommandQueueOverflows for StubFlipper {
        fn command_queue_overflows(&self) -> u64 {
            0
        }
    }

    /// REQ-LAT-005 (d): the profiler's latency sample is taken in the tick
    /// the flipper coil is energised, and equals SDL now − raw stamp, for
    /// raw stamps at or after the epoch (several epochs, with and without
    /// drops): the loop records it ([`SimLoop::run_ticks_profiled`]) and
    /// the tick-time-only step wrapper records tick time only.
    #[test]
    fn req_lat_005_latency_is_measured_to_the_energising_tick() {
        for (epoch, drops) in [
            (0u64, 0u64),
            (1_000_000_000, 0),
            (10_000_000_000, 120),
            (1_000_000_000, 5000),
        ] {
            let now = Arc::new(AtomicU64::new(epoch));
            let p = Profiler::new();
            let mut lp = p.sim_loop;
            let clk = Arc::clone(&now);
            let sim = ProfiledSim::new(StubFlipper::default(), p.sim, move || {
                clk.load(Ordering::Relaxed)
            })
            .tick_time_only();
            let (mut l, mut tx, _r) = setup(sim, 8);
            l.set_epoch_ns(epoch);
            l.skip_ticks(drops);
            let e = l.epoch_ns();
            let read = || now.load(Ordering::Relaxed);
            // Run to tick 500 with the clock at each tick's start.
            for k in 1..=500u64 {
                now.store(e + (k - 1) * TICK_NS, Ordering::Relaxed);
                l.run_ticks_profiled(1, &mut lp, read);
            }
            // Press mid tick 501 (the clock reads its end when 501 runs).
            let raw = e + 500 * TICK_NS + TICK_NS / 2;
            tx.push(InputEvent::button(raw, CabinetInput::LeftFlipper, true))
                .unwrap();
            now.store(e + 501 * TICK_NS, Ordering::Relaxed);
            l.run_ticks_profiled(1, &mut lp, read);
            let s = p.reader.snapshot();
            assert_eq!(
                l.sim().inner().energised_at,
                Some(501),
                "epoch {epoch} drops {drops}"
            );
            assert_eq!(s.input.count, 1, "sample taken in the energising tick");
            assert_eq!(s.input_clamped, 0);
            assert_eq!(s.input.max, e + 501 * TICK_NS - raw, "SDL now − raw stamp");
            assert!(s.input.p99 <= crate::profiler::INPUT_P99_TARGET_NS);
            assert_eq!(s.tick.count, 501, "the wrapper still records tick time");
        }
    }

    /// REQ-LAT-005 (F1, startup): epoch 1000 ms, a press stamped at 990 ms
    /// (queued before the sim started), applying tick 1 starts at 1001 ms:
    /// the recorded latency is the full SDL-clock 11 ms, not the 1 ms of
    /// the re-stamped value; not clamped. Delivery is unchanged (REQ-LAT-004):
    /// tick 1, re-stamped to 0.
    #[test]
    fn req_lat_005_stamp_before_the_epoch_at_startup_records_full_sdl_latency() {
        const MS: u64 = 1_000_000;
        let p = Profiler::new();
        let mut lp = p.sim_loop;
        let (mut l, mut tx, _r) = setup(Stamps::default(), 8);
        l.set_epoch_ns(1000 * MS);
        tx.push(InputEvent::button(
            990 * MS,
            CabinetInput::LeftFlipper,
            true,
        ))
        .unwrap();
        l.run_ticks_profiled(1, &mut lp, || 1001 * MS);
        assert_eq!(l.sim().seen[0], (1, vec![0]), "REQ-LAT-004 delivery");
        let s = p.reader.snapshot();
        assert_eq!(s.input.count, 1);
        assert_eq!(s.input.max, 11 * MS, "1001 ms − 990 ms, not 1001 − 1000");
        assert_eq!(s.input_clamped, 0);
    }

    /// REQ-LAT-005 (F1, dropped time): after a 30 ms stall the catch-up cap
    /// drops 22 ticks (`skip_ticks(22)`), so a press stamped 5 ms into the
    /// stall is below the new epoch. It is applied in the first catch-up
    /// tick (re-stamped to 0, REQ-LAT-004) and its latency is the full SDL
    /// now at that tick's start minus the raw stamp (25 ms), not the
    /// clipped `now − epoch` (18 ms).
    #[test]
    fn req_lat_005_stamp_in_dropped_time_records_full_sdl_latency() {
        for epoch in [0u64, 1_000_000_000, 10_000_000_000] {
            let p = Profiler::new();
            let mut lp = p.sim_loop;
            let (mut l, mut tx, _r) = setup(Stamps::default(), 8);
            l.set_epoch_ns(epoch);
            let mut clock = SimClock::new(epoch);
            for k in 1..=10u64 {
                let at = epoch + k * TICK_NS;
                assert_eq!(clock.advance(at), 1);
                l.run_ticks_profiled(1, &mut lp, || at);
            }
            let stall_start = epoch + 10 * TICK_NS;
            let raw = stall_start + 5 * TICK_NS;
            tx.push(InputEvent::button(raw, CabinetInput::RightFlipper, true))
                .unwrap();
            let wake = stall_start + 30 * TICK_NS;
            assert_eq!(clock.advance(wake), MAX_CATCH_UP_TICKS);
            let dropped = clock.dropped_ticks();
            assert_eq!(dropped, 22);
            l.skip_ticks(dropped);
            assert!(raw < l.epoch_ns(), "the stamp lies below the new epoch");
            l.run_ticks_profiled(MAX_CATCH_UP_TICKS, &mut lp, || wake);
            assert_eq!(l.sim().seen[10], (11, vec![0]), "REQ-LAT-004 delivery");
            let s = p.reader.snapshot();
            assert_eq!(s.input.count, 1, "epoch {epoch}");
            assert_eq!(s.input.max, wake - raw, "SDL now − raw stamp, in full");
            assert_eq!(s.input.max, 25 * TICK_NS);
            assert_eq!(s.input_clamped, 0);
        }
    }

    /// REQ-LAT-005 (F1): a raw stamp after the applying tick's start (but
    /// not after its end, so it is delivered) records 0 and is counted as
    /// clamped exactly once; buttons that drive no coil are not measured.
    #[test]
    fn req_lat_005_future_raw_stamp_is_clamped_once() {
        let p = Profiler::new();
        let mut lp = p.sim_loop;
        let (mut l, mut tx, _r) = setup(Stamps::default(), 8);
        let e = 1_000_000_000;
        l.set_epoch_ns(e);
        tx.push(InputEvent::button(e + 600_000, CabinetInput::Launch, true))
            .unwrap();
        tx.push(InputEvent::button(e + 100_000, CabinetInput::Start, true))
            .unwrap();
        l.run_ticks_profiled(1, &mut lp, || e + 200_000);
        assert_eq!(l.sim().seen[0], (1, vec![600_000, 100_000]));
        let s = p.reader.snapshot();
        assert_eq!((s.input.count, s.input.max, s.input_clamped), (1, 0, 1));
    }

    /// `run_ticks_profiled` hands the step exactly what `run_ticks` does
    /// (REQ-LAT-004 delivery is unchanged by measuring).
    #[test]
    fn req_lat_004_profiled_run_delivers_like_run_ticks() {
        let raws = [
            3u64,
            1_000_000_500,
            1_000_004_000,
            1_000_004_000,
            1_000_009_999,
        ];
        let run = |profiled: bool| {
            let p = Profiler::new();
            let mut lp = p.sim_loop;
            let (mut l, mut tx, _r) = setup(Stamps::default(), 8);
            l.set_epoch_ns(1_000_000_000);
            for r in raws {
                tx.push(InputEvent::button(r, CabinetInput::LeftUpper, true))
                    .unwrap();
            }
            l.run_ticks(2);
            l.skip_ticks(3);
            if profiled {
                l.run_ticks_profiled(6, &mut lp, || 1);
            } else {
                l.run_ticks(6);
            }
            (l.sim().seen.clone(), l.inputs_delivered())
        };
        assert_eq!(run(true), run(false));
    }

    proptest::proptest! {
        /// REQ-LAT-005 (F1): for any epoch, raw stamp and clock reading the
        /// loop records `start − raw` (or a counted clamp) for a delivered
        /// coil-driving press, where the re-stamp would saturate at 0.
        #[test]
        fn prop_req_lat_005_latency_is_start_minus_raw(
            epoch in 0u64..1_000_000_000_000,
            raw_off in 0u64..2_000_000_000,
            start_off in 0u64..2_000_000_000,
        ) {
            let p = Profiler::new();
            let mut lp = p.sim_loop;
            let (mut l, mut tx, _r) = setup(Stamps::default(), 2);
            l.set_epoch_ns(epoch);
            // Raw anywhere up to tick 1's end, below the epoch if it can be.
            let raw = (epoch + TICK_NS).saturating_sub(raw_off);
            let start = epoch.saturating_sub(1_000_000_000) + start_off;
            tx.push(InputEvent::button(raw, CabinetInput::RightFlipper, false))
                .unwrap();
            l.run_ticks_profiled(1, &mut lp, || start);
            proptest::prop_assert_eq!(&l.sim().seen[0].1, &vec![raw.saturating_sub(epoch)]);
            let s = p.reader.snapshot();
            proptest::prop_assert_eq!(s.input.count, 1);
            if raw <= start {
                proptest::prop_assert_eq!((s.input.max, s.input_clamped), (start - raw, 0));
            } else {
                proptest::prop_assert_eq!((s.input.max, s.input_clamped), (0, 1));
            }
        }
    }

    /// `with_profiled_sim` hands the loop writer to the sim thread (which
    /// then measures input latency in the loop); the idle config has none.
    #[test]
    fn with_profiled_sim_gives_the_sim_thread_the_loop_writer() {
        let (_tx, rx) = channel::<InputEvent>(2);
        let (cfg, _reader) =
            ThreadsConfig::with_profiled_sim(IdleSim, rx, monotonic_now_ns, Profiler::new());
        assert!(cfg.loop_profiler.is_some());
        let (_tx, rx) = channel::<InputEvent>(2);
        assert!(ThreadsConfig::idle(rx).loop_profiler.is_none());
    }
}

/// Late latency-trace install (gate 163 round 11, review 6070807835;
/// REQ-LAT-003 / REQ-LAT-004, SPEC §7.5): a writer is installed only before
/// the first tick, so no row ever takes the startup epoch as the mark of a
/// tick that is not tick 1.
#[cfg(test)]
mod late_trace_tests {
    use super::*;
    use crate::orchestrator::trace::{CollectedTrace, FlipperBindings, latency_trace};
    use pin_core::CabinetInput;
    use pin_core::spsc::{SpscProducer, channel};
    use std::cell::Cell;

    thread_local! { static NOW: Cell<u64> = const { Cell::new(0) }; }

    /// The controlled trace clock (per test thread).
    fn now() -> u64 {
        NOW.with(Cell::get)
    }

    const EPOCH: u64 = 3_000_000_000;

    fn new_loop() -> (SpscProducer<InputEvent>, SimLoop<IdleSim>) {
        let (tx, rx) = channel::<InputEvent>(16);
        let (w, _r) = TripleBuffer::new(FrameStateSnapshot::default());
        (tx, SimLoop::new(IdleSim, rx, w))
    }

    /// Runs ticks `from..=to` on time (the clock at tick `k`'s start is its
    /// end plus 50 us), pushing a press stamped inside every tick.
    fn run_with_presses(
        tx: &mut SpscProducer<InputEvent>,
        l: &mut SimLoop<IdleSim>,
        from: u64,
        to: u64,
    ) {
        for k in from..=to {
            let end = tick_end_ns(l.epoch_ns(), k);
            tx.push(InputEvent::button(end - 1, CabinetInput::LeftFlipper, true))
                .unwrap();
            NOW.with(|c| c.set(end + 50_000));
            l.run_ticks(1);
        }
    }

    /// Every row's previous-tick marks are those of the tick before its
    /// applying tick (the epoch only for tick 1).
    fn assert_prev_marks(t: &CollectedTrace, epoch: u64) {
        for r in &t.rows {
            let i = &r.input;
            let prev_end = tick_end_ns(epoch, i.apply_tick - 1);
            assert_eq!(i.prev_tick_end_sdl_ns, prev_end, "tick {}", i.apply_tick);
            let prev_drain = if i.apply_tick == 1 {
                epoch
            } else {
                prev_end + 50_000
            };
            assert_eq!(i.prev_drain_sdl_ns, prev_drain, "tick {}", i.apply_tick);
            assert!(!i.stall, "tick {}", i.apply_tick);
        }
    }

    /// Installed at tick 0 (after the epoch is set), the writer records the
    /// epoch as tick 1's previous marks and real marks afterwards.
    #[test]
    fn req_lat_003_install_at_tick_0_records_the_epoch() {
        let (mut tx, mut l) = new_loop();
        l.set_epoch_ns(EPOCH);
        let (c, sim, _fw) = latency_trace(FlipperBindings::new(), now);
        l.set_latency_trace(sim);
        assert!(l.latency_trace_installed());
        assert_eq!(l.late_trace_refusals(), 0);
        run_with_presses(&mut tx, &mut l, 1, 3);
        let t = c.collect(0);
        let ticks: Vec<u64> = t.rows.iter().map(|r| r.input.apply_tick).collect();
        assert_eq!(ticks, vec![1, 2, 3]);
        assert_eq!(t.rows[0].input.prev_tick_end_sdl_ns, EPOCH);
        assert_eq!(t.rows[0].input.prev_drain_sdl_ns, EPOCH);
        assert_prev_marks(&t, EPOCH);
    }

    /// `set_epoch_ns` after the install restarts the writer at the new
    /// epoch, as before.
    #[test]
    fn req_lat_004_set_epoch_after_install_restarts_the_writer() {
        let (mut tx, mut l) = new_loop();
        let (c, sim, _fw) = latency_trace(FlipperBindings::new(), now);
        l.set_latency_trace(sim);
        l.set_epoch_ns(EPOCH);
        assert!(l.latency_trace_installed());
        run_with_presses(&mut tx, &mut l, 1, 2);
        let t = c.collect(0);
        assert_eq!(t.rows.len(), 2);
        assert_eq!(t.rows[0].input.prev_tick_end_sdl_ns, EPOCH);
        assert_eq!(t.rows[0].input.prev_drain_sdl_ns, EPOCH);
        assert_prev_marks(&t, EPOCH);
    }

    /// Installed after ticks have run, the writer is refused (counted, no
    /// panic) and records nothing; the loop still runs its ticks.
    #[test]
    fn req_lat_003_install_after_a_tick_is_refused() {
        let (mut tx, mut l) = new_loop();
        l.set_epoch_ns(EPOCH);
        run_with_presses(&mut tx, &mut l, 1, 20);
        let (c, sim, _fw) = latency_trace(FlipperBindings::new(), now);
        l.set_latency_trace(sim);
        assert!(!l.latency_trace_installed());
        assert_eq!(l.late_trace_refusals(), 1);
        run_with_presses(&mut tx, &mut l, 21, 22);
        assert_eq!(l.tick(), 22);
        assert_eq!(l.inputs_delivered(), 22);
        let t = c.collect(0);
        assert!(
            t.rows.is_empty(),
            "a refused writer records no row (none with the startup epoch as tick 21's previous tick)"
        );
    }

    /// A late second install leaves the installed writer recording
    /// unchanged: tick 21's previous marks are tick 20's, never the epoch.
    #[test]
    fn req_lat_004_late_reinstall_keeps_the_installed_writer() {
        let (mut tx, mut l) = new_loop();
        l.set_epoch_ns(EPOCH);
        let (c, sim, _fw) = latency_trace(FlipperBindings::new(), now);
        l.set_latency_trace(sim);
        run_with_presses(&mut tx, &mut l, 1, 20);
        let (c2, sim2, _fw2) = latency_trace(FlipperBindings::new(), now);
        l.set_latency_trace(sim2);
        assert!(l.latency_trace_installed());
        assert_eq!(l.late_trace_refusals(), 1);
        run_with_presses(&mut tx, &mut l, 21, 22);
        let t = c.collect(0);
        let ticks: Vec<u64> = t.rows.iter().map(|r| r.input.apply_tick).collect();
        assert_eq!(ticks, (1..=22).collect::<Vec<u64>>());
        let r21 = &t.rows[20].input;
        assert_eq!(r21.prev_tick_end_sdl_ns, tick_end_ns(EPOCH, 20));
        assert_eq!(r21.prev_drain_sdl_ns, tick_end_ns(EPOCH, 20) + 50_000);
        assert_prev_marks(&t, EPOCH);
        assert!(
            c2.collect(0).rows.is_empty(),
            "the refused writer recorded nothing"
        );
    }
}
