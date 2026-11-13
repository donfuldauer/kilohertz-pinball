//! Independent Tester spec tests, Subphase 3.4, verification criterion 4:
//! the lock-free haptics ring (allocation-free push, drop-newest with a
//! counter, in-order SPSC delivery across threads). Black box, from the brief.

use pin_core::haptics::{HapticEvent, HapticKind};
use pin_core::ids::{CoilId, FlipperId};
use pin_core::input::HapticsTelemetryEvent;
use pin_input::haptics::{HapticsBus, HapticsConsumer, HapticsProducer, haptics_ring};
use proptest::prelude::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::VecDeque;

// ---------------------------------------------------------------- allocator

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

// ----------------------------------------------------------------- adapters
type Ev = HapticEvent;

/// A distinguishable event number `i`, cycling through every kind.
fn ev(i: u64) -> Ev {
    let n = (i % 4096) as u16;
    let p = [i as f32, -(i as f32) * 0.5, (i % 7) as f32];
    let kind = match i % 6 {
        0 => HapticKind::CoilFire { coil: CoilId(n) },
        1 => HapticKind::EosHit { flipper: FlipperId(n) },
        2 => HapticKind::Bumper { mech: n },
        3 => HapticKind::BallDrop { ball: n, position_mm: p },
        4 => HapticKind::BallRoll {
            ball: n,
            position_mm: p,
            mass_kg: 0.08,
            speed_m_s: (i % 300) as f32 / 100.0,
        },
        _ => HapticKind::Impulse { source: n },
    };
    HapticEvent::new(i, kind, (i % 101) as f32 / 100.0)
}

fn dropped(tx: &HapticsProducer, rx: &HapticsConsumer) -> u64 {
    assert_eq!(tx.dropped(), rx.dropped(), "both ends report the same drop count");
    tx.dropped()
}

/// Pushes until the ring refuses; returns how many it took.
fn fill(tx: &mut HapticsProducer, start: u64, limit: u64) -> u64 {
    let mut n = 0;
    while n < limit && tx.push(ev(start + n)) {
        n += 1;
    }
    n
}

#[test]
fn fifo_order_on_one_thread() {
    let (mut tx, mut rx) = haptics_ring(64);
    assert_eq!(rx.pop(), None);
    for i in 0..40 {
        assert!(tx.push(ev(i)));
    }
    for i in 0..40 {
        assert_eq!(rx.pop(), Some(ev(i)));
    }
    assert_eq!(rx.pop(), None);
    assert_eq!(dropped(&tx, &rx), 0);
}

