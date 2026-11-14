//! Bounded, lock-free single-producer / single-consumer ring (CR-001, PLAN §2.2).
//!
//! The main thread forwards timestamped [`InputEvent`](crate::InputEvent)s to the
//! sim thread, and window events to the render thread, through these rings.
//!
//! * [`channel`] allocates the ring once; afterwards [`SpscProducer::push`] and
//!   [`SpscConsumer::pop`] never block, never take a lock and never allocate.
//! * `capacity` is exact: the ring holds at most `capacity` items (any value
//!   ≥ 1, not rounded to a power of two). `push` on a full ring returns
//!   `Err(value)` and leaves the ring unchanged.
//! * Items come out in the order they went in (FIFO).
//!
//! Implementation: `head` (next position to read) and `tail` (next position to
//! write) run over `0..2 * capacity`, so a full ring (`tail - head ==
//! capacity`) and an empty one (`tail == head`) are distinct for any capacity;
//! position `p` maps to slot `p` or `p - capacity`. The producer is
//! the only writer of `tail`, the consumer the only writer of `head`; each
//! publishes with `Release` and reads the other's counter with `Acquire`, which
//! orders the slot contents. Each side caches the other's counter to avoid
//! touching the shared cache line on every call.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// Pads a counter to its own cache line so producer and consumer do not
/// false-share.
#[repr(align(128))]
struct CachePadded<T>(T);

struct Shared<T> {
    /// Next counter value the consumer reads. Written only by the consumer.
    head: CachePadded<AtomicUsize>,
    /// Next counter value the producer writes. Written only by the producer.
    tail: CachePadded<AtomicUsize>,
    slots: Box<[UnsafeCell<MaybeUninit<T>>]>,
}

impl<T> Shared<T> {
    #[inline]
    fn capacity(&self) -> usize {
        self.slots.len()
    }

    /// Slot of position `pos` (`pos < 2 * capacity`).
    #[inline]
    fn slot(&self, pos: usize) -> *mut MaybeUninit<T> {
        let cap = self.slots.len();
        let i = if pos >= cap { pos - cap } else { pos };
        self.slots[i].get()
    }

    /// Number of items between positions `head` and `tail`.
    #[inline]
    fn distance(&self, head: usize, tail: usize) -> usize {
        if tail >= head {
            tail - head
        } else {
            tail + 2 * self.slots.len() - head
        }
    }

    /// The position after `pos`.
    #[inline]
    fn next(&self, pos: usize) -> usize {
        if pos + 1 == 2 * self.slots.len() {
            0
        } else {
            pos + 1
        }
    }
}

// SAFETY: a slot is written only by the producer while it is outside
// `head..tail`, and read only by the consumer while it is inside; the hand-over
// is ordered by the Release/Acquire pairs on `tail` and `head`. `T: Send` is
// required because values cross threads.
unsafe impl<T: Send> Sync for Shared<T> {}
unsafe impl<T: Send> Send for Shared<T> {}

/// Creates a ring that holds at most `capacity` items and returns its two ends.
///
/// This is the ring's only allocation.
///
/// # Panics
/// If `capacity` is 0 or larger than `usize::MAX / 4`.
pub fn channel<T: Copy + Send>(capacity: usize) -> (SpscProducer<T>, SpscConsumer<T>) {
    assert!(capacity >= 1, "spsc::channel capacity must be at least 1");
    assert!(
        capacity <= usize::MAX / 4,
        "spsc::channel capacity too large"
    );
    let slots = (0..capacity)
        .map(|_| UnsafeCell::new(MaybeUninit::uninit()))
        .collect::<Vec<_>>()
        .into_boxed_slice();
    let shared = Arc::new(Shared {
        head: CachePadded(AtomicUsize::new(0)),
        tail: CachePadded(AtomicUsize::new(0)),
        slots,
    });
    (
        SpscProducer {
            shared: Arc::clone(&shared),
            tail: 0,
            head_cache: 0,
        },
        SpscConsumer {
            shared,
            head: 0,
            tail_cache: 0,
        },
    )
}

/// The writing end of a ring made by [`channel`].
pub struct SpscProducer<T: Copy + Send> {
    shared: Arc<Shared<T>>,
    /// Local copy of `shared.tail` (only this side writes it).
    tail: usize,
    /// Last observed `shared.head`.
    head_cache: usize,
}

impl<T: Copy + Send> SpscProducer<T> {
    /// Appends `value`. Returns `Err(value)` if the ring is full.
    ///
    /// Wait-free, lock-free and allocation-free.
    #[inline]
    pub fn push(&mut self, value: T) -> Result<(), T> {
        let shared = &*self.shared;
        let cap = shared.capacity();
        if shared.distance(self.head_cache, self.tail) >= cap {
            self.head_cache = shared.head.0.load(Ordering::Acquire);
            if shared.distance(self.head_cache, self.tail) >= cap {
                return Err(value);
            }
        }
        // SAFETY: the slot for `tail` is outside `head..tail`, so the consumer
        // does not read it until the Release store below publishes it.
        unsafe { (*shared.slot(self.tail)).write(value) };
        self.tail = shared.next(self.tail);
        shared.tail.0.store(self.tail, Ordering::Release);
        Ok(())
    }

