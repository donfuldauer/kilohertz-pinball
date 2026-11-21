//! Issue #81 (KP-001): flipper input epoch. Independent acceptance tests of
//! SPEC §7.5 REQ-LAT-004 and REQ-LAT-005 (SPEC §2.2 tick step 1), written
//! from the brief alone (black box).
//!
//! REQ-LAT-004: an input is applied in the first executed tick that has
//! received it and whose end is not before its timestamp, inside a stall,
//! inside dropped time and when forwarded late; neither the time between
//! process start and simulation start (the sim loop's epoch) nor dropped
//! ticks delay it.
//!
//! REQ-LAT-005: the profiler's input latency runs from the SDL timestamp to
//! the tick in which the input took effect, on the tick clock.
//!
//! The sim thread's sequence is mirrored headlessly on simulated time
//! (`SimClock::new(start)`, `set_epoch_ns(start)`, then per wake-up
//! `skip_ticks(newly dropped)` before `run_ticks(due)`); headless tests never
//! sleep and never read a wall clock (R6). The thread tests (REQ-LAT-004
//! thread test, REQ-LAT-005 thread path) poll with a bounded deadline as a
//! hang guard only and assert ticks and latencies, not wall durations.

mod apex_support;

use apex_support::{apex_bundle, on_big_stack};
use pin_app::orchestrator::{
    SimClock, SimLoop, TableSim, Threads, ThreadsConfig, load_validated, monotonic_now_ns,
    spawn_threads,
};
use pin_app::profiler::{ProfiledSim, Profiler};
use pin_core::haptics::{SOURCE_CLASS_COIL, SOURCE_CLASS_SHIFT};
use pin_core::ids::FlipperId;
use pin_core::input::HapticsTelemetryEvent;
use pin_core::sim::tick_end_ns;
use pin_core::spsc::{SpscProducer, channel};
use pin_core::{
    CabinetInput, FrameStateSnapshot, InputEvent, MAX_CATCH_UP_TICKS, SimStep, TICK_NS,
    TripleBuffer,
};
use pin_input::haptics::haptics_ring;
use pin_sim::{SimConfig, Simulation};
use proptest::prelude::*;
use proptest::test_runner::{Config, RngAlgorithm, TestRng, TestRunner};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};

/// One second, ns.
const SECOND_NS: u64 = 1_000_000_000;

/// The epochs of the headless sweeps: none, 1 s and 10 s of startup.
const EPOCHS: [u64; 3] = [0, SECOND_NS, 10 * SECOND_NS];

/// REQ-LAT-005's input-latency target: p99 at most 1.5 ms.
const INPUT_P99_TARGET_NS: u64 = 1_500_000;

/// Headless tests share this lock for reading; the thread tests take it for
/// writing, so the sim thread's wake-ups are not starved by the CPU-heavy
/// headless sweeps of the same binary (the thread tests measure latency).
static EXCLUSIVE: RwLock<()> = RwLock::new(());

fn shared() -> std::sync::RwLockReadGuard<'static, ()> {
    EXCLUSIVE
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn exclusive() -> std::sync::RwLockWriteGuard<'static, ()> {
    EXCLUSIVE
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// The two Apex Circuit flippers driven by these tests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Side {
    Left,
    Right,
}

impl Side {
    fn input(self) -> CabinetInput {
        match self {
            Side::Left => CabinetInput::LeftFlipper,
            Side::Right => CabinetInput::RightFlipper,
        }
    }

    fn index(self) -> usize {
        match self {
            Side::Left => 0,
            Side::Right => 1,
        }
    }
}

/// The Apex Circuit, loaded through the app's validated path.
fn apex() -> Simulation {
    load_validated(apex_bundle(), SimConfig::default()).unwrap_or_else(|e| panic!("{e}"))
}

/// The state of both flippers after one executed tick.
#[derive(Clone, Copy, Debug)]
struct Obs {
    /// The executed tick.
    tick: u64,
    /// Coil energised (`FlipperSet::is_energised`), [left, right].
    on: [bool; 2],
    /// Angle relative to rest, rad, [left, right].
    angle: [f32; 2],
    /// Angular velocity, rad/s, [left, right].
    vel: [f32; 2],
}

fn observe(s: &Simulation, ids: [FlipperId; 2], tick: u64) -> Obs {
    let fs = s.flipper_set();
    let l = s.flipper("left_flipper").expect("left_flipper");
    let r = s.flipper("right_flipper").expect("right_flipper");
    Obs {
        tick,
        on: [fs.is_energised(ids[0]), fs.is_energised(ids[1])],
        angle: [l.0, r.0],
        vel: [l.1, r.1],
    }
}

fn flipper_ids(s: &Simulation) -> [FlipperId; 2] {
    [
        s.ids().flipper("left_flipper").expect("left_flipper id"),
        s.ids().flipper("right_flipper").expect("right_flipper id"),
    ]
}

