//! 7.1 independent tests (black box, from the brief and the public rustdoc
//! of `pin_app::profiler`): recorder accuracy, target boundaries, input
//! latency, overflow counters, GPU frame time, HUD text and concurrency.
//! Simulated values only: no sleeps, no wall-clock assertions.

use pin_app::profiler::histogram::{BUCKETS, HistogramSummary, MAX_TRACKED_NS, nearest_rank};
use pin_app::profiler::{
    INPUT_P99_TARGET_NS, LatencyHistogram, Profiler, ProfilerSnapshot, RELATIVE_ERROR_BOUND,
    TICK_P99_9_TARGET_NS, drives_coil, format_ms, hud_lines, input_latency, input_target_met,
    tick_target_met,
};
use pin_core::{CabinetAxis, CabinetInput, InputEvent};
use proptest::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};

const PPMS: [u32; 3] = [500_000, 990_000, 999_000];

/// Exact nearest-rank percentile of `sorted` (rank max(1, ceil(ppm n / 1e6))).
fn exact(sorted: &[u64], ppm: u32) -> u64 {
    let n = sorted.len() as u128;
    let rank = ((u128::from(ppm) * n).div_ceil(1_000_000)).max(1) as usize;
    sorted[rank - 1]
}

fn hist_of(samples: &[u64]) -> LatencyHistogram {
    let h = LatencyHistogram::new();
    for &s in samples {
        h.record(s);
    }
    h
}

/// `reported` is within the documented error bound above `x` (never below).
fn within_bound(x: u64, reported: u64) -> bool {
    let slack = (x as f64 * RELATIVE_ERROR_BOUND).floor() as u64;
    reported >= x && reported <= x + slack
}

fn check_accuracy(samples: &[u64]) {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let h = hist_of(samples);
    let s = h.summary();
    assert_eq!(s.count, samples.len() as u64);
    assert_eq!(h.count(), samples.len() as u64);
    assert!(s.has_samples());
    assert_eq!(s.saturated, 0);
    let true_max = *sorted.last().unwrap();
    assert!(within_bound(true_max, s.max), "max {} vs {true_max}", s.max);
    assert_eq!(s.min, sorted[0], "min is exact");
    let sum: u128 = sorted.iter().map(|&v| u128::from(v)).sum();
    assert_eq!(u128::from(s.mean), sum / sorted.len() as u128, "mean");
    for (ppm, got) in PPMS.iter().zip([s.p50, s.p99, s.p99_9]) {
        let x = exact(&sorted, *ppm);
        assert!(within_bound(x, got), "ppm {ppm}: got {got}, exact {x}");
        assert_eq!(h.value_at_ppm(*ppm), Some(got), "value_at_ppm {ppm}");
        assert!(got <= s.max, "percentile above the maximum");
    }
    assert!(s.p50 <= s.p99 && s.p99 <= s.p99_9 && s.p99_9 <= s.max);
    // The top quantile is the maximum.
    assert_eq!(h.value_at_ppm(1_000_000), Some(s.max));
}

#[derive(Clone, Debug)]
enum Shape {
    Random(Vec<u64>),
    AllEqual(u64, usize),
    Bimodal(u64, u64, Vec<bool>),
    Ascending(Vec<u64>),
    Descending(Vec<u64>),
}

const TEN_S: u64 = 10_000_000_000;

fn shape() -> impl Strategy<Value = Shape> {
    let v = || 1u64..=TEN_S;
    prop_oneof![
        prop::collection::vec(v(), 1..20_000).prop_map(Shape::Random),
        (v(), 1usize..20_000).prop_map(|(a, n)| Shape::AllEqual(a, n)),
        (v(), v(), prop::collection::vec(any::<bool>(), 1..20_000))
            .prop_map(|(a, b, m)| Shape::Bimodal(a, b, m)),
        prop::collection::vec(v(), 1..20_000).prop_map(Shape::Ascending),
        prop::collection::vec(v(), 1..20_000).prop_map(Shape::Descending),
    ]
}

