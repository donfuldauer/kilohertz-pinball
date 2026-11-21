//! Unit tests of the profiler: targets at the exact boundaries, clock-domain
//! clamping, writers and reader, the step wrapper and the HUD text.

use super::*;
use pin_core::{CabinetAxis, TICK_NS};
use proptest::prelude::*;

#[test]
fn tick_target_is_strict_at_350_us() {
    assert!(tick_target_met(349_999));
    assert!(!tick_target_met(350_000), "0.35 ms fails the strict target");
    assert!(!tick_target_met(350_001));
    assert_eq!(TICK_P99_9_TARGET_NS, 350_000);
}

#[test]
fn input_target_is_inclusive_at_1_5_ms() {
    assert!(input_target_met(1_499_999));
    assert!(input_target_met(1_500_000), "1.5 ms passes the <= target");
    assert!(!input_target_met(1_500_001));
    assert_eq!(INPUT_P99_TARGET_NS, 1_500_000);
}

#[test]
fn snapshot_verdicts_at_the_boundaries() {
    // Through the histogram: a single sample is every percentile, and
    // values at the boundaries keep their exact verdict (the reported value
    // is the bucket's upper edge clamped to the exact max).
    for (tick, want) in [(349_999u64, true), (350_000, false)] {
        let mut p = Profiler::new();
        p.sim.record_tick_ns(tick);
        let s = p.reader.snapshot();
        assert_eq!(s.tick.p99_9, tick);
        assert_eq!(s.tick_target_met(), Some(want), "{tick}");
    }
    for (lat, want) in [(1_500_000u64, true), (1_500_001, false)] {
        let mut p = Profiler::new();
        p.sim.record_input(0, lat);
        let s = p.reader.snapshot();
        assert_eq!(s.input.p99, lat);
        assert_eq!(s.input_target_met(), Some(want), "{lat}");
    }
    let s = Profiler::default().reader.snapshot();
    assert_eq!(s.tick_target_met(), None);
    assert_eq!(s.input_target_met(), None);
}

#[test]
fn p99_9_uses_the_nearest_rank() {
    // 999 fast ticks and one slow: rank ⌈0.999 · 1000⌉ = 999 is fast.
    let mut p = Profiler::new();
    for _ in 0..999 {
        p.sim.record_tick_ns(100_000);
    }
    p.sim.record_tick_ns(5_000_000);
    let s = p.reader.snapshot();
    assert!(s.tick.p99_9 < 101_000, "{}", s.tick.p99_9);
    assert_eq!(s.tick_target_met(), Some(true));
    // Two slow ticks in 1000: rank 999 is slow.
    p.sim.record_tick_ns(5_000_000);
    let s = p.reader.snapshot();
    assert_eq!(s.tick.count, 1001);
    assert_eq!(s.tick_target_met(), Some(false));
}

#[test]
fn future_timestamps_clamp_to_zero_and_are_counted() {
    assert_eq!(
        input_latency(10, 1_000),
        InputLatency {
            ns: 990,
            clamped: false
        }
    );
    assert_eq!(
        input_latency(1_000, 1_000),
        InputLatency {
            ns: 0,
            clamped: false
        }
    );
    assert_eq!(
        input_latency(1_001, 1_000),
        InputLatency {
            ns: 0,
            clamped: true
        }
    );
    let mut p = Profiler::new();
    assert!(p.sim.record_input(u64::MAX, 0).clamped);
    assert!(!p.sim.record_input(0, u64::MAX).clamped);
    let s = p.reader.snapshot();
    assert_eq!(s.input_clamped, 1);
    assert_eq!(s.input.count, 2);
    assert_eq!(s.input.min, 0, "the clamped sample is recorded as 0");
    assert_eq!(s.input.saturated, 1);
}

