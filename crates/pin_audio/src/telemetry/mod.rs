//! UDP telemetry (PLAN.md Appendix B; normative spec: `docs/TELEMETRY_PROTOCOL.md`).
//!
//! * [`TelemetryEncoder`] is the pure, allocation-free per-tick differ and
//!   encoder that runs on the sim thread: fed each tick's [`TelemetryState`]
//!   and haptic impulses it fills a [`DatagramBatch`] with 0..n encoded
//!   datagrams.
//! * [`telemetry_channel`] / [`TelemetrySender`] move the datagrams through
//!   a lock-free ring to a non-sim thread that owns the
//!   [`std::net::UdpSocket`].
//! * [`decode_datagram`] is the test/tool-side decoder (allocates a `Vec`).
//!
//! # Encoder rules
//! * **Diff.** An entry is emitted for every switch, coil, lamp, GI string
//!   and magnet whose value differs from the previous tick's (values are
//!   first normalised: booleans to 0/1, levels clamped to `0..=1` with NaN
//!   and −0 → 0; "differs" compares the normalised bits). Every impulse
//!   passed for the tick is emitted as a kind-5 entry
//!   ([`HapticsTelemetryEvent::to_telemetry_entry`]; the impulse's own
//!   `tick` field is ignored, the datagram carries the encoded tick), at most
//!   [`MAX_IMPULSES_PER_TICK`] (the rest count in
//!   [`TelemetryEncoder::dropped_impulses`]).
//! * **Order.** Switches, coils, lamps, GI, magnets (each by ascending id),
//!   then impulses in the order given.
//! * **No change, no datagram.** A tick with no entry produces no datagram.
//! * **Full state.** The first `encode_tick` after [`TelemetryEncoder::new`]
//!   or [`TelemetryEncoder::force_full_state`] (table load) and every tick at
//!   least [`FULL_STATE_INTERVAL_TICKS`] (1000) after the previous full state
//!   send every element (all 256 switches, 256 coils, 256 lamps,
//!   [`GI_COUNT`] GI strings, [`MAGNET_COUNT`] magnets) followed by the
//!   tick's impulses, with [`FLAG_FULL_STATE`] set on every datagram of that
//!   tick. A tick lower than the previous one (a restarted session) also
//!   sends the full state.
//! * **Split.** More than 180 entries split into several datagrams with the
//!   same tick and flags, in order (`pin_core::telemetry::for_each_datagram`
//!   semantics).

mod sender;

pub use sender::{
    DEFAULT_TELEMETRY_RING_CAPACITY, TelemetryReceiver, TelemetrySender, TelemetrySenderHandle,
    TelemetryTx, telemetry_channel,
};

pub use pin_core::input::HapticsTelemetryEvent;
pub use pin_core::telemetry::{
    DEFAULT_DESTINATION, FLAG_FULL_STATE, MAX_DATAGRAM_LEN, MAX_ENTRIES_PER_DATAGRAM,
    TelemetryEntry, TelemetryError, TelemetryHeader, TelemetryKind,
};

use arrayvec::ArrayVec;
use pin_core::telemetry::encode_datagram;

/// Switches in [`TelemetryState`] (ids 0..=255).
pub const SWITCH_COUNT: usize = 256;
/// Coils in [`TelemetryState`].
pub const COIL_COUNT: usize = 256;
/// Lamps in [`TelemetryState`].
pub const LAMP_COUNT: usize = 256;
/// GI strings in [`TelemetryState`] (`N` for `gi`).
pub const GI_COUNT: usize = 32;
/// Magnets in [`TelemetryState`] (`N` for `magnets`).
pub const MAGNET_COUNT: usize = 32;
/// Most impulses encoded per tick.
pub const MAX_IMPULSES_PER_TICK: usize = 64;
/// Ticks between periodic full-state datagrams (one second at 1 kHz).
pub const FULL_STATE_INTERVAL_TICKS: u64 = 1000;
/// Entries in a full state (without impulses): 832.
pub const FULL_STATE_ENTRIES: usize =
    SWITCH_COUNT + COIL_COUNT + LAMP_COUNT + GI_COUNT + MAGNET_COUNT;
