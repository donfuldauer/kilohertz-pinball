//! Datagram ring (sim thread → telemetry thread) and the UDP sender.

use super::{Datagram, DatagramBatch};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use core::time::Duration;
use pin_core::spsc::{SpscConsumer, SpscProducer, channel};
use std::io;
use std::net::{Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::thread::JoinHandle;

/// Default ring capacity in datagrams (≈ 93 KB; more than ten full-state
/// ticks, or 64 ms of single-datagram ticks).
pub const DEFAULT_TELEMETRY_RING_CAPACITY: usize = 64;

/// Creates the datagram ring (`capacity` exact; 0 is treated as 1). This is
/// the ring's only allocation.
///
/// Use at least [`MAX_DATAGRAMS_PER_TICK`](super::MAX_DATAGRAMS_PER_TICK)
/// (5; the default [`DEFAULT_TELEMETRY_RING_CAPACITY`] is 64): a full state
/// takes that many datagrams, so a smaller ring can never carry one whole
/// and the [`force_full_state`](super::TelemetryEncoder::force_full_state)
/// recovery of [`TelemetryTx::push_batch`] cannot complete.
pub fn telemetry_channel(capacity: usize) -> (TelemetryTx, TelemetryReceiver) {
    let (tx, rx) = channel(capacity.max(1));
    let dropped = Arc::new(AtomicU64::new(0));
    (
        TelemetryTx {
            tx,
            dropped: Arc::clone(&dropped),
        },
        TelemetryReceiver { rx, dropped },
    )
}

/// Sim-thread end of the datagram ring. Wait-free and allocation-free.
pub struct TelemetryTx {
    tx: SpscProducer<Datagram>,
    dropped: Arc<AtomicU64>,
}

impl TelemetryTx {
    /// Queues one datagram; `false` (and a counted drop) when the ring is full.
    pub fn push(&mut self, d: &Datagram) -> bool {
        if self.tx.push(*d).is_ok() {
            true
        } else {
            self.dropped.fetch_add(1, Ordering::Relaxed);
            false
        }
    }

    /// Queues every datagram of `batch` in order; returns `true` when all
    /// fit. On `false` a listener may have missed a change, and the caller
    /// **must** call
    /// [`TelemetryEncoder::force_full_state`](super::TelemetryEncoder::force_full_state)
    /// so the next tick resynchronises it (the ring cannot do this itself):
    ///
    /// ```ignore
    /// encoder.encode_tick(tick, &state, &impulses, &mut batch);
    /// if !tx.push_batch(&batch) {
    ///     encoder.force_full_state();
    /// }
    /// ```
    ///
    /// A batch that does not fit is truncated (its first datagrams are
    /// queued, the rest dropped), so the ring needs room for a whole batch
    /// for the recovery to succeed: see [`telemetry_channel`].
    pub fn push_batch(&mut self, batch: &DatagramBatch) -> bool {
        let mut all = true;
        for d in batch.datagrams() {
            all &= self.push(d);
        }
        all
    }

    /// Datagrams dropped because the ring was full.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// Consumer end of the datagram ring.
pub struct TelemetryReceiver {
    rx: SpscConsumer<Datagram>,
    dropped: Arc<AtomicU64>,
}

impl TelemetryReceiver {
    /// The oldest queued datagram.
    pub fn pop(&mut self) -> Option<Datagram> {
        self.rx.pop()
    }

    /// Datagrams the producer dropped.
    pub fn dropped(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }
}

/// Owns the UDP socket and sends queued datagrams; runs on a non-sim thread
/// ([`spawn`](Self::spawn)) or is pumped by its owner.
pub struct TelemetrySender {
    rx: TelemetryReceiver,
    socket: UdpSocket,
    dest: SocketAddr,
    sent: u64,
    errors: u64,
}

impl TelemetrySender {
    /// Binds an ephemeral UDP socket of `dest`'s address family and sends to
    /// `dest` (default [`DEFAULT_DESTINATION`](super::DEFAULT_DESTINATION)).
    pub fn new(rx: TelemetryReceiver, dest: impl ToSocketAddrs) -> io::Result<Self> {
        let dest = dest
            .to_socket_addrs()?
            .next()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "no address"))?;
        let bind: SocketAddr = if dest.is_ipv4() {
            (Ipv4Addr::UNSPECIFIED, 0).into()
        } else {
            (Ipv6Addr::UNSPECIFIED, 0).into()
        };
        Ok(Self::with_socket(rx, UdpSocket::bind(bind)?, dest))
    }

    /// Uses an existing socket.
    pub fn with_socket(rx: TelemetryReceiver, socket: UdpSocket, dest: SocketAddr) -> Self {
        Self {
            rx,
            socket,
            dest,
            sent: 0,
            errors: 0,
        }
    }

    /// Sends the queued datagrams, at most the ring's capacity per call (so
    /// a producer refilling the ring concurrently cannot keep one call
    /// running forever); returns how many were taken from the ring. A
    /// failed send (e.g. no listener on loopback) is counted in
    /// [`send_errors`](Self::send_errors) and skipped.
    pub fn pump(&mut self) -> usize {
        let limit = self.rx.rx.capacity();
        let mut n = 0;
        while n < limit
            && let Some(d) = self.rx.pop()
        {
            match self.socket.send_to(d.as_bytes(), self.dest) {
                Ok(_) => self.sent += 1,
                Err(_) => self.errors += 1,
            }
            n += 1;
        }
        n
    }

    /// Datagrams sent.
    pub fn sent(&self) -> u64 {
        self.sent
    }

    /// Sends that failed.
    pub fn send_errors(&self) -> u64 {
        self.errors
    }

    /// Datagrams the sim thread dropped on a full ring.
    pub fn dropped(&self) -> u64 {
        self.rx.dropped()
    }

    /// The destination.
    pub fn destination(&self) -> SocketAddr {
        self.dest
    }

    /// The local socket address.
    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Moves the sender to its own thread ("pin-telemetry"), which pumps the
    /// ring and parks for up to `poll` whenever it is empty. On stop it does
    /// one final bounded [`pump`](Self::pump), so a producer that keeps
    /// pushing cannot delay shutdown. A datagram can
    /// therefore leave up to `poll` after the sim pushed it: use
    /// `poll ≤ 1 ms` (one tick) in the app.
    pub fn spawn(mut self, poll: Duration) -> io::Result<TelemetrySenderHandle> {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = Arc::clone(&stop);
        let thread = std::thread::Builder::new()
            .name("pin-telemetry".into())
            .spawn(move || {
                while !flag.load(Ordering::Acquire) {
                    if self.pump() == 0 {
                        std::thread::park_timeout(poll);
                    }
                }
                self.pump();
                self
            })?;
        Ok(TelemetrySenderHandle {
            stop,
            thread: Some(thread),
        })
    }
}

