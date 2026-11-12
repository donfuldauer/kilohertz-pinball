//! Lock-free triple buffer for the sim → render snapshot hand-off (D7).
//!
//! Three slots rotate between a single writer and a single reader. The writer
//! always owns one slot, the reader owns another, and the third ("back") slot is
//! exchanged through one atomic byte that holds the back slot's index plus a
//! "fresh" bit. [`TripleWriter::publish`] copies a value into the writer's slot
//! and swaps it with the back slot; [`TripleReader::read`] swaps the back slot in
//! when it is fresh. Neither side ever blocks, takes a lock or allocates; the
//! only allocation is the shared `Arc` in [`TripleBuffer::new`].
//!
//! The reader always sees the most recently *completed* publish (or the initial
//! value), never a partially written one, and never goes backwards in time.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU8, Ordering};
use std::sync::Arc;

/// Mask selecting the slot index in the shared back-slot byte.
const INDEX_MASK: u8 = 0b011;
/// Set when the back slot holds a value the reader has not taken yet.
const FRESH_BIT: u8 = 0b100;

/// The storage shared by a [`TripleWriter`] and a [`TripleReader`].
///
/// Construct it with [`TripleBuffer::new`], which hands out the two ends.
pub struct TripleBuffer<T: Copy> {
    slots: [UnsafeCell<T>; 3],
    /// Index of the back slot, plus [`FRESH_BIT`].
    back: AtomicU8,
}

// SAFETY: the slots are only accessed through the protocol below: each slot index
// is owned by exactly one of {writer, reader, back} at any time, and ownership is
// transferred only by the AcqRel swap on `back`, which orders the slot contents.
// `T: Send` is required because values written on one thread are read on another.
unsafe impl<T: Copy + Send> Sync for TripleBuffer<T> {}
unsafe impl<T: Copy + Send> Send for TripleBuffer<T> {}

impl<T: Copy> TripleBuffer<T> {
    /// Creates the buffer with all three slots set to `initial` and returns its
    /// writer and reader ends. This is the only allocation the buffer performs.
    #[allow(clippy::new_ret_no_self)]
    pub fn new(initial: T) -> (TripleWriter<T>, TripleReader<T>) {
        let shared = Arc::new(TripleBuffer {
            slots: [
                UnsafeCell::new(initial),
                UnsafeCell::new(initial),
                UnsafeCell::new(initial),
            ],
            back: AtomicU8::new(1),
        });
        (
            TripleWriter {
                shared: Arc::clone(&shared),
                index: 0,
            },
            TripleReader { shared, index: 2 },
        )
    }
}

/// The producing end (owned by the sim thread).
pub struct TripleWriter<T: Copy> {
    shared: Arc<TripleBuffer<T>>,
    index: u8,
}

// SAFETY: the writer only touches the slot it owns (`index`) and the atomic.
unsafe impl<T: Copy + Send> Send for TripleWriter<T> {}

impl<T: Copy> TripleWriter<T> {
    /// Publishes `value`, making it the latest value the reader will see.
    ///
    /// Wait-free, lock-free and allocation-free.
    #[inline]
    pub fn publish(&mut self, value: T) {
        let slot = self.shared.slots[usize::from(self.index)].get();
        // SAFETY: slot `self.index` is owned exclusively by the writer: the reader
        // owns a different index and the back index is a third one.
        unsafe { slot.write(value) };
        let previous = self
            .shared
            .back
            .swap(self.index | FRESH_BIT, Ordering::AcqRel);
        self.index = previous & INDEX_MASK;
    }
}

/// The consuming end (owned by the render thread).
pub struct TripleReader<T: Copy> {
    shared: Arc<TripleBuffer<T>>,
    index: u8,
}

// SAFETY: the reader only touches the slot it owns (`index`) and the atomic.
unsafe impl<T: Copy + Send> Send for TripleReader<T> {}

impl<T: Copy> TripleReader<T> {
    /// Returns the latest published value, or the initial value if nothing has
    /// been published yet. Wait-free, lock-free and allocation-free.
    #[inline]
    pub fn read(&mut self) -> &T {
        if self.shared.back.load(Ordering::Relaxed) & FRESH_BIT != 0 {
            let previous = self.shared.back.swap(self.index, Ordering::AcqRel);
            self.index = previous & INDEX_MASK;
        }
        // SAFETY: slot `self.index` is owned exclusively by the reader until the
        // next `read`, which needs `&mut self` and so outlives this borrow.
        unsafe { &*self.shared.slots[usize::from(self.index)].get() }
    }

    /// True when a value newer than the one last returned by [`read`](Self::read)
    /// is waiting. Useful for redraw-on-change surfaces.
    #[inline]
    pub fn has_update(&self) -> bool {
        self.shared.back.load(Ordering::Relaxed) & FRESH_BIT != 0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_value_until_first_publish() {
        let (mut w, mut r) = TripleBuffer::new(7u32);
        assert!(!r.has_update());
        assert_eq!(*r.read(), 7);
        assert_eq!(*r.read(), 7);
        w.publish(8);
        assert!(r.has_update());
        assert_eq!(*r.read(), 8);
        assert!(!r.has_update());
        assert_eq!(*r.read(), 8);
    }

    #[test]
    fn reader_sees_latest_of_many_publishes() {
        let (mut w, mut r) = TripleBuffer::new(0u64);
        for i in 1..=10 {
            w.publish(i);
        }
        assert_eq!(*r.read(), 10);
        w.publish(11);
        w.publish(12);
        assert_eq!(*r.read(), 12);
    }

    #[test]
    fn interleaved_single_thread_never_goes_backwards() {
        let (mut w, mut r) = TripleBuffer::new(0u64);
        let mut last = 0;
        for i in 1..1000u64 {
            w.publish(i);
            if i % 3 == 0 {
                let v = *r.read();
                assert!(v >= last);
                assert_eq!(v, i);
                last = v;
            }
        }
    }

    #[test]
    fn ends_are_send() {
        fn assert_send<S: Send>() {}
        assert_send::<TripleWriter<[u64; 4]>>();
        assert_send::<TripleReader<[u64; 4]>>();
    }

    #[test]
    fn slot_indices_stay_disjoint() {
        let (mut w, mut r) = TripleBuffer::new(0u8);
        for i in 0..50u8 {
            if i % 2 == 0 {
                w.publish(i);
            }
            if i % 5 == 0 {
                let _ = r.read();
            }
            let back = w.shared.back.load(Ordering::Relaxed) & INDEX_MASK;
            assert_ne!(w.index, r.index);
            assert_ne!(w.index, back);
            assert_ne!(r.index, back);
        }
    }
}