fn expand(s: Shape) -> Vec<u64> {
    match s {
        Shape::Random(v) => v,
        Shape::AllEqual(a, n) => vec![a; n],
        Shape::Bimodal(a, b, m) => m.into_iter().map(|x| if x { a } else { b }).collect(),
        Shape::Ascending(mut v) => {
            v.sort_unstable();
            v
        }
        Shape::Descending(mut v) => {
            v.sort_unstable_by(|a, b| b.cmp(a));
            v
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 48, ..ProptestConfig::default() })]

    #[test]
    fn percentiles_stay_within_the_documented_bound(s in shape()) {
        check_accuracy(&expand(s));
    }

    #[test]
    fn small_sets_with_full_range_values(
        v in prop::collection::vec(1u64..=TEN_S, 1..64)
    ) {
        check_accuracy(&v);
    }

    #[test]
    fn sample_order_does_not_change_the_result(
        v in prop::collection::vec(1u64..=TEN_S, 1..2000),
        seed in any::<u64>(),
    ) {
        let a = hist_of(&v).summary();
        let mut w = v.clone();
        w.reverse();
        let b = hist_of(&w).summary();
        // A deterministic shuffle.
        let mut x = seed | 1;
        for i in (1..w.len()).rev() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            w.swap(i, (x % (i as u64 + 1)) as usize);
        }
        let c = hist_of(&w).summary();
        prop_assert_eq!(a, b);
        prop_assert_eq!(a, c);
    }

    #[test]
    fn nearest_rank_matches_the_documented_formula(n in 0u64..1_000_000, ppm in 0u32..=1_200_000) {
        let r = nearest_rank(n, ppm);
        if n == 0 {
            prop_assert_eq!(r, 0);
        } else {
            let q = u128::from(ppm.min(1_000_000));
            let want = (q * u128::from(n)).div_ceil(1_000_000).max(1) as u64;
            prop_assert_eq!(r, want);
            prop_assert!(r >= 1 && r <= n);
        }
    }

    #[test]
    fn format_ms_is_milliseconds_rounded_down_to_us(ns in any::<u64>()) {
        let want = format!("{}.{:03}", ns / 1_000_000, (ns % 1_000_000) / 1000);
        prop_assert_eq!(format_ms(ns), want);
    }

    #[test]
    fn input_latency_is_the_clamped_difference(ts in any::<u64>(), start in any::<u64>()) {
        let l = input_latency(ts, start);
        if ts > start {
            prop_assert_eq!(l.ns, 0);
            prop_assert!(l.clamped);
        } else {
            prop_assert_eq!(l.ns, start - ts);
            prop_assert!(!l.clamped);
        }
    }
}

#[test]
fn documented_constants_and_format_examples() {
    assert_eq!(TICK_P99_9_TARGET_NS, 350_000);
    assert_eq!(INPUT_P99_TARGET_NS, 1_500_000);
    const { assert!(RELATIVE_ERROR_BOUND > 0.0 && RELATIVE_ERROR_BOUND < 0.01) };
    assert_eq!(format_ms(350_000), "0.350");
    assert_eq!(format_ms(1_234_567), "1.234");
    assert_eq!(format_ms(0), "0.000");
    const { assert!(BUCKETS > 0) };
}

#[test]
fn empty_recorder_gives_zeros_without_panicking() {
    let h = LatencyHistogram::new();
    assert_eq!(h.count(), 0);
    assert_eq!(h.value_at_ppm(500_000), None);
    let s = h.summary();
    assert!(!s.has_samples());
    assert_eq!(
        (
            s.count,
            s.min,
            s.max,
            s.mean,
            s.p50,
            s.p99,
            s.p99_9,
            s.saturated
        ),
        (0, 0, 0, 0, 0, 0, 0, 0)
    );
    let snap = Profiler::new().reader.snapshot();
    assert_eq!(snap.tick.count, 0);
    assert_eq!(snap.tick_target_met(), None);
    assert_eq!(snap.input_target_met(), None);
    assert_eq!(snap.command_queue_overflows(), 0);
}

#[test]
fn single_sample_and_adversarial_shapes_are_exact_enough() {
    check_accuracy(&[1]);
    check_accuracy(&[TEN_S]);
    check_accuracy(&vec![123_456; 5000]);
    let mut bi: Vec<u64> = vec![10_000; 990];
    bi.extend(vec![9_000_000; 10]);
    check_accuracy(&bi);
    check_accuracy(&(1..=20_000).collect::<Vec<u64>>());
    check_accuracy(&(1..=20_000).rev().map(|v| v * 1000).collect::<Vec<u64>>());
}

