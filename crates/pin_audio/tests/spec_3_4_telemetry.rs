//! Independent Tester spec tests, Subphase 3.4, verification criteria 2 and 3:
//! the per-tick telemetry differ/encoder, the decoder and the loopback path.
//! Written from the brief (Appendix B) alone; the only implementation-specific
//! parts are the small adapters below.

// Style lints only; test code favours explicit index loops and step-by-step setup.
#![allow(
    clippy::field_reassign_with_default,
    clippy::manual_is_multiple_of,
    clippy::needless_range_loop,
    clippy::type_complexity
)]

use pin_audio::telemetry::{DatagramBatch, TelemetryEncoder, TelemetryState, decode_datagram};
use pin_core::input::HapticsTelemetryEvent;
use pin_core::telemetry::{
    self as proto, FLAG_FULL_STATE, MAX_DATAGRAM_LEN, MAX_ENTRIES_PER_DATAGRAM, TelemetryEntry,
    TelemetryHeader, TelemetryKind,
};
use proptest::prelude::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};
use std::net::UdpSocket;
use std::time::Duration;

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

fn new_state() -> Box<TelemetryState> {
    Box::new(TelemetryState::default())
}

fn new_batch() -> Box<DatagramBatch> {
    Box::new(DatagramBatch::new())
}

/// Copies the datagrams out of a batch.
fn datagrams(batch: &DatagramBatch) -> Vec<Vec<u8>> {
    batch.iter().map(<[u8]>::to_vec).collect()
}

fn gi_len() -> usize {
    TelemetryState::default().gi.len()
}

fn magnet_len() -> usize {
    TelemetryState::default().magnets.len()
}

// ------------------------------------------------------------------ helpers

type Decoded = (TelemetryHeader, Vec<TelemetryEntry>);

/// Encodes one tick into a cleared batch and decodes everything it yields,
/// checking the per-datagram invariants of Appendix B on the way.
fn tick(
    enc: &mut TelemetryEncoder,
    batch: &mut DatagramBatch,
    t: u64,
    state: &TelemetryState,
    impulses: &[HapticsTelemetryEvent],
) -> Vec<Decoded> {
    batch.clear();
    enc.encode_tick(t, state, impulses, batch);
    datagrams(batch)
        .iter()
        .map(|bytes| check_datagram(bytes, t))
        .collect()
}

fn check_datagram(bytes: &[u8], t: u64) -> Decoded {
    assert!(
        bytes.len() <= MAX_DATAGRAM_LEN,
        "datagram of {} bytes",
        bytes.len()
    );
    assert_eq!(&bytes[0..4], b"PNTL", "magic");
    assert_eq!(bytes[4], 1, "version");
    let (h, e) = decode_datagram(bytes).expect("encoder output must decode");
    assert_eq!(h.tick, t, "header tick");
    assert_eq!(usize::from(h.entry_count), e.len());
    assert!(e.len() <= MAX_ENTRIES_PER_DATAGRAM);
    assert_eq!(bytes.len(), 16 + 8 * e.len());
    assert_eq!(
        u64::from_le_bytes(bytes[8..16].try_into().unwrap()),
        t,
        "tick is little-endian at offset 8"
    );
    assert_eq!(
        usize::from(u16::from_le_bytes([bytes[6], bytes[7]])),
        e.len(),
        "entry count is little-endian at offset 6"
    );
    // Bit-exact round trip through the normative pin_core codec.
    let mut again = [0u8; MAX_DATAGRAM_LEN];
    let n = proto::encode_datagram(h.tick, h.flags, &e, &mut again).unwrap();
    assert_eq!(
        &again[..n],
        bytes,
        "decode -> encode must reproduce the bytes"
    );
    // And the pin_core decoder agrees with the pin_audio one.
    let (h2, e2) = proto::decode_datagram(bytes).unwrap();
    assert_eq!(h2, h);
    assert_eq!(e2.as_slice(), e.as_slice());
    (h, e)
}

fn is_full(h: &TelemetryHeader) -> bool {
    h.flags & FLAG_FULL_STATE != 0
}

/// A listener's picture of the table: (kind, id) -> value bits. Absent = 0.
#[derive(Default, Clone, PartialEq, Debug)]
struct Model(BTreeMap<(u8, u16), u32>);