/// The real table sim behind the loop, recording both flippers after every
/// executed tick (so a batch `run_ticks(n)` can be inspected tick by tick).
struct Probe {
    table: TableSim,
    ids: [FlipperId; 2],
    log: Vec<Obs>,
}

impl Probe {
    fn new() -> Self {
        let sim = apex();
        let ids = flipper_ids(&sim);
        Self {
            table: TableSim::new(sim, None),
            ids,
            log: Vec::with_capacity(4096),
        }
    }

    /// The observation after executed tick `tick`.
    fn at(&self, tick: u64) -> Obs {
        let o = self.log[usize::try_from(tick - 1).unwrap()];
        assert_eq!(o.tick, tick, "probe log out of step");
        o
    }

    /// The first executed tick `>= from` with `side`'s coil in state `on`.
    fn first(&self, side: Side, on: bool, from: u64) -> Option<u64> {
        self.log
            .iter()
            .find(|o| o.tick >= from && o.on[side.index()] == on)
            .map(|o| o.tick)
    }
}

impl SimStep for Probe {
    fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
        self.table.step_1ms(tick, inputs, out);
        let o = observe(self.table.sim(), self.ids, tick);
        self.log.push(o);
    }
}

/// A sim loop over `sim` and the producer feeding its input ring.
fn looped<S: SimStep>(sim: S) -> (SimLoop<S>, SpscProducer<InputEvent>) {
    let (tx, rx) = channel(1 << 12);
    let (w, _r) = TripleBuffer::new(FrameStateSnapshot::default());
    (SimLoop::new(sim, rx, w), tx)
}

/// Mid-way through executed tick `tick` on the loop's current clock (its
/// epoch after every drop so far): `tick_end_ns(epoch, tick - 1) + TICK_NS/2`.
fn mid_tick<S: SimStep>(lp: &SimLoop<S>, tick: u64) -> u64 {
    tick_end_ns(lp.epoch_ns(), tick - 1) + TICK_NS / 2
}

/// One wake-up of the sim thread at `now` (the brief's sequence): newly
/// dropped ticks move the epoch before the due ticks run. Returns
/// `(due, newly_dropped)`.
fn wake<S: SimStep>(clock: &mut SimClock, lp: &mut SimLoop<S>, now: u64) -> (u32, u64) {
    let before = clock.dropped_ticks();
    let due = clock.advance(now);
    let newly = clock.dropped_ticks() - before;
    if newly > 0 {
        lp.skip_ticks(newly);
    }
    if due > 0 {
        lp.run_ticks(due);
    }
    (due, newly)
}

// ---------------------------------------------------------------------------
// 7. Control
// ---------------------------------------------------------------------------

/// REQ-LAT-004 control (issue #81): with a 1 s startup epoch and no dropped
/// ticks, a left-flipper press stamped mid tick 501 energises the coil in
/// tick 501, not in tick 1501 (the defect gates the same stamp against the
/// sim loop's epoch and the simulation's epoch 0, adding the epoch).
#[test]
fn control_epoch_1s_press_is_energised_in_tick_501_not_1501() {
    let _g = shared();
    let energised = on_big_stack(|| {
        let (mut lp, mut tx) = looped(Probe::new());
        lp.set_epoch_ns(SECOND_NS);
        tx.push(InputEvent::button(
            mid_tick(&lp, 501),
            CabinetInput::LeftFlipper,
            true,
        ))
        .unwrap();
        lp.run_ticks(1600);
        lp.sim().first(Side::Left, true, 1)
    });
    assert_eq!(
        energised,
        Some(501),
        "REQ-LAT-004: epoch 1 s, no drops: press mid tick 501 must energise in tick 501 \
         (1501 is the epoch-mismatch defect)"
    );
}

// ---------------------------------------------------------------------------
// 1. Headless regression over epoch x drops
// ---------------------------------------------------------------------------

/// The tick the drops are applied after (well before the press).
const DROP_AFTER_TICK: u64 = 100;

