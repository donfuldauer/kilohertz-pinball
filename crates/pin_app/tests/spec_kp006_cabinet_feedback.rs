//! KP-006 (#86) spec tests (Independent Tester): rumble, UDP telemetry, the
//! DOF bridge lifecycle (SPEC §4 3.4 and SPEC §4 3.5, `docs/TELEMETRY_PROTOCOL.md`).
//!
//! Black-box: written from the brief's pinned API only. The telemetry tests
//! compare the wire against an independent reference model of the protocol
//! (sender rules 1-6) and against a listener mirror built per the listener
//! guidance. Loopback tests drive simulated ticks and never assert on elapsed
//! time (R6): the sender thread is synchronised through the listener by
//! receiving exactly the datagrams the reference model predicts.
#![allow(clippy::needless_range_loop, clippy::field_reassign_with_default)]

use pin_app::dof::{DofBridge, DofLaunchConfig, ResolvedDofMap};
use pin_app::orchestrator::{
    TelemetryFeed, TelemetryMap, TelemetryReport, TelemetrySources, open_telemetry,
};
use pin_app::platform::rumble::{
    RUMBLE_COMMAND_MS, RUMBLE_RESEND_TICKS, RumbleCommand, RumbleDriver, RumbleOutput,
};
use pin_audio::telemetry::{
    FULL_STATE_INTERVAL_TICKS, MAX_DATAGRAMS_PER_TICK, TelemetryEntry, TelemetryHeader,
    TelemetryReceiver, decode_datagram, telemetry_channel,
};
use pin_core::haptics::{HapticEvent, HapticKind};
use pin_core::input::HapticsTelemetryEvent;
use pin_core::schema::table::TableManifest;
use pin_core::{CoilId, FrameStateSnapshot};
use pin_input::haptics::{RumbleEffect, RumbleSettings, haptics_ring};
use proptest::prelude::*;
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::net::UdpSocket;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

mod private_tmp;

// ---------------------------------------------------------------------------
// Counting allocator, armed per thread (the pattern of tests/orchestrator_alloc.rs).
// ---------------------------------------------------------------------------

struct Counting;
static ALLOCS: AtomicU64 = AtomicU64::new(0);
thread_local! { static ARMED: Cell<bool> = const { Cell::new(false) }; }

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        if ARMED.try_with(Cell::get).unwrap_or(false) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        if ARMED.try_with(Cell::get).unwrap_or(false) {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
        }
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

fn guarded<R>(f: impl FnOnce() -> R) -> (R, u64) {
    let before = ALLOCS.load(Ordering::Relaxed);
    ARMED.with(|a| a.set(true));
    let r = f();
    ARMED.with(|a| a.set(false));
    (r, ALLOCS.load(Ordering::Relaxed) - before)
}

// ---------------------------------------------------------------------------
// The scene: what the "sim" publishes, and the telemetry map over it.
// ---------------------------------------------------------------------------

/// Telemetry ids deliberately differ from sim ids (and sim ids that are not in
/// the map carry values that must never show up on the wire).
fn map() -> TelemetryMap {
    TelemetryMap {
        // (telemetry id, SwitchId)
        switches: vec![(40, 4), (255, 255), (1, 0)],
        coils: vec![(9, 2), (0, 0)],
        // (telemetry id, LampId): "lamp number 7 -> LampId 3"
        lamps: vec![(7, 3), (200, 5), (1, 6), (255, 20)],
        // (GI string, LampId)
        gi: vec![(0, 10), (31, 11), (5, 13)],
        // (telemetry id, MagnetId)
        magnets: vec![(31, 1), (0, 0)],
    }
}

const SPECIAL: [f32; 8] = [0.0, 0.25, 1.5, -0.2, f32::NAN, -0.0, f32::INFINITY, 1e-40];

/// Fills `snap`/`src` for schedule time `u` (the tick number stays separate so
/// a restarted session can show a different state).
fn scene(u: u64, snap: &mut FrameStateSnapshot, src: &mut TelemetrySources) {
    snap.lamps = [0.0; 256];
    snap.lamps[3] = ((u / 50) % 11) as f32 / 10.0;
    snap.lamps[5] = if (u / 7).is_multiple_of(2) { 0.0 } else { 1.0 };
    snap.lamps[6] = SPECIAL[((u / 13) % 8) as usize];
    snap.lamps[20] = ((u / 3) % 3) as f32 * 0.5;
    snap.lamps[10] = ((u / 100) % 5) as f32 / 4.0;
    snap.lamps[11] = SPECIAL[((u / 13 + 3) % 8) as usize];
    snap.lamps[13] = ((u / 29) % 2) as f32;
    // Not in the map: must stay invisible (LampId 7 vs telemetry id 7, etc.).
    snap.lamps[7] = 0.9;
    snap.lamps[200] = 0.7;
    snap.lamps[12] = (u % 256) as f32 / 255.0;

    src.switch_bits = [0; 4];
    let mut set = |i: usize, on: bool| {
        if on {
            src.switch_bits[i / 64] |= 1u64 << (i % 64);
        }
    };
    set(4, (u / 9) % 2 == 1);
    set(255, (u / 31) % 2 == 1);
    set(0, (u / 5) % 2 == 1);
    set(40, true); // unmapped sim id (telemetry id 40 is sim switch 4)
    src.coils = [false; 256];
    src.coils[2] = (u / 11) % 2 == 1;
    src.coils[0] = (u / 23) % 2 == 1;
    src.coils[9] = true; // unmapped
    src.coils[200] = u.is_multiple_of(2); // unmapped
    src.magnets = [0.0; 32];
    src.magnets[1] = [0.0, 0.5, 1.5, f32::NAN][((u / 17) % 4) as usize];
    src.magnets[0] = ((u / 40) % 2) as f32;
    src.magnets[31] = 0.8; // unmapped
}

// ---------------------------------------------------------------------------
// Reference model of the protocol (sender rules 1-6) and the listener mirror.
// ---------------------------------------------------------------------------

