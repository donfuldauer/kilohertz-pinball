//! Black-box spec tests for `pin_core::spsc` (Subphase 1.3 brief).
//!
//! Contract under test:
//! - `channel::<T: Copy + Send>(capacity)` returns a producer/consumer pair;
//! - `push` returns `Err(v)` (handing the value back) when full, never blocks;
//! - `pop` returns items in FIFO order, `None` when empty;
//! - `len` is the number of queued items; `capacity` is exact (>= 1);
//! - both halves are `Send`.
//!
//! No test sleeps or looks at a wall clock: the cross-thread tests spin with
//! `yield_now` and terminate on item counts only.

use pin_core::input::{CabinetAxis, CabinetInput, InputEvent, InputEventKind};
use pin_core::spsc::{SpscConsumer, SpscProducer, channel};
use proptest::prelude::*;
use std::collections::VecDeque;
use std::thread;

const TRANSFER: u64 = 100_000;

fn assert_send<T: Send>() {}

/// An event whose every field is derived from its sequence number, so both
/// reordering and corruption of the timestamp are detectable.
fn event_for(seq: u64) -> InputEvent {
    let timestamp_ns = seq.wrapping_mul(1_000_003).wrapping_add(0xDEAD_BEEF_0000_0000);
    if seq.is_multiple_of(3) {
        let axis = CabinetAxis::ALL[(seq as usize / 3) % CabinetAxis::ALL.len()];
        InputEvent::axis(timestamp_ns, axis, (seq % 2001) as f32 / 1000.0 - 1.0)
    } else {
        let input = CabinetInput::ALL[seq as usize % CabinetInput::ALL.len()];
        InputEvent::button(timestamp_ns, input, seq.is_multiple_of(2))
    }
}

// ---------------------------------------------------------------------------
// Static properties
// ---------------------------------------------------------------------------

#[test]
fn both_halves_are_send() {
    assert_send::<SpscProducer<u64>>();
    assert_send::<SpscConsumer<u64>>();
    assert_send::<SpscProducer<InputEvent>>();
    assert_send::<SpscConsumer<InputEvent>>();
    assert_send::<(SpscProducer<[u8; 64]>, SpscConsumer<[u8; 64]>)>();
}

#[test]
fn channel_signature_matches_the_brief() {
    let _f: fn(usize) -> (SpscProducer<u32>, SpscConsumer<u32>) = channel::<u32>;
    let _push: fn(&mut SpscProducer<u32>, u32) -> Result<(), u32> = SpscProducer::<u32>::push;
    let _pop: fn(&mut SpscConsumer<u32>) -> Option<u32> = SpscConsumer::<u32>::pop;
    let _len: fn(&SpscConsumer<u32>) -> usize = SpscConsumer::<u32>::len;
}

// ---------------------------------------------------------------------------
// Single-threaded behaviour
// ---------------------------------------------------------------------------

#[test]
fn new_channel_is_empty() {
    let (_tx, mut rx) = channel::<u32>(8);
    assert_eq!(rx.len(), 0);
    assert_eq!(rx.pop(), None);
    assert_eq!(rx.pop(), None);
    assert_eq!(rx.len(), 0);
}

#[test]
fn fifo_order_is_preserved() {
    let (mut tx, mut rx) = channel::<u32>(16);
    for v in [7, 3, 9, 1, 1, 0, u32::MAX, 42] {
        assert_eq!(tx.push(v), Ok(()));
    }
    assert_eq!(rx.len(), 8);
    for v in [7, 3, 9, 1, 1, 0, u32::MAX, 42] {
        assert_eq!(rx.pop(), Some(v));
    }
    assert_eq!(rx.pop(), None);
    assert_eq!(rx.len(), 0);
}