/// Runs the REQ-LAT-004 press scenario for `(epoch, drops)` and returns the
/// failures (empty when it holds).
fn press_scenario(epoch: u64, drops: u64) -> Vec<String> {
    let mut fails = Vec::new();
    let tag = format!("(epoch {} ms, drops {drops})", epoch / TICK_NS);
    let (mut lp, mut tx) = looped(Probe::new());
    lp.set_epoch_ns(epoch);
    lp.run_ticks(DROP_AFTER_TICK as u32);
    if drops > 0 {
        lp.skip_ticks(drops);
    }
    if lp.epoch_ns() != epoch + drops * TICK_NS {
        fails.push(format!(
            "{tag}: epoch after skip_ticks is {} ns, expected {}",
            lp.epoch_ns(),
            epoch + drops * TICK_NS
        ));
    }
    let left = CabinetInput::LeftFlipper;
    // Press mid tick 501, release mid tick 540; a second press mid tick 600
    // held to tick 800 measures the full sweep (the bat at rest on its stop).
    tx.push(InputEvent::button(mid_tick(&lp, 501), left, true))
        .unwrap();
    tx.push(InputEvent::button(mid_tick(&lp, 540), left, false))
        .unwrap();
    tx.push(InputEvent::button(mid_tick(&lp, 600), left, true))
        .unwrap();
    lp.run_ticks(800 - DROP_AFTER_TICK as u32);
    if lp.tick() != 800 {
        fails.push(format!(
            "{tag}: loop tick {} after 800 executed ticks",
            lp.tick()
        ));
        return fails;
    }
    let p = lp.sim();
    let first_on = p.first(Side::Left, true, 1);
    if first_on != Some(501) {
        fails.push(format!(
            "{tag}: REQ-LAT-004 coil first energised in tick {first_on:?}, expected 501 \
             (the delivering tick)"
        ));
        return fails;
    }
    let (o500, o501) = (p.at(500), p.at(501));
    if o500.on[0] || !o501.on[0] {
        fails.push(format!(
            "{tag}: REQ-LAT-004 energised 500={} 501={}",
            o500.on[0], o501.on[0]
        ));
    }
    if !(o501.vel[0] != 0.0 || o501.angle[0] != o500.angle[0]) {
        fails.push(format!(
            "{tag}: REQ-LAT-004 bat does not move in tick 501: {o500:?} -> {o501:?}"
        ));
    }
    // Full sweep: the bat resting on its end stop after holding to tick 800.
    let full = p.at(800).angle[0];
    let settled = (full - p.at(780).angle[0]).abs();
    if full == 0.0 || settled > 0.01 * full.abs() {
        fails.push(format!(
            "{tag}: full sweep not measured at rest: angle 780 {} rad, 800 {full} rad",
            p.at(780).angle[0]
        ));
    } else {
        let best = (501..=520)
            .map(|t| p.at(t).angle[0] / full)
            .fold(f32::MIN, f32::max);
        if best < 0.9 {
            fails.push(format!(
                "{tag}: REQ-LAT-004 bat reached only {:.1} % of its sweep ({full} rad) \
                 by tick 520",
                best * 100.0
            ));
        }
    }
    let (o539, o540) = (p.at(539), p.at(540));
    if !o539.on[0] || o540.on[0] {
        fails.push(format!(
            "{tag}: REQ-LAT-004 release mid tick 540 must de-energise in tick 540: \
             energised 539={} 540={}",
            o539.on[0], o540.on[0]
        ));
    }
    let second = p.first(Side::Left, true, 541);
    if second != Some(600) {
        fails.push(format!(
            "{tag}: REQ-LAT-004 second press mid tick 600 energised in {second:?}"
        ));
    }
    if p.first(Side::Right, true, 1).is_some() {
        fails.push(format!("{tag}: right coil energised without a press"));
    }
    fails
}

/// REQ-LAT-004 headless regression (issue #81 acceptance 1): Apex Circuit
/// behind `SimLoop`, epoch in {0, 1 s, 10 s} x dropped ticks in
/// {0, 120, 5000} before the press. A left-flipper press stamped mid tick 501
/// is energised in tick 501 (not 500), its bat moves in tick 501, reaches
/// 90 % of its sweep by tick 520, and the release stamped mid tick 540
/// de-energises the coil in tick 540.
#[test]
fn press_mid_tick_501_is_applied_in_tick_501_for_every_epoch_and_drop_count() {
    let _g = shared();
    let fails = on_big_stack(|| {
        let mut fails = Vec::new();
        for epoch in EPOCHS {
            for drops in [0, 120, 5000] {
                fails.extend(press_scenario(epoch, drops));
            }
        }
        fails
    });
    assert!(fails.is_empty(), "REQ-LAT-004:\n{}", fails.join("\n"));
}

// ---------------------------------------------------------------------------
// 2. Property tests
// ---------------------------------------------------------------------------

/// A stub sim with the simulation's input gate (epoch 0: an event is applied
/// in tick `k` iff its stamp is `<= tick_end_ns(0, k)`; later stamps wait in
/// the sim). Records, per event, the tick that received it, the stamp it was
/// handed and the tick that applied it.
#[derive(Default)]
struct GateStub {
    pending: Vec<(u64, InputEvent)>,
    /// (received tick, handed event, applied tick)
    applied: Vec<(u64, InputEvent, u64)>,
    received: usize,
}

impl SimStep for GateStub {
    fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
        out.tick = tick;
        self.received += inputs.len();
        self.pending.extend(inputs.iter().map(|e| (tick, *e)));
        let end = tick_end_ns(0, tick);
        let mut i = 0;
        while i < self.pending.len() {
            if self.pending[i].1.timestamp_ns <= end {
                let (rx, ev) = self.pending.remove(i);
                self.applied.push((rx, ev, tick));
            } else {
                i += 1;
            }
        }
    }
}

/// The delivery tick of `stamp` on a loop whose epoch is `epoch`: the tick
/// whose window `(epoch + (k-1) ms, epoch + k ms]` contains it.
fn delivery_tick(epoch: u64, stamp: u64) -> u64 {
    (stamp - epoch).div_ceil(TICK_NS)
}