/// Most entries one tick can produce.
pub const MAX_ENTRIES_PER_TICK: usize = FULL_STATE_ENTRIES + MAX_IMPULSES_PER_TICK;
/// Most datagrams one tick can produce (6).
pub const MAX_DATAGRAMS_PER_TICK: usize = MAX_ENTRIES_PER_TICK.div_ceil(MAX_ENTRIES_PER_DATAGRAM);

/// The cabinet state telemetry reports, one value per element.
#[derive(Clone, Debug, PartialEq)]
pub struct TelemetryState {
    /// Switch closed.
    pub switches: [bool; SWITCH_COUNT],
    /// Coil energised.
    pub coils: [bool; COIL_COUNT],
    /// Lamp intensity `0..=1`.
    pub lamps: [f32; LAMP_COUNT],
    /// GI string intensity `0..=1`.
    pub gi: [f32; GI_COUNT],
    /// Magnet power `0..=1`.
    pub magnets: [f32; MAGNET_COUNT],
}

impl Default for TelemetryState {
    fn default() -> Self {
        Self {
            switches: [false; SWITCH_COUNT],
            coils: [false; COIL_COUNT],
            lamps: [0.0; LAMP_COUNT],
            gi: [0.0; GI_COUNT],
            magnets: [0.0; MAGNET_COUNT],
        }
    }
}

impl TelemetryState {
    /// All off.
    pub fn new() -> Self {
        Self::default()
    }
}

/// One encoded datagram (fixed size, `Copy`, so it can travel through a
/// lock-free ring).
#[derive(Copy, Clone)]
pub struct Datagram {
    len: u16,
    bytes: [u8; MAX_DATAGRAM_LEN],
}

impl Datagram {
    const EMPTY: Datagram = Datagram {
        len: 0,
        bytes: [0; MAX_DATAGRAM_LEN],
    };

    /// The encoded bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.len)]
    }
}

impl core::fmt::Debug for Datagram {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Datagram").field("len", &self.len).finish()
    }
}

/// The datagrams of one tick: a fixed-capacity list
/// ([`MAX_DATAGRAMS_PER_TICK`]), each at most [`MAX_DATAGRAM_LEN`] bytes.
#[derive(Clone)]
pub struct DatagramBatch {
    len: usize,
    items: [Datagram; MAX_DATAGRAMS_PER_TICK],
}

impl Default for DatagramBatch {
    fn default() -> Self {
        Self::new()
    }
}

impl core::fmt::Debug for DatagramBatch {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_list().entries(self.iter().map(<[u8]>::len)).finish()
    }
}

impl DatagramBatch {
    /// Capacity in datagrams.
    pub const CAPACITY: usize = MAX_DATAGRAMS_PER_TICK;

    /// An empty batch.
    pub fn new() -> Self {
        Self {
            len: 0,
            items: [Datagram::EMPTY; MAX_DATAGRAMS_PER_TICK],
        }
    }

    /// Number of datagrams.
    pub fn len(&self) -> usize {
        self.len
    }

    /// True when there is no datagram.
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Removes every datagram.
    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Datagram `i`'s bytes.
    pub fn get(&self, i: usize) -> Option<&[u8]> {
        self.datagrams().get(i).map(Datagram::as_bytes)
    }

    /// The datagrams, in send order.
    pub fn datagrams(&self) -> &[Datagram] {
        &self.items[..self.len]
    }

