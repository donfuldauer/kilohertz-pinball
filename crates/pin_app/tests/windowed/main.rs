//! The windowed latency harness (KP-002, SPEC §7.5).
//!
//! * `verdict`: the pure checks a to d on a trace and the shutdown line
//!   (REQ-LAT-001, REQ-LAT-002, REQ-LAT-004), unit-tested here without a
//!   display.
//! * `driver`: the real windowed run (Xvfb, lavapipe, XTest key presses).
//!   `windowed_latency_60s` is `#[ignore]`d: it needs Xvfb, a release
//!   binary and a minute of wall time, and is run by the `windowed`
//!   workflow with `cargo test --release -p pin_app --test windowed --
//!   --ignored` (`scripts/windowed/run.sh`).
//! * Property, allocation and CLI tests of the trace on the public API.

mod driver;
mod verdict;

use pin_app::orchestrator::SimClock;
use pin_app::orchestrator::SimLoop;
use pin_app::orchestrator::trace::{FlipperBindings, RAISE_THRESHOLD_RAD, latency_trace};
use pin_core::snapshot::FlipperState;
use pin_core::spsc::channel;
use pin_core::{
    CabinetInput, FlipperId, FrameStateSnapshot, InputEvent, InputEventKind, SimStep, TICK_NS,
    TripleBuffer,
};
use proptest::prelude::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::atomic::{AtomicU64, Ordering};
use verdict::{
    Frame, Params, Row, Shutdown, Trace, evaluate, evaluate_text, parse_shutdown, parse_trace,
};

// ---------------------------------------------------------------------------
// Allocation guard (armed per thread).

struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
thread_local! { static ARMED: Cell<bool> = const { Cell::new(false) }; }

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if ARMED.try_with(Cell::get).unwrap_or(false) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if ARMED.try_with(Cell::get).unwrap_or(false) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn guarded<R>(f: impl FnOnce() -> R) -> (R, u64) {
    let before = ALLOCS.load(Ordering::Relaxed);
    ARMED.with(|a| a.set(true));
    let r = f();
    ARMED.with(|a| a.set(false));
    (r, ALLOCS.load(Ordering::Relaxed) - before)
}

// ---------------------------------------------------------------------------
// A controlled clock (per test thread) and a stub flipper step.

thread_local! { static NOW: Cell<u64> = const { Cell::new(0) }; }

fn now() -> u64 {
    NOW.with(Cell::get)
}

fn set_now(t: u64) {
    NOW.with(|c| c.set(t));
}

/// Two bats (ids 0 and 1, bound to the left and right flipper buttons);
/// a delivered press energises its bat `lag` ticks later (the bat then
/// rises until released, and falls back after a release).
#[derive(Clone)]
struct Bats {
    lag: u64,
    held: [Option<u64>; 2],
    angle: [f32; 2],
}

impl Bats {
    fn new(lag: u64) -> Self {
        Self {
            lag,
            held: [None; 2],
            angle: [0.0; 2],
        }
    }
}

impl SimStep for Bats {
    fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
        for e in inputs {
            if let InputEventKind::Button { input, pressed } = e.kind {
                let i = match input {
                    CabinetInput::LeftFlipper => 0,
                    CabinetInput::RightFlipper => 1,
                    _ => continue,
                };
                self.held[i] = pressed.then_some(tick);
            }
        }
        *out = FrameStateSnapshot {
            tick,
            ..FrameStateSnapshot::default()
        };
        for i in 0..2 {
            match self.held[i] {
                Some(t) if tick >= t + self.lag => self.angle[i] = (self.angle[i] + 0.1).min(0.9),
                Some(_) => {}
                None => self.angle[i] = (self.angle[i] - 0.05).max(0.0),
            }
            out.push_flipper(FlipperState {
                id: FlipperId(i as u16),
                angle_rad: self.angle[i],
                ..FlipperState::default()
            });
        }
    }
}

fn bindings() -> FlipperBindings {
    let mut b = FlipperBindings::new();
    b.bind(CabinetInput::LeftFlipper, 0);
    b.bind(CabinetInput::RightFlipper, 1);
    b
}

// ---------------------------------------------------------------------------
// Verdict: hand-written traces.

const T0: u64 = 10_000_000_000; // 10 s on the SDL clock

/// A press at `ts` applied (delivered) in a tick ending `late` ns later,
/// on time (1 ms drain gap), moving in the same tick.
fn press(seq: u64, ts: u64, late: u64) -> Row {
    let end = ts + late;
    let tick = (end - T0) / TICK_NS;
    Row {
        seq,
        input: "left_flipper".into(),
        down: true,
        sdl_ts_ns: ts,
        apply_tick: tick,
        apply_tick_end_sdl_ns: end,
        prev_tick_end_sdl_ns: end - TICK_NS,
        drain_sdl_ns: end + 50_000,
        prev_drain_sdl_ns: end - TICK_NS + 50_000,
        dropped_ticks_at_apply: 0,
        stall: false,
        move_tick: Some(tick),
        ..Row::default()
    }
}

/// Frames every 100 ms from `T0` (frame k presented at T0 + k * 100 ms,
/// drawing the tick of its snapshot read at the previous present).
fn frames(n: u64) -> Vec<Frame> {
    (0..n)
        .map(|k| Frame {
            frame: k,
            snap_tick: (k.max(1) - 1) * 100,
            present_ns: T0 + k * 100_000_000,
        })
        .collect()
}

/// Fills `frame` / `frames_to_photon` of the rows from the frame table the
/// way the app joins them (the first frame with a snapshot tick in
/// `[move_tick, release_tick]`); the run ends at the last frame's present.
fn joined(rows: Vec<Row>, frames: Vec<Frame>) -> Trace {
    let run_end_ns = frames.last().map_or(T0, |f| f.present_ns);
    joined_until(rows, frames, run_end_ns)
}

/// [`joined`] with an explicit `run_end_ns`.
fn joined_until(mut rows: Vec<Row>, frames: Vec<Frame>, run_end_ns: u64) -> Trace {
    let releases = verdict::release_ticks(&rows);
    for (r, release) in rows.iter_mut().zip(releases) {
        if let Some(m) = r.move_tick
            && let Some(f) = verdict::frame_showing(&frames, m, release)
        {
            r.frame = Some(f.frame);
            r.frame_snap_tick = Some(f.snap_tick);
            r.present_ns = Some(f.present_ns);
            // A press before the first presented frame counts frames
            // 0..=frame (O1).
            r.frames_to_photon = Some(
                match verdict::last_frame_at_or_before(&frames, r.sdl_ts_ns) {
                    Some(f0) => f.frame - f0,
                    None => f.frame + 1,
                },
            );
        }
    }
    Trace {
        rows,
        frames,
        overflows: 0,
        run_end_ns,
    }
}

fn params() -> Params {
    Params {
        min_presses: 1,
        expected_ticks: 60_000,
        ..Params::default()
    }
}

const OK_SHUTDOWN: Shutdown = Shutdown {
    ticks: 60_000,
    dropped: 0,
    frames: 600,
    input_dropped: 0,
};

/// REQ-LAT-004 check a: an on-time press passes; one applied in a tick
/// ending before its timestamp fails; 700 ms late with `stall = 0` fails.
#[test]
fn req_lat_004_check_a_on_time_early_and_late() {
    let ts = T0 + 1_234_567_891;
    let v = evaluate(
        &joined(vec![press(0, ts, 765_432)], frames(600)),
        &OK_SHUTDOWN,
        &params(),
    );
    assert!(v.a.passed(), "{:?}", v.a);
    assert!(v.passed(), "{}", v.report());

    let mut early = press(0, ts, 765_432);
    early.apply_tick_end_sdl_ns = ts - 1;
    let v = evaluate(&joined(vec![early], frames(600)), &OK_SHUTDOWN, &params());
    assert!(!v.a.passed());
    assert!(
        v.a.failures[0].contains("before its timestamp"),
        "{:?}",
        v.a
    );

    let late = press(0, ts, 700_000_000);
    let v = evaluate(&joined(vec![late], frames(600)), &OK_SHUTDOWN, &params());
    assert!(!v.a.passed());
    assert!(v.a.failures[0].contains("700000000 ns after"), "{:?}", v.a);
    assert_eq!(v.failed_checks(), vec!["a", "b"], "{}", v.report());

    // The bound is exactly one tick plus the tolerance.
    let edge = press(0, ts, TICK_NS + params().tolerance_ns);
    let v = evaluate(&joined(vec![edge], frames(600)), &OK_SHUTDOWN, &params());
    assert!(v.a.passed(), "{:?}", v.a);
    let over = press(0, ts, TICK_NS + params().tolerance_ns + 1);
    let v = evaluate(&joined(vec![over], frames(600)), &OK_SHUTDOWN, &params());
    assert!(!v.a.passed());
}

/// REQ-LAT-004 check a inside a stall: the observed stall
/// (`drain - prev_drain`) is added to the bound and no more.
#[test]
fn req_lat_004_check_a_stall_adds_the_observed_stall_only() {
    let ts = T0 + 2_000_000_000;
    let stall_ns = 20_000_000;
    let mut r = press(0, ts, 15_000_000);
    r.stall = true;
    r.prev_drain_sdl_ns = r.drain_sdl_ns - stall_ns;
    let v = evaluate(
        &joined(vec![r.clone()], frames(600)),
        &OK_SHUTDOWN,
        &params(),
    );
    assert!(v.a.passed(), "{:?}", v.a);
    assert_eq!(v.summary.stalled_presses, 1);
    // Beyond stall + bound.
    let mut beyond = r.clone();
    beyond.apply_tick_end_sdl_ns = ts + stall_ns + TICK_NS + params().tolerance_ns + 1;
    beyond.apply_tick = (beyond.apply_tick_end_sdl_ns - T0) / TICK_NS;
    beyond.move_tick = Some(beyond.apply_tick);
    let v = evaluate(&joined(vec![beyond], frames(600)), &OK_SHUTDOWN, &params());
    assert!(!v.a.passed(), "{:?}", v.a);
}

/// The `stall` flag cannot be inferred from the lateness: a late press
/// marked `stall = 1` without an observed scheduling gap gets no extra
/// allowance, and a late press with a gap but `stall = 0` none either.
#[test]
fn req_lat_004_stall_flag_is_not_inferred_from_lateness() {
    let ts = T0 + 3_000_000_000;
    let mut liar = press(0, ts, 700_000_000);
    liar.stall = true; // drain gap stays 1 ms
    let v = evaluate(&joined(vec![liar], frames(600)), &OK_SHUTDOWN, &params());
    assert!(!v.a.passed(), "a stall flag without a gap allows nothing");
    let mut unflagged = press(0, ts, 700_000_000);
    unflagged.prev_drain_sdl_ns = unflagged.drain_sdl_ns - 800_000_000;
    let v = evaluate(
        &joined(vec![unflagged], frames(600)),
        &OK_SHUTDOWN,
        &params(),
    );
    assert!(!v.a.passed(), "a gap without the flag allows nothing");
}

/// A press at `ts_ms` (ms after T0) marked as stalled with the stall
/// window `[prev_drain_ms, drain_ms)`, applied in the tick ending at
/// `end_ms` and moving in it.
fn stalled(seq: u64, input: &str, ts_ms: u64, end_ms: u64, window: (u64, u64)) -> Row {
    let mut r = press(seq, T0 + ts_ms * TICK_NS, (end_ms - ts_ms) * TICK_NS);
    r.input = input.into();
    r.stall = true;
    r.prev_drain_sdl_ns = T0 + window.0 * TICK_NS;
    r.drain_sdl_ns = T0 + window.1 * TICK_NS;
    r
}

/// REQ-LAT-004 check a (gate 30 round 12, finding 12): the stall allowance
/// is given only to a flagged press whose timestamp lies in the stall
/// window `[prev_drain_sdl_ns, drain_sdl_ns)`. The review's case (press at
/// 100 ms, stall 120 to 140 ms, applying tick ending at 120 ms: 20 ms late,
/// under the inflated 23 ms bound) fails once with the window named; a
/// press at 130 ms passes through the allowance; the lower bound is
/// inclusive, the upper exclusive; two presses of one catch-up batch (the
/// second delivered in a later tick of the batch) keep it.
#[test]
fn req_lat_004_verdict_stall_allowance_needs_the_press_in_the_stall_window() {
    let base = 5_000; // ms after T0, on the 100 ms frame grid
    let win = (base + 120, base + 140);
    let ordinary = TICK_NS + params().tolerance_ns;
    // The review's case.
    let r = stalled(0, "left_flipper", base + 100, base + 120, win);
    assert!(20 * TICK_NS > ordinary, "needs the allowance to pass");
    assert!(
        20 * TICK_NS
            <= verdict::coil_bound_ns(
                &{
                    let mut x = r.clone();
                    x.sdl_ts_ns = x.prev_drain_sdl_ns;
                    x
                },
                &params()
            ),
        "the inflated bound covers the 20 ms"
    );
    assert_eq!(verdict::coil_bound_ns(&r, &params()), ordinary);
    let v = evaluate(&joined(vec![r], frames(600)), &OK_SHUTDOWN, &params());
    assert!(!v.a.passed(), "{}", v.report());
    assert_eq!(v.a.failures.len(), 1, "recorded once: {:?}", v.a);
    assert!(
        v.a.failures[0].contains("outside the stall window")
            && v.a.failures[0].contains(&format!(
                "[{}, {})",
                T0 + win.0 * TICK_NS,
                T0 + win.1 * TICK_NS
            ))
            && v.a.failures[0].contains("press #0"),
        "{:?}",
        v.a
    );
    assert_eq!(v.summary.stalled_presses, 1);
    assert_eq!(v.summary.presses, 1);
    // Inside the window: 10 ms late, over the ordinary bound, passes.
    let r = stalled(0, "left_flipper", base + 130, base + 140, win);
    assert_eq!(
        verdict::coil_bound_ns(&r, &params()),
        ordinary + 20 * TICK_NS
    );
    let v = evaluate(&joined(vec![r], frames(600)), &OK_SHUTDOWN, &params());
    assert!(v.a.passed(), "{}", v.report());
    assert_eq!(v.summary.stalled_presses, 1);
    // Lower bound inclusive: a press stamped at the stall's start.
    let r = stalled(0, "left_flipper", base + 120, base + 140, win);
    let v = evaluate(&joined(vec![r], frames(600)), &OK_SHUTDOWN, &params());
    assert!(v.a.passed(), "{}", v.report());
    // Upper bound exclusive: a press stamped at the drain that ended it is
    // not in the stall (1 ms late, so only the flag fails it).
    let r = stalled(0, "left_flipper", base + 140, base + 141, win);
    let v = evaluate(&joined(vec![r], frames(600)), &OK_SHUTDOWN, &params());
    assert!(!v.a.passed(), "{}", v.report());
    assert!(
        v.a.failures[0].contains("outside the stall window"),
        "{:?}",
        v.a
    );
    // One catch-up batch: both rows carry the batch's window; the second
    // press is delivered in a later tick of the batch, 14 ms late.
    let rows = vec![
        stalled(0, "left_flipper", base + 121, base + 122, win),
        stalled(1, "right_flipper", base + 125, base + 139, win),
    ];
    let v = evaluate(&joined(rows, frames(600)), &OK_SHUTDOWN, &params());
    assert!(v.a.passed(), "{}", v.report());
    assert_eq!(v.summary.stalled_presses, 2);
}

