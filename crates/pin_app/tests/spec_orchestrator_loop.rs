//! Independent spec tests for `pin_app::orchestrator::{SimLoop, IdleSim, run_headless}`
//! (Subphase 1.4, verification criteria 1 and 4), written from the FIXED API alone.
//!
//! Everything here runs on simulated time: no sleeping, no wall clock (D8).

use pin_app::orchestrator::{IdleSim, SimLoop, run_headless};
use pin_core::sim::{SimStep, TICK_NS};
use pin_core::snapshot::BallState;
use pin_core::spsc::{self, SpscProducer};
use pin_core::{
    CabinetAxis, CabinetInput, FrameStateSnapshot, InputEvent, InputEventKind, TripleBuffer,
    TripleReader,
};
use std::sync::{Arc, Mutex};

struct SplitMix(u64);

impl SplitMix {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_1EB5);
        z ^ (z >> 31)
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

type Log = Arc<Mutex<Vec<(u64, Vec<InputEvent>)>>>;

/// Records every `step_1ms` call (test-side sim, free to allocate) and writes the tick.
struct Recorder {
    log: Log,
}

impl SimStep for Recorder {
    fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
        self.log.lock().unwrap().push((tick, inputs.to_vec()));
        *out = FrameStateSnapshot::default();
        out.tick = tick;
        out.lamps[0] = inputs.len() as f32;
    }
}

/// A small deterministic "physics": state depends on the whole input history.
struct Integrator {
    x: f32,
    v: f32,
    held: bool,
    hash: u64,
}

impl Integrator {
    fn new() -> Self {
        Self {
            x: 0.0,
            v: 0.0,
            held: false,
            hash: 0xCBF2_9CE4_8422_2325,
        }
    }
}

impl SimStep for Integrator {
    fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
        for ev in inputs {
            self.hash = (self.hash ^ ev.timestamp_ns).wrapping_mul(0x100_0000_01B3);
            match ev.kind {
                InputEventKind::Button { input, pressed } => {
                    self.hash = (self.hash ^ input.index() as u64).wrapping_mul(0x100_0000_01B3);
                    if input == CabinetInput::LeftFlipper {
                        self.held = pressed;
                    }
                }
                InputEventKind::Axis { value, .. } => {
                    self.v += value * 0.125;
                }
            }
        }
        if self.held {
            self.v += 0.001;
        }
        self.v *= 0.999;
        self.x += self.v * 0.001;
        *out = FrameStateSnapshot::default();
        out.tick = tick;
        out.push_ball(BallState {
            position_m: [self.x, self.v, 0.0],
            ..BallState::default()
        });
        out.lamps[0] = (self.hash & 0xFFFF) as f32;
        out.lamps[1] = ((self.hash >> 16) & 0xFFFF) as f32;
        out.lamps[2] = inputs.len() as f32;
    }
}

struct Rig<S: SimStep> {
    sim_loop: SimLoop<S>,
    input: SpscProducer<InputEvent>,
    reader: TripleReader<FrameStateSnapshot>,
}

fn rig<S: SimStep>(sim: S, queue: usize) -> Rig<S> {
    let (input, consumer) = spsc::channel::<InputEvent>(queue);
    let (writer, reader) = TripleBuffer::new(FrameStateSnapshot::default());
    Rig {
        sim_loop: SimLoop::new(sim, consumer, writer),
        input,
        reader,
    }
}

fn recorder_rig(queue: usize) -> (Rig<Recorder>, Log) {
    let log: Log = Arc::new(Mutex::new(Vec::new()));
    (rig(Recorder { log: log.clone() }, queue), log)
}

fn button(ts: u64) -> InputEvent {
    InputEvent::button(ts, CabinetInput::LeftFlipper, true)
}

/// The tick an event with timestamp `ts` belongs to: the first k ≥ 1 with ts ≤ k·TICK_NS.
fn expected_tick(ts: u64) -> u64 {
    ts.div_ceil(TICK_NS).max(1)
}

// ---------------------------------------------------------------------------------------
// tick numbering and publishing
// ---------------------------------------------------------------------------------------

#[test]
fn tick_is_zero_before_any_tick_and_nothing_is_published() {
    let (mut r, log) = recorder_rig(16);
    assert_eq!(r.sim_loop.tick(), 0);
    r.sim_loop.run_ticks(0);
    assert_eq!(r.sim_loop.tick(), 0);
    assert!(log.lock().unwrap().is_empty(), "run_ticks(0) must not step");
    assert_eq!(r.reader.read().tick, 0);
}