/// REQ-LAT-004 property (issue #81 acceptance 2), stub sim with the
/// simulation's epoch-0 gate: over epoch 0..=10 s, drops 0..=5000 (applied
/// at a tick before the press), press tick 1..=2000, a stamp anywhere in the
/// press tick's window and either flipper, the event is received and applied
/// in its delivery tick, handed to the sim re-stamped on the tick clock
/// (raw stamp minus the loop's epoch).
#[test]
fn prop_stub_event_applied_in_delivery_tick() {
    let _g = shared();
    let cfg = Config {
        cases: 256,
        failure_persistence: None,
        ..Config::default()
    };
    let mut runner =
        TestRunner::new_with_rng(cfg, TestRng::from_seed(RngAlgorithm::ChaCha, &[0x81; 32]));
    let strat = (
        0u64..=10 * SECOND_NS,
        0u64..=5000,
        1u64..=2000,
        0u64..2000,
        1u64..=TICK_NS,
        any::<bool>(),
    );
    runner
        .run(&strat, |(epoch, drops, press, drop_seed, offset, left)| {
            let drop_at = drop_seed % press; // 0..press-1
            let input = if left {
                CabinetInput::LeftFlipper
            } else {
                CabinetInput::RightFlipper
            };
            let (mut lp, mut tx) = looped(GateStub::default());
            lp.set_epoch_ns(epoch);
            lp.run_ticks(u32::try_from(drop_at).unwrap());
            lp.skip_ticks(drops);
            let stamp = tick_end_ns(lp.epoch_ns(), press - 1) + offset;
            prop_assert_eq!(delivery_tick(lp.epoch_ns(), stamp), press);
            tx.push(InputEvent::button(stamp, input, true)).unwrap();
            lp.run_ticks(u32::try_from(press - drop_at + 5).unwrap());
            let s = lp.sim();
            prop_assert_eq!(s.received, 1, "REQ-LAT-004: event received once");
            prop_assert_eq!(s.applied.len(), 1, "REQ-LAT-004: event applied");
            let (rx, ev, at) = s.applied[0];
            prop_assert_eq!(
                rx,
                press,
                "REQ-LAT-004: received in the delivery tick (epoch {}, drops {})",
                epoch,
                drops
            );
            prop_assert_eq!(
                at,
                press,
                "REQ-LAT-004: applied in the delivery tick (epoch {}, drops {})",
                epoch,
                drops
            );
            prop_assert_eq!(
                ev.timestamp_ns,
                stamp - (epoch + drops * TICK_NS),
                "REQ-LAT-004: handed on the tick clock"
            );
            Ok(())
        })
        .unwrap();
}

/// REQ-LAT-004 property (issue #81 acceptance 2), real Apex Circuit
/// flippers, fewer cases (24: each loads the table): over epoch 0..=10 s,
/// drops 0..=5000, press tick 1..=2000 and either flipper, the coil is
/// first energised in the delivery tick.
#[test]
fn prop_real_flipper_energised_in_delivery_tick() {
    let _g = shared();
    on_big_stack(|| {
        let cfg = Config {
            cases: 24,
            failure_persistence: None,
            ..Config::default()
        };
        let mut runner =
            TestRunner::new_with_rng(cfg, TestRng::from_seed(RngAlgorithm::ChaCha, &[0x18; 32]));
        let strat = (
            0u64..=10 * SECOND_NS,
            0u64..=5000,
            1u64..=2000,
            0u64..2000,
            1u64..=TICK_NS,
            any::<bool>(),
        );
        runner
            .run(&strat, |(epoch, drops, press, drop_seed, offset, left)| {
                let drop_at = drop_seed % press;
                let side = if left { Side::Left } else { Side::Right };
                let (mut lp, mut tx) = looped(Probe::new());
                lp.set_epoch_ns(epoch);
                lp.run_ticks(u32::try_from(drop_at).unwrap());
                lp.skip_ticks(drops);
                let stamp = tick_end_ns(lp.epoch_ns(), press - 1) + offset;
                tx.push(InputEvent::button(stamp, side.input(), true))
                    .unwrap();
                lp.run_ticks(u32::try_from(press - drop_at + 5).unwrap());
                prop_assert_eq!(
                    lp.sim().first(side, true, 1),
                    Some(press),
                    "REQ-LAT-004: {:?} energised in the delivery tick \
                         (epoch {}, drops {}, drop at {})",
                    side,
                    epoch,
                    drops,
                    drop_at
                );
                Ok(())
            })
            .unwrap();
    });
}

// ---------------------------------------------------------------------------
// 3. Stall
// ---------------------------------------------------------------------------