#[test]
fn values_below_256_ns_are_exact() {
    for v in 1..256u64 {
        let s = hist_of(&[v, v, v]).summary();
        assert_eq!((s.p50, s.p99, s.p99_9, s.max), (v, v, v, v));
    }
}

#[test]
fn oversized_samples_saturate_instead_of_panicking() {
    let h = LatencyHistogram::new();
    for _ in 0..10 {
        h.record(1_000);
    }
    h.record(MAX_TRACKED_NS + 1);
    h.record(u64::MAX);
    let s = h.summary();
    assert_eq!(s.count, 12);
    assert_eq!(s.saturated, 2);
    assert_eq!(s.max, u64::MAX, "the exact maximum is kept");
    assert!(s.p99_9 >= 1_000 && s.p99_9 <= s.max);
    assert!(s.p50 <= s.p99 && s.p99 <= s.p99_9);
    // A snapshot of a profiler fed with such values and its HUD do not panic.
    let mut p = Profiler::new();
    p.sim.record_tick_ns(u64::MAX);
    p.present.record_frame_ns(u64::MAX);
    p.sim.record_input(0, u64::MAX);
    let _ = hud_lines(&p.reader.snapshot());
}

// ---- requirement 2: target boundaries ----

#[test]
fn tick_p99_9_boundary_is_strict() {
    for (v, pass) in [
        (350_000u64, false),
        (349_999, true),
        (350_001, false),
        (1, true),
    ] {
        let mut p = Profiler::new();
        for _ in 0..2000 {
            p.sim.record_tick_ns(v);
        }
        let s = p.reader.snapshot();
        assert_eq!(s.tick.p99_9, v, "all-equal samples are reported exactly");
        assert_eq!(s.tick_target_met(), Some(pass), "tick {v}");
        assert_eq!(tick_target_met(v), pass, "tick_target_met({v})");
    }
}

#[test]
fn input_p99_boundary_is_inclusive() {
    for (v, pass) in [
        (1_500_000u64, true),
        (1_499_999, true),
        (1_500_001, false),
        (0, true),
    ] {
        let mut p = Profiler::new();
        for _ in 0..2000 {
            p.sim.record_input(1_000, 1_000 + v);
        }
        let s = p.reader.snapshot();
        assert_eq!(s.input.p99, v);
        assert_eq!(s.input_target_met(), Some(pass), "input {v}");
        assert_eq!(input_target_met(v), pass, "input_target_met({v})");
    }
}

#[test]
fn targets_far_from_the_edge_hold_for_mixed_sets() {
    // 1000 samples: 999 fast and one slow outlier. The p99.9 rank is 999, so
    // a single outlier does not fail the tick target; two do.
    let mut p = Profiler::new();
    for _ in 0..999 {
        p.sim.record_tick_ns(100_000);
    }
    p.sim.record_tick_ns(900_000);
    assert_eq!(p.reader.snapshot().tick_target_met(), Some(true));
    p.sim.record_tick_ns(900_000);
    p.sim.record_tick_ns(900_000);
    assert_eq!(p.reader.snapshot().tick_target_met(), Some(false));
    // Input: 1.5% of samples at 5 ms fail the p99 target; 0.5% pass.
    let mut p = Profiler::new();
    for i in 0..2000u64 {
        let lat = if i % 200 == 0 { 5_000_000 } else { 400_000 };
        p.sim.record_input(0, lat);
    }
    assert_eq!(p.reader.snapshot().input_target_met(), Some(true));
    let mut p = Profiler::new();
    for i in 0..2000u64 {
        let lat = if i % 50 == 0 { 5_000_000 } else { 400_000 };
        p.sim.record_input(0, lat);
    }
    assert_eq!(p.reader.snapshot().input_target_met(), Some(false));
}

// ---- requirement 3: input latency ----

