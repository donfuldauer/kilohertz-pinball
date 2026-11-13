//! Subphase 3.4 verification 4 and 5 (Implementer tests): the haptics ring
//! is allocation-free and wait-free on the producer side, drops and counts on
//! a full ring, delivers in order across threads; the rumble mapper is
//! deterministic, bounded, decays and merges by max.

use pin_core::haptics::{HapticEvent, HapticKind};
use pin_core::input::HapticsTelemetryEvent;
use pin_core::{CoilId, FlipperId};
use pin_input::haptics::{
    HapticsBus, MAX_ACTIVE_RUMBLES, Rumble, RumbleEffect, RumbleMapper, RumbleSettings,
    haptics_ring,
};
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

fn roll(tick: u64, ball: u16) -> HapticEvent {
    HapticEvent::new(
        tick,
        HapticKind::BallRoll {
            ball,
            position_mm: [tick as f32, 2.0, 3.0],
            mass_kg: 0.08,
            speed_m_s: 1.5,
        },
        0.5,
    )
}

fn kind_of(i: u64) -> HapticKind {
    match i % 5 {
        0 => HapticKind::CoilFire {
            coil: CoilId(i as u16),
        },
        1 => HapticKind::EosHit {
            flipper: FlipperId(i as u16 % 4),
        },
        2 => HapticKind::Bumper { mech: i as u16 % 3 },
        3 => HapticKind::BallDrop {
            ball: 0,
            position_mm: [0.0; 3],
        },
        _ => HapticKind::BallRoll {
            ball: 1,
            position_mm: [0.0; 3],
            mass_kg: 0.08,
            speed_m_s: 2.0,
        },
    }
}

#[test]
fn counter_detects_allocation() {
    let before = allocs();
    let v = std::hint::black_box(vec![1u8; 64]);
    assert!(allocs() > before);
    drop(v);
}

#[test]
fn producer_and_consumer_never_allocate() {
    let (mut tx, mut rx) = haptics_ring::<HapticEvent>(64);
    let mut bus = HapticsBus::<HapticEvent>::new();
    let mut a = bus.add_sink(16).unwrap();
    let mut mapper = RumbleMapper::new(RumbleSettings::default());
    let before = allocs();
    for t in 0..10_000u64 {
        let e = HapticEvent::new(t, kind_of(t), 0.7);
        tx.push(e);
        bus.push(e);
        if t % 3 == 0 {
            while let Some(e) = rx.pop() {
                mapper.feed(e);
            }
            while a.pop().is_some() {}
            let r = mapper.output_at(t);
            assert!(r.low <= 1.0 && r.high <= 1.0);
        }
    }
    // A full ring's drop path does not allocate either.
    for t in 0..200 {
        tx.push(roll(t, 0));
        bus.push(roll(t, 0));
    }
    assert_eq!(allocs(), before, "haptics hot path allocated");
    assert!(tx.dropped() > 0);
    assert!(bus.dropped() > 0);
}

#[test]
fn full_ring_drops_newest_and_counts() {
    let (mut tx, mut rx) = haptics_ring(4);
    for t in 0..10 {
        assert_eq!(tx.push(roll(t, 0)), t < 4);
    }
    assert_eq!(tx.dropped(), 6);
    assert_eq!(rx.dropped(), 6);
    let got: Vec<u64> = std::iter::from_fn(|| rx.pop()).map(|e| e.tick).collect();
    assert_eq!(got, vec![0, 1, 2, 3]);
    // The legacy event type works through the same ring.
    let (mut tx, mut rx) = haptics_ring(2);
    let old = HapticsTelemetryEvent {
        tick: 1,
        source: 2,
        strength: 0.5,
    };
    assert!(tx.push(old));
    assert_eq!(rx.pop(), Some(old));
}

#[test]
fn cross_thread_stress_in_order() {
    const N: u64 = 200_000;
    let (mut tx, mut rx) = haptics_ring(64);
    let producer = std::thread::spawn(move || {
        let mut pushed = 0u64;
        let mut t = 0u64;
        while t < N {
            if tx.push(roll(t, (t % 16) as u16)) {
                pushed += 1;
                t += 1;
            } else {
                std::hint::spin_loop();
            }
        }
        (pushed, tx.dropped())
    });
    let mut next = 0u64;
    while next < N {
        if let Some(e) = rx.pop() {
            assert_eq!(e, roll(next, (next % 16) as u16));
            next += 1;
        } else {
            std::hint::spin_loop();
        }
    }
    let (pushed, dropped) = producer.join().unwrap();
    assert_eq!(pushed, N);
    // Retried pushes were counted as drops; every event still arrived once.
    assert_eq!(rx.dropped(), dropped);
    assert_eq!(rx.pop(), None);
}

/// The output of a single event in isolation.
fn alone(settings: &RumbleSettings, e: HapticEvent, tick: u64) -> Rumble {
    let mut m = RumbleMapper::new(settings.clone());
    m.feed(e);
    m.output_at(tick)
}

fn event() -> impl Strategy<Value = HapticEvent> {
    (0u64..400, 0u64..5, -0.5f32..1.5).prop_map(|(t, k, s)| HapticEvent::new(t, kind_of(k), s))
}

proptest! {
    #[test]
    fn mapper_deterministic_bounded_max_merge(
        mut events in prop::collection::vec(event(), 0..MAX_ACTIVE_RUMBLES),
        master in 0.0f32..2.0,
    ) {
        events.sort_by_key(|e| e.tick);
        let settings = RumbleSettings { master, ..RumbleSettings::default() };
        let run = || {
            let mut m = RumbleMapper::new(settings.clone());
            let mut out = Vec::new();
            let mut i = 0;
            for t in 0..600u64 {
                while i < events.len() && events[i].tick <= t {
                    m.feed(events[i]);
                    i += 1;
                }
                out.push(m.output_at(t));
            }
            out
        };
        let a = run();
        prop_assert_eq!(&a, &run());
        for (t, r) in a.iter().enumerate() {
            prop_assert!((0.0..=1.0).contains(&r.low) && (0.0..=1.0).contains(&r.high));
            // Max merge: equal to the max of each event alone.
            let mut low = 0.0f32;
            let mut high = 0.0f32;
            for e in &events {
                let s = alone(&settings, *e, t as u64);
                low = low.max(s.low);
                high = high.max(s.high);
            }
            prop_assert_eq!(r.low, low);
            prop_assert_eq!(r.high, high);
        }
        // Everything decays to zero after the longest duration (80 ticks).
        prop_assert_eq!(a[599], Rumble::OFF);
    }

    #[test]
    fn decays_to_zero_exactly_after_duration(
        dur in 1u32..2000, t0 in 0u64..1_000_000, s in 0.01f32..1.0,
    ) {
        let settings = RumbleSettings::default()
            .with_override(7, RumbleEffect::new(1.0, 0.5, dur));
        let mut m = RumbleMapper::new(settings);
        m.feed(HapticEvent::new(t0, HapticKind::CoilFire { coil: CoilId(7) }, s));
        let peak = m.output_at(t0);
        prop_assert!((peak.low - s).abs() <= 1e-6);
        let mut prev = peak.low;
        for t in (t0 + 1)..(t0 + u64::from(dur)) {
            let r = m.output_at(t);
            prop_assert!(r.low > 0.0 && r.low <= prev);
            prev = r.low;
        }
        prop_assert_eq!(m.output_at(t0 + u64::from(dur)), Rumble::OFF);
        prop_assert_eq!(m.output_at(t0 + u64::from(dur) + 1000), Rumble::OFF);
    }
}
