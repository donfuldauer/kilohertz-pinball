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
use pin_app::orchestrator::trace::{FlipperBindings, latency_trace};
use pin_core::snapshot::FlipperState;
use pin_core::spsc::channel;
use pin_core::{
    CabinetInput, FlipperId, FrameStateSnapshot, InputEvent, InputEventKind, SimStep, TICK_NS,
    TripleBuffer,
};
use pin_render::gpu::SurfaceStatus;
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
/// way the app joins them.
fn joined(mut rows: Vec<Row>, frames: Vec<Frame>) -> Trace {
    for r in &mut rows {
        if let Some(m) = r.move_tick
            && let Some(f) = frames.iter().find(|f| f.snap_tick >= m)
        {
            r.frame = Some(f.frame);
            r.frame_snap_tick = Some(f.snap_tick);
            r.present_ns = Some(f.present_ns);
            r.frames_to_photon =
                Some(f.frame - verdict::last_frame_at_or_before(&frames, r.sdl_ts_ns));
        }
    }
    Trace {
        rows,
        frames,
        overflows: 0,
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
/// tick: a bat that moves long after the delivering tick fails check a
/// (the pre-#81 re-stamp bug delivered on time but energised late).
#[test]
fn req_lat_004_check_a_coil_takes_effect_in_the_applying_tick() {
    let ts = T0 + 4_000_000_000;
    for (delay, ok) in [(0u64, true), (2, true), (3, false), (450, false)] {
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
        assert_eq!(v.summary.max_frames_checked, want, "move {move_ms}");
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
    assert_eq!(v.summary.max_frames_checked, 1);
    assert!(v.b.passed(), "{:?}", v.b);
}

/// REQ-LAT-001 check b: F0 is taken when REQ-LAT-004 lets the bat move
/// (timestamp + check-a bound + coil ticks), so a frame presented within
/// that window of the press does not count against the renderer; the raw
/// column (F0 at the timestamp) is reported, not checked.
#[test]
fn req_lat_001_check_b_counts_from_the_coil_deadline() {
    // Frame 10 presents 0.5 ms after the press; the bat moves 1 ms after.
    let ts = T0 + 1_000_000_000 - 500_000;
    let mut r = press(0, ts, 800_000);
    r.move_tick = Some(1001);
    r.apply_tick = 1000;
    let v = evaluate(&joined(vec![r], frames(600)), &OK_SHUTDOWN, &params());
    assert!(v.b.passed(), "{}", v.report());
    assert_eq!(v.summary.max_frames_checked, 2);
    assert_eq!(v.summary.max_frames_raw, 3);
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
    fw.presented(&[SurfaceStatus::Presented], 1);
    let text = c.collect().to_csv();
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
            frames: 512
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
    assert!(report.contains("verdict: FAIL (checks b, d)"), "{report}");
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
    }
    let t = joined(vec![press(0, T0 + 5_000_000_000, 500_000)], frames(4));
    let (c, ..) = latency_trace(bindings(), now);
    let mut text = c.collect().to_csv();
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
}

/// Runs `s` like the sim thread does (accumulator clock, catch-up cap,
/// dropped ticks skipped), with the render thread's frames interleaved.
/// Returns the trace text, the executed ticks and the events' (raw stamp,
/// arrival time).
fn run_scenario(s: &Scenario) -> (String, Vec<Executed>, Vec<(u64, u64)>) {
    let (mut tx, rx) = channel::<InputEvent>(256);
    let (w, _r) = TripleBuffer::new(FrameStateSnapshot::default());
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
            fw.presented(&[SurfaceStatus::Presented], l.tick());
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
            });
        }
    }
    let text = c.collect().to_csv();
    (
        text,
        executed,
        evs.iter().map(|(e, a)| (e.timestamp_ns, *a)).collect(),
    )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// REQ-LAT-004, REQ-LAT-001: on a controlled clock with stalls and
    /// dropped ticks, every row's applying tick ends at or after its stamp
    /// and is the first executed tick that had received it (drained at or
    /// after its arrival) and whose end is not before its stamp;
    /// `frames_to_photon` equals `frame - F0`.
    #[test]
    fn req_lat_004_first_executed_tick_and_req_lat_001_frames(s in scenario()) {
        let (text, executed, evs) = run_scenario(&s);
        let t = parse_trace(&text).unwrap();
        prop_assert_eq!(t.rows.len(), evs.len());
        prop_assert_eq!(t.overflows, 0);
        for (r, (ts, arrival)) in t.rows.iter().zip(&evs) {
            prop_assert_eq!(r.sdl_ts_ns, *ts);
            prop_assert!(r.apply_tick_end_sdl_ns >= r.sdl_ts_ns);
            let first = executed
                .iter()
                .find(|e| e.drain >= *arrival && e.end >= *ts)
                .expect("an executed tick applies it");
            prop_assert_eq!(r.apply_tick, first.tick, "row {:?}", r);
            prop_assert_eq!(r.apply_tick_end_sdl_ns, first.end);
            prop_assert_eq!(r.drain_sdl_ns, first.drain);
            if let (Some(f), Some(n)) = (r.frame, r.frames_to_photon) {
                let f0 = verdict::last_frame_at_or_before(&t.frames, r.sdl_ts_ns);
                prop_assert_eq!(n, f - f0);
                let m = r.move_tick.unwrap();
                let first_frame = t.frames.iter().find(|fr| fr.snap_tick >= m).unwrap();
                prop_assert_eq!(f, first_frame.frame);
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
    let (_, n) = guarded(|| Box::new(1u8));
    assert!(n >= 1, "allocator guard not armed");
    let (mut tx, rx) = channel::<InputEvent>(4096);
    let (w, _r) = TripleBuffer::new(FrameStateSnapshot::default());
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
                fw.presented(&[SurfaceStatus::Presented], l.tick());
            }
            if k % 300 == 0 {
                l.skip_ticks(5);
            }
        }
        l.run_ticks(8);
        fw.presented(
            &[SurfaceStatus::Skipped, SurfaceStatus::Presented],
            l.tick(),
        );
    });
    assert_eq!(allocs, 0, "the traced tick and frame paths allocated");
    let t = c.collect();
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
        let rows = collector.map_or(0, |c| c.collect().rows.len());
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
// The 60 s windowed run.

/// REQ-LAT-001, REQ-LAT-002, REQ-LAT-003, REQ-LAT-004 end to end: the
/// release app with the grey-box table under Xvfb on lavapipe, flipper
/// presses through XTest for 60 s, checks a to d on its trace and shutdown
/// line. Artifacts (trace, stderr, screenshots, summary) go to
/// `KP_WINDOWED_OUT` (default `<target>/windowed`).
#[test]
#[ignore = "60 s windowed run: needs Xvfb, lavapipe and a release build (windowed.yml, scripts/windowed/run.sh)"]
fn windowed_latency_60s() {
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
        "LP_NUM_THREADS={} seed={} run_ms={} app exit {}\nscreenshots: {:?}\n{report}",
        cfg.lp_num_threads, cfg.seed, cfg.run_ms, out.exit, out.screenshots
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
