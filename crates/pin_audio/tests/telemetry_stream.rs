//! Subphase 3.4 verification 2 and 3 (Implementer tests): UDP telemetry
//! encoder, splitting, full-state cadence, round trip, loopback delivery,
//! decoder robustness, allocation guard and a 10-run stable stream hash.

use pin_audio::telemetry::{
    DatagramBatch, FLAG_FULL_STATE, FULL_STATE_ENTRIES, FULL_STATE_INTERVAL_TICKS,
    HapticsTelemetryEvent, MAX_DATAGRAM_LEN, TelemetryEncoder, TelemetryEntry, TelemetryKind,
    TelemetrySender, TelemetryState, decode_datagram, telemetry_channel,
};
use proptest::prelude::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::net::UdpSocket;
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

fn decode_all(b: &DatagramBatch) -> Vec<(u64, u8, Vec<TelemetryEntry>)> {
    b.iter()
        .map(|d| {
            assert!(d.len() <= MAX_DATAGRAM_LEN);
            let (h, e) = decode_datagram(d).unwrap();
            (h.tick, h.flags, e)
        })
        .collect()
}

/// A scripted cabinet: deterministic state changes per tick.
fn script(tick: u64, st: &mut TelemetryState, imp: &mut Vec<HapticsTelemetryEvent>) {
    imp.clear();
    let t = tick as usize;
    if t.is_multiple_of(7) {
        st.switches[t % 256] = !st.switches[t % 256];
    }
    if t.is_multiple_of(50) {
        st.coils[(t / 50) % 8] = !st.coils[(t / 50) % 8];
        imp.push(HapticsTelemetryEvent {
            tick,
            source: (t % 8) as u16,
            strength: 0.75,
        });
    }
    if t.is_multiple_of(3) {
        st.lamps[(t / 3) % 256] = ((t % 11) as f32) / 10.0;
    }
    if t.is_multiple_of(400) {
        st.gi[(t / 400) % 32] = 0.5;
        st.magnets[(t / 400) % 32] = 1.0;
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
fn flipper_pulse_reaches_a_loopback_listener_on_the_same_tick() {
    let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
    listener
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let (mut tx, rx) = telemetry_channel(16);
    let mut sender = TelemetrySender::new(rx, listener.local_addr().unwrap()).unwrap();

    let mut enc = TelemetryEncoder::new();
    let mut st = TelemetryState::default();
    let mut out = DatagramBatch::new();
    // Load: full state, then quiet ticks.
    enc.encode_tick(0, &st, &[], &mut out);
    assert!(decode_all(&out).iter().all(|d| d.1 & FLAG_FULL_STATE != 0));
    for t in 1..500 {
        enc.encode_tick(t, &st, &[], &mut out);
        assert!(out.is_empty(), "tick {t}");
    }
    const T: u64 = 500;
    st.coils[12] = true;
    let pulse = HapticsTelemetryEvent {
        tick: T,
        source: 12,
        strength: 1.0,
    };
    enc.encode_tick(T, &st, &[pulse], &mut out);
    assert_eq!(out.len(), 1);
    assert!(tx.push_batch(&out));
    assert_eq!(sender.pump(), 1);

    let mut buf = [0u8; 2048];
    let n = listener.recv(&mut buf).unwrap();
    assert_eq!(&buf[..n], out.get(0).unwrap());
    let (h, e) = decode_datagram(&buf[..n]).unwrap();
    assert_eq!(h.tick, T);
    assert!(!h.is_full_state());
    assert_eq!(
        e,
        vec![
            TelemetryEntry {
                kind: TelemetryKind::Coil,
                id: 12,
                value: 1.0
            },
            TelemetryEntry {
                kind: TelemetryKind::HapticImpulse,
                id: 12,
                value: 1.0
            },
        ]
    );
    // The coil releasing is the next datagram.
    st.coils[12] = false;
    enc.encode_tick(T + 30, &st, &[], &mut out);
    assert_eq!(decode_all(&out)[0].2[0].value, 0.0);
}

#[test]
fn full_state_on_load_and_every_1000_ticks() {
    let mut enc = TelemetryEncoder::new();
    let st = TelemetryState::default();
    let mut out = DatagramBatch::new();
    let mut full_ticks = Vec::new();
    for t in 0..3500u64 {
        if t == 1700 {
            enc.force_full_state();
        }
        enc.encode_tick(t, &st, &[], &mut out);
        if !out.is_empty() {
            let ds = decode_all(&out);
            assert!(ds.iter().all(|d| d.0 == t && d.1 == FLAG_FULL_STATE));
            let n: usize = ds.iter().map(|d| d.2.len()).sum();
            assert_eq!(n, FULL_STATE_ENTRIES);
            full_ticks.push(t);
        }
    }
    assert_eq!(full_ticks, vec![0, 1000, 1700, 2700]);
    assert_eq!(FULL_STATE_INTERVAL_TICKS, 1000);
}

#[test]
fn more_than_180_changes_split_with_the_same_tick() {
    let mut enc = TelemetryEncoder::new();
    let mut st = TelemetryState::default();
    let mut out = DatagramBatch::new();
    enc.encode_tick(10, &st, &[], &mut out);
    for i in 0..256 {
        st.lamps[i] = 1.0;
        st.switches[i] = i % 2 == 0;
    }
    enc.encode_tick(11, &st, &[], &mut out);
    let ds = decode_all(&out);
    assert_eq!(ds.len(), 3);
    assert!(ds.iter().all(|d| d.0 == 11 && d.1 == 0));
    assert_eq!(
        ds.iter().map(|d| d.2.len()).collect::<Vec<_>>(),
        vec![180, 180, 24]
    );
    let all: Vec<_> = ds.into_iter().flat_map(|d| d.2).collect();
    assert!(all[..128].iter().all(|e| e.kind == TelemetryKind::Switch));
    assert!(all[128..].iter().all(|e| e.kind == TelemetryKind::Lamp));
    assert!(all[128..].windows(2).all(|w| w[0].id < w[1].id));
}

#[test]
fn encode_tick_and_ring_never_allocate() {
    let mut enc = TelemetryEncoder::new();
    let mut st = TelemetryState::default();
    let mut out = DatagramBatch::new();
    let mut imp = Vec::with_capacity(8);
    let (mut tx, mut rx) = telemetry_channel(8);
    let before = allocs();
    for t in 0..5000u64 {
        script(t, &mut st, &mut imp);
        enc.encode_tick(t, &st, &imp, &mut out);
        tx.push_batch(&out);
        while rx.pop().is_some() {}
    }
    assert_eq!(allocs(), before, "telemetry sim-thread path allocated");
}

#[test]
fn stream_hash_is_stable_over_ten_runs() {
    fn run() -> u64 {
        let mut enc = TelemetryEncoder::new();
        let mut st = TelemetryState::default();
        let mut out = DatagramBatch::new();
        let mut imp = Vec::new();
        // FNV-1a over every datagram byte.
        let mut h = 0xcbf2_9ce4_8422_2325u64;
        for t in 0..5000u64 {
            script(t, &mut st, &mut imp);
            enc.encode_tick(t, &st, &imp, &mut out);
            for d in &out {
                for &b in d {
                    h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
                }
            }
        }
        h
    }
    let first = run();
    for _ in 0..9 {
        assert_eq!(run(), first);
    }
}

fn entry() -> impl Strategy<Value = (usize, u8, f32)> {
    (
        0usize..256,
        0u8..5,
        prop_oneof![0.0f32..=1.0, Just(f32::NAN), -2.0f32..3.0],
    )
}

proptest! {
    #[test]
    fn decoded_stream_reconstructs_the_state(
        ticks in prop::collection::vec(prop::collection::vec(entry(), 0..300), 1..20),
        strengths in prop::collection::vec(0.0f32..=1.0, 0..5),
    ) {
        let mut enc = TelemetryEncoder::new();
        let mut st = TelemetryState::default();
        let mut out = DatagramBatch::new();
        // Listener-side mirror, rebuilt only from decoded datagrams.
        let mut mirror = TelemetryState::default();
        let imp: Vec<_> = strengths.iter().enumerate().map(|(i, &s)| HapticsTelemetryEvent {
            tick: 0, source: i as u16, strength: s }).collect();
        for (t, changes) in ticks.iter().enumerate() {
            for &(id, kind, v) in changes {
                match kind {
                    0 => st.switches[id] = v > 0.5,
                    1 => st.coils[id] = v > 0.5,
                    2 => st.lamps[id] = v,
                    3 => st.gi[id % 32] = v,
                    _ => st.magnets[id % 32] = v,
                }
            }
            enc.encode_tick(t as u64, &st, &imp, &mut out);
            let mut impulses = 0;
            for d in &out {
                let (h, es) = decode_datagram(d).unwrap();
                prop_assert_eq!(h.tick, t as u64);
                // Bit-exact round trip of the raw bytes.
                let mut re = [0u8; MAX_DATAGRAM_LEN];
                let n = pin_core::telemetry::encode_datagram(h.tick, h.flags, &es, &mut re).unwrap();
                prop_assert_eq!(&re[..n], d);
                for e in es {
                    let i = usize::from(e.id);
                    match e.kind {
                        TelemetryKind::Switch => mirror.switches[i] = e.value == 1.0,
                        TelemetryKind::Coil => mirror.coils[i] = e.value == 1.0,
                        TelemetryKind::Lamp => mirror.lamps[i] = e.value,
                        TelemetryKind::Gi => mirror.gi[i] = e.value,
                        TelemetryKind::Magnet => mirror.magnets[i] = e.value,
                        TelemetryKind::HapticImpulse => impulses += 1,
                    }
                }
            }
            prop_assert_eq!(impulses, imp.len());
            prop_assert_eq!(mirror.switches, st.switches);
            prop_assert_eq!(mirror.coils, st.coils);
            let norm = |v: f32| if v > 0.0 { v.min(1.0) } else { 0.0 };
            for i in 0..256 {
                prop_assert_eq!(mirror.lamps[i].to_bits(), norm(st.lamps[i]).to_bits());
            }
            for i in 0..32 {
                prop_assert_eq!(mirror.gi[i].to_bits(), norm(st.gi[i]).to_bits());
                prop_assert_eq!(mirror.magnets[i].to_bits(), norm(st.magnets[i]).to_bits());
            }
        }
    }

    #[test]
    fn decoder_never_panics_on_random_bytes(bytes in prop::collection::vec(any::<u8>(), 0..3000)) {
        let _ = decode_datagram(&bytes);
    }

    #[test]
    fn decoder_never_panics_on_mutated_datagrams(
        flips in prop::collection::vec((0usize..1456, any::<u8>()), 1..16),
        cut in 0usize..1500,
    ) {
        let mut enc = TelemetryEncoder::new();
        let mut out = DatagramBatch::new();
        enc.encode_tick(3, &TelemetryState::default(), &[], &mut out);
        let mut d = out.get(0).unwrap().to_vec();
        for (i, b) in flips {
            if i < d.len() {
                d[i] = b;
            }
        }
        d.truncate(cut);
        if let Ok((h, e)) = decode_datagram(&d) {
            prop_assert_eq!(usize::from(h.entry_count), e.len());
        }
    }
}

// ------------------------------------------------------- review fix round 1

/// N2: a full state with the maximum 64 impulses (896 entries) fits in
/// `MAX_DATAGRAMS_PER_TICK` = 5 datagrams, nothing is cut off.
#[test]
fn worst_case_tick_fits_in_five_datagrams() {
    use pin_audio::telemetry::{MAX_DATAGRAMS_PER_TICK, MAX_IMPULSES_PER_TICK};
    assert_eq!(MAX_DATAGRAMS_PER_TICK, 5);
    let mut enc = TelemetryEncoder::new();
    let mut out = DatagramBatch::new();
    let imp: Vec<_> = (0..MAX_IMPULSES_PER_TICK as u16)
        .map(|source| HapticsTelemetryEvent {
            tick: 0,
            source,
            strength: 0.5,
        })
        .collect();
    enc.encode_tick(0, &TelemetryState::default(), &imp, &mut out);
    let ds = decode_all(&out);
    assert_eq!(ds.len(), 5);
    let n: usize = ds.iter().map(|d| d.2.len()).sum();
    assert_eq!(n, FULL_STATE_ENTRIES + MAX_IMPULSES_PER_TICK);
    assert_eq!(enc.dropped_impulses(), 0);
}

/// N3: a new session (lower tick) arrives as a full state, which listeners
/// must accept although its tick is below the last full-state tick.
#[test]
fn new_session_with_lower_tick_is_a_full_state() {
    let mut enc = TelemetryEncoder::new();
    let st = TelemetryState::default();
    let mut out = DatagramBatch::new();
    enc.encode_tick(5000, &st, &[], &mut out);
    enc.encode_tick(5001, &st, &[], &mut out);
    assert!(out.is_empty());
    enc.encode_tick(3, &st, &[], &mut out);
    let ds = decode_all(&out);
    assert!(!ds.is_empty());
    assert!(ds.iter().all(|d| d.0 == 3 && d.1 & FLAG_FULL_STATE != 0));
}

/// N5: the back-pressure loop: a dropped batch makes the sim loop force a
/// full state, which reaches the listener once the ring drains.
#[test]
fn ring_overflow_then_forced_full_state_resynchronises() {
    let (mut tx, mut rx) = telemetry_channel(2);
    let mut enc = TelemetryEncoder::new();
    let mut st = TelemetryState::default();
    let mut out = DatagramBatch::new();
    enc.encode_tick(0, &st, &[], &mut out);
    // 5 datagrams into a 2-slot ring: overflow.
    assert!(!tx.push_batch(&out));
    enc.force_full_state();
    while rx.pop().is_some() {}
    st.lamps[9] = 0.5;
    enc.encode_tick(1, &st, &[], &mut out);
    assert!(decode_all(&out).iter().all(|d| d.1 & FLAG_FULL_STATE != 0));
    let mut sent = 0;
    for d in out.datagrams() {
        assert!(tx.push(d));
        let got = rx.pop().unwrap();
        assert_eq!(got.as_bytes(), d.as_bytes());
        sent += 1;
    }
    assert_eq!(sent, 5);
    assert_eq!(tx.dropped(), 3);
}