#[test]
fn full_ring_drops_the_newest_and_counts() {
    for capacity in [1usize, 2, 3, 8, 100, 1024] {
        let (mut tx, mut rx) = haptics_ring(capacity);
        let held = fill(&mut tx, 0, 1_000_000);
        assert!(
            held >= capacity as u64 && held < 1_000_000,
            "capacity {capacity}: ring accepted {held} events"
        );
        assert_eq!(dropped(&tx, &rx), 1, "the refused push that ended the fill");
        for k in 0..37u64 {
            assert!(!tx.push(ev(5_000_000 + k)), "push on a full ring must return false");
            assert_eq!(dropped(&tx, &rx), 2 + k);
        }
        // The oldest events survive, in order; the dropped ones never appear.
        for i in 0..held {
            assert_eq!(rx.pop(), Some(ev(i)), "capacity {capacity}, item {i}");
        }
        assert_eq!(rx.pop(), None);
        // Space is usable again and the counter keeps its value.
        assert!(tx.push(ev(77)));
        assert_eq!(rx.pop(), Some(ev(77)));
        assert_eq!(dropped(&tx, &rx), 38);
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
fn push_and_pop_do_not_allocate() {
    let (mut tx, mut rx) = haptics_ring(256);
    let before = allocs();
    let mut accepted = 0u64;
    let mut refused = 0u64;
    let mut popped = 0u64;
    for i in 0..100_000u64 {
        if std::hint::black_box(tx.push(ev(i))) {
            accepted += 1;
        } else {
            refused += 1;
        }
        // Drain in bursts so the ring is sometimes full, sometimes empty.
        if i % 1000 < 300 {
            while std::hint::black_box(rx.pop()).is_some() {
                popped += 1;
            }
        }
    }
    let d = dropped(&tx, &rx);
    let after = allocs();
    assert_eq!(after - before, 0, "push/pop/dropped allocated");
    assert!(refused > 0 && accepted > 0);
    assert_eq!(d, refused);
    while rx.pop().is_some() {
        popped += 1;
    }
    assert_eq!(popped, accepted);
}

#[test]
fn cross_thread_stress_delivers_in_order() {
    const N: u64 = 400_000;
    for capacity in [1usize, 7, 64, 4096] {
        let (mut tx, mut rx) = haptics_ring(capacity);
        let producer = std::thread::spawn(move || {
            let before = allocs();
            let mut i = 0u64;
            let mut refused = 0u64;
            while i < N {
                // Lossless stream: retry a refused event (the sim would drop it;
                // here the retry lets the consumer check for gaps).
                if tx.push(ev(i)) {
                    i += 1;
                } else {
                    refused += 1;
                    std::hint::spin_loop();
                }
            }
            let allocated = allocs() - before;
            (tx, refused, allocated)
        });
        let mut next = 0u64;
        while next < N {
            match rx.pop() {
                Some(e) => {
                    assert_eq!(e, ev(next), "capacity {capacity}: out of order or corrupted");
                    next += 1;
                }
                None => std::thread::yield_now(),
            }
        }
        let (tx, refused, allocated) = producer.join().unwrap();
        assert_eq!(rx.pop(), None);
        assert_eq!(dropped(&tx, &rx), refused, "every refused push is counted once");
        assert_eq!(allocated, 0, "producer thread allocated while pushing");
    }
}

#[test]
fn cross_thread_drop_newest_keeps_an_ordered_subsequence() {
    // The producer never retries (as the sim thread would not): whatever
    // arrives is strictly increasing, and arrivals + drops == pushes.
    const N: u64 = 300_000;
    let (mut tx, mut rx) = haptics_ring(32);
    let producer = std::thread::spawn(move || {
        let mut accepted = 0u64;
        for i in 0..N {
            if tx.push(ev(i)) {
                accepted += 1;
            }
        }
        (tx, accepted)
    });
    let mut last: Option<u64> = None;
    let mut received = 0u64;
    let mut check = |e: Ev| {
        assert_eq!(e, ev(e.tick));
        if let Some(l) = last {
            assert!(e.tick > l, "event {} after {l}", e.tick);
        }
        last = Some(e.tick);
        received += 1;
    };
    while !producer.is_finished() {
        match rx.pop() {
            Some(e) => check(e),
            None => std::thread::yield_now(),
        }
    }
    let (tx, accepted) = producer.join().unwrap();
    while let Some(e) = rx.pop() {
        check(e);
    }
    assert_eq!(received, accepted);
    assert_eq!(dropped(&tx, &rx), N - accepted);
}

#[test]
fn the_old_event_type_still_travels_through_a_ring() {
    let (mut tx, mut rx) = haptics_ring::<HapticsTelemetryEvent>(4);
    let e = |i: u64| HapticsTelemetryEvent { tick: i, source: i as u16, strength: 0.5 };
    let mut held = 0;
    while held < 1000 && tx.push(e(held)) {
        held += 1;
    }
    assert!((4..1000).contains(&held));
    assert_eq!(tx.dropped(), 1);
    for i in 0..held {
        assert_eq!(rx.pop(), Some(e(i)));
    }
    assert_eq!(rx.pop(), None);
    // And it converts into the rich event without losing anything.
    let rich = HapticEvent::from(e(9));
    assert_eq!(rich.to_telemetry_event(), Some(e(9)));
}

#[test]
fn bus_fans_out_every_event_to_every_sink_in_order() {
    let mut bus: HapticsBus = HapticsBus::new();
    let mut sinks: Vec<HapticsConsumer> = (0..3).map(|_| bus.add_sink(512).unwrap()).collect();
    assert_eq!(bus.sinks(), 3);
    let before = allocs();
    for i in 0..500 {
        assert!(bus.push(ev(i)));
    }
    assert_eq!(allocs() - before, 0, "bus push allocated");
    assert_eq!(bus.dropped(), 0);
    for rx in &mut sinks {
        for i in 0..500 {
            assert_eq!(rx.pop(), Some(ev(i)));
        }
        assert_eq!(rx.pop(), None);
    }
}

#[test]
fn bus_counts_drops_when_a_sink_is_full_without_starving_the_others() {
    let mut bus: HapticsBus = HapticsBus::new();
    let mut small = bus.add_sink(4).unwrap();
    let mut large = bus.add_sink(4096).unwrap();
    for i in 0..1000 {
        bus.push(ev(i));
    }
    assert!(bus.dropped() > 0, "the small sink overflowed");
    for i in 0..1000 {
        assert_eq!(large.pop(), Some(ev(i)), "the large sink must not lose event {i}");
    }
    let mut n = 0;
    while let Some(e) = small.pop() {
        assert_eq!(e, ev(n), "the small sink keeps the oldest events");
        n += 1;
    }
    assert!((4..1000).contains(&n));
}

proptest! {
    /// Against a model queue: push succeeds exactly when the model has room
    /// (the ring's real capacity, learnt by filling it once), pops match,
    /// and the drop counter equals the number of refused pushes.
    #[test]
    fn behaves_like_a_bounded_queue(
        capacity in 1usize..40,
        ops in proptest::collection::vec(any::<bool>(), 0..400),
    ) {
        let (mut tx, mut rx) = haptics_ring(capacity);
        let real = fill(&mut tx, 0, 10_000) as usize;
        prop_assert!(real >= capacity && real < 10_000);
        while rx.pop().is_some() {}
        let mut drops = 1u64;
        let mut model = VecDeque::new();
        for (i, push) in ops.into_iter().enumerate() {
            if push {
                let ok = tx.push(ev(i as u64));
                prop_assert_eq!(ok, model.len() < real);
                if ok { model.push_back(ev(i as u64)); } else { drops += 1; }
            } else {
                prop_assert_eq!(rx.pop(), model.pop_front());
            }
            prop_assert_eq!(dropped(&tx, &rx), drops);
        }
        while let Some(e) = model.pop_front() {
            prop_assert_eq!(rx.pop(), Some(e));
        }
        prop_assert_eq!(rx.pop(), None);
    }
}
