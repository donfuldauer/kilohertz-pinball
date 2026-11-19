//! w8f-q6: the 7.1 profiler wired into the app's threads (GPU-free).
//!
//! * (a) The production wrapping, `ProfiledSim<Box<TableSim>, fn() -> u64>`
//!   (the app's boxed table sim and SDL-style clock function type), on the
//!   Apex Circuit driven by `SimLoop`: no allocation per tick under the
//!   counting allocator of `pin_sim::alloc_guard`, and the same final
//!   `state_hash` as the unprofiled run.
//! * (b) Hook 2: an input burst that overflows the per-tick input buffer,
//!   run by the real sim thread (`spawn_threads`): the reader's
//!   `input_overflow_ticks` equals the loop's count.
//! * (c) Hook 1 end to end: `ThreadsConfig::with_profiled_sim` around a
//!   shipped table, a short run with a flipper press: ticks and an
//!   input-latency sample are recorded.
//!
//! The threaded tests drive the sim thread from a test-controlled clock
//! (one per test, read by the `fn() -> u64` handed to `with_profiled_sim`,
//! so the sim loop and the profiler share it). The clock moves only when
//! the test moves it: once the sim thread has read its epoch, one tick
//! (`TICK_NS`) at a time, and the test waits for the sim thread to publish
//! that tick before the next advance. Tick, overflow and input-latency
//! counts are a function of the advances alone, so the tests assert exact
//! counts. The wait is a bounded `yield_now` loop used as a hang guard
//! only: no timed wait, no clock read, no assertion on durations (rule 5).

mod apex_support;

use apex_support::{apex_bundle, on_big_stack};
use pin_app::orchestrator::{
    INPUT_BUFFER_CAPACITY, IdleSim, SimLoop, TableSim, Threads, ThreadsConfig, monotonic_now_ns,
    spawn_threads,
};
use pin_app::profiler::{ProfiledSim, Profiler, ProfilerReader};
use pin_core::spsc::channel;
use pin_core::{CabinetInput, FrameStateSnapshot, InputEvent, SimStep, TICK_NS, TripleBuffer};
use pin_sim::alloc_guard::{CountingAlloc, count_allocs, count_allocs_split, is_installed};
use pin_sim::{SimConfig, Simulation};
use std::sync::atomic::{AtomicU64, Ordering};

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

fn apex() -> Simulation {
    Simulation::load(apex_bundle(), SimConfig::default()).unwrap_or_else(|e| panic!("{e}"))
}

/// Flipper presses and releases every 50 ticks, on the loop's tick clock
/// (epoch 0: tick `t` ends at `t · TICK_NS`).
fn flipper_events(ticks: u64) -> Vec<InputEvent> {
    (1..ticks / 50)
        .map(|k| {
            let input = if k % 4 < 2 {
                CabinetInput::LeftFlipper
            } else {
                CabinetInput::RightFlipper
            };
            InputEvent::button(k * 50 * TICK_NS, input, k % 2 == 1)
        })
        .collect()
}

/// A sim loop over `sim` with `events` queued.
fn looped<S: SimStep>(sim: S, events: &[InputEvent]) -> SimLoop<S> {
    let (mut tx, rx) = channel(1 << 12);
    for ev in events {
        tx.push(*ev).unwrap();
    }
    let (w, _r) = TripleBuffer::new(FrameStateSnapshot::default());
    SimLoop::new(sim, rx, w)
}