#[test]
fn only_coil_buttons_count_for_input_latency() {
    use CabinetInput as C;
    let coil = [
        C::LeftFlipper,
        C::RightFlipper,
        C::LeftUpper,
        C::RightUpper,
        C::Launch,
    ];
    for c in CabinetInput::ALL {
        assert_eq!(drives_coil(c), coil.contains(&c), "{c:?}");
    }
    let mut p = Profiler::new();
    let evs = [
        InputEvent::button(100, C::LeftFlipper, true),
        InputEvent::button(200, C::Start, true),
        InputEvent::axis(300, CabinetAxis::Plunger, 0.5),
        InputEvent::button(400, C::RightFlipper, false),
        InputEvent::button(5_000, C::Launch, true),
    ];
    p.sim.record_inputs(&evs, 1_000);
    let s = p.reader.snapshot();
    assert_eq!(s.input.count, 3);
    assert_eq!(s.input.max, 900);
    assert_eq!(s.input_clamped, 1);
}

#[test]
fn overflow_counters_are_monotonic_and_summed() {
    let mut p = Profiler::new();
    p.sim.set_script_queue_overflows(3);
    p.sim.set_script_queue_overflows(2);
    p.sim_loop.set_input_overflow_ticks(5);
    p.sim_loop.set_input_overflow_ticks(4);
    let s = p.reader.clone().snapshot();
    assert_eq!(s.script_queue_overflows, 3);
    assert_eq!(s.input_overflow_ticks, 5);
    assert_eq!(s.command_queue_overflows(), 8);
    let big = ProfilerSnapshot {
        script_queue_overflows: u64::MAX,
        input_overflow_ticks: 1,
        ..ProfilerSnapshot::default()
    };
    assert_eq!(big.command_queue_overflows(), u64::MAX);
}

#[test]
fn gpu_frame_time() {
    let mut p = Profiler::new();
    for ns in [6_000_000u64, 7_000_000, 16_000_000] {
        p.present.record_frame(1_000, 1_000 + ns);
    }
    p.present.record_frame(10, 5);
    let s = p.reader.snapshot();
    assert_eq!(s.gpu_frame.count, 4);
    assert_eq!(s.gpu_frame.min, 0);
    assert_eq!(s.gpu_frame.max, 16_000_000);
    assert!(s.gpu_frame.p50 >= 6_000_000 && s.gpu_frame.p50 < 6_100_000);
}

#[test]
fn readers_see_writers_on_other_threads() {
    let Profiler {
        mut sim,
        reader,
        mut present,
        ..
    } = Profiler::new();
    let t = std::thread::spawn(move || {
        for i in 0..10_000u64 {
            sim.record_tick_ns(i);
        }
    });
    let g = std::thread::spawn(move || {
        for _ in 0..100 {
            present.record_frame_ns(8_000_000);
        }
    });
    // Reads while writing never fail and stay ordered.
    for _ in 0..20 {
        let s = reader.snapshot();
        assert!(s.tick.p50 <= s.tick.p99 && s.tick.p99 <= s.tick.max);
    }
    t.join().unwrap();
    g.join().unwrap();
    let s = reader.snapshot();
    assert_eq!(s.tick.count, 10_000);
    assert_eq!(s.gpu_frame.count, 100);
}

/// A step that writes a deterministic snapshot and counts overflows.
struct Counter(u64);
impl SimStep for Counter {
    fn step_1ms(&mut self, tick: u64, inputs: &[InputEvent], out: &mut FrameStateSnapshot) {
        self.0 += inputs.len() as u64;
        out.tick = tick;
        out.lamps[0] = self.0 as f32;
    }
}
impl CommandQueueOverflows for Counter {
    fn command_queue_overflows(&self) -> u64 {
        self.0
    }
}

