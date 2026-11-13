//! Independent Tester spec tests, Subphase 3.4, verification criterion 5:
//! the SDL-free rumble mapper is deterministic, bounded to 0..=1, decays to 0
//! after the configured duration and merges overlapping events by max.
//!
//! Black box: written from the brief plus the public signatures and module
//! documentation of `pin_input::haptics` (settings type, envelope law).

use pin_core::haptics::{HapticEvent, HapticKind};
use pin_core::ids::{CoilId, FlipperId};
use pin_core::input::HapticsTelemetryEvent;
use pin_input::haptics::{Rumble, RumbleEffect, RumbleMapper, RumbleSettings};
use proptest::prelude::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

struct Counting;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

fn bump() {
    let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
}

// SAFETY: forwards to the system allocator, only adding a thread-local counter.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        bump();
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        bump();
        unsafe { System.realloc(ptr, layout, new_size) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        unsafe { System.dealloc(ptr, layout) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn allocs() -> u64 {
    ALLOCS.with(Cell::get)
}

// ------------------------------------------------------------------ helpers

const COIL: RumbleEffect = RumbleEffect::new(0.6, 0.3, 60);
const EOS: RumbleEffect = RumbleEffect::new(0.2, 0.7, 30);
const BUMPER: RumbleEffect = RumbleEffect::new(0.9, 0.5, 80);
const DROP: RumbleEffect = RumbleEffect::new(0.7, 0.2, 70);
const ROLL: RumbleEffect = RumbleEffect::new(0.2, 0.05, 20);
const IMPULSE: RumbleEffect = RumbleEffect::new(0.5, 0.5, 50);

fn settings() -> RumbleSettings {
    let mut s = RumbleSettings::default();
    s.enabled = true;
    s.master = 1.0;
    s.coil_fire = COIL;
    s.eos_hit = EOS;
    s.bumper = BUMPER;
    s.ball_drop = DROP;
    s.ball_roll = ROLL;
    s.impulse = IMPULSE;
    s
}

fn coil(tick: u64, n: u16, strength: f32) -> HapticEvent {
    HapticEvent::new(tick, HapticKind::CoilFire { coil: CoilId(n) }, strength)
}

fn eos(tick: u64, n: u16, strength: f32) -> HapticEvent {
    HapticEvent::new(tick, HapticKind::EosHit { flipper: FlipperId(n) }, strength)
}

fn bumper(tick: u64, n: u16, strength: f32) -> HapticEvent {
    HapticEvent::new(tick, HapticKind::Bumper { mech: n }, strength)
}

fn kind_event(k: u8, tick: u64, n: u16, strength: f32) -> HapticEvent {
    let kind = match k % 6 {
        0 => HapticKind::CoilFire { coil: CoilId(n) },
        1 => HapticKind::EosHit { flipper: FlipperId(n) },
        2 => HapticKind::Bumper { mech: n },
        3 => HapticKind::BallDrop { ball: n, position_mm: [10.0, 500.0, 0.0] },
        4 => HapticKind::BallRoll {
            ball: n,
            position_mm: [10.0, 500.0, 0.0],
            mass_kg: 0.08,
            speed_m_s: 1.5,
        },
        _ => HapticKind::Impulse { source: n },
    };
    HapticEvent::new(tick, kind, strength)
}

fn effect_of(k: u8) -> RumbleEffect {
    [COIL, EOS, BUMPER, DROP, ROLL, IMPULSE][usize::from(k % 6)]
}

/// Feeds `events` (sorted by tick) as the main thread would: every tick,
/// first the events of that tick, then one `output_at`.
fn run(settings: RumbleSettings, events: &[HapticEvent], from: u64, to: u64) -> Vec<Rumble> {
    let mut m = RumbleMapper::new(settings);
    let mut next = 0;
    let mut out = Vec::with_capacity((to - from) as usize);
    for t in from..to {
        while next < events.len() && events[next].tick <= t {
            m.feed(events[next]);
            next += 1;
        }
        out.push(m.output_at(t));
    }
    out
}

fn bits(v: &[Rumble]) -> Vec<(u32, u32)> {
    v.iter().map(|r| (r.low.to_bits(), r.high.to_bits())).collect()
}

fn in_range(r: &Rumble) -> bool {
    (0.0..=1.0).contains(&r.low) && (0.0..=1.0).contains(&r.high)
}

fn clamp01(v: f32) -> f32 {
    if v > 0.0 { v.min(1.0) } else { 0.0 }
}

/// The documented envelope of one event at tick `t`.
fn envelope(e: &HapticEvent, fx: RumbleEffect, master: f32, t: u64) -> (f32, f32) {
    if fx.duration_ticks == 0 || t < e.tick || t >= e.tick + u64::from(fx.duration_ticks) {
        return (0.0, 0.0);
    }
    let k = 1.0 - (t - e.tick) as f32 / fx.duration_ticks as f32;
    let s = clamp01(e.strength) * clamp01(master);
    (clamp01(fx.low) * s * k, clamp01(fx.high) * s * k)
}

// -------------------------------------------------------------------- tests

#[test]
fn silent_without_events() {
    let out = run(settings(), &[], 0, 500);
    assert!(out.iter().all(|r| r.low == 0.0 && r.high == 0.0));
    let mut m = RumbleMapper::new(RumbleSettings::default());
    let r = m.output_at(0);
    assert_eq!((r.low, r.high), (0.0, 0.0));
}

#[test]
fn flipper_sling_and_bumper_events_rumble() {
    for (e, name) in [
        (coil(10, 1, 1.0), "flipper/sling coil"),
        (bumper(10, 3, 1.0), "bumper"),
        (eos(10, 0, 1.0), "EOS"),
    ] {
        // With explicit settings and with the defaults.
        for s in [settings(), RumbleSettings::default()] {
            let out = run(s, &[e], 0, 300);
            assert!(out[..10].iter().all(|r| r.low == 0.0 && r.high == 0.0), "{name}: early");
            assert!(out[10].low > 0.0 || out[10].high > 0.0, "{name}: no rumble on its tick");
            assert!(out.iter().all(in_range), "{name}");
            let last = out.last().unwrap();
            assert_eq!((last.low, last.high), (0.0, 0.0), "{name}: never stopped");
        }
    }
}

#[test]
fn decays_to_zero_after_the_configured_duration() {
    for duration in [1u32, 2, 15, 60, 250, 1000] {
        let mut s = settings();
        s.coil_fire = RumbleEffect::new(0.8, 0.4, duration);
        let t0 = 100u64;
        let out = run(s, &[coil(t0, 2, 1.0)], 0, t0 + u64::from(duration) + 50);
        let d = duration as usize;
        let start = t0 as usize;
        assert!(out[start].low > 0.0 && out[start].high > 0.0, "duration {duration}");
        assert!((out[start].low - 0.8).abs() < 1e-6 && (out[start].high - 0.4).abs() < 1e-6);
        for (i, r) in out.iter().enumerate() {
            if i < start || i >= start + d {
                assert_eq!((r.low, r.high), (0.0, 0.0), "duration {duration}, tick {i}");
            } else {
                assert!(r.low > 0.0 && r.high > 0.0, "duration {duration}, tick {i}: ended early");
            }
        }
        // Decay: never rises while a single event rings out.
        for w in out[start..].windows(2) {
            assert!(w[1].low <= w[0].low && w[1].high <= w[0].high, "duration {duration}");
        }
    }
}

#[test]
fn zero_duration_zero_strength_disabled_and_zero_master_are_silent() {
    let e = [coil(5, 1, 1.0)];
    let mut zero_duration = settings();
    zero_duration.coil_fire = RumbleEffect::new(1.0, 1.0, 0);
    let mut disabled = settings();
    disabled.enabled = false;
    let mut muted = settings();
    muted.master = 0.0;
    let mut none = settings();
    none.coil_fire = RumbleEffect::NONE;
    for (s, name) in [
        (zero_duration, "zero duration"),
        (disabled, "disabled"),
        (muted, "master 0"),
        (none, "effect NONE"),
    ] {
        let out = run(s, &e, 0, 200);
        assert!(out.iter().all(|r| r.low == 0.0 && r.high == 0.0), "{name}");
    }
    for strength in [0.0, -1.0, f32::NAN, f32::NEG_INFINITY] {
        let out = run(settings(), &[coil(5, 1, strength)], 0, 200);
        assert!(out.iter().all(|r| r.low == 0.0 && r.high == 0.0), "strength {strength}");
    }
}

#[test]
fn strength_scales_and_saturates() {
    let peak = |s: f32| run(settings(), &[coil(0, 1, s)], 0, 1)[0];
    let (quarter, half, full, over, inf) =
        (peak(0.25), peak(0.5), peak(1.0), peak(5.0), peak(f32::INFINITY));
    assert!(quarter.low < half.low && half.low < full.low);
    assert!(quarter.high < half.high && half.high < full.high);
    assert!((half.low - 0.5 * full.low).abs() < 1e-6);
    assert_eq!((over.low, over.high), (full.low, full.high), "strength above 1 saturates");
    assert_eq!((inf.low, inf.high), (full.low, full.high));
    assert!((full.low - COIL.low).abs() < 1e-6 && (full.high - COIL.high).abs() < 1e-6);
}

#[test]
fn overlapping_events_merge_by_max_not_sum() {
    // Two strong events on top of each other: a sum would exceed 1.
    let mut s = settings();
    s.coil_fire = RumbleEffect::new(0.9, 0.8, 100);
    s.bumper = RumbleEffect::new(0.7, 1.0, 40);
    let a = coil(20, 1, 1.0);
    let b = bumper(35, 2, 1.0);
    let only_a = run(s.clone(), &[a], 0, 200);
    let only_b = run(s.clone(), &[b], 0, 200);
    let both = run(s.clone(), &[a, b], 0, 200);
    let mut overlap_seen = false;
    for t in 0..200 {
        let want = (only_a[t].low.max(only_b[t].low), only_a[t].high.max(only_b[t].high));
        assert!(
            (both[t].low - want.0).abs() < 1e-6 && (both[t].high - want.1).abs() < 1e-6,
            "tick {t}: got {:?}, max is {want:?}",
            both[t]
        );
        assert!(in_range(&both[t]));
        if only_a[t].low > 0.0 && only_b[t].low > 0.0 {
            overlap_seen = true;
            assert!(both[t].low < only_a[t].low + only_b[t].low, "tick {t}: summed");
        }
    }
    assert!(overlap_seen);

    // Sixteen identical full-strength events on one tick are no louder than one.
    let many: Vec<_> = (0..16).map(|i| coil(20, i, 1.0)).collect();
    let stacked = run(s.clone(), &many, 0, 200);
    assert_eq!(bits(&stacked), bits(&only_a));

    // The same source retriggered: the fresh event takes over, never above the peak.
    let retrig = run(s, &[coil(20, 1, 1.0), coil(50, 1, 1.0)], 0, 200);
    assert!(retrig.iter().all(|r| r.low <= 0.9 + 1e-6 && r.high <= 0.8 + 1e-6));
    assert!((retrig[50].low - 0.9).abs() < 1e-6);
    assert_eq!((retrig[150].low, retrig[150].high), (0.0, 0.0));
}

#[test]
fn per_source_settings() {
    let left = coil(0, 1, 1.0);
    let right = coil(0, 2, 1.0);
    let custom = RumbleEffect::new(0.1, 1.0, 7);
    let mut s = settings();
    assert!(s.set_override(left.source(), custom));
    assert_eq!(s.override_for(left.source()), Some(custom));
    assert_eq!(s.override_for(right.source()), None);

    let l = run(s.clone(), &[left], 0, 100);
    assert!((l[0].low - 0.1).abs() < 1e-6 && (l[0].high - 1.0).abs() < 1e-6);
    assert!(l[6].high > 0.0);
    assert_eq!((l[7].low, l[7].high), (0.0, 0.0));
    let r = run(s.clone(), &[right], 0, 100);
    assert!((r[0].low - COIL.low).abs() < 1e-6 && (r[0].high - COIL.high).abs() < 1e-6);
    assert!(r[59].low > 0.0);
    assert_eq!((r[60].low, r[60].high), (0.0, 0.0));

    // Muting one source leaves the others alone.
    let muted = settings().with_override(left.source(), RumbleEffect::NONE);
    assert!(run(muted.clone(), &[left], 0, 100).iter().all(|r| r.low == 0.0 && r.high == 0.0));
    assert!(run(muted, &[right], 0, 1)[0].low > 0.0);

    // Removing the override restores the kind's effect.
    assert!(s.clear_override(left.source()));
    assert!(!s.clear_override(left.source()));
    assert_eq!(bits(&run(s, &[left], 0, 100)), bits(&r));
}

#[test]
fn kinds_use_their_own_effect() {
    for k in 0..6u8 {
        let fx = effect_of(k);
        let e = kind_event(k, 3, 5, 1.0);
        let out = run(settings(), &[e], 0, 200);
        assert!((out[3].low - fx.low).abs() < 1e-6, "kind {k}: {:?}", out[3]);
        assert!((out[3].high - fx.high).abs() < 1e-6, "kind {k}: {:?}", out[3]);
        let end = 3 + fx.duration_ticks as usize;
        assert!(out[end - 1].low > 0.0, "kind {k}");
        assert_eq!((out[end].low, out[end].high), (0.0, 0.0), "kind {k}");
    }
}

#[test]
fn the_old_telemetry_event_is_accepted() {
    let old = HapticsTelemetryEvent { tick: 4, source: 77, strength: 1.0 };
    let mut a = RumbleMapper::new(settings());
    a.feed(old);
    let mut b = RumbleMapper::new(settings());
    b.feed(HapticEvent::from(old));
    for t in 4..80 {
        let (x, y) = (a.output_at(t), b.output_at(t));
        assert_eq!((x.low.to_bits(), x.high.to_bits()), (y.low.to_bits(), y.high.to_bits()));
    }
    let mut c = RumbleMapper::new(settings());
    c.feed(old);
    assert!(c.output_at(4).low > 0.0);
}

#[test]
fn a_rolling_ball_gives_a_steady_bounded_rumble_that_stops_with_the_ball() {
    let events: Vec<_> = (0..500u64).map(|t| kind_event(4, t, 0, 0.8)).collect();
    let out = run(settings(), &events, 0, 700);
    for r in &out[..500] {
        assert!((r.low - 0.8 * ROLL.low).abs() < 1e-6 && (r.high - 0.8 * ROLL.high).abs() < 1e-6);
    }
    let stop = 499 + ROLL.duration_ticks as usize;
    assert!(out[stop..].iter().all(|r| r.low == 0.0 && r.high == 0.0));
}

#[test]
fn reset_stops_everything() {
    let mut m = RumbleMapper::new(settings());
    m.feed(coil(0, 1, 1.0));
    m.feed(bumper(0, 1, 1.0));
    assert!(m.output_at(1).low > 0.0);
    m.reset();
    let r = m.output_at(2);
    assert_eq!((r.low, r.high), (0.0, 0.0));
}

#[test]
fn deterministic_over_10_runs_and_allocation_free() {
    let mut x = 0x9e37_79b9u32;
    let mut events = Vec::new();
    for t in 0..3000u64 {
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        if x % 5 == 0 {
            events.push(kind_event((x >> 8) as u8, t, (x >> 16) as u16 % 12, (x >> 4) as f32 % 1.3));
        }
        if x % 97 == 0 {
            for i in 0..50 {
                events.push(coil(t, i, 0.1 + i as f32 / 60.0));
            }
        }
    }
    let first = run(settings(), &events, 0, 3200);
    assert!(first.iter().any(|r| r.low > 0.0));
    assert!(first.iter().all(in_range));
    for _ in 0..9 {
        assert_eq!(bits(&run(settings(), &events, 0, 3200)), bits(&first));
    }

    let mut m = RumbleMapper::new(settings());
    let mut next = 0;
    let mut acc = 0.0f32;
    let before = allocs();
    for t in 0..3200u64 {
        while next < events.len() && events[next].tick <= t {
            m.feed(events[next]);
            next += 1;
        }
        acc += std::hint::black_box(m.output_at(t)).low;
    }
    let after = allocs();
    assert_eq!(after - before, 0, "feed/output_at allocated");
    assert!(acc > 0.0);
}

fn any_f32() -> impl Strategy<Value = f32> {
    prop_oneof![
        any::<f32>(),
        -2.0f32..3.0,
        Just(f32::NAN),
        Just(f32::INFINITY),
        Just(f32::NEG_INFINITY),
    ]
}

fn effect_strategy() -> impl Strategy<Value = RumbleEffect> {
    (any_f32(), any_f32(), prop_oneof![0u32..300, any::<u32>()])
        .prop_map(|(l, h, d)| RumbleEffect::new(l, h, d))
}

proptest! {
    /// Whatever the settings and events (NaN, infinities, huge durations,
    /// floods beyond the slot count, far-apart ticks): never panics, always
    /// finite and inside 0..=1, and identical on a second run.
    #[test]
    fn always_bounded_and_deterministic(
        fx in proptest::collection::vec(effect_strategy(), 6),
        master in any_f32(),
        overrides in proptest::collection::vec((0u16..16, effect_strategy()), 0..6),
        ops in proptest::collection::vec(
            (prop_oneof![0u64..3, 0u64..200, any::<u64>()], any::<u8>(), 0u16..16, any_f32(), 0usize..40),
            1..60),
    ) {
        let mut s = RumbleSettings::default();
        s.master = master;
        s.coil_fire = fx[0];
        s.eos_hit = fx[1];
        s.bumper = fx[2];
        s.ball_drop = fx[3];
        s.ball_roll = fx[4];
        s.impulse = fx[5];
        for (src, e) in overrides {
            s.set_override(src, e);
        }
        let go = |s: RumbleSettings| {
            let mut m = RumbleMapper::new(s);
            let mut t = 0u64;
            let mut out = Vec::new();
            for &(gap, k, n, strength, burst) in &ops {
                t = t.saturating_add(gap);
                for i in 0..burst {
                    m.feed(kind_event(k.wrapping_add(i as u8), t, n, strength));
                }
                out.push(m.output_at(t));
                out.push(m.output_at(t.saturating_add(1)));
                t = t.saturating_add(1);
            }
            out
        };
        let a = go(s.clone());
        for r in &a {
            prop_assert!(r.low.is_finite() && r.high.is_finite(), "{:?}", r);
            prop_assert!(in_range(r), "{:?}", r);
        }
        prop_assert_eq!(bits(&a), bits(&go(s)));
    }

    /// Up to a few overlapping events with sane settings: the output is the
    /// per-motor maximum of the single-event outputs, and everything is
    /// silent once the longest duration has passed.
    #[test]
    fn merge_is_the_maximum_of_the_parts(
        master in 0.0f32..=1.0,
        events in proptest::collection::vec((0u64..150, 0u8..6, 0u16..4, 0.0f32..=1.5), 1..12),
    ) {
        let mut s = settings();
        s.master = master;
        let mut evs: Vec<HapticEvent> =
            events.iter().map(|&(t, k, n, st)| kind_event(k, t, n, st)).collect();
        evs.sort_by_key(|e| e.tick);
        let all = run(s.clone(), &evs, 0, 260);
        let singles: Vec<Vec<Rumble>> =
            evs.iter().map(|e| run(s.clone(), std::slice::from_ref(e), 0, 260)).collect();
        for t in 0..260usize {
            let low = singles.iter().map(|o| o[t].low).fold(0.0f32, f32::max);
            let high = singles.iter().map(|o| o[t].high).fold(0.0f32, f32::max);
            prop_assert!((all[t].low - low).abs() < 1e-6, "tick {}: {} vs {}", t, all[t].low, low);
            prop_assert!((all[t].high - high).abs() < 1e-6, "tick {}", t);
            prop_assert!(in_range(&all[t]));
            // The documented linear envelope.
            let model = evs.iter().enumerate().map(|(i, e)| {
                envelope(e, effect_of(events_kind(&events, &evs, i)), master, t as u64)
            }).fold((0.0f32, 0.0f32), |a, b| (a.0.max(b.0), a.1.max(b.1)));
            prop_assert!((all[t].low - model.0).abs() < 1e-5, "tick {}: {} vs model {}", t, all[t].low, model.0);
            prop_assert!((all[t].high - model.1).abs() < 1e-5, "tick {}", t);
        }
        // Latest event at tick < 150, longest duration 80.
        for r in &all[230..] {
            prop_assert_eq!((r.low, r.high), (0.0, 0.0));
        }
    }
}

/// The kind index (0..6) of the `i`-th sorted event.
fn events_kind(_raw: &[(u64, u8, u16, f32)], sorted: &[HapticEvent], i: usize) -> u8 {
    match sorted[i].kind {
        HapticKind::CoilFire { .. } => 0,
        HapticKind::EosHit { .. } => 1,
        HapticKind::Bumper { .. } => 2,
        HapticKind::BallDrop { .. } => 3,
        HapticKind::BallRoll { .. } => 4,
        _ => 5,
    }
}