/// REQ-LAT-001 check b (gate 30 round 12, finding 3): a frame presented
/// before the press cannot show it. The review's case (press at 100 ms,
/// moved in tick 101, the trace's frame drew tick 101 at 99 ms, both
/// tables agree) fails check b with the press, frame and both times named,
/// and the frame is not counted (no zero-latency sample: the only press
/// leaves nothing checked); a frame presented at the press's timestamp
/// passes. The row drains the press at its timestamp (gate 30 round 13,
/// finding 14: a frame presented before the drain is rejected too, so the
/// equal-time case needs a drain no later than the present).
#[test]
fn req_lat_001_verdict_rejects_frame_presented_before_the_press() {
    let ts = T0 + 100 * TICK_NS;
    let row = || {
        let mut r = press(0, ts, TICK_NS);
        assert_eq!(r.apply_tick, 101);
        r.move_tick = Some(101);
        r.drain_sdl_ns = ts;
        r.prev_drain_sdl_ns = ts - TICK_NS;
        r
    };
    let fr = frames_at(&[90, 99, 220, 59_900], &[80, 101, 210, 59_000]);
    let t = joined(vec![row()], fr);
    assert_eq!(t.rows[0].frame, Some(1));
    assert_eq!(t.rows[0].present_ns, Some(T0 + 99 * TICK_NS));
    let v = evaluate(&t, &OK_SHUTDOWN, &params());
    assert!(v.a.passed(), "{}", v.report());
    assert!(!v.b.passed(), "{}", v.report());
    assert!(
        v.b.failures[0].contains("press #0")
            && v.b.failures[0].contains("frame 1")
            && v.b.failures[0].contains(&format!("presented at {} ns", T0 + 99 * TICK_NS))
            && v.b.failures[0].contains(&format!("timestamp {ts} ns")),
        "{:?}",
        v.b
    );
    assert!(
        v.b.failures
            .iter()
            .any(|f| f == "no press could be checked"),
        "the frame is not counted: {:?}",
        v.b
    );
    assert_eq!(v.summary.max_frames_raw, 0);
    assert_eq!(v.summary.p50_photon_ms, 0.0);
    // Presented at the press's timestamp: accepted (0 frames, 0 ms). The
    // shutdown line counts the 4 frames of the table (finding 15).
    let fr = frames_at(&[90, 100, 220, 59_900], &[80, 101, 210, 59_000]);
    let shutdown = Shutdown {
        frames: 4,
        ..OK_SHUTDOWN
    };
    let v = evaluate(&joined(vec![row()], fr), &shutdown, &params());
    assert!(v.passed(), "{}", v.report());
}

/// REQ-LAT-004 / REQ-LAT-005: the press must take effect in the applying
/// tick: a bat raised 1 degree more than `coil_ticks` after the delivering
/// tick fails check a (the pre-#81 re-stamp bug delivered on time but
/// energised hundreds of ms late).
#[test]
fn req_lat_004_check_a_coil_takes_effect_in_the_applying_tick() {
    let ts = T0 + 4_000_000_000;
    // The bound is `Params::coil_ticks` (8: 1 degree needs about 4 ticks).
    for (delay, ok) in [(0u64, true), (4, true), (8, true), (9, false), (450, false)] {
        let mut r = press(0, ts, 500_000);
        r.move_tick = Some(r.apply_tick + delay);
        let v = evaluate(&joined(vec![r], frames(600)), &OK_SHUTDOWN, &params());
        assert_eq!(v.a.passed(), ok, "delay {delay}: {:?}", v.a);
    }
}

/// REQ-LAT-001 check b: 1 and 2 frames pass, 3 fail.
#[test]
fn req_lat_001_check_b_frame_counts() {
    // Frames present every 100 ms; frame k draws tick (k-1)*100. A press
    // at T0 + 950 ms + x: F0 = frame 9 (due time well before frame 10).
    for (move_ms, want) in [(950u64, 2u64), (1000, 2), (1001, 3)] {
        let ts = T0 + 950_000_000;
        let mut r = press(0, ts, 300_000);
        r.move_tick = Some(move_ms);
        r.apply_tick = move_ms;
        let v = evaluate(&joined(vec![r], frames(600)), &OK_SHUTDOWN, &params());
        assert_eq!(v.summary.max_frames_adjusted, want, "move {move_ms}");
        assert_eq!(v.b.passed(), want <= 2, "move {move_ms}: {:?}", v.b);
    }
    // One frame: a frame that read its snapshot after the move. Frames
    // here read at present - 1 ms.
    let fr: Vec<Frame> = (0..600)
        .map(|k| Frame {
            frame: k,
            snap_tick: k * 100,
            present_ns: T0 + k * 100_000_000 + 1_000_000,
        })
        .collect();
    let ts = T0 + 950_000_000;
    let mut r = press(0, ts, 300_000);
    r.move_tick = Some(951);
    let v = evaluate(&joined(vec![r], fr), &OK_SHUTDOWN, &params());
    assert_eq!(v.summary.max_frames_adjusted, 1);
    assert!(v.b.passed(), "{:?}", v.b);
}

/// REQ-LAT-001, REQ-LAT-003 check b (O1): a press before the first
/// presented frame counts from frame index 0, so a bat first shown in frame
/// 2 is 3 frames after it (not 2: "no frame before the press" is not
/// "frame 0 was"); a press after frame 0 presented, shown in frame 2, is 2.
/// The in-flight allowance keeps its meaning: one frame, never more.
#[test]
fn req_lat_001_check_b_press_before_the_first_frame() {
    let fr = frames_at(&[10, 20, 30], &[4, 4, 5]);
    let before = T0 + 5 * TICK_NS;
    let after = T0 + 15 * TICK_NS;
    assert_eq!(verdict::last_frame_at_or_before(&fr, before), None);
    assert_eq!(
        verdict::last_frame_at_or_before(&fr, T0 + 10 * TICK_NS),
        Some(0)
    );
    assert_eq!(verdict::last_frame_at_or_before(&[], after), None);
    assert_eq!(
        verdict::frame_count(&fr, before, 2, 0, 2, false),
        (3, 3, false)
    );
    assert_eq!(
        verdict::frame_count(&fr, before, 2, u64::MAX, 2, true),
        (3, 2, true),
        "one frame of allowance: the in-flight frame is frame 0"
    );
    assert_eq!(
        verdict::frame_count(&fr, after, 2, u64::MAX, 2, true),
        (2, 2, false)
    );
    // Through the verdict: frame 0 in flight before the coil deadline
    // (press + 11 ms): raw 3, shifted to 2, passes.
    let r = press(0, before, 300_000);
    let t = joined_until(vec![r], fr, T0 + 2_000_000_000);
    assert_eq!(t.rows[0].frame, Some(2));
    assert_eq!(t.rows[0].frames_to_photon, Some(3));
    let v = evaluate(&t, &OK_SHUTDOWN, &params());
    assert!(v.b.passed(), "{}", v.report());
    assert_eq!(v.summary.max_frames_raw, 3);
    assert_eq!(v.summary.max_frames_adjusted, 2);
    assert_eq!(v.summary.in_flight_shifted, 1);
    // Frame 0 presented after the coil deadline: no allowance, 3 fails.
    let late = frames_at(&[20, 30, 40], &[4, 4, 5]);
    let t = joined_until(vec![press(0, before, 300_000)], late, T0 + 2_000_000_000);
    assert_eq!(t.rows[0].frames_to_photon, Some(3));
    let v = evaluate(&t, &OK_SHUTDOWN, &params());
    assert!(!v.b.passed(), "{}", v.report());
    assert_eq!(v.summary.max_frames_raw, 3);
}

/// REQ-LAT-003 check d (O2): the shutdown line's `input dropped N` is
/// parsed and required; dropped input events (each a missing trace row)
/// fail completeness, none passes.
#[test]
fn req_lat_003_check_d_fails_on_dropped_input_events() {
    let line = "pin_app: 60000 sim ticks (0 dropped), 600 frames, input dropped 4, 0 haptic \
                events\n";
    assert_eq!(
        parse_shutdown(line).unwrap(),
        Shutdown {
            ticks: 60_000,
            dropped: 0,
            frames: 600,
            input_dropped: 4,
        }
    );
    // Without the field (the app always prints it): malformed.
    let e = parse_shutdown("pin_app: 60000 sim ticks (0 dropped), 600 frames, 0 haptic events\n")
        .unwrap_err();
    assert!(e.starts_with("malformed shutdown line"), "{e}");
    assert!(parse_shutdown("pin_app: 1 sim ticks (0 dropped), 2 frames, input dropped x").is_err());
    assert_eq!(
        parse_shutdown("pin_app: 1 sim ticks (0 dropped), 2 frames, input dropped 7")
            .unwrap()
            .input_dropped,
        7,
        "last field"
    );
    let t = joined(vec![press(0, T0 + 5_000_000_000, 500_000)], frames(600));
    let ok = evaluate(&t, &OK_SHUTDOWN, &params());
    assert!(ok.d.passed(), "{:?}", ok.d);
    let dropped = Shutdown {
        input_dropped: 4,
        ..OK_SHUTDOWN
    };
    let v = evaluate(&t, &dropped, &params());
    assert!(!v.d.passed());
    assert!(
        v.d.failures
            .iter()
            .any(|f| f.contains("4 input events dropped") && f.contains("missing trace row")),
        "{:?}",
        v.d
    );
    assert!(
        v.a.passed() && v.b.passed() && v.c.passed(),
        "{}",
        v.report()
    );
}

/// REQ-LAT-001 check b: a raw count of 3 passes only through the one-frame
/// in-flight allowance (frame 10 presents 0.5 ms after the press, before
/// the coil deadline); the raw count is gated, the allowance counted.
#[test]
fn req_lat_001_check_b_in_flight_frame_before_the_deadline() {
    // Frame 10 presents 0.5 ms after the press; the bat moves 1 ms after.
    let ts = T0 + 1_000_000_000 - 500_000;
    let mut r = press(0, ts, 800_000);
    r.move_tick = Some(1001);
    r.apply_tick = 1000;
    let v = evaluate(&joined(vec![r], frames(600)), &OK_SHUTDOWN, &params());
    assert!(v.b.passed(), "{}", v.report());
    assert_eq!(v.summary.max_frames_adjusted, 2);
    assert_eq!(v.summary.max_frames_raw, 3);
    assert_eq!(v.summary.in_flight_shifted, 1);
    assert!(v.b.info.contains("in-flight shifted 1"), "{}", v.b.info);
}

/// Frames for the in-flight table: frame k presented at `present_ms[k]`
/// (ms after T0) drawing tick `snap[k]`.
fn frames_at(present_ms: &[u64], snap: &[u64]) -> Vec<Frame> {
    present_ms
        .iter()
        .zip(snap)
        .enumerate()
        .map(|(k, (&p, &t))| Frame {
            frame: k as u64,
            snap_tick: t,
            present_ns: T0 + p * TICK_NS,
        })
        .collect()
}

/// REQ-LAT-001 check b (table-driven): the raw count from the press is
/// gated (`n_raw <= 2`), with exactly one allowance: when the frame in
/// flight at the press presented within `(ts, ts + bound_a + coil_ticks]`
/// the count may start after it (`n_adj <= 2`). Raw 1 and 2 pass; raw 3
/// with the in-flight frame inside that window passes (the review's case:
/// press at 100 ms, frames at 101, 220 and 340 ms, the bat first visible
/// at 340 ms); raw 3 with it outside fails; raw 4 fails even shifted; a
/// press 700 ms late fails check a and gets no allowance.
#[test]
fn req_lat_001_check_b_raw_count_with_one_in_flight_frame() {
    let p = Params {
        coil_ticks: 2,
        ..params()
    };
    let ts = T0 + 100 * TICK_NS;
    // A press at 100 ms applied in tick 101 (1 ms late; bound_a 3 ms).
    let deadline_ms = 100 + 3 + p.coil_ticks;
    // (in-flight present ms, in-flight snap, move tick, raw, adjusted,
    // shifted, pass)
    for (inflight_ms, inflight_snap, mv, raw, adj, shifted, pass) in [
        (101u64, 101u64, 101u64, 1u64, 1u64, false, true), // raw 1
        (101, 90, 101, 2, 2, false, true),                 // raw 2
        (101, 90, 102, 3, 2, true, true),                  // the review's case
        (deadline_ms, 90, 102, 3, 2, true, true),          // window edge
        (deadline_ms + 1, 90, 102, 3, 3, false, false),    // outside the window
    ] {
        let fr = frames_at(
            &[90, inflight_ms, 220, 340, 460, 59_900],
            &[80, inflight_snap, 101, 220, 340, 59_000],
        );
        let mut r = press(0, ts, TICK_NS);
        r.move_tick = Some(mv);
        // Tick 101 drained the press at 100.5 ms, before the 101 ms frame
        // that draws it (gate 30 round 13, finding 14: a frame presented
        // before the drain cannot show the press).
        r.drain_sdl_ns = ts + TICK_NS / 2;
        r.prev_drain_sdl_ns = r.drain_sdl_ns - TICK_NS;
        let v = evaluate(&joined(vec![r], fr), &OK_SHUTDOWN, &p);
        let case = format!("in-flight {inflight_ms} ms, move {mv}: {}", v.report());
        assert!(v.a.passed(), "{case}");
        assert_eq!(v.summary.max_frames_raw, raw, "{case}");
        assert_eq!(v.summary.max_frames_adjusted, adj, "{case}");
        assert_eq!(v.summary.in_flight_shifted, usize::from(shifted), "{case}");
        assert_eq!(v.b.passed(), pass, "{case}");
    }
    // Raw 4 (3 after the in-flight frame): fails even with the shift.
    let fr = frames_at(
        &[90, 101, 150, 220, 340, 59_900],
        &[80, 90, 95, 101, 220, 59_000],
    );
    let mut r = press(0, ts, TICK_NS);
    r.move_tick = Some(102);
    let v = evaluate(&joined(vec![r], fr), &OK_SHUTDOWN, &p);
    assert!(v.a.passed(), "{}", v.report());
    assert!(!v.b.passed(), "{}", v.report());
    assert_eq!(v.summary.max_frames_raw, 4);
    assert_eq!(v.summary.in_flight_shifted, 0);
    // 700 ms late: fails check a, and its raw 3 gets no allowance.
    let fr = frames_at(&[90, 101, 220, 900, 59_900], &[80, 90, 101, 800, 59_000]);
    let mut late = press(0, ts, 700 * TICK_NS);
    late.move_tick = Some(late.apply_tick);
    let v = evaluate(&joined(vec![late], fr), &OK_SHUTDOWN, &p);
    assert!(!v.a.passed(), "{}", v.report());
    assert!(!v.b.passed(), "{}", v.report());
    assert_eq!(v.summary.max_frames_raw, 3);
    assert_eq!(v.summary.in_flight_shifted, 0);
    assert!(
        v.b.failures[0].contains("no in-flight allowance"),
        "{:?}",
        v.b
    );
}

/// REQ-LAT-004, REQ-LAT-005 check a: a press whose bat never moved fails,
/// unless its applying tick ended within `coil_ticks + 2` ticks of the
/// run's end.
#[test]
fn req_lat_004_check_a_press_that_never_moved() {
    let p = params();
    let mut r = press(0, T0 + 30_000_000_000, 500_000);
    r.move_tick = None;
    let v = evaluate(&joined(vec![r.clone()], frames(600)), &OK_SHUTDOWN, &p);
    assert!(!v.a.passed());
    assert!(
        v.a.failures[0].contains("the bat never moved after the press was applied"),
        "{:?}",
        v.a
    );
    // Applied right before the run ended: no time to move.
    let tail = (p.coil_ticks + 2) * TICK_NS;
    let end = r.apply_tick_end_sdl_ns + tail;
    let v = evaluate(
        &joined_until(vec![r.clone()], frames(600), end),
        &OK_SHUTDOWN,
        &p,
    );
    assert!(v.a.passed(), "{:?}", v.a);
    let v = evaluate(
        &joined_until(vec![r], frames(600), end + 1),
        &OK_SHUTDOWN,
        &p,
    );
    assert!(!v.a.passed(), "{:?}", v.a);
}

/// REQ-LAT-001 (F4): the final-500 ms exclusion counts from the trace's
/// `run_end_ns`, not the last frame: a render freeze (no frames in the
/// last 5 s) still checks the presses made during it and fails them; a
/// marker earlier than a press is handled without underflow and (gate 30
/// round 13, finding 13) fails check d for each such press, which is not
/// excluded from check b.
#[test]
fn req_lat_001_render_freeze_cannot_hide_presses() {
    let frozen: Vec<Frame> = frames(550); // last present at T0 + 54.9 s
    let run_end = T0 + 60_000_000_000;
    let rows = vec![
        press(0, T0 + 30_000_000_000, 500_000),
        press(1, T0 + 57_000_000_000, 500_000),
        press(2, T0 + 58_000_000_000, 500_000),
    ];
    let v = evaluate(
        &joined_until(rows.clone(), frozen.clone(), run_end),
        &OK_SHUTDOWN,
        &params(),
    );
    assert!(!v.b.passed(), "{}", v.report());
    assert_eq!(v.summary.excluded_presses, 0);
    assert_eq!(
        v.b.failures
            .iter()
            .filter(|f| f.contains("no presented frame showed the bat raised while held"))
            .count(),
        2,
        "{:?}",
        v.b
    );
    // A marker before a press: no underflow, no panic; the two presses
    // after it are not excluded and each fails check d.
    let v = evaluate(
        &joined_until(rows, frozen, T0 + 40_000_000_000),
        &OK_SHUTDOWN,
        &params(),
    );
    assert_eq!(v.summary.excluded_presses, 0);
    assert!(v.a.passed(), "{:?}", v.a);
    assert!(!v.b.passed(), "{}", v.report());
    assert_eq!(
        v.d.failures
            .iter()
            .filter(|f| f.contains("after the run-end marker"))
            .count(),
        2,
        "{:?}",
        v.d
    );
}

