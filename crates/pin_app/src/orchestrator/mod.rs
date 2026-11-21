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
//!   wraps the step in a [`ProfiledSim`](crate::profiler::ProfiledSim) on
//!   the sim loop's clock and hands the loop writer to the sim thread
//!   (input-overflow ticks) and the GPU frame-time writer to the render
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
    DISPLAY_RING_CAPACITY, DisplayCommand, FlipperOutline, SIM_STACK_BYTES, SOUND_RING_CAPACITY,
    TABLE_LINE_COLOR, TableSim, ValidatedTable, load_checked, load_validated, on_sim_stack,
    run_headless_table, table_lines, table_source_positions, validate_snapshot, validate_table,
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
    /// on the input timestamp clock (SDL's ns clock in the app). The epoch
    /// only gates the raw timestamps in [`run_ticks`](Self::run_ticks); the
    /// step itself always sees the tick clock (tick `k` ends at
    /// `k · TICK_NS`), because every delivered event is re-stamped to
    /// `raw − epoch_ns` (REQ-LAT-004). An epoch of 0 (the default) makes the
    /// re-stamp the identity.
    pub fn set_epoch_ns(&mut self, epoch_ns: u64) {
        self.epoch_ns = epoch_ns;
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
        for _ in 0..n {
            self.tick += 1;
            let end = tick_end_ns(self.epoch_ns, self.tick);
            self.inputs.clear();
            while !self.inputs.is_full() {
                match self.input.peek() {
                    Some(ev) if ev.timestamp_ns <= end => {
                        let _ = self.input.pop();
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
/// gates on the raw stamp and hands the step the tick clock.
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
    /// the flipper coil is energised, and equals SDL now − raw stamp, with
    /// the epoch shared as the sim thread shares it.
    #[test]
    fn req_lat_005_latency_is_measured_to_the_energising_tick() {
        for (epoch, drops) in [
            (0u64, 0u64),
            (1_000_000_000, 0),
            (10_000_000_000, 120),
            (1_000_000_000, 5000),
        ] {
            let now = Arc::new(AtomicU64::new(epoch));
            let shared = Arc::new(AtomicU64::new(0));
            let p = Profiler::new();
            let clk = Arc::clone(&now);
            let sim = ProfiledSim::new(StubFlipper::default(), p.sim, move || {
                clk.load(Ordering::Relaxed)
            })
            .with_epoch(Arc::clone(&shared));
            let (mut l, mut tx, _r) = setup(sim, 8);
            l.set_epoch_ns(epoch);
            shared.store(l.epoch_ns(), Ordering::Relaxed);
            l.skip_ticks(drops);
            shared.store(l.epoch_ns(), Ordering::Relaxed);
            let e = l.epoch_ns();
            // Run to tick 500 with the clock at each tick's start.
            for k in 1..=500u64 {
                now.store(e + (k - 1) * TICK_NS, Ordering::Relaxed);
                l.run_ticks(1);
            }
            // Press mid tick 501 (the clock reads its end when 501 runs).
            let raw = e + 500 * TICK_NS + TICK_NS / 2;
            tx.push(InputEvent::button(raw, CabinetInput::LeftFlipper, true))
                .unwrap();
            now.store(e + 501 * TICK_NS, Ordering::Relaxed);
            l.run_ticks(1);
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
        }
    }

    /// The sim thread's epoch publication stores the loop's epoch.
    #[test]
    fn publish_epoch_stores_the_loop_epoch() {
        let (mut l, _tx, _r) = setup(Stamps::default(), 2);
        let cell = AtomicU64::new(0);
        l.set_epoch_ns(42);
        threads::publish_epoch(&l, Some(&cell));
        assert_eq!(cell.load(Ordering::Relaxed), 42);
        l.skip_ticks(3);
        threads::publish_epoch(&l, Some(&cell));
        assert_eq!(cell.load(Ordering::Relaxed), 42 + 3 * TICK_NS);
        threads::publish_epoch(&l, None);
    }

    /// `with_profiled_sim` shares one epoch cell with the sim thread.
    #[test]
    fn with_profiled_sim_shares_an_epoch_cell() {
        let (_tx, rx) = channel::<InputEvent>(2);
        let (cfg, _reader) =
            ThreadsConfig::with_profiled_sim(IdleSim, rx, monotonic_now_ns, Profiler::new());
        assert!(cfg.sim_epoch_ns.is_some());
        let (_tx, rx) = channel::<InputEvent>(2);
        assert!(ThreadsConfig::idle(rx).sim_epoch_ns.is_none());
    }
}