#[test]
fn len_tracks_pushes_and_pops() {
    let (mut tx, mut rx) = channel::<u8>(4);
    assert_eq!(rx.len(), 0);
    tx.push(1).unwrap();
    assert_eq!(rx.len(), 1);
    tx.push(2).unwrap();
    tx.push(3).unwrap();
    assert_eq!(rx.len(), 3);
    assert_eq!(rx.pop(), Some(1));
    assert_eq!(rx.len(), 2);
    tx.push(4).unwrap();
    tx.push(5).unwrap();
    assert_eq!(rx.len(), 4);
    assert_eq!(rx.pop(), Some(2));
    assert_eq!(rx.pop(), Some(3));
    assert_eq!(rx.pop(), Some(4));
    assert_eq!(rx.len(), 1);
    assert_eq!(rx.pop(), Some(5));
    assert_eq!(rx.len(), 0);
}

/// Capacity is exact: exactly `capacity` pushes succeed on an empty ring, the
/// next is refused. Covers powers of two and their neighbours, because rings
/// that round up internally or keep one slot empty get exactly these wrong.
#[test]
fn capacity_is_exact() {
    for capacity in [1usize, 2, 3, 4, 5, 7, 8, 9, 15, 16, 17, 31, 33, 100, 1000, 1024, 1025] {
        let (mut tx, mut rx) = channel::<usize>(capacity);
        for i in 0..capacity {
            assert_eq!(tx.push(i), Ok(()), "capacity {capacity}: push #{i} must fit");
            assert_eq!(rx.len(), i + 1, "capacity {capacity}");
        }
        assert_eq!(
            tx.push(usize::MAX),
            Err(usize::MAX),
            "capacity {capacity}: push #{capacity} must be refused"
        );
        assert_eq!(rx.len(), capacity, "capacity {capacity}: len after refusal");
        for i in 0..capacity {
            assert_eq!(rx.pop(), Some(i), "capacity {capacity}");
        }
        assert_eq!(rx.pop(), None, "capacity {capacity}: refused value must not appear");
    }
}

#[test]
fn full_push_returns_the_rejected_value_and_leaves_contents_untouched() {
    let (mut tx, mut rx) = channel::<(u64, u64)>(3);
    tx.push((1, 10)).unwrap();
    tx.push((2, 20)).unwrap();
    tx.push((3, 30)).unwrap();
    // Repeated refusals each hand back exactly the value offered.
    assert_eq!(tx.push((4, 40)), Err((4, 40)));
    assert_eq!(tx.push((5, 50)), Err((5, 50)));
    assert_eq!(tx.push((6, 60)), Err((6, 60)));
    assert_eq!(rx.len(), 3);
    assert_eq!(rx.pop(), Some((1, 10)));
    assert_eq!(rx.pop(), Some((2, 20)));
    assert_eq!(rx.pop(), Some((3, 30)));
    assert_eq!(rx.pop(), None);
}

#[test]
fn one_pop_frees_exactly_one_slot() {
    let (mut tx, mut rx) = channel::<u32>(4);
    for i in 0..4 {
        tx.push(i).unwrap();
    }
    assert_eq!(tx.push(99), Err(99));
    assert_eq!(rx.pop(), Some(0));
    assert_eq!(tx.push(4), Ok(()));
    assert_eq!(tx.push(5), Err(5));
    assert_eq!(rx.len(), 4);
    for i in 1..=4 {
        assert_eq!(rx.pop(), Some(i));
    }
    assert_eq!(rx.pop(), None);
}

#[test]
fn capacity_one_alternates() {
    let (mut tx, mut rx) = channel::<u64>(1);
    for i in 0..10_000u64 {
        assert_eq!(rx.pop(), None);
        assert_eq!(tx.push(i), Ok(()));
        assert_eq!(tx.push(i + 1), Err(i + 1));
        assert_eq!(rx.len(), 1);
        assert_eq!(rx.pop(), Some(i));
        assert_eq!(rx.len(), 0);
    }
}