#[test]
fn profiled_sim_forwards_untouched_and_records() {
    let p = Profiler::new();
    // A simulated clock: 40 µs per read.
    let mut now = 1_000_000u64;
    let clock = move || {
        now += 40_000;
        now
    };
    let mut wrapped = ProfiledSim::new(Counter(0), p.sim, clock);
    let mut plain = Counter(0);
    let (mut a, mut b) = (FrameStateSnapshot::default(), FrameStateSnapshot::default());
    for tick in 1..=50u64 {
        let evs = [InputEvent::button(
            tick * 10_000,
            CabinetInput::LeftFlipper,
            tick % 2 == 0,
        )];
        wrapped.step_1ms(tick, &evs, &mut a);
        plain.step_1ms(tick, &evs, &mut b);
        assert_eq!(a, b, "tick {tick}");
    }
    assert_eq!(wrapped.inner().0, 50);
    wrapped.inner_mut().0 += 1;
    let s = p.reader.snapshot();
    assert_eq!(s.tick.count, 50);
    assert_eq!((s.tick.min, s.tick.max), (40_000, 40_000));
    assert_eq!(s.input.count, 50);
    assert_eq!(s.script_queue_overflows, 50);
    let (inner, _writer) = wrapped.into_parts();
    assert_eq!(inner.0, 51);
}

#[test]
fn queue_overflow_sources() {
    assert_eq!(IdleSim.command_queue_overflows(), 0);
}

#[test]
fn format_ms_truncates_to_microseconds() {
    assert_eq!(format_ms(0), "0.000");
    assert_eq!(format_ms(349_999), "0.349");
    assert_eq!(format_ms(350_000), "0.350");
    assert_eq!(format_ms(1_234_567), "1.234");
    assert_eq!(format_ms(16_000_000), "16.000");
}

#[test]
fn hud_lines_without_data() {
    let l = hud_lines(&ProfilerSnapshot::default());
    assert_eq!(
        l,
        [
            "tick p99.9 -- ms (target < 0.350 ms) no data | mean -- max -- n 0",
            "input p99 -- ms (target <= 1.500 ms) no data | p50 -- max -- n 0 clamped 0",
            "gpu frame p50 -- p99 -- max -- ms | n 0",
            "cmd queue overflows 0 (script 0, input ticks 0)",
        ]
    );
}

#[test]
fn hud_lines_with_data() {
    let mut p = Profiler::new();
    p.sim.record_tick_ns(120_000);
    p.sim.record_input(0, 1_600_000);
    p.sim.record_input(10, 0);
    p.present.record_frame_ns(8_000_000);
    p.sim.set_script_queue_overflows(2);
    p.sim_loop.set_input_overflow_ticks(1);
    let l = hud_lines(&p.reader.snapshot());
    assert_eq!(
        l[0],
        "tick p99.9 0.120 ms (target < 0.350 ms) OK | mean 0.120 max 0.120 n 1"
    );
    assert_eq!(
        l[1],
        "input p99 1.600 ms (target <= 1.500 ms) OVER | p50 0.000 max 1.600 n 2 clamped 1"
    );
    assert_eq!(l[2], "gpu frame p50 8.000 p99 8.000 max 8.000 ms | n 1");
    assert_eq!(l[3], "cmd queue overflows 3 (script 2, input ticks 1)");
    let mut p = Profiler::new();
    p.sim.record_tick_ns(350_000);
    assert!(hud_lines(&p.reader.snapshot())[0].contains("OVER"));
}

proptest! {
    #[test]
    fn input_latency_never_negative(ts in any::<u64>(), start in any::<u64>()) {
        let l = input_latency(ts, start);
        prop_assert_eq!(l.clamped, ts > start);
        prop_assert_eq!(l.ns, start.saturating_sub(ts));
    }

    #[test]
    fn targets_match_their_comparisons(ns in any::<u64>()) {
        prop_assert_eq!(tick_target_met(ns), ns < 350_000);
        prop_assert_eq!(input_target_met(ns), ns <= 1_500_000);
    }

    #[test]
    fn format_ms_round_trips_to_microseconds(ns in any::<u64>()) {
        let s = format_ms(ns);
        let (a, b) = s.split_once('.').unwrap();
        prop_assert_eq!(b.len(), 3);
        let us = a.parse::<u64>().unwrap() * 1000 + b.parse::<u64>().unwrap();
        prop_assert_eq!(us, ns / 1000);
    }

    #[test]
    fn snapshot_tracks_every_sample(v in prop::collection::vec(0u64..10_000_000, 1..200)) {
        let mut p = Profiler::new();
        for &x in &v {
            p.sim.record_tick_ns(x);
            p.sim.record_input(0, x);
            p.present.record_frame_ns(x);
        }
        let s = p.reader.snapshot();
        let max = *v.iter().max().unwrap();
        for h in [s.tick, s.input, s.gpu_frame] {
            prop_assert_eq!(h.count, v.len() as u64);
            prop_assert_eq!(h.max, max);
        }
        prop_assert_eq!(s.tick, s.input);
        prop_assert_eq!(hud_lines(&s).len(), 4);
    }
}