    /// Maximum number of queued items.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.shared.capacity()
    }

    /// Number of items currently queued (a snapshot; the consumer may be
    /// popping concurrently, so the true value can only be lower).
    #[inline]
    pub fn len(&self) -> usize {
        self.shared
            .distance(self.shared.head.0.load(Ordering::Acquire), self.tail)
    }

    /// `true` if no items are queued (snapshot, see [`len`](Self::len)).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// `true` if a [`push`](Self::push) would fail right now.
    #[inline]
    pub fn is_full(&self) -> bool {
        self.len() >= self.capacity()
    }
}

/// The reading end of a ring made by [`channel`].
pub struct SpscConsumer<T: Copy + Send> {
    shared: Arc<Shared<T>>,
    /// Local copy of `shared.head` (only this side writes it).
    head: usize,
    /// Last observed `shared.tail`.
    tail_cache: usize,
}

impl<T: Copy + Send> SpscConsumer<T> {
    /// Removes and returns the oldest item, or `None` if the ring is empty.
    ///
    /// Wait-free, lock-free and allocation-free.
    #[inline]
    pub fn pop(&mut self) -> Option<T> {
        if self.head == self.tail_cache {
            self.tail_cache = self.shared.tail.0.load(Ordering::Acquire);
            if self.head == self.tail_cache {
                return None;
            }
        }
        // SAFETY: the slot for `head` is inside `head..tail`; the Acquire load
        // of `tail` that made it visible ordered the producer's write before
        // this read, and the producer will not overwrite it until `head`
        // moves past it.
        let value = unsafe { (*self.shared.slot(self.head)).assume_init_read() };
        self.head = self.shared.next(self.head);
        self.shared.head.0.store(self.head, Ordering::Release);
        Some(value)
    }

    /// Returns a copy of the oldest item without removing it.
    #[inline]
    pub fn peek(&mut self) -> Option<T> {
        if self.head == self.tail_cache {
            self.tail_cache = self.shared.tail.0.load(Ordering::Acquire);
            if self.head == self.tail_cache {
                return None;
            }
        }
        // SAFETY: as in `pop`; the slot stays owned by the consumer.
        Some(unsafe { (*self.shared.slot(self.head)).assume_init_read() })
    }

    /// Number of items currently queued (a snapshot; the producer may be
    /// pushing concurrently, so the true value can only be higher).
    #[inline]
    pub fn len(&self) -> usize {
        self.shared
            .distance(self.head, self.shared.tail.0.load(Ordering::Acquire))
    }

    /// `true` if no items are queued (snapshot, see [`len`](Self::len)).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Maximum number of queued items.
    #[inline]
    pub fn capacity(&self) -> usize {
        self.shared.capacity()
    }
}

// Compile-time checks: both ends are `Send` for a `Send` payload.
const _: () = {
    const fn assert_send<T: Send>() {}
    assert_send::<SpscProducer<crate::InputEvent>>();
    assert_send::<SpscConsumer<crate::InputEvent>>();
};

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CabinetInput, InputEvent};

    #[test]
    fn exact_capacity_and_fifo() {
        for cap in [1usize, 2, 3, 7, 8, 100] {
            let (mut p, mut c) = channel::<u32>(cap);
            assert_eq!(p.capacity(), cap);
            assert_eq!(c.capacity(), cap);
            assert!(c.is_empty() && p.is_empty());
            for i in 0..cap as u32 {
                assert_eq!(p.push(i), Ok(()));
            }
            assert!(p.is_full());
            assert_eq!(p.push(999), Err(999));
            assert_eq!(c.len(), cap);
            assert_eq!(p.len(), cap);
            assert_eq!(c.peek(), Some(0));
            for i in 0..cap as u32 {
                assert_eq!(c.pop(), Some(i));
            }
            assert_eq!(c.pop(), None);
            assert_eq!(c.peek(), None);
            assert!(c.is_empty());
        }
    }

    #[test]
    fn wraps_around_many_times() {
        let (mut p, mut c) = channel::<u64>(3);
        let mut next_in = 0u64;
        let mut next_out = 0u64;
        for round in 0..1000u64 {
            let n = round % 4;
            for _ in 0..n {
                if p.push(next_in).is_ok() {
                    next_in += 1;
                }
            }
            for _ in 0..(round % 3) {
                if let Some(v) = c.pop() {
                    assert_eq!(v, next_out);
                    next_out += 1;
                }
            }
            assert_eq!(c.len() as u64, next_in - next_out);
        }
    }

    #[test]
    fn input_events_keep_timestamps() {
        let (mut p, mut c) = channel(4);
        p.push(InputEvent::button(42, CabinetInput::Start, true))
            .unwrap();
        assert_eq!(
            c.pop(),
            Some(InputEvent::button(42, CabinetInput::Start, true))
        );
    }

    #[test]
    #[should_panic(expected = "at least 1")]
    fn zero_capacity_panics() {
        let _ = channel::<u8>(0);
    }

    #[test]
    fn positions_wrap_for_odd_capacity() {
        let (mut p, mut c) = channel::<u32>(3);
        for i in 0..1000u32 {
            p.push(i).unwrap();
            p.push(i + 10_000).unwrap();
            assert_eq!(c.len(), 2);
            assert_eq!(c.pop(), Some(i));
            assert_eq!(c.pop(), Some(i + 10_000));
            assert!(p.tail < 6 && c.head < 6);
        }
    }

    #[test]
    #[should_panic(expected = "too large")]
    fn huge_capacity_panics() {
        let _ = channel::<u8>(usize::MAX);
    }
}
