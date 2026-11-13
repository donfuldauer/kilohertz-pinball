//! Haptics bus (Subphase 3.4): lock-free event rings from the sim thread and
//! the SDL-free gamepad rumble mapper.
//!
//! # Event flow
//! The sim thread produces [`HapticEvent`]s (coil fire, EOS hit, bumper, ball
//! drop, ball roll; `pin_core::haptics`, CR-003). Each consumer thread gets
//! its own single-producer/single-consumer ring:
//!
//! * the main thread drains one into a [`RumbleMapper`] and drives gamepad
//!   rumble (`pin_app::platform::rumble`);
//! * the audio thread drains one into the SSF exciter mixer
//!   (`pin_audio::ssf::SsfMixer`);
//! * telemetry impulses go with the per-tick state to the telemetry encoder
//!   (`pin_audio::telemetry`).
//!
//! [`HapticsBus`] fans one event out to up to [`MAX_HAPTICS_SINKS`] rings in
//! a single call. [`haptics_ring`] builds one ring; it is generic over the
//! event type (any `Copy + Send` type, normally [`HapticEvent`], but the
//! older [`HapticsTelemetryEvent`](pin_core::input::HapticsTelemetryEvent)
//! works too).
//!
//! # Guarantees
//! * [`HapticsProducer::push`] is wait-free and allocation-free: it performs
//!   one SPSC push (`pin_core::spsc`) and, on a full ring, one relaxed atomic
//!   increment. A full ring keeps its contents and **drops the newest**
//!   event, counting it in [`HapticsProducer::dropped`] (also readable from
//!   the consumer).
//! * Events come out in the order they went in.
//! * [`haptics_ring`] performs the only allocations (the slots and the shared
//!   drop counter).

mod rumble;

pub use rumble::{
    IntoHapticEvent, MAX_ACTIVE_RUMBLES, MAX_RUMBLE_OVERRIDES, Rumble, RumbleEffect, RumbleMapper,
    RumbleSettings,
};

pub use pin_core::haptics::{HapticEvent, HapticKind};

use arrayvec::ArrayVec;
use core::sync::atomic::{AtomicU64, Ordering};
use pin_core::spsc::{SpscConsumer, SpscProducer, channel};
use std::sync::Arc;

/// Default ring capacity: 256 events (a quarter second of one event per
/// tick, far more than the main or audio thread ever falls behind).
pub const DEFAULT_HAPTICS_RING_CAPACITY: usize = 256;

/// Most rings one [`HapticsBus`] feeds.
pub const MAX_HAPTICS_SINKS: usize = 4;

/// Creates a haptics ring holding exactly `capacity` events (0 is treated as
/// 1). This is the ring's only allocation.
pub fn haptics_ring<E: Copy + Send>(capacity: usize) -> (HapticsProducer<E>, HapticsConsumer<E>) {
    let (tx, rx) = channel(capacity.max(1));
    let dropped = Arc::new(AtomicU64::new(0));
    (
        HapticsProducer {
            tx,
            dropped: Arc::clone(&dropped),
        },
        HapticsConsumer { rx, dropped },
    )
}

/// Sim-thread end of a haptics ring.
pub struct HapticsProducer<E: Copy + Send = HapticEvent> {
    tx: SpscProducer<E>,
    dropped: Arc<AtomicU64>,
}

impl<E: Copy + Send> HapticsProducer<E> {
    /// Queues `e`. Returns `false` and counts a drop when the ring is full
    /// (the newest event is the one dropped). Wait-free, never allocates.
    #[inline]
    pub fn push(&mut self, e: E) -> bool {
        if self.tx.push(e).is_ok() {
            true
        } else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            false
        }
    }

    /// Events dropped because the ring was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Ring capacity.
    pub fn capacity(&self) -> usize {
        self.tx.capacity()
    }

    /// Events currently queued (a snapshot).
    pub fn len(&self) -> usize {
        self.tx.len()
    }

    /// True when nothing is queued (a snapshot).
    pub fn is_empty(&self) -> bool {
        self.tx.is_empty()
    }
}

/// Consumer end of a haptics ring.
pub struct HapticsConsumer<E: Copy + Send = HapticEvent> {
    rx: SpscConsumer<E>,
    dropped: Arc<AtomicU64>,
}