#[test]
fn synthetic_event_and_tick_pairs_give_the_expected_distribution() {
    let mut p = Profiler::new();
    // Event i is stamped i * 7_000 ns; the applying tick starts 100_000 +
    // (i % 10) * 50_000 ns later.
    let mut lats = Vec::new();
    for i in 0..1000u64 {
        let ts = 10_000_000 + i * 7_000;
        let lat = 100_000 + (i % 10) * 50_000;
        let m = p.sim.record_input(ts, ts + lat);
        assert_eq!((m.ns, m.clamped), (lat, false));
        lats.push(lat);
    }
    lats.sort_unstable();
    let s = p.reader.snapshot();
    assert_eq!(s.input.count, 1000);
    assert_eq!(s.input_clamped, 0);
    assert!(within_bound(exact(&lats, 500_000), s.input.p50));
    assert!(within_bound(exact(&lats, 990_000), s.input.p99));
    assert_eq!(s.input.min, 100_000);
    assert!(within_bound(550_000, s.input.max));
}

#[test]
fn future_timestamps_clamp_to_zero_and_are_counted() {
    let mut p = Profiler::new();
    let mut want_clamped = 0u64;
    for i in 0..500u64 {
        let start = 1_000_000 + i * 1_000;
        // Every third event is stamped after its tick start.
        let ts = if i % 3 == 0 {
            want_clamped += 1;
            start + 1 + i
        } else {
            start - 500
        };
        let m = p.sim.record_input(ts, start);
        assert_eq!(m.clamped, i % 3 == 0);
        assert_eq!(m.ns, if i % 3 == 0 { 0 } else { 500 });
    }
    // Extreme values never panic.
    assert!(p.sim.record_input(u64::MAX, 0).clamped);
    want_clamped += 1;
    assert_eq!(p.sim.record_input(0, 0).ns, 0);
    let s = p.reader.snapshot();
    assert_eq!(s.input_clamped, want_clamped);
    assert_eq!(s.input.count, 502, "a clamped sample is still recorded");
    assert_eq!(s.input.min, 0);
    assert!(
        s.input.max <= 500 + 1,
        "no clamped sample leaks a huge latency"
    );
}

#[test]
fn record_inputs_measures_only_coil_driving_buttons() {
    let mut p = Profiler::new();
    let mut evs = Vec::new();
    let mut want = 0u64;
    for c in CabinetInput::ALL {
        for pressed in [true, false] {
            evs.push(InputEvent::button(9_000_000, c, pressed));
            if drives_coil(c) {
                want += 1;
            }
        }
    }
    evs.push(InputEvent::axis(9_000_000, CabinetAxis::Plunger, 0.5));
    for c in [
        CabinetInput::LeftFlipper,
        CabinetInput::RightFlipper,
        CabinetInput::Launch,
    ] {
        assert!(drives_coil(c), "{c:?} drives a coil");
    }
    for c in [
        CabinetInput::Action,
        CabinetInput::Start,
        CabinetInput::Tilt,
    ] {
        assert!(!drives_coil(c), "{c:?} does not");
    }
    p.sim.record_inputs(&evs, 9_800_000);
    let s = p.reader.snapshot();
    assert_eq!(s.input.count, want);
    assert_eq!((s.input.min, s.input.max), (800_000, 800_000));
    assert_eq!(s.input_clamped, 0);
    // Nothing to record.
    p.sim.record_inputs(&[], 1);
    assert_eq!(p.reader.snapshot().input.count, want);
}

#[test]
fn tick_helpers_record_durations_and_clamp_reversed_ranges() {
    let mut p = Profiler::new();
    p.sim.record_tick(1_000, 301_000);
    p.sim.record_tick(5_000, 1_000);
    let s = p.reader.snapshot();
    assert_eq!(s.tick.count, 2);
    assert_eq!((s.tick.min, s.tick.max), (0, 300_000));
    p.present.record_frame(10, 6_000_010);
    p.present.record_frame(50, 10);
    let g = p.reader.snapshot().gpu_frame;
    assert_eq!((g.count, g.min, g.max), (2, 0, 6_000_000));
}

// ---- requirement 4: overflow counters and GPU frame time ----