/// REQ-LAT-004 stall (issue #81 acceptance 3; DECISIONS "REQ-LAT-004 bounds
/// inputs that arrive during a stall separately"): after T = 100 ticks from
/// S0 = 1 s, a 20 ms stall with cap 8 runs 8 catch-up ticks and drops 12.
/// With S = S0 + 100 ms: press A (left) stamped S + 5 ms (inside the dropped
/// time) is energised in the first catch-up tick T+1; press B (right)
/// stamped S + 19 ms is energised in tick T+7, the catch-up tick ending at
/// S + 19 ms, neither earlier nor later. After the catch-up the plain bound
/// holds again: an event stamped inside the next tick's window is applied in
/// that tick.
#[test]
fn stall_presses_are_applied_in_the_first_catch_up_tick_whose_end_is_not_before_them() {
    let _g = shared();
    const S0: u64 = SECOND_NS;
    const T: u64 = 100;
    on_big_stack(|| {
        let (mut lp, mut tx) = looped(Probe::new());
        let mut clock = SimClock::new(S0);
        lp.set_epoch_ns(S0);
        for k in 1..=T {
            assert_eq!(wake(&mut clock, &mut lp, S0 + k * TICK_NS), (1, 0));
        }
        assert_eq!(lp.tick(), T);
        let s = S0 + T * TICK_NS;
        tx.push(InputEvent::button(
            s + 5 * TICK_NS,
            CabinetInput::LeftFlipper,
            true,
        ))
        .unwrap();
        tx.push(InputEvent::button(
            s + 19 * TICK_NS,
            CabinetInput::RightFlipper,
            true,
        ))
        .unwrap();
        let (due, dropped) = wake(&mut clock, &mut lp, s + 20 * TICK_NS);
        assert_eq!(
            (due, dropped),
            (MAX_CATCH_UP_TICKS, 12),
            "a 20 ms stall: 8 catch-up ticks, 12 dropped"
        );
        assert_eq!(lp.tick(), T + 8);
        // Catch-up tick j ends at S + 12 ms + j ms.
        for j in 1..=8u64 {
            assert_eq!(
                tick_end_ns(lp.epoch_ns(), T + j),
                s + (12 + j) * TICK_NS,
                "catch-up tick {j} end"
            );
        }
        let p = lp.sim();
        assert!(!p.at(T).on[0] && !p.at(T).on[1]);
        assert_eq!(
            p.first(Side::Left, true, 1),
            Some(T + 1),
            "REQ-LAT-004: press 5 ms into the stall (dropped time) applies in the first \
             catch-up tick"
        );
        assert_eq!(
            p.first(Side::Right, true, 1),
            Some(T + 7),
            "REQ-LAT-004: press 19 ms into the stall applies in the catch-up tick ending \
             at 19 ms"
        );
        // After the catch-up: the plain bound. Release A stamped inside tick
        // T+9's window (S + 20 ms, S + 21 ms] and wake at its end.
        tx.push(InputEvent::button(
            s + 20 * TICK_NS + 300_000,
            CabinetInput::LeftFlipper,
            false,
        ))
        .unwrap();
        assert_eq!(wake(&mut clock, &mut lp, s + 21 * TICK_NS), (1, 0));
        let p = lp.sim();
        assert!(p.at(T + 8).on[0], "left still held in T+8");
        assert!(
            !p.at(T + 9).on[0],
            "REQ-LAT-004: after the catch-up an event is applied in its own tick (T+9)"
        );
        // And once more a tick later, at the very end of the window.
        tx.push(InputEvent::button(
            s + 22 * TICK_NS,
            CabinetInput::LeftFlipper,
            true,
        ))
        .unwrap();
        assert_eq!(wake(&mut clock, &mut lp, s + 22 * TICK_NS), (1, 0));
        assert_eq!(
            lp.sim().first(Side::Left, true, T + 9),
            Some(T + 10),
            "REQ-LAT-004: press stamped at the end of tick T+10 applies in T+10"
        );
    });
}

// ---------------------------------------------------------------------------
// 4. Late forwarding
// ---------------------------------------------------------------------------

/// REQ-LAT-004 late forwarding (issue #81 acceptance 4), stub sim with the
/// simulation's epoch-0 gate, epochs {0, 1 s, 10 s}: an event stamped inside
/// tick 5's window but pushed after tick 10 has run is applied in tick 11
/// (not dropped, not delayed further); an event stamped inside tick 15's
/// window, pushed after tick 11, stays queued until tick 15.
#[test]
fn late_forwarded_event_is_applied_in_the_next_executed_tick() {
    let _g = shared();
    for epoch in EPOCHS {
        let (mut lp, mut tx) = looped(GateStub::default());
        lp.set_epoch_ns(epoch);
        lp.run_ticks(10);
        tx.push(InputEvent::button(
            mid_tick(&lp, 5),
            CabinetInput::LeftFlipper,
            true,
        ))
        .unwrap();
        lp.run_ticks(1);
        let s = lp.sim();
        assert_eq!(s.applied.len(), 1, "REQ-LAT-004 epoch {epoch}: not dropped");
        assert_eq!(
            (s.applied[0].0, s.applied[0].2),
            (11, 11),
            "REQ-LAT-004 epoch {epoch}: late event applied in the next executed tick"
        );
        tx.push(InputEvent::button(
            tick_end_ns(lp.epoch_ns(), 15) - 300_000,
            CabinetInput::RightFlipper,
            false,
        ))
        .unwrap();
        lp.run_ticks(9);
        let s = lp.sim();
        assert_eq!(s.applied.len(), 2, "REQ-LAT-004 epoch {epoch}");
        assert_eq!(
            (s.applied[1].0, s.applied[1].2),
            (15, 15),
            "REQ-LAT-004 epoch {epoch}: a future event waits for the tick whose end is \
             not before it"
        );
    }
}

