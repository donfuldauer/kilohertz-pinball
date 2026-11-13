//! Subphase 3.4 verification 1 (Implementer tests): the four-corner panner
//! law along the PLAN path, equal power everywhere, and the SSF voice mixer
//! (allocation-free, bounded, deterministic, panned by position).

use pin_audio::ssf::{
    CH_BL, CH_BR, CH_FC, CH_FL, CH_FR, CH_LFE, CH_SL, CH_SR, SsfConfig, SsfMixer, SsfPanner,
};
use pin_core::haptics::{HapticEvent, HapticKind};
use pin_core::{CoilId, FlipperId};
use proptest::prelude::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::f64::consts::FRAC_PI_2;

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

/// The law, computed independently in f64.
fn law(x: f64, y: f64) -> [f64; 4] {
    let u = ((x + 257.0) / 514.0).clamp(0.0, 1.0) * FRAC_PI_2;
    let w = (y / 1070.0).clamp(0.0, 1.0) * FRAC_PI_2;
    [
        u.cos() * w.cos(),
        u.sin() * w.cos(),
        u.cos() * w.sin(),
        u.sin() * w.sin(),
    ]
}

fn panner() -> SsfPanner {
    SsfPanner::new(-257.0, 257.0, 0.0, 1070.0)
}

#[test]
fn counter_detects_allocation() {
    let before = allocs();
    let v = std::hint::black_box(vec![1u8; 64]);
    assert!(allocs() > before);
    drop(v);
}

#[test]
fn path_matches_the_law_and_is_continuous() {
    let p = panner();
    const N: usize = 100;
    let mut prev: Option<[f32; 8]> = None;
    for i in 0..N {
        let t = i as f64 / (N - 1) as f64;
        let x = -150.0 + 300.0 * t;
        let y = 100.0 + 700.0 * t;
        let g = p.gains(x as f32, y as f32);
        let want = law(f64::from(x as f32), f64::from(y as f32));
        for (ch, w) in [CH_SL, CH_SR, CH_BL, CH_BR].into_iter().zip(want) {
            assert!((f64::from(g[ch]) - w).abs() < 1e-6, "point {i} ch {ch}");
        }
        for ch in [CH_FL, CH_FR, CH_FC, CH_LFE] {
            assert_eq!(g[ch], 0.0);
        }
        let sq: f32 = g.iter().map(|v| v * v).sum();
        assert!((sq - 1.0).abs() < 1e-5);
        if let Some(q) = prev {
            for ch in 0..8 {
                assert!((g[ch] - q[ch]).abs() <= 0.05, "step at {i} ch {ch}");
            }
        }
        if i == 0 {
            assert!(
                g.iter().all(|&v| v <= g[CH_SL]),
                "SL dominates at the start"
            );
        }
        if i == N - 1 {
            assert!(g.iter().all(|&v| v <= g[CH_BR]), "BR dominates at the end");
        }
        prev = Some(g);
    }
}

proptest! {
    #[test]
    fn equal_power_everywhere(x in -2000.0f32..2000.0, y in -2000.0f32..3000.0) {
        let g = panner().gains(x, y);
        let sq: f32 = g.iter().map(|v| v * v).sum();
        prop_assert!((sq - 1.0).abs() < 1e-5);
        prop_assert!(g.iter().all(|v| (0.0..=1.0).contains(v)));
        let want = law(f64::from(x), f64::from(y));
        for (ch, w) in [CH_SL, CH_SR, CH_BL, CH_BR].into_iter().zip(want) {
            prop_assert!((f64::from(g[ch]) - w).abs() < 1e-6);
        }
    }

    #[test]
    fn mixer_output_bounded_for_any_events(
        evs in prop::collection::vec(
            (0u8..6, any::<u16>(), -1.0f32..2.0, -5000.0f32..5000.0, -5000.0f32..5000.0, -1.0f32..20.0),
            0..40),
    ) {
        let mut m = SsfMixer::new(SsfConfig::default());
        let mut out = vec![0.0f32; 8 * 256];
        for (k, id, s, x, y, v) in evs {
            let kind = match k {
                0 => HapticKind::CoilFire { coil: CoilId(id) },
                1 => HapticKind::EosHit { flipper: FlipperId(id) },
                2 => HapticKind::Bumper { mech: id },
                3 => HapticKind::BallDrop { ball: id, position_mm: [x, y, 0.0] },
                4 => HapticKind::BallRoll { ball: id % 20, position_mm: [x, y, 0.0], mass_kg: v / 10.0, speed_m_s: v },
                _ => HapticKind::Impulse { source: id },
            };
            m.feed(HapticEvent::new(0, kind, s));
            m.render(&mut out);
            prop_assert!(out.iter().all(|v| v.is_finite() && (-1.0..=1.0).contains(v)));
            prop_assert!(out.chunks_exact(8).all(|f| f[CH_FL] == 0.0 && f[CH_FR] == 0.0 && f[CH_FC] == 0.0 && f[CH_LFE] == 0.0));
        }
    }
}

