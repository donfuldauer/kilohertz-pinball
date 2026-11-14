//! Black-box spec tests for `pin_core::triple_buffer` (Subphase 1.1 brief).

use pin_core::snapshot::FrameStateSnapshot;
use pin_core::triple_buffer::{TripleBuffer, TripleReader, TripleWriter};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::thread;

const PUBLISHES: u64 = 100_000;

/// Every field is derived from one counter, so a torn read (fields from two
/// different publishes) is detectable.
#[derive(Copy, Clone, Debug, PartialEq)]
struct Payload {
    tick: u64,
    inverted: u64,
    words: [u64; 30],
    scaled: f64,
    low: u8,
}

impl Payload {
    fn from_counter(n: u64) -> Self {
        let mut words = [0u64; 30];
        for (i, w) in words.iter_mut().enumerate() {
            *w = n.wrapping_mul(i as u64 + 1).wrapping_add(i as u64);
        }
        Payload {
            tick: n,
            inverted: !n,
            words,
            scaled: n as f64 * 0.5,
            low: (n & 0xff) as u8,
        }
    }

    fn is_consistent(&self) -> bool {
        *self == Payload::from_counter(self.tick)
    }
}

fn assert_send<T: Send>() {}

#[allow(clippy::field_reassign_with_default)]
fn snapshot_at(tick: u64) -> FrameStateSnapshot {
    let mut snap = FrameStateSnapshot::default();
    snap.tick = tick;
    snap
}

#[test]
fn writer_and_reader_are_send() {
    assert_send::<TripleWriter<u64>>();
    assert_send::<TripleReader<u64>>();
    assert_send::<TripleWriter<Payload>>();
    assert_send::<TripleReader<Payload>>();
    assert_send::<TripleWriter<FrameStateSnapshot>>();
    assert_send::<TripleReader<FrameStateSnapshot>>();
}

#[test]
fn initial_value_is_readable_before_any_publish() {
    let (_writer, mut reader) = TripleBuffer::new(41u32);
    assert_eq!(*reader.read(), 41);
    // Reading again without a publish keeps returning the same value.
    assert_eq!(*reader.read(), 41);
    assert_eq!(*reader.read(), 41);
}

#[test]
fn read_returns_the_published_value() {
    let (mut writer, mut reader) = TripleBuffer::new(0u32);
    writer.publish(7);
    assert_eq!(*reader.read(), 7);
    assert_eq!(*reader.read(), 7, "a read without a new publish repeats");
}

#[test]
fn latest_publish_wins() {
    let (mut writer, mut reader) = TripleBuffer::new(0u64);
    for n in 1..=10 {
        writer.publish(n);
    }
    assert_eq!(*reader.read(), 10);
    writer.publish(11);
    writer.publish(12);
    assert_eq!(*reader.read(), 12);
    assert_eq!(*reader.read(), 12);
}

#[test]
fn alternating_publish_and_read_never_shows_stale_data() {
    let (mut writer, mut reader) = TripleBuffer::new(Payload::from_counter(0));
    for n in 1..=5_000u64 {
        writer.publish(Payload::from_counter(n));
        let got = *reader.read();
        assert_eq!(got, Payload::from_counter(n));
    }
}

#[test]
fn irregular_single_thread_interleavings_always_yield_latest() {
    // Deterministic pseudo-random bursts of publishes between reads.
    let (mut writer, mut reader) = TripleBuffer::new(0u64);
    let mut state: u32 = 0x1234_5678;
    let mut last = 0u64;
    for _ in 0..10_000 {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let burst = (state >> 29) as u64; // 0..=7
        for _ in 0..burst {
            last += 1;
            writer.publish(last);
        }
        assert_eq!(*reader.read(), last);
    }
}

#[test]
fn writer_can_outlive_reader_and_vice_versa() {
    let (mut writer, reader) = TripleBuffer::new(1u8);
    drop(reader);
    writer.publish(2);
    writer.publish(3);

    let (mut writer, mut reader) = TripleBuffer::new(1u8);
    writer.publish(9);
    drop(writer);
    assert_eq!(*reader.read(), 9);
    assert_eq!(*reader.read(), 9);
}

#[test]
fn works_with_frame_state_snapshot() {
    let (mut writer, mut reader) = TripleBuffer::new(FrameStateSnapshot::default());
    assert_eq!(reader.read().tick, 0);
    writer.publish(snapshot_at(1234));
    assert_eq!(reader.read().tick, 1234);
}

/// Verification criterion: writer and reader threads over 100,000 publishes,
/// zero torn reads, monotonic tick numbers.
#[test]
fn two_thread_stress_has_no_torn_reads_and_is_monotonic() {
    let (mut writer, mut reader) = TripleBuffer::new(Payload::from_counter(0));
    let reads = AtomicU64::new(0);
    let reader_ready = AtomicBool::new(false);

    thread::scope(|s| {
        let reader_handle = s.spawn(|| {
            let mut last = 0u64;
            let mut distinct = 0u64;
            reader_ready.store(true, Ordering::Release);
            loop {
                let got = *reader.read();
                assert!(got.is_consistent(), "torn read at tick {}", got.tick);
                assert!(
                    got.tick >= last,
                    "tick went backwards: {} after {last}",
                    got.tick
                );
                assert!(got.tick <= PUBLISHES);
                if got.tick != last {
                    distinct += 1;
                }
                last = got.tick;
                reads.fetch_add(1, Ordering::Relaxed);
                if last == PUBLISHES {
                    break;
                }
                std::hint::spin_loop();
            }
            distinct
        });

        s.spawn(|| {
            while !reader_ready.load(Ordering::Acquire) {
                std::hint::spin_loop();
            }
            for n in 1..=PUBLISHES {
                writer.publish(Payload::from_counter(n));
                if n.is_multiple_of(64) {
                    // Give the reader a chance to interleave on single-core hosts.
                    thread::yield_now();
                }
            }
        });

        let distinct = reader_handle.join().expect("reader thread panicked");
        assert!(distinct >= 1, "reader must observe the final value");
    });

    assert!(reads.load(Ordering::Relaxed) >= 1);
    // After both threads finish, the final value is still the latest.
    assert_eq!(*reader.read(), Payload::from_counter(PUBLISHES));
}

/// Same criterion with the real snapshot type and moved (not scoped) handles.
#[test]
fn two_thread_stress_with_snapshot_ticks_is_monotonic() {
    let (mut writer, mut reader) = TripleBuffer::new(FrameStateSnapshot::default());

    let w = thread::spawn(move || {
        for n in 1..=PUBLISHES {
            writer.publish(snapshot_at(n));
            if n.is_multiple_of(64) {
                thread::yield_now();
            }
        }
    });
    let r = thread::spawn(move || {
        let mut last = 0u64;
        while last != PUBLISHES {
            let tick = reader.read().tick;
            assert!(tick >= last, "tick went backwards: {tick} after {last}");
            assert!(tick <= PUBLISHES);
            last = tick;
            std::hint::spin_loop();
        }
        last
    });

    w.join().expect("writer thread panicked");
    assert_eq!(r.join().expect("reader thread panicked"), PUBLISHES);
}