#[test]
fn ticks_are_one_based_consecutive_and_span_run_ticks_calls() {
    let (mut r, log) = recorder_rig(16);
    let mut expected_last = 0u64;
    for n in [1u32, 1, 3, 10, 0, 250, 7] {
        r.sim_loop.run_ticks(n);
        expected_last += u64::from(n);
        assert_eq!(r.sim_loop.tick(), expected_last);
        assert_eq!(r.reader.read().tick, expected_last, "latest snapshot is published");
    }
    let log = log.lock().unwrap();
    assert_eq!(log.len() as u64, expected_last);
    for (i, (tick, inputs)) in log.iter().enumerate() {
        assert_eq!(*tick, i as u64 + 1, "first call gets tick 1, then +1 each");
        assert!(inputs.is_empty());
    }
}

#[test]
fn one_snapshot_is_published_per_tick() {
    let mut r = rig(Integrator::new(), 16);
    for k in 1..=500u64 {
        r.sim_loop.run_ticks(1);
        assert!(r.reader.has_update(), "tick {k} must publish");
        let s = *r.reader.read();
        assert_eq!(s.tick, k);
        assert!(!r.reader.has_update());
    }
}

#[test]
fn idle_sim_sets_the_tick_and_leaves_the_default_snapshot() {
    let mut r = rig(IdleSim, 16);
    for n in [1u32, 2, 97] {
        r.sim_loop.run_ticks(n);
        let s = *r.reader.read();
        let expected = FrameStateSnapshot {
            tick: r.sim_loop.tick(),
            ..FrameStateSnapshot::default()
        };
        assert_eq!(s, expected);
    }
    assert_eq!(r.sim_loop.tick(), 100);

    // Directly, without the loop, and with inputs present.
    let mut idle = IdleSim;
    let mut out = FrameStateSnapshot::default();
    idle.step_1ms(41, &[button(1)], &mut out);
    assert_eq!(
        out,
        FrameStateSnapshot {
            tick: 41,
            ..FrameStateSnapshot::default()
        }
    );
}

// ---------------------------------------------------------------------------------------
// input delivery on the exact tick
// ---------------------------------------------------------------------------------------

#[test]
fn inputs_arrive_on_the_tick_their_timestamp_falls_in() {
    let (mut r, log) = recorder_rig(64);
    let stamps = [
        0u64,              // ≤ 1·TICK → tick 1
        1,                 // tick 1
        TICK_NS,           // exactly the end of tick 1 → tick 1
        TICK_NS + 1,       // tick 2
        2 * TICK_NS - 1,   // tick 2
        5 * TICK_NS,       // tick 5
        5 * TICK_NS + 1,   // tick 6
        9 * TICK_NS + 500, // tick 10
    ];
    for &ts in &stamps {
        r.input.push(button(ts)).unwrap();
    }
    r.sim_loop.run_ticks(12);
    let log = log.lock().unwrap();
    let got: Vec<(u64, Vec<u64>)> = log
        .iter()
        .filter(|(_, i)| !i.is_empty())
        .map(|(t, i)| (*t, i.iter().map(|e| e.timestamp_ns).collect()))
        .collect();
    assert_eq!(
        got,
        vec![
            (1, vec![0, 1, TICK_NS]),
            (2, vec![TICK_NS + 1, 2 * TICK_NS - 1]),
            (5, vec![5 * TICK_NS]),
            (6, vec![5 * TICK_NS + 1]),
            (10, vec![9 * TICK_NS + 500]),
        ]
    );
}

#[test]
fn future_events_stay_queued_across_run_ticks_calls() {
    let (mut r, log) = recorder_rig(64);
    r.input.push(button(40 * TICK_NS + 1)).unwrap(); // tick 41
    r.sim_loop.run_ticks(10);
    assert!(log.lock().unwrap().iter().all(|(_, i)| i.is_empty()));
    assert_eq!(r.input.len(), 1, "an event in the future stays in the queue");
    r.sim_loop.run_ticks(30);
    assert!(log.lock().unwrap().iter().all(|(_, i)| i.is_empty()));
    r.sim_loop.run_ticks(1);
    {
        let log = log.lock().unwrap();
        let (tick, inputs) = log.last().unwrap();
        assert_eq!(*tick, 41);
        assert_eq!(inputs.len(), 1);
        assert_eq!(inputs[0], button(40 * TICK_NS + 1));
    }
    assert_eq!(r.input.len(), 0);
}