/// REQ-LAT-004 late forwarding with the real Apex Circuit flipper (epoch
/// 1 s): a press stamped inside tick 5's window pushed after tick 10 has run
/// energises the coil in tick 11.
#[test]
fn late_forwarded_press_energises_the_real_flipper_in_the_next_tick() {
    let _g = shared();
    let first = on_big_stack(|| {
        let (mut lp, mut tx) = looped(Probe::new());
        lp.set_epoch_ns(SECOND_NS);
        lp.run_ticks(10);
        tx.push(InputEvent::button(
            mid_tick(&lp, 5),
            CabinetInput::LeftFlipper,
            true,
        ))
        .unwrap();
        lp.run_ticks(20);
        lp.sim().first(Side::Left, true, 1)
    });
    assert_eq!(
        first,
        Some(11),
        "REQ-LAT-004: late press applied in tick 11"
    );
}

// ---------------------------------------------------------------------------
// Thread-test clocks
// ---------------------------------------------------------------------------

/// A clock based at 1 s: `1 s + (monotonic now - first reading) + jump`,
/// the first reading taken on the first call. `jump` lets a test inject a
/// stall (the clock leaps forward once).
struct BasedClock {
    first: AtomicU64,
    jump: AtomicU64,
    first_thread: OnceLock<String>,
}

impl BasedClock {
    const fn new() -> Self {
        Self {
            first: AtomicU64::new(0),
            jump: AtomicU64::new(0),
            first_thread: OnceLock::new(),
        }
    }

    fn read(&self) -> u64 {
        let now = monotonic_now_ns().max(1);
        let base = match self
            .first
            .compare_exchange(0, now, Ordering::AcqRel, Ordering::Acquire)
        {
            Ok(_) => {
                let _ = self.first_thread.set(
                    std::thread::current()
                        .name()
                        .unwrap_or("<unnamed>")
                        .to_owned(),
                );
                now
            }
            Err(f) => f,
        };
        SECOND_NS + now.saturating_sub(base) + self.jump.load(Ordering::Acquire)
    }

    fn first_caller(&self) -> &str {
        self.first_thread
            .get()
            .map_or("<never called>", String::as_str)
    }
}

static HAPTIC_CLOCK: BasedClock = BasedClock::new();
fn haptic_clock() -> u64 {
    HAPTIC_CLOCK.read()
}

static PROFILED_PLAIN_CLOCK: BasedClock = BasedClock::new();
fn profiled_plain_clock() -> u64 {
    PROFILED_PLAIN_CLOCK.read()
}

static PROFILED_STALL_CLOCK: BasedClock = BasedClock::new();
fn profiled_stall_clock() -> u64 {
    PROFILED_STALL_CLOCK.read()
}

/// Hang guard of the thread tests (a deadline on polling, never asserted as
/// a duration).
const HANG_GUARD: Duration = Duration::from_secs(20);

/// Polls `cond` (yielding) until it holds or the hang guard expires.
fn poll(mut cond: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + HANG_GUARD;
    while Instant::now() < deadline {
        if cond() {
            return true;
        }
        std::thread::yield_now();
    }
    cond()
}

// ---------------------------------------------------------------------------
// 5. Thread test: haptic coil event
// ---------------------------------------------------------------------------