fn norm(v: f32) -> f32 {
    if v.is_nan() {
        0.0
    } else {
        v.clamp(0.0, 1.0) + 0.0
    }
}

/// (kind, id, value bits)
type Entry = (u8, u16, u32);

#[derive(Clone, PartialEq)]
struct Norm {
    kinds: [[u32; 256]; 5],
}

impl Norm {
    fn zero() -> Norm {
        Norm {
            kinds: [[0; 256]; 5],
        }
    }
}

fn expected_state(m: &TelemetryMap, snap: &FrameStateSnapshot, src: &TelemetrySources) -> Norm {
    let mut n = Norm::zero();
    let b = |on: bool| if on { 1.0f32.to_bits() } else { 0 };
    for &(tid, sid) in &m.switches {
        let s = sid as usize;
        n.kinds[0][tid as usize] = b(src.switch_bits[s / 64] >> (s % 64) & 1 == 1);
    }
    for &(tid, cid) in &m.coils {
        n.kinds[1][tid as usize] = b(src.coils[cid as usize]);
    }
    for &(tid, lid) in &m.lamps {
        n.kinds[2][tid as usize] = norm(snap.lamps[lid as usize]).to_bits();
    }
    for &(g, lid) in &m.gi {
        n.kinds[3][g as usize] = norm(snap.lamps[lid as usize]).to_bits();
    }
    for &(tid, mid) in &m.magnets {
        n.kinds[4][tid as usize] = norm(src.magnets[mid as usize]).to_bits();
    }
    n
}

const COUNTS: [usize; 5] = [256, 256, 256, 32, 32];

/// Independent model of the encoder: what datagrams a tick must produce.
#[derive(Default)]
struct RefEnc {
    prev: Option<Norm>,
    last_full: Option<u64>,
    last_tick: Option<u64>,
}

struct RefDatagram {
    full: bool,
    entries: Vec<Entry>,
}

impl RefEnc {
    fn tick(&mut self, t: u64, cur: &Norm, impulses: &[(u16, f32)]) -> Vec<RefDatagram> {
        let full = self.prev.is_none()
            || self.last_tick.is_some_and(|l| t < l)
            || self
                .last_full
                .is_none_or(|f| t >= f + FULL_STATE_INTERVAL_TICKS);
        let mut entries = Vec::new();
        for k in 0..5 {
            for id in 0..COUNTS[k] {
                let changed = self
                    .prev
                    .as_ref()
                    .is_none_or(|p| p.kinds[k][id] != cur.kinds[k][id]);
                if full || changed {
                    entries.push((k as u8, id as u16, cur.kinds[k][id]));
                }
            }
        }
        for &(src, s) in impulses.iter().take(64) {
            entries.push((5, src, norm(s).to_bits()));
        }
        self.prev = Some(cur.clone());
        self.last_tick = Some(t);
        if full {
            self.last_full = Some(t);
        }
        if entries.is_empty() {
            return Vec::new();
        }
        entries
            .chunks(180)
            .map(|c| RefDatagram {
                full,
                entries: c.to_vec(),
            })
            .collect()
    }
}

/// The listener guidance of the protocol, as a mirror of all state elements.
struct Mirror {
    known: bool,
    last_full: u64,
    /// (tick the element was last set from, value bits)
    vals: [[(u64, u32); 256]; 5],
}

impl Mirror {
    fn new() -> Mirror {
        Mirror {
            known: false,
            last_full: 0,
            vals: [[(0, 0); 256]; 5],
        }
    }

    fn apply(&mut self, h: &TelemetryHeader, es: &[TelemetryEntry]) {
        if h.is_full_state() {
            if self.known && h.tick < self.last_full {
                self.vals = [[(0, 0); 256]; 5];
            }
            self.known = true;
            self.last_full = h.tick;
            for e in es {
                let k = e.kind as u8 as usize;
                if k < 5 {
                    self.vals[k][e.id as usize] = (h.tick, e.value.to_bits());
                }
            }
        } else {
            if !self.known || h.tick < self.last_full {
                return;
            }
            for e in es {
                let k = e.kind as u8 as usize;
                if k < 5 && h.tick >= self.vals[k][e.id as usize].0 {
                    self.vals[k][e.id as usize] = (h.tick, e.value.to_bits());
                }
            }
        }
    }

    fn apply_bytes(&mut self, bytes: &[u8]) {
        let (h, e) = decode_datagram(bytes).expect("decodes");
        self.apply(&h, &e);
    }

    fn matches(&self, n: &Norm) -> bool {
        self.known && (0..5).all(|k| (0..COUNTS[k]).all(|i| self.vals[k][i].1 == n.kinds[k][i]))
    }

    fn first_diff(&self, n: &Norm) -> Option<(usize, usize, u32, u32)> {
        for k in 0..5 {
            for i in 0..COUNTS[k] {
                if self.vals[k][i].1 != n.kinds[k][i] {
                    return Some((k, i, self.vals[k][i].1, n.kinds[k][i]));
                }
            }
        }
        None
    }
}

/// Parses the wire format by hand and checks it is well formed; returns the
/// header fields and entries, after asserting `decode_datagram` agrees.
fn check_wire(bytes: &[u8]) -> (u64, bool, Vec<Entry>) {
    assert!(
        bytes.len() >= 16 && bytes.len() <= 1456,
        "len {}",
        bytes.len()
    );
    assert_eq!(&bytes[0..4], b"PNTL");
    assert_eq!(bytes[4], 1, "version");
    let flags = bytes[5];
    assert_eq!(flags & !1, 0, "only flag bit 0 is defined");
    let count = u16::from_le_bytes([bytes[6], bytes[7]]) as usize;
    assert!(count <= 180 && count > 0, "count {count}");
    assert_eq!(bytes.len(), 16 + 8 * count, "length = 16 + 8 * count");
    let tick = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
    let mut es = Vec::new();
    for i in 0..count {
        let e = &bytes[16 + 8 * i..24 + 8 * i];
        assert!(e[0] <= 5, "kind {}", e[0]);
        assert_eq!(e[1], 0, "reserved byte");
        es.push((
            e[0],
            u16::from_le_bytes([e[2], e[3]]),
            u32::from_le_bytes([e[4], e[5], e[6], e[7]]),
        ));
    }
    let (h, dec) = decode_datagram(bytes).expect("decode_datagram accepts it");
    assert_eq!(
        (h.tick, h.is_full_state(), h.entry_count as usize),
        (tick, flags & 1 == 1, count)
    );
    let dec: Vec<Entry> = dec
        .iter()
        .map(|e| (e.kind as u8, e.id, e.value.to_bits()))
        .collect();
    assert_eq!(dec, es);
    (tick, flags & 1 == 1, es)
}