impl<E: Copy + Send> HapticsConsumer<E> {
    /// Takes the oldest queued event. Never blocks or allocates.
    #[inline]
    pub fn pop(&mut self) -> Option<E> {
        self.rx.pop()
    }

    /// Calls `f` with every queued event, oldest first; returns how many.
    pub fn drain(&mut self, mut f: impl FnMut(E)) -> usize {
        let mut n = 0;
        while let Some(e) = self.rx.pop() {
            f(e);
            n += 1;
        }
        n
    }

    /// Events the producer dropped because the ring was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    /// Ring capacity.
    pub fn capacity(&self) -> usize {
        self.rx.capacity()
    }

    /// Events currently queued (a snapshot).
    pub fn len(&self) -> usize {
        self.rx.len()
    }

    /// True when nothing is queued (a snapshot).
    pub fn is_empty(&self) -> bool {
        self.rx.is_empty()
    }
}

/// Fans each sim-thread event out to several haptics rings (one per
/// consumer thread). Sinks are added at setup; [`push`](Self::push) is
/// wait-free and allocation-free.
pub struct HapticsBus<E: Copy + Send = HapticEvent> {
    sinks: ArrayVec<HapticsProducer<E>, MAX_HAPTICS_SINKS>,
}

impl<E: Copy + Send> Default for HapticsBus<E> {
    fn default() -> Self {
        Self::new()
    }
}

impl<E: Copy + Send> HapticsBus<E> {
    /// A bus with no sinks.
    pub fn new() -> Self {
        Self {
            sinks: ArrayVec::new(),
        }
    }

    /// Adds a sink ring of `capacity` events and returns its consumer, or
    /// `None` when the bus already has [`MAX_HAPTICS_SINKS`] sinks. Setup
    /// only: allocates the ring.
    pub fn add_sink(&mut self, capacity: usize) -> Option<HapticsConsumer<E>> {
        if self.sinks.is_full() {
            return None;
        }
        let (tx, rx) = haptics_ring(capacity);
        self.sinks.push(tx);
        Some(rx)
    }

    /// Pushes `e` to every sink; returns `true` when every sink accepted it
    /// (each full sink counts its own drop).
    pub fn push(&mut self, e: E) -> bool {
        let mut all = true;
        for s in &mut self.sinks {
            all &= s.push(e);
        }
        all
    }

    /// Number of sinks.
    pub fn sinks(&self) -> usize {
        self.sinks.len()
    }

    /// Drops summed over all sinks.
    pub fn dropped(&self) -> u64 {
        self.sinks.iter().map(HapticsProducer::dropped).sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pin_core::CoilId;

    fn ev(tick: u64) -> HapticEvent {
        HapticEvent::new(tick, HapticKind::CoilFire { coil: CoilId(1) }, 1.0)
    }

    #[test]
    fn ring_fifo_and_drops() {
        let (mut p, mut c) = haptics_ring::<HapticEvent>(0);
        assert_eq!(p.capacity(), 1);
        assert!(p.is_empty() && c.is_empty());
        assert!(p.push(ev(1)));
        assert!(!p.push(ev(2)));
        assert_eq!((p.dropped(), c.dropped()), (1, 1));
        assert_eq!((p.len(), c.len(), c.capacity()), (1, 1, 1));
        assert_eq!(c.pop(), Some(ev(1)));
        assert_eq!(c.pop(), None);
        assert!(p.push(ev(3)));
        let mut got = Vec::new();
        assert_eq!(c.drain(|e| got.push(e.tick)), 1);
        assert_eq!(got, vec![3]);
    }

    #[test]
    fn bus_fans_out() {
        let mut bus = HapticsBus::<HapticEvent>::default();
        let mut a = bus.add_sink(2).unwrap();
        let mut b = bus.add_sink(1).unwrap();
        assert!(bus.add_sink(1).is_some() && bus.add_sink(1).is_some());
        assert!(bus.add_sink(1).is_none());
        assert_eq!(bus.sinks(), 4);
        assert!(bus.push(ev(1)));
        assert!(!bus.push(ev(2)));
        assert_eq!(bus.dropped(), 3);
        assert_eq!(a.pop().map(|e| e.tick), Some(1));
        assert_eq!(a.pop().map(|e| e.tick), Some(2));
        assert_eq!(b.pop().map(|e| e.tick), Some(1));
        assert_eq!(b.pop(), None);
    }
}