/// Many times round the ring with a varying fill level: order, exact capacity
/// and `len` must hold at every wrap position.
#[test]
fn wraparound_keeps_order_capacity_and_len() {
    for capacity in [1usize, 2, 3, 5, 8, 13] {
        let (mut tx, mut rx) = channel::<u64>(capacity);
        let mut next_in = 0u64;
        let mut next_out = 0u64;
        for round in 0..(capacity * 50) {
            let burst = round % (capacity + 2);
            for _ in 0..burst {
                let queued = (next_in - next_out) as usize;
                let r = tx.push(next_in);
                if queued < capacity {
                    assert_eq!(r, Ok(()), "capacity {capacity}");
                    next_in += 1;
                } else {
                    assert_eq!(r, Err(next_in), "capacity {capacity}");
                }
            }
            assert_eq!(rx.len(), (next_in - next_out) as usize);
            let drain = (round * 7 + 1) % (capacity + 1);
            for _ in 0..drain {
                if next_out < next_in {
                    assert_eq!(rx.pop(), Some(next_out), "capacity {capacity}");
                    next_out += 1;
                } else {
                    assert_eq!(rx.pop(), None, "capacity {capacity}");
                }
            }
            assert_eq!(rx.len(), (next_in - next_out) as usize);
        }
        while next_out < next_in {
            assert_eq!(rx.pop(), Some(next_out));
            next_out += 1;
        }
        assert_eq!(rx.pop(), None);
    }
}

#[test]
fn input_events_round_trip_with_timestamps_intact() {
    let (mut tx, mut rx) = channel::<InputEvent>(64);
    for seq in 0..64 {
        tx.push(event_for(seq)).unwrap();
    }
    let rejected = event_for(64);
    assert_eq!(tx.push(rejected), Err(rejected));
    for seq in 0..64 {
        let got = rx.pop().expect("queued event");
        assert_eq!(got, event_for(seq));
        assert_eq!(got.timestamp_ns, event_for(seq).timestamp_ns);
    }
    assert_eq!(rx.pop(), None);
}

#[test]
fn extreme_timestamps_survive() {
    let (mut tx, mut rx) = channel::<InputEvent>(4);
    let a = InputEvent::button(0, CabinetInput::ALL[0], true);
    let b = InputEvent::button(u64::MAX, CabinetInput::ALL[1], false);
    let c = InputEvent::axis(u64::MAX - 1, CabinetAxis::ALL[0], -1.0);
    tx.push(a).unwrap();
    tx.push(b).unwrap();
    tx.push(c).unwrap();
    assert_eq!(rx.pop(), Some(a));
    assert_eq!(rx.pop(), Some(b));
    let got = rx.pop().unwrap();
    assert_eq!(got.timestamp_ns, u64::MAX - 1);
    assert!(matches!(got.kind, InputEventKind::Axis { .. }));
    assert_eq!(got, c);
}

#[test]
fn works_with_large_and_zero_sized_payloads() {
    let (mut tx, mut rx) = channel::<[u64; 32]>(3);
    for n in 0..3u64 {
        tx.push([n; 32]).unwrap();
    }
    assert_eq!(tx.push([9; 32]), Err([9; 32]));
    for n in 0..3u64 {
        assert_eq!(rx.pop(), Some([n; 32]));
    }

    let (mut tx, mut rx) = channel::<()>(2);
    assert_eq!(tx.push(()), Ok(()));
    assert_eq!(tx.push(()), Ok(()));
    assert_eq!(tx.push(()), Err(()));
    assert_eq!(rx.len(), 2);
    assert_eq!(rx.pop(), Some(()));
    assert_eq!(rx.pop(), Some(()));
    assert_eq!(rx.pop(), None);
}

#[test]
fn independent_channels_do_not_share_state() {
    let (mut tx_a, mut rx_a) = channel::<u32>(2);
    let (mut tx_b, mut rx_b) = channel::<u32>(2);
    tx_a.push(1).unwrap();
    tx_b.push(100).unwrap();
    tx_a.push(2).unwrap();
    assert_eq!(rx_b.len(), 1);
    assert_eq!(rx_a.len(), 2);
    assert_eq!(rx_b.pop(), Some(100));
    assert_eq!(rx_b.pop(), None);
    assert_eq!(rx_a.pop(), Some(1));
    assert_eq!(rx_a.pop(), Some(2));
}

#[test]
fn queued_items_survive_dropping_the_producer() {
    let (mut tx, mut rx) = channel::<u32>(4);
    tx.push(1).unwrap();
    tx.push(2).unwrap();
    drop(tx);
    assert_eq!(rx.len(), 2);
    assert_eq!(rx.pop(), Some(1));
    assert_eq!(rx.pop(), Some(2));
    assert_eq!(rx.pop(), None);
}