fn drain(rx: &mut TelemetryReceiver) -> Vec<Vec<u8>> {
    let mut v = Vec::new();
    while let Some(d) = rx.pop() {
        v.push(d.as_bytes().to_vec());
    }
    v
}

fn is_sorted_by_kind_then_id(es: &[Entry]) -> bool {
    let states: Vec<&Entry> = es.iter().filter(|e| e.0 < 5).collect();
    states
        .windows(2)
        .all(|w| (w[0].0, w[0].1) < (w[1].0, w[1].1))
}

// ---------------------------------------------------------------------------
// 1. Loopback stream (acceptance 1) and 5. open_telemetry / report.
// ---------------------------------------------------------------------------

/// Runs ticks `0..=last` through `open_telemetry` to a loopback listener.
/// `quiet_after`: the scene stops changing after that tick.
fn run_loopback(last: u64, quiet_after: u64) -> (TelemetryReport, Vec<u64>, usize) {
    let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
    listener
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, link) = match open_telemetry(&addr.to_string()) {
        Ok(x) => x,
        Err(e) => panic!("open_telemetry({addr}) failed: {e}"),
    };
    assert_eq!(link.destination(), addr);
    let m = map();
    let mut feed = TelemetryFeed::new(tx, m.clone());
    let mut snap = Box::<FrameStateSnapshot>::default();
    let mut src = TelemetrySources::default();
    let mut model = RefEnc::default();
    let mut mirror = Mirror::new();
    let mut full_ticks = Vec::new();
    let mut silent_ticks = 0usize;
    let mut buf = [0u8; 2048];
    for t in 0..=last {
        scene(t.min(quiet_after), &mut snap, &mut src);
        snap.tick = t;
        feed.tick(t, &snap, &src, &[]);
        let cur = expected_state(&m, &snap, &src);
        let want = model.tick(t, &cur, &[]);
        if want.is_empty() {
            silent_ticks += 1;
        }
        if want.first().is_some_and(|d| d.full) {
            full_ticks.push(t);
            assert_eq!(want.len(), 5, "a full state takes 5 datagrams");
        }
        for (i, w) in want.iter().enumerate() {
            let n = listener.recv(&mut buf).unwrap_or_else(|e| {
                panic!("tick {t}: datagram {i} of {} missing: {e}", want.len())
            });
            let (tick, full, es) = check_wire(&buf[..n]);
            assert_eq!(
                tick, t,
                "datagram of another tick (a no-change tick sent something?)"
            );
            assert_eq!(full, w.full, "tick {t} full flag");
            assert_eq!(es, w.entries, "tick {t} datagram {i} entries");
            if !full {
                assert!(is_sorted_by_kind_then_id(&es), "tick {t} order");
            }
            mirror.apply_bytes(&buf[..n]);
        }
        // Checkpoint: the mirror equals the snapshot (through the id mapping).
        assert!(
            mirror.matches(&cur),
            "tick {t}: mirror differs {:?}",
            mirror.first_diff(&cur)
        );
    }
    let report = link.stop();
    // Nothing more arrives after the final flush.
    listener
        .set_read_timeout(Some(Duration::from_millis(200)))
        .unwrap();
    assert!(
        listener.recv(&mut buf).is_err(),
        "unexpected extra datagram"
    );
    assert_eq!(feed.forced_full_states(), 0);
    assert_eq!(feed.ring_dropped(), 0);
    assert!(mirror.matches(&expected_state(&m, &snap, &src)));
    (report, full_ticks, silent_ticks)
}

/// SPEC §4 3.4 / KP-006 acceptance 1: full state, then deltas, over 1,000 ticks.
#[test]
fn kp006_loopback_1000_ticks_full_state_then_deltas() {
    let (report, full_ticks, silent) = run_loopback(999, u64::MAX);
    assert_eq!(full_ticks, [0], "a full state on the first tick only");
    assert!(silent > 50, "the scene has {silent} no-change ticks");
    assert_eq!(report.send_errors, 0);
    assert_eq!(report.ring_dropped, 0);
    assert!(report.sent > 5, "{report:?}");
}

/// SPEC §4 3.4: a tick >= 1000 after the previous full state is a full state,
/// even when nothing changed.
#[test]
fn kp006_loopback_full_state_every_1000_ticks_even_if_unchanged() {
    let (report, full_ticks, silent) = run_loopback(1001, 5);
    assert_eq!(full_ticks, [0, 1000]);
    assert!(silent > 900);
    assert_eq!(report.send_errors, 0);
    assert_eq!(report.ring_dropped, 0);
}

/// KP-006 item 5: `TelemetryReport.sent` equals the datagrams the listener got.
#[test]
fn kp006_report_sent_equals_listener_count() {
    let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
    listener
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, link) = open_telemetry(&addr.to_string()).expect("opens");
    let mut feed = TelemetryFeed::new(tx, map());
    let mut snap = Box::<FrameStateSnapshot>::default();
    let mut src = TelemetrySources::default();
    let mut model = RefEnc::default();
    let m = map();
    let mut buf = [0u8; 2048];
    let mut received = 0u64;
    for t in 0..60u64 {
        scene(t, &mut snap, &mut src);
        feed.tick(t, &snap, &src, &[]);
        let want = model.tick(t, &expected_state(&m, &snap, &src), &[]).len();
        for _ in 0..want {
            listener.recv(&mut buf).expect("datagram");
            received += 1;
        }
    }
    let report = link.stop();
    assert_eq!(report.sent, received);
    assert_eq!((report.send_errors, report.ring_dropped), (0, 0));
    assert!(received >= 5);
}