    /// Iterates over the datagrams' bytes, in send order.
    pub fn iter(&self) -> impl Iterator<Item = &[u8]> + '_ {
        self.datagrams().iter().map(Datagram::as_bytes)
    }

    fn push_encoded(&mut self, tick: u64, flags: u8, entries: &[TelemetryEntry]) {
        if self.len == MAX_DATAGRAMS_PER_TICK {
            return;
        }
        let d = &mut self.items[self.len];
        // Cannot fail: ≤ 180 entries into a MAX_DATAGRAM_LEN buffer.
        if let Ok(n) = encode_datagram(tick, flags, entries, &mut d.bytes) {
            // n ≤ 1456.
            d.len = n as u16;
            self.len += 1;
        }
    }
}

impl<'a> IntoIterator for &'a DatagramBatch {
    type Item = &'a [u8];
    type IntoIter = core::iter::Map<core::slice::Iter<'a, Datagram>, fn(&Datagram) -> &[u8]>;

    fn into_iter(self) -> Self::IntoIter {
        self.datagrams().iter().map(Datagram::as_bytes)
    }
}

#[inline]
fn level(v: f32) -> f32 {
    if v > 0.0 { v.min(1.0) } else { 0.0 }
}

#[inline]
fn flag(b: bool) -> f32 {
    if b { 1.0 } else { 0.0 }
}

/// The per-tick telemetry differ/encoder (module docs for the rules). Pure
/// and allocation-free after construction.
pub struct TelemetryEncoder {
    prev: TelemetryState,
    entries: ArrayVec<TelemetryEntry, MAX_ENTRIES_PER_TICK>,
    full_pending: bool,
    last_full: u64,
    /// Last tick encoded (a lower tick means a new session: resync).
    last_tick: Option<u64>,
    dropped_impulses: u64,
}

impl Default for TelemetryEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl TelemetryEncoder {
    /// A new encoder; its first tick sends the full state.
    pub fn new() -> Self {
        Self {
            prev: TelemetryState::default(),
            entries: ArrayVec::new(),
            full_pending: true,
            last_full: 0,
            last_tick: None,
            dropped_impulses: 0,
        }
    }

    /// The next [`encode_tick`](Self::encode_tick) sends the full state
    /// (call on table load).
    pub fn force_full_state(&mut self) {
        self.full_pending = true;
    }

    /// Impulses beyond [`MAX_IMPULSES_PER_TICK`] that were not encoded.
    pub fn dropped_impulses(&self) -> u64 {
        self.dropped_impulses
    }

    /// Whether a full state is due at `tick`.
    fn full_due(&self, tick: u64) -> bool {
        self.full_pending
            || tick.saturating_sub(self.last_full) >= FULL_STATE_INTERVAL_TICKS
            || self.last_tick.is_some_and(|t| tick < t)
    }

    /// Diffs `state` against the previous tick and encodes this tick's
    /// datagrams into `out` (cleared first; left empty when nothing
    /// changed). Never allocates.
    pub fn encode_tick(
        &mut self,
        tick: u64,
        state: &TelemetryState,
        impulses: &[HapticsTelemetryEvent],
        out: &mut DatagramBatch,
    ) {
        out.clear();
        self.entries.clear();
        let full = self.full_due(tick);
        self.last_tick = Some(tick);
        let entries = &mut self.entries;
        let prev = &mut self.prev;
        {
            let mut add = |kind: TelemetryKind, id: usize, new: f32, old: &mut f32| {
                if full || new.to_bits() != old.to_bits() {
                    // id < 256: lossless.
                    entries.push(TelemetryEntry {
                        kind,
                        id: id as u16,
                        value: new,
                    });
                }
                *old = new;
            };
            for (i, (&n, o)) in state.switches.iter().zip(&mut prev.switches).enumerate() {
                let mut old = flag(*o);
                add(TelemetryKind::Switch, i, flag(n), &mut old);
                *o = n;
            }
            for (i, (&n, o)) in state.coils.iter().zip(&mut prev.coils).enumerate() {
                let mut old = flag(*o);
                add(TelemetryKind::Coil, i, flag(n), &mut old);
                *o = n;
            }
            for (i, (&n, o)) in state.lamps.iter().zip(&mut prev.lamps).enumerate() {
                add(TelemetryKind::Lamp, i, level(n), o);
            }
            for (i, (&n, o)) in state.gi.iter().zip(&mut prev.gi).enumerate() {
                add(TelemetryKind::Gi, i, level(n), o);
            }
            for (i, (&n, o)) in state.magnets.iter().zip(&mut prev.magnets).enumerate() {
                add(TelemetryKind::Magnet, i, level(n), o);
            }
        }
        for (i, imp) in impulses.iter().enumerate() {
            if i >= MAX_IMPULSES_PER_TICK {
                self.dropped_impulses += (impulses.len() - i) as u64;
                break;
            }
            self.entries.push(imp.to_telemetry_entry());
        }
        let flags = if full { FLAG_FULL_STATE } else { 0 };
        if full {
            self.full_pending = false;
            self.last_full = tick;
        }
        if self.entries.is_empty() && !full {
            return;
        }
        for chunk in self.entries.chunks(MAX_ENTRIES_PER_DATAGRAM) {
            out.push_encoded(tick, flags, chunk);
        }
    }
}