#[test]
fn late_events_are_delivered_on_the_next_tick() {
    let (mut r, log) = recorder_rig(64);
    r.sim_loop.run_ticks(20);
    // Timestamp far in the past (tick 3 has long gone): ≤ 21·TICK_NS, so tick 21 gets it.
    r.input.push(button(2 * TICK_NS + 5)).unwrap();
    r.input.push(button(20 * TICK_NS)).unwrap();
    r.sim_loop.run_ticks(3);
    let log = log.lock().unwrap();
    let with_inputs: Vec<(u64, usize)> = log
        .iter()
        .filter(|(_, i)| !i.is_empty())
        .map(|(t, i)| (*t, i.len()))
        .collect();
    assert_eq!(with_inputs, vec![(21, 2)]);
}

#[test]
fn events_keep_arrival_order_and_payload() {
    let (mut r, log) = recorder_rig(64);
    let events = [
        InputEvent::button(100, CabinetInput::LeftFlipper, true),
        InputEvent::axis(100, CabinetAxis::Plunger, 0.25),
        InputEvent::button(100, CabinetInput::RightFlipper, true),
        InputEvent::button(200, CabinetInput::LeftFlipper, false),
        InputEvent::axis(900_000, CabinetAxis::NudgeX, -1.0),
        InputEvent::button(1_000_000, CabinetInput::Start, true),
    ];
    for e in events {
        r.input.push(e).unwrap();
    }
    r.sim_loop.run_ticks(2);
    let log = log.lock().unwrap();
    assert_eq!(log[0].0, 1);
    assert_eq!(log[0].1, events.to_vec());
    assert!(log[1].1.is_empty());
}

/// Randomised: any non-decreasing timestamp sequence, any chunking of `run_ticks`, every
/// event is delivered exactly once, in order, on `expected_tick(timestamp)`.
#[test]
fn property_every_event_is_delivered_once_on_its_tick() {
    let mut rng = SplitMix(0x1A_57E9);
    for case in 0..60 {
        let (mut r, log) = recorder_rig(2048);
        let horizon_ticks = 1 + rng.below(300);
        let n_events = rng.below(600) as usize;
        let mut stamps: Vec<u64> = (0..n_events)
            .map(|_| match rng.below(4) {
                0 => rng.below(horizon_ticks + 1) * TICK_NS, // exactly on a boundary
                1 => rng.below(horizon_ticks) * TICK_NS + 1, // just after a boundary
                _ => rng.below(horizon_ticks * TICK_NS + 1),
            })
            .collect();
        stamps.sort_unstable();
        let events: Vec<InputEvent> = stamps
            .iter()
            .enumerate()
            .map(|(i, &ts)| {
                if i % 3 == 0 {
                    InputEvent::axis(ts, CabinetAxis::Plunger, i as f32)
                } else {
                    InputEvent::button(ts, CabinetInput::ALL[i % 8], i % 2 == 0)
                }
            })
            .collect();
        for e in &events {
            r.input.push(*e).unwrap();
        }
        let mut done = 0u64;
        while done < horizon_ticks {
            let chunk = (1 + rng.below(40)).min(horizon_ticks - done);
            r.sim_loop.run_ticks(chunk as u32);
            done += chunk;
            assert_eq!(r.sim_loop.tick(), done);
            assert_eq!(r.reader.read().tick, done);
        }
        let log = log.lock().unwrap();
        assert_eq!(log.len() as u64, horizon_ticks);
        let mut delivered = Vec::new();
        for (i, (tick, inputs)) in log.iter().enumerate() {
            assert_eq!(*tick, i as u64 + 1);
            for e in inputs {
                assert_eq!(
                    expected_tick(e.timestamp_ns),
                    *tick,
                    "case {case}: event at {} ns delivered on tick {tick}",
                    e.timestamp_ns
                );
                delivered.push(*e);
            }
        }
        assert_eq!(delivered, events, "case {case}: each event once, in arrival order");
    }
}

/// More events on one tick than the fixed input buffer holds must not panic, and the loop
/// keeps ticking. (What happens to the overflow is not fixed by the brief; only that no
/// call sees more than the documented buffer capacity of 256.)
#[test]
fn a_burst_larger_than_the_input_buffer_does_not_panic() {
    let (mut r, log) = recorder_rig(1024);
    for i in 0..700u64 {
        r.input.push(button(i)).unwrap();
    }
    r.sim_loop.run_ticks(10);
    assert_eq!(r.sim_loop.tick(), 10);
    assert_eq!(r.reader.read().tick, 10);
    let log = log.lock().unwrap();
    assert_eq!(log.len(), 10);
    assert!(log.iter().all(|(_, i)| i.len() <= 256));
    assert_eq!(log[0].1.len(), 256, "tick 1 gets a full buffer");
    // Whatever was delivered kept arrival order.
    let seen: Vec<u64> = log
        .iter()
        .flat_map(|(_, i)| i.iter().map(|e| e.timestamp_ns))
        .collect();
    assert!(seen.windows(2).all(|w| w[0] < w[1]));
}

