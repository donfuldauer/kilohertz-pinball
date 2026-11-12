//! Subphase 3.2 implementer tests: plunger and nudge filters against synthetic
//! traces (generated fixtures), fallbacks, property tests and the allocation
//! guard (gate step 6).

use pin_input::calibration::{
    DigitalNudge, NudgeConfig, NudgeFilter, PlungerConfig, PlungerFilter, PlungerOutput,
    thumbstick_nudge,
};
use pin_input::profiles::NudgeDirection;
use proptest::prelude::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::f32::consts::PI;

struct Counting;
thread_local! { static ALLOCS: Cell<u64> = const { Cell::new(0) }; }
// SAFETY: forwards to the system allocator, only adding a thread-local counter.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.realloc(p, l, n) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
}
#[global_allocator]
static GLOBAL: Counting = Counting;
fn allocs() -> u64 {
    ALLOCS.with(Cell::get)
}

// ---- synthetic plunger trace fixtures (1 sample per 1 ms tick) ----

/// Deterministic jitter in `-amp..=amp` (LCG; no RNG dependency).
fn jitter(seed: &mut u32, amp: f32) -> f32 {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    ((*seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0) * amp
}

/// Rest, slow pull to `depth` over `pull_ms`, hold, slow linear return over
/// `return_ms`, rest.
fn slow_pull_slow_return(depth: f32, pull_ms: u32, return_ms: u32) -> Vec<f32> {
    let mut t = vec![0.0; 50];
    t.extend((0..=pull_ms).map(|i| depth * i as f32 / pull_ms as f32));
    t.extend(vec![depth; 300]);
    t.extend(
        (0..=return_ms)
            .rev()
            .map(|i| depth * i as f32 / return_ms as f32),
    );
    t.extend(vec![0.0; 300]);
    t
}

/// Rest, pull to `depth`, hold, then a spring snap: x(t) = depth·cos(ω t)
/// until it passes rest, followed by a damped bounce. Returns the trace and
/// the analytic peak forward speed `depth·ω`.
fn pull_and_snap(depth: f32, snap_ms: f32) -> (Vec<f32>, f32) {
    let mut t = vec![0.0; 50];
    t.extend((0..=400).map(|i| depth * i as f32 / 400.0));
    t.extend(vec![depth; 200]);
    // Quarter period = snap_ms.
    let w = PI / 2.0 / (snap_ms / 1000.0);
    let mut ms = 1.0f32;
    loop {
        let x = depth * (w * ms / 1000.0).cos();
        t.push(x);
        if x < 0.0 {
            break;
        }
        ms += 1.0;
    }
    // Damped bounce around rest, peaking above the deadzone.
    for i in 0..200 {
        let s = i as f32 / 1000.0;
        t.push(-0.08 * (-s * 25.0).exp() * (s * 60.0).cos());
    }
    t.extend(vec![0.0; 300]);
    (t, depth * w)
}

fn run(f: &mut PlungerFilter, trace: &[f32]) -> Vec<PlungerOutput> {
    trace
        .iter()
        .enumerate()
        .map(|(i, &x)| f.update(x, 1000 + i as u64))
        .collect()
}

#[test]
fn plunger_slow_pull_slow_return_no_release() {
    for (depth, ret) in [(1.0, 1000), (0.5, 800), (0.3, 2000), (1.0, 400)] {
        let mut f = PlungerFilter::new(PlungerConfig::default());
        let out = run(&mut f, &slow_pull_slow_return(depth, 500, ret));
        assert!(out.iter().all(|o| o.release.is_none()), "{depth} {ret}");
        assert!(out.iter().any(|o| o.position > 0.25));
        assert_eq!(out.last().unwrap().position, 0.0);
    }
}

#[test]
fn plunger_pull_and_snap_one_release_within_5_percent() {
    for depth in [0.3f32, 0.6, 1.0] {
        for snap_ms in [8.0f32, 15.0, 30.0, 60.0] {
            let (trace, peak) = pull_and_snap(depth, snap_ms);
            let mut f = PlungerFilter::new(PlungerConfig::default());
            let out = run(&mut f, &trace);
            let rel: Vec<f32> = out.iter().filter_map(|o| o.release).collect();
            assert_eq!(rel.len(), 1, "depth {depth} snap {snap_ms}: {rel:?}");
            let err = (rel[0] - peak).abs() / peak;
            assert!(
                err < 0.05,
                "depth {depth} snap {snap_ms}: {} vs {peak}",
                rel[0]
            );
            // Reported on the tick the plunger is back within the deadzone.
            let i = out.iter().position(|o| o.release.is_some()).unwrap();
            assert!(trace[i] <= 0.02 && trace[i - 1] > 0.02);
            assert_eq!(out[i].position, 0.0);
        }
    }
}

#[test]
fn plunger_rest_drift_is_tracked_away() {
    let mut f = PlungerFilter::new(PlungerConfig::default());
    let mut seed = 7;
    // Rest drifts from 0 to 0.15 over 30 s (sensor warm-up) with jitter.
    let mut trace = Vec::new();
    for i in 0..30_000 {
        trace.push(0.15 * i as f32 / 30_000.0 + jitter(&mut seed, 0.005));
    }
    trace.extend(vec![0.15; 1000]);
    let out = run(&mut f, &trace);
    assert!(out.iter().all(|o| o.position == 0.0 && o.release.is_none()));
    assert!((f.rest() - 0.15).abs() < 0.01, "{}", f.rest());
    // A real pull from the drifted rest still reads, with 1.0 at full pull.
    let o = f.update(1.0, 50_000);
    assert_eq!(o.position, 1.0);
    f.update(1.0, 50_001);
    f.update(0.6, 50_002);
    let o = f.update(0.15, 50_003);
    assert!(o.release.is_some());
}

#[test]
fn plunger_jitter_inside_deadzone_never_reports() {
    for amp in [0.005f32, 0.01, 0.015, 0.019] {
        let mut f = PlungerFilter::new(PlungerConfig::default());
        let mut seed = 99;
        let trace: Vec<f32> = (0..20_000).map(|_| jitter(&mut seed, amp)).collect();
        let out = run(&mut f, &trace);
        assert!(
            out.iter().all(|o| o.position == 0.0 && o.release.is_none()),
            "amp {amp}"
        );
        assert!(!f.is_pulled());
    }
}

#[test]
fn plunger_not_tracked_while_pulled() {
    let mut f = PlungerFilter::new(PlungerConfig::default());
    for t in 0..20_000 {
        f.update(0.5, t);
    }
    assert_eq!(f.rest(), 0.0);
    let o = f.update(0.5, 20_000);
    assert!((o.position - 0.48 / 0.98).abs() < 1e-6);
}

fn count_releases(f: &mut PlungerFilter, trace: &[f32], t0: u64) -> usize {
    trace
        .iter()
        .enumerate()
        .filter(|(i, x)| f.update(**x, t0 + *i as u64).release.is_some())
        .count()
}

/// Review B1(a): one noisy sample just outside the deadzone is no launch.
#[test]
fn plunger_single_edge_spike_is_not_a_release() {
    let mut f = PlungerFilter::new(PlungerConfig::default());
    let mut trace = vec![0.0f32; 100];
    trace.push(0.025);
    trace.extend(vec![0.0; 100]);
    // A spike up to just below the qualifying depth, too.
    trace.push(0.049);
    trace.extend(vec![0.0; 100]);
    assert_eq!(count_releases(&mut f, &trace, 0), 0);
}

/// Review B1(b): a device resting at the deadzone edge with ADC jitter
/// neither launches nor stops rest tracking from converging.
#[test]
fn plunger_jitter_at_the_deadzone_edge_converges() {
    let mut f = PlungerFilter::new(PlungerConfig::default());
    let trace: Vec<f32> = (0..3000)
        .map(|i| if i % 2 == 0 { 0.015 } else { 0.021 })
        .collect();
    assert_eq!(count_releases(&mut f, &trace, 0), 0);
    assert!((f.rest() - 0.018).abs() < 0.004, "{}", f.rest());
    // Once converged, the jitter reads as rest.
    let o = f.update(0.021, 3000);
    assert_eq!(o.position, 0.0);
}

/// Review B1(c): an 8-bit device toggling between two codes at the edge.
#[test]
fn plunger_8bit_code_toggle_is_not_a_release() {
    let cfg = PlungerConfig {
        rest_tracking_per_s: 0.0,
        ..PlungerConfig::default()
    };
    let mut f = PlungerFilter::new(cfg);
    let trace: Vec<f32> = (0..2000)
        .map(|i| {
            if (i / 3) % 2 == 0 {
                5.0 / 255.0
            } else {
                6.0 / 255.0
            }
        })
        .collect();
    assert_eq!(count_releases(&mut f, &trace, 0), 0);
}

/// Review B1(d): rest held below 0, then small jitter, is no launch.
#[test]
fn plunger_low_rest_then_jitter_is_not_a_release() {
    let mut f = PlungerFilter::new(PlungerConfig::default());
    let mut trace = vec![-0.015f32; 3000];
    trace.extend((0..2000).map(|i| if i % 2 == 0 { 0.008 } else { 0.004 }));
    let n = trace.len();
    let out = run(&mut f, &trace);
    assert!(out.iter().all(|o| o.release.is_none()));
    assert!(out[n - 1000..].iter().all(|o| o.position < 0.01));
}

/// Review N1: a plunger resting above the deadzone at power-on is acquired as
/// rest and then launches normally.
#[test]
fn plunger_power_on_rest_is_acquired() {
    for rest in [0.05f32, 0.12, 0.2] {
        let mut f = PlungerFilter::new(PlungerConfig::default());
        let mut seed = 3;
        let idle: Vec<f32> = (0..1500).map(|_| rest + jitter(&mut seed, 0.004)).collect();
        let out = run(&mut f, &idle);
        assert!(out.iter().all(|o| o.release.is_none()));
        assert!(!f.is_pulled(), "rest {rest}");
        assert!((f.rest() - rest).abs() < 0.005, "{} vs {rest}", f.rest());
        assert_eq!(out.last().unwrap().position, 0.0);
        // A real pull and snap from the acquired rest: one release.
        let mut t = 2500u64; // `run` used ticks 1000..2500
        let mut releases = Vec::new();
        for k in 0..=400 {
            f.update(rest + (1.0 - rest) * k as f32 / 400.0, t);
            t += 1;
        }
        for k in (0..=20).rev() {
            releases.extend(f.update(rest + (1.0 - rest) * k as f32 / 20.0, t).release);
            t += 1;
        }
        for _ in 0..300 {
            releases.extend(f.update(rest, t).release);
            t += 1;
        }
        assert_eq!(releases.len(), 1, "rest {rest}");
        assert!((releases[0] - (1.0 - rest) * 50.0).abs() < 1.0);
    }
    // Resting above MAX_REST_OFFSET is a held pull, not a rest.
    let mut f = PlungerFilter::new(PlungerConfig::default());
    run(&mut f, &[0.4; 2000]);
    assert!(f.is_pulled() && f.rest() == 0.0);
    // Only at power-on: after a release a held pull is never re-zeroed.
    let mut f = PlungerFilter::new(PlungerConfig::default());
    let (snap, _) = pull_and_snap(0.8, 15.0);
    run(&mut f, &snap);
    run(&mut f, &[0.1; 2000]);
    assert!(f.is_pulled() && f.rest().abs() < 0.01);
}

/// Review N2: a very slow pull may drag rest along, but only to
/// MAX_REST_OFFSET, and rest is re-acquired downward once the plunger sits
/// below it.
#[test]
fn plunger_slow_drag_is_bounded_and_reacquired() {
    let mut f = PlungerFilter::new(PlungerConfig::default());
    // 0.01 full-scale/s (below deadzone × tracking rate) up to 0.5.
    let mut trace: Vec<f32> = (0..50_000).map(|i| 0.5 * i as f32 / 50_000.0).collect();
    trace.extend((0..=1000).rev().map(|i| 0.5 * i as f32 / 1000.0)); // slow return
    trace.extend(vec![0.0; 1500]);
    let out = run(&mut f, &trace);
    assert!(out.iter().all(|o| o.release.is_none()));
    assert!(f.rest().abs() < 0.01, "rest {} not re-acquired", f.rest());
    // Rest never exceeded the bound on the way.
    let mut g = PlungerFilter::new(PlungerConfig::default());
    for (i, &x) in trace[..50_000].iter().enumerate() {
        g.update(x, i as u64);
        assert!(g.rest() <= pin_input::calibration::MAX_REST_OFFSET);
    }
    // A small pull reads again.
    let o = f.update(0.1, 60_000_000);
    assert!(o.position > 0.05, "{}", o.position);
}

/// Review N3: a spring bounce 95–150 ms after the release, and a zero peak
/// window, give no second release.
#[test]
fn plunger_late_bounce_is_held_off() {
    for window in [0u32, 30, 100] {
        for gap in [95usize, 120, 150, 190] {
            let cfg = PlungerConfig {
                release_window_ms: window,
                ..PlungerConfig::default()
            };
            let mut f = PlungerFilter::new(cfg);
            let mut trace = vec![0.0f32; 50];
            trace.extend((0..=300).map(|i| i as f32 / 300.0));
            trace.extend((0..=20).rev().map(|i| i as f32 / 20.0));
            trace.extend(vec![0.0; gap]);
            trace.extend([0.06, 0.12, 0.08, 0.0]); // bounce, fast return
            trace.extend(vec![0.0; 300]);
            assert_eq!(
                count_releases(&mut f, &trace, 0),
                1,
                "window {window} gap {gap}"
            );
        }
    }
}

// ---- round 2: the filter never gets stuck (review B2, N-a, N-c) ----

/// Holds `level` for `ms` ticks; returns the releases seen.
fn hold(f: &mut PlungerFilter, t: &mut u64, level: f32, ms: u64) -> Vec<f32> {
    let mut r = Vec::new();
    for _ in 0..ms {
        r.extend(f.update(level, *t).release);
        *t += 1;
    }
    r
}

/// Linear ramp from `a` to `b` over `ms` ticks (excluding `a`).
fn ramp(f: &mut PlungerFilter, t: &mut u64, a: f32, b: f32, ms: u64) -> Vec<f32> {
    let mut r = Vec::new();
    for k in 1..=ms {
        r.extend(f.update(a + (b - a) * k as f32 / ms as f32, *t).release);
        *t += 1;
    }
    r
}

/// A normal launch from `rest`: 300 ms pull to `rest + depth`, 100 ms hold,
/// `snap_ms` linear snap, 300 ms at rest. Asserts exactly one release within
/// 5 % of the true speed `depth / snap_ms`.
fn assert_launch(
    f: &mut PlungerFilter,
    t: &mut u64,
    rest: f32,
    depth: f32,
    snap_ms: u64,
    what: &str,
) {
    let mut r = ramp(f, t, rest, rest + depth, 300);
    r.extend(hold(f, t, rest + depth, 100));
    r.extend(ramp(f, t, rest + depth, rest, snap_ms));
    r.extend(hold(f, t, rest, 300));
    assert_eq!(r.len(), 1, "{what}: releases {r:?}");
    let want = depth / snap_ms as f32 * 1000.0;
    assert!(
        (r[0] - want).abs() <= 0.05 * want,
        "{what}: {} vs {want}",
        r[0]
    );
}

/// B2: holding the knob pushed in, for 1.5 s or 6 s, never disables launches:
/// after ≤ 2 s back at rest the next launch (shallow or full) reports.
#[test]
fn plunger_push_in_never_disables_launches() {
    for push in [-0.03f32, -0.1, -0.3] {
        for hold_ms in [1500u64, 6000] {
            let what = format!("push {push} for {hold_ms} ms");
            let mut f = PlungerFilter::new(PlungerConfig::default());
            let mut t = 0;
            assert!(hold(&mut f, &mut t, 0.0, 5000).is_empty());
            let mut r = ramp(&mut f, &mut t, 0.0, push, 20);
            for _ in 0..hold_ms {
                let o = f.update(push, t);
                t += 1;
                assert_eq!(o.position, 0.0, "{what}: pushed in reads 0");
                r.extend(o.release);
            }
            // Let go: spring back with a small overshoot.
            r.extend(ramp(&mut f, &mut t, push, 0.01, 15));
            r.extend(ramp(&mut f, &mut t, 0.01, 0.0, 10));
            r.extend(hold(&mut f, &mut t, 0.0, 2000));
            assert!(r.is_empty(), "{what}: {r:?}");
            assert!(!f.is_pulled(), "{what}: stuck pulled, rest {}", f.rest());
            assert_launch(&mut f, &mut t, 0.0, 0.1, 10, &what);
            assert_launch(&mut f, &mut t, 0.0, 0.9, 20, &what);
        }
    }
}

/// B2: a power-on rest of 0.06, then a dropout to 0.0 (reconnect), then back.
#[test]
fn plunger_dropout_from_power_on_rest_recovers() {
    for drop_ms in [1500u64, 6000] {
        let what = format!("dropout {drop_ms} ms");
        let mut f = PlungerFilter::new(PlungerConfig::default());
        let mut t = 0;
        assert!(hold(&mut f, &mut t, 0.06, 1500).is_empty());
        assert!((f.rest() - 0.06).abs() < 1e-3);
        assert!(hold(&mut f, &mut t, 0.0, drop_ms).is_empty());
        assert!(hold(&mut f, &mut t, 0.06, 2000).is_empty(), "{what}");
        assert!(!f.is_pulled(), "{what}: rest {}", f.rest());
        assert_launch(&mut f, &mut t, 0.06, 0.1, 10, &what);
        assert_launch(&mut f, &mut t, 0.06, 0.8, 20, &what);
    }
}

/// N-c: the true rest steps up after the power-on window. Steps below the
/// qualifying depth are re-acquired within 1 s; a larger step is
/// indistinguishable from a held pull and stays one (documented bound).
#[test]
fn plunger_rest_step_after_power_on_window() {
    for step in [0.03f32, 0.04, 0.049] {
        let what = format!("step {step}");
        let mut f = PlungerFilter::new(PlungerConfig::default());
        let mut t = 0;
        hold(&mut f, &mut t, 0.0, 5000);
        assert_launch(&mut f, &mut t, 0.0, 0.5, 20, &what);
        assert!(hold(&mut f, &mut t, step, 2000).is_empty());
        assert!((f.rest() - step).abs() < 0.005, "{what}: rest {}", f.rest());
        assert_launch(&mut f, &mut t, step, 0.1, 10, &what);
    }
    let mut f = PlungerFilter::new(PlungerConfig::default());
    let mut t = 0;
    hold(&mut f, &mut t, 0.0, 5000);
    hold(&mut f, &mut t, 0.08, 3000);
    assert!(
        f.is_pulled() && f.rest().abs() < 0.01,
        "held pull, not a rest"
    );
    // Back at the old rest it launches again at once.
    hold(&mut f, &mut t, 0.0, 100);
    assert_launch(&mut f, &mut t, 0.0, 0.5, 20, "after held 0.08");
}

/// Invariant: from arbitrary prior motion, after ≤ 2 s still at the true rest
/// (0) the next normal launch reports exactly one release.
mod never_stuck {
    use super::*;
    proptest! {
        #![proptest_config(ProptestConfig::with_cases(48))]
        #[test]
        fn any_history_then_rest_then_launch(
            segs in prop::collection::vec((-0.35f32..1.0, 1u64..4000, any::<bool>()), 0..8),
            depth in 0.1f32..0.9,
            snap_ms in 5u64..40,
        ) {
            let mut f = PlungerFilter::new(PlungerConfig::default());
            let mut t = 0u64;
            let mut last = 0.0f32;
            for (level, ms, ramped) in segs {
                if ramped {
                    ramp(&mut f, &mut t, last, level, ms.min(500));
                }
                hold(&mut f, &mut t, level, ms);
                last = level;
            }
            ramp(&mut f, &mut t, last, 0.0, 300);
            hold(&mut f, &mut t, 0.0, 2000);
            let mut r = ramp(&mut f, &mut t, 0.0, depth, 300);
            r.extend(hold(&mut f, &mut t, depth, 100));
            r.extend(ramp(&mut f, &mut t, depth, 0.0, snap_ms));
            r.extend(hold(&mut f, &mut t, 0.0, 300));
            prop_assert_eq!(r.len(), 1, "rest {} releases {:?}", f.rest(), r);
            let want = depth / snap_ms as f32 * 1000.0;
            prop_assert!((r[0] - want).abs() <= 0.05 * want, "{} vs {}", r[0], want);
        }
    }
}

/// N-a: one- and two-sample spikes are not launches; a 3 ms / 3 ms tap is.
#[test]
fn plunger_spikes_versus_fast_taps() {
    for spike in [vec![0.2f32], vec![0.9], vec![0.3, 0.3], vec![0.5, 0.9]] {
        let mut f = PlungerFilter::new(PlungerConfig::default());
        let mut t = 0;
        hold(&mut f, &mut t, 0.0, 500);
        let mut r = Vec::new();
        for &x in &spike {
            r.extend(f.update(x, t).release);
            t += 1;
        }
        r.extend(hold(&mut f, &mut t, 0.0, 300));
        assert!(r.is_empty(), "spike {spike:?}: {r:?}");
        // The filter still launches right after.
        assert_launch(&mut f, &mut t, 0.0, 0.5, 20, "after spike");
    }
    for depth in [0.3f32, 1.0] {
        let mut f = PlungerFilter::new(PlungerConfig::default());
        let mut t = 0;
        hold(&mut f, &mut t, 0.0, 500);
        let mut r = ramp(&mut f, &mut t, 0.0, depth, 3);
        r.extend(ramp(&mut f, &mut t, depth, 0.0, 3));
        r.extend(hold(&mut f, &mut t, 0.0, 300));
        assert_eq!(r.len(), 1, "tap {depth}");
        let want = depth / 3.0 * 1000.0;
        assert!((r[0] - want).abs() <= 0.05 * want, "{} vs {want}", r[0]);
    }
}

/// N-b (partial): a bounce that never settles at rest cannot qualify.
#[test]
fn plunger_unsettled_bounce_is_not_a_release() {
    let mut f = PlungerFilter::new(PlungerConfig::default());
    let mut t = 0;
    hold(&mut f, &mut t, 0.0, 500);
    assert_launch(&mut f, &mut t, 0.0, 0.8, 15, "first");
    // 300 ms of ringing around rest with excursions past the deadzone, then a
    // 6 % bounce and fast return: never 50 ms at rest, so no second launch.
    let mut r = Vec::new();
    for k in 0..300u64 {
        let x = if (k / 20) % 2 == 0 { 0.03 } else { -0.03 };
        r.extend(f.update(x, t).release);
        t += 1;
    }
    r.extend(ramp(&mut f, &mut t, -0.03, 0.06, 5));
    r.extend(ramp(&mut f, &mut t, 0.06, 0.0, 3));
    r.extend(hold(&mut f, &mut t, 0.0, 300));
    assert!(r.is_empty(), "{r:?}");
}

// ---- nudge ----

fn linear_cfg() -> NudgeConfig {
    NudgeConfig {
        high_pass_hz: 2.0,
        gain: 1.0,
        curve_exponent: 1.0,
        deadzone: 0.0,
        max: 1.0,
    }
}

#[test]
fn nudge_step_response_follows_cutoff() {
    for hz in [0.5f32, 2.0, 5.0] {
        let mut f = NudgeFilter::new(NudgeConfig {
            high_pass_hz: hz,
            ..linear_cfg()
        });
        let tau = 1.0 / (2.0 * PI * hz);
        let dt = 0.001;
        let first = f.update([0.8, -0.4], dt);
        assert!(first[0] > 0.97 * 0.8 && first[1] < -0.97 * 0.4, "{first:?}");
        // The first update is the step itself, at t = 0.
        let mut t = 0.0;
        let mut out = first;
        while t < 5.0 * tau {
            out = f.update([0.8, -0.4], dt);
            t += dt;
            // Never decays slower than the analog exponential (+0.5 % slack).
            let env = (-t / tau).exp() * 1.005 + 1e-6;
            assert!(out[0] <= 0.8 * env && out[0] >= 0.0, "{hz} {t} {out:?}");
        }
        assert!(out[0].abs() < 0.008 && out[1].abs() < 0.004, "{out:?}");
        // At one time constant ≈ e^-1.
        let mut g = NudgeFilter::new(NudgeConfig {
            high_pass_hz: hz,
            ..linear_cfg()
        });
        let n = (tau / dt).round() as usize;
        let mut o = [0.0; 2];
        for _ in 0..=n {
            o = g.update([0.8, 0.0], dt);
        }
        assert!((o[0] / 0.8 - (-1.0f32).exp()).abs() < 0.01, "{o:?}");
    }
}

#[test]
fn nudge_passes_ten_times_cutoff() {
    for dt in [0.001f32, 0.002, 1.0 / 240.0] {
        let mut f = NudgeFilter::new(linear_cfg());
        let freq = 20.0f32;
        let amp = 0.6f32;
        let n = (3.0 / dt) as usize;
        let mut peak = 0.0f32;
        for i in 0..n {
            let t = i as f32 * dt;
            let o = f.update([amp * (2.0 * PI * freq * t).sin(), 0.0], dt);
            if t > 2.0 {
                peak = peak.max(f.high_passed()[0].abs());
                assert!(o[0].abs() <= 1.0);
            }
        }
        assert!(peak / amp >= 0.95, "dt {dt}: gain {}", peak / amp);
        assert!(peak / amp <= 1.01, "dt {dt}: gain {}", peak / amp);
    }
}

#[test]
fn nudge_curve_formula() {
    let cfg = NudgeConfig {
        high_pass_hz: 0.0,
        gain: 1.5,
        curve_exponent: 1.7,
        deadzone: 0.05,
        max: 0.8,
    };
    for x in [-0.9f32, -0.3, -0.04, 0.0, 0.03, 0.2, 0.5, 0.95] {
        let mut f = NudgeFilter::new(cfg);
        let o = f.update([x, -x], 0.001);
        let want = if x.abs() <= 0.05 {
            0.0
        } else {
            x.signum() * (1.5 * x.abs().powf(1.7)).min(0.8)
        };
        assert!((o[0] - want).abs() < 1e-5, "{x}: {o:?} vs {want}");
        assert!((o[1] + want).abs() < 1e-5);
    }
}

#[test]
fn digital_nudge_formula() {
    let mut d = DigitalNudge::new(0.4, 50);
    assert_eq!(d.sample(0), [0.0, 0.0]);
    d.press(NudgeDirection::Left, 100);
    assert_eq!(d.sample(99), [0.0, 0.0]);
    assert_eq!(d.sample(100), [-0.4, 0.0]);
    assert_eq!(d.sample(149), [-0.4, 0.0]);
    assert_eq!(d.sample(150), [0.0, 0.0]);
    d.press(NudgeDirection::Forward, 120);
    d.press(NudgeDirection::Right, 130);
    assert_eq!(d.sample(125), [-0.4, 0.4]);
    let s = d.sample(135);
    assert!(s[0].abs() < 1e-6 && (s[1] - 0.4).abs() < 1e-6);
    d.press(NudgeDirection::Forward, 125);
    d.press(NudgeDirection::Forward, 126);
    assert_eq!(d.sample(140)[1], 1.0); // 3 × 0.4 clamps to 1
    let mut e = DigitalNudge::new(0.7, 10);
    e.press(NudgeDirection::Right, 0);
    e.press(NudgeDirection::Right, 5);
    assert_eq!(e.sample(7), [1.0, 0.0]);
    assert_eq!(e.sample(12), [0.7, 0.0]);
    assert_eq!(e.sample(15), [0.0, 0.0]);
}

#[test]
fn thumbstick_formula() {
    assert_eq!(thumbstick_nudge(0.1, 0.1, 0.2), [0.0, 0.0]);
    let o = thumbstick_nudge(0.6, 0.0, 0.2);
    assert!((o[0] - 0.5).abs() < 1e-6 && o[1] == 0.0);
    let o = thumbstick_nudge(0.0, -1.0, 0.2);
    assert_eq!(o, [0.0, -1.0]);
    let o = thumbstick_nudge(0.3, 0.4, 0.0);
    assert!((o[0] - 0.3).abs() < 1e-6 && (o[1] - 0.4).abs() < 1e-6);
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn plunger_outputs_bounded(trace in prop::collection::vec(prop_oneof![
        -2.0f32..2.0, Just(f32::NAN), Just(f32::INFINITY), Just(0.0f32), Just(1.0f32)
    ], 1..400), step in 0u64..3) {
        let mut f = PlungerFilter::new(PlungerConfig::default());
        let mut t = 0;
        let mut pulled_before = false;
        for x in trace {
            let o = f.update(x, t);
            prop_assert!((0.0..=1.0).contains(&o.position));
            if let Some(r) = o.release {
                prop_assert!(r.is_finite() && r >= PlungerConfig::default().release_min_speed);
                prop_assert!(pulled_before);
                prop_assert!(!f.is_pulled());
            }
            pulled_before = f.is_pulled();
            t += step;
        }
        prop_assert!(f.rest().is_finite());
    }

    #[test]
    fn nudge_outputs_bounded(samples in prop::collection::vec(
        (prop::num::f32::ANY, prop::num::f32::ANY, prop::num::f32::ANY), 1..200),
        hz in prop::num::f32::ANY, gain in prop::num::f32::ANY, e in prop::num::f32::ANY,
        dz in prop::num::f32::ANY, max in prop::num::f32::ANY)
    {
        let mut f = NudgeFilter::new(NudgeConfig { high_pass_hz: hz, gain, curve_exponent: e, deadzone: dz, max });
        let lim = if max.is_finite() { max.abs() } else { 1.0 };
        for (x, y, dt) in samples {
            let o = f.update([x, y], dt);
            for v in o {
                prop_assert!(!v.is_nan() && v.abs() <= lim);
            }
        }
    }

    #[test]
    fn thumbstick_bounded(x in prop::num::f32::ANY, y in prop::num::f32::ANY, dz in prop::num::f32::ANY) {
        let o = thumbstick_nudge(x, y, dz);
        prop_assert!(o.iter().all(|v| v.is_finite() && v.abs() <= 1.0));
        prop_assert!(o[0] * o[0] + o[1] * o[1] <= 1.0 + 1e-5);
    }

    #[test]
    fn digital_bounded(presses in prop::collection::vec((0u8..3, 0u64..500), 0..64),
                       s in prop::num::f32::ANY, dur in 0u32..300, at in 0u64..800) {
        let mut d = DigitalNudge::new(s, dur);
        for (k, t) in presses {
            d.press(NudgeDirection::ALL[k as usize], t);
        }
        let v = d.sample(at);
        prop_assert!(v.iter().all(|c| !c.is_nan() && c.abs() <= 1.0));
    }
}

#[test]
fn filters_do_not_allocate() {
    let mut p = PlungerFilter::new(PlungerConfig::default());
    let mut n = NudgeFilter::new(NudgeConfig::default());
    let mut d = DigitalNudge::new(0.3, 40);
    let (snap, _) = pull_and_snap(1.0, 15.0);
    let mut acc = 0.0f32;
    let before = allocs();
    for tick in 0..20_000u64 {
        let raw = snap[(tick as usize) % snap.len()];
        let o = p.update(if tick % 997 == 0 { f32::NAN } else { raw }, tick);
        acc += o.position + o.release.unwrap_or(0.0);
        let s = (tick as f32 * 0.05).sin();
        let v = n.update([s, -s], 0.001);
        if tick % 300 == 0 {
            d.press(NudgeDirection::ALL[(tick / 300 % 3) as usize], tick);
        }
        let w = d.sample(tick);
        let t = thumbstick_nudge(s, 0.5, 0.1);
        acc += v[0] + w[0] + w[1] + t[0];
        if tick % 5000 == 0 {
            n.reset();
        }
    }
    assert_eq!(allocs(), before, "filters allocated");
    assert!(acc.is_finite());
}

/// Gate step 6 analogue: the filters are pure functions of their inputs, so a
/// hash over every output bit is identical over 10 runs.
#[test]
fn filter_output_hash_is_stable_over_10_runs() {
    let (snap, _) = pull_and_snap(0.8, 20.0);
    let hash_once = || {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        let mut mix = |v: f32| {
            h ^= u64::from(v.to_bits());
            h = h.wrapping_mul(0x0100_0000_01b3);
        };
        let mut p = PlungerFilter::new(PlungerConfig::default());
        let mut n = NudgeFilter::new(NudgeConfig {
            curve_exponent: 1.3,
            ..NudgeConfig::default()
        });
        for (i, &x) in snap.iter().enumerate() {
            let o = p.update(x, i as u64);
            mix(o.position);
            mix(o.release.unwrap_or(-1.0));
            let s = (i as f32 * 0.07).sin() * 0.5;
            let v = n.update([s, x], 0.001);
            mix(v[0]);
            mix(v[1]);
        }
        h
    };
    let first = hash_once();
    for _ in 0..9 {
        assert_eq!(hash_once(), first);
    }
}