impl Model {
    fn apply(&mut self, decoded: &[Decoded]) {
        for (_, entries) in decoded {
            for e in entries {
                if e.kind != TelemetryKind::HapticImpulse {
                    self.0.insert((e.kind as u8, e.id), e.value.to_bits());
                }
            }
        }
    }

    fn get(&self, kind: TelemetryKind, id: usize) -> f32 {
        f32::from_bits(self.0.get(&(kind as u8, id as u16)).copied().unwrap_or(0))
    }

    fn assert_matches(&self, s: &TelemetryState, ctx: &str) {
        for i in 0..256 {
            let sw = if s.switches[i] { 1.0 } else { 0.0 };
            let co = if s.coils[i] { 1.0 } else { 0.0 };
            assert_eq!(self.get(TelemetryKind::Switch, i), sw, "{ctx}: switch {i}");
            assert_eq!(self.get(TelemetryKind::Coil, i), co, "{ctx}: coil {i}");
            assert_eq!(
                self.get(TelemetryKind::Lamp, i).to_bits(),
                s.lamps[i].to_bits(),
                "{ctx}: lamp {i}"
            );
        }
        for i in 0..s.gi.len() {
            assert_eq!(
                self.get(TelemetryKind::Gi, i).to_bits(),
                s.gi[i].to_bits(),
                "{ctx}: gi {i}"
            );
        }
        for i in 0..s.magnets.len() {
            assert_eq!(
                self.get(TelemetryKind::Magnet, i).to_bits(),
                s.magnets[i].to_bits(),
                "{ctx}: magnet {i}"
            );
        }
    }
}

fn all_entries(decoded: &[Decoded]) -> Vec<TelemetryEntry> {
    decoded
        .iter()
        .flat_map(|(_, e)| e.iter().copied())
        .collect()
}

fn impulses_of(decoded: &[Decoded]) -> Vec<(u16, f32)> {
    all_entries(decoded)
        .into_iter()
        .filter(|e| e.kind == TelemetryKind::HapticImpulse)
        .map(|e| (e.id, e.value))
        .collect()
}

/// A fresh encoder that has already sent its table-load full state at `t0`.
fn loaded(state: &TelemetryState, t0: u64) -> (TelemetryEncoder, Box<DatagramBatch>) {
    let mut enc = TelemetryEncoder::new();
    let mut batch = new_batch();
    enc.force_full_state();
    let d = tick(&mut enc, &mut batch, t0, state, &[]);
    assert!(!d.is_empty(), "table load must send a full-state datagram");
    (enc, batch)
}

fn busy_state() -> Box<TelemetryState> {
    let mut s = new_state();
    for i in 0..256 {
        s.switches[i] = i % 3 == 0;
        s.coils[i] = i % 7 == 1;
        s.lamps[i] = (i % 11) as f32 / 10.0;
    }
    for (i, v) in s.gi.iter_mut().enumerate() {
        *v = 1.0 - i as f32 / 64.0;
    }
    for (i, v) in s.magnets.iter_mut().enumerate() {
        *v = (i % 4) as f32 / 4.0;
    }
    s
}

// ------------------------------------------------- criterion 2: same tick

