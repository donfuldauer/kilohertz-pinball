//! Lock-free trigger ring from the sim thread to the audio thread, built on
//! [`pin_core::spsc`].

use pin_core::SoundId;
use pin_core::script::HostCommandQueue;
use pin_core::spsc::{SpscConsumer, SpscProducer, channel};

/// One "play this sound" request.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SoundTrigger {
    /// Sound to start.
    pub sound: SoundId,
    /// Voice gain (clamped to `≥ 0` by the mixer; NaN plays silently).
    pub gain: f32,
}

/// Creates the trigger ring. `capacity` is exact (a value of 0 is treated as
/// 1). This is the ring's only allocation.
pub fn trigger_ring(capacity: usize) -> (SoundSender, SoundReceiver) {
    let (tx, rx) = channel(capacity.max(1));
    (SoundSender { tx, dropped: 0 }, SoundReceiver { rx })
}

/// Sim-thread end of the ring. `send` never blocks, locks or allocates.
pub struct SoundSender {
    tx: SpscProducer<SoundTrigger>,
    dropped: u64,
}

impl SoundSender {
    /// Queues `t`; returns `false` (and counts it in [`dropped`](Self::dropped))
    /// when the ring is full.
    pub fn send(&mut self, t: SoundTrigger) -> bool {
        if self.tx.push(t).is_ok() {
            true
        } else {
            self.dropped += 1;
            false
        }
    }

    /// Sends every `sounds` entry of `q` in order; returns how many were sent
    /// (entries that did not fit are counted as dropped).
    pub fn send_queue(&mut self, q: &HostCommandQueue) -> usize {
        let mut sent = 0;
        for &(sound, gain) in &q.sounds {
            if self.send(SoundTrigger { sound, gain }) {
                sent += 1;
            }
        }
        sent
    }

    /// Triggers rejected because the ring was full.
    pub fn dropped(&self) -> u64 {
        self.dropped
    }

    /// Ring capacity.
    pub fn capacity(&self) -> usize {
        self.tx.capacity()
    }
}

/// Audio-thread end of the ring, owned by the [`Mixer`](super::Mixer).
pub struct SoundReceiver {
    rx: SpscConsumer<SoundTrigger>,
}

impl SoundReceiver {
    /// Takes the oldest pending trigger.
    pub fn pop(&mut self) -> Option<SoundTrigger> {
        self.rx.pop()
    }

    /// Ring capacity.
    pub fn capacity(&self) -> usize {
        self.rx.capacity()
    }

    /// Pending triggers.
    pub fn len(&self) -> usize {
        self.rx.len()
    }

    /// No pending triggers.
    pub fn is_empty(&self) -> bool {
        self.rx.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_ring_counts_drops() {
        let (mut tx, mut rx) = trigger_ring(2);
        let t = |i| SoundTrigger {
            sound: SoundId(i),
            gain: 1.0,
        };
        assert!(tx.send(t(1)));
        assert!(tx.send(t(2)));
        assert!(!tx.send(t(3)));
        assert_eq!(tx.dropped(), 1);
        assert_eq!(rx.len(), 2);
        assert_eq!(rx.pop(), Some(t(1)));
        assert_eq!(rx.pop(), Some(t(2)));
        assert!(rx.is_empty());
        assert_eq!(tx.capacity(), 2);
        assert_eq!(rx.capacity(), 2);
        assert_eq!(trigger_ring(0).0.capacity(), 1);
    }

    #[test]
    fn send_queue_in_order() {
        let (mut tx, mut rx) = trigger_ring(3);
        let mut q = HostCommandQueue::default();
        for i in 0..5 {
            q.play_sound(SoundId(i), i as f32).unwrap();
        }
        assert_eq!(tx.send_queue(&q), 3);
        assert_eq!(tx.dropped(), 2);
        for i in 0..3 {
            assert_eq!(rx.pop().unwrap().sound, SoundId(i));
        }
    }
}