#[test]
fn dropping_the_consumer_does_not_break_the_producer() {
    let (mut tx, rx) = channel::<u32>(2);
    drop(rx);
    // Never blocks, never panics: the ring simply fills up.
    assert_eq!(tx.push(1), Ok(()));
    assert_eq!(tx.push(2), Ok(()));
    assert_eq!(tx.push(3), Err(3));
}

// ---------------------------------------------------------------------------
// Cross-thread transfer
// ---------------------------------------------------------------------------

/// Moves `TRANSFER` events through a ring of `capacity` from a producer thread
/// to a consumer thread. Terminates on counts only; a refused push is retried
/// with the value the ring handed back.
fn cross_thread_transfer(capacity: usize) {
    let (mut tx, mut rx) = channel::<InputEvent>(capacity);

    let producer = thread::spawn(move || {
        let mut refused = 0u64;
        for seq in 0..TRANSFER {
            let mut item = event_for(seq);
            loop {
                match tx.push(item) {
                    Ok(()) => break,
                    Err(back) => {
                        assert_eq!(back, event_for(seq), "refused push must return its value");
                        item = back;
                        refused += 1;
                        thread::yield_now();
                    }
                }
            }
        }
        refused
    });

    let consumer = thread::spawn(move || {
        let mut expected = 0u64;
        while expected < TRANSFER {
            assert!(rx.len() <= capacity, "len exceeded capacity {capacity}");
            match rx.pop() {
                Some(got) => {
                    let want = event_for(expected);
                    assert_eq!(
                        got.timestamp_ns, want.timestamp_ns,
                        "item #{expected}: out of order or corrupted timestamp"
                    );
                    assert_eq!(got, want, "item #{expected}: payload corrupted");
                    expected += 1;
                }
                None => thread::yield_now(),
            }
        }
        // Everything was consumed; nothing extra may appear once the producer
        // is done (checked by the caller after join, via the returned handle).
        rx
    });

    producer.join().expect("producer thread panicked");
    let mut rx = consumer.join().expect("consumer thread panicked");
    assert_eq!(rx.len(), 0);
    assert_eq!(rx.pop(), None, "no duplicate or phantom item after the transfer");
}

#[test]
fn cross_thread_100k_in_order_with_timestamps_intact() {
    cross_thread_transfer(1024);
}

#[test]
fn cross_thread_100k_with_capacity_one() {
    cross_thread_transfer(1);
}

#[test]
fn cross_thread_100k_with_small_non_power_of_two_capacity() {
    cross_thread_transfer(7);
}

/// The two halves can each be moved to a thread other than the one that
/// created them, and moved again afterwards.
#[test]
fn halves_can_migrate_between_threads() {
    let (mut tx, mut rx) = channel::<u64>(8);
    tx.push(1).unwrap();
    let mut tx = thread::spawn(move || {
        tx.push(2).unwrap();
        tx
    })
    .join()
    .unwrap();
    let mut rx = thread::spawn(move || {
        assert_eq!(rx.pop(), Some(1));
        rx
    })
    .join()
    .unwrap();
    tx.push(3).unwrap();
    assert_eq!(rx.pop(), Some(2));
    assert_eq!(rx.pop(), Some(3));
    assert_eq!(rx.pop(), None);
}

/// Burst producer: the consumer sees a monotonically consistent `len` bound
/// and strictly sequential values even when the producer fills the ring in
/// bursts and the consumer drains it completely each time it wakes.
#[test]
fn cross_thread_burst_drain_is_sequential() {
    const CAP: usize = 256;
    let (mut tx, mut rx) = channel::<u64>(CAP);
    let producer = thread::spawn(move || {
        let mut seq = 0u64;
        while seq < TRANSFER {
            match tx.push(seq) {
                Ok(()) => seq += 1,
                Err(back) => {
                    assert_eq!(back, seq);
                    thread::yield_now();
                }
            }
        }
    });
    let mut expected = 0u64;
    while expected < TRANSFER {
        let visible = rx.len();
        assert!(visible <= CAP);
        // Everything `len` reported is really there (single consumer, so the
        // count can only grow underneath us).
        for _ in 0..visible {
            assert_eq!(rx.pop(), Some(expected));
            expected += 1;
        }
        if visible == 0 {
            thread::yield_now();
        }
    }
    producer.join().unwrap();
    assert_eq!(rx.pop(), None);
}