/// REQ-LAT-004 thread test (issue #81 acceptance 5): the real threads over
/// Apex Circuit with an input clock based at 1 s. A left-flipper press
/// stamped at 1 s + 50.5 ms (the end of tick 50 plus 0.5 ms slack) produces
/// a coil haptic event in tick 50..=60 (the delivering tick is about 51; the
/// window absorbs the sim thread's start offset and wake-up jitter), far
/// below tick 1050 (today: 1051).
#[test]
fn thread_press_at_tick_50_produces_coil_haptic_by_about_tick_51() {
    let _g = exclusive();
    let sim = on_big_stack(apex);
    let (htx, mut hrx) = haptics_ring::<HapticsTelemetryEvent>(4096);
    let (mut tx, rx) = channel(64);
    let mut cfg = ThreadsConfig::with_sim(Box::new(TableSim::new(sim, Some(htx))), rx);
    cfg.now_ns = haptic_clock;
    // The test keeps the haptics consumer to read the event ticks (the audio
    // thread would otherwise consume them).
    cfg.audio.haptics = None;
    tx.push(InputEvent::button(
        SECOND_NS + 50 * TICK_NS + TICK_NS / 2,
        CabinetInput::LeftFlipper,
        true,
    ))
    .unwrap();
    let threads = spawn_threads(cfg).expect("spawn");
    let mut coil: Option<HapticsTelemetryEvent> = None;
    let mut seen = 0u64;
    let found = poll(|| {
        hrx.drain(|e| {
            seen += 1;
            if coil.is_none() && e.source >> SOURCE_CLASS_SHIFT == SOURCE_CLASS_COIL {
                coil = Some(e);
            }
        });
        coil.is_some()
    });
    let stats = threads.stats();
    let _ = threads.shutdown();
    eprintln!(
        "REQ-LAT-004 thread test: coil haptic {coil:?} ({seen} haptic events); first clock \
         caller: {}; stats {stats:?}",
        HAPTIC_CLOCK.first_caller()
    );
    assert!(found, "REQ-LAT-004: no coil haptic event; stats {stats:?}");
    let tick = coil.unwrap().tick;
    assert!(
        (50..=60).contains(&tick),
        "REQ-LAT-004: coil haptic in tick {tick}, expected about 51 (50..=60); \
         1051 is the epoch defect"
    );
}

// ---------------------------------------------------------------------------
// 6. Profiler
// ---------------------------------------------------------------------------

/// Runs the REQ-LAT-005 deterministic profiler scenario for
/// `(epoch, drops)`: returns `(snapshot, failures)`.
fn profiled_scenario(epoch: u64, drops: u64) -> (pin_app::profiler::ProfilerSnapshot, Vec<String>) {
    const EVENTS: u64 = 200;
    const FIRST_EVENT_TICK: u64 = 200;
    const SPACING: u64 = 5;
    let tag = format!("(epoch {} ms, drops {drops})", epoch / TICK_NS);
    let mut fails = Vec::new();
    let profiler = Profiler::new();
    let now = Arc::new(AtomicU64::new(0));
    let epoch_atomic = Arc::new(AtomicU64::new(0));
    let clock_now = Arc::clone(&now);
    let sim = ProfiledSim::new(
        Box::new(TableSim::new(apex(), None)),
        profiler.sim,
        move || clock_now.load(Ordering::Acquire),
    )
    .with_epoch(Arc::clone(&epoch_atomic));
    let ids = flipper_ids(sim.inner().sim());
    let (mut lp, mut tx) = looped(sim);
    lp.set_epoch_ns(epoch);
    epoch_atomic.store(lp.epoch_ns(), Ordering::Release);
    // One tick: the clock reads the SDL-domain time at which it executes, the
    // end of the tick on the loop's epoch clock plus 0..200 us of processing.
    let step = |lp: &mut SimLoop<_>, now: &AtomicU64| {
        let k = lp.tick() + 1;
        let jitter = (k * 7919 % 200) * 1_000;
        now.store(tick_end_ns(lp.epoch_ns(), k) + jitter, Ordering::Release);
        lp.run_ticks(1);
    };
    let skip = |lp: &mut SimLoop<_>, n: u64, e: &AtomicU64| {
        lp.skip_ticks(n);
        e.store(lp.epoch_ns(), Ordering::Release);
    };
    while lp.tick() < 100 {
        step(&mut lp, &now);
    }
    if drops > 0 {
        skip(&mut lp, drops / 2, &epoch_atomic);
    }
    for i in 0..EVENTS {
        if i == EVENTS / 2 && drops > 0 {
            skip(&mut lp, drops - drops / 2, &epoch_atomic);
        }
        let k = FIRST_EVENT_TICK + i * SPACING;
        while lp.tick() < k - 1 {
            step(&mut lp, &now);
        }
        let side = if i % 4 < 2 { Side::Left } else { Side::Right };
        let pressed = i % 2 == 0;
        // Stamped 0..1 ms before the end of tick k.
        let before = (i * 37 % 1000) * 1_000;
        let stamp = tick_end_ns(lp.epoch_ns(), k) - before;
        tx.push(InputEvent::button(stamp, side.input(), pressed))
            .unwrap();
        step(&mut lp, &now);
        let on = lp
            .sim()
            .inner()
            .sim()
            .flipper_set()
            .is_energised(ids[side.index()]);
        if on != pressed {
            fails.push(format!(
                "{tag}: REQ-LAT-004 event {i} ({side:?} pressed={pressed}) not applied in its \
                 tick {k}"
            ));
        }
    }
    for _ in 0..10 {
        step(&mut lp, &now);
    }
    let snap = profiler.reader.snapshot();
    if snap.input.count != EVENTS {
        fails.push(format!(
            "{tag}: REQ-LAT-005 input.count {} != {EVENTS} coil-driving events",
            snap.input.count
        ));
    }
    if snap.input.p99 > INPUT_P99_TARGET_NS {
        fails.push(format!(
            "{tag}: REQ-LAT-005 input p99 {} ns > 1.5 ms: {:?}",
            snap.input.p99, snap.input
        ));
    }
    if snap.input_clamped != 0 {
        fails.push(format!(
            "{tag}: REQ-LAT-005 {} clamped input samples",
            snap.input_clamped
        ));
    }
    (snap, fails)
}