#[test]
fn flipper_pulse_is_reported_on_its_own_tick() {
    const T: u64 = 4242;
    const COIL: usize = 17;
    let mut s = new_state();
    let (mut enc, mut batch) = loaded(&s, 4000);
    for t in 4001..T {
        assert!(
            tick(&mut enc, &mut batch, t, &s, &[]).is_empty(),
            "no change at tick {t}, so no datagram"
        );
    }
    s.coils[COIL] = true;
    let impulse = HapticsTelemetryEvent {
        tick: T,
        source: COIL as u16,
        strength: 0.75,
    };
    let d = tick(&mut enc, &mut batch, T, &s, &[impulse]);
    assert_eq!(d.len(), 1, "one datagram for the tick with changes");
    let (h, entries) = &d[0];
    assert_eq!(h.tick, T);
    assert!(
        !is_full(h),
        "an incremental datagram does not set the full-state flag"
    );
    assert!(
        entries.contains(&TelemetryEntry {
            kind: TelemetryKind::Coil,
            id: COIL as u16,
            value: 1.0
        }),
        "coil entry missing: {entries:?}"
    );
    assert!(
        entries.contains(&TelemetryEntry {
            kind: TelemetryKind::HapticImpulse,
            id: COIL as u16,
            value: 0.75
        }),
        "impulse entry missing: {entries:?}"
    );
    assert_eq!(entries.len(), 2, "only what changed: {entries:?}");

    // Exact wire bytes (Appendix B): the two entries in either order.
    let bytes = &datagrams(&batch)[0];
    assert_eq!(bytes.len(), 32);
    assert_eq!(&bytes[..8], &[b'P', b'N', b'T', b'L', 1, 0, 2, 0]);
    assert_eq!(&bytes[8..16], &T.to_le_bytes());
    let coil = [1u8, 0, COIL as u8, 0, 0x00, 0x00, 0x80, 0x3f];
    let imp = [5u8, 0, COIL as u8, 0, 0x00, 0x00, 0x40, 0x3f];
    let got: BTreeSet<&[u8]> = bytes[16..].chunks(8).collect();
    assert_eq!(got, BTreeSet::from([&coil[..], &imp[..]]));

    // A real loopback receiver gets exactly those bytes.
    let rx = UdpSocket::bind("127.0.0.1:0").unwrap();
    rx.set_read_timeout(Some(Duration::from_secs(10))).unwrap();
    let tx = UdpSocket::bind("127.0.0.1:0").unwrap();
    tx.send_to(bytes, rx.local_addr().unwrap()).unwrap();
    let mut buf = [0u8; 2048];
    let (n, from) = rx.recv_from(&mut buf).unwrap();
    assert_eq!(from, tx.local_addr().unwrap());
    assert_eq!(&buf[..n], bytes.as_slice());
    let (h, e) = decode_datagram(&buf[..n]).unwrap();
    assert_eq!(h.tick, T);
    assert!(
        e.iter()
            .any(|e| e.kind == TelemetryKind::Coil && e.value == 1.0)
    );
    assert!(e.iter().any(|e| e.kind == TelemetryKind::HapticImpulse));

    // The coil dropping again is reported on that tick, with value 0.
    s.coils[COIL] = false;
    let d = tick(&mut enc, &mut batch, T + 30, &s, &[]);
    assert_eq!(d.len(), 1);
    assert_eq!(
        d[0].1,
        vec![TelemetryEntry {
            kind: TelemetryKind::Coil,
            id: COIL as u16,
            value: 0.0
        }]
    );
}

#[test]
fn impulse_alone_makes_a_datagram_and_is_not_repeated() {
    let s = new_state();
    let (mut enc, mut batch) = loaded(&s, 1);
    let ev = [
        HapticsTelemetryEvent {
            tick: 2,
            source: 9,
            strength: 1.0,
        },
        HapticsTelemetryEvent {
            tick: 2,
            source: 300,
            strength: 0.25,
        },
    ];
    let d = tick(&mut enc, &mut batch, 2, &s, &ev);
    assert_eq!(d.len(), 1);
    assert!(!is_full(&d[0].0));
    assert_eq!(impulses_of(&d), vec![(9, 1.0), (300, 0.25)]);
    assert_eq!(d[0].1.len(), 2);
    assert!(
        tick(&mut enc, &mut batch, 3, &s, &[]).is_empty(),
        "impulses are one-shot"
    );
}

#[test]
fn impulse_strength_is_clamped_like_pin_core() {
    let s = new_state();
    let (mut enc, mut batch) = loaded(&s, 1);
    let ev = [
        HapticsTelemetryEvent {
            tick: 2,
            source: 1,
            strength: 7.5,
        },
        HapticsTelemetryEvent {
            tick: 2,
            source: 2,
            strength: -3.0,
        },
        HapticsTelemetryEvent {
            tick: 2,
            source: 3,
            strength: f32::NAN,
        },
    ];
    let d = tick(&mut enc, &mut batch, 2, &s, &ev);
    assert_eq!(impulses_of(&d), vec![(1, 1.0), (2, 0.0), (3, 0.0)]);
}

// ------------------------------------------- criterion 3: quiet / full state

#[test]
fn no_datagram_without_changes() {
    for s in [new_state(), busy_state()] {
        let (mut enc, mut batch) = loaded(&s, 10_000);
        for t in 10_001..10_999 {
            let d = tick(&mut enc, &mut batch, t, &s, &[]);
            assert!(d.is_empty(), "tick {t}: unexpected datagram(s) {d:?}");
        }
    }
}