/// REQ-LAT-003 (F10): the parser wants the frame header right after the
/// event table, each overflow counter exactly once, and the run-end line;
/// an empty frame table stays valid.
#[test]
fn req_lat_003_parser_is_strict_about_headers_and_trailer() {
    let (c, mut sim, mut fw) = latency_trace(bindings(), now);
    set_now(T0);
    sim.start(T0);
    let snap = FrameStateSnapshot::default();
    sim.begin_tick(T0 + TICK_NS, &snap);
    sim.delivered(
        1,
        &InputEvent::button(T0 + 300_000, CabinetInput::LeftFlipper, true),
    );
    sim.end_tick(1, &snap);
    fw.presented(
        true,
        &FrameStateSnapshot {
            tick: 1,
            ..FrameStateSnapshot::default()
        },
    );
    let text = c.collect(T0 + 5).to_csv();
    let t = parse_trace(&text).unwrap();
    assert_eq!(t.run_end_ns, T0 + 5);
    assert_eq!(t.frames.len(), 1);
    let header = format!("{}\n", verdict::FRAME_HEADER);
    for (bad, why) in [
        (text.replacen(&header, "", 1), "frame header missing"),
        (
            text.replace("input=0 move=0 frame=0 pending=0", "input=0"),
            "trailer needs",
        ),
        (text.replace("move=0", "input=0"), "twice"),
        (
            text.replace("pending=0", "pending=0 extra=1"),
            "unknown trailer counter",
        ),
        (
            text.replace(&format!("# run_end_ns={}\n", T0 + 5), ""),
            "run_end_ns",
        ),
    ] {
        let e = parse_trace(&bad).unwrap_err();
        assert!(e.contains(why), "{e} / {bad}");
    }
    // The trailer before the frame header is an error too.
    let swapped =
        text.replacen(&header, "", 1)
            .replacen("# run_end_ns", &format!("{header}# run_end_ns"), 1);
    assert!(parse_trace(&swapped).is_err());
    // Header with no rows: valid.
    let (c, ..) = latency_trace(bindings(), now);
    let empty = c.collect(1).to_csv();
    let t = parse_trace(&empty).unwrap();
    assert!(t.rows.is_empty() && t.frames.is_empty());
}

/// A trace text with no events and the given frame ids (present times 1 ms
/// apart).
fn trace_with_frame_ids(ids: &[u64]) -> String {
    let mut t = format!("{}\n\n{}\n", verdict::EVENT_HEADER, verdict::FRAME_HEADER);
    for (k, id) in ids.iter().enumerate() {
        t.push_str(&format!("{id},{},{}\n", k * 10, 1_000_000 * (k + 1)));
    }
    t.push_str("# overflows: input=0 move=0 frame=0 pending=0\n# run_end_ns=99000000\n");
    t
}

/// REQ-LAT-001 check b (gate 30 round 11): frame ids must be consecutive
/// from 0. A gap, a start at 1, a duplicate and a decreasing id are
/// rejected naming the line, the expected and the found id; an empty table
/// and two concatenated engine traces (second engine offset by the first's
/// last frame + 1) stay valid. The review's case (frames 0, 2, 3, 4 with
/// frame 2 meeting the deadline and the raise at frame 4, which check b
/// would pass as raw 4 less one allowance of two frames) never reaches
/// check b.
#[test]
fn req_lat_001_parse_trace_rejects_non_consecutive_frame_ids() {
    for (ids, line, want, found) in [
        (&[0u64, 1, 3][..], 6, 2, 3),
        (&[1, 2, 3][..], 4, 0, 1),
        (&[0, 1, 1][..], 6, 2, 1),
        (&[0, 1, 2, 1][..], 7, 3, 1),
        (&[0, 2, 3, 4][..], 5, 1, 2),
    ] {
        let e = parse_trace(&trace_with_frame_ids(ids)).unwrap_err();
        let msg = format!("trace line {line}: frame id {found}, expected {want}");
        assert!(e.starts_with(&msg), "{ids:?}: {e}");
    }
    assert!(
        parse_trace(&trace_with_frame_ids(&[]))
            .unwrap()
            .frames
            .is_empty()
    );
    assert_eq!(
        parse_trace(&trace_with_frame_ids(&[0, 1, 2, 3, 4, 5]))
            .unwrap()
            .frames
            .len(),
        6
    );
    // Two engine traces joined by the app: the second's ids continue.
    let second: Vec<u64> = (0..3).map(|i| 3 + i).collect();
    let first_ids = [0u64, 1, 2];
    let both: Vec<u64> = first_ids.iter().copied().chain(second).collect();
    assert_eq!(
        parse_trace(&trace_with_frame_ids(&both))
            .unwrap()
            .frames
            .len(),
        6
    );
    // The review's scenario through the whole text path: unreadable.
    let e = evaluate_text(
        &trace_with_frame_ids(&[0, 2, 3, 4]),
        "pin_app: 1000 sim ticks (0 dropped), 4 frames, input dropped 0, 0 haptic events\n",
        &Params::default(),
    )
    .unwrap_err();
    assert!(e.contains("frame id 2, expected 1"), "{e}");
}

/// REQ-LAT-001 check b: a press whose bat never moved, or that no frame
/// drew, fails; in the final 500 ms it is excluded and counted.
#[test]
fn req_lat_001_check_b_missing_move_and_end_exclusion() {
    let fr = frames(600); // last frame at T0 + 59.9 s
    let ts = T0 + 30_000_000_000;
    let mut r = press(0, ts, 500_000);
    r.move_tick = None;
    let v = evaluate(
        &joined(vec![r.clone()], fr.clone()),
        &OK_SHUTDOWN,
        &params(),
    );
    assert!(!v.b.passed());
    assert!(v.b.failures[0].contains("never moved"), "{:?}", v.b);
    let mut undrawn = press(0, ts, 500_000);
    undrawn.move_tick = Some(99_999);
    let v = evaluate(&joined(vec![undrawn], fr.clone()), &OK_SHUTDOWN, &params());
    assert!(!v.b.passed());
    // Final 500 ms: excluded, counted, and not failed.
    let end_ts = T0 + 59_900_000_000 - 400_000_000;
    let mut tail = press(1, end_ts, 500_000);
    tail.move_tick = None;
    let ok = press(0, ts, 500_000);
    let v = evaluate(&joined(vec![ok, tail], fr.clone()), &OK_SHUTDOWN, &params());
    assert!(v.b.passed(), "{:?}", v.b);
    assert_eq!(v.summary.excluded_presses, 1);
    // Just outside the window: checked (and failing without a move).
    let mut before = press(1, T0 + 59_900_000_000 - 500_000_000, 500_000);
    before.move_tick = None;
    let v = evaluate(&joined(vec![before], fr), &OK_SHUTDOWN, &params());
    assert!(!v.b.passed());
}

/// Check b: a row whose `frame` or `frames_to_photon` disagrees with the
/// frame table fails (the app's join is checked, not trusted).
#[test]
fn req_lat_001_check_b_recomputes_the_join() {
    let ts = T0 + 5_000_000_000;
    let mut t = joined(vec![press(0, ts, 500_000)], frames(600));
    t.rows[0].frame = Some(t.rows[0].frame.unwrap() + 1);
    assert!(!evaluate(&t, &OK_SHUTDOWN, &params()).b.passed());
    let mut t = joined(vec![press(0, ts, 500_000)], frames(600));
    assert_eq!(t.rows[0].frames_to_photon, Some(1));
    t.rows[0].frames_to_photon = Some(5);
    assert!(!evaluate(&t, &OK_SHUTDOWN, &params()).b.passed());
}

/// REQ-LAT-001 check b (B3): the app picks the first candidate frame whose
/// snapshot shows the bat raised (the angles are not in the file), so a
/// later candidate passes when it is consistent with the frame table; a
/// frame before the move, before the first candidate or disagreeing with
/// its frame-table row fails, and a press without a frame fails.
#[test]
fn req_lat_001_check_b_accepts_a_later_candidate_frame_only() {
    let ts = T0 + 5_000_000_000;
    let base = joined(vec![press(0, ts, 500_000)], frames(600));
    assert_eq!(base.rows[0].frame, Some(51));
    assert!(evaluate(&base, &OK_SHUTDOWN, &params()).b.passed());
    let set = |t: &mut Trace, f: u64| {
        let fr = t.frames[f as usize];
        let r = &mut t.rows[0];
        r.frame = Some(f);
        r.frame_snap_tick = Some(fr.snap_tick);
        r.present_ns = Some(fr.present_ns);
        r.frames_to_photon = Some(f - 50);
    };
    // Frame 51 drew the bat fallen back: the app took frame 52.
    let mut t = base.clone();
    set(&mut t, 52);
    let v = evaluate(&t, &OK_SHUTDOWN, &params());
    assert!(v.b.passed(), "{}", v.report());
    assert_eq!(v.summary.max_frames_raw, 2);
    // Frame 50 drew tick 4900, before the move: not a candidate.
    let mut t = base.clone();
    set(&mut t, 50);
    t.rows[0].frames_to_photon = Some(0);
    assert!(!evaluate(&t, &OK_SHUTDOWN, &params()).b.passed());
    // The row's snapshot tick disagrees with the frame table.
    let mut t = base.clone();
    set(&mut t, 52);
    t.rows[0].frame_snap_tick = Some(5000);
    assert!(!evaluate(&t, &OK_SHUTDOWN, &params()).b.passed());
    // No frame showed the bat raised.
    let mut t = base;
    t.rows[0].frame = None;
    t.rows[0].frames_to_photon = None;
    let v = evaluate(&t, &OK_SHUTDOWN, &params());
    assert!(
        v.b.failures
            .iter()
            .any(|f| f.contains("no presented frame showed the bat raised while held")),
        "{:?}",
        v.b
    );
}

/// REQ-LAT-002 check c: 60 dropped of 60 060 passes, 61 of 60 061 fails;
/// a short run fails.
#[test]
fn req_lat_002_check_c_dropped_ratio_and_run_length() {
    let t = joined(vec![press(0, T0 + 5_000_000_000, 500_000)], frames(600));
    let c = |ticks, dropped| {
        evaluate(
            &t,
            &Shutdown {
                ticks,
                dropped,
                frames: 600,
                input_dropped: 0,
            },
            &params(),
        )
        .c
    };
    assert!(c(60_000, 60).passed());
    assert!(!c(60_000, 61).passed());
    assert!(c(60_000, 0).passed());
    assert!(c(59_400, 0).passed(), "within 1 %");
    assert!(!c(59_399, 0).passed(), "a run that died early");
    assert!(!c(60_601, 0).passed());
    assert!(!c(0, 0).passed());
}

/// REQ-LAT-002 check c (gate 30, M14): the arithmetic cannot overflow.
/// Counters from a malformed shutdown line (`2^61` dropped) or `u64::MAX`
/// for both are rejected with the check's message, never a panic or a
/// wrap; the 0.1 % boundary (`dropped * 1000 <= ticks + dropped`) is
/// unchanged for normal values.
#[test]
fn req_lat_002_check_c_counters_never_overflow() {
    let t = joined(vec![press(0, T0 + 5_000_000_000, 500_000)], frames(600));
    let c = |ticks, dropped, expected| {
        evaluate(
            &t,
            &Shutdown {
                ticks,
                dropped,
                frames: 600,
                input_dropped: 0,
            },
            &Params {
                expected_ticks: expected,
                ..params()
            },
        )
        .c
    };
    let dropped_msg = |c: &verdict::Check| c.failures.iter().any(|f| f.contains("more than 0.1 %"));
    let line = "pin_app: 60000 sim ticks (2305843009213693952 dropped), 600 frames, input dropped \
                0, 0 haptic events\n";
    let sd = parse_shutdown(line).unwrap();
    assert_eq!(sd.dropped, 2_305_843_009_213_693_952);
    let v = evaluate(&t, &sd, &params());
    assert!(!v.c.passed() && dropped_msg(&v.c), "{:?}", v.c);
    // The text path (an app-written empty trace) rejects it the same way.
    let (tc, ..) = latency_trace(bindings(), now);
    let v = verdict::evaluate_text(&tc.collect(T0).to_csv(), line, &params()).unwrap();
    assert!(!v.c.passed() && dropped_msg(&v.c), "{:?}", v.c);
    assert!(v.report().contains("check c real time (REQ-LAT-002): FAIL"));
    let m = c(u64::MAX, u64::MAX, u64::MAX);
    assert!(!m.passed() && dropped_msg(&m), "{m:?}");
    let m = c(u64::MAX, 0, 1);
    assert!(!m.passed() && m.failures.iter().any(|f| f.contains("within 1 %")));
    assert!(c(u64::MAX, 0, u64::MAX).passed());
    // 1 in 1000 passes, 1 over it fails (dropped rule alone).
    assert!(!dropped_msg(&c(999, 1, 999)));
    assert!(dropped_msg(&c(998, 1, 998)));
    assert!(!dropped_msg(&c(59_940, 60, 59_940)));
    assert!(dropped_msg(&c(59_939, 60, 59_939)));
    // 1 % of the expected ticks: exact boundary.
    assert!(c(99, 0, 100).passed());
    assert!(!c(98, 0, 100).passed());
    assert!(c(101, 0, 100).passed());
    assert!(!c(102, 0, 100).passed());
}

/// The overall summary (gate 30, M5): a failed app exit makes the overall
/// line `verdict: FAIL (app exit status …)` whatever the measurements say,
/// and keeps the measurement report; a successful exit leaves the report
/// unchanged.
#[test]
fn req_lat_002_summary_carries_the_app_exit_status() {
    let pass = "check a: PASS\n| m | v |\nverdict: PASS\n";
    let fail = "check a: FAIL\nverdict: FAIL (checks a)\n";
    fn overall(s: &str) -> Vec<&str> {
        s.lines().filter(|l| l.starts_with("verdict: ")).collect()
    }
    assert_eq!(verdict::overall_report(pass, None), pass);
    assert_eq!(verdict::overall_report(fail, None), fail);
    let s = verdict::overall_report(pass, Some("exit status: 1"));
    assert_eq!(
        overall(&s),
        ["verdict: FAIL (app exit status exit status: 1)"]
    );
    assert!(
        s.contains("check a: PASS") && s.contains("| m | v |"),
        "{s}"
    );
    assert!(s.contains("measurements verdict: PASS"), "{s}");
    assert!(
        s.ends_with("verdict: FAIL (app exit status exit status: 1)\n"),
        "{s}"
    );
    let s = verdict::overall_report(fail, Some("signal: 9 (SIGKILL)"));
    assert_eq!(
        overall(&s),
        ["verdict: FAIL (app exit status signal: 9 (SIGKILL))"]
    );
    assert!(s.contains("measurements verdict: FAIL (checks a)"), "{s}");
}

/// Check d: overflows, too few presses and injected-vs-traced mismatches
/// fail.
#[test]
fn check_d_completeness() {
    let mut t = joined(vec![press(0, T0 + 5_000_000_000, 500_000)], frames(600));
    let p = Params {
        min_presses: 1,
        injected_presses: Some(1),
        ..Params::default()
    };
    assert!(evaluate(&t, &OK_SHUTDOWN, &p).d.passed());
    let p2 = Params {
        injected_presses: Some(2),
        ..p
    };
    assert!(!evaluate(&t, &OK_SHUTDOWN, &p2).d.passed());
    let p3 = Params {
        min_presses: 100,
        ..p
    };
    assert!(!evaluate(&t, &OK_SHUTDOWN, &p3).d.passed());
    t.overflows = 1;
    assert!(!evaluate(&t, &OK_SHUTDOWN, &p).d.passed());
    // Releases and other inputs are not presses.
    let mut t = joined(vec![press(0, T0 + 5_000_000_000, 500_000)], frames(600));
    t.rows[0].down = false;
    assert_eq!(evaluate(&t, &OK_SHUTDOWN, &p).summary.presses, 0);
    t.rows[0].down = true;
    t.rows[0].input = "launch".into();
    assert_eq!(evaluate(&t, &OK_SHUTDOWN, &p).summary.presses, 0);
}