#[test]
fn profiled_boxed_table_sim_with_fn_clock_does_not_allocate_per_tick() {
    assert!(is_installed());
    let (v, n) = count_allocs(|| std::hint::black_box(vec![1u8; 8]));
    assert!(n >= 1 && v.len() == 8, "guard not armed");
    const TICKS: u64 = 5_000;
    const WARM_UP: u32 = 500;
    let (plain, profiled, allocs, snap) = on_big_stack(|| {
        let events = flipper_events(TICKS);
        // Unprofiled reference.
        let mut lp = looped(TableSim::new(apex(), None), &events);
        lp.run_ticks(TICKS as u32);
        let plain = lp.sim().sim().state_hash();

        // The app's shape: a boxed TableSim, a `fn() -> u64` clock.
        let p = Profiler::new();
        let clock: fn() -> u64 = monotonic_now_ns;
        let sim: ProfiledSim<Box<TableSim>, fn() -> u64> =
            ProfiledSim::new(Box::new(TableSim::new(apex(), None)), p.sim, clock);
        let mut lp = looped(sim, &events);
        let mut loop_profiler = p.sim_loop;
        lp.run_ticks(WARM_UP); // First calls size lazily grown buffers.
        let mut allocs = 0;
        for _ in 0..((TICKS - u64::from(WARM_UP)) / 100) {
            let ((), n, _exempt) = count_allocs_split(|| {
                lp.run_ticks(100);
                loop_profiler.set_input_overflow_ticks(lp.input_overflow_ticks());
            });
            allocs += n;
        }
        assert_eq!(lp.tick(), TICKS);
        let profiled = lp.sim().inner().sim().state_hash();
        (plain, profiled, allocs, p.reader.snapshot())
    });
    assert_eq!(
        allocs, 0,
        "ProfiledSim<Box<TableSim>, fn() -> u64> allocated"
    );
    assert_eq!(profiled, plain, "the profiler changed the simulation");
    assert_eq!(snap.tick.count, TICKS);
    assert_eq!(
        snap.input.count,
        flipper_events(TICKS).len() as u64,
        "every flipper event is a latency sample"
    );
}

/// Checks of `cond` before [`wait_for`] gives up. A hang guard only: the
/// awaited condition is reached by the test's own clock advances, not by
/// elapsed time, so the bound is generous but finite and a hang still ends.
const SPIN_LIMIT: u64 = 200_000_000;

/// Polls `cond` until it holds, yielding the core between checks
/// (`yield_now`, so the threads under test run) under an iteration bound of
/// [`SPIN_LIMIT`]: no timed wait, no clock. A hang guard only: the test's clock
/// advance has already made `cond` due, the loop just lets the sim thread
/// get there. Returns whether it held.
fn wait_for(threads: &Threads, cond: impl Fn(&Threads) -> bool) -> bool {
    for _ in 0..SPIN_LIMIT {
        if cond(threads) {
            return true;
        }
        std::thread::yield_now();
    }
    cond(threads)
}

/// The controlled clocks' start value (nonzero, so the sim loop's epoch is
/// not the zero default).
const CLOCK_BASE_NS: u64 = 1_000_000_000;

/// A test-controlled clock: `now` moves only when the test calls
/// [`tick_to`]; `reads` counts the clock function's calls. Only the sim
/// thread calls the clock function (the sim loop, its timer and the
/// `ProfiledSim`), so `reads > 0` means the sim thread has taken its epoch.
struct TestClock {
    now: AtomicU64,
    reads: AtomicU64,
}

impl TestClock {
    /// A clock at [`CLOCK_BASE_NS`], never read.
    const fn new() -> Self {
        Self {
            now: AtomicU64::new(CLOCK_BASE_NS),
            reads: AtomicU64::new(0),
        }
    }

    /// The current time (no advance on read); counts the read. The value is
    /// loaded before the read is published, so a test that sees `reads > 0`
    /// and then advances the clock cannot shift the reader's epoch.
    fn read(&self) -> u64 {
        let t = self.now.load(Ordering::Acquire);
        self.reads.fetch_add(1, Ordering::AcqRel);
        t
    }
}

/// The overflow test's controlled clock (one per test: the tests of this
/// binary run in parallel).
static OVERFLOW_CLOCK: TestClock = TestClock::new();

/// Reads [`OVERFLOW_CLOCK`].
fn overflow_clock() -> u64 {
    OVERFLOW_CLOCK.read()
}

/// The table-run test's controlled clock.
static TABLE_CLOCK: TestClock = TestClock::new();

/// Reads [`TABLE_CLOCK`].
fn table_clock() -> u64 {
    TABLE_CLOCK.read()
}

/// Waits until the sim thread has read `clock` (its epoch,
/// `CLOCK_BASE_NS`): an advance before that read would move the epoch
/// instead of running a tick.
fn await_epoch(threads: &Threads, clock: &TestClock) {
    assert!(
        wait_for(threads, |_| clock.reads.load(Ordering::Acquire) > 0),
        "the sim thread never read its clock: {:?}",
        threads.stats()
    );
}