fn roll(ball: u16, x: f32, y: f32, speed: f32) -> HapticEvent {
    HapticEvent::new(
        0,
        HapticKind::BallRoll {
            ball,
            position_mm: [x, y, 25.0],
            mass_kg: 0.0806,
            speed_m_s: speed,
        },
        1.0,
    )
}

fn rms(buf: &[f32], ch: usize) -> f64 {
    let n = buf.len() / 8;
    (buf.chunks_exact(8)
        .map(|f| f64::from(f[ch]).powi(2))
        .sum::<f64>()
        / n as f64)
        .sqrt()
}

#[test]
fn roll_gain_grows_with_speed_and_follows_the_ball() {
    let level = |speed: f32| {
        let mut m = SsfMixer::new(SsfConfig::default());
        let mut out = vec![0.0f32; 8 * 1200];
        m.feed(roll(0, 0.0, 0.0, speed));
        m.render(&mut out);
        rms(&out[8 * 600..], CH_SL) + rms(&out[8 * 600..], CH_SR)
    };
    let (slow, fast) = (level(0.5), level(2.5));
    assert!(slow > 0.0 && fast > 2.0 * slow, "slow {slow} fast {fast}");
    assert_eq!(level(0.0), 0.0);

    // The ball rolls from the front left to the back right, one event per
    // millisecond (48 frames).
    let mut m = SsfMixer::new(SsfConfig::default());
    let mut out = vec![0.0f32; 8 * 48];
    let (mut first, mut last) = ([0.0; 4], [0.0; 4]);
    for i in 0..1000 {
        let t = i as f32 / 999.0;
        m.feed(roll(3, -200.0 + 400.0 * t, 50.0 + 950.0 * t, 2.0));
        m.render(&mut out);
        let e = [CH_SL, CH_SR, CH_BL, CH_BR].map(|c| rms(&out, c));
        if i == 100 {
            first = e;
        }
        if i == 999 {
            last = e;
        }
    }
    assert!(first[0] > 3.0 * first[3], "front-left first: {first:?}");
    assert!(last[3] > 3.0 * last[0], "back-right last: {last:?}");
}

#[test]
fn render_and_feed_never_allocate_and_hash_is_stable() {
    fn run(check_allocs: bool) -> u64 {
        let mut m = SsfMixer::new(SsfConfig::default());
        m.set_source_position(5, -200.0, 100.0);
        let mut out = vec![0.0f32; 8 * 48];
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        let before = allocs();
        for t in 0..2000u64 {
            let x = (t as f32 * 0.3).sin() * 250.0;
            m.feed(roll(
                (t % 3) as u16,
                x,
                (t % 1000) as f32,
                (t % 300) as f32 / 100.0,
            ));
            if t.is_multiple_of(37) {
                m.feed(HapticEvent::new(
                    t,
                    HapticKind::CoilFire { coil: CoilId(5) },
                    0.8,
                ));
            }
            if t.is_multiple_of(91) {
                m.feed(HapticEvent::new(t, HapticKind::Bumper { mech: 1 }, 1.0));
            }
            m.render(&mut out);
            for v in &out {
                h = (h ^ u64::from(v.to_bits())).wrapping_mul(0x100_0000_01b3);
            }
        }
        if check_allocs {
            assert_eq!(allocs(), before, "SSF render/feed allocated");
        }
        h
    }
    let first = run(true);
    for _ in 0..9 {
        assert_eq!(run(false), first);
    }
}