/// KP-006 item 5: bad endpoints are errors with a message; a good one parses.
#[test]
fn kp006_open_telemetry_endpoints() {
    for bad in [
        "",
        "not-an-endpoint",
        "127.0.0.1:notaport",
        "127.0.0.1:",
        "127.0.0.1:99999",
    ] {
        let e = open_telemetry(bad)
            .err()
            .unwrap_or_else(|| panic!("{bad:?} accepted"));
        assert!(!e.is_empty(), "{bad:?}: empty message");
    }
    let l = UdpSocket::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let (tx, link) = open_telemetry(&format!("127.0.0.1:{}", addr.port())).expect("ok");
    assert_eq!(link.destination(), addr);
    drop(tx);
    let r = link.stop();
    assert_eq!(r, TelemetryReport::default());
}

// ---------------------------------------------------------------------------
// 2. Impulses.
// ---------------------------------------------------------------------------

fn manual_feed(cap: usize) -> (TelemetryFeed, TelemetryReceiver) {
    let (tx, rx) = telemetry_channel(cap);
    (TelemetryFeed::new(tx, map()), rx)
}

fn impulses(n: usize) -> Vec<HapticsTelemetryEvent> {
    (0..n)
        .map(|i| HapticsTelemetryEvent {
            tick: 0,
            source: 100 + i as u16,
            strength: if i == 3 { 2.0 } else { i as f32 / 70.0 },
        })
        .collect()
}

fn want_impulses(imp: &[HapticsTelemetryEvent]) -> Vec<(u16, f32)> {
    imp.iter().map(|e| (e.source, e.strength)).collect()
}

/// SPEC §4 3.4 sender rule 2: impulses follow the states, at most 64 per tick.
#[test]
fn kp006_impulses_after_states_capped_at_64() {
    let (mut feed, mut rx) = manual_feed(64);
    let m = map();
    let mut snap = Box::<FrameStateSnapshot>::default();
    let mut src = TelemetrySources::default();
    let mut model = RefEnc::default();
    let mut step = |feed: &mut TelemetryFeed,
                    rx: &mut TelemetryReceiver,
                    t: u64,
                    u: u64,
                    imp: &[HapticsTelemetryEvent]|
     -> Vec<Vec<u8>> {
        scene(u, &mut snap, &mut src);
        feed.tick(t, &snap, &src, imp);
        let want = model.tick(t, &expected_state(&m, &snap, &src), &want_impulses(imp));
        let got = drain(rx);
        assert_eq!(got.len(), want.len(), "tick {t} datagram count");
        for (g, w) in got.iter().zip(&want) {
            let (tick, full, es) = check_wire(g);
            assert_eq!((tick, full), (t, w.full));
            assert_eq!(es, w.entries, "tick {t}");
        }
        got
    };
    assert_eq!(step(&mut feed, &mut rx, 0, 0, &[]).len(), 5);
    // 70 impulses and state changes in the same tick: the states, then 64 impulses.
    let imp70 = impulses(70);
    let got = step(&mut feed, &mut rx, 1, 50, &imp70);
    assert_eq!(got.len(), 1);
    let (_, full, es) = check_wire(&got[0]);
    assert!(!full);
    assert!(es.len() > 64, "state entries are present");
    assert_eq!(es.iter().filter(|e| e.0 == 5).count(), 64);
    assert!(es[..es.len() - 64].iter().all(|e| e.0 < 5), "states first");
    assert!(
        es[es.len() - 64..].iter().all(|e| e.0 == 5),
        "impulses last"
    );
    for (i, e) in es[es.len() - 64..].iter().enumerate() {
        assert_eq!(e.1, 100 + i as u16, "source id / order");
        let want = if i == 3 { 1.0f32 } else { i as f32 / 70.0 };
        assert_eq!(f32::from_bits(e.2), want);
    }
    // Impulses alone make a datagram on a tick without state changes.
    let got = step(&mut feed, &mut rx, 2, 50, &impulses(3));
    assert_eq!(got.len(), 1);
    let (_, full, es) = check_wire(&got[0]);
    assert!(!full && es.len() == 3 && es.iter().all(|e| e.0 == 5));
    // No change and no impulses: nothing.
    assert!(step(&mut feed, &mut rx, 3, 50, &[]).is_empty());
}

/// SPEC §4 3.4: a full state is followed by that tick's impulses, flag on all.
#[test]
fn kp006_impulses_on_a_full_state_tick() {
    for n in [4usize, 70] {
        let (mut feed, mut rx) = manual_feed(64);
        let mut snap = Box::<FrameStateSnapshot>::default();
        let mut src = TelemetrySources::default();
        scene(0, &mut snap, &mut src);
        feed.tick(0, &snap, &src, &impulses(n));
        let got = drain(&mut rx);
        let parsed: Vec<_> = got.iter().map(|g| check_wire(g)).collect();
        assert_eq!(parsed.len(), (832 + n.min(64)).div_ceil(180));
        assert!(
            parsed.iter().all(|p| p.1 && p.0 == 0),
            "flag bit 0 on every datagram"
        );
        let all: Vec<Entry> = parsed.into_iter().flat_map(|p| p.2).collect();
        assert_eq!(all.len(), 832 + n.min(64));
        assert!(all[..832].iter().all(|e| e.0 < 5));
        assert!(all[832..].iter().all(|e| e.0 == 5));
        assert!(is_sorted_by_kind_then_id(&all));
    }
}

