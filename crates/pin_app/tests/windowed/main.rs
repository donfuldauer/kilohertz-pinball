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
use verdict::{Frame, Params, Row, Shutdown, Trace, evaluate, parse_shutdown, parse_trace};

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

/// REQ-LAT-001 check b: a raw count of 1 and 2 frames passes, 3 fails.
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
        assert_eq!(v.summary.max_frames_raw, want, "move {move_ms}");
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
    assert_eq!(v.summary.max_frames_raw, 1);
    assert!(v.b.passed(), "{:?}", v.b);
}

/// REQ-LAT-001, REQ-LAT-003 check b (O1): a press before the first
/// presented frame counts from frame index 0, so a bat first shown in frame
/// 2 is 3 frames after it (not 2: "no frame before the press" is not
/// "frame 0 was"); a press after frame 0 presented, shown in frame 2, is 2.
/// There is no in-flight allowance (#149): frame 0, in flight at the press
/// and presented 5 ms after it, is counted, so the raw 3 fails.
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
    assert_eq!(verdict::raw_frames(&fr, before, 2), 3);
    assert_eq!(verdict::raw_frames(&fr, before, 0), 1);
    assert_eq!(verdict::raw_frames(&fr, after, 2), 2);
    // Through the verdict: frame 0 presented inside the old allowance
    // window (press + 11 ms): raw 3, not rescued, fails.
    let r = press(0, before, 300_000);
    let t = joined_until(vec![r], fr, T0 + 2_000_000_000);
    assert_eq!(t.rows[0].frame, Some(2));
    assert_eq!(t.rows[0].frames_to_photon, Some(3));
    let v = evaluate(&t, &OK_SHUTDOWN, &params());
    assert!(v.a.passed(), "{}", v.report());
    assert!(!v.b.passed(), "{}", v.report());
    assert_eq!(v.summary.max_frames_raw, 3);
    assert_b_names_press(&v, 0, 3);
    // Frame 0 presented after the old window: the same raw 3, the same
    // failure.
    let late = frames_at(&[20, 30, 40], &[4, 4, 5]);
    let t = joined_until(vec![press(0, before, 300_000)], late, T0 + 2_000_000_000);
    assert_eq!(t.rows[0].frames_to_photon, Some(3));
    let v = evaluate(&t, &OK_SHUTDOWN, &params());
    assert!(!v.b.passed(), "{}", v.report());
    assert_eq!(v.summary.max_frames_raw, 3);
    assert_b_names_press(&v, 0, 3);
    // Shown in frame 1 (frames 0 and 1 count): raw 2 passes.
    let fr = frames_at(&[10, 20, 30], &[4, 5, 6]);
    let t = joined_until(vec![press(0, before, 300_000)], fr, T0 + 2_000_000_000);
    assert_eq!(t.rows[0].frames_to_photon, Some(2));
    let v = evaluate(&t, &OK_SHUTDOWN, &params());
    assert!(v.b.passed(), "{}", v.report());
    assert_eq!(v.summary.max_frames_raw, 2);
}

