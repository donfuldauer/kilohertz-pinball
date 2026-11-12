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
    let o = f.update(0.15, 50_001);
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