/// Handle of a spawned [`TelemetrySender`]; stops it on drop.
pub struct TelemetrySenderHandle {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<TelemetrySender>>,
}

impl TelemetrySenderHandle {
    /// Stops the thread after a final pump and returns the sender (`None` if
    /// the thread panicked).
    pub fn stop(mut self) -> Option<TelemetrySender> {
        self.shutdown()
    }

    fn shutdown(&mut self) -> Option<TelemetrySender> {
        self.stop.store(true, Ordering::Release);
        let t = self.thread.take()?;
        t.thread().unpark();
        t.join().ok()
    }
}

impl Drop for TelemetrySenderHandle {
    fn drop(&mut self) {
        let _ = self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::super::{TelemetryEncoder, TelemetryState, decode_datagram};
    use super::*;

    #[test]
    fn ring_drops_and_sender_sends() {
        let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
        listener
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let (mut tx, rx) = telemetry_channel(0);
        let mut enc = TelemetryEncoder::new();
        let mut batch = DatagramBatch::new();
        enc.encode_tick(7, &TelemetryState::new(), &[], &mut batch);
        assert!(!tx.push_batch(&batch));
        assert_eq!(tx.dropped(), batch.len() as u64 - 1);
        let mut s = TelemetrySender::new(rx, listener.local_addr().unwrap()).unwrap();
        assert_eq!(s.destination(), listener.local_addr().unwrap());
        assert!(s.local_addr().is_ok());
        assert_eq!(s.dropped(), tx.dropped());
        assert_eq!(s.pump(), 1);
        assert_eq!((s.sent(), s.send_errors()), (1, 0));
        let mut buf = [0u8; 2048];
        let n = listener.recv(&mut buf).unwrap();
        assert_eq!(decode_datagram(&buf[..n]).unwrap().0.tick, 7);

        let h = s.spawn(Duration::from_millis(1)).unwrap();
        assert!(tx.push(&batch.datagrams()[1]));
        let n = listener.recv(&mut buf).unwrap();
        assert_eq!(&buf[..n], batch.get(1).unwrap());
        let s = h.stop().unwrap();
        assert_eq!(s.sent(), 2);
        // IPv6 may be unavailable in CI containers; only the error path is certain.
        let _ = TelemetrySender::new(s.rx, "[::1]:9");
        let (_, rx3) = telemetry_channel(1);
        assert!(TelemetrySender::new(rx3, &[][..] as &[SocketAddr]).is_err());
        let (_, rx2) = telemetry_channel(1);
        let h = TelemetrySender::new(rx2, "127.0.0.1:9")
            .unwrap()
            .spawn(Duration::from_millis(1))
            .unwrap();
        drop(h);
    }

    /// With the documented minimum capacity, a full state that overflowed a
    /// busy ring is delivered whole by the forced full state of the next
    /// tick when the consumer pumps between ticks.
    #[test]
    fn forced_full_state_recovers_at_minimum_capacity() {
        use super::super::MAX_DATAGRAMS_PER_TICK;
        let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
        listener
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let (mut tx, rx) = telemetry_channel(MAX_DATAGRAMS_PER_TICK);
        let mut s = TelemetrySender::new(rx, listener.local_addr().unwrap()).unwrap();
        let mut enc = TelemetryEncoder::new();
        let st = TelemetryState::new();
        let mut batch = DatagramBatch::new();
        // Tick 0's full state fills the ring, so tick 1's overflows.
        enc.encode_tick(0, &st, &[], &mut batch);
        assert!(tx.push_batch(&batch));
        enc.force_full_state();
        enc.encode_tick(1, &st, &[], &mut batch);
        assert_eq!(batch.len(), MAX_DATAGRAMS_PER_TICK);
        let mut tick = 1;
        let mut ok = tx.push_batch(&batch);
        assert!(!ok, "the first full state must overflow");
        while !ok {
            enc.force_full_state();
            s.pump();
            tick += 1;
            enc.encode_tick(tick, &st, &[], &mut batch);
            ok = tx.push_batch(&batch);
            assert!(tick < 10, "recovery never completed");
        }
        assert_eq!(s.pump(), MAX_DATAGRAMS_PER_TICK);
        let mut buf = [0u8; 2048];
        let mut got = 0;
        loop {
            let n = listener.recv(&mut buf).unwrap();
            let (h, _) = decode_datagram(&buf[..n]).unwrap();
            if h.tick == tick {
                got += 1;
                if got == MAX_DATAGRAMS_PER_TICK {
                    break;
                }
            }
        }
    }

    /// A producer that keeps refilling the ring cannot keep `pump` (or the
    /// spawned thread's shutdown) running forever.
    #[test]
    fn pump_and_stop_are_bounded_under_continuous_production() {
        let dead = UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let (mut tx, rx) = telemetry_channel(4);
        let d = {
            let mut enc = TelemetryEncoder::new();
            let mut batch = DatagramBatch::new();
            enc.encode_tick(0, &TelemetryState::new(), &[], &mut batch);
            batch.datagrams()[0]
        };
        let run = Arc::new(AtomicBool::new(true));
        let producer = {
            let run = Arc::clone(&run);
            std::thread::spawn(move || {
                while run.load(Ordering::Relaxed) {
                    tx.push(&d);
                }
            })
        };
        let mut s = TelemetrySender::new(rx, dead).unwrap();
        for _ in 0..100 {
            assert!(s.pump() <= 4);
        }
        let h = s.spawn(Duration::from_millis(1)).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        assert!(h.stop().is_some());
        run.store(false, Ordering::Relaxed);
        producer.join().unwrap();
    }
}