#[test]
fn only_the_changed_element_is_sent() {
    let mut s = busy_state();
    let (mut enc, mut batch) = loaded(&s, 0);
    let cases: [(TelemetryKind, u16, f32); 5] = [
        (TelemetryKind::Switch, 255, 0.0),
        (TelemetryKind::Coil, 0, 1.0),
        (TelemetryKind::Lamp, 128, 0.123_456_79),
        (TelemetryKind::Gi, 0, 0.5),
        (TelemetryKind::Magnet, (magnet_len() - 1) as u16, 0.875),
    ];
    for (n, (kind, id, value)) in cases.into_iter().enumerate() {
        let i = usize::from(id);
        match kind {
            TelemetryKind::Switch => s.switches[i] = value != 0.0,
            TelemetryKind::Coil => s.coils[i] = value != 0.0,
            TelemetryKind::Lamp => s.lamps[i] = value,
            TelemetryKind::Gi => s.gi[i] = value,
            TelemetryKind::Magnet => s.magnets[i] = value,
            TelemetryKind::HapticImpulse => unreachable!(),
        }
        let t = 1 + 2 * n as u64;
        let d = tick(&mut enc, &mut batch, t, &s, &[]);
        assert_eq!(d.len(), 1, "{kind:?} {id}");
        assert!(!is_full(&d[0].0));
        assert_eq!(
            d[0].1,
            vec![TelemetryEntry { kind, id, value }],
            "{kind:?} {id}"
        );
        assert_eq!(d[0].1[0].value.to_bits(), value.to_bits());
        assert!(tick(&mut enc, &mut batch, t + 1, &s, &[]).is_empty());
    }
}

#[test]
fn forced_full_state_carries_the_whole_table() {
    let s = busy_state();
    let mut enc = TelemetryEncoder::new();
    let mut batch = new_batch();
    enc.force_full_state();
    let d = tick(&mut enc, &mut batch, 77, &s, &[]);
    assert!(!d.is_empty());
    for (h, _) in &d {
        assert!(
            is_full(h),
            "every datagram of a full state sets flags bit 0"
        );
        assert_eq!(h.tick, 77);
    }
    let mut m = Model::default();
    m.apply(&d);
    m.assert_matches(&s, "full state");
    let entries = all_entries(&d);
    for e in &entries {
        let limit = match e.kind {
            TelemetryKind::Switch | TelemetryKind::Coil | TelemetryKind::Lamp => 256,
            TelemetryKind::Gi => gi_len(),
            TelemetryKind::Magnet => magnet_len(),
            TelemetryKind::HapticImpulse => panic!("no impulse was fed"),
        };
        assert!(
            usize::from(e.id) < limit,
            "{e:?} is outside the state arrays"
        );
    }
    let keys: BTreeSet<(u8, u16)> = entries.iter().map(|e| (e.kind as u8, e.id)).collect();
    assert_eq!(
        keys.len(),
        entries.len(),
        "an element appears twice in one full state"
    );
    // Minimal splitting: every datagram but the last is full.
    assert_eq!(
        d.len(),
        entries.len().div_ceil(MAX_ENTRIES_PER_DATAGRAM).max(1)
    );

    // force_full_state is one-shot.
    assert!(tick(&mut enc, &mut batch, 78, &s, &[]).is_empty());
    // ...and works again mid-game, even with nothing changed.
    enc.force_full_state();
    let d2 = tick(&mut enc, &mut batch, 79, &s, &[]);
    assert!(!d2.is_empty() && d2.iter().all(|(h, _)| is_full(h)));
    let mut m2 = Model::default();
    m2.apply(&d2);
    assert_eq!(m, m2, "the same state gives the same full-state content");
}

#[test]
fn full_state_of_an_all_off_table_is_still_sent() {
    let s = new_state();
    let mut enc = TelemetryEncoder::new();
    let mut batch = new_batch();
    enc.force_full_state();
    let d = tick(&mut enc, &mut batch, 5, &s, &[]);
    assert!(
        !d.is_empty(),
        "a listener must see the load even if everything is off"
    );
    assert!(d.iter().all(|(h, _)| is_full(h)));
}

