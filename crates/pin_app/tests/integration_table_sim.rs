//! 6.0 C1/C2: the real simulation runs behind the orchestrator: through
//! `run_headless_with` / `run_headless_table` on simulated time, through
//! `SimLoop::run_ticks` without allocating (D5; the Lua VM's capped arena is
//! the PLAN §2.3 rule 4 exemption), and through `spawn_threads` with its
//! haptic impulses reaching the audio thread. The `IdleSim` paths are
//! unchanged.

mod support;

use pin_app::orchestrator::{
    AudioConfig, IdleSim, SimLoop, TableSim, ThreadsConfig, load_validated, run_headless,
    run_headless_returning, run_headless_table, run_headless_with, spawn_threads,
};
use pin_core::input::HapticsTelemetryEvent;
use pin_core::spsc::channel;
use pin_core::{CabinetInput, FrameStateSnapshot, InputEvent, SimStep, TICK_NS, TripleBuffer};
use pin_input::haptics::haptics_ring;
use pin_sim::SimConfig;
use pin_sim::alloc_guard::{CountingAlloc, count_allocs, count_allocs_split, is_installed};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use support::{greybox_bundle, on_big_stack};

#[global_allocator]
static GLOBAL: CountingAlloc = CountingAlloc;

#[test]
fn idle_paths_unchanged() {
    assert_eq!(run_headless(250), Ok(250));
    assert_eq!(run_headless_with(IdleSim, 3), Ok(3));
    let (last, sim) = run_headless_returning(IdleSim, 4).unwrap();
    assert_eq!((last, sim), (4, IdleSim));
}

#[test]
fn real_sim_runs_behind_run_headless_with() {
    let dir = greybox_bundle().to_path_buf();
    on_big_stack(move || {
        let sim = load_validated(&dir, SimConfig::default()).unwrap();
        assert_eq!(run_headless_with(TableSim::new(sim, None), 500), Ok(500));
        let sim = load_validated(&dir, SimConfig::default()).unwrap();
        let (last, table) = run_headless_returning(TableSim::new(sim, None), 1200).unwrap();
        assert_eq!(last, 1200);
        assert_eq!(table.sim().tick(), 1200);
        assert!(!table.sim().script_faulted());
        // The table sim publishes real snapshots: three flippers.
        assert_eq!(table.sim().snapshot().flipper_count, 3);
        let (last2, hash) = run_headless_table(&dir, 1200).unwrap();
        assert_eq!(last2, 1200);
        assert_eq!(hash, table.sim().state_hash(), "same table, same ticks");
        assert!(run_headless_table(&dir.join("missing"), 1).is_err());
    });
}

/// Builds the inputs of a short game outside the guard: start, launch,
/// flippers.
fn game_inputs() -> Vec<InputEvent> {
    let mut v = Vec::new();
    let at = |ms: u64| ms * TICK_NS;
    v.push(InputEvent::button(at(5), CabinetInput::Start, true));
    v.push(InputEvent::button(at(40), CabinetInput::Start, false));
    v.push(InputEvent::button(at(1500), CabinetInput::Launch, true));
    v.push(InputEvent::button(at(1550), CabinetInput::Launch, false));
    for k in 0..40u64 {
        let t = 2000 + k * 150;
        let input = if k % 2 == 0 {
            CabinetInput::LeftFlipper
        } else {
            CabinetInput::RightFlipper
        };
        v.push(InputEvent::button(at(t), input, true));
        v.push(InputEvent::button(at(t + 80), input, false));
    }
    v
}

#[test]
fn sim_loop_with_the_real_sim_does_not_allocate() {
    assert!(is_installed());
    let (v, n) = count_allocs(|| std::hint::black_box(vec![7u8; 16]));
    assert!(n >= 1 && v.len() == 16, "guard not armed");
    let dir = greybox_bundle().to_path_buf();
    let (allocs, exempt, haptics) = on_big_stack(move || {
        let sim = load_validated(&dir, SimConfig::default()).unwrap();
        let (htx, mut hrx) = haptics_ring::<HapticsTelemetryEvent>(4096);
        let (mut tx, rx) = channel(4096);
        for ev in game_inputs() {
            tx.push(ev).unwrap();
        }
        let (w, mut r) = TripleBuffer::new(FrameStateSnapshot::default());
        let mut lp = SimLoop::new(TableSim::new(sim, Some(htx)), rx, w);
        // Warm-up outside the guard (first Lua calls size the VM).
        lp.run_ticks(200);
        let mut allocs = 0;
        let mut exempt = 0;
        for _ in 0..80 {
            let ((), n, e) = count_allocs_split(|| lp.run_ticks(100));
            allocs += n;
            exempt += e;
        }
        assert_eq!(r.read().tick, 8200);
        assert!(!lp.sim().sim().script_faulted());
        let mut haptics = 0;
        hrx.drain(|_| haptics += 1);
        (allocs, exempt, haptics)
    });
    assert_eq!(allocs, 0, "step_1ms allocated outside the Lua arena");
    eprintln!("Lua arena (exempt) allocations: {exempt}; haptic impulses: {haptics}");
    assert!(haptics > 0, "flipper coils produce haptic impulses");
}