/// REQ-LAT-005 (c): behind a sim loop with an epoch the loop records the
/// latency from the raw stamp to the clock at the tick start, and the
/// tick-time-only wrapper records tick time alone, so the latency equals
/// SDL now − raw stamp and an event stamped within the delivering tick is
/// never clamped; one sample per event (no double recording).
#[test]
fn req_lat_005_loop_measures_now_minus_raw_stamp() {
    use crate::orchestrator::SimLoop;
    use pin_core::TripleBuffer;
    for epoch in [0u64, 1_000_000_000, 10_000_000_000, u64::MAX / 2] {
        let p = Profiler::new();
        let mut lp = p.sim_loop;
        let now = Arc::new(AtomicU64::new(0));
        let clk = Arc::clone(&now);
        let sim = ProfiledSim::new(Counter(0), p.sim, move || clk.load(Ordering::Relaxed))
            .tick_time_only();
        assert!(!sim.records_inputs());
        let (mut tx, rx) = pin_core::spsc::channel::<InputEvent>(4);
        let (w, _r) = TripleBuffer::new(FrameStateSnapshot::default());
        let mut l = SimLoop::new(sim, rx, w);
        l.set_epoch_ns(epoch);
        l.run_ticks_profiled(1, &mut lp, || now.load(Ordering::Relaxed));
        let mut want_max = 0;
        for tick in 2..=201u64 {
            // Tick `tick` (raw end `epoch + tick` ms) starts after the end
            // of the previous tick plus a wake-up below 1 ms; the press is
            // stamped within the previous millisecond, so the loop delivers
            // it in this tick.
            let start_raw = epoch + (tick - 1) * TICK_NS + (tick * 7_919) % TICK_NS;
            let raw = epoch + (tick - 1) * TICK_NS - (tick * 104_729) % TICK_NS;
            now.store(start_raw, Ordering::Relaxed);
            tx.push(InputEvent::button(raw, CabinetInput::RightFlipper, true))
                .unwrap();
            l.run_ticks_profiled(1, &mut lp, || now.load(Ordering::Relaxed));
            want_max = want_max.max(start_raw - raw);
        }
        let s = p.reader.snapshot();
        assert_eq!(l.inputs_delivered(), 200, "epoch {epoch}");
        assert_eq!(s.input.count, 200, "epoch {epoch}");
        assert_eq!(s.input_clamped, 0, "epoch {epoch}");
        assert_eq!(s.input.max, want_max, "SDL now − raw stamp, epoch {epoch}");
        // Tick time: both readings of a tick see the same clock value.
        assert_eq!((s.tick.count, s.tick.max), (201, 0));
    }
}

/// A bare `ProfiledSim::new` (no loop epoch, so the stamps it receives are
/// raw) records the input latency itself on the raw clock.
#[test]
fn profiled_sim_without_epoch_uses_the_raw_clock() {
    let p = Profiler::new();
    let mut sim = ProfiledSim::new(Counter(0), p.sim, || 5_000_000);
    assert!(sim.records_inputs());
    let mut out = FrameStateSnapshot::default();
    sim.step_1ms(
        1,
        &[InputEvent::button(
            4_000_000,
            CabinetInput::LeftFlipper,
            true,
        )],
        &mut out,
    );
    let s = p.reader.snapshot();
    assert_eq!(
        (s.input.count, s.input.max, s.input_clamped),
        (1, 1_000_000, 0)
    );
}

