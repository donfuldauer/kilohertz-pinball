//! Independent Tester spec tests, Subphase 3.4, verification criterion 2 and
//! deliverable 4's sender: datagrams travel sim thread → ring → a thread that
//! owns a `std::net::UdpSocket` → a real loopback receiver.
//!
//! Black box: the brief leaves the sender API open, so these tests use the
//! public signatures and documentation of `pin_audio::telemetry`
//! (`telemetry_channel`, `TelemetryTx`, `TelemetrySender`). No sleeps: every
//! wait is a blocking `recv` with a timeout.

// Style lints only; test code favours explicit index loops and step-by-step setup.
#![allow(
    clippy::field_reassign_with_default,
    clippy::manual_is_multiple_of,
    clippy::needless_range_loop,
    clippy::type_complexity
)]

use pin_audio::telemetry::{
    DatagramBatch, TelemetryEncoder, TelemetrySender, TelemetryState, decode_datagram,
    telemetry_channel,
};
use pin_core::input::HapticsTelemetryEvent;
use pin_core::telemetry::{
    DEFAULT_DESTINATION, FLAG_FULL_STATE, MAX_DATAGRAM_LEN, TelemetryEntry, TelemetryHeader,
    TelemetryKind,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::net::{SocketAddr, UdpSocket};
use std::time::Duration;

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

type Decoded = (TelemetryHeader, Vec<TelemetryEntry>);

fn listener() -> (UdpSocket, SocketAddr) {
    let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
    rx.set_read_timeout(Some(Duration::from_secs(20))).unwrap();
    let addr = rx.local_addr().unwrap();
    (rx, addr)
}

fn recv(rx: &UdpSocket) -> (Vec<u8>, SocketAddr) {
    let mut buf = [0u8; 4096];
    let (n, from) = rx
        .recv_from(&mut buf)
        .expect("datagram did not arrive on loopback");
    assert!(n <= MAX_DATAGRAM_LEN);
    (buf[..n].to_vec(), from)
}

fn recv_decoded(rx: &UdpSocket) -> Decoded {
    decode_datagram(&recv(rx).0).expect("received datagram must decode")
}

fn assert_nothing_queued(rx: &UdpSocket) {
    rx.set_nonblocking(true).unwrap();
    let mut buf = [0u8; 4096];
    assert!(rx.recv_from(&mut buf).is_err(), "unexpected extra datagram");
    rx.set_nonblocking(false).unwrap();
}

#[test]
fn default_destination_is_appendix_b() {
    assert_eq!(DEFAULT_DESTINATION, "127.0.0.1:35120");
    assert_eq!(pin_audio::telemetry::DEFAULT_DESTINATION, "127.0.0.1:35120");
    let (_tx, ring_rx) = telemetry_channel(4);
    let sender = TelemetrySender::new(ring_rx, DEFAULT_DESTINATION).unwrap();
    assert_eq!(
        sender.destination(),
        "127.0.0.1:35120".parse::<SocketAddr>().unwrap()
    );
    assert_ne!(
        sender.local_addr().unwrap().port(),
        0,
        "the sender owns a bound socket"
    );
}

/// Criterion 2, end to end and single-threaded (deterministic): the coil
/// pulse of tick T reaches a loopback socket in a datagram stamped T.
#[test]
fn flipper_pulse_reaches_a_loopback_receiver_with_its_tick() {
    const T: u64 = 98_765;
    let (rx, addr) = listener();
    let (mut tx, ring_rx) = telemetry_channel(64);
    let mut sender = TelemetrySender::new(ring_rx, addr).unwrap();
    assert_eq!(sender.destination(), addr);

    let mut state = Box::new(TelemetryState::default());
    let mut enc = TelemetryEncoder::new();
    let mut batch = Box::new(DatagramBatch::new());

    // Table load.
    enc.force_full_state();
    enc.encode_tick(T - 10, &state, &[], &mut batch);
    let load_count = batch.iter().count();
    assert!(load_count >= 1);
    assert!(tx.push_batch(&batch));
    assert_eq!(sender.pump(), load_count);
    for _ in 0..load_count {
        let (h, _) = recv_decoded(&rx);
        assert_eq!(h.tick, T - 10);
        assert_ne!(h.flags & FLAG_FULL_STATE, 0);
    }

    // Quiet ticks: nothing is queued, nothing is sent.
    for t in T - 9..T {
        enc.encode_tick(t, &state, &[], &mut batch);
        assert_eq!(batch.iter().count(), 0);
        assert!(tx.push_batch(&batch));
        assert_eq!(sender.pump(), 0);
    }
    assert_nothing_queued(&rx);

    // The flipper fires at T.
    state.coils[4] = true;
    let impulse = HapticsTelemetryEvent {
        tick: T,
        source: 4,
        strength: 1.0,
    };
    enc.encode_tick(T, &state, &[impulse], &mut batch);
    assert_eq!(batch.iter().count(), 1);
    let expected = batch.iter().next().unwrap().to_vec();
    assert!(tx.push_batch(&batch));
    assert_eq!(sender.pump(), 1);

    let (bytes, from) = recv(&rx);
    assert!(from.ip().is_loopback());
    assert_eq!(
        from.port(),
        sender.local_addr().unwrap().port(),
        "sent from the sender's socket"
    );
    assert_eq!(
        bytes, expected,
        "the wire carries the encoder's bytes unchanged"
    );
    let (h, entries) = decode_datagram(&bytes).unwrap();
    assert_eq!(h.tick, T);
    assert_eq!(h.flags & FLAG_FULL_STATE, 0);
    assert!(entries.contains(&TelemetryEntry {
        kind: TelemetryKind::Coil,
        id: 4,
        value: 1.0
    }));
    assert!(entries.contains(&TelemetryEntry {
        kind: TelemetryKind::HapticImpulse,
        id: 4,
        value: 1.0
    }));
    assert_nothing_queued(&rx);
    assert_eq!(sender.sent(), load_count as u64 + 1);
    assert_eq!(sender.send_errors(), 0);
    assert_eq!(sender.dropped(), 0);
}

/// The real arrangement: the encoder on this ("sim") thread, the socket on
/// the sender's own thread. A listener that applies everything it receives
/// ends up with the table state; datagrams arrive in tick order.
#[test]
fn spawned_sender_thread_delivers_every_tick_in_order() {
    let (rx, addr) = listener();
    let (mut tx, ring_rx) = telemetry_channel(64);
    let handle = TelemetrySender::new(ring_rx, addr)
        .unwrap()
        .spawn(Duration::from_millis(1))
        .unwrap();

    let mut state = Box::new(TelemetryState::default());
    let mut enc = TelemetryEncoder::new();
    let mut batch = Box::new(DatagramBatch::new());
    let mut pushed = 0u64;
    let mut expected: Vec<Vec<u8>> = Vec::new();
    let mut seen_switch = [0.0f32; 256];
    let mut seen_lamp = [0.0f32; 256];

    enc.force_full_state();
    for t in 0..400u64 {
        if t % 4 == 1 {
            let i = (t * 7 % 256) as usize;
            state.switches[i] = !state.switches[i];
            state.lamps[(t % 256) as usize] = (t % 10) as f32 / 10.0;
        }
        let impulses: &[HapticsTelemetryEvent] = if t % 50 == 49 {
            &[HapticsTelemetryEvent {
                tick: t,
                source: 3,
                strength: 0.5,
            }]
        } else {
            &[]
        };
        enc.encode_tick(t, &state, impulses, &mut batch);
        let n = batch.iter().count();
        assert!(tx.push_batch(&batch), "tick {t}: ring overflowed");
        expected.extend(batch.iter().map(<[u8]>::to_vec));
        pushed += n as u64;
        // Take this tick's datagrams off the wire before the next tick, so
        // neither the ring nor the socket buffer can overflow.
        for _ in 0..n {
            let (bytes, _) = recv(&rx);
            let want = &expected[expected.len() - n..];
            assert!(want.contains(&bytes), "tick {t}: unexpected datagram");
            let (h, entries) = decode_datagram(&bytes).unwrap();
            assert_eq!(h.tick, t, "datagrams arrive on their own tick, in order");
            for e in entries {
                match e.kind {
                    TelemetryKind::Switch => seen_switch[usize::from(e.id)] = e.value,
                    TelemetryKind::Lamp => seen_lamp[usize::from(e.id)] = e.value,
                    _ => {}
                }
            }
        }
    }
    assert!(pushed > 100);
    for i in 0..256 {
        assert_eq!(
            seen_switch[i],
            if state.switches[i] { 1.0 } else { 0.0 },
            "switch {i}"
        );
        assert_eq!(seen_lamp[i], state.lamps[i], "lamp {i}");
    }
    let sender = handle.stop().expect("sender thread must not panic");
    assert_eq!(sender.sent(), pushed);
    assert_eq!(sender.send_errors(), 0);
    assert_eq!(sender.dropped(), 0);
    assert_eq!(tx.dropped(), 0);
    assert_nothing_queued(&rx);
}

#[test]
fn stop_flushes_what_is_still_queued() {
    let (rx, addr) = listener();
    let (mut tx, ring_rx) = telemetry_channel(16);
    // A very long poll: only the final pump of `stop` (or a wake-up) can send.
    let handle = TelemetrySender::new(ring_rx, addr)
        .unwrap()
        .spawn(Duration::from_secs(3600))
        .unwrap();
    let state = Box::new(TelemetryState::default());
    let mut enc = TelemetryEncoder::new();
    let mut batch = Box::new(DatagramBatch::new());
    enc.force_full_state();
    enc.encode_tick(12, &state, &[], &mut batch);
    let n = batch.iter().count();
    assert!(tx.push_batch(&batch));
    let sender = handle.stop().expect("sender thread must not panic");
    assert_eq!(sender.sent(), n as u64);
    for _ in 0..n {
        assert_eq!(recv_decoded(&rx).0.tick, 12);
    }
}

#[test]
fn dropping_the_handle_stops_the_thread() {
    let (_rx, addr) = listener();
    let (_tx, ring_rx) = telemetry_channel(4);
    let handle = TelemetrySender::new(ring_rx, addr)
        .unwrap()
        .spawn(Duration::from_secs(3600))
        .unwrap();
    drop(handle); // must return (join) instead of hanging the test
}

#[test]
fn full_ring_drops_and_counts_without_blocking() {
    let (rx, addr) = listener();
    let (mut tx, ring_rx) = telemetry_channel(2);
    let mut sender = TelemetrySender::new(ring_rx, addr).unwrap();
    let mut state = Box::new(TelemetryState::default());
    let mut enc = TelemetryEncoder::new();
    let mut batch = Box::new(DatagramBatch::new());
    enc.force_full_state();
    enc.encode_tick(0, &state, &[], &mut batch);
    let n = batch.iter().count();
    assert!(
        n > 2,
        "a full state (768 + GI + magnets entries) needs several datagrams"
    );
    assert!(!tx.push_batch(&batch), "5 datagrams cannot fit a ring of 2");
    assert_eq!(tx.dropped(), n as u64 - 2);
    assert_eq!(sender.dropped(), n as u64 - 2);
    assert_eq!(sender.pump(), 2);
    assert_eq!(sender.pump(), 0);
    // The two that fit are the first two, in order.
    let first: Vec<Vec<u8>> = batch.iter().take(2).map(<[u8]>::to_vec).collect();
    assert_eq!(recv(&rx).0, first[0]);
    assert_eq!(recv(&rx).0, first[1]);
    // The ring works again afterwards.
    state.coils[1] = true;
    enc.encode_tick(1, &state, &[], &mut batch);
    assert!(tx.push_batch(&batch));
    assert_eq!(sender.pump(), 1);
    assert_eq!(recv_decoded(&rx).0.tick, 1);
    assert_eq!(tx.dropped(), n as u64 - 2);
}

#[test]
fn sending_to_nobody_neither_panics_nor_stalls() {
    // Bind and drop to get a loopback port with no listener.
    let dead = {
        let s = UdpSocket::bind("127.0.0.1:0").unwrap();
        s.local_addr().unwrap()
    };
    let (mut tx, ring_rx) = telemetry_channel(64);
    let mut sender = TelemetrySender::new(ring_rx, dead).unwrap();
    let mut state = Box::new(TelemetryState::default());
    let mut enc = TelemetryEncoder::new();
    let mut batch = Box::new(DatagramBatch::new());
    let mut pushed = 0;
    let mut pumped = 0;
    for t in 0..50u64 {
        state.coils[(t % 256) as usize] = true;
        enc.encode_tick(t, &state, &[], &mut batch);
        pushed += batch.iter().count();
        assert!(tx.push_batch(&batch));
        pumped += sender.pump();
    }
    assert_eq!(
        pumped, pushed,
        "failed sends are skipped, the ring keeps draining"
    );
    assert_eq!(sender.sent() + sender.send_errors(), pushed as u64);
}

#[test]
fn counter_detects_allocation() {
    let before = allocs();
    let v = std::hint::black_box(vec![1u8; 64]);
    assert!(allocs() > before);
    drop(v);
}

/// The sim-thread side (encode + push into the ring) never allocates, also
/// when the ring is full.
#[test]
fn sim_side_push_does_not_allocate() {
    let (mut tx, mut ring_rx) = telemetry_channel(8);
    let mut state = Box::new(TelemetryState::default());
    let mut enc = TelemetryEncoder::new();
    let mut batch = Box::new(DatagramBatch::new());
    let mut popped = 0u64;
    let mut pushed = 0u64;
    let before = allocs();
    for t in 0..3000u64 {
        state.switches[(t % 256) as usize] ^= true;
        enc.encode_tick(t, &state, &[], &mut batch);
        pushed += batch.iter().count() as u64;
        if !tx.push_batch(&batch) {
            enc.force_full_state();
        }
        if t % 16 == 0 {
            while let Some(d) = ring_rx.pop() {
                popped += u64::from(d.as_bytes().len() >= 16);
            }
        }
    }
    let dropped = tx.dropped();
    let after = allocs();
    assert_eq!(after - before, 0, "encode/push/pop allocated");
    assert!(dropped > 0, "the scenario must overflow the ring");
    while ring_rx.pop().is_some() {
        popped += 1;
    }
    assert_eq!(popped + dropped, pushed);
    assert_eq!(ring_rx.dropped(), dropped);
}