#[test]
fn a_new_encoder_resynchronises_a_listener_on_its_first_tick() {
    // "Full state on load": without any explicit call, whatever the first
    // tick sends must be enough for a listener to know the whole table.
    let s = busy_state();
    let mut enc = TelemetryEncoder::new();
    let mut batch = new_batch();
    let d = tick(&mut enc, &mut batch, 0, &s, &[]);
    assert!(!d.is_empty());
    assert!(
        d.iter().any(|(h, _)| is_full(h)),
        "first datagram(s) must be a full state"
    );
    let mut m = Model::default();
    m.apply(&d);
    m.assert_matches(&s, "first tick");
}

fn periodic_full_state_ticks(t0: u64) -> Vec<u64> {
    let s = busy_state();
    let (mut enc, mut batch) = loaded(&s, t0);
    let mut seen = Vec::new();
    for t in t0 + 1..=t0 + 3500 {
        let d = tick(&mut enc, &mut batch, t, &s, &[]);
        if d.is_empty() {
            continue;
        }
        assert!(
            d.iter().all(|(h, _)| is_full(h)),
            "tick {t}: nothing changed, so only a periodic full state may be sent"
        );
        let mut m = Model::default();
        m.apply(&d);
        m.assert_matches(&s, "periodic full state");
        seen.push(t);
    }
    seen
}

#[test]
fn full_state_once_per_second() {
    for t0 in [0u64, 1, 137, 999, 1000, 123_456] {
        let seen = periodic_full_state_ticks(t0);
        assert!(
            seen.len() == 3 || seen.len() == 4,
            "t0 = {t0}: 3500 quiet ticks must hold 3 or 4 full states, got {seen:?}"
        );
        assert!(
            seen[0] - t0 <= 1000,
            "t0 = {t0}: first periodic full state at {} is more than 1 s after load",
            seen[0]
        );
        for w in seen.windows(2) {
            assert_eq!(w[1] - w[0], 1000, "t0 = {t0}: full states at {seen:?}");
        }
    }
}

// ------------------------------------------------------- criterion 3: split

#[test]
fn more_than_180_changes_split_with_the_same_tick() {
    let mut s = new_state();
    let (mut enc, mut batch) = loaded(&s, 500);
    let mut expect = BTreeMap::new();
    for i in 0..200usize {
        s.switches[i] = true;
        expect.insert((TelemetryKind::Switch as u8, i as u16), 1.0f32.to_bits());
    }
    for i in 40..240usize {
        let v = 0.25 + i as f32 / 1024.0;
        s.lamps[i] = v;
        expect.insert((TelemetryKind::Lamp as u8, i as u16), v.to_bits());
    }
    let ev = [HapticsTelemetryEvent {
        tick: 501,
        source: 4,
        strength: 0.5,
    }];
    let d = tick(&mut enc, &mut batch, 501, &s, &ev);
    assert_eq!(d.len(), 3, "401 entries need exactly 3 datagrams");
    let mut got = BTreeMap::new();
    let mut total = 0;
    for (h, entries) in &d {
        assert_eq!(h.tick, 501);
        assert!(!is_full(h));
        assert!(!entries.is_empty() && entries.len() <= 180);
        total += entries.len();
        for e in entries {
            if e.kind != TelemetryKind::HapticImpulse {
                assert!(
                    got.insert((e.kind as u8, e.id), e.value.to_bits())
                        .is_none(),
                    "{e:?} sent twice"
                );
            }
        }
    }
    assert_eq!(total, 401);
    assert_eq!(got, expect);
    assert_eq!(impulses_of(&d), vec![(4, 0.5)]);
    assert!(tick(&mut enc, &mut batch, 502, &s, &[]).is_empty());
}

#[test]
fn exactly_180_changes_fit_one_datagram_and_181_need_two() {
    for n in [179usize, 180, 181] {
        let mut s = new_state();
        let (mut enc, mut batch) = loaded(&s, 0);
        for i in 0..n {
            s.coils[i] = true;
        }
        let d = tick(&mut enc, &mut batch, 1, &s, &[]);
        assert_eq!(d.len(), if n <= 180 { 1 } else { 2 }, "{n} changes");
        assert_eq!(all_entries(&d).len(), n);
    }
}

