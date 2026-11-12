use super::*;
use pin_core::spsc::{SpscProducer, channel};
use pin_core::{CabinetInput, TripleReader};
use proptest::prelude::*;

#[test]
fn clock_basic_and_cap() {
    let mut c = SimClock::new(1_000);
    assert_eq!(c.start_ns(), 1_000);
    assert_eq!(c.next_due_ns(), 1_000 + TICK_NS);
    assert_eq!(c.advance(1_000), 0);
    assert_eq!(c.advance(1_000 + TICK_NS - 1), 0);
    assert_eq!(c.advance(1_000 + TICK_NS), 1);
    assert_eq!(c.advance(1_000 + 3 * TICK_NS + 5), 2);
    assert_eq!(c.ticks_due_total(), 3);
    assert_eq!(c.next_due_ns(), 1_000 + 4 * TICK_NS);
    // Stall of 20 ticks: 8 due, 12 dropped, remainder (5 ns) kept.
    assert_eq!(c.advance(1_000 + 23 * TICK_NS + 5), 8);
    assert_eq!(c.dropped_ticks(), 12);
    assert_eq!(c.ticks_due_total(), 11);
    assert_eq!(c.advance(1_000 + 24 * TICK_NS + 4), 0);
    assert_eq!(c.advance(1_000 + 24 * TICK_NS + 5), 1);
    // Going backwards changes nothing.
    let before = c;
    assert_eq!(c.advance(0), 0);
    assert_eq!(c, before);
}

#[test]
fn clock_handles_huge_jumps() {
    let mut c = SimClock::new(0);
    assert_eq!(c.advance(u64::MAX), MAX_CATCH_UP_TICKS);
    assert_eq!(
        c.ticks_due_total() + c.dropped_ticks(),
        u64::MAX / TICK_NS
    );
    assert_eq!(c.next_due_ns(), u64::MAX);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// Never > 8; due + dropped = floor((now - start)/TICK) for the largest
    /// `now` so far; monotone counters; remainder kept.
    #[test]
    fn clock_invariants(start in 0u64..1u64 << 50, steps in proptest::collection::vec((0u64..30_000_000, any::<bool>()), 1..200)) {
        let mut c = SimClock::new(start);
        let mut now = start;
        let mut max_now = start;
        let (mut due_prev, mut drop_prev) = (0, 0);
        for (delta, backwards) in steps {
            now = if backwards { now.saturating_sub(delta) } else { now + delta };
            let before = c;
            let due = c.advance(now);
            prop_assert!(due <= MAX_CATCH_UP_TICKS);
            if now < max_now {
                prop_assert_eq!(due, 0);
                prop_assert_eq!(c, before);
            }
            max_now = max_now.max(now);
            prop_assert_eq!(c.ticks_due_total() + c.dropped_ticks(), (max_now - start) / TICK_NS);
            prop_assert!(c.ticks_due_total() >= due_prev && c.dropped_ticks() >= drop_prev);
            prop_assert_eq!(c.ticks_due_total() - due_prev, u64::from(due));
            due_prev = c.ticks_due_total();
            drop_prev = c.dropped_ticks();
            // Remainder kept: the next tick is due exactly at the next boundary.
            prop_assert!(c.next_due_ns() > max_now);
            prop_assert!(c.next_due_ns() - max_now <= TICK_NS);
        }
    }

    /// Splitting an interval into sub-tick steps gives the same count as one
    /// step when no ticks are dropped.
    #[test]
    fn clock_remainder_accumulates(parts in proptest::collection::vec(0u64..TICK_NS, 1..40)) {
        let mut a = SimClock::new(0);
        let mut total = 0u64;
        let mut got = 0u64;
        for p in &parts {
            total += p;
            got += u64::from(a.advance(total));
        }
        prop_assert_eq!(got + a.dropped_ticks(), total / TICK_NS);
        prop_assert_eq!(a.dropped_ticks(), 0);
    }
}

/// Records each tick's inputs; snapshot carries a digest of them.
#[derive(Default)]
struct Recorder {
    seen: Vec<(u64, Vec<InputEvent>)>,
}

impl SimStep for Recorder {
    fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
        self.seen.push((tick, inputs.to_vec()));
        out.tick = tick;
        out.lamps[0] += inputs.len() as f32;
        out.gi[0] = tick as f32;
    }
}

fn setup<S: SimStep>(
    sim: S,
    cap: usize,
) -> (
    SimLoop<S>,
    SpscProducer<InputEvent>,
    TripleReader<FrameStateSnapshot>,
) {
    let (tx, rx) = channel(cap);
    let (w, r) = TripleBuffer::new(FrameStateSnapshot::default());
    (SimLoop::new(sim, rx, w), tx, r)
}

#[test]
fn inputs_land_on_their_tick() {
    let (mut l, mut tx, mut r) = setup(Recorder::default(), 64);
    let ts = [0, 1, TICK_NS, TICK_NS + 1, 3 * TICK_NS - 1, 3 * TICK_NS, 10 * TICK_NS];
    for t in ts {
        tx.push(InputEvent::button(t, CabinetInput::Start, true)).unwrap();
    }
    assert_eq!(l.tick(), 0);
    l.run_ticks(4);
    assert_eq!(l.tick(), 4);
    assert_eq!(r.read().tick, 4);
    let per_tick: Vec<Vec<u64>> = l
        .sim()
        .seen
        .iter()
        .map(|(_, v)| v.iter().map(|e| e.timestamp_ns).collect())
        .collect();
    assert_eq!(
        per_tick,
        vec![
            vec![0, 1, TICK_NS],
            vec![TICK_NS + 1],
            vec![3 * TICK_NS - 1, 3 * TICK_NS],
            vec![],
        ]
    );
    assert_eq!(l.inputs_delivered(), 6);
    // The 10 ms event stays queued until tick 10.
    l.run_ticks(5);
    assert!(l.sim().seen.last().unwrap().1.is_empty());
    l.run_ticks(1);
    assert_eq!(l.sim().seen.last().unwrap().1.len(), 1);
    assert_eq!(l.snapshot().tick, 10);
}