// ---------------------------------------------------------------------------
// 3. Back-pressure (acceptance 2).
// ---------------------------------------------------------------------------

/// SPEC §4 3.4 sender rule 6: a full ring forces a full state and the
/// listener converges, including to elements that changed while dropping.
#[test]
fn kp006_back_pressure_forces_full_state_and_listener_converges() {
    let (mut feed, mut rx) = manual_feed(MAX_DATAGRAMS_PER_TICK);
    let m = map();
    let mut snap = Box::<FrameStateSnapshot>::default();
    let mut src = TelemetrySources::default();
    let mut mirror = Mirror::new();
    let mut t = 0u64;
    // Changing state every tick (u = 7 * t crosses every schedule boundary).
    while !(feed.forced_full_states() > 0 && feed.ring_dropped() > 0) {
        assert!(t < 20, "the ring never filled up");
        scene(7 * t, &mut snap, &mut src);
        feed.tick(t, &snap, &src, &[]);
        t += 1;
    }
    for _ in 0..5 {
        scene(7 * t, &mut snap, &mut src);
        feed.tick(t, &snap, &src, &[]);
        t += 1;
    }
    assert!(feed.forced_full_states() > 0 && feed.ring_dropped() > 0);
    // The listener reads what made it through.
    for d in drain(&mut rx) {
        mirror.apply_bytes(&d);
    }
    let current = expected_state(&m, &snap, &src);
    assert!(
        !mirror.matches(&current),
        "dropped datagrams must have lost information for this test to mean anything"
    );
    // One more tick, with the state unchanged: still a full state.
    feed.tick(t, &snap, &src, &[]);
    let got = drain(&mut rx);
    assert_eq!(got.len(), 5, "a full state takes 5 datagrams");
    for g in &got {
        let (tick, full, _) = check_wire(g);
        assert!(full && tick == t, "recovery datagram is not a full state");
        mirror.apply_bytes(g);
    }
    assert!(
        mirror.matches(&current),
        "diff {:?}",
        mirror.first_diff(&current)
    );
    t += 1;
    // The full state is sent once: an unchanged tick is silent, a changed one a delta.
    feed.tick(t, &snap, &src, &[]);
    assert!(drain(&mut rx).is_empty());
    t += 1;
    scene(7 * t + 500, &mut snap, &mut src);
    feed.tick(t, &snap, &src, &[]);
    let got = drain(&mut rx);
    assert!(!got.is_empty());
    for g in &got {
        let (tick, full, _) = check_wire(g);
        assert!(!full && tick == t);
        mirror.apply_bytes(g);
    }
    assert!(mirror.matches(&expected_state(&m, &snap, &src)));
}

/// KP-006 item 3 (sender thread variant): a stalled-then-started sender thread
/// delivers a listener that converges.
#[test]
fn kp006_back_pressure_listener_converges_over_udp() {
    use pin_audio::telemetry::TelemetrySender;
    let listener = UdpSocket::bind("127.0.0.1:0").unwrap();
    listener
        .set_read_timeout(Some(Duration::from_millis(500)))
        .unwrap();
    let (tx, rx) = telemetry_channel(MAX_DATAGRAMS_PER_TICK);
    let m = map();
    let mut feed = TelemetryFeed::new(tx, m.clone());
    let mut snap = Box::<FrameStateSnapshot>::default();
    let mut src = TelemetrySources::default();
    for t in 0..12u64 {
        scene(7 * t, &mut snap, &mut src);
        feed.tick(t, &snap, &src, &[]);
    }
    assert!(feed.forced_full_states() > 0 && feed.ring_dropped() > 0);
    let mut sender = TelemetrySender::new(rx, listener.local_addr().unwrap()).unwrap();
    sender.pump();
    scene(7 * 12, &mut snap, &mut src);
    feed.tick(12, &snap, &src, &[]);
    sender.pump();
    let mut mirror = Mirror::new();
    let mut buf = [0u8; 2048];
    while let Ok(n) = listener.recv(&mut buf) {
        mirror.apply_bytes(&buf[..n]);
    }
    assert!(mirror.matches(&expected_state(&m, &snap, &src)));
}

// ---------------------------------------------------------------------------
// 4. Session restart.
// ---------------------------------------------------------------------------

/// SPEC §4 3.4 sender rule 4: a tick that goes backwards is a full state and
/// the mirror converges to the new session.
#[test]
fn kp006_session_restart_sends_full_state() {
    // (ticks before the restart, restart tick)
    for (before, restart) in [(500u64, 0u64), (1500, 0), (500, 100), (1500, 1)] {
        let (mut feed, mut rx) = manual_feed(64);
        let m = map();
        let mut snap = Box::<FrameStateSnapshot>::default();
        let mut src = TelemetrySources::default();
        let mut model = RefEnc::default();
        let mut mirror = Mirror::new();
        let mut run = |feed: &mut TelemetryFeed,
                       rx: &mut TelemetryReceiver,
                       mirror: &mut Mirror,
                       t: u64,
                       u: u64|
         -> (usize, bool) {
            scene(u, &mut snap, &mut src);
            feed.tick(t, &snap, &src, &[]);
            let cur = expected_state(&m, &snap, &src);
            let want = model.tick(t, &cur, &[]);
            let got = drain(rx);
            assert_eq!(got.len(), want.len(), "tick {t} (u {u})");
            let mut full = false;
            for (g, w) in got.iter().zip(&want) {
                let (_, f, es) = check_wire(g);
                assert_eq!((f, &es), (w.full, &w.entries));
                full |= f;
                mirror.apply_bytes(g);
            }
            assert!(mirror.matches(&cur), "t {t}: {:?}", mirror.first_diff(&cur));
            (got.len(), full)
        };
        for t in 0..=before {
            run(&mut feed, &mut rx, &mut mirror, t, t);
        }
        // The new session: a different state on a lower tick.
        let (n, full) = run(&mut feed, &mut rx, &mut mirror, restart, 777 + restart);
        assert!(
            full && n == 5,
            "restart {before}->{restart}: full={full} n={n}"
        );
        for t in restart + 1..restart + 30 {
            run(&mut feed, &mut rx, &mut mirror, t, 777 + t);
        }
    }
}