// ---------------------------------------------------------------------------------------
// replay determinism
// ---------------------------------------------------------------------------------------

fn replay_script(seed: u64, ticks: u64) -> Vec<InputEvent> {
    let mut rng = SplitMix(seed);
    let mut ts = 0u64;
    let mut held = false;
    let mut out = Vec::new();
    while ts < ticks * TICK_NS {
        ts += rng.below(6 * TICK_NS);
        if rng.below(3) == 0 {
            let v = (rng.below(2001) as f32 - 1000.0) / 1000.0;
            out.push(InputEvent::axis(ts, CabinetAxis::NudgeX, v));
        } else {
            held = !held;
            out.push(InputEvent::button(ts, CabinetInput::LeftFlipper, held));
        }
    }
    out
}

/// Runs the script and returns every published snapshot (one `run_ticks(chunk)` at a time,
/// reading back after each call).
fn run_replay(script: &[InputEvent], ticks: u64, chunk: u32) -> Vec<FrameStateSnapshot> {
    let mut r = rig(Integrator::new(), script.len().max(1));
    for e in script {
        r.input.push(*e).unwrap();
    }
    let mut out = Vec::new();
    let mut done = 0u64;
    while done < ticks {
        let n = u64::from(chunk).min(ticks - done);
        r.sim_loop.run_ticks(n as u32);
        done += n;
        out.push(*r.reader.read());
    }
    out
}

#[test]
fn replaying_the_same_inputs_gives_identical_snapshot_sequences() {
    for seed in [1u64, 2, 3, 0xDEAD_BEEF] {
        let script = replay_script(seed, 2_000);
        let a = run_replay(&script, 2_000, 1);
        let b = run_replay(&script, 2_000, 1);
        assert_eq!(a.len(), 2_000);
        assert!(a == b, "seed {seed}: replay diverged");
        // The script actually drove the sim (guards against a vacuous comparison).
        assert!(a.iter().any(|s| s.lamps[2] > 0.0));
        assert!(a.last().unwrap().balls()[0].position_m[0] != 0.0);
        for (i, s) in a.iter().enumerate() {
            assert_eq!(s.tick, i as u64 + 1);
        }
    }
}

#[test]
fn replay_is_stable_over_ten_runs() {
    let script = replay_script(99, 1_500);
    let first = run_replay(&script, 1_500, 1);
    for run in 1..10 {
        assert!(run_replay(&script, 1_500, 1) == first, "run {run} diverged");
    }
}

/// How the ticks are batched into `run_ticks` calls must not change the simulation.
#[test]
fn chunking_does_not_change_the_result() {
    let script = replay_script(7, 1_200);
    let per_tick = run_replay(&script, 1_200, 1);
    for chunk in [2u32, 3, 8, 100, 1_200] {
        let chunked = run_replay(&script, 1_200, chunk);
        for (i, s) in chunked.iter().enumerate() {
            let tick = ((i as u64 + 1) * u64::from(chunk)).min(1_200);
            assert_eq!(s.tick, tick);
            assert!(
                *s == per_tick[tick as usize - 1],
                "chunk {chunk}: snapshot at tick {tick} differs from the per-tick run"
            );
        }
    }
}

/// A different input sequence gives a different result (the comparison above has teeth).
#[test]
fn different_inputs_give_different_snapshots() {
    let a = run_replay(&replay_script(1, 500), 500, 500);
    let b = run_replay(&replay_script(2, 500), 500, 500);
    assert!(a != b);
}

// ---------------------------------------------------------------------------------------
// run_headless
// ---------------------------------------------------------------------------------------

#[test]
fn run_headless_returns_the_last_tick() {
    assert_eq!(run_headless(1), Ok(1));
    assert_eq!(run_headless(8), Ok(8));
    assert_eq!(run_headless(9), Ok(9));
    assert_eq!(run_headless(10_000), Ok(10_000));
}

/// Simulated time only: a count that would take minutes in real time returns promptly.
/// (No wall-clock assertion, per D8; a sleeping implementation shows up as a hung test.)
#[test]
fn run_headless_handles_a_long_run() {
    assert_eq!(run_headless(300_000), Ok(300_000));
}