/// A simulated clock for the threads (D8): every read advances it by 250 us,
/// so the sim thread's pacing is driven by reads, not by wall time.
static SIM_CLOCK_NS: AtomicU64 = AtomicU64::new(1_000_000_000);

fn simulated_now_ns() -> u64 {
    SIM_CLOCK_NS.fetch_add(250_000, Ordering::Relaxed)
}

/// Upper bound on progress polls (a count of polls, not a duration): the
/// simulated clock makes the sim thread run as fast as it can, so the
/// condition holds long before it.
const MAX_POLLS: u64 = 200_000_000;

#[test]
fn threads_run_the_table_and_feed_the_audio_thread() {
    let dir = greybox_bundle().to_path_buf();
    // Load on a big stack, then hand the sim to the threads (Lua tables need
    // no big stack to step).
    let sim = on_big_stack(move || load_validated(&dir, SimConfig::default()).unwrap());
    let (htx, hrx) = haptics_ring(256);
    let (mut tx, rx) = channel(64);
    let mut cfg = ThreadsConfig::with_sim(Box::new(TableSim::new(sim, Some(htx))), rx);
    cfg.now_ns = simulated_now_ns;
    cfg.audio = AudioConfig {
        haptics: Some(hrx),
        period: Duration::from_millis(1),
        ..AudioConfig::default()
    };
    let threads = spawn_threads(cfg).unwrap();
    assert_eq!(threads.names(), ["sim", "render", "audio", "io"]);
    // Flipper presses (timestamps on the simulated clock the threads use).
    let now = simulated_now_ns();
    for k in 0..6u64 {
        let t = now + k * 20 * TICK_NS;
        tx.push(InputEvent::button(t, CabinetInput::LeftFlipper, k % 2 == 0))
            .unwrap();
    }
    let mut polls = 0u64;
    // Wait for every outcome the assertions below need (explicit progress,
    // not a delay): impulses received by the audio thread (SSF exciter path), SSF frames rendered.
    let progressed = |t: &pin_app::orchestrator::Threads| {
        let a = t.audio_stats();
        a.haptic_events > 0 && a.frames > 0 && t.stats().sim_ticks >= 200
    };
    while !progressed(&threads) {
        polls += 1;
        assert!(
            polls < MAX_POLLS,
            "no haptics reached the audio thread: {:?} {:?}",
            threads.stats(),
            threads.audio_stats()
        );
        std::thread::yield_now();
    }
    let audio = threads.audio_stats();
    let stats = threads.shutdown().unwrap();
    assert!(stats.sim_ticks >= 200);
    assert!(audio.frames > 0, "SSF renders into the null sink");
    assert!(
        audio.haptic_events > 0,
        "impulses reached the audio thread and the SSF exciter mixer"
    );
}

/// `SimStep` is implemented by the table sim (also boxed, as the threads
/// hold it).
#[test]
fn table_sim_is_a_boxed_sim_step() {
    let dir = greybox_bundle().to_path_buf();
    on_big_stack(move || {
        let sim = load_validated(&dir, SimConfig::default()).unwrap();
        let mut boxed: Box<dyn SimStep> = Box::new(TableSim::new(sim, None));
        let mut out = FrameStateSnapshot::default();
        boxed.step_1ms(1, &[], &mut out);
        assert_eq!(out.tick, 1);
        let mut t = TableSim::new(load_validated(&dir, SimConfig::default()).unwrap(), None);
        t.sim_mut().advance(3);
        assert_eq!(t.haptics_dropped(), 0);
        assert_eq!(t.into_inner().tick(), 3);
    });
}