/// Moves `clock` to the end of tick `k` (`CLOCK_BASE_NS + k · TICK_NS`, one
/// tick past the previous call) and waits until the sim thread has run and
/// published tick `k` (`sim_ticks == k`). One tick per advance, so the
/// catch-up cap never drops a tick, and the clock never moves while a tick
/// runs.
fn tick_to(threads: &Threads, clock: &TestClock, k: u64) {
    clock
        .now
        .store(CLOCK_BASE_NS + k * TICK_NS, Ordering::Release);
    assert!(
        wait_for(threads, |t| t.stats().sim_ticks == k),
        "tick {k} never ran: {:?}",
        threads.stats()
    );
}

#[test]
fn sim_thread_publishes_input_overflow_ticks() {
    // 2 full buffers plus 5: ticks 1 and 2 overflow, tick 3 drains the rest,
    // so the loop counts exactly 2 overflow ticks (`SimLoop::run_ticks`).
    let n = 2 * INPUT_BUFFER_CAPACITY + 5;
    let (mut tx, rx) = channel(4 * INPUT_BUFFER_CAPACITY);
    for i in 0..n {
        // Stamped at 0: due on the first tick.
        tx.push(InputEvent::button(0, CabinetInput::Tilt, i % 2 == 0))
            .unwrap();
    }
    let (cfg, reader) =
        ThreadsConfig::with_profiled_sim(IdleSim, rx, overflow_clock, Profiler::new());
    assert!(cfg.loop_profiler.is_some() && cfg.render.present_profiler.is_some());
    assert!(cfg.render.hud.is_none(), "the HUD is opt-in");
    let threads = spawn_threads(cfg).expect("spawn");
    await_epoch(&threads, &OVERFLOW_CLOCK);
    for k in 1..=3 {
        tick_to(&threads, &OVERFLOW_CLOCK, k);
    }
    let fin = threads.shutdown().expect("clean shutdown");
    assert_eq!(fin.sim_ticks, 3, "{fin:?}");
    let s = reader.snapshot();
    assert_eq!(s.input_overflow_ticks, 2, "the loop's count: {s:?}");
    assert_eq!(s.command_queue_overflows(), 2);
    // IdleSim was profiled too: one tick sample per tick.
    assert_eq!(s.tick.count, 3, "{s:?}");
}

#[test]
fn profiled_table_run_records_ticks_and_input_latency() {
    let sim = on_big_stack(|| Box::new(TableSim::new(apex(), None)));
    let (mut tx, rx) = channel(64);
    let (cfg, reader): (ThreadsConfig, ProfilerReader) =
        ThreadsConfig::with_profiled_sim(sim, rx, table_clock, Profiler::new());
    assert!(
        std::ptr::fn_addr_eq(cfg.now_ns, table_clock as fn() -> u64),
        "the sim loop runs on the profiler's clock"
    );
    let threads = spawn_threads(cfg).expect("spawn");
    await_epoch(&threads, &TABLE_CLOCK);
    for k in 1..=50 {
        tick_to(&threads, &TABLE_CLOCK, k);
    }
    let r = reader.clone();
    assert!(
        wait_for(&threads, move |_| r.snapshot().tick.count >= 50),
        "tick samples not published: {:?}",
        reader.snapshot()
    );
    assert_eq!(reader.snapshot().tick.count, 50, "{:?}", reader.snapshot());
    // A flipper press stamped now on the shared clock (the end of tick 50):
    // tick 51 applies it and starts at the next advance, 1 ms later, so the
    // press is not after the tick start and is not clamped.
    tx.push(InputEvent::button(
        table_clock(),
        CabinetInput::LeftFlipper,
        true,
    ))
    .unwrap();
    tick_to(&threads, &TABLE_CLOCK, 51);
    let r = reader.clone();
    assert!(
        wait_for(&threads, move |_| r.snapshot().input.count >= 1),
        "flipper press not recorded: {:?}",
        reader.snapshot()
    );
    let fin = threads.shutdown().expect("clean shutdown");
    let s = reader.snapshot();
    assert_eq!(fin.sim_ticks, 51, "{fin:?}");
    assert_eq!(s.tick.count, 51, "one tick sample per tick: {s:?}");
    assert_eq!(s.input.count, 1, "{s:?}");
    assert_eq!(s.input_clamped, 0, "{s:?}");
    assert_eq!(s.gpu_frame.count, 0, "no GPU, no GPU samples");
}