// ---------------------------------------------------------------------------
// 6. Allocation guard (R4).
// ---------------------------------------------------------------------------

/// R4: `TelemetryFeed::tick` never allocates, including on full-state ticks,
/// failed pushes, restarts and capped impulses.
#[test]
fn kp006_feed_tick_is_allocation_free() {
    let (_, n) = guarded(|| Box::new(1u8));
    assert!(n >= 1, "allocator guard not armed");

    let (mut feed, mut rx) = manual_feed(MAX_DATAGRAMS_PER_TICK);
    let mut snap = Box::<FrameStateSnapshot>::default();
    let mut src = TelemetrySources::default();
    let imp70 = impulses(70);
    let mut total = 0;
    let mut t = 0u64;
    for chunk in 0..60u64 {
        let (_, allocs) = guarded(|| {
            for i in 0..100u64 {
                // Every 7th chunk restarts the session (tick goes backwards).
                let tick = if chunk % 7 == 6 { i } else { t + i };
                scene(chunk * 100 + i, &mut snap, &mut src);
                snap.tick = tick;
                // Every 3rd chunk never drains: failed pushes.
                feed.tick(tick, &snap, &src, if i % 10 == 0 { &imp70 } else { &[] });
                if chunk % 3 != 0 {
                    while rx.pop().is_some() {}
                }
            }
        });
        total += allocs;
        t += 100;
        while rx.pop().is_some() {}
    }
    assert_eq!(total, 0, "TelemetryFeed::tick allocated {total} times");
    assert!(
        feed.forced_full_states() > 0,
        "failed-push path not exercised"
    );
    assert!(feed.ring_dropped() > 0);
}

// ---------------------------------------------------------------------------
// Property tests: normalisation on the wire.
// ---------------------------------------------------------------------------

fn any_f32() -> impl Strategy<Value = f32> {
    prop_oneof![
        3 => any::<u32>().prop_map(f32::from_bits),
        2 => proptest::sample::select(&SPECIAL[..]),
        2 => -2.0f32..3.0,
    ]
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 128, failure_persistence: None, ..ProptestConfig::default() })]

    /// SPEC §4 3.4 value normalisation: levels in [0,1], never NaN or -0.0;
    /// switches and coils exactly 0.0 / 1.0.
    #[test]
    fn kp006_wire_values_are_normalised(
        a in proptest::collection::vec(any_f32(), 8),
        b in proptest::collection::vec(any_f32(), 8),
        sw in proptest::collection::vec(any::<bool>(), 6),
    ) {
        let (tx, mut rx) = telemetry_channel(64);
        let m = TelemetryMap {
            switches: vec![(10, 0), (11, 1), (12, 2)],
            coils: vec![(10, 0), (11, 1), (12, 2)],
            lamps: vec![(10, 0), (11, 1), (12, 2)],
            gi: vec![(0, 3), (1, 4), (31, 5)],
            magnets: vec![(0, 0), (31, 1)],
        };
        let mut feed = TelemetryFeed::new(tx, m.clone());
        let mut snap = Box::<FrameStateSnapshot>::default();
        let mut src = TelemetrySources::default();
        let mut mirror = Mirror::new();
        for (t, vals) in [(0u64, &a), (1, &b)] {
            snap.lamps[..6].copy_from_slice(&vals[..6]);
            src.magnets[0] = vals[6];
            src.magnets[1] = vals[7];
            src.switch_bits[0] = sw[..3].iter().enumerate().fold(0, |acc, (i, &x)| acc | (u64::from(x) << i));
            src.coils[..3].copy_from_slice(&sw[3..6]);
            if t == 1 {
                src.switch_bits[0] = !src.switch_bits[0] & 7;
                for c in &mut src.coils[..3] { *c = !*c; }
            }
            feed.tick(t, &snap, &src, &[]);
            for g in drain(&mut rx) {
                let (_, _, es) = check_wire(&g);
                for (k, id, bits) in es {
                    let v = f32::from_bits(bits);
                    prop_assert!(!v.is_nan(), "NaN on the wire (kind {k} id {id})");
                    if k <= 1 {
                        prop_assert!(bits == 0 || bits == 1.0f32.to_bits(), "kind {k} = {v}");
                    } else {
                        prop_assert!((0.0..=1.0).contains(&v), "kind {k} id {id} = {v}");
                        prop_assert!(bits != (-0.0f32).to_bits(), "-0.0 on the wire");
                    }
                }
                mirror.apply_bytes(&g);
            }
            let want = expected_state(&m, &snap, &src);
            prop_assert!(mirror.matches(&want), "{:?}", mirror.first_diff(&want));
        }
    }
}

// ---------------------------------------------------------------------------
// 7. Rumble (acceptance 3).
// ---------------------------------------------------------------------------

#[derive(Default)]
struct Mock {
    cmds: Vec<RumbleCommand>,
}

impl RumbleOutput for Mock {
    fn set_gamepad_rumble(&mut self, cmd: RumbleCommand) -> usize {
        self.cmds.push(cmd);
        1
    }
}

fn off(c: &RumbleCommand) -> bool {
    c.low == 0 && c.high == 0
}

fn coil(tick: u64, strength: f32) -> HapticEvent {
    HapticEvent::new(tick, HapticKind::CoilFire { coil: CoilId(0) }, strength)
}

fn long_settings() -> RumbleSettings {
    let mut s = RumbleSettings::default();
    s.coil_fire = RumbleEffect::new(1.0, 1.0, 5000);
    s
}