/// `LoopProfiler::record_input(s)` measures like `SimProfiler`'s: only
/// coil-driving buttons, future stamps clamped and counted.
#[test]
fn loop_profiler_records_input_latency() {
    let mut p = Profiler::new();
    let evs = [
        InputEvent::button(400, CabinetInput::LeftFlipper, true),
        InputEvent::button(100, CabinetInput::Start, true),
        InputEvent::button(1_500, CabinetInput::Launch, true),
    ];
    p.sim_loop.record_inputs(&evs, 1_000);
    let l = p.sim_loop.record_input(2_000, 1_000);
    assert_eq!(
        l,
        InputLatency {
            ns: 0,
            clamped: true
        }
    );
    let s = p.reader.snapshot();
    assert_eq!((s.input.count, s.input.max, s.input_clamped), (3, 600, 2));
}

/// REQ-LAT-003 (e): the rate lines, exact text, with and without data; the
/// four profiler lines stay first and unchanged.
#[test]
fn req_lat_003_hud_shows_presented_fps_and_dropped_ticks() {
    assert_eq!(
        hud_rate_lines(&HudRates::default()),
        ["presented fps --", "dropped ticks 0"]
    );
    let r = HudRates {
        dropped_ticks: 12,
        presented_fps_milli: Some(59_940),
    };
    assert_eq!(
        hud_rate_lines(&r),
        ["presented fps 59.9", "dropped ticks 12"]
    );
    let snap = ProfilerSnapshot::default();
    let all = hud_lines_with_rates(&snap, &r);
    assert_eq!(all.len(), 6);
    assert_eq!(all[..4], hud_lines(&snap)[..]);
    assert_eq!(all[4], "presented fps 59.9");
    assert_eq!(all[5], "dropped ticks 12");
    assert_eq!(format_fps(0), "0.0");
    assert_eq!(format_fps(144_000), "144.0");
    assert_eq!(format_fps(999), "0.9");
}

/// The fps meter: rate between readings in thousandths; none on the first
/// reading, with no elapsed time or with a counter or clock going back.
#[test]
fn frame_rate_meter_measures_between_readings() {
    let mut m = FrameRateMeter::new();
    assert_eq!(m.sample(1_000, 10), None, "first reading");
    // 15 frames in 250 ms = 60 fps.
    assert_eq!(m.sample(1_000 + 250_000_000, 25), Some(60_000));
    // 1 frame in 3 s = 0.333 fps.
    assert_eq!(m.sample(1_000 + 3_250_000_000, 26), Some(333));
    assert_eq!(m.sample(1_000 + 3_250_000_000, 27), None, "no time passed");
    assert_eq!(
        m.sample(1_000 + 4_000_000_000, 5),
        None,
        "counter went back"
    );
    assert_eq!(m.sample(10, 6), None, "clock went back");
    assert_eq!(m.sample(10 + 1, u64::MAX), Some(u64::MAX), "saturates");
    assert_eq!(FrameRateMeter::default(), FrameRateMeter::new());
}

proptest! {
    /// Rate lines: fps rounds down to one decimal of the thousandths value.
    #[test]
    fn fps_line_matches_the_value(milli in any::<u64>(), dropped in any::<u64>()) {
        let [fps, drop] = hud_rate_lines(&HudRates {
            dropped_ticks: dropped,
            presented_fps_milli: Some(milli),
        });
        prop_assert_eq!(drop, format!("dropped ticks {dropped}"));
        let v = fps.strip_prefix("presented fps ").unwrap();
        let (a, b) = v.split_once('.').unwrap();
        prop_assert_eq!(b.len(), 1);
        prop_assert_eq!(a.parse::<u64>().unwrap() * 10 + b.parse::<u64>().unwrap(), milli / 100);
    }
}