/// The text path: a trace written by the app's own renderer parses with
/// the harness's independent parser; the shutdown line is found among
/// other stderr lines; the report names the failed checks.
#[test]
fn req_lat_003_text_round_trip_and_report() {
    let (c, mut sim, mut fw) = latency_trace(bindings(), now);
    set_now(T0);
    sim.start(T0);
    let snap = FrameStateSnapshot::default();
    sim.begin_tick(T0 + TICK_NS, &snap);
    sim.delivered(
        1,
        &InputEvent::button(T0 + 300_000, CabinetInput::LeftFlipper, true),
    );
    sim.end_tick(1, &snap);
    fw.presented(
        true,
        &FrameStateSnapshot {
            tick: 1,
            ..FrameStateSnapshot::default()
        },
    );
    let text = c.collect(T0 + 1_000_000_000).to_csv();
    let t = parse_trace(&text).unwrap();
    assert_eq!(t.rows.len(), 1);
    assert_eq!(t.rows[0].input, "left_flipper");
    assert_eq!(t.rows[0].apply_tick_end_sdl_ns, T0 + TICK_NS);
    assert_eq!(t.rows[0].move_tick, None);
    assert_eq!(t.frames.len(), 1);
    let stderr = "pin_app: table \"Grey Box\" loaded\n\
                  pin_app: 60000 sim ticks (3 dropped), 512 frames, input dropped 0, 9 haptic events\n\
                  pin_app: render: gpu ready after 1 ms\n";
    assert_eq!(
        parse_shutdown(stderr).unwrap(),
        Shutdown {
            ticks: 60_000,
            dropped: 3,
            frames: 512,
            input_dropped: 0,
        }
    );
    assert!(parse_shutdown("pin_app: render: x\n").is_err());
    assert!(parse_shutdown("pin_app: 12 sim ticks (x dropped), 1 frames").is_err());
    let v = verdict::evaluate_text(&text, stderr, &Params::default()).unwrap();
    let report = v.report();
    assert!(
        report.contains("check b key to photon in frames (REQ-LAT-001): FAIL"),
        "{report}"
    );
    // The press never moved its bat, 1 s before the run's end: checks a
    // (never moved, REQ-LAT-005) and b fail; d wants 100 presses.
    assert!(
        report.contains("verdict: FAIL (checks a, b, d)"),
        "{report}"
    );
    assert!(report.contains("| dropped ratio |"), "{report}");
    assert_eq!(verdict::percentile(&[1, 2, 3], 50), 2);
    assert_eq!(verdict::percentile(&[], 99), 0);
}

/// R11: the trace and shutdown-line parsers survive bounded random bytes
/// and every truncation of a valid trace.
#[test]
fn parsers_survive_random_bytes() {
    let mut rng = driver::Schedule::new(7);
    for _ in 0..3000 {
        let len = (rng.next_u64() % 400) as usize;
        let bytes: Vec<u8> = (0..len).map(|_| rng.next_u64() as u8).collect();
        let s = String::from_utf8_lossy(&bytes);
        let _ = parse_trace(&s);
        let _ = parse_shutdown(&s);
        let _ = parse_trace(&format!("{}\n{s}", verdict::EVENT_HEADER));
        let _ = parse_shutdown(&format!("pin_app: {s} sim ticks ({s} dropped), {s} frames"));
        let _ = parse_shutdown(&format!(
            "pin_app: 1 sim ticks (0 dropped), 2 frames, input dropped {s}"
        ));
    }
    let t = joined(vec![press(0, T0 + 5_000_000_000, 500_000)], frames(4));
    let (c, ..) = latency_trace(bindings(), now);
    let mut text = c.collect(T0).to_csv();
    // Splice a valid row in.
    text = text.replacen(
        "\n\n",
        &format!(
            "\n{}\n\n",
            "0,left_flipper,1,1,1,1000000,0,1000050,50,0,0,1,0,0,1,0"
        ),
        1,
    );
    assert_eq!(parse_trace(&text).unwrap().rows.len(), 1);
    for cut in 0..text.len() {
        let _ = parse_trace(&text[..cut]);
    }
    let _ = evaluate(&t, &OK_SHUTDOWN, &params());
}

// ---------------------------------------------------------------------------
// Property: the trace on a controlled clock with stalls and dropped ticks.

#[derive(Clone, Debug)]
struct Scenario {
    epoch: u64,
    /// (gap to the previous event's stamp in us, forwarding delay in us,
    /// left?, down?)
    events: Vec<(u64, u64, bool, bool)>,
    /// (wake index, stall in us)
    stalls: Vec<(usize, u64)>,
    /// Wake jitter seed and frame period (us).
    jitter: u64,
    frame_period_us: u64,
    lag: u64,
}

fn scenario() -> impl Strategy<Value = Scenario> {
    (
        0u64..20_000_000_000,
        prop::collection::vec(
            (0u64..40_000, 0u64..3_000, any::<bool>(), any::<bool>()),
            1..40,
        ),
        prop::collection::vec((0usize..600, 2_000u64..40_000), 0..6),
        any::<u64>(),
        5_000u64..150_000,
        0u64..3,
    )
        .prop_map(
            |(epoch, events, stalls, jitter, frame_period_us, lag)| Scenario {
                epoch,
                events,
                stalls,
                jitter,
                frame_period_us,
                lag,
            },
        )
}

/// One executed tick as the "sim thread" ran it.
struct Executed {
    tick: u64,
    end: u64,
    drain: u64,
    /// The bats' angles (flippers 0 and 1) at the tick's end.
    angles: [f32; 2],
}

/// The angles of flippers 0 and 1 in `snap` (0 when absent).
fn bat_angles(snap: &FrameStateSnapshot) -> [f32; 2] {
    let mut a = [0.0; 2];
    for st in snap.flippers() {
        if let Some(slot) = a.get_mut(usize::from(st.id.0)) {
            *slot = st.angle_rad;
        }
    }
    a
}

/// What [`run_scenario`] observed: the trace text, the executed ticks, the
/// events' (raw stamp, arrival time) and the bat angles each presented
/// frame drew (by frame index).
type ScenarioRun = (String, Vec<Executed>, Vec<(u64, u64)>, Vec<[f32; 2]>);

/// Runs `s` like the sim thread does (accumulator clock, catch-up cap,
/// dropped ticks skipped), with the render thread's frames interleaved
/// (each drawing the latest published snapshot).
fn run_scenario(s: &Scenario) -> ScenarioRun {
    let (mut tx, rx) = channel::<InputEvent>(256);
    let (w, mut reader) = TripleBuffer::new(FrameStateSnapshot::default());
    let mut l = SimLoop::new(Bats::new(s.lag), rx, w);
    let (c, sim, mut fw) = latency_trace(bindings(), now);
    l.set_latency_trace(sim);
    let start = s.epoch;
    let mut clock = SimClock::new(start);
    l.set_epoch_ns(start);
    // Event stamps after the start, arrivals in order.
    let mut evs = Vec::new();
    let mut ts = start;
    let mut arrival = start;
    for &(gap, fwd, left, down) in &s.events {
        ts += gap * 1000 + 137;
        arrival = arrival.max(ts + fwd * 1000);
        let input = if left {
            CabinetInput::LeftFlipper
        } else {
            CabinetInput::RightFlipper
        };
        evs.push((InputEvent::button(ts, input, down), arrival));
    }
    let end_time = arrival + 200_000_000;
    let mut t = start;
    let mut next_ev = 0;
    let mut next_frame = start + s.frame_period_us * 1000;
    let mut executed = Vec::new();
    let mut rng = driver::Schedule::new(s.jitter);
    let mut wake = 0usize;
    let mut frame_angles = Vec::new();
    while t < end_time {
        // Next wake: about one tick later, or a stall.
        let stall = s
            .stalls
            .iter()
            .find(|(i, _)| *i == wake)
            .map(|(_, us)| us * 1000);
        t += stall.unwrap_or(TICK_NS - 200_000 + rng.next_u64() % 400_000);
        wake += 1;
        // Frames presented while the sim slept.
        while next_frame <= t {
            set_now(next_frame);
            let snap = reader.read();
            assert_eq!(snap.tick, l.tick());
            frame_angles.push(bat_angles(snap));
            fw.presented(true, snap);
            next_frame += s.frame_period_us * 1000;
        }
        while next_ev < evs.len() && evs[next_ev].1 <= t {
            tx.push(evs[next_ev].0).unwrap();
            next_ev += 1;
        }
        set_now(t);
        let dropped = clock.dropped_ticks();
        let due = clock.advance(t);
        let newly = clock.dropped_ticks() - dropped;
        if newly > 0 {
            l.skip_ticks(newly);
        }
        for _ in 0..due {
            l.run_ticks(1);
            executed.push(Executed {
                tick: l.tick(),
                end: pin_core::sim::tick_end_ns(l.epoch_ns(), l.tick()),
                drain: t,
                angles: bat_angles(reader.read()),
            });
        }
    }
    let text = c.collect(end_time).to_csv();
    (
        text,
        executed,
        evs.iter().map(|(e, a)| (e.timestamp_ns, *a)).collect(),
        frame_angles,
    )
}

/// The oracle's exclusive upper bound on a press's frame snapshot ticks
/// (`snap_tick < bound`), per row, derived independently from the rows in
/// event order: the next release of the same input bounds the press
/// inclusively, except that when that release is followed, in event order, by
/// a press of the same input in the same tick (same `apply_tick`) the release
/// tick is excluded. `u64::MAX` when there is no release.
fn oracle_frame_bound(rows: &[Row]) -> Vec<u64> {
    (0..rows.len())
        .map(|i| {
            let Some(j) =
                (i + 1..rows.len()).find(|&j| !rows[j].down && rows[j].input == rows[i].input)
            else {
                return u64::MAX;
            };
            let tick = rows[j].apply_tick;
            let repressed = rows[j + 1..]
                .iter()
                .any(|x| x.down && x.input == rows[j].input && x.apply_tick == tick);
            if repressed {
                tick
            } else {
                tick.saturating_add(1)
            }
        })
        .collect()
}