#[test]
fn every_element_changing_at_once_fits_the_batch() {
    let mut s = new_state();
    let (mut enc, mut batch) = loaded(&s, 0);
    let mut flip = false;
    for t in 1..=6u64 {
        flip = !flip;
        for i in 0..256 {
            s.switches[i] = flip;
            s.coils[i] = flip;
            s.lamps[i] = if flip { 1.0 } else { 0.0 };
        }
        for v in s.gi.iter_mut().chain(s.magnets.iter_mut()) {
            *v = if flip { 1.0 } else { 0.0 };
        }
        let d = tick(&mut enc, &mut batch, t, &s, &[]);
        let n = 768 + gi_len() + magnet_len();
        assert_eq!(all_entries(&d).len(), n, "tick {t}");
        assert_eq!(d.len(), n.div_ceil(180));
        assert!(d.iter().all(|(h, _)| h.tick == t));
    }
}

// -------------------------------------------------------- allocation guard

#[test]
fn counter_detects_allocation() {
    let before = allocs();
    let v = std::hint::black_box(vec![1u8; 64]);
    assert!(allocs() > before);
    drop(v);
}

#[test]
fn encode_tick_does_not_allocate() {
    let mut s = busy_state();
    let mut enc = TelemetryEncoder::new();
    let mut batch = new_batch();
    let ev = [
        HapticsTelemetryEvent {
            tick: 0,
            source: 1,
            strength: 0.5,
        },
        HapticsTelemetryEvent {
            tick: 0,
            source: 2,
            strength: 0.9,
        },
    ];
    let mut produced = 0usize;
    let before = allocs();
    enc.force_full_state();
    for t in 0..2500u64 {
        // Quiet ticks, small changes, a burst over 180 entries, a forced
        // full state and two periodic ones.
        if t % 7 == 0 {
            s.coils[(t % 256) as usize] ^= true;
        }
        if t % 500 == 3 {
            for i in 0..256 {
                s.lamps[i] = (t % 5) as f32 / 5.0;
                s.switches[i] ^= true;
            }
        }
        if t == 1700 {
            enc.force_full_state();
        }
        batch.clear();
        let impulses: &[HapticsTelemetryEvent] = if t % 13 == 0 { &ev } else { &[] };
        enc.encode_tick(t, &s, impulses, &mut batch);
        produced += batch.iter().count();
    }
    let after = allocs();
    assert_eq!(after - before, 0, "encode_tick allocated");
    assert!(
        produced > 300,
        "the scenario must actually produce datagrams"
    );
}

// ------------------------------------------------------------- determinism

#[test]
fn same_inputs_same_bytes_over_10_runs() {
    fn run() -> Vec<Vec<u8>> {
        let mut s = new_state();
        let mut enc = TelemetryEncoder::new();
        let mut batch = new_batch();
        let mut out = Vec::new();
        enc.force_full_state();
        let mut x = 0x2545_f491u32;
        for t in 0..2200u64 {
            for _ in 0..3 {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                let i = (x >> 8) as usize % 256;
                match x % 4 {
                    0 => s.switches[i] ^= true,
                    1 => s.coils[i] ^= true,
                    2 => s.lamps[i] = (x >> 16) as f32 / 65535.0,
                    _ => {}
                }
            }
            batch.clear();
            enc.encode_tick(t, &s, &[], &mut batch);
            out.extend(datagrams(&batch));
        }
        out
    }
    let first = run();
    assert!(first.len() > 2000);
    for _ in 0..9 {
        assert_eq!(run(), first);
    }
}

// ------------------------------------------------------------------ decoder

#[test]
fn decoder_rejects_malformed_datagrams() {
    let e = TelemetryEntry {
        kind: TelemetryKind::Coil,
        id: 9,
        value: 1.0,
    };
    let mut buf = [0u8; 64];
    let n = proto::encode_datagram(5, 0, &[e, e], &mut buf).unwrap();
    assert_eq!(n, 32);
    let good = &buf[..n];
    let (h, entries) = decode_datagram(good).unwrap();
    assert_eq!(
        h,
        TelemetryHeader {
            flags: 0,
            entry_count: 2,
            tick: 5
        }
    );
    assert_eq!(entries, vec![e, e]);

    assert!(decode_datagram(&[]).is_err());
    for cut in 0..n {
        assert!(decode_datagram(&good[..cut]).is_err(), "truncated to {cut}");
    }
    assert!(decode_datagram(&buf[..n + 1]).is_err(), "trailing byte");
    assert!(decode_datagram(&buf[..n + 8]).is_err(), "trailing entry");
    let mut bad = good.to_vec();
    bad[0] = b'X';
    assert!(decode_datagram(&bad).is_err(), "magic");
    for v in [0u8, 2, 255] {
        let mut bad = good.to_vec();
        bad[4] = v;
        assert!(decode_datagram(&bad).is_err(), "version {v}");
    }
    for k in 6u8..=255 {
        let mut bad = good.to_vec();
        bad[24] = k;
        assert!(decode_datagram(&bad).is_err(), "kind {k}");
    }
    // 181 entries is over the limit even if the length is consistent.
    let mut big = vec![0u8; 16 + 181 * 8];
    big[..4].copy_from_slice(b"PNTL");
    big[4] = 1;
    big[6..8].copy_from_slice(&181u16.to_le_bytes());
    assert!(decode_datagram(&big).is_err());
}