/// Decodes one datagram (test/tool side; allocates the entry list). Never
/// panics: bad magic, version, length or kind are errors.
pub fn decode_datagram(
    bytes: &[u8],
) -> Result<(TelemetryHeader, Vec<TelemetryEntry>), TelemetryError> {
    pin_core::telemetry::decode_datagram(bytes).map(|(h, e)| (h, e.to_vec()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_tick_is_full_then_diffs() {
        let mut enc = TelemetryEncoder::default();
        let mut st = TelemetryState::new();
        let mut out = DatagramBatch::default();
        enc.encode_tick(0, &st, &[], &mut out);
        assert_eq!(out.len(), FULL_STATE_ENTRIES.div_ceil(180));
        let mut n = 0;
        for d in &out {
            let (h, e) = decode_datagram(d).unwrap();
            assert!(h.is_full_state());
            n += e.len();
        }
        assert_eq!(n, FULL_STATE_ENTRIES);
        enc.encode_tick(1, &st, &[], &mut out);
        assert!(out.is_empty());
        st.lamps[3] = 2.0;
        st.gi[1] = f32::NAN;
        st.magnets[0] = 0.5;
        enc.encode_tick(2, &st, &[], &mut out);
        let (h, e) = decode_datagram(out.get(0).unwrap()).unwrap();
        assert_eq!((h.tick, h.flags), (2, 0));
        assert_eq!(e.len(), 2);
        assert_eq!((e[0].kind, e[0].id, e[0].value), (TelemetryKind::Lamp, 3, 1.0));
        assert_eq!(e[1].kind, TelemetryKind::Magnet);
        assert!(out.get(1).is_none());
        assert_eq!(format!("{out:?}"), "[32]");
        assert!(format!("{:?}", out.datagrams()[0]).contains("len"));
    }

    #[test]
    fn impulses_capped() {
        let mut enc = TelemetryEncoder::new();
        let st = TelemetryState::default();
        let mut out = DatagramBatch::new();
        enc.encode_tick(0, &st, &[], &mut out);
        let imp = [HapticsTelemetryEvent {
            tick: 0,
            source: 1,
            strength: 0.5,
        }; 70];
        enc.encode_tick(1, &st, &imp, &mut out);
        assert_eq!(enc.dropped_impulses(), 6);
        let (_, e) = decode_datagram(out.get(0).unwrap()).unwrap();
        assert_eq!(e.len(), MAX_IMPULSES_PER_TICK);
        // A tick going backwards (new session) resynchronises.
        enc.encode_tick(5, &st, &[], &mut out);
        assert!(out.is_empty());
        enc.encode_tick(4, &st, &[], &mut out);
        assert!(!out.is_empty());
        assert_eq!(MAX_DATAGRAMS_PER_TICK, DatagramBatch::CAPACITY);
    }
}