/// REQ-LAT-005 deterministic profiler path (issue #81 acceptance 6a): a
/// `ProfiledSim` over Apex Circuit with the loop's epoch mirrored into the
/// shared epoch (after `set_epoch_ns` and every `skip_ticks`), epoch in
/// {0, 1 s, 10 s} x drops in {0, 5000}, 200 flipper presses and releases
/// stamped 0..1 ms before their tick's end: input p99 <= 1.5 ms, one sample
/// per coil-driving event, no clamped samples (and each event energises or
/// releases its coil in its own tick, REQ-LAT-004).
#[test]
fn profiled_input_latency_is_on_the_tick_clock_for_every_epoch_and_drop_count() {
    let _g = shared();
    let fails = on_big_stack(|| {
        let mut fails = Vec::new();
        for epoch in EPOCHS {
            for drops in [0, 5000] {
                let (snap, f) = profiled_scenario(epoch, drops);
                eprintln!(
                    "REQ-LAT-005 (epoch {} ms, drops {drops}): input {:?}, clamped {}",
                    epoch / TICK_NS,
                    snap.input,
                    snap.input_clamped
                );
                fails.extend(f);
            }
        }
        fails
    });
    assert!(fails.is_empty(), "REQ-LAT-005:\n{}", fails.join("\n"));
}

/// The REQ-LAT-005 thread path: presses and releases stamped now on
/// `clock`, one every few ticks; with `stall`, the clock leaps 20 ms forward
/// once half way (an injected stall) and the next event is stamped after
/// the leap.
fn profiled_thread_run(clock: fn() -> u64, based: &'static BasedClock, stall: bool) {
    const EVENTS: u64 = 12;
    let sim = on_big_stack(|| Box::new(TableSim::new(apex(), None)));
    let (mut tx, rx) = channel(64);
    let (cfg, reader) = ThreadsConfig::with_profiled_sim(sim, rx, clock, Profiler::new());
    let threads = spawn_threads(cfg).expect("spawn");
    let ticks = |t: &Threads| t.stats().sim_ticks;
    assert!(
        poll(|| ticks(&threads) >= 20),
        "sim thread never ran: {:?}",
        threads.stats()
    );
    for i in 0..EVENTS {
        if stall && i == EVENTS / 2 {
            based.jump.store(20 * TICK_NS, Ordering::Release);
        }
        let from = ticks(&threads);
        assert!(
            poll(|| ticks(&threads) >= from + 5),
            "sim thread stopped: {:?}",
            threads.stats()
        );
        let side = if i % 4 < 2 { Side::Left } else { Side::Right };
        tx.push(InputEvent::button(clock(), side.input(), i % 2 == 0))
            .unwrap();
        let r = reader.clone();
        assert!(
            poll(|| r.snapshot().input.count > i),
            "REQ-LAT-005: event {i} never recorded: {:?}",
            reader.snapshot()
        );
    }
    let stats = threads.stats();
    let _ = threads.shutdown();
    let s = reader.snapshot();
    eprintln!(
        "REQ-LAT-005 thread path (stall {stall}): input {:?}, clamped {}, first clock caller \
         {}, stats {stats:?}",
        s.input,
        s.input_clamped,
        based.first_caller()
    );
    assert_eq!(s.input.count, EVENTS, "REQ-LAT-005: one sample per event");
    assert!(
        s.input.p99 <= INPUT_P99_TARGET_NS,
        "REQ-LAT-005 (stall {stall}): input p99 {} ns > 1.5 ms: {:?}",
        s.input.p99,
        s.input
    );
    assert_eq!(
        s.input_clamped, 0,
        "REQ-LAT-005 (stall {stall}): clamped samples"
    );
}

/// REQ-LAT-005 thread path without a stall (issue #81 acceptance 6b):
/// `ThreadsConfig::with_profiled_sim` over Apex Circuit with the 1 s-based
/// clock; events stamped now are applied by the next tick: p99 <= 1.5 ms,
/// no clamped samples.
#[test]
fn profiled_thread_path_input_latency_without_stall() {
    let _g = exclusive();
    profiled_thread_run(profiled_plain_clock, &PROFILED_PLAIN_CLOCK, false);
}

/// REQ-LAT-005 thread path with an injected 20 ms stall (issue #81
/// acceptance 6b): the clock leaps 20 ms once (8 catch-up ticks, 12
/// dropped); events stamped now are still applied by the next tick and
/// measured on the tick clock: p99 <= 1.5 ms, no clamped samples.
#[test]
fn profiled_thread_path_input_latency_with_stall() {
    let _g = exclusive();
    profiled_thread_run(profiled_stall_clock, &PROFILED_STALL_CLOCK, true);
}