fn entry_strategy() -> impl Strategy<Value = TelemetryEntry> {
    (0u8..6, any::<u16>(), any::<u32>()).prop_map(|(k, id, bits)| TelemetryEntry {
        kind: TelemetryKind::try_from(k).unwrap(),
        id,
        value: f32::from_bits(bits),
    })
}

#[derive(Debug, Clone)]
enum Change {
    Switch(u8, bool),
    Coil(u8, bool),
    Lamp(u8, f32),
    Gi(u16, f32),
    Magnet(u16, f32),
    Impulse(u16, f32),
    ForceFull,
}

fn change_strategy() -> impl Strategy<Value = Change> {
    let level = prop_oneof![Just(0.0f32), Just(1.0f32), 0.0f32..=1.0];
    prop_oneof![
        (any::<u8>(), any::<bool>()).prop_map(|(i, v)| Change::Switch(i, v)),
        (any::<u8>(), any::<bool>()).prop_map(|(i, v)| Change::Coil(i, v)),
        (any::<u8>(), level.clone()).prop_map(|(i, v)| Change::Lamp(i, v)),
        (any::<u16>(), level.clone()).prop_map(|(i, v)| Change::Gi(i, v)),
        (any::<u16>(), level.clone()).prop_map(|(i, v)| Change::Magnet(i, v)),
        (any::<u16>(), level).prop_map(|(i, v)| Change::Impulse(i, v)),
        Just(Change::ForceFull),
    ]
}