/// SPEC §4 3.4 (REQ rumble): a coil fire produces a non-zero command via `service`.
#[test]
fn kp006_rumble_service_emits_a_command() {
    let (mut p, c) = haptics_ring::<HapticEvent>(64);
    let mut d = RumbleDriver::new(RumbleSettings::default(), c);
    let mut out = Mock::default();
    for t in 0..100 {
        assert_eq!(d.service(t, &mut out), None);
    }
    assert!(out.cmds.is_empty());
    assert!(p.push(coil(100, 1.0)));
    let r = d
        .service(100, &mut out)
        .expect("a command at the event tick");
    assert!(!off(&r));
    assert_eq!(r.duration_ms, RUMBLE_COMMAND_MS);
    assert_eq!(out.cmds, [r], "the command reaches the output exactly once");
    // It decays over the following ticks.
    let mut last = r;
    for t in 101..400 {
        if let Some(c) = d.service(t, &mut out) {
            last = c;
        }
    }
    assert!(off(&last), "the envelope ends with OFF: {last:?}");
}

/// KP-006 acceptance 3: a session started within 1 s of the previous one has
/// no leftover envelope.
#[test]
fn kp006_rumble_new_session_has_no_leftover_envelope() {
    for restart in [0u64, 20, 149] {
        let (mut p, c) = haptics_ring::<HapticEvent>(64);
        let mut d = RumbleDriver::new(long_settings(), c);
        let mut out = Mock::default();
        for t in 0..100 {
            d.service(t, &mut out);
        }
        assert!(p.push(coil(100, 1.0)));
        for t in 100..=150 {
            d.service(t, &mut out);
        }
        assert!(
            !off(out.cmds.last().expect("motors on")),
            "motors must be on before the restart"
        );
        let n = out.cmds.len();
        // The tick goes backwards: a new session, with no new events.
        let r = d.service(restart, &mut out);
        assert!(r.as_ref().is_some_and(off), "restart at {restart}: {r:?}");
        assert_eq!(out.cmds.len(), n + 1, "OFF exactly once");
        for t in restart + 1..=restart + 1500 {
            assert_eq!(
                d.service(t, &mut out),
                None,
                "restart {restart}: command at tick {t}"
            );
        }
        assert_eq!(
            out.cmds.len(),
            n + 1,
            "no non-zero command after the restart"
        );
        // (d) the driver is not dead: a new event works.
        assert!(p.push(coil(restart + 1501, 1.0)));
        let c = d.service(restart + 1501, &mut out).expect("new event");
        assert!(!off(&c));
    }
}

/// KP-006: a new session's own first event works right after the restart.
#[test]
fn kp006_rumble_driver_alive_after_restart() {
    let (mut p, c) = haptics_ring::<HapticEvent>(64);
    let mut d = RumbleDriver::new(long_settings(), c);
    let mut out = Mock::default();
    assert!(p.push(coil(100, 1.0)));
    for t in 100..=150 {
        d.service(t, &mut out);
    }
    for t in 0..3 {
        d.service(t, &mut out);
    }
    assert!(off(out.cmds.last().unwrap()));
    assert!(p.push(coil(3, 0.5)));
    let c = d
        .service(3, &mut out)
        .expect("a command for the new session");
    assert!(!off(&c));
    assert_eq!(out.cmds.last(), Some(&c));
}

/// KP-006: `reset()` returns OFF only when motors were on, and discards queued events.
#[test]
fn kp006_rumble_reset() {
    let (mut p, c) = haptics_ring::<HapticEvent>(64);
    let mut d = RumbleDriver::new(long_settings(), c);
    let mut out = Mock::default();
    // Fresh: motors off, nothing queued.
    assert_eq!(d.reset(), None);
    // A stale event queued before reset() never fires.
    assert!(p.push(coil(10, 1.0)));
    assert_eq!(d.reset(), None, "motors were never on");
    for t in 1..=300 {
        assert_eq!(d.service(t, &mut out), None, "stale event fired at {t}");
    }
    assert!(out.cmds.is_empty());
    // Motors on: reset() answers OFF, once.
    assert!(p.push(coil(400, 1.0)));
    let on = d.service(400, &mut out).expect("on");
    assert!(!off(&on));
    assert!(p.push(coil(420, 1.0)), "queued, not yet serviced");
    let r = d.reset().expect("motors were on");
    assert!(off(&r));
    assert_eq!(d.reset(), None, "already off");
    for t in 1..=500 {
        assert_eq!(d.service(t, &mut out), None, "leftover at {t}");
    }
    // The driver still works afterwards.
    assert!(p.push(coil(501, 1.0)));
    assert!(!off(&d.service(501, &mut out).expect("alive")));
}

/// SPEC §4 3.4: `poll()` is unchanged for monotonic ticks: a command on
/// change, a resend every `RUMBLE_RESEND_TICKS` while non-zero, OFF at the end.
#[test]
fn kp006_rumble_poll_semantics() {
    let mut s = RumbleSettings::default();
    s.coil_fire = RumbleEffect::new(0.5, 0.5, 4_000_000_000); // effectively constant level
    let (mut p, c) = haptics_ring::<HapticEvent>(64);
    let mut d = RumbleDriver::new(s, c);
    assert_eq!(d.poll(9), None);
    assert!(p.push(coil(10, 0.5)));
    let first = d.poll(10).expect("command on change");
    assert!(!off(&first));
    assert_eq!(first.duration_ms, RUMBLE_COMMAND_MS);
    for t in 11..10 + RUMBLE_RESEND_TICKS {
        assert_eq!(d.poll(t), None, "tick {t}: nothing changed");
    }
    assert_eq!(d.poll(10 + RUMBLE_RESEND_TICKS), Some(first), "resend");
    for t in 10 + RUMBLE_RESEND_TICKS + 1..10 + 2 * RUMBLE_RESEND_TICKS {
        assert_eq!(d.poll(t), None);
    }
    assert_eq!(
        d.poll(10 + 2 * RUMBLE_RESEND_TICKS),
        Some(first),
        "resend again"
    );

    // A short effect decays to exactly one OFF.
    let mut s = RumbleSettings::default();
    s.coil_fire = RumbleEffect::new(1.0, 1.0, 20);
    let (mut p, c) = haptics_ring::<HapticEvent>(64);
    let mut d = RumbleDriver::new(s, c);
    assert!(p.push(coil(10, 1.0)));
    let cmds: Vec<RumbleCommand> = (9..300).filter_map(|t| d.poll(t)).collect();
    assert!(cmds.len() >= 2);
    let (last, rest) = cmds.split_last().unwrap();
    assert!(off(last), "{last:?}");
    assert!(rest.iter().all(|c| !off(c)), "{rest:?}");
}