#[test]
fn epoch_and_skip_shift_tick_ends() {
    let (mut l, mut tx, _r) = setup(Recorder::default(), 8);
    l.set_epoch_ns(5 * TICK_NS);
    assert_eq!(l.epoch_ns(), 5 * TICK_NS);
    tx.push(InputEvent::button(6 * TICK_NS, CabinetInput::Start, true)).unwrap();
    tx.push(InputEvent::button(9 * TICK_NS, CabinetInput::Start, false)).unwrap();
    l.run_ticks(1); // ends at 6 ms
    assert_eq!(l.sim().seen[0].1.len(), 1);
    l.skip_ticks(2); // tick 2 now ends at 9 ms
    l.run_ticks(1);
    assert_eq!(l.sim().seen[1].1.len(), 1);
    l.skip_ticks(u64::MAX);
    assert_eq!(l.epoch_ns(), u64::MAX);
}

#[test]
fn input_overflow_defers_to_next_tick() {
    let n = INPUT_BUFFER_CAPACITY + 10;
    let (mut l, mut tx, _r) = setup(Recorder::default(), n);
    for i in 0..n {
        tx.push(InputEvent::button(i as u64, CabinetInput::Tilt, true)).unwrap();
    }
    l.run_ticks(2);
    assert_eq!(l.sim().seen[0].1.len(), INPUT_BUFFER_CAPACITY);
    assert_eq!(l.sim().seen[1].1.len(), 10);
    assert_eq!(l.input_overflow_ticks(), 1);
    // Arrival order preserved.
    assert_eq!(l.sim().seen[1].1[0].timestamp_ns, INPUT_BUFFER_CAPACITY as u64);
}

#[test]
fn replay_is_identical() {
    let run = || {
        let (mut l, mut tx, mut r) = setup(Recorder::default(), 128);
        let mut snaps = Vec::new();
        for k in 0..100u64 {
            if k % 7 == 0 {
                tx.push(InputEvent::button(k * TICK_NS + 500, CabinetInput::LeftFlipper, k % 2 == 0))
                    .unwrap();
            }
            l.run_ticks(1);
            snaps.push(*r.read());
        }
        snaps
    };
    let a = run();
    assert_eq!(a, run());
    assert_eq!(a.last().unwrap().tick, 100);
}

#[test]
fn idle_sim_writes_default_with_tick() {
    let mut out = FrameStateSnapshot::default();
    out.ball_count = 3;
    IdleSim.step_1ms(42, &[], &mut out);
    assert_eq!(
        out,
        FrameStateSnapshot {
            tick: 42,
            ..FrameStateSnapshot::default()
        }
    );
}

#[test]
fn headless_runs_and_detects_violations() {
    assert_eq!(run_headless(0), Ok(0));
    assert_eq!(run_headless(1000), Ok(1000));
    struct Stuck;
    impl SimStep for Stuck {
        fn step_1ms(&mut self, tick: u64, _: &[InputEvent], out: &mut FrameStateSnapshot) {
            out.tick = tick.min(5);
        }
    }
    let e = run_headless_with(Stuck, 10).unwrap_err();
    assert!(e.contains("expected tick 6"), "{e}");
}

#[test]
fn hybrid_timer_reaches_deadline_and_stops() {
    let t = HybridTimer::default();
    let now = threads::monotonic_now_ns;
    let start = now();
    let end = t.wait_until(start + 2 * TICK_NS, now, || false);
    assert!(end >= start + 2 * TICK_NS);
    // Past deadline: returns at once.
    let e2 = t.wait_until(0, now, || false);
    assert!(e2 >= end);
    // Stop flag ends a far deadline at once.
    let e3 = t.wait_until(u64::MAX, now, || true);
    assert!(e3 < u64::MAX);
    // A simulated clock: spin path only.
    let c = core::cell::Cell::new(0u64);
    let fake = || {
        c.set(c.get() + 100);
        c.get()
    };
    let spin = HybridTimer { spin_ns: u64::MAX };
    assert!(spin.wait_until(10_000, fake, || false) >= 10_000);
}

#[test]
fn close_policy_and_drop_monitor() {
    assert!(ClosePolicy::PlayfieldOnly.quits(0));
    assert!(!ClosePolicy::PlayfieldOnly.quits(1));
    assert!(ClosePolicy::AnyWindow.quits(2));
    assert!(!ClosePolicy::Ignore.quits(0));
    assert_eq!(ClosePolicy::default(), ClosePolicy::PlayfieldOnly);
    let mut m = DropMonitor::default();
    assert_eq!(m.check(0), None);
    assert_eq!(m.check(3), Some(3));
    assert_eq!(m.check(3), None);
    assert_eq!(m.check(5), Some(2));
    assert_eq!(m.total(), 5);
}

#[test]
fn default_radius_uses_builtin_profiles() {
    assert_eq!(default_ball_radius(pin_core::ProfileId(0)), 0.0135);
    assert_eq!(default_ball_radius(pin_core::ProfileId(999)), 0.0135);
    assert!(default_ball_radius(pin_core::ProfileId(1)) > 0.0);
}