proptest! {
    #[test]
    fn decoder_survives_random_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..3000)) {
        let ours = decode_datagram(&bytes);
        let core = proto::decode_datagram(&bytes);
        prop_assert_eq!(ours.is_ok(), core.is_ok());
    }

    #[test]
    fn decoder_survives_random_bodies_behind_a_valid_header(
        count in 0u16..400, flags in any::<u8>(), tick in any::<u64>(),
        body in proptest::collection::vec(any::<u8>(), 0..1700),
        kind_mask in 0u8..=7,
    ) {
        let mut bytes = Vec::with_capacity(16 + body.len());
        bytes.extend_from_slice(b"PNTL");
        bytes.push(1);
        bytes.push(flags);
        bytes.extend_from_slice(&count.to_le_bytes());
        bytes.extend_from_slice(&tick.to_le_bytes());
        bytes.extend_from_slice(&body);
        // Make valid kinds likely so the success path is exercised too.
        for chunk in bytes[16..].chunks_mut(8) {
            chunk[0] &= kind_mask;
        }
        let ours = decode_datagram(&bytes);
        let core = proto::decode_datagram(&bytes);
        prop_assert_eq!(ours.is_ok(), core.is_ok());
        if let (Ok((h, e)), Ok((h2, e2))) = (ours, core) {
            prop_assert_eq!(h, h2);
            prop_assert_eq!(h.tick, tick);
            prop_assert_eq!(h.flags, flags);
            prop_assert_eq!(usize::from(h.entry_count), e.len());
            prop_assert_eq!(bytes.len(), 16 + 8 * e.len());
            let a: Vec<_> = e.iter().map(|e| (e.kind, e.id, e.value.to_bits())).collect();
            let b: Vec<_> = e2.iter().map(|e| (e.kind, e.id, e.value.to_bits())).collect();
            prop_assert_eq!(a, b);
        }
    }

    /// encode -> decode is bit-exact for any entries, including NaN payloads.
    #[test]
    fn codec_round_trip_is_bit_exact(
        tick in any::<u64>(), flags in any::<u8>(),
        entries in proptest::collection::vec(entry_strategy(), 0..=180),
    ) {
        let mut buf = [0u8; MAX_DATAGRAM_LEN];
        let n = proto::encode_datagram(tick, flags, &entries, &mut buf).unwrap();
        let (h, got) = decode_datagram(&buf[..n]).unwrap();
        prop_assert_eq!(h, TelemetryHeader { flags, entry_count: entries.len() as u16, tick });
        let a: Vec<_> = got.iter().map(|e| (e.kind, e.id, e.value.to_bits())).collect();
        let b: Vec<_> = entries.iter().map(|e| (e.kind, e.id, e.value.to_bits())).collect();
        prop_assert_eq!(a, b);
        let mut again = [0u8; MAX_DATAGRAM_LEN];
        let m = proto::encode_datagram(h.tick, h.flags, &got, &mut again).unwrap();
        prop_assert_eq!(&again[..m], &buf[..n]);
    }

    /// A listener that applies every datagram tracks the table exactly; quiet
    /// ticks are silent; each tick's datagrams carry exactly the changes.
    #[test]
    fn listener_tracks_any_change_sequence(
        t0 in 0u64..5000,
        steps in proptest::collection::vec(
            (1u64..40, proptest::collection::vec(change_strategy(), 0..6)), 1..40),
    ) {
        let mut s = new_state();
        let (gi_n, mag_n) = (gi_len(), magnet_len());
        let mut enc = TelemetryEncoder::new();
        let mut batch = new_batch();
        enc.force_full_state();
        let mut model = Model::default();
        model.apply(&tick(&mut enc, &mut batch, t0, &s, &[]));
        model.assert_matches(&s, "load");
        let mut t = t0;
        for (gap, changes) in steps {
            // Quiet ticks in between: silent, or a periodic full state.
            for _ in 1..gap {
                t += 1;
                let d = tick(&mut enc, &mut batch, t, &s, &[]);
                prop_assert!(d.iter().all(|(h, _)| is_full(h)), "tick {}: nothing changed", t);
                model.apply(&d);
                model.assert_matches(&s, "quiet tick");
            }
            t += 1;
            let old = model.clone();
            let mut impulses = Vec::new();
            let mut forced = false;
            for c in &changes {
                match *c {
                    Change::Switch(i, v) => s.switches[usize::from(i)] = v,
                    Change::Coil(i, v) => s.coils[usize::from(i)] = v,
                    Change::Lamp(i, v) => s.lamps[usize::from(i)] = v,
                    Change::Gi(i, v) => s.gi[usize::from(i) % gi_n] = v,
                    Change::Magnet(i, v) => s.magnets[usize::from(i) % mag_n] = v,
                    Change::Impulse(source, strength) => {
                        impulses.push(HapticsTelemetryEvent { tick: t, source, strength });
                    }
                    Change::ForceFull => {
                        enc.force_full_state();
                        forced = true;
                    }
                }
            }
            let d = tick(&mut enc, &mut batch, t, &s, &impulses);
            model.apply(&d);
            model.assert_matches(&s, "after diff");
            let want: Vec<(u16, f32)> = impulses.iter().map(|e| (e.source, e.strength)).collect();
            prop_assert_eq!(impulses_of(&d), want);
            let full = d.iter().any(|(h, _)| is_full(h));
            if forced {
                prop_assert!(full, "force_full_state must give a full state on the next tick");
            }
            if !full {
                // Incremental: exactly the elements whose value changed, once each.
                let sent: Vec<(u8, u16)> = all_entries(&d)
                    .into_iter()
                    .filter(|e| e.kind != TelemetryKind::HapticImpulse)
                    .map(|e| (e.kind as u8, e.id))
                    .collect();
                let sent_set: BTreeSet<(u8, u16)> = sent.iter().copied().collect();
                prop_assert_eq!(sent_set.len(), sent.len(), "an element is reported twice");
                let changed: BTreeSet<(u8, u16)> = model
                    .0
                    .iter()
                    .filter(|(k, v)| old.0.get(*k).copied().unwrap_or(0) != **v)
                    .map(|(k, _)| *k)
                    .collect();
                prop_assert_eq!(sent_set, changed, "tick {}: entries are not exactly the changes", t);
                prop_assert_eq!(d.len(), (sent.len() + impulses.len()).div_ceil(180));
            }
        }
    }
}