#[test]
fn overflow_counts_are_monotonic_and_exact() {
    let mut p = Profiler::new();
    let mut last = 0;
    for i in 1..=100_000u64 {
        p.sim.set_script_queue_overflows(i);
        p.sim_loop.set_input_overflow_ticks(i / 2);
        if i % 997 == 0 {
            // A smaller value is ignored.
            p.sim.set_script_queue_overflows(i / 3);
            p.sim_loop.set_input_overflow_ticks(0);
            let s = p.reader.snapshot();
            assert_eq!(s.script_queue_overflows, i);
            assert_eq!(s.input_overflow_ticks, i / 2);
            assert_eq!(s.command_queue_overflows(), i + i / 2);
            assert!(s.command_queue_overflows() >= last);
            last = s.command_queue_overflows();
        }
    }
    let s = p.reader.snapshot();
    assert_eq!(s.script_queue_overflows, 100_000);
    assert_eq!(s.input_overflow_ticks, 50_000);
    assert_eq!(s.command_queue_overflows(), 150_000);
    // Saturating sum.
    p.sim.set_script_queue_overflows(u64::MAX);
    p.sim_loop.set_input_overflow_ticks(5);
    assert_eq!(p.reader.snapshot().command_queue_overflows(), u64::MAX);
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 32, ..ProptestConfig::default() })]

    #[test]
    fn overflow_totals_never_decrease(totals in prop::collection::vec(0u64..1_000_000, 1..200)) {
        let mut p = Profiler::new();
        let mut hi = 0;
        for t in totals {
            p.sim.set_script_queue_overflows(t);
            hi = hi.max(t);
            prop_assert_eq!(p.reader.snapshot().script_queue_overflows, hi);
        }
    }

    #[test]
    fn gpu_frame_times_are_summarised(v in prop::collection::vec(1u64..100_000_000, 1..3000)) {
        let mut p = Profiler::new();
        for &x in &v {
            p.present.record_frame_ns(x);
        }
        let g = p.reader.snapshot().gpu_frame;
        let mut sorted = v.clone();
        sorted.sort_unstable();
        prop_assert_eq!(g.count, v.len() as u64);
        prop_assert!(within_bound(exact(&sorted, 500_000), g.p50));
        prop_assert!(within_bound(exact(&sorted, 990_000), g.p99));
        prop_assert!(within_bound(*sorted.last().unwrap(), g.max));
        prop_assert!(g.p50 <= g.p99 && g.p99 <= g.max);
    }
}

// ---- requirement 5: HUD ----

fn find<'a>(lines: &'a [String], key: &str) -> &'a str {
    lines
        .iter()
        .find(|l| l.contains(key))
        .unwrap_or_else(|| panic!("no HUD line with {key:?} in {lines:?}"))
}

#[test]
fn hud_lines_show_each_metric_with_value_and_verdict() {
    let mut p = Profiler::new();
    for _ in 0..1000 {
        p.sim.record_tick_ns(349_999);
        p.sim.record_input(0, 1_500_001);
        p.present.record_frame_ns(16_666_000);
    }
    p.sim.set_script_queue_overflows(7);
    p.sim_loop.set_input_overflow_ticks(5);
    let snap = p.reader.snapshot();
    let lines = hud_lines(&snap);
    assert_eq!(lines.len(), 4, "{lines:?}");
    let tick = find(&lines, "tick");
    assert!(tick.contains(&format_ms(349_999)), "{tick}");
    assert!(tick.contains("OK") && !tick.contains("OVER"), "{tick}");
    assert!(tick.contains("0.350"), "the target is shown: {tick}");
    let input = find(&lines, "input");
    assert!(input.contains(&format_ms(1_500_001)), "{input}");
    assert!(input.contains("OVER") && !input.contains("OK"), "{input}");
    assert!(input.contains("1.500"), "the target is shown: {input}");
    let gpu = find(&lines, "gpu");
    assert!(gpu.contains(&format_ms(16_666_000)), "{gpu}");
    assert!(
        !gpu.contains("OK") && !gpu.contains("OVER"),
        "no target: {gpu}"
    );
    let q = find(&lines, "overflows");
    assert!(q.contains("12"), "total 7 + 5: {q}");
    // The same order as documented.
    assert!(lines[0].contains("tick") && lines[1].contains("input"));
    assert!(lines[2].contains("gpu") && lines[3].contains("overflows"));
}

#[test]
fn hud_verdict_follows_the_boundary() {
    for (v, ok) in [(350_000u64, false), (349_999, true)] {
        let mut p = Profiler::new();
        for _ in 0..100 {
            p.sim.record_tick_ns(v);
        }
        let lines = hud_lines(&p.reader.snapshot());
        let tick = find(&lines, "tick");
        assert_eq!(tick.contains("OVER"), !ok, "{tick}");
    }
}