/// Check b failed for press `seq` with raw count `n`: some failure names
/// the press's sequence number and the count (REQ-LAT-001).
fn assert_b_names_press(v: &verdict::Verdict, seq: u64, n: u64) {
    assert!(!v.b.failures.is_empty(), "{}", v.report());
    assert!(
        v.b.failures
            .iter()
            .any(|f| f.contains(&format!("press #{seq} ("))
                && f.contains(&format!(" {n} presented frames after the press"))),
        "check b failure must name press #{seq} and {n} frames: {:?}",
        v.b
    );
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

/// REQ-LAT-001 check b: a raw count of 3 fails even when frame 10, in
/// flight at the press, presents 0.5 ms after it (before the coil
/// deadline): the frame in flight is not skipped, there is no allowance.
#[test]
fn req_lat_001_check_b_in_flight_frame_before_the_deadline() {
    // Frame 10 presents 0.5 ms after the press; the bat moves 1 ms after.
    let ts = T0 + 1_000_000_000 - 500_000;
    let mut r = press(0, ts, 800_000);
    r.move_tick = Some(1001);
    r.apply_tick = 1000;
    let v = evaluate(&joined(vec![r], frames(600)), &OK_SHUTDOWN, &params());
    assert!(v.a.passed(), "{}", v.report());
    assert!(!v.b.passed(), "{}", v.report());
    assert_eq!(v.summary.max_frames_raw, 3);
    assert_b_names_press(&v, 0, 3);
    let info = v.b.info.to_lowercase();
    assert!(
        !info.contains("in-flight") && !info.contains("shifted") && !info.contains("adjusted"),
        "{}",
        v.b.info
    );
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
/// gated (`n_raw <= 2`) with no allowance: the frame in flight at the press
/// counts wherever it presented. Raw 1 and 2 pass; raw 3 fails with the
/// in-flight frame inside the old window `(ts, ts + bound_a + coil_ticks]`
/// (press at 100 ms, frames at 101, 220 and 340 ms, the bat first visible
/// at 340 ms), at its edge and outside it; raw 4 fails; a press 700 ms late
/// fails check a and check b, rescued by nothing.
#[test]
fn req_lat_001_check_b_raw_count_with_one_in_flight_frame() {
    let p = Params {
        coil_ticks: 2,
        ..params()
    };
    let ts = T0 + 100 * TICK_NS;
    // A press at 100 ms applied in tick 101 (1 ms late; bound_a 3 ms).
    let deadline_ms = 100 + 3 + p.coil_ticks;
    // (in-flight present ms, in-flight snap, move tick, raw, pass)
    for (inflight_ms, inflight_snap, mv, raw, pass) in [
        (101u64, 101u64, 101u64, 1u64, true), // raw 1
        (101, 90, 101, 2, true),              // raw 2
        (101, 90, 102, 3, false),             // inside the old window
        (deadline_ms, 90, 102, 3, false),     // the old window's edge
        (deadline_ms + 1, 90, 102, 3, false), // outside the old window
    ] {
        let fr = frames_at(
            &[90, inflight_ms, 220, 340, 460, 59_900],
            &[80, inflight_snap, 101, 220, 340, 59_000],
        );
        let mut r = press(0, ts, TICK_NS);
        r.move_tick = Some(mv);
        let t = joined(vec![r], fr);
        assert_eq!(t.rows[0].frames_to_photon, Some(raw));
        let v = evaluate(&t, &OK_SHUTDOWN, &p);
        let case = format!("in-flight {inflight_ms} ms, move {mv}: {}", v.report());
        assert!(v.a.passed(), "{case}");
        assert_eq!(v.summary.max_frames_raw, raw, "{case}");
        assert_eq!(v.b.passed(), pass, "{case}");
        if !pass {
            assert_b_names_press(&v, 0, raw);
        }
    }
    // Raw 4: fails.
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
    assert_b_names_press(&v, 0, 4);
    // 700 ms late: fails check a, and its raw 3 fails check b.
    let fr = frames_at(&[90, 101, 220, 900, 59_900], &[80, 90, 101, 800, 59_000]);
    let mut late = press(0, ts, 700 * TICK_NS);
    late.move_tick = Some(late.apply_tick);
    let v = evaluate(&joined(vec![late], fr), &OK_SHUTDOWN, &p);
    assert!(!v.a.passed(), "{}", v.report());
    assert!(!v.b.passed(), "{}", v.report());
    assert_eq!(v.summary.max_frames_raw, 3);
    assert_b_names_press(&v, 0, 3);
}

/// The frame table of the #149 in-flight table: frame 0 presented 10 ms
/// before the press at `ts` (F0, drawing tick 89), frame 1 the frame in
/// flight at the press, presented `inflight_ns` after it, then a frame every
/// 16 ms. Frames 1 to `raised - 1` draw tick 95 (before the move at tick
/// 101), frame `raised` and later draw tick 101 onwards, so frame `raised`
/// is the first candidate and the raw count is `raised`.
fn in_flight_table(ts: u64, inflight_ns: u64, raised: u64) -> Vec<Frame> {
    let mut fr = vec![Frame {
        frame: 0,
        snap_tick: 89,
        present_ns: ts - 10 * TICK_NS,
    }];
    for k in 1..8u64 {
        fr.push(Frame {
            frame: k,
            snap_tick: if k < raised {
                95
            } else {
                101 + 16 * (k - raised)
            },
            present_ns: ts + inflight_ns + (k - 1) * 16 * TICK_NS,
        });
    }
    fr
}

/// REQ-LAT-001 (#149, CI form, table-driven): the bat first shown raised
/// at raw count 1 or 2 passes check b and at 3 or 4 fails it, whether the
/// frame in flight at the press presented 0.2 ms, 1 ms, 4.7 ms or 30 ms
/// after the press (0.2 to 4.7 ms lie inside the removed allowance window
/// `(ts, ts + bound_a + 8 ticks]`, 30 ms outside it): the verdict does not
/// depend on when the in-flight frame presented.
#[test]
fn req_lat_001_check_b_raw_count_table_ignores_in_flight_present_time() {
    let ts = T0 + 100 * TICK_NS;
    for inflight_ns in [200_000u64, 1_000_000, 4_700_000, 30_000_000] {
        for raised in 1u64..=4 {
            let fr = in_flight_table(ts, inflight_ns, raised);
            // Applied in tick 101 (ends 1 ms after the press), coil at once.
            let mut r = press(0, ts, TICK_NS);
            r.move_tick = Some(101);
            let t = joined_until(vec![r], fr.clone(), T0 + 2_000_000_000);
            let case = format!("in-flight +{inflight_ns} ns, raised at {raised}");
            assert_eq!(t.rows[0].frame, Some(raised), "{case}");
            assert_eq!(t.rows[0].frames_to_photon, Some(raised), "{case}");
            assert_eq!(verdict::raw_frames(&fr, ts, raised), raised, "{case}");
            let v = evaluate(&t, &OK_SHUTDOWN, &params());
            let case = format!("{case}: {}", v.report());
            assert!(v.a.passed() && v.c.passed() && v.d.passed(), "{case}");
            assert_eq!(v.summary.excluded_presses, 0, "{case}");
            assert_eq!(v.summary.max_frames_raw, raised, "{case}");
            assert_eq!(v.summary.p99_frames_raw, raised, "{case}");
            assert_eq!(v.b.passed(), raised <= 2, "{case}");
            assert_eq!(v.passed(), raised <= 2, "{case}");
            if raised > 2 {
                assert_b_names_press(&v, 0, raised);
                assert_eq!(v.failed_checks(), vec!["b"], "{case}");
            } else {
                assert!(v.b.failures.is_empty(), "{case}");
            }
        }
    }
}

/// The wording the removed allowance used to print, none of which may
/// appear in the report, check b's info line or its failures.
const ALLOWANCE_WORDS: [&str; 5] = [
    "in-flight shifted",
    "shifted",
    "adjusted",
    "allowance",
    "in-flight",
];

/// REQ-LAT-001 (#149): the summary and check b's info line print the raw
/// max and p99 only: no "in-flight shifted", no "adjusted" (nor any other
/// allowance wording) in a passing or a failing verdict, in the report, the
/// info line or the failures.
#[test]
fn req_lat_001_report_has_no_in_flight_or_adjusted_wording() {
    let ts = T0 + 100 * TICK_NS;
    for (inflight_ns, raised) in [(1_000_000u64, 2u64), (1_000_000, 3), (30_000_000, 4)] {
        let mut r = press(0, ts, TICK_NS);
        r.move_tick = Some(101);
        let t = joined_until(
            vec![r],
            in_flight_table(ts, inflight_ns, raised),
            T0 + 2_000_000_000,
        );
        let v = evaluate(&t, &OK_SHUTDOWN, &params());
        assert_eq!(v.b.passed(), raised <= 2, "{}", v.report());
        let text = format!("{}\n{}\n{:?}", v.report(), v.b.info, v.b.failures).to_lowercase();
        for w in ALLOWANCE_WORDS {
            assert!(!text.contains(w), "{w:?} in: {text}");
        }
        // The raw max and p99 are what the info line reports.
        assert!(
            v.b.info.contains(&format!("raw max {raised} p99 {raised}")),
            "{}",
            v.b.info
        );
    }
    // The text path too (the windowed run's report).
    let (c, ..) = latency_trace(bindings(), now);
    let v = verdict::evaluate_text(
        &c.collect(T0).to_csv(),
        "pin_app: 60000 sim ticks (0 dropped), 600 frames, input dropped 0, 0 haptic events\n",
        &Params::default(),
    )
    .unwrap();
    let text = format!("{}\n{}", v.report(), v.b.info).to_lowercase();
    for w in ALLOWANCE_WORDS {
        assert!(!text.contains(w), "{w:?} in: {text}");
    }
}

/// REQ-LAT-001 (#149): the rest of check b behaves as before under the
/// raw-count rule. A press stamped less than 500 ms before `run_end_ns` is
/// excluded and counted even with a raw count of 3 (at exactly 500 ms it is
/// checked and fails); a press whose bat never moved fails; a trace whose
/// `frames_to_photon` disagrees with the frame table fails (including the
/// old shifted value, raw - 1, and a press before the first frame written
/// as `frame` instead of `frame + 1`).
#[test]
fn req_lat_001_check_b_exclusion_no_move_mismatch_and_first_frame_unchanged() {
    let ts = T0 + 100 * TICK_NS;
    let raw3 = |run_end_ns: u64| {
        let mut r = press(0, ts, TICK_NS);
        r.move_tick = Some(101);
        joined_until(vec![r], in_flight_table(ts, 1_000_000, 3), run_end_ns)
    };
    // Within the final 500 ms: excluded, counted, its raw 3 not failed (a
    // run whose every press is excluded fails as a whole, as before, so
    // the press is checked by name).
    for end in [ts + 499_999_999, ts + 400_000_000] {
        let v = evaluate(&raw3(end), &OK_SHUTDOWN, &params());
        assert_eq!(v.summary.excluded_presses, 1, "{}", v.report());
        assert!(
            !v.b.failures.iter().any(|f| f.contains("press #0 (")),
            "{}",
            v.report()
        );
    }
    // Exactly 500 ms before the end: checked, and the raw 3 fails.
    let v = evaluate(&raw3(ts + 500_000_000), &OK_SHUTDOWN, &params());
    assert!(!v.b.passed(), "{}", v.report());
    assert_eq!(v.summary.excluded_presses, 0);
    assert_b_names_press(&v, 0, 3);

    // No move: fails check b.
    let mut t = raw3(T0 + 2_000_000_000);
    t.rows[0].move_tick = None;
    t.rows[0].frame = None;
    t.rows[0].frame_snap_tick = None;
    t.rows[0].present_ns = None;
    t.rows[0].frames_to_photon = None;
    let v = evaluate(&t, &OK_SHUTDOWN, &params());
    assert!(!v.b.passed(), "{}", v.report());
    assert!(
        v.b.failures.iter().any(|f| f.contains("never moved")),
        "{:?}",
        v.b
    );

    // `frames_to_photon` mismatches fail; the correct raw value is
    // accepted (and fails only on the count).
    let t = raw3(T0 + 2_000_000_000);
    assert_eq!(t.rows[0].frames_to_photon, Some(3));
    for wrong in [2u64, 4, 0] {
        let mut t = t.clone();
        t.rows[0].frames_to_photon = Some(wrong);
        let v = evaluate(&t, &OK_SHUTDOWN, &params());
        assert!(!v.b.passed(), "written {wrong}: {}", v.report());
        assert!(v.a.passed(), "written {wrong}: {}", v.report());
    }
    let mut r = press(0, ts, TICK_NS);
    r.move_tick = Some(101);
    let ok = joined_until(
        vec![r],
        in_flight_table(ts, 1_000_000, 2),
        T0 + 2_000_000_000,
    );
    assert!(evaluate(&ok, &OK_SHUTDOWN, &params()).b.passed());
    let mut bad = ok.clone();
    bad.rows[0].frames_to_photon = Some(1);
    assert!(!evaluate(&bad, &OK_SHUTDOWN, &params()).b.passed());

    // A press before the first presented frame counts `frame + 1`.
    let fr = frames_at(&[10, 20, 30, 40], &[4, 6, 7, 8]);
    let before = T0 + 5 * TICK_NS;
    let t = joined_until(vec![press(0, before, 300_000)], fr, T0 + 2_000_000_000);
    assert_eq!(t.rows[0].frame, Some(1));
    assert_eq!(t.rows[0].frames_to_photon, Some(2));
    let v = evaluate(&t, &OK_SHUTDOWN, &params());
    assert!(v.b.passed(), "{}", v.report());
    assert_eq!(v.summary.max_frames_raw, 2);
    let mut without_plus_one = t.clone();
    without_plus_one.rows[0].frames_to_photon = Some(1);
    assert!(
        !evaluate(&without_plus_one, &OK_SHUTDOWN, &params())
            .b
            .passed()
    );
}

/// A generated press and frame timeline for the check b property.
#[derive(Clone, Debug)]
struct Timeline {
    /// Frame gaps (us), cycled until the run ends.
    frame_gaps_us: Vec<u64>,
    /// Per frame (cycled): how late in its interval the frame latched its
    /// snapshot, in per mille of the gap before it.
    latch_permille: Vec<u64>,
    /// Per press: (gap to the previous press in us, coil ticks, later
    /// candidate skipped, right bat?, a frame presented this many us after
    /// the press if any).
    presses: Vec<(u64, u64, u64, bool, Option<u64>)>,
    /// Run end after the last press (us).
    tail_us: u64,
}

fn timeline() -> impl Strategy<Value = Timeline> {
    (
        prop::collection::vec(4_000u64..40_000, 1..12),
        prop::collection::vec(0u64..=1000, 1..12),
        prop::collection::vec(
            (
                20_000u64..400_000,
                0u64..=8,
                0u64..3,
                any::<bool>(),
                prop::option::of(50u64..12_000),
            ),
            1..25,
        ),
        100_000u64..1_500_000,
    )
        .prop_map(
            |(frame_gaps_us, latch_permille, presses, tail_us)| Timeline {
                frame_gaps_us,
                latch_permille,
                presses,
                tail_us,
            },
        )
}

/// Builds the trace of `tl` and the raw count of every press, computed
/// here from the frame table alone (F0 the last frame presented at or
/// before the press, `frame + 1` without one), with the excluded flag
/// (stamped less than 500 ms before `run_end_ns`).
fn build_timeline(tl: &Timeline) -> (Trace, Vec<(u64, bool)>) {
    // Press stamps (the first may precede the first frame), never on a
    // whole millisecond.
    let mut stamps = Vec::new();
    let mut ts = T0 + 137;
    for (i, p) in tl.presses.iter().enumerate() {
        ts += if i == 0 { p.0 / 4 } else { p.0 } * 1000 + 211;
        stamps.push(ts);
    }
    let run_end_ns = ts + tl.tail_us * 1000;
    // Present times: the cycled gaps plus the frames placed just after a
    // press (inside the old allowance window).
    let mut presents = Vec::new();
    let mut t = T0;
    for k in 0.. {
        t += tl.frame_gaps_us[k % tl.frame_gaps_us.len()] * 1000;
        if t > run_end_ns {
            break;
        }
        presents.push(t);
    }
    for (p, &ts) in tl.presses.iter().zip(&stamps) {
        if let Some(us) = p.4 {
            presents.push(ts + us * 1000);
        }
    }
    presents.sort_unstable();
    presents.dedup();
    let mut frames: Vec<Frame> = Vec::new();
    let mut prev = T0;
    for (k, &present_ns) in presents.iter().enumerate() {
        let lag = (present_ns - prev) * tl.latch_permille[k % tl.latch_permille.len()] / 1000;
        let tick = (present_ns - lag - T0) / TICK_NS;
        let snap_tick = frames.last().map_or(tick, |f| f.snap_tick.max(tick));
        frames.push(Frame {
            frame: k as u64,
            snap_tick,
            present_ns,
        });
        prev = present_ns;
    }
    let mut rows = Vec::new();
    let mut counts = Vec::new();
    for (seq, (p, &ts)) in tl.presses.iter().zip(&stamps).enumerate() {
        // Applied in the first tick ending at or after the stamp.
        let apply_tick = (ts - T0) / TICK_NS + 1;
        let mut r = press(seq as u64, ts, T0 + apply_tick * TICK_NS - ts);
        assert_eq!(r.apply_tick, apply_tick);
        if p.3 {
            r.input = "right_flipper".into();
        }
        let move_tick = apply_tick + p.1;
        r.move_tick = Some(move_tick);
        // The first candidate, or a later one (the bat shown raised only
        // there); the last frame is always a candidate.
        let first = frames
            .iter()
            .position(|f| f.snap_tick >= move_tick)
            .expect("a frame after the move");
        let chosen = &frames[(first + p.2 as usize).min(frames.len() - 1)];
        let n = match frames.iter().rposition(|f| f.present_ns <= ts) {
            Some(f0) => chosen.frame - f0 as u64,
            None => chosen.frame + 1,
        };
        r.frame = Some(chosen.frame);
        r.frame_snap_tick = Some(chosen.snap_tick);
        r.present_ns = Some(chosen.present_ns);
        r.frames_to_photon = Some(n);
        rows.push(r);
        counts.push((n, run_end_ns - ts < 500_000_000));
    }
    (
        Trace {
            rows,
            frames,
            overflows: 0,
            run_end_ns,
        },
        counts,
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    /// REQ-LAT-001 (#149, CI form): over generated press and frame
    /// timelines (frames placed 0.05 to 12 ms after presses, inside the
    /// removed allowance window, among them), check b passes if and only
    /// if every press not stamped within 500 ms of `run_end_ns` has a raw
    /// count from the press of at most 2; checks a, c and d hold; the
    /// excluded presses are counted; every failing press is named with its
    /// raw count. When the in-flight frame presented plays no part.
    #[test]
    fn req_lat_001_check_b_passes_iff_every_raw_count_is_at_most_two(tl in timeline()) {
        let (t, counts) = build_timeline(&tl);
        let v = evaluate(&t, &OK_SHUTDOWN, &params());
        prop_assert!(v.a.passed(), "{}", v.report());
        prop_assert!(v.c.passed(), "{}", v.report());
        prop_assert!(v.d.passed(), "{}", v.report());
        let excluded = counts.iter().filter(|c| c.1).count();
        prop_assert_eq!(v.summary.excluded_presses, excluded);
        // A run whose every press is excluded fails check b as before.
        let ok = excluded < counts.len() && counts.iter().all(|&(n, ex)| ex || n <= 2);
        prop_assert_eq!(v.b.passed(), ok, "{:?}\n{}", counts, v.report());
        if excluded == 0 {
            let max = counts.iter().map(|c| c.0).max().unwrap();
            prop_assert_eq!(v.summary.max_frames_raw, max);
        }
        // Every press a failure names is a checked press with a raw count
        // above 2, named with that count (the list is capped, so not every
        // failing press need appear), and a failing count is named.
        for (seq, &(n, ex)) in counts.iter().enumerate() {
            let tag = format!("press #{seq} (");
            for f in v.b.failures.iter().filter(|f| f.contains(&tag)) {
                prop_assert!(!ex && n > 2, "press #{} raw {} named: {}", seq, n, f);
                prop_assert!(
                    f.contains(&format!(" {n} presented frames after the press")),
                    "press #{} raw {}: {}", seq, n, f
                );
            }
        }
        if counts.iter().any(|&(n, ex)| !ex && n > 2) {
            prop_assert!(
                v.b.failures.iter().any(|f| f.contains("presented frames after the press")),
                "{:?}", v.b
            );
        }
    }
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
/// marker earlier than a press is handled without underflow.
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
    // A marker before a press: excluded, no underflow, no panic.
    let v = evaluate(
        &joined_until(rows, frozen, T0 + 40_000_000_000),
        &OK_SHUTDOWN,
        &params(),
    );
    assert_eq!(v.summary.excluded_presses, 2);
    assert!(v.a.passed(), "{:?}", v.a);
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

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// REQ-LAT-004, REQ-LAT-001: on a controlled clock with stalls and
    /// dropped ticks, every row's applying tick ends at or after its stamp
    /// and is the first executed tick that had received it (drained at or
    /// after its arrival) and whose end is not before its stamp;
    /// `frames_to_photon` equals `frame - F0`, where `frame` is the first
    /// frame with a snapshot tick in `[move_tick, release_tick]` whose drawn
    /// bat is raised at least 1 degree from the applying tick's base
    /// (re-derived here from the frames' own snapshots).
    #[test]
    fn req_lat_004_first_executed_tick_and_req_lat_001_frames(s in scenario()) {
        let (text, executed, evs, frame_angles) = run_scenario(&s);
        let t = parse_trace(&text).unwrap();
        prop_assert_eq!(t.rows.len(), evs.len());
        prop_assert_eq!(t.overflows, 0);
        let releases = verdict::release_ticks(&t.rows);
        for ((r, (ts, arrival)), release) in t.rows.iter().zip(&evs).zip(&releases) {
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
            let until = release.unwrap_or(u64::MAX);
            let expected = r.move_tick.and_then(|m| {
                t.frames.iter().find(|f| {
                    f.snap_tick >= m
                        && f.snap_tick <= until
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
        |k| {
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
        |k| {
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
        |k| {
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

/// REQ-LAT-001 harness (C7): a key whose release always fails is retried
/// until the injected clock reaches the budget (never by waiting: the
/// pause only advances the fake clock), then returned with its flag still
/// set; the other key is released; the original injection error is kept.
#[test]
fn req_lat_001_harness_release_retry_budget_is_bounded() {
    use driver::{Key, finish_injection, release_held};
    let clock = std::cell::Cell::new(0u64);
    let mut held = [true, true];
    let (mut lefts, mut rights) = (0u32, 0u32);
    let failed = release_held(
        &mut held,
        |k| {
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
    // Rounds at 0, 50, 100, 150 and 200 ms (the budget): 5 tries.
    assert_eq!(lefts, 5);
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
        |_| {
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
    // Real-process evidence for the bounded runner: wall time is measured
    // here, in the windowed run, never in the headless suite (R6).
    driver::self_check_bounded_runner().unwrap_or_else(|e| panic!("{e}"));
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
    let _ = std::fs::write(cfg.out_dir.join("summary.txt"), &summary);
    println!("{summary}");
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