// ---------------------------------------------------------------------------
// 8. DOF lifecycle (acceptance 4).
// ---------------------------------------------------------------------------

const TABLE_TOML: &str = include_str!("../../../tools/dof_bridge/tests/fixtures/table.toml");

#[cfg(unix)]
mod dof {
    use super::*;
    use std::path::{Path, PathBuf};
    use std::sync::Mutex;
    use std::time::Instant;

    /// Serialises the process tests (write-then-exec of scripts races with
    /// forks in sibling threads: ETXTBSY).
    static SERIAL: Mutex<()> = Mutex::new(());

    fn table() -> TableManifest {
        TableManifest::from_toml_str(TABLE_TOML).unwrap()
    }

    /// A stand-in bridge: leaves a marker, then runs until stdin closes
    /// (what `--exit-on-stdin-eof` asks for) and exits 0.
    fn stand_in(dir: &Path) -> PathBuf {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let p = dir.join("stand-in-bridge.sh");
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o700)
            .open(&p)
            .unwrap();
        f.write_all(b"#!/bin/sh\n: > \"$(dirname \"$0\")/ran.marker\"\ncat >/dev/null\nexit 0\n")
            .unwrap();
        f.sync_all().unwrap();
        drop(f);
        p
    }

    fn config(script: &Path, dir: &Path) -> DofLaunchConfig {
        let mut c = DofLaunchConfig::new(script, 35120);
        c.map_dir = dir.to_path_buf();
        c
    }

    fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !f() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn process_gone(pid: u32) -> bool {
        !Path::new(&format!("/proc/{pid}")).exists()
    }

    /// KP-006 acceptance 4 (a), (b): started on load, stopped on exit.
    #[test]
    fn kp006_dof_started_on_load_and_stopped_on_exit() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = private_tmp::create("kp006-dof-a");
        let script = stand_in(&dir);
        let cfg = config(&script, &dir);
        let mut bridge = DofBridge::start_for_table_on(true, &table(), &cfg)
            .expect("starts")
            .expect("a bridge for a table with [dof]");
        assert!(bridge.try_wait().unwrap().is_none(), "alive after load");
        let map_file = bridge.map_file().to_path_buf();
        assert!(map_file.starts_with(&dir));
        assert!(map_file.exists(), "map file written");
        let want = ResolvedDofMap::resolve(&table())
            .unwrap()
            .unwrap()
            .to_json();
        assert_eq!(std::fs::read_to_string(&map_file).unwrap(), want);
        let pid = bridge.id();
        assert!(pid > 0);
        let status = bridge.stop().wait().expect("exit status");
        assert!(status.success(), "{status:?}");
        assert!(!map_file.exists(), "map file removed on stop");
        assert!(dir.join("ran.marker").exists(), "the stand-in really ran");
        #[cfg(target_os = "linux")]
        assert!(process_gone(pid), "process {pid} still exists");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// KP-006 acceptance 4 (c): dropping a bridge stops it too.
    #[test]
    fn kp006_dof_drop_stops_the_bridge() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = private_tmp::create("kp006-dof-c");
        let script = stand_in(&dir);
        let cfg = config(&script, &dir);
        let bridge = DofBridge::start_for_table_on(true, &table(), &cfg)
            .unwrap()
            .unwrap();
        let map_file = bridge.map_file().to_path_buf();
        let pid = bridge.id();
        assert!(map_file.exists());
        drop(bridge);
        wait_until("the map file to be removed", || !map_file.exists());
        #[cfg(target_os = "linux")]
        wait_until("the process to be reaped", || process_gone(pid));
        let _ = pid;
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// KP-006 acceptance 4 (d), (e): unsupported platform or a table without
    /// `[dof]` spawns nothing.
    #[test]
    fn kp006_dof_unsupported_or_no_dof_spawns_nothing() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = private_tmp::create("kp006-dof-d");
        let script = stand_in(&dir);
        let cfg = config(&script, &dir);
        let r = DofBridge::start_for_table_on(false, &table(), &cfg).expect("no error");
        assert!(r.is_none());
        drop(r);
        let mut no_dof = table();
        no_dof.dof = None;
        let r = DofBridge::start_for_table_on(true, &no_dof, &cfg).expect("no error");
        assert!(r.is_none());
        drop(r);
        // A missing executable is not even looked at without [dof].
        let missing = config(&dir.join("does-not-exist"), &dir);
        assert!(
            DofBridge::start_for_table_on(true, &no_dof, &missing)
                .unwrap()
                .is_none()
        );
        assert!(
            DofBridge::start_for_table_on(false, &table(), &missing)
                .unwrap()
                .is_none()
        );
        assert!(!dir.join("ran.marker").exists(), "the script was run");
        let names: Vec<_> = std::fs::read_dir(&dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "nothing written but the script: {names:?}");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    /// KP-006 acceptance 4 (f): off Windows `start_for_table` is a no-op.
    #[test]
    fn kp006_dof_start_for_table_is_a_no_op_off_windows() {
        let _g = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
        let dir = private_tmp::create("kp006-dof-f");
        let script = stand_in(&dir);
        let cfg = config(&script, &dir);
        assert!(
            DofBridge::start_for_table(&table(), &cfg)
                .unwrap()
                .is_none()
        );
        assert!(!dir.join("ran.marker").exists());
        // `for_app` may or may not find a bridge next to the test binary; it
        // must not panic.
        let _ = DofLaunchConfig::for_app(35120);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