// ---------------------------------------------------------------------------
// Model-based property tests
// ---------------------------------------------------------------------------

#[derive(Copy, Clone, Debug)]
enum Op {
    Push(u32),
    Pop,
    Len,
}

fn op_strategy() -> impl Strategy<Value = Op> {
    prop_oneof![
        3 => any::<u32>().prop_map(Op::Push),
        2 => Just(Op::Pop),
        1 => Just(Op::Len),
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]

    /// The ring behaves exactly like a `VecDeque` bounded at `capacity`.
    #[test]
    fn matches_bounded_vecdeque_model(
        capacity in 1usize..40,
        ops in proptest::collection::vec(op_strategy(), 0..400),
    ) {
        let (mut tx, mut rx) = channel::<u32>(capacity);
        let mut model: VecDeque<u32> = VecDeque::new();
        for op in ops {
            match op {
                Op::Push(v) => {
                    let got = tx.push(v);
                    if model.len() < capacity {
                        prop_assert_eq!(got, Ok(()));
                        model.push_back(v);
                    } else {
                        prop_assert_eq!(got, Err(v));
                    }
                }
                Op::Pop => prop_assert_eq!(rx.pop(), model.pop_front()),
                Op::Len => {}
            }
            prop_assert_eq!(rx.len(), model.len());
            prop_assert!(rx.len() <= capacity);
        }
        // Drain: the remainder comes out in model order.
        while let Some(want) = model.pop_front() {
            prop_assert_eq!(rx.pop(), Some(want));
        }
        prop_assert_eq!(rx.pop(), None);
        prop_assert_eq!(rx.len(), 0);
    }

    /// Push-biased sequences keep the ring pinned at its capacity boundary,
    /// where off-by-one errors live.
    #[test]
    fn model_holds_near_full(
        capacity in 1usize..10,
        ops in proptest::collection::vec(
            prop_oneof![5 => any::<u32>().prop_map(Op::Push), 1 => Just(Op::Pop)],
            0..300,
        ),
    ) {
        let (mut tx, mut rx) = channel::<u32>(capacity);
        let mut model: VecDeque<u32> = VecDeque::new();
        for op in ops {
            match op {
                Op::Push(v) => {
                    let got = tx.push(v);
                    if model.len() < capacity {
                        prop_assert_eq!(got, Ok(()));
                        model.push_back(v);
                    } else {
                        prop_assert_eq!(got, Err(v));
                    }
                }
                Op::Pop | Op::Len => prop_assert_eq!(rx.pop(), model.pop_front()),
            }
            prop_assert_eq!(rx.len(), model.len());
        }
    }

    /// Whatever goes in comes out unchanged and in order, for any capacity
    /// large enough to hold it, including timestamps of full `InputEvent`s.
    #[test]
    fn events_round_trip_unchanged(
        stamps in proptest::collection::vec(any::<u64>(), 1..200),
        slack in 0usize..5,
    ) {
        let (mut tx, mut rx) = channel::<InputEvent>(stamps.len() + slack);
        let events: Vec<InputEvent> = stamps
            .iter()
            .enumerate()
            .map(|(i, &ts)| InputEvent::button(ts, CabinetInput::ALL[i % CabinetInput::ALL.len()], i.is_multiple_of(2)))
            .collect();
        for e in &events {
            prop_assert_eq!(tx.push(*e), Ok(()));
        }
        prop_assert_eq!(rx.len(), events.len());
        for e in &events {
            let got = rx.pop();
            prop_assert_eq!(got, Some(*e));
            prop_assert_eq!(got.unwrap().timestamp_ns, e.timestamp_ns);
        }
        prop_assert_eq!(rx.pop(), None);
    }
}