#[test]
fn hud_handles_empty_snapshots() {
    let snap = Profiler::new().reader.snapshot();
    let lines = hud_lines(&snap);
    assert_eq!(lines.len(), 4);
    assert!(find(&lines, "tick").contains("no data"));
    assert!(find(&lines, "input").contains("no data"));
    assert!(find(&lines, "overflows").contains('0'));
}

#[test]
fn hud_handles_saturated_snapshots() {
    let full = HistogramSummary {
        count: u64::MAX,
        min: u64::MAX,
        max: u64::MAX,
        mean: u64::MAX,
        p50: u64::MAX,
        p99: u64::MAX,
        p99_9: u64::MAX,
        saturated: u64::MAX,
    };
    let snap = ProfilerSnapshot {
        tick: full,
        input: full,
        input_clamped: u64::MAX,
        gpu_frame: full,
        script_queue_overflows: u64::MAX,
        input_overflow_ticks: u64::MAX,
    };
    let lines = hud_lines(&snap);
    assert_eq!(lines.len(), 4);
    assert!(find(&lines, "tick").contains("OVER"));
    assert!(find(&lines, "input").contains("OVER"));
    assert!(find(&lines, "overflows").contains(&u64::MAX.to_string()));
}

// ---- requirement 7: concurrency ----

#[test]
fn one_writer_per_metric_with_a_snapshotting_reader() {
    const N: u64 = 200_000;
    let Profiler {
        mut sim,
        mut sim_loop,
        mut present,
        reader,
    } = Profiler::new();
    let done = AtomicBool::new(false);
    let (writers_left, reads) = std::thread::scope(|sc| {
        let done = &done;
        let r2 = reader.clone();
        let rd = sc.spawn(move || {
            let (mut ticks, mut inputs, mut gpu, mut ovf) = (0u64, 0u64, 0u64, 0u64);
            let mut reads = 0u64;
            loop {
                let finished = done.load(Ordering::SeqCst);
                let s = r2.snapshot();
                assert!(s.tick.count >= ticks, "tick count went down");
                assert!(s.input.count >= inputs, "input count went down");
                assert!(s.gpu_frame.count >= gpu, "gpu count went down");
                assert!(s.script_queue_overflows >= ovf, "overflow count went down");
                (ticks, inputs, gpu, ovf) = (
                    s.tick.count,
                    s.input.count,
                    s.gpu_frame.count,
                    s.script_queue_overflows,
                );
                let _ = hud_lines(&s);
                reads += 1;
                if finished {
                    return reads;
                }
                std::thread::yield_now();
            }
        });
        let w1 = sc.spawn(move || {
            for i in 1..=N {
                sim.record_tick_ns(100_000 + i % 1000);
                sim.record_input(i, i + 50_000);
                sim.set_script_queue_overflows(i / 10);
                sim_loop.set_input_overflow_ticks(i / 100);
            }
        });
        let w2 = sc.spawn(move || {
            for i in 1..=N {
                present.record_frame_ns(16_000_000 + i % 5000);
            }
        });
        w1.join().unwrap();
        w2.join().unwrap();
        done.store(true, Ordering::SeqCst);
        (0, rd.join().unwrap())
    });
    assert_eq!(writers_left, 0);
    assert!(reads >= 1);
    let s = reader.snapshot();
    assert_eq!(s.tick.count, N);
    assert_eq!(s.input.count, N);
    assert_eq!(s.gpu_frame.count, N);
    assert_eq!(s.script_queue_overflows, N / 10);
    assert_eq!(s.input_overflow_ticks, N / 100);
    assert_eq!(s.tick.max, 100_000 + 999);
    assert_eq!(s.input.p99, 50_000);
    assert_eq!(s.tick_target_met(), Some(true));
}

#[test]
fn a_shared_histogram_counts_every_sample_from_many_threads() {
    let h = LatencyHistogram::new();
    std::thread::scope(|sc| {
        for t in 0..4u64 {
            let h = &h;
            sc.spawn(move || {
                for i in 0..50_000u64 {
                    h.record(1 + (i + t) % 1000);
                }
            });
        }
    });
    assert_eq!(h.count(), 200_000);
    assert_eq!(h.summary().max, 1000);
}