/// REQ-LAT-001, REQ-LAT-004 (round 14, thread 4713880962): a left press at
/// 137 ns, its release at 3_000_274 ns and a re-press at 3_000_411 ns with
/// wakes at 2, 4 and 6 ms: the release and re-press both apply in tick 4 and
/// the first frame (5 ms) draws tick 4. The first press's frame is empty (the
/// release tick is excluded), the re-press's frame is that frame.
#[test]
fn req_lat_004_same_tick_repress_excludes_release_tick_from_oracle() {
    let row = |seq: u64, down: bool, ts: u64, tick: u64, mv: Option<u64>| Row {
        seq,
        input: "left_flipper".into(),
        down,
        sdl_ts_ns: ts,
        apply_tick: tick,
        move_tick: mv,
        ..Row::default()
    };
    let rows = vec![
        row(0, true, 137, 2, Some(2)),
        row(1, false, 3_000_274, 4, None),
        row(2, true, 3_000_411, 4, Some(4)),
    ];
    let frames = [Frame {
        frame: 0,
        snap_tick: 4,
        present_ns: 5_000_000,
    }];
    let bound = oracle_frame_bound(&rows);
    let first = |i: usize| {
        let m = rows[i].move_tick?;
        frames
            .iter()
            .find(|f| f.snap_tick >= m && f.snap_tick < bound[i])
    };
    assert!(first(0).is_none(), "first press's frame must be empty");
    assert_eq!(first(2).map(|f| f.frame), Some(0));
    // Without the re-press the release tick is inclusive.
    let bound = oracle_frame_bound(&rows[..2]);
    assert_eq!(bound[0], 5);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// REQ-LAT-004, REQ-LAT-001: on a controlled clock with stalls and
    /// dropped ticks, every row's applying tick ends at or after its stamp
    /// and is the first executed tick that had received it (drained at or
    /// after its arrival) and whose end is not before its stamp;
    /// `frames_to_photon` equals `frame - F0`, where `frame` is the first
    /// frame with a snapshot tick in `[move_tick, release_tick]` (the release tick
    /// excluded when the same input is pressed again in that tick) whose drawn
    /// bat is raised at least 1 degree from the applying tick's base
    /// (re-derived here from the frames' own snapshots).
    #[test]
    fn req_lat_004_first_executed_tick_and_req_lat_001_frames(s in scenario()) {
        let (text, executed, evs, frame_angles) = run_scenario(&s);
        let t = parse_trace(&text).unwrap();
        prop_assert_eq!(t.rows.len(), evs.len());
        prop_assert_eq!(t.overflows, 0);
        let bounds = oracle_frame_bound(&t.rows);
        for ((r, (ts, arrival)), until) in t.rows.iter().zip(&evs).zip(&bounds) {
            prop_assert_eq!(r.sdl_ts_ns, *ts);
            prop_assert!(r.apply_tick_end_sdl_ns >= r.sdl_ts_ns);
            let first = executed
                .iter()
                .find(|e| e.drain >= *arrival && e.end >= *ts)
                .expect("an executed tick applies it");
            prop_assert_eq!(r.apply_tick, first.tick, "row {:?}", r);
            prop_assert_eq!(r.apply_tick_end_sdl_ns, first.end);
            prop_assert_eq!(r.drain_sdl_ns, first.drain);
            // The join: derived from move_tick, the frame table and the
            // drawn angles first, then compared with the row (both present
            // or both absent). The base is the bat at the applying tick's
            // start: the previous executed tick's end (0 before the first).
            let bat = usize::from(r.input == "right_flipper");
            let base = executed
                .iter()
                .take_while(|e| e.tick < r.apply_tick)
                .last()
                .map_or(0.0, |e| e.angles[bat]);
            let expected = r.move_tick.and_then(|m| {
                t.frames.iter().find(|f| {
                    f.snap_tick >= m
                        && f.snap_tick < *until
                        && frame_angles[f.frame as usize][bat] - base >= RAISE_THRESHOLD_RAD
                })
            });
            match expected {
                Some(fr) => {
                    // `frame - F0`; `frame + 1` without a frame before the
                    // press (frames 0..=frame all count).
                    let n = match verdict::last_frame_at_or_before(&t.frames, r.sdl_ts_ns) {
                        Some(f0) => fr.frame - f0,
                        None => fr.frame + 1,
                    };
                    prop_assert_eq!(r.frame, Some(fr.frame), "row {:?}", r);
                    prop_assert_eq!(r.frames_to_photon, Some(n), "row {:?}", r);
                    prop_assert_eq!(r.frame_snap_tick, Some(fr.snap_tick));
                    prop_assert_eq!(r.present_ns, Some(fr.present_ns));
                }
                None => {
                    prop_assert_eq!(r.frame, None, "row {:?}", r);
                    prop_assert_eq!(r.frames_to_photon, None, "row {:?}", r);
                }
            }
            if r.down && r.move_tick.is_some() {
                prop_assert!(r.move_tick.unwrap() >= r.apply_tick);
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Allocation guard and "tracing does not change the sim".

/// REQ-LAT-003 (D5): with the trace installed, the sim loop's ticks (input
/// rows, bat-move tracking, dropped ticks) and the frame writer do not
/// allocate.
#[test]
fn req_lat_003_trace_record_paths_do_not_allocate() {
    let (_, n) = guarded(|| std::hint::black_box(Box::new(1u8)));
    assert!(n >= 1, "allocator guard not armed");
    let (mut tx, rx) = channel::<InputEvent>(4096);
    let (w, mut reader) = TripleBuffer::new(FrameStateSnapshot::default());
    let mut l = SimLoop::new(Bats::new(1), rx, w);
    let (c, sim, mut fw) = latency_trace(bindings(), now);
    l.set_latency_trace(sim);
    l.set_epoch_ns(T0);
    for i in 0..3000u64 {
        let input = if i % 3 == 0 {
            CabinetInput::RightFlipper
        } else {
            CabinetInput::LeftFlipper
        };
        tx.push(InputEvent::button(
            T0 + i * 5 * TICK_NS / 2,
            input,
            i % 2 == 0,
        ))
        .unwrap();
    }
    let ((), allocs) = guarded(|| {
        for k in 0..1500u64 {
            set_now(T0 + k * TICK_NS);
            l.run_ticks(1);
            if k % 7 == 0 {
                fw.presented(true, reader.read());
            }
            if k % 300 == 0 {
                l.skip_ticks(5);
            }
        }
        l.run_ticks(8);
        fw.presented(false, reader.read());
        fw.presented(true, reader.read());
    });
    assert_eq!(allocs, 0, "the traced tick and frame paths allocated");
    let t = c.collect(0);
    assert!(t.rows.len() > 500, "{}", t.rows.len());
    assert!(t.rows.iter().any(|r| r.move_tick.is_some()));
}

/// REQ-LAT-003: tracing does not change what the simulation computes: the
/// same inputs give the same snapshot every tick with and without the
/// trace installed.
#[test]
fn req_lat_003_tracing_leaves_the_simulation_unchanged() {
    let run = |traced: bool| {
        let (mut tx, rx) = channel::<InputEvent>(1024);
        let (w, mut r) = TripleBuffer::new(FrameStateSnapshot::default());
        let mut l = SimLoop::new(Bats::new(0), rx, w);
        let mut collector = None;
        if traced {
            let (c, sim, _fw) = latency_trace(bindings(), now);
            collector = Some(c);
            l.set_latency_trace(sim);
        }
        l.set_epoch_ns(T0);
        for i in 0..200u64 {
            tx.push(InputEvent::button(
                T0 + i * 7 * TICK_NS + 333,
                CabinetInput::LeftFlipper,
                i % 2 == 0,
            ))
            .unwrap();
        }
        let mut out = Vec::new();
        for k in 0..1600u64 {
            set_now(T0 + k * TICK_NS);
            if k == 700 {
                l.skip_ticks(30);
            }
            l.run_ticks(1);
            out.push(*r.read());
        }
        let rows = collector.map_or(0, |c| c.collect(0).rows.len());
        (out, l.inputs_delivered(), rows)
    };
    let (plain, n_plain, _) = run(false);
    let (traced, n_traced, rows) = run(true);
    assert_eq!(n_plain, n_traced);
    assert_eq!(rows as u64, n_traced, "every delivered button event traced");
    assert!(plain == traced, "tracing changed a snapshot");
}

// ---------------------------------------------------------------------------
// CLI (no display needed: these never open a window).

/// REQ-LAT-003: an unwritable `--latency-trace` path fails before the app
/// starts, with `pin_app: --latency-trace: <error>` and a non-zero exit.
#[test]
fn req_lat_003_unwritable_trace_path_fails_before_start() {
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_pin_app"))
        .args([
            "--latency-trace",
            "/nonexistent-kp002-dir/sub/trace.csv",
            "--run-ms",
            "1",
        ])
        .env("SDL_VIDEODRIVER", "dummy")
        .output()
        .unwrap();
    assert!(!out.status.success());
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.lines()
            .any(|l| l
                .starts_with("pin_app: --latency-trace: /nonexistent-kp002-dir/sub/trace.csv: ")),
        "{err}"
    );
    assert!(
        !err.contains("sim ticks"),
        "the app must not have started: {err}"
    );
}

/// REQ-LAT-003: a headless run is unaffected by `--latency-trace` (same
/// output, no trace written).
#[test]
fn req_lat_003_headless_run_is_unaffected() {
    let dir = std::path::Path::new(env!("CARGO_TARGET_TMPDIR")).join("kp002_headless");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("t.csv");
    let _ = std::fs::remove_file(&path);
    let run = |extra: &[&str]| {
        let out = std::process::Command::new(env!("CARGO_BIN_EXE_pin_app"))
            .args(["--headless-ticks", "250"])
            .args(extra)
            .output()
            .unwrap();
        assert!(out.status.success(), "{out:?}");
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    let plain = run(&[]);
    let traced = run(&["--latency-trace", path.to_str().unwrap()]);
    assert_eq!(plain, "headless: 250 ticks, last tick 250\n");
    assert_eq!(plain, traced);
    assert!(!path.exists(), "headless runs write no trace");
}

// ---------------------------------------------------------------------------
// The driver's pure parts (no display needed).

/// REQ-LAT-001 (F8g): the press plan holds every press at least three
/// frame intervals (capped at 1 s) plus jitter, gaps 100 to 250 ms per key,
/// never a whole number of ticks, keys alternating and overlapping, the
/// two solo screenshot holds and the two rest shots, every down released,
/// and at least 100 presses in a 55 s span; same seed, same plan.
#[test]
fn req_lat_001_press_plan_bounds() {
    use driver::{Action, Key, Step};
    let span = 55_000_000u64;
    let plan = driver::press_plan(driver::DEFAULT_SEED, span, 125);
    assert_eq!(plan, driver::press_plan(driver::DEFAULT_SEED, span, 125));
    assert!(
        driver::planned_presses(&plan) >= 100,
        "{}",
        driver::planned_presses(&plan)
    );
    assert_eq!(driver::base_hold_us(125), 375_000);
    assert_eq!(driver::base_hold_us(400), 1_000_000, "capped at 1 s");
    let mut down_at: [Option<u64>; 2] = [None, None];
    let mut last_up: [Option<u64>; 2] = [None, None];
    let mut overlaps = 0;
    let mut gaps = Vec::new();
    let mut shots = Vec::new();
    let solo_us = driver::SHOT_HOLD.as_micros() as u64;
    for &Action { at_us, step } in &plan {
        assert!(at_us < span);
        match step {
            Step::Down(k) => {
                let i = usize::from(k == Key::Right);
                assert!(down_at[i].is_none(), "{k:?} pressed twice");
                if down_at[1 - i].is_some() {
                    overlaps += 1;
                }
                if let Some(up) = last_up[i] {
                    let gap = at_us - up;
                    assert!(gap >= 100_000, "gap {gap}");
                    gaps.push(gap);
                }
                assert_ne!(at_us % 1000, 0, "a down at a whole ms: {at_us}");
                down_at[i] = Some(at_us);
            }
            Step::Up(k) => {
                let i = usize::from(k == Key::Right);
                let hold = at_us - down_at[i].take().expect("up without down");
                assert!(
                    (375_000..545_000).contains(&hold) || hold == solo_us + 1,
                    "hold {hold}"
                );
                last_up[i] = Some(at_us);
            }
            Step::Shot(n) => shots.push((n, down_at)),
        }
    }
    assert_eq!(down_at, [None, None], "every press released");
    // Gaps are 100 to 250 ms, except the waits around the solo holds.
    let long = gaps.iter().filter(|g| **g >= 250_000).count();
    assert!(long <= 9, "{long} long gaps of {}", gaps.len());
    assert!(overlaps > 20, "keys overlap: {overlaps}");
    let names: Vec<&str> = shots.iter().map(|(n, _)| *n).collect();
    assert_eq!(
        names,
        [
            "frame_0_rest.png",
            "frame_1_left_held.png",
            "frame_2_right_held.png",
            "frame_3_rest.png"
        ]
    );
    // Held shots: only their key is down; rest shots: none.
    assert_eq!(
        shots[1].1[0].is_some() as u8 + shots[1].1[1].is_some() as u8,
        1
    );
    assert!(shots[1].1[0].is_some());
    assert!(shots[2].1[1].is_some() && shots[2].1[0].is_none());
    assert_eq!(shots[3].1, [None, None]);
}

use driver::{Waitable, wait_with};
#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;
#[cfg(windows)]
use std::os::windows::process::ExitStatusExt;
use std::time::Duration;

/// A successful exit status on any platform (`from_raw` takes an `i32` on
/// Unix, a `u32` on Windows).
fn success_status() -> std::process::ExitStatus {
    std::process::ExitStatus::from_raw(0)
}

/// A stub child: exits at its `exit_at`-th poll (never when `None`), or its
/// poll fails when `fail` is set; counts kills and reaps.
struct StubChild {
    polls: u32,
    exit_at: Option<u32>,
    fail: bool,
    kills: u32,
    reaps: u32,
}

impl StubChild {
    fn new(exit_at: Option<u32>, fail: bool) -> Self {
        Self {
            polls: 0,
            exit_at,
            fail,
            kills: 0,
            reaps: 0,
        }
    }
}

impl Waitable for StubChild {
    fn try_wait(&mut self) -> std::io::Result<Option<std::process::ExitStatus>> {
        self.polls += 1;
        if self.fail {
            return Err(std::io::Error::other("poll broke"));
        }
        Ok((Some(self.polls) == self.exit_at).then(success_status))
    }
    fn kill(&mut self) {
        self.kills += 1;
    }
    fn reap(&mut self) {
        self.reaps += 1;
    }
}

/// REQ-LAT-001 harness (R11 / F8a, R6): a helper that does not exit is
/// killed and reaped once when the fake clock reaches the deadline. No real
/// process, sleep or wall clock.
#[test]
fn req_lat_001_harness_bounded_wait_kills_at_the_deadline() {
    let clock = std::cell::Cell::new(0u64);
    let mut child = StubChild::new(None, false);
    let e = wait_with(
        &mut child,
        Duration::from_millis(300),
        &mut || Duration::from_millis(clock.get()),
        &mut || clock.set(clock.get() + 5),
        "helper",
    )
    .unwrap_err();
    assert!(e.contains("helper") && e.contains("killed"), "{e}");
    assert_eq!((child.kills, child.reaps), (1, 1));
    assert_eq!(
        clock.get(),
        300,
        "killed when the clock reached the deadline"
    );
}

/// REQ-LAT-001 harness: a child that exits at the third poll returns its
/// status and is never killed.
#[test]
fn req_lat_001_harness_bounded_wait_returns_an_exit() {
    let clock = std::cell::Cell::new(0u64);
    let mut child = StubChild::new(Some(3), false);
    let st = wait_with(
        &mut child,
        Duration::from_millis(300),
        &mut || Duration::from_millis(clock.get()),
        &mut || clock.set(clock.get() + 5),
        "helper",
    )
    .unwrap();
    assert!(st.success());
    assert_eq!((child.polls, child.kills, child.reaps), (3, 0, 0));
}

/// REQ-LAT-001 harness: a failing poll kills, reaps and returns the error.
#[test]
fn req_lat_001_harness_bounded_wait_poll_error_kills() {
    let mut child = StubChild::new(None, true);
    let e = wait_with(
        &mut child,
        Duration::from_millis(300),
        &mut || Duration::ZERO,
        &mut || {},
        "helper",
    )
    .unwrap_err();
    assert!(e.contains("helper") && e.contains("poll broke"), "{e}");
    assert_eq!((child.kills, child.reaps), (1, 1));
}

/// REQ-LAT-001 harness (R2 finding): a failing key-down for the second key
/// still releases the first and the second, and the original error is the
/// one returned.
#[test]
fn req_lat_001_harness_failed_keydown_releases_every_key() {
    use driver::{Key, finish_injection, key_down, release_held};
    let mut held = [false; 2];
    key_down(&mut held, Key::Left, |_| Ok(())).unwrap();
    let injection = key_down(&mut held, Key::Right, |_| Err("x server gone".into()));
    assert_eq!(held, [true, true], "marked before the send");
    let mut ups = Vec::new();
    let failed = release_held(
        &mut held,
        |k, _| {
            ups.push(k);
            Ok(())
        },
        Duration::from_millis(400),
        &mut || Duration::ZERO,
        &mut || panic!("no retry needed"),
    );
    assert!(failed.is_empty());
    assert_eq!(ups, [Key::Left, Key::Right]);
    assert_eq!(held, [false, false]);
    let e = finish_injection(injection, &failed).unwrap_err();
    assert_eq!(e, "x server gone");
}

/// REQ-LAT-001 harness: a failing key-up for the first key does not stop
/// the second's key-up; a key whose release keeps failing stays marked
/// held (C7: never silently cleared) and is returned; the original
/// injection error is kept over the release failure, and a release failure
/// alone is reported.
#[test]
fn req_lat_001_harness_failed_keyup_does_not_stop_the_rest() {
    use driver::{Key, finish_injection, release_held};
    let mut held = [true, true];
    let mut ups = Vec::new();
    // A budget already spent: one round, no retry.
    let failed = release_held(
        &mut held,
        |k, _| {
            ups.push(k);
            if k == Key::Left {
                Err("no reply".to_string())
            } else {
                Ok(())
            }
        },
        Duration::from_millis(400),
        &mut || Duration::from_millis(400),
        &mut || panic!("budget spent: no retry"),
    );
    assert_eq!(ups, [Key::Left, Key::Right]);
    assert_eq!(failed, [(Key::Left, "no reply".to_string())]);
    assert_eq!(held, [true, false], "the failed key stays held");
    assert_eq!(
        finish_injection(Err("original".into()), &failed).unwrap_err(),
        "original"
    );
    let e = finish_injection(Ok(()), &failed).unwrap_err();
    assert!(e.starts_with("key release: "), "{e}");
    assert!(finish_injection(Ok(()), &[]).is_ok());
}

/// REQ-LAT-001 harness (C7): a key whose first release fails and second
/// succeeds ends released (held cleared), retried after one injected
/// pause; the other key is released in the first round and not sent again.
#[test]
fn req_lat_001_harness_failed_release_is_retried() {
    use driver::{Key, release_held};
    let clock = std::cell::Cell::new(0u64);
    let mut held = [true, true];
    let mut ups = Vec::new();
    let mut left_tries = 0;
    let failed = release_held(
        &mut held,
        |k, _| {
            ups.push(k);
            if k == Key::Left {
                left_tries += 1;
                if left_tries == 1 {
                    return Err("busy".to_string());
                }
            }
            Ok(())
        },
        Duration::from_millis(400),
        &mut || Duration::from_millis(clock.get()),
        &mut || clock.set(clock.get() + 50),
    );
    assert!(failed.is_empty(), "{failed:?}");
    assert_eq!(held, [false, false]);
    assert_eq!(ups, [Key::Left, Key::Right, Key::Left]);
    assert_eq!(clock.get(), 50, "one injected pause");
}

/// REQ-LAT-001 harness (C7, gate 30 M7): a key whose release always fails
/// is retried while the injected clock is below the budget (never by
/// waiting: the pause only advances the fake clock; no retry once the
/// budget is reached), then returned with its flag still set; the other key
/// is released; the original injection error is kept.
#[test]
fn req_lat_001_harness_release_retry_budget_is_bounded() {
    use driver::{Key, finish_injection, release_held};
    let clock = std::cell::Cell::new(0u64);
    let mut held = [true, true];
    let (mut lefts, mut rights) = (0u32, 0u32);
    let failed = release_held(
        &mut held,
        |k, _| {
            if k == Key::Left {
                lefts += 1;
                Err("stuck".to_string())
            } else {
                rights += 1;
                Ok(())
            }
        },
        Duration::from_millis(200),
        &mut || Duration::from_millis(clock.get()),
        &mut || clock.set(clock.get() + 50),
    );
    assert_eq!(failed, [(Key::Left, "stuck".to_string())]);
    assert_eq!(held, [true, false], "still pending");
    assert_eq!(rights, 1);
    // Rounds at 0, 50, 100 and 150 ms; none at 200 ms (the budget).
    assert_eq!(lefts, 4);
    assert_eq!(clock.get(), 200);
    assert_eq!(
        finish_injection(Err("original".into()), &failed).unwrap_err(),
        "original"
    );
    let e = finish_injection(Ok(()), &failed).unwrap_err();
    assert_eq!(e, "key release: Left: stuck");
    // A clock that never advances: the round cap still ends it.
    let mut held = [true, false];
    let mut tries = 0;
    let failed = release_held(
        &mut held,
        |_, _| {
            tries += 1;
            Err("stuck".to_string())
        },
        Duration::from_millis(200),
        &mut || Duration::ZERO,
        &mut || {},
    );
    assert_eq!(failed.len(), 1);
    assert_eq!(tries, driver::KEY_RELEASE_MAX_ROUNDS);
    assert_eq!(held, [true, false]);
}

/// REQ-LAT-001 harness (gate 30, M7): the cleanup key-ups share one
/// deadline. Each attempt is bounded by the budget left (capped at the
/// helper timeout, floored at `KEY_RELEASE_MIN_TIMEOUT`): a first key-up
/// that hangs for its whole bound spends the budget, the second key still
/// gets its first attempt with the floor, and nothing is retried.
#[test]
fn req_lat_001_harness_release_shares_one_deadline() {
    use driver::{HELPER_TIMEOUT, KEY_RELEASE_BUDGET, KEY_RELEASE_MIN_TIMEOUT, Key, release_held};
    let clock = std::cell::Cell::new(Duration::ZERO);
    let mut held = [true, true];
    let mut sent = Vec::new();
    let failed = release_held(
        &mut held,
        |k, t| {
            sent.push((k, t));
            // A key-up that hangs until its bound kills it.
            clock.set(clock.get() + t);
            Err("timed out".to_string())
        },
        KEY_RELEASE_BUDGET,
        &mut || clock.get(),
        &mut || panic!("budget spent: no retry"),
    );
    assert_eq!(
        sent,
        [
            (Key::Left, KEY_RELEASE_BUDGET),
            (Key::Right, KEY_RELEASE_MIN_TIMEOUT)
        ]
    );
    assert_eq!(failed.len(), 2);
    assert_eq!(held, [true, true]);
    assert_eq!(clock.get(), KEY_RELEASE_BUDGET + KEY_RELEASE_MIN_TIMEOUT);
    // A partly spent budget bounds the next attempt by what is left; a
    // budget above the helper timeout is capped by it.
    assert_eq!(
        driver::key_up_timeout(KEY_RELEASE_BUDGET, Duration::from_millis(150)),
        Duration::from_millis(250)
    );
    assert_eq!(
        driver::key_up_timeout(KEY_RELEASE_BUDGET, Duration::from_millis(390)),
        KEY_RELEASE_MIN_TIMEOUT
    );
    assert_eq!(
        driver::key_up_timeout(Duration::from_secs(60), Duration::ZERO),
        HELPER_TIMEOUT
    );
    // A retry round: one 120 ms attempt each, then one pause of 50 ms; the
    // second round's bounds are what is left of the shared budget, and a
    // retry that would start after the budget is skipped.
    let clock = std::cell::Cell::new(Duration::ZERO);
    let mut held = [true, true];
    let mut sent = Vec::new();
    release_held(
        &mut held,
        |_, t| {
            sent.push(t);
            clock.set(clock.get() + Duration::from_millis(120));
            if sent.len() <= 2 {
                Err("busy".to_string())
            } else {
                Ok(())
            }
        },
        KEY_RELEASE_BUDGET,
        &mut || clock.get(),
        &mut || clock.set(clock.get() + Duration::from_millis(50)),
    );
    assert_eq!(
        sent,
        [400, 280, 110].map(Duration::from_millis),
        "bounds from one shared deadline; Right's retry starts after the budget (410 ms of \
         400 ms) and is skipped (gate 30 round 2, A2)"
    );
    assert_eq!(held, [false, true]);
}

/// REQ-LAT-004 harness (gate 30 round 2): timing fields above 2^60 ns are
/// rejected by the parser (unreadable output), exactly 2^60 parses, and the
/// check-a bound cannot overflow, so a stalled row with a huge drain gap
/// never panics or wraps.
#[test]
fn req_lat_004_harness_out_of_range_timing_is_unreadable() {
    use verdict::{MAX_TIMING_NS, coil_bound_ns, evaluate_text, parse_trace};
    let csv = |ts: u64, drain: u64, prev: u64, stall: u8, present: u64, run_end: u64| {
        format!(
            "{}\n0,left_flipper,1,{ts},1,{ts},0,{drain},{prev},0,{stall},,,,,\n\n{}\n0,1,{present}\n\
             # overflows: input=0 move=0 frame=0 pending=0\n# run_end_ns={run_end}\n",
            verdict::EVENT_HEADER,
            verdict::FRAME_HEADER
        )
    };
    let stderr =
        "pin_app: 1000 sim ticks (0 dropped), 1 frames, input dropped 0, 0 haptic events\n";
    let huge = csv(1000, u64::MAX, 0, 1, 5, 9);
    let e = evaluate_text(&huge, stderr, &Params::default()).unwrap_err();
    assert!(
        e.contains("drain_sdl_ns") && e.contains("out of range (limit 2^60 ns)"),
        "{e}"
    );
    assert!(parse_trace(&csv(1000, MAX_TIMING_NS, 0, 1, 5, 9)).is_ok());
    let e = parse_trace(&csv(1000, MAX_TIMING_NS + 1, 0, 1, 5, 9)).unwrap_err();
    assert!(e.starts_with("trace line 2: drain_sdl_ns "), "{e}");
    assert!(parse_trace(&csv(MAX_TIMING_NS + 1, 0, 0, 0, 5, 9)).is_err());
    assert!(parse_trace(&csv(1000, 0, 0, 0, MAX_TIMING_NS + 1, 9)).is_err());
    assert!(parse_trace(&csv(1000, 0, 0, 0, 5, MAX_TIMING_NS + 1)).is_err());
    // The largest in-range stall, with a large tolerance: no overflow.
    let t = parse_trace(&csv(1000, MAX_TIMING_NS, 0, 1, 5, 9)).unwrap();
    let p = Params {
        tolerance_ns: u64::MAX,
        ..Params::default()
    };
    assert_eq!(coil_bound_ns(&t.rows[0], &p), u64::MAX);
    assert_eq!(
        coil_bound_ns(&t.rows[0], &Params::default()),
        verdict::TICK_NS + Params::default().tolerance_ns + MAX_TIMING_NS
    );
}

/// REQ-LAT-001 harness (gate 30 round 2): a retry after the budget is spent
/// is skipped. Left's first attempt and retry spend the budget; Right had
/// its first attempt in round 1 and gets no second one. A spent budget at
/// the start still gives both keys a first attempt with the floor.
#[test]
fn req_lat_001_harness_release_retry_respects_the_deadline() {
    use driver::{KEY_RELEASE_BUDGET, KEY_RELEASE_MIN_TIMEOUT, Key, release_held};
    let clock = std::cell::Cell::new(Duration::ZERO);
    let mut held = [true, true];
    let mut sent = Vec::new();
    let failed = release_held(
        &mut held,
        |k, t| {
            sent.push((k, t));
            // Round 1: Left takes 100 ms, Right 10 ms. Left's retry spends
            // the rest of the budget.
            let spend = match (k, sent.iter().filter(|(x, _)| *x == k).count()) {
                (Key::Left, 1) => 100,
                (Key::Right, 1) => 10,
                _ => 1000,
            };
            clock.set(clock.get() + Duration::from_millis(spend));
            Err("busy".to_string())
        },
        KEY_RELEASE_BUDGET,
        &mut || clock.get(),
        &mut || clock.set(clock.get() + Duration::from_millis(50)),
    );
    let right_sends = sent.iter().filter(|(k, _)| *k == Key::Right).count();
    assert_eq!(right_sends, 1, "no second Right attempt: {sent:?}");
    assert_eq!(sent.len(), 3, "{sent:?}");
    assert_eq!(sent[2].0, Key::Left);
    assert_eq!(failed.len(), 2, "both keys stay failed: {failed:?}");
    assert_eq!(held, [true, true]);
    // The budget already spent: first attempts for both keys, floor bound.
    let mut held = [true, true];
    let mut sent = Vec::new();
    let failed = release_held(
        &mut held,
        |k, t| {
            sent.push((k, t));
            Err("e".to_string())
        },
        KEY_RELEASE_BUDGET,
        &mut || KEY_RELEASE_BUDGET * 2,
        &mut || panic!("budget spent: no retry"),
    );
    assert_eq!(
        sent,
        [
            (Key::Left, KEY_RELEASE_MIN_TIMEOUT),
            (Key::Right, KEY_RELEASE_MIN_TIMEOUT)
        ]
    );
    assert_eq!(failed.len(), 2);
}

/// REQ-LAT-001 harness (gate 30 round 3): a retry is bounded by the budget
/// left with no 50 ms floor, so a retry that starts 1 ms before the shared
/// deadline is sent with exactly 1 ms; a retry at the start is capped by
/// the helper timeout and the budget; first attempts keep the floor even
/// when the budget is spent.
#[test]
fn req_lat_001_harness_release_retry_has_no_floor() {
    use driver::{
        HELPER_TIMEOUT, KEY_RELEASE_BUDGET, KEY_RELEASE_MIN_TIMEOUT, Key, key_up_retry_timeout,
        key_up_timeout, release_held,
    };
    let ms = Duration::from_millis;
    // The bound itself.
    assert_eq!(
        key_up_retry_timeout(KEY_RELEASE_BUDGET, KEY_RELEASE_BUDGET - ms(1)),
        ms(1)
    );
    assert_eq!(key_up_retry_timeout(KEY_RELEASE_BUDGET, ms(390)), ms(10));
    assert_eq!(
        key_up_retry_timeout(KEY_RELEASE_BUDGET, Duration::ZERO),
        KEY_RELEASE_BUDGET
    );
    assert_eq!(
        key_up_retry_timeout(Duration::from_secs(60), Duration::ZERO),
        HELPER_TIMEOUT
    );
    assert_eq!(
        key_up_retry_timeout(KEY_RELEASE_BUDGET, KEY_RELEASE_BUDGET * 2),
        Duration::ZERO
    );
    assert_eq!(
        key_up_timeout(KEY_RELEASE_BUDGET, KEY_RELEASE_BUDGET - ms(1)),
        KEY_RELEASE_MIN_TIMEOUT
    );
    // A retry that starts with 1 ms left: sent with exactly 1 ms.
    let clock = std::cell::Cell::new(Duration::ZERO);
    let mut held = [true, false];
    let mut sent = Vec::new();
    let failed = release_held(
        &mut held,
        |k, t| {
            sent.push((k, t));
            clock.set(clock.get() + ms(10));
            if sent.len() == 1 {
                Err("busy".to_string())
            } else {
                Ok(())
            }
        },
        KEY_RELEASE_BUDGET,
        &mut || clock.get(),
        &mut || clock.set(KEY_RELEASE_BUDGET - ms(1)),
    );
    assert_eq!(sent, [(Key::Left, KEY_RELEASE_BUDGET), (Key::Left, ms(1))]);
    assert!(failed.is_empty(), "{failed:?}");
    assert_eq!(held, [false, false]);
    // Retries at time 0: capped by the budget, and by the helper timeout
    // when the budget is larger.
    for (budget, want) in [
        (KEY_RELEASE_BUDGET, KEY_RELEASE_BUDGET),
        (Duration::from_secs(60), HELPER_TIMEOUT),
    ] {
        let mut held = [false, true];
        let mut sent = Vec::new();
        release_held(
            &mut held,
            |k, t| {
                sent.push((k, t));
                if sent.len() < 3 {
                    Err("busy".to_string())
                } else {
                    Ok(())
                }
            },
            budget,
            &mut || Duration::ZERO,
            &mut || {},
        );
        assert_eq!(sent, [(Key::Right, want); 3], "budget {budget:?}");
        assert_eq!(held, [false, false]);
    }
    // First attempts at a spent budget still get the floor; no retry.
    let mut held = [true, true];
    let mut sent = Vec::new();
    let failed = release_held(
        &mut held,
        |k, t| {
            sent.push((k, t));
            Err("e".to_string())
        },
        KEY_RELEASE_BUDGET,
        &mut || KEY_RELEASE_BUDGET + ms(1),
        &mut || panic!("budget spent: no retry"),
    );
    assert_eq!(
        sent,
        [
            (Key::Left, KEY_RELEASE_MIN_TIMEOUT),
            (Key::Right, KEY_RELEASE_MIN_TIMEOUT)
        ]
    );
    assert_eq!(failed.len(), 2);
}

/// REQ-LAT-003 harness (gate 30 round 2): the stderr capture decodes
/// lossily and keeps going after a non-UTF-8 line; a read error is logged
/// as a failure line, EOF is silent.
#[test]
fn req_lat_003_harness_capture_survives_non_utf8_and_reports_read_errors() {
    use driver::capture_lines;
    use std::sync::{Arc, Mutex};
    let log = Arc::new(Mutex::new(String::new()));
    capture_lines(
        std::io::Cursor::new(b"ok line\n\xff\xfe bad\r\npin_app: render on x\nlast\n".to_vec()),
        Arc::clone(&log),
    )
    .join()
    .unwrap();
    assert_eq!(
        *log.lock().unwrap(),
        "ok line\n\u{fffd}\u{fffd} bad\npin_app: render on x\nlast\n"
    );
    struct FailAfter(Option<&'static [u8]>);
    impl std::io::Read for FailAfter {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            match self.0.take() {
                Some(b) => {
                    buf[..b.len()].copy_from_slice(b);
                    Ok(b.len())
                }
                None => Err(std::io::Error::other("boom")),
            }
        }
    }
    let log = Arc::new(Mutex::new(String::new()));
    capture_lines(FailAfter(Some(b"one\n")), Arc::clone(&log))
        .join()
        .unwrap();
    assert_eq!(
        *log.lock().unwrap(),
        "one\nwindowed: stderr read failed: boom\n"
    );
}

/// REQ-LAT-001 harness: a key-up that fails keeps the key marked held (so
/// the final release retries it); one that succeeds clears it.
#[test]
fn req_lat_001_harness_keyup_tracks_held() {
    use driver::{Key, key_up};
    let mut held = [true, true];
    assert!(key_up(&mut held, Key::Left, |_| Err("e".into())).is_err());
    assert_eq!(held, [true, true]);
    key_up(&mut held, Key::Left, |_| Ok(())).unwrap();
    assert_eq!(held, [false, true]);
}

/// F8b: the screenshot is two argument vectors, never shell source: a
/// path with a single quote and a display string stay single arguments.
#[test]
fn screenshot_argv_is_shell_free() {
    let p = std::path::Path::new("/tmp/it's here/frame_1.png");
    let [xwd, convert] = driver::screenshot_argv(":93", p);
    assert_eq!(
        xwd,
        ["-n", "10", "xwd", "-root", "-silent", "-display", ":93"].map(std::ffi::OsString::from)
    );
    assert_eq!(
        convert,
        [
            "-n",
            "10",
            "convert",
            "xwd:-",
            "png:/tmp/it's here/frame_1.png"
        ]
        .map(std::ffi::OsString::from)
    );
    assert!(!xwd.iter().chain(&convert).any(|a| a == "sh" || a == "-c"));
    let xvfb = driver::xvfb_args();
    assert!(
        xvfb.windows(2).any(|w| w == ["-displayfd", "1"]),
        "{xvfb:?}"
    );
    assert!(xvfb.windows(2).any(|w| w == ["-nolisten", "tcp"]));
}

/// F8e, F8f: a failure summary names what failed and carries the stderr
/// tail; too few screenshots, or none with a flipper held, are named.
#[test]
fn failure_summary_and_missing_captures() {
    let stderr: String = (0..60).map(|i| format!("pin_app: line {i}\n")).collect();
    let s = driver::failure_summary("key injection: xdotool failed", &stderr);
    assert!(s.starts_with("verdict: FAIL (windowed driver: key injection: xdotool failed)"));
    assert!(s.contains("pin_app: line 59") && s.contains("pin_app: line 20"));
    assert!(!s.contains("pin_app: line 19\n"), "tail of 40 lines");
    let p = |n: &str| std::path::PathBuf::from("/out").join(n);
    let all = [
        p("frame_0_rest.png"),
        p("frame_1_left_held.png"),
        p("frame_2_right_held.png"),
        p("frame_3_rest.png"),
    ];
    assert_eq!(driver::missing_captures(&all), None);
    assert_eq!(driver::missing_captures(&all[..3]), None);
    let m = driver::missing_captures(&all[..2]).unwrap();
    assert!(
        m.contains("2 screenshots") && m.contains("frame_2_right_held.png"),
        "{m}"
    );
    let m = driver::missing_captures(&[all[0].clone(), all[3].clone(), p("frame_9.png")]).unwrap();
    assert!(
        m.contains("none") && m.contains("frame_1_left_held.png"),
        "{m}"
    );
    assert_eq!(driver::FORCE_FALLBACK_ENV, "PIN_FORCE_FALLBACK_ADAPTER");
}

/// A reader that yields `first`, then reports that it blocked and blocks
/// until `hold` is dropped (a child that keeps the app's stderr open).
struct StuckReader {
    first: Option<&'static [u8]>,
    reached: std::sync::mpsc::Sender<()>,
    hold: std::sync::mpsc::Receiver<()>,
}

impl std::io::Read for StuckReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(b) = self.first.take() {
            buf[..b.len()].copy_from_slice(b);
            return Ok(b.len());
        }
        let _ = self.reached.send(());
        let _ = self.hold.recv();
        Ok(0)
    }
}

/// REQ-LAT-003 harness (gate 30, I3): the stderr drain after the app was
/// reaped is bounded. A pipe that never reaches its end (a process the app
/// started holds it) returns at the bound on the injected clock with every
/// line captured so far and a timeout note; a pipe that ends returns the
/// complete text without a note. No sleep and no wall-clock assertion
/// (R6): the clock is a fake and only the pause advances it.
#[test]
fn req_lat_003_harness_stderr_drain_is_bounded() {
    use driver::{STDERR_DRAIN_TIMEOUT, capture_lines, drain_timeout_note, finish_capture};
    use std::sync::{Arc, Mutex, mpsc};
    let (reached_tx, reached_rx) = mpsc::channel();
    let (hold_tx, hold_rx) = mpsc::channel::<()>();
    let log = Arc::new(Mutex::new(String::new()));
    let reader = capture_lines(
        StuckReader {
            first: Some(b"line 1\nline 2\n"),
            reached: reached_tx,
            hold: hold_rx,
        },
        Arc::clone(&log),
    );
    // Both lines are in the log once the reader blocks; blocks until it
    // does (a completion signal, no clock: R6; a hang is the runner's).
    reached_rx
        .recv()
        .expect("the reader reached its blocking read");
    let clock = Cell::new(Duration::ZERO);
    let (done, text) = finish_capture(
        reader,
        &log,
        STDERR_DRAIN_TIMEOUT,
        &mut || clock.get(),
        &mut || clock.set(clock.get() + Duration::from_millis(5)),
    );
    assert!(!done, "a pipe held open is not waited for");
    assert_eq!(clock.get(), STDERR_DRAIN_TIMEOUT);
    // The note is on the returned snapshot, not on the live buffer.
    assert_eq!(*log.lock().unwrap(), "line 1\nline 2\n");
    assert_eq!(
        text,
        format!(
            "line 1\nline 2\n{}",
            drain_timeout_note(STDERR_DRAIN_TIMEOUT)
        )
    );
    assert!(text.contains("stderr drain timed out after 5 s"), "{text}");
    // Let the detached reader end.
    drop(hold_tx);

    // A pipe that ends: the complete log, joined, no note. The clock never
    // advances, so only the reader's end returns.
    let log = Arc::new(Mutex::new(String::new()));
    let reader = capture_lines(
        std::io::Cursor::new(b"a\npin_app: render on x\nlast".to_vec()),
        Arc::clone(&log),
    );
    let (done, text) = finish_capture(
        reader,
        &log,
        STDERR_DRAIN_TIMEOUT,
        &mut || Duration::ZERO,
        &mut std::thread::yield_now,
    );
    assert!(done);
    assert_eq!(text, "a\npin_app: render on x\nlast\n");
    assert_eq!(*log.lock().unwrap(), text);
}

/// A reader that yields `first`, blocks until `hold` is dropped, then
/// yields `late` and ends (a child that writes after the drain gave up).
/// `appended` is signalled by the first read after `late` was returned:
/// `capture_lines` processes a complete line between two reads, so by then
/// `late` is in the live log (a completion signal, not a clock).
struct LateReader {
    first: Option<&'static [u8]>,
    late: Option<&'static [u8]>,
    late_returned: bool,
    reached: std::sync::mpsc::Sender<()>,
    hold: std::sync::mpsc::Receiver<()>,
    appended: std::sync::mpsc::Sender<()>,
}

impl std::io::Read for LateReader {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if let Some(b) = self.first.take() {
            buf[..b.len()].copy_from_slice(b);
            return Ok(b.len());
        }
        if self.late.is_some() {
            let _ = self.reached.send(());
            let _ = self.hold.recv();
        }
        match self.late.take() {
            Some(b) => {
                buf[..b.len()].copy_from_slice(b);
                self.late_returned = true;
                Ok(b.len())
            }
            None => {
                if std::mem::take(&mut self.late_returned) {
                    let _ = self.appended.send(());
                }
                Ok(0)
            }
        }
    }
}

/// REQ-LAT-003 harness (gate 30 round 11): after a drain timeout the saved
/// snapshot is a copy taken under the lock with the note appended to the
/// copy; a line the detached reader appends afterwards reaches the live
/// buffer only, so the saved file ends with the note.
#[test]
fn req_lat_003_harness_timeout_snapshot_excludes_late_reader_lines() {
    use driver::{STDERR_DRAIN_TIMEOUT, capture_lines, drain_timeout_note, finish_capture};
    use std::sync::{Arc, Mutex, mpsc};
    let (reached_tx, reached_rx) = mpsc::channel();
    let (hold_tx, hold_rx) = mpsc::channel::<()>();
    let (appended_tx, appended_rx) = mpsc::channel();
    let log = Arc::new(Mutex::new(String::new()));
    let reader = capture_lines(
        LateReader {
            first: Some(b"early\n"),
            late: Some(b"late\n"),
            late_returned: false,
            reached: reached_tx,
            hold: hold_rx,
            appended: appended_tx,
        },
        Arc::clone(&log),
    );
    // Blocks until the reader reached its blocking read (`early` is in the
    // log by then); a hang is the test runner's to report (R6).
    reached_rx
        .recv()
        .expect("the reader reached its blocking read");
    let clock = Cell::new(Duration::ZERO);
    let (done, saved) = finish_capture(
        reader,
        &log,
        STDERR_DRAIN_TIMEOUT,
        &mut || clock.get(),
        &mut || clock.set(clock.get() + Duration::from_millis(5)),
    );
    assert!(!done);
    let want = format!("early\n{}", drain_timeout_note(STDERR_DRAIN_TIMEOUT));
    assert_eq!(saved, want);
    // The detached reader now appends its line; it signals once the line
    // is in the live log (no clock, no polling: R6).
    drop(hold_tx);
    appended_rx
        .recv()
        .expect("the reader appended the late line");
    assert_eq!(
        *log.lock().unwrap(),
        "early\nlate\n",
        "the live log has the late line"
    );
    assert_eq!(saved, want, "the saved snapshot is a copy");
    assert!(saved.ends_with(&drain_timeout_note(STDERR_DRAIN_TIMEOUT)));
    assert!(!saved.contains("late"));
}

/// REQ-LAT-001: a down that wakes on time keeps the planned 375 ms hold, a
/// down that wakes 200 ms late keeps the same hold (the slip grows by the
/// measured lateness after the sleep), and a step that is already late
/// grows the slip by `now - due`, exactly once per step.
#[test]
fn req_lat_001_harness_oversleep_shifts_later_steps() {
    use driver::schedule_step;
    let plan = [Duration::from_millis(1000), Duration::from_millis(1375)];
    let run = |oversleep: Duration| {
        let clock = Cell::new(Duration::ZERO);
        let first = Cell::new(true);
        let mut slip = Duration::ZERO;
        let mut sent = Vec::new();
        for at in plan {
            schedule_step(at, &mut slip, &mut || clock.get(), &mut |d| {
                let extra = if first.replace(false) {
                    oversleep
                } else {
                    Duration::ZERO
                };
                clock.set(clock.get() + d + extra);
            });
            sent.push(clock.get());
        }
        (sent, slip)
    };
    let (sent, slip) = run(Duration::ZERO);
    assert_eq!(sent[1] - sent[0], Duration::from_millis(375));
    assert_eq!(slip, Duration::ZERO);
    let (sent, slip) = run(Duration::from_millis(200));
    assert_eq!(sent[0], Duration::from_millis(1200));
    assert_eq!(sent[1] - sent[0], Duration::from_millis(375));
    assert_eq!(slip, Duration::from_millis(200));
    // A late step (no sleep): slip grows by now - due, once.
    let clock = Cell::new(Duration::from_millis(1300));
    let mut slip = Duration::ZERO;
    let slept = Cell::new(false);
    schedule_step(
        Duration::from_millis(1000),
        &mut slip,
        &mut || clock.get(),
        &mut |_| slept.set(true),
    );
    assert!(!slept.get());
    assert_eq!(slip, Duration::from_millis(300));
}

/// REQ-LAT-003 harness (gate 30 round 11): the verdict summary is written
/// through a function that reports the failure with the path (a parent that
/// is a regular file cannot be written under), and the good case writes
/// exactly the text.
#[test]
fn req_lat_003_harness_summary_write_failure_is_reported() {
    let dir = driver_dir("summary_write");
    std::fs::create_dir_all(&dir).unwrap();
    write_summary(&dir, "verdict: PASS\n").unwrap();
    assert_eq!(
        std::fs::read_to_string(dir.join("summary.txt")).unwrap(),
        "verdict: PASS\n"
    );
    let blocker = dir.join("not_a_dir");
    std::fs::write(&blocker, "x").unwrap();
    let e = write_summary(&blocker, "verdict: PASS\n").unwrap_err();
    assert!(
        e.contains("summary.txt") && e.contains("not_a_dir") && e.contains("writing"),
        "{e}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// Writes the verdict summary to `<out_dir>/summary.txt`; the error names
/// the full path, so a full disk fails the test instead of leaving the
/// artifacts without a verdict.
fn write_summary(out_dir: &std::path::Path, summary: &str) -> Result<(), String> {
    let path = out_dir.join("summary.txt");
    std::fs::write(&path, summary).map_err(|e| format!("writing {}: {e}", path.display()))
}

/// The windowed driver's artifact directory for `tag` (emptied first).
fn driver_dir(tag: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("kp_windowed_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

/// The windowed driver (gate 30, M6): the preflight creates the artifact
/// directory and removes the previous run's trace, stderr log, summary and
/// screenshots BEFORE its tool, binary and bundle checks, and a failed
/// check writes a fresh `summary.txt` (`verdict: FAIL (windowed driver:
/// …)`), so a stale passing summary never survives a run that could not
/// start.
#[test]
fn req_lat_002_harness_preflight_replaces_stale_artifacts() {
    let dir = driver_dir("preflight");
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    for f in [
        "summary.txt",
        "latency_trace.csv",
        "pin_app_stderr.log",
        "frame_1_left_held.png",
    ] {
        std::fs::write(out.join(f), "verdict: PASS\n").unwrap();
    }
    std::fs::write(out.join("keep.txt"), "other").unwrap();
    let app = dir.join("pin_app");
    std::fs::write(&app, "").unwrap();
    let cfg = driver::DriverConfig {
        app,
        bundle: dir.join("no_bundle"),
        out_dir: out.clone(),
        run_ms: 60_000,
        lp_num_threads: "2".into(),
        seed: 1,
        frame_interval_ms: 125,
    };
    let e = driver::preflight(&cfg, &|_| true).unwrap_err();
    assert!(e.starts_with("no grey-box bundle at "), "{e}");
    let summary = std::fs::read_to_string(out.join("summary.txt")).unwrap();
    assert!(
        summary.starts_with(&format!("verdict: FAIL (windowed driver: {e})")),
        "{summary}"
    );
    assert!(!summary.contains("verdict: PASS"), "{summary}");
    let mut left: Vec<String> = std::fs::read_dir(&out)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    left.sort();
    assert_eq!(left, ["keep.txt", "summary.txt"]);
    // A missing tool, into an artifact directory that does not exist yet.
    let fresh = dir.join("fresh");
    let cfg = driver::DriverConfig {
        out_dir: fresh.clone(),
        ..cfg
    };
    let e = driver::preflight(&cfg, &|t| t != "xdotool").unwrap_err();
    assert!(e.starts_with("xdotool not found"), "{e}");
    let summary = std::fs::read_to_string(fresh.join("summary.txt")).unwrap();
    assert!(
        summary.starts_with("verdict: FAIL (windowed driver: xdotool not found"),
        "{summary}"
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The windowed driver (gate 30 round 2): a startup failure after the
/// preflight (Xvfb, or an app that cannot execute) writes a failure
/// `summary.txt` through the same writer and no `pin_app_stderr.log`.
#[test]
fn req_lat_002_harness_startup_failure_writes_a_summary() {
    let dir = driver_dir("startup");
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let app = dir.join("pin_app");
    std::fs::write(&app, "not a program").unwrap();
    let cfg = driver::DriverConfig {
        app,
        bundle: dir.join("bundle"),
        out_dir: out.clone(),
        run_ms: 1000,
        lp_num_threads: "2".into(),
        seed: 1,
        frame_interval_ms: 125,
    };
    // The shared writer, with a Xvfb-style error.
    let what = "Xvfb did not start: no display";
    assert_eq!(driver::startup_failed(&cfg, what), what);
    let summary = std::fs::read_to_string(out.join("summary.txt")).unwrap();
    assert!(
        summary.starts_with(&format!("verdict: FAIL (windowed driver: {what})")),
        "{summary}"
    );
    std::fs::remove_file(out.join("summary.txt")).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt as _;
        std::fs::set_permissions(&cfg.app, std::fs::Permissions::from_mode(0o644)).unwrap();
        let e = driver::spawn_app(&cfg, ":99", &out.join("t.csv")).unwrap_err();
        assert!(e.starts_with("starting "), "{e}");
        let shown = driver::startup_failed(&cfg, &e);
        assert_eq!(shown, e);
        let summary = std::fs::read_to_string(out.join("summary.txt")).unwrap();
        assert!(
            summary.starts_with("verdict: FAIL (windowed driver: starting"),
            "{summary}"
        );
        assert!(!out.join("pin_app_stderr.log").exists());
    }
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The windowed driver (gate 30 round 12, finding 13): `KP_PIN_APP` is
/// resolved to an absolute path once, so the preflight's file check and
/// the launch name the same file: a bare name becomes `<cwd>/<name>` (not a
/// `PATH` search), and `spawn_app` and the preflight use and report that
/// path; an absolute value (what `scripts/windowed/run.sh` exports) is
/// unchanged. Nothing is executed (the bare name does not exist).
#[test]
fn req_lat_001_harness_app_path_is_resolved_once_for_check_and_launch() {
    let cwd = std::env::current_dir().unwrap();
    let name = format!("kp_no_such_pin_app_wrapper_{}", std::process::id());
    let lookup = |k: &str| (k == "KP_PIN_APP").then(|| name.clone());
    let cfg = driver::DriverConfig::from_lookup(&lookup);
    assert_eq!(cfg.app, cwd.join(&name));
    assert_eq!(
        driver::resolve_app(std::path::Path::new(&name)),
        cwd.join(&name)
    );
    // An absolute value is kept as is.
    let abs = std::env::temp_dir().join("kp_abs").join("pin_app");
    let abs_s = abs.to_str().unwrap().to_owned();
    let cfg_abs =
        driver::DriverConfig::from_lookup(&|k: &str| (k == "KP_PIN_APP").then(|| abs_s.clone()));
    assert_eq!(cfg_abs.app, abs);
    assert_eq!(driver::resolve_app(&abs), abs);
    // Unset or empty: this build's binary, absolute.
    let def = driver::DriverConfig::from_lookup(&|_: &str| Some(String::new()));
    assert_eq!(
        def.app,
        std::path::PathBuf::from(env!("CARGO_BIN_EXE_pin_app"))
    );
    assert!(def.app.is_absolute());
    // A config built with the bare name directly: the launch and the
    // preflight both use `<cwd>/<name>`.
    let dir = driver_dir("app_path");
    let raw = driver::DriverConfig {
        app: std::path::PathBuf::from(&name),
        out_dir: dir.join("out"),
        ..cfg
    };
    let e = driver::spawn_app(&raw, ":99", &dir.join("t.csv")).unwrap_err();
    assert!(
        e.starts_with(&format!("starting {}: ", cwd.join(&name).display())),
        "{e}"
    );
    let e = driver::preflight(&raw, &|_| true).unwrap_err();
    assert_eq!(e, format!("no app binary at {}", cwd.join(&name).display()));
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The windowed driver (gate 30 round 12, finding 14): a failure to write
/// `pin_app_stderr.log` is never dropped. Both fine: the drive's output;
/// drive fine, write failed: an error naming the log path and the I/O
/// error; drive failed and write failed: the drive failure first
/// (primary), then the write failure on its own line.
#[test]
fn req_lat_003_harness_stderr_log_write_failure_is_reported() {
    use driver::with_log_write;
    let path = std::path::Path::new("/out/pin_app_stderr.log");
    let disk_full = || std::io::Error::other("disk full");
    assert_eq!(with_log_write(Ok(7), Ok(()), path), Ok(7));
    assert_eq!(
        with_log_write(Ok(7), Err(disk_full()), path),
        Err::<i32, _>("writing /out/pin_app_stderr.log: disk full".to_owned())
    );
    assert_eq!(
        with_log_write::<i32>(Err("no first frame".into()), Ok(()), path),
        Err("no first frame".to_owned())
    );
    let e =
        with_log_write::<i32>(Err("no first frame".into()), Err(disk_full()), path).unwrap_err();
    assert_eq!(
        e,
        "no first frame\nwriting /out/pin_app_stderr.log: disk full"
    );
    assert!(
        e.starts_with("no first frame"),
        "the drive failure is primary"
    );
}

/// A driver configuration whose preflight checks pass: an app file and a
/// grey-box bundle (`table.toml`) under `dir`, artifacts into `out`.
fn passing_cfg(dir: &std::path::Path, out: std::path::PathBuf) -> driver::DriverConfig {
    std::fs::create_dir_all(dir.join("bundle")).unwrap();
    std::fs::write(dir.join("bundle").join("table.toml"), "").unwrap();
    std::fs::write(dir.join("pin_app"), "").unwrap();
    driver::DriverConfig {
        app: dir.join("pin_app"),
        bundle: dir.join("bundle"),
        out_dir: out,
        run_ms: 60_000,
        lp_num_threads: "2".into(),
        seed: 1,
        frame_interval_ms: 125,
    }
}

/// The I/O error text `std::fs::write` gives for `path` (a directory here).
fn write_error_text(path: &std::path::Path) -> String {
    std::fs::write(path, "").unwrap_err().to_string()
}

/// The windowed driver (gate 30 round 13, finding 9): the press hold is
/// three frame intervals capped at 1 s, computed with saturating
/// arithmetic, so a huge `KP_WINDOWED_FRAME_MS` gives the cap (not an
/// overflow panic or a wrapped 2 ms hold); the default 125 ms interval
/// keeps its 375 ms hold.
#[test]
fn req_lat_001_harness_base_hold_cap_cannot_overflow() {
    assert_eq!(driver::base_hold_us(125), 375_000);
    assert_eq!(
        driver::base_hold_us(driver::DEFAULT_FRAME_INTERVAL_MS),
        375_000
    );
    assert_eq!(driver::base_hold_us(1), 3_000);
    assert_eq!(driver::base_hold_us(333), 999_000);
    assert_eq!(driver::base_hold_us(334), 1_000_000);
    for big in [6_148_914_691_236_517_206u64, u64::MAX, u64::MAX / 3 + 1] {
        assert_eq!(driver::base_hold_us(big), 1_000_000, "interval {big} ms");
    }
    // From the environment through the plan: the cap, a usable plan.
    let cfg = driver::DriverConfig::from_lookup(&|k: &str| {
        (k == "KP_WINDOWED_FRAME_MS").then(|| "6148914691236517206".to_owned())
    });
    assert_eq!(cfg.frame_interval_ms, 6_148_914_691_236_517_206);
    let plan = driver::press_plan(1, 30_000_000, cfg.frame_interval_ms);
    assert!(driver::planned_presses(&plan) > 10);
}

/// The windowed driver (gate 30 round 13, finding 10): the preflight stops
/// when a previous run's artifact cannot be removed (here an artifact name
/// that is a directory: `remove_file` fails on it), naming the path, and
/// writes a failure summary; an absent artifact is fine, and so are
/// removable ones (the preflight then passes with everything removed).
#[test]
fn req_lat_002_harness_preflight_stops_when_old_evidence_stays() {
    let dir = driver_dir("cleanup");
    // A screenshot name that cannot be removed.
    let out = dir.join("out_frame");
    std::fs::create_dir_all(out.join("frame_x.png")).unwrap();
    std::fs::write(out.join("latency_trace.csv"), "old").unwrap();
    let cfg = passing_cfg(&dir, out.clone());
    let e = driver::preflight(&cfg, &|_| true).unwrap_err();
    let stuck = out.join("frame_x.png");
    assert!(
        e.contains(&format!("removing {}: ", stuck.display())),
        "the stuck path is named: {e}"
    );
    assert!(
        !out.join("latency_trace.csv").exists(),
        "the rest is removed"
    );
    let summary = std::fs::read_to_string(out.join("summary.txt")).unwrap();
    assert!(
        summary.starts_with("verdict: FAIL (windowed driver: ")
            && summary.contains(&stuck.display().to_string()),
        "{summary}"
    );
    // A summary that cannot be removed: the error names it, and the
    // failure summary cannot be written over it either (reported, finding
    // 11), the cleanup error first.
    let out = dir.join("out_summary");
    std::fs::create_dir_all(out.join("summary.txt")).unwrap();
    let cfg = passing_cfg(&dir, out.clone());
    let e = driver::preflight(&cfg, &|_| true).unwrap_err();
    let path = out.join("summary.txt");
    assert!(
        e.starts_with(&format!("removing {}: ", path.display())),
        "{e}"
    );
    assert!(
        e.contains(&format!(
            "\nwriting {}: {}",
            path.display(),
            write_error_text(&path)
        )),
        "{e}"
    );
    // Nothing old (absent artifacts are fine) and removable old artifacts.
    let out = dir.join("out_clean");
    let cfg = passing_cfg(&dir, out.clone());
    driver::preflight(&cfg, &|_| true).unwrap();
    for f in ["summary.txt", "pin_app_stderr.log", "frame_0_rest.png"] {
        std::fs::write(out.join(f), "verdict: PASS\n").unwrap();
    }
    driver::preflight(&cfg, &|_| true).unwrap();
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The windowed driver (gate 30 round 13, finding 11): a failure to write
/// the failure `summary.txt` is reported after the failure itself, with the
/// summary path and the I/O error (the shared writer and the startup path);
/// a successful write returns the failure text unchanged.
#[test]
fn req_lat_003_harness_failure_summary_write_error_is_reported() {
    let dir = driver_dir("failure_summary");
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let cfg = passing_cfg(&dir, out.clone());
    assert_eq!(
        driver::write_failure_summary(&cfg, "no first frame", "line\n"),
        "no first frame"
    );
    let summary = std::fs::read_to_string(out.join("summary.txt")).unwrap();
    assert!(
        summary.starts_with("verdict: FAIL (windowed driver: no first frame)")
            && summary.contains("line"),
        "{summary}"
    );
    std::fs::remove_file(out.join("summary.txt")).unwrap();
    // `summary.txt` is a directory: the write fails.
    let path = out.join("summary.txt");
    std::fs::create_dir_all(&path).unwrap();
    let io = write_error_text(&path);
    let want = format!("no first frame\nwriting {}: {io}", path.display());
    assert_eq!(
        driver::write_failure_summary(&cfg, "no first frame", "line\n"),
        want
    );
    let what = "Xvfb did not start: no display";
    assert_eq!(
        driver::startup_failed(&cfg, what),
        format!("{what}\nwriting {}: {io}", path.display())
    );
    std::fs::remove_dir_all(&dir).unwrap();
}

/// The windowed driver (gate 30 round 13, finding 12): the real-window
/// path clears the previous artifacts BEFORE the bounded-runner self-check,
/// and a failing self-check publishes a fresh failure summary, so a stale
/// `verdict: PASS` never survives it; a passing self-check writes nothing.
/// The self-check is injected (nothing runs, no wall clock).
#[test]
fn req_lat_002_harness_self_check_failure_replaces_a_stale_pass() {
    let dir = driver_dir("self_check");
    let out = dir.join("out");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("summary.txt"), "verdict: PASS\n").unwrap();
    std::fs::write(out.join("frame_1_left_held.png"), "old").unwrap();
    let cfg = passing_cfg(&dir, out.clone());
    let why = "bounded runner: spawning \"sleep\": No such file or directory";
    let e = driver::prepare(&cfg, &|_| true, &|| Err(why.to_owned())).unwrap_err();
    assert!(e.contains(why), "{e}");
    let summary = std::fs::read_to_string(out.join("summary.txt")).unwrap();
    assert!(
        summary.starts_with("verdict: FAIL (windowed driver: ") && summary.contains(why),
        "{summary}"
    );
    assert!(!summary.contains("verdict: PASS"), "{summary}");
    assert!(!out.join("frame_1_left_held.png").exists());
    // A passing self-check: the cleaned directory stays empty.
    std::fs::write(out.join("summary.txt"), "verdict: PASS\n").unwrap();
    driver::prepare(&cfg, &|_| true, &|| Ok(())).unwrap();
    assert_eq!(std::fs::read_dir(&out).unwrap().count(), 0);
    std::fs::remove_dir_all(&dir).unwrap();
}

/// REQ-LAT-003 / REQ-LAT-004 (gate 30 round 13, finding 13): a run-end
/// marker that predates a delivered press contradicts the app's causal
/// order (the marker is read after the last pump). The review's case
/// (a valid press at 30 s; a press at 57 s whose bat never moved; marker at
/// 56 s) fails, naming the press and the marker, and neither the check-a
/// tail excuse nor the check-b end exclusion is applied to it. Presses
/// genuinely close to the end keep both exceptions (marker 57.1 s with the
/// press at 57.095 s, and a marker equal to the press's stamp).
#[test]
fn req_lat_003_verdict_rejects_a_run_end_marker_before_a_press() {
    let rows = |second_ms: u64| {
        let mut late = press(1, T0 + second_ms * TICK_NS, 300_000);
        late.input = "right_flipper".into();
        late.move_tick = None;
        vec![press(0, T0 + 30_000 * TICK_NS, 300_000), late]
    };
    let marker = T0 + 56_000 * TICK_NS;
    let t = joined_until(rows(57_000), frames(600), marker);
    assert!(t.rows[0].frame.is_some());
    assert_eq!(t.rows[1].frame, None);
    let v = evaluate(&t, &OK_SHUTDOWN, &params());
    assert!(!v.passed(), "{}", v.report());
    let stamp = T0 + 57_000 * TICK_NS;
    assert!(
        v.d.failures.iter().any(|f| f.contains("press #1")
            && f.contains(&format!("{stamp} ns"))
            && f.contains(&format!("run-end marker {marker} ns"))),
        "{}",
        v.report()
    );
    assert!(
        v.a.failures
            .iter()
            .any(|f| f.contains("press #1") && f.contains("never moved")),
        "no tail excuse: {}",
        v.report()
    );
    assert!(
        v.b.failures.iter().any(|f| f.contains("press #1")),
        "not excluded: {}",
        v.report()
    );
    assert_eq!(v.summary.excluded_presses, 0);
    // Controls: the press inside both windows of a consistent marker.
    for (second_ms, end) in [
        (57_095, T0 + 57_100 * TICK_NS),
        (57_000, T0 + 57_000 * TICK_NS),
    ] {
        // Applied in the tick ending 0.3 ms after the stamp: within the
        // tail excuse's ten ticks of the marker.
        let v = evaluate(
            &joined_until(rows(second_ms), frames(600), end),
            &OK_SHUTDOWN,
            &params(),
        );
        assert!(v.passed(), "press at {second_ms} ms: {}", v.report());
        assert_eq!(v.summary.excluded_presses, 1);
    }
}

/// REQ-LAT-001 check b (gate 30 round 13, finding 14): the response frame
/// cannot show a snapshot of input the sim had not drained yet. Press at
/// 100 ms, drained at 102 ms, the trace's frame presented at 101 ms with
/// both tables agreeing: check b fails (press, frame, present and drain
/// times named) and the frame is not counted; presented at the drain time
/// (the causal bound, equal passes): accepted.
#[test]
fn req_lat_001_verdict_rejects_frame_presented_before_the_drain() {
    let ts = T0 + 100 * TICK_NS;
    let row = || {
        let mut r = press(0, ts, TICK_NS);
        assert_eq!(r.apply_tick, 101);
        r.move_tick = Some(101);
        r.prev_drain_sdl_ns = T0 + 101 * TICK_NS;
        r.drain_sdl_ns = T0 + 102 * TICK_NS;
        r
    };
    let fr = frames_at(&[90, 101, 220, 59_900], &[80, 101, 210, 59_000]);
    let t = joined(vec![row()], fr);
    assert_eq!(t.rows[0].frame, Some(1));
    let shutdown = Shutdown {
        frames: 4,
        ..OK_SHUTDOWN
    };
    let v = evaluate(&t, &shutdown, &params());
    assert!(v.a.passed(), "{}", v.report());
    assert!(!v.b.passed(), "{}", v.report());
    assert_eq!(v.b.failures.len(), 2, "one for the row: {:?}", v.b);
    assert!(
        v.b.failures[0].contains("press #0")
            && v.b.failures[0].contains("frame 1")
            && v.b.failures[0].contains(&format!("presented at {} ns", T0 + 101 * TICK_NS))
            && v.b.failures[0].contains(&format!("drained at {} ns", T0 + 102 * TICK_NS)),
        "{:?}",
        v.b
    );
    assert_eq!(v.b.failures[1], "no press could be checked", "not counted");
    // Presented at the drain time: accepted.
    let fr = frames_at(&[90, 102, 220, 59_900], &[80, 101, 210, 59_000]);
    let v = evaluate(&joined(vec![row()], fr), &shutdown, &params());
    assert!(v.passed(), "{}", v.report());
}

/// REQ-LAT-003 check d (gate 30 round 13, finding 15): in the windowed run
/// (one playfield surface) the shutdown line's frame count is the number
/// of playfield presents, so a trace whose frame table is shorter or
/// longer (a missing last row without a recorded overflow) fails check d;
/// equal counts pass; a trace with recorded overflows fails once, for the
/// overflow.
#[test]
fn req_lat_003_check_d_frame_rows_match_the_shutdown_count() {
    let t = joined(vec![press(0, T0 + 1_234_567_891, 765_432)], frames(600));
    let v = evaluate(&t, &OK_SHUTDOWN, &params());
    assert!(v.passed(), "{}", v.report());
    for f in [601u64, 599] {
        let shutdown = Shutdown {
            frames: f,
            ..OK_SHUTDOWN
        };
        let v = evaluate(&t, &shutdown, &params());
        assert_eq!(v.failed_checks(), vec!["d"], "{}", v.report());
        assert_eq!(v.d.failures.len(), 1, "{:?}", v.d);
        assert!(
            v.d.failures[0].contains("600 frame rows")
                && v.d.failures[0].contains(&format!("{f} frames"))
                && v.d.failures[0].contains("frame rows missing or extra"),
            "{:?}",
            v.d
        );
    }
    let mut lost = t.clone();
    lost.overflows = 1;
    let shutdown = Shutdown {
        frames: 601,
        ..OK_SHUTDOWN
    };
    let v = evaluate(&lost, &shutdown, &params());
    assert_eq!(v.d.failures.len(), 1, "one cause, one failure: {:?}", v.d);
    assert!(v.d.failures[0].contains("overflows"), "{:?}", v.d);
}

// ---------------------------------------------------------------------------
// The 60 s windowed run.

/// REQ-LAT-001, REQ-LAT-002, REQ-LAT-003, REQ-LAT-004 end to end: the
/// release app with the grey-box table under Xvfb on lavapipe, flipper
/// presses through XTest for 60 s, checks a to d on its trace and shutdown
/// line. Artifacts (trace, stderr, screenshots, summary) go to
/// `KP_WINDOWED_OUT` (default `<target>/windowed`).
#[test]
#[ignore = "60 s windowed run: needs Xvfb, lavapipe and a release build (windowed.yml, scripts/windowed/run.sh)"]
fn windowed_latency_60s() {
    // `driver::run` starts with `driver::prepare`: the preflight clears the
    // previous run's artifacts, then the bounded runner's real-process
    // self-check runs (wall time is measured here, in the windowed run,
    // never in the headless suite, R6); a failed self-check leaves a fresh
    // failure summary.
    let cfg = driver::DriverConfig::from_env();
    let out = driver::run(&cfg).unwrap_or_else(|e| panic!("windowed driver failed: {e}"));
    let p = Params {
        expected_ticks: cfg.run_ms,
        injected_presses: Some(out.injected_presses),
        ..Params::default()
    };
    let verdict = verdict::evaluate_text(&out.trace, &out.stderr, &p);
    let report = match &verdict {
        Ok(v) => v.report(),
        Err(e) => format!("verdict: FAIL (unreadable output: {e})\n"),
    };
    // A failed app exit is the overall verdict whatever the measurements
    // say, so the artifact and the job agree (gate 30, M5).
    let exit_failure = (!out.exit.success()).then(|| out.exit.to_string());
    let report = verdict::overall_report(&report, exit_failure.as_deref());
    let summary = format!(
        "LP_NUM_THREADS={} seed={} run_ms={} frame_interval_ms={} injected={} app exit {}\n\
         screenshots: {:?}\n{report}",
        cfg.lp_num_threads,
        cfg.seed,
        cfg.run_ms,
        cfg.frame_interval_ms,
        out.injected_presses,
        out.exit,
        out.screenshots
    );
    println!("{summary}");
    write_summary(&cfg.out_dir, &summary).unwrap_or_else(|e| panic!("summary not saved: {e}"));
    assert!(
        out.exit.success(),
        "pin_app exited with {}\n{}",
        out.exit,
        driver::tail(&out.stderr, 40)
    );
    assert!(
        verdict.as_ref().is_ok_and(verdict::Verdict::passed),
        "windowed latency checks failed:\n{report}"
    );
}
